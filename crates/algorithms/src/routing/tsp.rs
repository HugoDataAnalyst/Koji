//! tsp-mt adapter: route cluster centers with the tsp-geo ILS solver.
//! `solve` = standalone (`SortBy::Tsp`, the Route default); `refine` =
//! improve an already-sorted (S2-seeded) order (`SortBy::TspHybrid`).
//!
//! No wire-exposed knobs: fixed defaults except `max_neighbors` 10 → 20 so
//! sparse/gappy areas (islands, rivers) keep cross-gap candidate edges.
//! Note the wall-clock budget (auto 2–120 s from n) makes results
//! machine-dependent across runs even with the fixed seed.

use koji_core::SingleVec;
use tsp_geo::{GeoPoint, SolverConfig};

use super::sorting::sort_s2;

/// Solver resource limits. The solver default (`threads: 0` = every core,
/// wall-clock budget `n/1000` s up to 120 s) saturates the machine for results
/// that stop improving long before the budget ends, so the default here caps
/// both: threads at `KOJI_TSP_THREADS` (else min(cores, 4)) and the budget at
/// 0.2 ms/point, at most `KOJI_TSP_MAX_SECS` (else 20 s).
#[derive(Debug, Clone, Copy)]
pub(super) struct Budget {
    pub threads: usize,
    pub time_limit: Option<std::time::Duration>,
}

fn env_num<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok()?.trim().parse().ok()
}

/// Max solver threads: `KOJI_TSP_THREADS`, else min(cores, 4).
pub(super) fn thread_cap() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |c| c.get());
    env_num::<usize>("KOJI_TSP_THREADS")
        .filter(|t| *t > 0)
        .unwrap_or(cores.min(4))
}

impl Budget {
    /// Budget for one standalone solve over `n` points.
    pub fn global(n: usize) -> Self {
        let threads = thread_cap();
        let max_secs = env_num::<f64>("KOJI_TSP_MAX_SECS").unwrap_or(20.0).max(0.1);
        // The ILS is within ~0.1% of its 20 s result after ~0.1 ms/point (measured
        // on 5k Porto clusters), so spend 0.2 ms/point rather than the solver's
        // 1 ms/point default.
        let secs = (n as f64 * 0.0002).clamp(0.3, max_secs);
        Self {
            threads,
            time_limit: Some(std::time::Duration::from_secs_f64(secs)),
        }
    }

    /// Budget for one small single-threaded sub-solve (a worker segment).
    pub fn segment(n: usize) -> Self {
        let per_pt = env_num::<f64>("KOJI_SEG_MS_PER_POINT").unwrap_or(1.0);
        let ms = ((n as f64 * per_pt) as u64).clamp(10, 1_500);
        Self {
            threads: 1,
            time_limit: Some(std::time::Duration::from_millis(ms)),
        }
    }
}

fn config(budget: Budget) -> SolverConfig {
    let mut cfg = SolverConfig::default();
    cfg.max_neighbors = 20;
    cfg.threads = budget.threads;
    cfg.time_limit = budget.time_limit;
    cfg
}

fn geo_points(clusters: &SingleVec) -> Vec<GeoPoint> {
    clusters
        .iter()
        .map(|c| GeoPoint::from_lat_lng(c[0], c[1]))
        .collect()
}

fn reorder(clusters: SingleVec, order: Vec<u32>) -> SingleVec {
    order.into_iter().map(|i| clusters[i as usize]).collect()
}

/// Standalone tsp-mt: greedy construction + ILS over the raw clusters.
/// Solver rejection (out-of-range or non-finite coords) logs and falls back to the S2 sort.
pub(super) fn solve(clusters: SingleVec, budget: Budget) -> SingleVec {
    if clusters.len() < 2 {
        return clusters;
    }
    match tsp_geo::solve_order(&geo_points(&clusters), &config(budget)) {
        Ok(order) => reorder(clusters, order),
        Err(e) => {
            log::error!("tsp solve failed: {e}; falling back to s2 sort");
            sort_s2(clusters)
        }
    }
}

/// Hybrid: refine an already-S2-sorted order (identity seed tour).
/// Solver rejection logs and keeps the seed order.
pub(super) fn refine(clusters: SingleVec, budget: Budget) -> SingleVec {
    if clusters.len() < 2 {
        return clusters;
    }
    let initial: Vec<u32> = (0..clusters.len() as u32).collect();
    // `initial` is a freshly-built identity tour of the correct length, so
    // `refine_order`'s permutation-rejection Err arm is dead by construction here;
    // the only reachable Err is coord validation, which the fallback below handles.
    match tsp_geo::refine_order(&geo_points(&clusters), &initial, &config(budget)) {
        Ok(order) => reorder(clusters, order),
        Err(e) => {
            log::error!("tsp refine failed: {e}; keeping s2 seed order");
            clusters
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koji_core::Precision;

    fn sorted_xy(order: &SingleVec) -> Vec<(i64, i64)> {
        let mut s: Vec<(i64, i64)> = order
            .iter()
            .map(|p| ((p[0] * 1e6).round() as i64, (p[1] * 1e6).round() as i64))
            .collect();
        s.sort();
        s
    }

    /// Cyclic haversine-ish tour length in degrees-space chord (adequate for
    /// relative comparisons in tests).
    fn tour_len(order: &SingleVec) -> Precision {
        let n = order.len();
        if n < 2 {
            return 0.0;
        }
        (0..n)
            .map(|i| {
                let (a, b) = (order[i], order[(i + 1) % n]);
                ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
            })
            .sum()
    }

    fn bowtie() -> SingleVec {
        vec![[0.0, 0.0], [0.01, 0.01], [0.0, 0.01], [0.01, 0.0]]
    }

    #[test]
    fn solve_is_permutation_and_uncrosses() {
        let crossed = bowtie();
        let out = solve(crossed.clone(), Budget::segment(4));
        assert_eq!(sorted_xy(&out), sorted_xy(&crossed), "same multiset");
        assert!(tour_len(&out) < tour_len(&crossed) - 1e-9, "must shorten");
    }

    #[test]
    fn refine_is_permutation_and_never_worsens() {
        let crossed = bowtie();
        let out = refine(crossed.clone(), Budget::segment(4));
        assert_eq!(sorted_xy(&out), sorted_xy(&crossed), "same multiset");
        assert!(tour_len(&out) <= tour_len(&crossed) + 1e-12);
    }

    #[test]
    fn tiny_inputs_pass_through() {
        for clusters in [SingleVec::new(), vec![[1.0, 2.0]]] {
            assert_eq!(solve(clusters.clone(), Budget::segment(1)), clusters);
            assert_eq!(refine(clusters.clone(), Budget::segment(1)), clusters);
        }
    }
}
