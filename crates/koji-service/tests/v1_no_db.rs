//! No-DB tests for the legacy `/api/v1` compatibility layer: the endpoints that
//! do not touch the database (`/health`, `/info`, `/s2/*`, `/convert/*`,
//! `/calc/area`) with the original v1 request bodies and `{message, status,
//! status_code, data, stats}` envelope. The calc endpoints that resolve areas or
//! golbat data need a DB and are covered by the end-to-end run.

use actix_web::{App, test, web};
use serde_json::{Value, json};

macro_rules! app {
    () => {{
        test::init_service(
            App::new()
                .app_data(web::JsonConfig::default().limit(1024 * 1024 * 10))
                .service(web::scope("/api").service(koji_service::v1_scope())),
        )
        .await
    }};
}

macro_rules! call {
    ($app:expr, $req:expr) => {{
        let resp = test::call_service(&$app, $req).await;
        let status = resp.status().as_u16();
        let bytes = test::read_body(resp).await;
        (status, serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null))
    }};
}

fn polygon_fc() -> Value {
    json!({
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature", "properties": {},
            "geometry": {"type": "Polygon",
                "coordinates": [[[0.0,0.0],[0.01,0.0],[0.01,0.01],[0.0,0.0]]]}
        }]
    })
}

#[actix_web::test]
async fn health_is_200() {
    let app = app!();
    let resp = test::call_service(&app, test::TestRequest::get().uri("/api/v1/health").to_request()).await;
    assert_eq!(resp.status().as_u16(), 200);
}

#[actix_web::test]
async fn info_lists_algorithms() {
    let app = app!();
    let (s, v) = call!(app, test::TestRequest::get().uri("/api/v1/info/").to_request());
    assert_eq!(s, 200);
    assert!(v["routing"].as_array().unwrap().iter().any(|x| x == "tsp"));
    assert!(v["clustering"].is_array() && v["bootstrap"].is_array());
}

#[actix_web::test]
async fn s2_cell_coverage_uses_v1_envelope() {
    let app = app!();
    let req = test::TestRequest::post()
        .uri("/api/v1/s2/cell-coverage")
        .set_json(json!({"lat": 41.15, "lon": -8.6, "level": 15, "size": 3}))
        .to_request();
    let (s, v) = call!(app, req);
    assert_eq!(s, 200);
    assert_eq!(v["status"], "ok");
    assert_eq!(v["message"], "Success");
    assert_eq!(v["status_code"], 200);
    assert!(!v["data"].as_array().unwrap().is_empty());
}

#[actix_web::test]
async fn s2_level_cells_take_flat_bounds() {
    let app = app!();
    let req = test::TestRequest::post()
        .uri("/api/v1/s2/15")
        .set_json(json!({"min_lat": 41.14, "min_lon": -8.62, "max_lat": 41.15, "max_lon": -8.60}))
        .to_request();
    let (s, v) = call!(app, req);
    assert_eq!(s, 200);
    assert!(!v["data"].as_array().unwrap().is_empty());
}

#[actix_web::test]
async fn convert_data_default_return_type_follows_area_shape() {
    let app = app!();
    let req = test::TestRequest::post()
        .uri("/api/v1/convert/data")
        .set_json(json!({"area": polygon_fc()}))
        .to_request();
    let (s, v) = call!(app, req);
    assert_eq!(s, 200);
    assert_eq!(v["data"]["type"], "FeatureCollection");

    let req = test::TestRequest::post()
        .uri("/api/v1/convert/data")
        .set_json(json!({"area": polygon_fc(), "return_type": "SingleArray"}))
        .to_request();
    let (_, v) = call!(app, req);
    assert!(v["data"].is_array(), "got {v}");
}

#[actix_web::test]
async fn convert_merge_points_builds_one_multipoint() {
    let app = app!();
    let pts = json!({"type":"FeatureCollection","features":[
        {"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[1.0,2.0]}},
        {"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[3.0,4.0]}}]});
    let req = test::TestRequest::post()
        .uri("/api/v1/convert/merge-points")
        .set_json(json!({"area": pts}))
        .to_request();
    let (s, v) = call!(app, req);
    assert_eq!(s, 200);
    let feats = v["data"]["features"].as_array().unwrap();
    assert_eq!(feats.len(), 1);
    assert_eq!(feats[0]["geometry"]["type"], "MultiPoint");
    assert_eq!(feats[0]["geometry"]["coordinates"].as_array().unwrap().len(), 2);
}

#[actix_web::test]
async fn calc_area_returns_positive_area() {
    let app = app!();
    let req = test::TestRequest::post()
        .uri("/api/v1/calc/area")
        .set_json(json!({"area": polygon_fc()}))
        .to_request();
    let (s, v) = call!(app, req);
    assert_eq!(s, 200);
    assert!(v["data"]["area"].as_f64().unwrap() > 0.0);
}

#[actix_web::test]
async fn convert_rejects_non_geojson_area_with_v1_error() {
    let app = app!();
    let req = test::TestRequest::post()
        .uri("/api/v1/convert/data")
        .set_json(json!({"area": "not geojson"}))
        .to_request();
    let (s, v) = call!(app, req);
    assert_eq!(s, 400);
    assert_eq!(v["status"], "error");
}
