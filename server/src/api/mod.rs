//! axum router assembly.

mod audio;
mod devices;
mod meta;
mod modes;
mod presets;
mod radio;
mod reserved;
mod sessions;
pub mod webui;

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
        // The embedded web client — open the server's own address in a browser
        // and it connects to that same origin with nothing to type.
        .route("/", get(webui::index))
        .route("/index.html", get(webui::index))
        .route("/nwr-stations.json", get(webui::nwr_stations))
        .route("/fm-stations.json", get(webui::fm_stations))
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
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        // Let the browser cache the preflight so a burst of PATCHes (e.g. the
        // client's seek scan) doesn't pay an OPTIONS round-trip each time.
        .max_age(std::time::Duration::from_secs(3600));

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

#[cfg(test)]
mod tests {
    use super::router;
    use crate::config::Config;
    use crate::media::WebrtcEngine;
    use crate::model::{Ports, RadioConfig};
    use crate::radio::RadioManager;
    use crate::registry::DeviceRegistry;
    use crate::sessions::SessionStore;
    use crate::state::AppState;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use clap::Parser;
    use serde_json::{json, Value};
    use std::net::Ipv4Addr;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    async fn app() -> axum::Router {
        let config = Config::parse_from(["lanline-server", "--no-beacon"]);
        let registry = Arc::new(DeviceRegistry::new()); // no device selected
        let radio_cfg = Arc::new(Mutex::new(RadioConfig::default_noaa()));
        let radio_mgr = RadioManager::new(radio_cfg.clone(), registry.clone(), None);
        let sessions = Arc::new(SessionStore::new(15, 45, 8));
        let (webrtc, audio_out) = WebrtcEngine::new(
            Ipv4Addr::LOCALHOST.into(),
            0,
            Ipv4Addr::LOCALHOST.into(),
            radio_mgr.clone(),
            sessions.clone(),
        )
        .await
        .unwrap();
        let state = AppState::new(
            config,
            Ports { c2: 0, audio_out, audio_in: 0 },
            Ipv4Addr::LOCALHOST.into(),
            registry,
            sessions,
            radio_cfg,
            radio_mgr,
            webrtc,
        );
        router(state)
    }

    async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, body)
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    fn json_req(method: &str, uri: &str, token: Option<&str>, body: Value) -> Request<Body> {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        b.body(Body::from(body.to_string())).unwrap()
    }

    #[tokio::test]
    async fn health_and_server_info() {
        let app = app().await;
        let (s, b) = send(&app, get("/health")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["status"], "ok");

        let (s, b) = send(&app, get("/api/v1/server")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["protocol_version"], 1);
        assert!(b["capabilities"].as_array().unwrap().iter().any(|c| c == "nbfm"));
    }

    #[tokio::test]
    async fn patch_radio_requires_auth() {
        let app = app().await;
        let (s, b) = send(
            &app,
            json_req("PATCH", "/api/v1/radio", None, json!({ "frequency_hz": 162_400_000 })),
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert_eq!(b["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn session_lifecycle_and_ownership() {
        let app = app().await;
        let (s, b) = send(
            &app,
            json_req("POST", "/api/v1/sessions", None, json!({ "client": { "name": "test" } })),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
        let token = b["token"].as_str().unwrap().to_string();
        let id = b["session_id"].as_str().unwrap().to_string();

        let (s, _) = send(&app, {
            let mut r = get(&format!("/api/v1/sessions/{id}"));
            r.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
            r
        })
        .await;
        assert_eq!(s, StatusCode::OK);

        // someone else's session id -> 404
        let other = uuid::Uuid::new_v4();
        let (s, _) = send(&app, {
            let mut r = get(&format!("/api/v1/sessions/{other}"));
            r.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
            r
        })
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND);

        let (s, _) = send(
            &app,
            json_req("DELETE", &format!("/api/v1/sessions/{id}"), Some(&token), Value::Null),
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn validation_and_stub_codes() {
        let app = app().await;
        let (_, b) = send(
            &app,
            json_req("POST", "/api/v1/sessions", None, json!({ "client": { "name": "t" } })),
        )
        .await;
        let token = b["token"].as_str().unwrap().to_string();

        // unknown mode -> 422
        let (s, b) = send(
            &app,
            json_req("PATCH", "/api/v1/radio", Some(&token), json!({ "mode": "bogus" })),
        )
        .await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(b["error"]["details"]["field"], "mode");

        // nbfm start with no device -> 503
        let (s, b) = send(
            &app,
            json_req("POST", "/api/v1/radio/start", Some(&token), Value::Null),
        )
        .await;
        assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(b["error"]["code"], "device_unavailable");

        // reserved tx endpoint -> 501
        let (s, _) = send(&app, {
            let mut r = get("/api/v1/radio/tx");
            r.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
            r
        })
        .await;
        assert_eq!(s, StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn serves_embedded_web_client() {
        let app = app().await;

        let res = app.clone().oneshot(get("/")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let ct = res.headers().get("content-type").unwrap().to_str().unwrap().to_string();
        assert!(ct.starts_with("text/html"), "got {ct}");
        let body = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("LANline"));

        let res = app.clone().oneshot(get("/nwr-stations.json")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("application/json"));
    }

    #[tokio::test]
    async fn debug_tone_start_stop() {
        let app = app().await;
        let (_, b) = send(
            &app,
            json_req("POST", "/api/v1/sessions", None, json!({ "client": { "name": "t" } })),
        )
        .await;
        let token = b["token"].as_str().unwrap().to_string();

        let (s, _) = send(
            &app,
            json_req("PATCH", "/api/v1/radio", Some(&token), json!({ "mode": "debug_tone" })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);

        let (s, b) = send(
            &app,
            json_req("POST", "/api/v1/radio/start", Some(&token), Value::Null),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["running"], true);

        let (s, b) = send(
            &app,
            json_req("POST", "/api/v1/radio/stop", Some(&token), Value::Null),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["running"], false);
    }
}
