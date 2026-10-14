//! Routing algorithm-parameter config.

use super::SortBy;

#[derive(Debug, Clone)]
pub struct RoutingConfig {
    pub sort_by: SortBy,
    pub plugin_args: String,
    /// Number of consumer workers the route will be cut across. `0`/`1` = one
    /// global route; `>1` = that many equal-size compact loops (see `segments`).
    pub workers: usize,
    /// Observed average seconds a worker spends per cluster; drives the
    /// predicted loop times in `Stats::route_plan`. `0` = [`DEFAULT_LOCATION_DELAY_SECS`].
    pub location_delay_secs: f64,
    /// Ceiling on CPU use, percent of all cores combined (1..=100). `0` =
    /// `KOJI_HOG_CPU`, else 90.
    pub hog_cpu: u8,
}

pub const DEFAULT_LOCATION_DELAY_SECS: f64 = 8.0;
