use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortBy {
    Unset,
    GeoHash,
    PointCount,
    Random,
    S2Cell,
    LatLon,
    /// Built-in TSP solver (replaces the external OR-tools `tsp` plugin).
    Tsp,
    /// Built-in TSP solver refining an S2-sorted seed order.
    TspHybrid,
    Custom(String),
}

impl<'de> Deserialize<'de> for SortBy {
    fn deserialize<D>(deserializer: D) -> Result<SortBy, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;

        match s.to_lowercase().as_str() {
            "geohash" => Ok(SortBy::GeoHash),
            "cluster_count" | "point_count" | "clustercount" | "pointcount" => {
                Ok(SortBy::PointCount)
            }
            "random" => Ok(SortBy::Random),
            "s2" | "s2cell" => Ok(SortBy::S2Cell),
            "latlon" => Ok(SortBy::LatLon),
            "" | "none" | "unset" => Ok(SortBy::Unset),
            // `tsp` used to be the external OR-tools plugin; it is now built in.
            "tsp" | "optimized" | "2opt" => Ok(SortBy::Tsp),
            "tsphybrid" | "hybrid" => Ok(SortBy::TspHybrid),
            _ => Ok(SortBy::Custom(s)),
        }
    }
}
