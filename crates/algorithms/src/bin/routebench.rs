//! Routing benchmark: cluster a CSV of points, route the clusters, and report
//! timing plus how well the route survives being cut into `--workers` slices.
//!
//! Usage:
//!   routebench --csv points.csv [--mode fastest] [--radius 70] [--sort tsp]
//!              [--workers 15] [--seed-clusters-csv clusters.csv]
//!
//! Output is one machine-greppable `RESULT` line. `slice_loop_m` is the summed
//! closed-loop length of every slice cut at `n*i/workers` (what the workers
//! walk); `slice_max_span_m` the largest slice bounding-box diagonal.

use web_time::Instant;

use algorithms::clustering::{CalculationMode, ClusterMode, ClusteringConfig, S2Config};
use algorithms::routing::{self, RoutingConfig, SortBy};
use algorithms::{clustering, stats::Stats};
use geojson::FeatureCollection;
use koji_core::{Precision, SingleVec};

fn arg(name: &str) -> Option<String> {
    let argv: Vec<String> = std::env::args().collect();
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1).cloned())
}

fn load_csv(path: &str) -> SingleVec {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {path}: {e}"))
        .lines()
        .filter_map(|l| {
            let mut p = l.split(',');
            Some([p.next()?.trim().parse().ok()?, p.next()?.trim().parse().ok()?])
        })
        .collect()
}

fn hav(a: [Precision; 2], b: [Precision; 2]) -> Precision {
    let (la1, la2) = (a[0].to_radians(), b[0].to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2)
        + la1.cos() * la2.cos() * ((b[1] - a[1]).to_radians() / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * h.sqrt().asin()
}

fn main() {
    let csv = arg("--csv").expect("--csv required");
    let radius: Precision = arg("--radius").map_or(70.0, |v| v.parse().unwrap());
    let workers: usize = arg("--workers").map_or(0, |v| v.parse().unwrap());
    let sort = arg("--sort").unwrap_or_else(|| "tsp".into());
    let mode = arg("--mode").unwrap_or_else(|| "fastest".into());
    let points = load_csv(&csv);

    let cluster_mode = ClusterMode::from_str_opt(&mode).expect("mode");
    let cfg = ClusteringConfig {
        mode: cluster_mode,
        radius,
        min_points: 1,
        max_clusters: usize::MAX,
        calculation_mode: CalculationMode::Radius,
        s2: S2Config { level: 15, size: 1 },
        center_clusters: false,
        plugin_args: String::new(),
    };
    let mut stats = Stats::new("routebench".into(), 1);
    let t = Instant::now();
    let clusters = clustering::main(&points, &cfg, FeatureCollection::default(), &mut stats);
    let cluster_s = t.elapsed().as_secs_f64();

    let rcfg = RoutingConfig {
        sort_by: SortBy::from_str_opt(&sort).expect("sort"),
        plugin_args: String::new(),
        workers,
        location_delay_secs: arg("--location-delay").map_or(0.0, |v| v.parse().unwrap()),
    };
    let t = Instant::now();
    let route = routing::main(&points, clusters.clone(), radius, &rcfg, &mut stats);
    let route_s = t.elapsed().as_secs_f64();
    assert_eq!(route.len(), clusters.len(), "route must keep every cluster");

    let n = route.len();
    let w = arg("--cut").map_or(workers, |v| v.parse().unwrap()).max(1);
    let (mut loop_sum, mut max_span, mut min_len, mut max_len) = (0.0, 0.0f64, usize::MAX, 0);
    for i in 0..w {
        let s = &route[n * i / w..n * (i + 1) / w];
        min_len = min_len.min(s.len());
        max_len = max_len.max(s.len());
        if s.len() < 2 {
            continue;
        }
        loop_sum += (0..s.len()).map(|j| hav(s[j], s[(j + 1) % s.len()])).sum::<Precision>();
        let (mut lo, mut hi) = ([Precision::MAX; 2], [Precision::MIN; 2]);
        for p in s {
            for a in 0..2 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        max_span = max_span.max(hav(lo, hi));
    }
    if let Some(plan) = &stats.route_plan {
        let loops: Vec<String> = plan.per_worker.iter().map(|w| format!("{:.0}", w.loop_secs)).collect();
        println!("PLAN workers={} delay_s={} cycle_s={:.0} ({:.1} min) total_worker_s={:.0} loops_s=[{}]", plan.workers, plan.location_delay_secs, plan.cycle_secs, plan.cycle_secs / 60.0, plan.total_secs, loops.join(","));
    }
    println!(
        "RESULT points={} clusters={n} radius={radius} sort={sort} workers={workers} cluster_s={cluster_s:.2} route_s={route_s:.2} global_loop_m={:.0} slice_loop_m={loop_sum:.0} slice_max_span_m={max_span:.0} slice_len={min_len}..{max_len}",
        points.len(),
        stats.total_distance,
    );
}
