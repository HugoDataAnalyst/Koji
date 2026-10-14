//! Routing algorithm-parameter config.

use super::SortBy;

#[derive(Debug, Clone)]
pub struct RoutingConfig {
    pub sort_by: SortBy,
    pub plugin_args: String,
    /// Number of consumer workers the route will be cut across. `0`/`1` = one
    /// global route; `>1` = that many equal-size compact loops (see `segments`).
    pub workers: usize,
}
