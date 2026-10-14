//! v2 scanner import source — `GET /api/v2/scanner/areas`.
//!
//! Lists the configured scanner's (Dragonite's) areas that have a geofence, so
//! the map's import dialog can load them onto the map. Needs `DRAGONITE_URL` +
//! `DRAGONITE_BEARER`; without them it answers `422` with a hint rather than an
//! empty list, so the UI can say why nothing is available.

use actix_web::{HttpResponse, get, web};
use koji_dragonite::{DragoniteClient, patch::Tri};
use serde_json::json;

use crate::utils::api_response::{ApiError, ApiResponse};
use crate::utils::error::ServiceError;

/// `GET /api/v2/scanner/areas` — `[ { id, name, enabled, geofence: Feature } ]`
/// for every scanner area that has a base geofence.
#[utoipa::path(
    get,
    path = "/api/v2/scanner/areas",
    tag = "scanner",
    responses(
        (status = 200, description = "Scanner areas that have a geofence", body = Object),
        (status = 422, description = "No scanner (Dragonite) configured", body = ApiError),
        (status = 500, description = "Scanner request failed", body = ApiError),
    ),
)]
#[get("/scanner/areas")]
pub(crate) async fn list_areas(
    dragonite: web::Data<Option<DragoniteClient>>,
) -> Result<HttpResponse, ServiceError> {
    let Some(client) = dragonite.get_ref() else {
        return Err(ServiceError::Unprocessable {
            field: None,
            message: "no scanner configured: set DRAGONITE_URL and DRAGONITE_BEARER".to_string(),
        });
    };
    let areas = client.list_areas().await.map_err(ServiceError::internal)?;
    let out: Vec<_> = areas
        .into_iter()
        .filter_map(|a| match a.geofence {
            Tri::Value(feature) => Some(json!({
                "id": a.id,
                "name": a.name,
                "enabled": a.enabled,
                "geofence": feature,
            })),
            _ => None,
        })
        .collect();
    Ok(ApiResponse::success(json!(out)))
}
