//! Per-request routing options that sit beside `sort_by`.

/// Seconds a worker is assumed to spend per location when predicting loop times.
pub const DEFAULT_LOCATION_DELAY_SECS: f64 = 8.0;

#[derive(Debug, Clone, Default)]
pub struct RouteOpts {
    /// Number of consumer workers the route will be cut across. `0`/`1` = one
    /// global route; `>1` = that many equal-size compact loops (see `segments`).
    pub workers: usize,
    /// Seconds a worker spends per location (fractions allowed). `0` = default (8).
    pub location_delay_secs: f64,
}

impl RouteOpts {
    /// The effective per-location delay: the request value if positive and
    /// finite, else [`DEFAULT_LOCATION_DELAY_SECS`].
    pub fn location_delay(&self) -> f64 {
        if self.location_delay_secs.is_finite() && self.location_delay_secs > 0.0 {
            self.location_delay_secs
        } else {
            DEFAULT_LOCATION_DELAY_SECS
        }
    }
}
