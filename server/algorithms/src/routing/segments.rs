//! Worker-aware routing.
//!
//! The controller that consumes a route cuts it into `workers` equal slices
//! (e.g. 1 500 points / 15 workers = 15 × 100) and hands one slice to each
//! worker, which then loops it. A single global tour is a poor input for that:
//! the slices can be long strings that criss-cross each other, and a worker
//! that is recycled drags its whole slice with it.
//!
//! This mode instead builds the route *for* the cut:
//!   1. balanced recursive bisection splits the points into `workers` compact
//!      regions whose sizes differ by at most one, so slice `i` is exactly the
//!      `i`-th region when the controller cuts at `n*i/workers`;
//!   2. every region is solved independently (small, single-threaded, in
//!      parallel on a bounded pool) as a closed loop, which is what a worker
//!      actually walks;
//!   3. regions are chained along a tour of their centroids, and each loop is
//!      rotated/oriented so consecutive slices join end-to-start cheaply.
//!
//! The result is a plain ordered list — same wire shape as any other sort.

use model::api::{Precision, point_array::PointArray, single_vec::SingleVec};
use rayon::prelude::*;

use model::api::sort_by::SortBy;
use super::tsp::{self, Budget};

const M_PER_DEG: Precision = 111_320.0;

/// Equirectangular frame (metres); the error is negligible at the extents a
/// single route covers, and it is only used for partitioning and joins.
struct Frame {
    lat0: Precision,
    kx: Precision,
}

impl Frame {
    fn new(points: &SingleVec) -> Self {
        let lat0 =
            points.iter().map(|p| p[0]).sum::<Precision>() / points.len().max(1) as Precision;
        Self {
            lat0,
            kx: lat0.to_radians().cos() * M_PER_DEG,
        }
    }

    fn xy(&self, p: PointArray) -> [Precision; 2] {
        [p[1] * self.kx, (p[0] - self.lat0) * M_PER_DEG]
    }
}

fn dist(a: [Precision; 2], b: [Precision; 2]) -> Precision {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Split `idx` into `k` groups of size ⌊n/k⌋ or ⌈n/k⌉ by recursive median cuts
/// along the longer side of each group's bounding box.
fn bisect(xy: &[[Precision; 2]], idx: &mut [u32], k: usize, out: &mut Vec<Vec<u32>>) {
    if k <= 1 || idx.len() <= 1 {
        out.push(idx.to_vec());
        return;
    }
    let (mut lo, mut hi) = ([Precision::MAX; 2], [Precision::MIN; 2]);
    for &i in idx.iter() {
        for a in 0..2 {
            lo[a] = lo[a].min(xy[i as usize][a]);
            hi[a] = hi[a].max(xy[i as usize][a]);
        }
    }
    let axis = if hi[0] - lo[0] >= hi[1] - lo[1] { 0 } else { 1 };
    let k1 = k / 2;
    let cut = (idx.len() * k1 / k).clamp(1, idx.len() - 1);
    idx.select_nth_unstable_by(cut, |&a, &b| {
        xy[a as usize][axis].total_cmp(&xy[b as usize][axis])
    });
    let (left, right) = idx.split_at_mut(cut);
    bisect(xy, left, k1, out);
    bisect(xy, right, k - k1, out);
}

fn centroid(points: &[PointArray]) -> PointArray {
    let n = points.len().max(1) as Precision;
    [
        points.iter().map(|p| p[0]).sum::<Precision>() / n,
        points.iter().map(|p| p[1]).sum::<Precision>() / n,
    ]
}

fn solve_part(part: SingleVec, sort_by: &SortBy) -> SingleVec {
    use super::sorting::{SortGeohash, SortLatLng, SortRandom, SortS2};
    let budget = Budget::segment(part.len());
    match sort_by {
        SortBy::TspHybrid => tsp::refine(part.sort_s2(), budget),
        SortBy::LatLon => part.sort_lat_lng(),
        SortBy::GeoHash => part.sort_geohash(),
        SortBy::S2Cell => part.sort_s2(),
        SortBy::Random => part.sort_random(),
        // tsp, point-count, unset and plugins all reduce to the TSP loop here:
        // a segment's whole point is a short loop.
        _ => tsp::solve(part, budget),
    }
}

/// Route `clusters` as `workers` contiguous, equally sized, compact loops.
/// Slice `i` of the result (cut at `n*i/workers`) is the `i`-th loop.
pub(super) fn route(clusters: SingleVec, workers: usize, sort_by: &SortBy) -> SingleVec {
    let n = clusters.len();
    let workers = workers.min(n);
    if workers < 2 {
        return clusters;
    }
    let frame = Frame::new(&clusters);
    let xy: Vec<[Precision; 2]> = clusters.iter().map(|p| frame.xy(*p)).collect();

    let mut idx: Vec<u32> = (0..n as u32).collect();
    let mut groups = Vec::with_capacity(workers);
    bisect(&xy, &mut idx, workers, &mut groups);

    let solve_all = || -> Vec<SingleVec> {
        groups
            .par_iter()
            .map(|g| solve_part(g.iter().map(|&i| clusters[i as usize]).collect(), sort_by))
            .collect()
    };
    let loops = match rayon::ThreadPoolBuilder::new()
        .num_threads(tsp::thread_cap())
        .build()
    {
        Ok(pool) => pool.install(solve_all),
        Err(_) => solve_all(),
    };

    // Chain the loops along a tour of their centroids.
    let centroids: SingleVec = loops.iter().map(|l| centroid(l)).collect();
    let order: Vec<usize> = if workers >= 3 {
        let seq = tsp::solve(centroids.clone(), Budget::segment(workers));
        // Map reordered centroids back to loop indices; each loop used once
        // even if two share an exact centroid.
        let mut used = vec![false; workers];
        seq.iter()
            .filter_map(|c| {
                let j = (0..workers).find(|&j| !used[j] && centroids[j] == *c)?;
                used[j] = true;
                Some(j)
            })
            .collect()
    } else {
        (0..workers).collect()
    };

    let mut route: SingleVec = Vec::with_capacity(n);
    let mut prev_end: Option<[Precision; 2]> = None;
    for (pos, &li) in order.iter().enumerate() {
        let lp = &loops[li];
        let m = lp.len();
        if m == 0 {
            continue;
        }
        let next_c = frame.xy(centroids[order[(pos + 1) % order.len()]]);
        // start: the loop node nearest to where the previous slice ended.
        let start = match prev_end {
            Some(e) => (0..m)
                .min_by(|&a, &b| dist(e, frame.xy(lp[a])).total_cmp(&dist(e, frame.xy(lp[b]))))
                .unwrap_or(0),
            None => 0,
        };
        // direction: finish the slice on the side facing the next slice.
        let end_fwd = frame.xy(lp[(start + m - 1) % m]);
        let end_bwd = frame.xy(lp[(start + 1) % m]);
        let forward = dist(end_fwd, next_c) <= dist(end_bwd, next_c);
        for s in 0..m {
            let i = if forward {
                (start + s) % m
            } else {
                (start + m - s) % m
            };
            route.push(lp[i]);
        }
        prev_end = route.last().map(|p| frame.xy(*p));
    }
    route
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(w: usize, h: usize) -> SingleVec {
        (0..w * h)
            .map(|i| {
                [
                    40.0 + (i / w) as Precision * 0.001,
                    -74.0 + (i % w) as Precision * 0.001,
                ]
            })
            .collect()
    }

    fn key(p: &PointArray) -> (i64, i64) {
        ((p[0] * 1e7).round() as i64, (p[1] * 1e7).round() as i64)
    }

    #[test]
    fn is_a_permutation() {
        let pts = grid(30, 20);
        let out = route(pts.clone(), 7, &SortBy::Tsp);
        let mut a: Vec<_> = pts.iter().map(key).collect();
        let mut b: Vec<_> = out.iter().map(key).collect();
        a.sort();
        b.sort();
        assert_eq!(a, b);
    }

    #[test]
    fn groups_are_balanced_to_within_one() {
        let pts = grid(31, 17);
        let frame = Frame::new(&pts);
        let xy: Vec<_> = pts.iter().map(|p| frame.xy(*p)).collect();
        let mut idx: Vec<u32> = (0..pts.len() as u32).collect();
        let mut g = vec![];
        bisect(&xy, &mut idx, 15, &mut g);
        assert_eq!(g.len(), 15);
        let (mn, mx) = g
            .iter()
            .fold((usize::MAX, 0), |(a, b), x| (a.min(x.len()), b.max(x.len())));
        assert!(mx - mn <= 1, "{mn}..{mx}");
    }

    #[test]
    fn slices_cut_at_n_i_over_w_are_compact() {
        let pts = grid(40, 40);
        let w = 16;
        let out = route(pts.clone(), w, &SortBy::Tsp);
        // each 100-point slice of a 40x40 grid must be a small block, not a
        // strip across the whole grid.
        for i in 0..w {
            let s = &out[pts.len() * i / w..pts.len() * (i + 1) / w];
            let span = |a: usize| {
                s.iter().map(|p| p[a]).fold(Precision::MIN, Precision::max)
                    - s.iter().map(|p| p[a]).fold(Precision::MAX, Precision::min)
            };
            assert!(
                span(0) <= 0.011 && span(1) <= 0.011,
                "slice {i} spans {} x {}",
                span(0),
                span(1)
            );
        }
    }

    #[test]
    fn degenerate_inputs_pass_through() {
        assert_eq!(route(vec![], 4, &SortBy::Tsp), Vec::<PointArray>::new());
        let one = vec![[1.0, 2.0]];
        assert_eq!(route(one.clone(), 4, &SortBy::Tsp), one);
        let three = grid(3, 1);
        assert_eq!(route(three.clone(), 1, &SortBy::Tsp), three);
        assert_eq!(route(three.clone(), 9, &SortBy::Tsp).len(), 3);
    }
}
