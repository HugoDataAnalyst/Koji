//! Legacy `/api/v1` compatibility layer.
//!
//! The v2 stack replaced the flat, snake_case `Args` body and the
//! `{message,status,status_code,data,stats}` envelope of `main`'s `/api/v1`.
//! Existing integrations still speak v1, so this module re-exposes the same
//! paths and request/response shapes as a thin translation over the v2
//! compute paths — nothing is re-implemented:
//!
//! - **calc** (`/calc/...`): the flat legacy body is translated into a v2
//!   [`CalcRequest`](crate::requests::CalcRequest) JSON ([`legacy_calc_body`]),
//!   resolved with [`build_calc_payload`] and computed with [`compute_sync`]
//!   (the same code `POST /api/v2/jobs {sync:true}` runs), then the result is
//!   re-shaped to the requested legacy `return_type` inside the v1 envelope.
//! - **convert** (`/convert/...`), **s2** (`/s2/...`), `/health`, `/info`: same
//!   bodies as v1, backed by the v2 geometry / S2 helpers.
//! - **geofence / route reads** (`/geofence/{all,reference,area,{rt}[/{project}]}`,
//!   `/route/{all,area,{rt}}`, `DELETE /geofence/{id}`): backed by the v2 `koji-db`
//!   reads. The write/push endpoints (`save-koji`, `save-scanner`, `push`) answer
//!   `501` pointing at their `/api/v2` replacement.
//!
//! Auth is the same `public_validator` as `/api/v2`. The new routing options
//! (`workers`, `location_delay`, `hog_cpu`) are accepted here too.
//!
//! Not carried over: the persistence flags (`save_to_db`, `save_to_scanner`,
//! `save_to_scanner_only`) — v2's calc is pure compute — so a request that sets
//! one is rejected with 400 rather than silently ignored. `route_split_level` /
//! `cluster_split_level` are accepted and ignored (v2 no longer has them).
//! `area` accepts GeoJSON (FeatureCollection / Feature / Geometry) or an
//! `instance` / `parent` name; the legacy bare-array/text area shapes are not
//! supported.

use actix_web::{HttpResponse, ResponseError, delete, get, http::StatusCode, post, web};
use geojson::{Feature, FeatureCollection, GeometryValue};
use koji_core::{Precision, UnknownId};
use koji_db::KojiDb;
use koji_jobs::JobQueue;
use serde_json::{Map, Value, json};

use crate::public::v2::calc::CalcPayload;
use crate::public::v2::jobs::{build_calc_payload, compute_sync};
use crate::requests::{CalcRequest, ReturnTypeArg, area_collection, get_return_type};
use crate::utils::error::ServiceError;
use crate::utils::response::response_body;

// ── envelope ────────────────────────────────────────────────────────────────

/// v1 success envelope.
fn v1_ok(data: Value, stats: Option<Value>) -> HttpResponse {
    HttpResponse::Ok().json(json!({
        "message": "Success",
        "status": "ok",
        "status_code": 200,
        "data": data,
        "stats": stats,
    }))
}

/// v1 error envelope (`Response::send_error` shape).
fn v1_err(status: StatusCode, message: &str) -> HttpResponse {
    HttpResponse::build(status).json(json!({
        "message": message,
        "status": "error",
        "status_code": status.as_u16(),
        "data": null,
        "stats": null,
    }))
}

fn service_err(e: ServiceError) -> HttpResponse {
    let status = e.status_code();
    let message = match &e {
        ServiceError::Invalid { message, .. } | ServiceError::Unprocessable { message, .. } => {
            message.clone()
        }
        _ if status == StatusCode::NOT_FOUND => "not_found".to_string(),
        _ => "internal_error".to_string(),
    };
    log::error!("[v1] {e}");
    v1_err(status, &message)
}

// ── legacy Args → v2 request ────────────────────────────────────────────────

/// First non-null value under any of `keys` (legacy snake_case, or camelCase).
fn pick<'a>(m: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .filter_map(|k| m.get(*k))
        .find(|v| !v.is_null())
}

fn set(out: &mut Map<String, Value>, key: &str, v: Option<&Value>) {
    if let Some(v) = v {
        out.insert(key.to_string(), v.clone());
    }
}

/// Lowercase an enum-ish string (`"Balanced"` -> `"balanced"`) so the legacy
/// PascalCase spellings hit the v2 case-sensitive parsers. Non-strings pass
/// through untouched.
fn lowered(v: Option<&Value>) -> Option<Value> {
    v.map(|v| match v.as_str() {
        Some(s) => Value::String(s.to_lowercase()),
        None => v.clone(),
    })
}

/// Names v2's `SortBy` knows (case-insensitively). Anything else is a routing
/// plugin name and keeps its case.
const SORT_NAMES: &[&str] = &[
    "unset", "none", "geohash", "pointcount", "point_count", "cluster_count", "clustercount",
    "random", "s2cell", "s2", "latlon", "tsp", "optimized", "2opt", "tsphybrid", "hybrid",
];

fn sort_by_value(v: Option<&Value>) -> Option<Value> {
    let v = v?;
    match v.as_str() {
        Some(s) if SORT_NAMES.contains(&s.to_lowercase().as_str()) => {
            Some(Value::String(s.to_lowercase()))
        }
        _ => Some(v.clone()),
    }
}

fn obj(entries: Vec<(&str, Option<Value>)>) -> Value {
    Value::Object(
        entries
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| (k.to_string(), v)))
            .collect(),
    )
}

/// The persistence flags v2's pure-compute calc cannot honor.
fn unsupported_save(m: &Map<String, Value>) -> Option<&'static str> {
    for key in ["save_to_db", "save_to_scanner", "save_to_scanner_only"] {
        if m.get(key).and_then(Value::as_bool).unwrap_or(false) {
            return Some(match key {
                "save_to_db" => "save_to_db is not supported on /api/v1; use /api/v2/routes",
                "save_to_scanner" => "save_to_scanner is not supported on /api/v1",
                _ => "save_to_scanner_only is not supported on /api/v1",
            });
        }
    }
    None
}

fn routing_group(m: &Map<String, Value>, force_tsp: bool) -> Value {
    let mut sort_by = sort_by_value(pick(m, &["sort_by", "sortBy"]));
    if force_tsp && sort_by.as_ref().is_none_or(|v| v == "unset" || v == "") {
        sort_by = Some(json!("tsp"));
    }
    obj(vec![
        ("sortBy", sort_by),
        ("pluginArgs", pick(m, &["routing_args", "routingArgs"]).cloned()),
        ("workers", pick(m, &["workers", "segments"]).cloned()),
        (
            "locationDelay",
            pick(m, &["location_delay", "locationDelay", "jump_secs", "jumpSecs"]).cloned(),
        ),
        ("hogCpu", pick(m, &["hog_cpu", "hogCpu"]).cloned()),
    ])
}

fn output_group(m: &Map<String, Value>) -> Value {
    obj(vec![
        ("returnType", pick(m, &["return_type", "returnType"]).cloned()),
        ("simplify", pick(m, &["simplify"]).cloned()),
    ])
}

fn dev_group(m: &Map<String, Value>) -> Value {
    obj(vec![(
        "benchmarkMode",
        pick(m, &["benchmark_mode", "benchmarkMode"]).cloned(),
    )])
}

/// Translate a flat legacy `Args` body into the tagged v2 `CalcRequest` JSON for
/// `op` (`cluster` | `route` | `bootstrap` | `reroute` | `routeStats`).
pub(crate) fn legacy_calc_body(op: &str, m: &Map<String, Value>) -> Value {
    let mut out = Map::new();
    out.insert("mode".into(), json!(op));
    match op {
        "cluster" | "route" => {
            set(&mut out, "area", pick(m, &["area"]));
            set(&mut out, "dataPoints", pick(m, &["data_points", "dataPoints"]));
            set(&mut out, "parent", pick(m, &["parent"]));
            set(&mut out, "instance", pick(m, &["instance"]));
            let mode = lowered(pick(m, &["cluster_mode", "clusterMode"])).or_else(|| {
                pick(m, &["fast"])
                    .and_then(Value::as_bool)
                    .map(|f| json!(if f { "fastest" } else { "balanced" }))
            });
            // legacy `max_clusters: 0` meant "unlimited"
            let max_clusters = pick(m, &["max_clusters", "maxClusters"])
                .filter(|v| v.as_u64() != Some(0))
                .cloned();
            out.insert(
                "clustering".into(),
                obj(vec![
                    ("radius", pick(m, &["radius"]).cloned()),
                    ("minPoints", pick(m, &["min_points", "minPoints"]).cloned()),
                    ("maxClusters", max_clusters),
                    ("mode", mode),
                    (
                        "calculationMode",
                        lowered(pick(m, &["calculation_mode", "calculationMode"])),
                    ),
                    ("s2Level", pick(m, &["s2_level", "s2Level"]).cloned()),
                    ("s2Size", pick(m, &["s2_size", "s2Size"]).cloned()),
                    (
                        "centerClusters",
                        pick(m, &["center_clusters", "centerClusters"]).cloned(),
                    ),
                    (
                        "pluginArgs",
                        pick(m, &["clustering_args", "clusteringArgs"]).cloned(),
                    ),
                ]),
            );
            out.insert("routing".into(), routing_group(m, op == "route"));
            out.insert("output".into(), output_group(m));
            out.insert("dev".into(), dev_group(m));
            out.insert(
                "dataFilter".into(),
                obj(vec![
                    ("lastSeen", pick(m, &["last_seen", "lastSeen"]).cloned()),
                    ("tth", pick(m, &["tth"]).cloned()),
                ]),
            );
        }
        "bootstrap" => {
            set(&mut out, "area", pick(m, &["area"]));
            set(&mut out, "parent", pick(m, &["parent"]));
            set(&mut out, "instance", pick(m, &["instance"]));
            out.insert(
                "bootstrap".into(),
                obj(vec![
                    (
                        "calculationMode",
                        lowered(pick(m, &["calculation_mode", "calculationMode"])),
                    ),
                    ("radius", pick(m, &["radius"]).cloned()),
                    ("s2Level", pick(m, &["s2_level", "s2Level"]).cloned()),
                    ("s2Size", pick(m, &["s2_size", "s2Size"]).cloned()),
                    (
                        "pluginArgs",
                        pick(m, &["bootstrapping_args", "bootstrappingArgs"]).cloned(),
                    ),
                ]),
            );
            out.insert("routing".into(), routing_group(m, false));
            out.insert("output".into(), output_group(m));
            out.insert("dev".into(), dev_group(m));
        }
        "reroute" | "routeStats" => {
            set(&mut out, "dataPoints", pick(m, &["data_points", "dataPoints"]));
            set(&mut out, "clusters", pick(m, &["clusters"]));
            set(&mut out, "radius", pick(m, &["radius"]));
            set(&mut out, "instance", pick(m, &["instance"]));
            if op == "reroute" {
                out.insert("routing".into(), routing_group(m, false));
            } else {
                set(&mut out, "minPoints", pick(m, &["min_points", "minPoints"]));
            }
            out.insert("output".into(), output_group(m));
            out.insert("dev".into(), dev_group(m));
        }
        _ => {}
    }
    Value::Object(out)
}

fn args_map(body: Value) -> Result<Map<String, Value>, HttpResponse> {
    match body {
        Value::Object(m) => Ok(m),
        Value::Null => Ok(Map::new()),
        _ => Err(v1_err(StatusCode::BAD_REQUEST, "body must be a JSON object")),
    }
}

// ── result shaping ──────────────────────────────────────────────────────────

/// The v1 `return_type` for a legacy body: the explicit `return_type` if given,
/// else the default implied by the `area` container shape (none -> SingleArray).
fn legacy_return_type(m: &Map<String, Value>) -> ReturnTypeArg {
    let default_rt = match pick(m, &["area"]).and_then(|a| a.get("type")).and_then(Value::as_str)
    {
        Some("FeatureCollection") => ReturnTypeArg::FeatureCollection,
        Some("Feature") => ReturnTypeArg::Feature,
        Some(_) => ReturnTypeArg::Geometry,
        None => ReturnTypeArg::SingleArray,
    };
    match pick(m, &["return_type", "returnType"]).and_then(Value::as_str) {
        Some(s) => get_return_type(s, &default_rt),
        None => default_rt,
    }
}

/// Re-shape a v2 `{ data: <FeatureCollection|null>, stats }` result into the v1
/// envelope with `data` rendered as `return_type`.
fn shape_result(result: Value, rt: ReturnTypeArg) -> HttpResponse {
    let stats = result.get("stats").cloned();
    let data = match result.get("data") {
        None | Some(Value::Null) => Value::Null,
        Some(fc) => {
            let fc: FeatureCollection = match serde_json::from_value(fc.clone()) {
                Ok(fc) => fc,
                Err(e) => {
                    log::error!("[v1] result is not a FeatureCollection: {e}");
                    return v1_err(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
                }
            };
            match koji_core::KojiGeometryCollection::try_from(fc) {
                Ok(coll) => response_body(&coll, rt),
                Err(e) => {
                    log::error!("[v1] result conversion failed: {e}");
                    return v1_err(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
                }
            }
        }
    };
    v1_ok(data, stats)
}

/// Common tail for the calc endpoints that go through `build_calc_payload`.
async fn run_calc(
    conn: &KojiDb,
    jobs: &JobQueue,
    op: &'static str,
    category: String,
    body: Value,
    rt: ReturnTypeArg,
) -> HttpResponse {
    let req: CalcRequest = match serde_json::from_value(body) {
        Ok(r) => r,
        Err(e) => return v1_err(StatusCode::BAD_REQUEST, &format!("invalid request: {e}")),
    };
    let payload = match build_calc_payload(conn, op, category, &req).await {
        Ok(p) => p,
        Err(e) => return service_err(e),
    };
    match compute_sync(jobs, payload).await {
        Ok(result) => shape_result(result, rt),
        Err(e) => service_err(e),
    }
}

// ── /calc ───────────────────────────────────────────────────────────────────

#[post("/bootstrap")]
async fn bootstrap(
    conn: web::Data<KojiDb>,
    jobs: web::Data<JobQueue>,
    payload: web::Json<Value>,
) -> HttpResponse {
    let m = match args_map(payload.into_inner()) {
        Ok(m) => m,
        Err(r) => return r,
    };
    if let Some(msg) = unsupported_save(&m) {
        return v1_err(StatusCode::BAD_REQUEST, msg);
    }
    let body = legacy_calc_body("bootstrap", &m);
    run_calc(&conn, &jobs, "bootstrap", "pokestop".into(), body, legacy_return_type(&m)).await
}

#[post("/reroute")]
async fn reroute(
    conn: web::Data<KojiDb>,
    jobs: web::Data<JobQueue>,
    payload: web::Json<Value>,
) -> HttpResponse {
    let m = match args_map(payload.into_inner()) {
        Ok(m) => m,
        Err(r) => return r,
    };
    let body = legacy_calc_body("reroute", &m);
    run_calc(&conn, &jobs, "reroute", "pokestop".into(), body, legacy_return_type(&m)).await
}

/// Score `clusters` against `data_points` (benchmark/stats-only result, as v1).
async fn route_stats_inner(
    jobs: &JobQueue,
    m: &Map<String, Value>,
    payload_fn: impl FnOnce(&Map<String, Value>) -> Result<CalcPayload, HttpResponse>,
) -> HttpResponse {
    let payload = match payload_fn(m) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match compute_sync(jobs, payload).await {
        Ok(result) => shape_result(result, ReturnTypeArg::Feature),
        Err(e) => service_err(e),
    }
}

fn stats_request(m: &Map<String, Value>) -> Value {
    let mut body = legacy_calc_body("routeStats", m);
    // v1 route-stats always returned stats only.
    body["dev"] = json!({ "benchmarkMode": true });
    body
}

fn stats_payload(
    m: &Map<String, Value>,
    area: FeatureCollection,
    data_points: koji_core::SingleVec,
    category: &str,
) -> Result<CalcPayload, HttpResponse> {
    let req = stats_request(m);
    let parsed: CalcRequest = serde_json::from_value(req.clone())
        .map_err(|e| v1_err(StatusCode::BAD_REQUEST, &format!("invalid request: {e}")))?;
    let inputs = parsed.enqueue_inputs();
    if inputs.clusters.is_empty() && data_points.is_empty() {
        return Err(v1_err(
            StatusCode::BAD_REQUEST,
            "no_clusters_or_data_points_found",
        ));
    }
    Ok(CalcPayload {
        mode: "routeStats".into(),
        category: category.to_string(),
        request: req,
        area,
        data_points,
        clusters: inputs.clusters,
    })
}

#[post("/route-stats")]
async fn route_stats(jobs: web::Data<JobQueue>, payload: web::Json<Value>) -> HttpResponse {
    let m = match args_map(payload.into_inner()) {
        Ok(m) => m,
        Err(r) => return r,
    };
    route_stats_inner(&jobs, &m, |m| {
        let req: CalcRequest = serde_json::from_value(stats_request(m))
            .map_err(|e| v1_err(StatusCode::BAD_REQUEST, &format!("invalid request: {e}")))?;
        let points = req.enqueue_inputs().data_points;
        stats_payload(m, FeatureCollection::default(), points, "pokestop")
    })
    .await
}

#[post("/route-stats/{category}")]
async fn route_stats_category(
    conn: web::Data<KojiDb>,
    jobs: web::Data<JobQueue>,
    url: web::Path<String>,
    payload: web::Json<Value>,
) -> HttpResponse {
    use koji_golbat::GenericDataToVec;
    let category = url.into_inner();
    let m = match args_map(payload.into_inner()) {
        Ok(m) => m,
        Err(r) => return r,
    };
    let req: CalcRequest = match serde_json::from_value(stats_request(&m)) {
        Ok(r) => r,
        Err(e) => return v1_err(StatusCode::BAD_REQUEST, &format!("invalid request: {e}")),
    };
    let supplied = req.enqueue_inputs().data_points;

    // The category-aware variant also takes an area/instance/parent to fetch the
    // golbat points inside it, when no data_points were supplied.
    let area_val = pick(&m, &["area"]).cloned();
    let area_in: Option<crate::requests::GeoInput> =
        area_val.and_then(|v| serde_json::from_value(v).ok());
    let instance = pick(&m, &["instance"])
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let parent: Option<UnknownId> =
        pick(&m, &["parent"]).and_then(|v| serde_json::from_value(v.clone()).ok());
    let last_seen = pick(&m, &["last_seen", "lastSeen"])
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let tth = pick(&m, &["tth"])
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or(koji_core::SpawnpointTth::All);

    let area = match crate::utils::create_or_find_collection(
        &instance,
        &conn,
        area_collection(&area_in),
        &parent,
        &supplied,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => return service_err(e.into()),
    };
    let points = if !supplied.is_empty() {
        supplied
    } else {
        match crate::utils::points_from_area(&area, &category, &conn, last_seen, tth).await {
            Ok(p) => p.to_single_vec(),
            Err(e) => return service_err(e.into()),
        }
    };
    route_stats_inner(&jobs, &m, |m| stats_payload(m, area, points, &category)).await
}

#[post("/area")]
async fn calculate_area(payload: web::Json<Value>) -> HttpResponse {
    let m = match args_map(payload.into_inner()) {
        Ok(m) => m,
        Err(r) => return r,
    };
    let area: Option<crate::requests::GeoInput> =
        pick(&m, &["area"]).and_then(|v| serde_json::from_value(v.clone()).ok());
    v1_ok(json!({ "area": total_area(&area_collection(&area)) }), None)
}

fn total_area(fc: &FeatureCollection) -> Precision {
    use geo::{ChamberlainDuquetteArea, MultiPolygon, Polygon};
    fc.features
        .iter()
        .filter_map(|f| f.geometry.as_ref())
        .map(|g| match &g.value {
            GeometryValue::MultiPolygon { .. } => MultiPolygon::<f64>::try_from(g)
                .map(|mp| mp.chamberlain_duquette_unsigned_area())
                .unwrap_or(0.0),
            GeometryValue::Polygon { .. } => Polygon::<f64>::try_from(g)
                .map(|p| p.chamberlain_duquette_unsigned_area())
                .unwrap_or(0.0),
            _ => 0.0,
        })
        .sum()
}

/// `POST /calc/{mode}/{category}` — `mode` is `cluster` or `route`.
#[post("/{mode}/{category}")]
async fn cluster(
    conn: web::Data<KojiDb>,
    jobs: web::Data<JobQueue>,
    url: web::Path<(String, String)>,
    payload: web::Json<Value>,
) -> HttpResponse {
    let (mode, category) = url.into_inner();
    let op: &'static str = match mode.as_str() {
        "cluster" => "cluster",
        "route" => "route",
        _ => return v1_err(StatusCode::NOT_FOUND, "unknown_calc_mode"),
    };
    let m = match args_map(payload.into_inner()) {
        Ok(m) => m,
        Err(r) => return r,
    };
    if let Some(msg) = unsupported_save(&m) {
        return v1_err(StatusCode::BAD_REQUEST, msg);
    }
    let body = legacy_calc_body(op, &m);
    run_calc(&conn, &jobs, op, category, body, legacy_return_type(&m)).await
}

// ── /convert ────────────────────────────────────────────────────────────────

/// Resolve a legacy convert body to its area collection + return type.
fn convert_inputs(
    m: &Map<String, Value>,
) -> Result<(FeatureCollection, ReturnTypeArg, bool, bool), HttpResponse> {
    let area_in: Option<crate::requests::GeoInput> = match pick(m, &["area"]) {
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| {
            v1_err(
                StatusCode::BAD_REQUEST,
                &format!("area must be GeoJSON on /api/v1: {e}"),
            )
        })?),
        None => None,
    };
    let default_rt = match &area_in {
        Some(crate::requests::GeoInput::FeatureCollection(_)) => ReturnTypeArg::FeatureCollection,
        Some(crate::requests::GeoInput::Feature(_)) => ReturnTypeArg::Feature,
        Some(crate::requests::GeoInput::Geometry(_)) => ReturnTypeArg::Geometry,
        None => ReturnTypeArg::SingleArray,
    };
    let rt = match pick(m, &["return_type", "returnType"]).and_then(Value::as_str) {
        Some(s) => get_return_type(s, &default_rt),
        None => default_rt,
    };
    let do_simplify = pick(m, &["simplify"]).and_then(Value::as_bool).unwrap_or(false);
    let benchmark = pick(m, &["benchmark_mode", "benchmarkMode"])
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok((area_collection(&area_in), rt, do_simplify, benchmark))
}

fn respond_collection(fc: FeatureCollection, rt: ReturnTypeArg) -> HttpResponse {
    match koji_core::KojiGeometryCollection::try_from(fc) {
        Ok(coll) => v1_ok(response_body(&coll, rt), None),
        Err(e) => service_err(ServiceError::internal(e)),
    }
}

#[post("/data")]
async fn convert_data(payload: web::Json<Value>) -> HttpResponse {
    use koji_core::{FeatureHelpers, GeometryHelpers, TrimPrecision};
    let m = match args_map(payload.into_inner()) {
        Ok(m) => m,
        Err(r) => return r,
    };
    let (area, rt, do_simplify, benchmark) = match convert_inputs(&m) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let area = if do_simplify { area.simplify() } else { area }
        .into_iter()
        .map(|feat| feat.remove_internal_props())
        .collect::<FeatureCollection>()
        .trim_precision(6);
    if benchmark {
        return v1_ok(Value::Null, None);
    }
    respond_collection(area, rt)
}

#[post("/simplify")]
async fn simplify(payload: web::Json<Value>) -> HttpResponse {
    use koji_core::GeometryHelpers;
    let m = match args_map(payload.into_inner()) {
        Ok(m) => m,
        Err(r) => return r,
    };
    let (area, rt, _, _) = match convert_inputs(&m) {
        Ok(v) => v,
        Err(r) => return r,
    };
    respond_collection(area.simplify(), rt)
}

#[post("/merge-points")]
async fn merge_points(payload: web::Json<Value>) -> HttpResponse {
    let m = match args_map(payload.into_inner()) {
        Ok(m) => m,
        Err(r) => return r,
    };
    let (area, rt, _, _) = match convert_inputs(&m) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let points: Vec<geojson::Position> = area
        .features
        .into_iter()
        .filter_map(|f| f.geometry)
        .filter_map(|g| match g.value {
            GeometryValue::Point { coordinates } => Some(coordinates),
            _ => None,
        })
        .collect();
    let feature = Feature {
        geometry: Some(geojson::Geometry::new(GeometryValue::MultiPoint {
            coordinates: points,
        })),
        ..Default::default()
    };
    respond_collection(
        FeatureCollection {
            bbox: None,
            features: vec![feature],
            foreign_members: None,
        },
        rt,
    )
}

// ── /s2 ─────────────────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
struct CoverageArgs {
    lat: Precision,
    lon: Precision,
    radius: Option<Precision>,
    size: Option<u8>,
    level: u8,
}

#[post("/circle-coverage")]
async fn circle_coverage(payload: web::Json<CoverageArgs>) -> HttpResponse {
    let a = payload.into_inner();
    v1_ok(
        json!(koji_core::s2::circle_coverage(
            a.lat,
            a.lon,
            a.radius.unwrap_or(70.),
            a.level
        )),
        None,
    )
}

#[post("/cell-coverage")]
async fn cell_coverage(payload: web::Json<CoverageArgs>) -> HttpResponse {
    let a = payload.into_inner();
    let cells: Vec<String> = koji_core::s2::cell_coverage(a.lat, a.lon, a.size.unwrap_or(15), a.level)
        .iter()
        .map(|c| c.to_string())
        .collect();
    v1_ok(json!(cells), None)
}

#[post("/polygons")]
async fn cell_polygons(payload: web::Json<Vec<String>>) -> HttpResponse {
    v1_ok(json!(koji_core::s2::get_polygons(payload.into_inner())), None)
}

#[post("/{cell_level}")]
async fn s2_cells(payload: web::Json<koji_core::BoundsArg>, url: web::Path<u8>) -> HttpResponse {
    use std::collections::HashSet;
    let mut bounds = payload.into_inner();
    let ids: Option<HashSet<String>> = bounds.ids.take().map(|i| i.into_iter().collect());
    let cells = koji_core::s2::get_cells(
        url.into_inner(),
        bounds.bbox.min_lat,
        bounds.bbox.min_lon,
        bounds.bbox.max_lat,
        bounds.bbox.max_lon,
    );
    let cells: Vec<_> = match ids {
        None => cells,
        Some(ids) => cells.into_iter().filter(|c| ids.contains(&c.id)).collect(),
    };
    v1_ok(json!(cells), None)
}

// ── /geofence, /route, /project ─────────────────────────────────────────────

use koji_db::db::{geofence, route};
use koji_db::query_args::ApiQueryArgs;

fn model_err(e: koji_db::ModelError) -> HttpResponse {
    service_err(ServiceError::from(e))
}

fn not_supported(what: &str, instead: &str) -> HttpResponse {
    v1_err(
        StatusCode::NOT_IMPLEMENTED,
        &format!("{what} is not available on /api/v1; use {instead}"),
    )
}

/// `rt` from the path/`?rt=` -> the v1 default for a FeatureCollection read.
fn read_rt(name: &str) -> ReturnTypeArg {
    get_return_type(name, &ReturnTypeArg::FeatureCollection)
}

fn shaped(coll: koji_core::KojiGeometryCollection, rt: ReturnTypeArg) -> HttpResponse {
    v1_ok(response_body(&coll, rt), None)
}

#[get("/all")]
async fn geofence_all(conn: web::Data<KojiDb>) -> HttpResponse {
    match geofence::Query::get_all_koji(&conn.koji).await {
        Ok(c) => shaped(c, ReturnTypeArg::FeatureCollection),
        Err(e) => model_err(e),
    }
}

#[get("/reference")]
async fn geofence_reference(conn: web::Data<KojiDb>) -> HttpResponse {
    match geofence::Query::get_all(&conn.koji).await {
        Ok(rows) => v1_ok(
            json!(
                rows.into_iter()
                    .map(|m| json!({"id": m.id, "name": m.name, "mode": m.mode, "parent": m.parent}))
                    .collect::<Vec<_>>()
            ),
            None,
        ),
        Err(e) => service_err(e.into()),
    }
}

#[get("/reference/{project}")]
async fn geofence_reference_project(
    conn: web::Data<KojiDb>,
    project: web::Path<String>,
) -> HttpResponse {
    match geofence::Query::by_project(&conn.koji, project.into_inner()).await {
        Ok(rows) => v1_ok(json!(rows), None),
        Err(e) => service_err(e.into()),
    }
}

#[get("/area/{geofence}")]
async fn geofence_area(
    conn: web::Data<KojiDb>,
    id: web::Path<String>,
    args: web::Query<ApiQueryArgs>,
) -> HttpResponse {
    let rt = read_rt(args.rt.as_deref().unwrap_or("feature"));
    match geofence::Query::get_one_koji(&conn.koji, id.into_inner()).await {
        Ok(kg) => shaped(koji_core::KojiGeometryCollection::new(vec![kg]), rt),
        Err(e) => model_err(e),
    }
}

#[delete("/{id}")]
async fn geofence_remove(conn: web::Data<KojiDb>, id: web::Path<u32>) -> HttpResponse {
    match geofence::Query::delete(&conn.koji, id.into_inner()).await {
        Ok(r) => v1_ok(json!(r.rows_affected), None),
        Err(e) => service_err(e.into()),
    }
}

#[post("/save-koji")]
async fn geofence_save_koji() -> HttpResponse {
    not_supported("geofence save-koji", "POST /api/v2/geofences")
}

#[post("/save-scanner")]
async fn geofence_save_scanner() -> HttpResponse {
    not_supported("geofence save-scanner", "POST /api/v2/geofences/{id}/publish")
}

#[get("/push/{id}")]
async fn geofence_push() -> HttpResponse {
    not_supported("geofence push", "POST /api/v2/geofences/{id}/publish")
}

#[get("/{return_type}")]
async fn geofence_return_type(
    conn: web::Data<KojiDb>,
    rt: web::Path<String>,
) -> HttpResponse {
    match geofence::Query::get_all_koji(&conn.koji).await {
        Ok(c) => shaped(c, read_rt(&rt.into_inner())),
        Err(e) => model_err(e),
    }
}

#[get("/{return_type}/{project}")]
async fn geofence_project(
    conn: web::Data<KojiDb>,
    url: web::Path<(String, String)>,
    args: web::Query<ApiQueryArgs>,
) -> HttpResponse {
    let (rt, project) = url.into_inner();
    match geofence::Query::project_as_koji(&conn.koji, project, &args).await {
        Ok(c) => shaped(c, read_rt(&rt)),
        Err(e) => model_err(e),
    }
}

#[get("/all")]
async fn route_all(conn: web::Data<KojiDb>) -> HttpResponse {
    match route::Query::as_koji_collection(&conn.koji).await {
        Ok(c) => shaped(c, ReturnTypeArg::FeatureCollection),
        Err(e) => model_err(e),
    }
}

#[get("/area/{id}")]
async fn route_area(
    conn: web::Data<KojiDb>,
    id: web::Path<String>,
    args: web::Query<ApiQueryArgs>,
) -> HttpResponse {
    let rt = read_rt(args.rt.as_deref().unwrap_or("feature"));
    match route::Query::get_one_koji(&conn.koji, id.into_inner()).await {
        Ok(kg) => shaped(koji_core::KojiGeometryCollection::new(vec![kg]), rt),
        Err(e) => model_err(e),
    }
}

#[post("/save-koji")]
async fn route_save_koji() -> HttpResponse {
    not_supported("route save-koji", "POST /api/v2/routes")
}

#[get("/push/{id}")]
async fn route_push() -> HttpResponse {
    not_supported("route push", "the /api/v2/routes publish endpoints")
}

#[get("/reference")]
async fn route_reference() -> HttpResponse {
    not_supported("route reference", "GET /api/v2/routes")
}

#[get("/{return_type}")]
async fn route_return_type(conn: web::Data<KojiDb>, rt: web::Path<String>) -> HttpResponse {
    match route::Query::as_koji_collection(&conn.koji).await {
        Ok(c) => shaped(c, read_rt(&rt.into_inner())),
        Err(e) => model_err(e),
    }
}

#[get("/push/{id}")]
async fn project_push() -> HttpResponse {
    not_supported("project push", "the /api/v2/projects endpoints")
}

// ── /health, /info ──────────────────────────────────────────────────────────

#[get("/health")]
async fn health() -> HttpResponse {
    HttpResponse::Ok().finish()
}

#[get("/")]
async fn info() -> HttpResponse {
    use ::algorithms::{bootstrap, clustering, routing};
    HttpResponse::Ok().json(json!({
        "routing": routing::all_routing_options(),
        "clustering": clustering::all_clustering_options(),
        "bootstrap": bootstrap::all_bootstrap_options(),
    }))
}

/// The `/api/v1` scope (mounted under `/api` behind `public_validator`).
pub(crate) fn scope() -> actix_web::Scope {
    web::scope("/v1")
        .service(health)
        .service(
            web::scope("/calc")
                .service(bootstrap)
                .service(route_stats)
                .service(route_stats_category)
                .service(reroute)
                .service(calculate_area)
                .service(cluster),
        )
        .service(
            web::scope("/convert")
                .service(convert_data)
                .service(merge_points)
                .service(simplify),
        )
        .service(
            web::scope("/s2")
                .service(circle_coverage)
                .service(cell_coverage)
                .service(cell_polygons)
                .service(s2_cells),
        )
        .service(
            web::scope("/geofence")
                .service(geofence_all)
                .service(geofence_reference)
                .service(geofence_reference_project)
                .service(geofence_save_koji)
                .service(geofence_save_scanner)
                .service(geofence_remove)
                .service(geofence_push)
                .service(geofence_area)
                .service(geofence_return_type)
                .service(geofence_project),
        )
        .service(
            web::scope("/route")
                .service(route_all)
                .service(route_reference)
                .service(route_save_koji)
                .service(route_push)
                .service(route_area)
                .service(route_return_type),
        )
        .service(web::scope("/project").service(project_push))
        .service(web::scope("/info").service(info))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(op: &str, v: Value) -> Value {
        legacy_calc_body(op, v.as_object().unwrap())
    }

    #[test]
    fn cluster_args_map_to_v2_groups() {
        let v = body(
            "cluster",
            json!({
                "radius": 70, "min_points": 3, "cluster_mode": "Balanced",
                "sort_by": "TspHybrid", "max_clusters": 0, "return_type": "SingleArray",
                "last_seen": 100, "tth": "Known", "instance": "x",
                "area": {"type":"FeatureCollection","features":[]},
                "routing_args": "--a 1", "workers": 15, "location_delay": 6.8
            }),
        );
        assert_eq!(v["mode"], "cluster");
        assert_eq!(v["clustering"]["minPoints"], 3);
        assert_eq!(v["clustering"]["mode"], "balanced");
        assert!(v["clustering"].get("maxClusters").is_none(), "0 = unlimited");
        assert_eq!(v["routing"]["sortBy"], "tsphybrid");
        assert_eq!(v["routing"]["pluginArgs"], "--a 1");
        assert_eq!(v["routing"]["workers"], 15);
        assert_eq!(v["routing"]["locationDelay"], 6.8);
        assert_eq!(v["output"]["returnType"], "SingleArray");
        assert_eq!(v["dataFilter"]["lastSeen"], 100);
        assert_eq!(v["dataFilter"]["tth"], "Known");
        assert_eq!(v["instance"], "x");
        // and it is a valid v2 request
        let req: CalcRequest = serde_json::from_value(v).unwrap();
        assert!(matches!(req, CalcRequest::Cluster(_)));
    }

    #[test]
    fn route_defaults_to_tsp_and_fast_maps_to_mode() {
        let v = body("route", json!({"fast": true}));
        assert_eq!(v["routing"]["sortBy"], "tsp");
        assert_eq!(v["clustering"]["mode"], "fastest");
        let v = body("cluster", json!({"fast": false}));
        assert_eq!(v["clustering"]["mode"], "balanced");
        assert!(v["routing"].get("sortBy").is_none());
    }

    #[test]
    fn plugin_sort_names_keep_their_case() {
        let v = body("cluster", json!({"sort_by": "MyPlugin"}));
        assert_eq!(v["routing"]["sortBy"], "MyPlugin");
    }

    #[test]
    fn reroute_and_stats_bodies_parse_as_v2() {
        let pts = json!([[40.0, -74.0], [40.1, -74.0]]);
        let v = body("reroute", json!({"clusters": pts, "radius": 70, "sort_by": "Tsp"}));
        assert!(matches!(
            serde_json::from_value::<CalcRequest>(v).unwrap(),
            CalcRequest::Reroute(_)
        ));
        let v = body("routeStats", json!({"clusters": pts, "data_points": pts, "min_points": 2}));
        assert!(matches!(
            serde_json::from_value::<CalcRequest>(v).unwrap(),
            CalcRequest::RouteStats(_)
        ));
    }

    #[test]
    fn bootstrap_body_parses_as_v2() {
        let v = body("bootstrap", json!({"radius": 500, "calculation_mode": "S2", "s2_level": 15}));
        assert_eq!(v["bootstrap"]["calculationMode"], "s2");
        assert!(matches!(
            serde_json::from_value::<CalcRequest>(v).unwrap(),
            CalcRequest::Bootstrap(_)
        ));
    }

    #[test]
    fn save_flags_are_rejected_not_ignored() {
        let m = json!({"save_to_db": true});
        assert!(unsupported_save(m.as_object().unwrap()).is_some());
        let m = json!({"save_to_db": false});
        assert!(unsupported_save(m.as_object().unwrap()).is_none());
    }

    #[test]
    fn envelope_shapes() {
        let ok = v1_ok(json!([1]), None);
        assert_eq!(ok.status(), StatusCode::OK);
        let e = v1_err(StatusCode::BAD_REQUEST, "x");
        assert_eq!(e.status(), StatusCode::BAD_REQUEST);
    }
}
