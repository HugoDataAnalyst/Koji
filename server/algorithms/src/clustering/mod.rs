use std::{time::Instant, vec};

use crate::{
    plugin::{Folder, JoinFunction, Plugin},
    stats::Stats,
};

use self::greedy::Greedy;

use super::*;

use geojson::FeatureCollection;
use model::api::{Precision, single_vec::SingleVec};

pub use model::api::{calc_mode::CalculationMode, cluster_mode::ClusterMode};

mod candidates;
mod config;
mod crucible;
mod fastest;
/// Shared planar-geometry primitives (deterministic Welzl MEC, circle
/// intersections) used by both the crucible and fastest clusterers.
pub(crate) mod geometry;
mod greedy;
mod partition;
mod s2;
pub(crate) mod select;

pub use config::{ClusteringConfig, S2Config};

/// Warm-start entry point for incremental re-clustering (`Crucible::run_seeded`).
pub use crucible::Crucible;

/// Crucible for the quality modes, cap-aware: crucible's internal enforce ranks
/// reps at weight 1 per distinct cell (multiplicity-blind), so [`main`]'s
/// point-weighted selector owns the cap (crucible runs unbounded here).
///
/// `quick` splits the two quality tiers: Better runs the single-variant
/// crucible (a fraction of the cost, most of the quality), Best runs the full
/// restart portfolio + recombination. Both use this same cap-aware wrapper.
///
/// A CLEARLY binding cap (cheap greedy count > 5/4 · cap) resolves the cap
/// right here as a two-arm race instead of handing an unbounded pool to
/// [`main`]'s selector: a binding cap changes the objective to max-k-cover,
/// which the capped selector solves from the full arrangement space, so
/// crucible construction only seeds refined multi-point centers into one
/// arm's pool (at the tier's own cost: quick for Better, full for Best).
///
/// The two arms: greedy-only pool (bit-identical to what capped Balanced
/// produces) and greedy ∪ quick-crucible. Keeping whichever covers more makes
/// capped Better ≥ capped Balanced BY CONSTRUCTION — greedy max-coverage is
/// not monotonic in its candidate pool (extra pool centers can bait an early
/// commit that ends worse; observed on the urban synthetic: the unioned pool
/// covered 5 fewer than Balanced's), so the superset pool alone guarantees
/// nothing. Returning an already-capped solution means [`main`]'s cap step
/// sees len ≤ cap and is a no-op.
///
/// In the boundary band (greedy count ≤ 5/4 · cap) the full crucible runs —
/// its unbounded solution may genuinely fit the cap and then IS the result.
/// When it exceeds the cap instead, greedy is unioned into the selection
/// pool: crucible optimizes full-coverage-minimal-count, so its minimal tiling
/// has no slack and capping it drops whole neighborhoods; the union gives the
/// selector real choices. A finite but NON-binding cap must stay a no-op —
/// unioning unconditionally shipped the raw doubled pool when nothing pruned
/// it (caught by adversarial review: +39% mygod_score on a generous cap).
fn crucible_with_capped_pool(
    data_points: &SingleVec,
    cfg: &ClusteringConfig,
    quick: bool,
) -> SingleVec {
    let crucible = crucible::Crucible {
        radius: cfg.radius,
        min_points: cfg.min_points,
        max_clusters: usize::MAX,
        quick,
    };
    if cfg.max_clusters == usize::MAX {
        return crucible.run(data_points);
    }
    let mut greedy = Greedy::default();
    greedy
        .set_cluster_mode(ClusterMode::Balanced)
        .set_min_points(cfg.min_points)
        .set_radius(cfg.radius);
    let extra = greedy.run(data_points);
    if extra.len() > cfg.max_clusters.saturating_mul(5) / 4 {
        log::info!(
            "crucible: cap {} clearly binds ({} greedy centers) — two-arm capped race (quick: {quick})",
            cfg.max_clusters,
            extra.len()
        );
        let mut union_pool = crucible.run(data_points);
        union_pool.extend(extra.iter().copied());
        let arm_union =
            select::cap_radius_clusters(union_pool, data_points, cfg.radius, cfg.max_clusters);
        let arm_greedy =
            select::cap_radius_clusters(extra, data_points, cfg.radius, cfg.max_clusters);
        let cover_u = coverage_count(&arm_union, data_points, cfg.radius);
        let cover_g = coverage_count(&arm_greedy, data_points, cfg.radius);
        log::info!(
            "crucible: capped race — union pool covers {cover_u}, greedy pool covers {cover_g}"
        );
        // Prefer the union arm on ties (it carries crucible's refined centers).
        return if cover_g > cover_u {
            arm_greedy
        } else {
            arm_union
        };
    }
    let mut centers = crucible.run(data_points);
    if centers.len() > cfg.max_clusters {
        log::info!(
            "crucible: cap {} binds ({} centers) — unioning {} greedy centers into the selection pool",
            cfg.max_clusters,
            centers.len(),
            extra.len()
        );
        centers.extend(extra);
    }
    centers
}

/// Coverage as the scoreboard counts it, for judging the two-arm capped race:
/// distinct S2 L20 cells with at least one point within `radius` of a center
/// (`Stats::cluster_stats` collects covered points into a set keyed by the
/// L20 cell id, so `points_covered` is cell-granular).
fn coverage_count(centers: &SingleVec, points: &SingleVec, radius: Precision) -> usize {
    let tree = rtree::spawn(radius, points);
    let mut covered: hashbrown::HashSet<u64> = hashbrown::HashSet::new();
    for c in centers {
        covered.extend(tree.locate_all_at_point(*c).map(|p| p.cell_id.0));
    }
    covered.len()
}

fn run(
    data_points: &SingleVec,
    cfg: &ClusteringConfig,
    collection: FeatureCollection,
    stats: &mut Stats,
) -> SingleVec {
    if data_points.is_empty() {
        return vec![];
    }
    let time = Instant::now();
    let clusters = match cfg.calculation_mode {
        CalculationMode::S2 => collection
            .into_iter()
            .flat_map(|feature| {
                s2::cluster(
                    feature,
                    data_points,
                    cfg.s2.level,
                    cfg.s2.size,
                    cfg.min_points,
                )
            })
            .collect(),
        _ => match cfg.mode.clone() {
            ClusterMode::Fastest => fastest::main(data_points, cfg.radius, cfg.min_points),
            // The quality tiers share the cap-aware crucible wrapper and differ
            // only in construction depth: Better = quick single-variant, Best =
            // full restart portfolio + recombination.
            ClusterMode::Better => crucible_with_capped_pool(data_points, cfg, true),
            ClusterMode::Best => crucible_with_capped_pool(data_points, cfg, false),
            ClusterMode::Honeycomb | ClusterMode::Fast | ClusterMode::Balanced => {
                let mut greedy = Greedy::default();
                greedy
                    .set_cluster_mode(cfg.mode.clone())
                    .set_min_points(cfg.min_points)
                    .set_radius(cfg.radius);
                greedy.run(data_points)
            }
            ClusterMode::Custom(plugin) => {
                match Plugin::new(
                    &plugin,
                    Folder::Clustering,
                    cfg.cluster_split_level,
                    &cfg.plugin_args,
                ) {
                    Ok(plugin_manager) => {
                        match plugin_manager.run_multi::<JoinFunction>(data_points, None) {
                            Ok(sorted_clusters) => sorted_clusters,
                            Err(e) => {
                                log::error!("Error while running plugin: {}", e);
                                vec![]
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Plugin not found: {}", e);
                        vec![]
                    }
                }
            }
        },
    };
    // max_clusters enforcement — uniform across EVERY mode (greedy, crucible,
    // fastest, s2, plugins): keep the max-coverage subset, never an arbitrary
    // prefix or truncation. Algorithms run unbounded so the selector sees the
    // full candidate pool; an internal early-stop would lock in a worse subset.
    // Deliberate trade-off: a BINDING cap now pays the full unbounded
    // clustering cost (measured ~2–2.7× vs the old early-stop on large inputs)
    // in exchange for keeping the best clusters instead of the first ones.
    // Capped requests are the rare path; revisit only if it hurts in practice.
    let clusters = if clusters.len() > cfg.max_clusters {
        match cfg.calculation_mode {
            CalculationMode::S2 => select::cap_s2_clusters(
                clusters,
                data_points,
                cfg.s2.level,
                cfg.s2.size,
                cfg.max_clusters,
            ),
            _ => select::cap_radius_clusters(clusters, data_points, cfg.radius, cfg.max_clusters),
        }
    } else {
        clusters
    };

    let clusters = if cfg.center_clusters {
        sec::with_data(cfg.radius, data_points, &clusters)
    } else {
        clusters
    };

    stats.set_cluster_time(time);
    stats.cluster_stats(cfg.radius, data_points, &clusters);
    stats.set_score();

    clusters
}

pub fn clustering_plugins() -> Vec<String> {
    utils::get_plugin_list("algorithms/src/clustering/plugins").unwrap_or(vec![])
}

pub fn all_clustering_options() -> Vec<String> {
    let mut options = clustering_plugins();
    options.push("honeycomb".to_string());
    options.push("fastest".to_string());
    options.push("balanced".to_string());
    options.push("fast".to_string());
    options.push("better".to_string());
    options.push("best".to_string());
    options
}

/// Cluster `data_points`. Keeps `main`'s long-standing signature; the work is done
/// by [`run`] (the crucible-based clusterer).
#[allow(clippy::too_many_arguments)]
pub fn main(
    data_points: &SingleVec,
    cluster_mode: ClusterMode,
    radius: f64,
    min_points: usize,
    stats: &mut Stats,
    cluster_split_level: u64,
    max_clusters: usize,
    calculation_mode: CalculationMode,
    s2_level: u8,
    s2_size: u8,
    collection: FeatureCollection,
    clustering_args: &str,
    center_clusters: bool,
    _genetic_post_processing: bool,
) -> SingleVec {
    let cfg = ClusteringConfig {
        mode: cluster_mode,
        radius,
        min_points,
        max_clusters,
        calculation_mode,
        s2: S2Config {
            level: s2_level,
            size: s2_size,
        },
        center_clusters,
        plugin_args: clustering_args.to_string(),
        cluster_split_level,
    };
    run(data_points, &cfg, collection, stats)
}
