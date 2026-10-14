use std::time::Instant;

use model::api::{single_vec::SingleVec, sort_by::SortBy};

use self::sorting::{SortGeohash, SortLatLng, SortPointCount, SortRandom, SortS2};
use crate::{
    plugin::{Folder, Plugin},
    stats::Stats,
    utils,
};

mod join;
mod opts;
mod segments;
pub mod sorting;
mod tsp;
// pub mod vrp;

pub use opts::{DEFAULT_LOCATION_DELAY_SECS, RouteOpts};

pub fn main(
    data_points: &SingleVec,
    clusters: SingleVec,
    sort_by: &SortBy,
    route_split_level: u64,
    radius: f64,
    stats: &mut Stats,
    routing_args: &str,
    opts: &RouteOpts,
) -> SingleVec {
    let route_time = Instant::now();
    // Worker-aware mode: the consumer cuts the route into `workers` equal
    // slices, so build it as that many compact loops. The slice boundaries are
    // meaningful, so the global "rotate to best" below must not shift them.
    if opts.workers > 1 && clusters.len() >= 2 * opts.workers {
        let clusters = segments::route(clusters, opts.workers, sort_by);
        stats.set_route_time(route_time);
        stats.distance_stats(&clusters);
        stats.plan_route(&clusters, opts.workers, opts.location_delay());
        return clusters;
    }
    let clusters = match sort_by {
        SortBy::PointCount => clusters.sort_point_count(&data_points, radius),
        SortBy::LatLon => clusters.sort_lat_lng(),
        SortBy::GeoHash => clusters.sort_geohash(),
        SortBy::S2Cell => clusters.sort_s2(),
        SortBy::Random => clusters.sort_random(),
        SortBy::Tsp => {
            let budget = tsp::Budget::global(clusters.len());
            tsp::solve(clusters, budget)
        }
        SortBy::TspHybrid => {
            let budget = tsp::Budget::global(clusters.len());
            tsp::refine(clusters.sort_s2(), budget)
        }
        SortBy::Unset => clusters,
        SortBy::Custom(plugin) => {
            let clusters = clusters.sort_s2();
            match Plugin::new(plugin, Folder::Routing, route_split_level, routing_args) {
                Ok(plugin_manager) => match plugin_manager.run_multi(&clusters, Some(join::join)) {
                    Ok(sorted_clusters) => sorted_clusters,
                    Err(e) => {
                        log::error!("Error while running plugin: {}", e);
                        clusters
                    }
                },
                Err(e) => {
                    log::error!("Plugin not found: {}", e);
                    clusters
                }
            }
        }
    };
    let clusters = utils::rotate_to_best(clusters, stats);

    stats.set_route_time(route_time);
    stats.distance_stats(&clusters);
    stats.plan_route(&clusters, opts.workers, opts.location_delay());

    clusters
}

pub fn routing_plugins() -> Vec<String> {
    utils::get_plugin_list("algorithms/src/routing/plugins").unwrap_or(vec![])
}

pub fn all_routing_options() -> Vec<String> {
    let mut options = routing_plugins();
    options.push("point_count".to_string());
    options.push("latlon".to_string());
    options.push("geohash".to_string());
    options.push("s2".to_string());
    options.push("random".to_string());
    options.push("tsp".to_string());
    options.push("tsphybrid".to_string());
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(n: usize) -> SingleVec {
        (0..n)
            .map(|i| [40.0 + (i / 10) as f64 * 0.001, -74.0 + (i % 10) as f64 * 0.001])
            .collect()
    }

    #[test]
    fn tsp_keeps_every_cluster() {
        let mut stats = Stats::new("t".into(), 1);
        let out = main(&vec![], pts(50), &SortBy::Tsp, 0, 70.0, &mut stats, "", &RouteOpts::default());
        assert_eq!(out.len(), 50);
        assert!(stats.total_distance > 0.0);
    }

    #[test]
    fn workers_mode_builds_equal_slices_and_plan() {
        let mut stats = Stats::new("t".into(), 1);
        let opts = RouteOpts { workers: 5, location_delay_secs: 6.8 };
        let out = main(&vec![], pts(100), &SortBy::Tsp, 0, 70.0, &mut stats, "", &opts);
        assert_eq!(out.len(), 100);
        let plan = stats.route_plan.expect("plan");
        assert_eq!(plan.workers, 5);
        assert!(plan.per_worker.iter().all(|w| w.clusters == 20));
        assert!((plan.cycle_secs - 20.0 * 6.8).abs() < 1e-9);
    }

    #[test]
    fn plan_defaults_to_8s_per_location() {
        let mut stats = Stats::new("t".into(), 1);
        main(&vec![], pts(10), &SortBy::LatLon, 0, 70.0, &mut stats, "", &RouteOpts::default());
        let plan = stats.route_plan.expect("plan");
        assert_eq!(plan.location_delay_secs, 8.0);
        assert_eq!(plan.cycle_secs, 80.0);
    }

    #[test]
    fn too_few_clusters_fall_back_to_one_route() {
        let mut stats = Stats::new("t".into(), 1);
        let opts = RouteOpts { workers: 15, location_delay_secs: 0.0 };
        let out = main(&vec![], pts(10), &SortBy::Tsp, 0, 70.0, &mut stats, "", &opts);
        assert_eq!(out.len(), 10);
    }
}
