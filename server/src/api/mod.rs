//! axum router assembly.

mod audio;
mod devices;
mod meta;
mod modes;
mod presets;
mod radio;
mod reserved;
mod sessions;

use crate::state::AppState;
use axum::http::{header, HeaderValue, Method};
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

pub fn router(state: AppState) -> Router {
    let cors = build_cors(&state.config.cors_origins);

    let v1 = Router::new()
        .route("/server", get(meta::server_info))
        .route("/devices", get(devices::list))
        .route("/device", get(devices::selected).put(devices::select))
        .route("/device/health", get(devices::health))
        .route("/modes", get(modes::list))
        .route("/presets", get(presets::list))
        .route("/presets/{id}/apply", post(presets::apply))
        .route("/radio", get(radio::get_radio).patch(radio::patch_radio))
        .route("/radio/start", post(radio::start))
        .route("/radio/stop", post(radio::stop))
        .route("/radio/status", get(radio::status))
        .route("/radio/tx", get(reserved::stub).patch(reserved::stub))
        .route("/radio/tx/ptt", post(reserved::stub))
        .route("/sessions", get(sessions::list).post(sessions::create))
        .route("/sessions/{id}", get(sessions::get_one).delete(sessions::delete))
        .route("/sessions/{id}/heartbeat", post(sessions::heartbeat))
        .route("/sessions/{id}/audio/offer", post(audio::offer))
        .route("/sessions/{id}/audio/ice", post(audio::ice))
        .route("/sessions/{id}/audio", get(audio::state).delete(audio::close))
        .route("/sessions/{id}/broadcast/offer", post(reserved::stub))
        .route(
            "/sessions/{id}/broadcast",
            get(reserved::stub).delete(reserved::stub),
        );

    Router::new()
        .route("/health", get(meta::health))
        .nest("/api/v1", v1)
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

fn build_cors(origins: &str) -> CorsLayer {
    let base = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    if origins.trim() == "*" {
        base.allow_origin(Any)
    } else {
        let list: Vec<HeaderValue> = origins
            .split(',')
            .filter_map(|o| HeaderValue::from_str(o.trim()).ok())
            .collect();
        base.allow_origin(list)
    }
}
