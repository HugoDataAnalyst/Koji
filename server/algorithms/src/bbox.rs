//! Minimal lat/lon bounding box used by the clusterers (ported from koji-core).

use model::api::Precision;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KojiBbox {
    pub min_lat: Precision,
    pub min_lon: Precision,
    pub max_lat: Precision,
    pub max_lon: Precision,
}

impl KojiBbox {
    /// Build from `[lat, lon]` points; `None` for an empty set.
    pub fn from_points(points: &[[Precision; 2]]) -> Option<Self> {
        if points.is_empty() {
            return None;
        }
        let mut b = Self {
            min_lat: Precision::INFINITY,
            min_lon: Precision::INFINITY,
            max_lat: Precision::NEG_INFINITY,
            max_lon: Precision::NEG_INFINITY,
        };
        for &[lat, lon] in points {
            b.min_lat = b.min_lat.min(lat);
            b.max_lat = b.max_lat.max(lat);
            b.min_lon = b.min_lon.min(lon);
            b.max_lon = b.max_lon.max(lon);
        }
        Some(b)
    }

    /// Grow the box outward by `deg` degrees on every side.
    pub fn expand(self, deg: Precision) -> Self {
        Self {
            min_lat: self.min_lat - deg,
            min_lon: self.min_lon - deg,
            max_lat: self.max_lat + deg,
            max_lon: self.max_lon + deg,
        }
    }

    /// Round every edge to `decimals` places (6 = the geojson/SQL output convention).
    pub fn trim(self, decimals: u32) -> Self {
        let f = 10f64.powi(decimals as i32);
        let r = |v: Precision| (v * f).round() / f;
        Self {
            min_lat: r(self.min_lat),
            min_lon: r(self.min_lon),
            max_lat: r(self.max_lat),
            max_lon: r(self.max_lon),
        }
    }

    /// GeoJSON-standard bbox array `[min_lon, min_lat, max_lon, max_lat]`.
    pub fn to_geojson_bbox(self) -> [Precision; 4] {
        [self.min_lon, self.min_lat, self.max_lon, self.max_lat]
    }

    /// GeoJSON-standard bbox as a `Vec<Precision>`.
    pub fn to_geojson_bbox_vec(self) -> Vec<Precision> {
        self.to_geojson_bbox().to_vec()
    }

    /// Latitude midpoint.
    pub fn center_lat(&self) -> Precision {
        0.5 * (self.min_lat + self.max_lat)
    }

    /// Longitude midpoint.
    pub fn center_lon(&self) -> Precision {
        0.5 * (self.min_lon + self.max_lon)
    }
}
