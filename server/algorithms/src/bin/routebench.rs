//! Clustering + routing benchmark: cluster a CSV of `lat,lon` points, route the
//! clusters, and report timing plus how well the route survives being cut into
//! `--workers` slices.
//!
//! Usage:
//!   routebench --csv points.csv [--mode fastest] [--radius 70] [--sort tsp]
//!              [--workers 15] [--cut 15] [--hog-cpu 90] [--location-delay 6.8]
//!
//! Output: a `PLAN` line (predicted worker loop times) and one `RESULT` line.
//! `slice_loop_m` is the summed closed-loop length of every slice cut at
//! `n*i/cut`; `slice_max_span_m` the largest slice bounding-box diagonal.

use std::time::Instant;

use algorithms::routing::{self, RouteOpts};
use algorithms::{clustering, stats::Stats};
use geojson::FeatureCollection;
use model::api::{
    Precision, calc_mode::CalculationMode, cluster_mode::ClusterMode, single_vec::SingleVec,
    sort_by::SortBy,
};
use serde::Deserialize;

fn arg(name: &str) -> Option<String> {
    let argv: Vec<String> = std::env::args().collect();
    argv.iter()
        .position(|a| a == name)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn parse<T: for<'de> Deserialize<'de>>(s: &str) -> T {
    serde_json::from_value(serde_json::Value::String(s.to_string())).expect("bad value")
}

fn load_csv(path: &str) -> SingleVec {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {path}: {e}"))
        .lines()
        .filter_map(|l| {
            let mut p = l.split(',');
            Some([
                p.next()?.trim().parse().ok()?,
                p.next()?.trim().parse().ok()?,
            ])
        })
        .collect()
}

fn hav(a: [Precision; 2], b: [Precision; 2]) -> Precision {
    let (la1, la2) = (a[0].to_radians(), b[0].to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2)
        + la1.cos() * la2.cos() * ((b[1] - a[1]).to_radians() / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * h.sqrt().asin()
}

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }
    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

fn main() {
    if std::env::var("BENCH_VERBOSE").is_ok() {
        let _ = log::set_logger(&LOGGER).map(|_| log::set_max_level(log::LevelFilter::Info));
    }
    let csv = arg("--csv").expect("--csv required");
    let radius: Precision = arg("--radius").map_or(70.0, |v| v.parse().unwrap());
    let workers: usize = arg("--workers").map_or(0, |v| v.parse().unwrap());
    let sort = arg("--sort").unwrap_or_else(|| "tsp".into());
    let mode = arg("--mode").unwrap_or_else(|| "fastest".into());
    let hog: u8 = arg("--hog-cpu").map_or(0, |v| v.parse().unwrap());
    let points = load_csv(&csv);

    let cluster_mode: ClusterMode = parse(&mode);
    let sort_by: SortBy = parse(&sort);
    let opts = RouteOpts {
        workers,
        location_delay_secs: arg("--location-delay").map_or(0.0, |v| v.parse().unwrap()),
    };

    let mut stats = Stats::new("routebench".into(), 1);
    let t = Instant::now();
    let clusters = algorithms::utils::with_thread_cap(hog, || {
        clustering::main(
            &points,
            cluster_mode,
            radius,
            1,
            &mut stats,
            0,
            usize::MAX,
            CalculationMode::Radius,
            15,
            9,
            FeatureCollection::default(),
            "",
            false,
            false,
        )
    });
    let cluster_s = t.elapsed().as_secs_f64();

    let t = Instant::now();
    let route = algorithms::utils::with_thread_cap(hog, || {
        routing::main(
            &points,
            clusters.clone(),
            &sort_by,
            0,
            radius,
            &mut stats,
            "",
            &opts,
        )
    });
    let route_s = t.elapsed().as_secs_f64();
    assert_eq!(route.len(), clusters.len(), "route must keep every cluster");

    let n = route.len();
    let w = arg("--cut")
        .map_or(workers, |v| v.parse().unwrap())
        .max(1);
    let (mut loop_sum, mut max_span, mut min_len, mut max_len) = (0.0, 0.0f64, usize::MAX, 0);
    for i in 0..w {
        let s = &route[n * i / w..n * (i + 1) / w];
        min_len = min_len.min(s.len());
        max_len = max_len.max(s.len());
        if s.len() < 2 {
            continue;
        }
        loop_sum += (0..s.len())
            .map(|j| hav(s[j], s[(j + 1) % s.len()]))
            .sum::<Precision>();
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
        println!(
            "PLAN workers={} delay_s={} cycle_s={:.0} ({:.1} min) total_worker_s={:.0}",
            plan.workers,
            plan.location_delay_secs,
            plan.cycle_secs,
            plan.cycle_secs / 60.0,
            plan.total_secs
        );
    }
    println!(
        "RESULT points={} clusters={n} radius={radius} mode={mode} sort={sort} workers={workers} cluster_s={cluster_s:.2} route_s={route_s:.2} global_loop_m={:.0} slice_loop_m={loop_sum:.0} slice_max_span_m={max_span:.0} slice_len={min_len}..{max_len}",
        points.len(),
        stats.total_distance,
    );
}
