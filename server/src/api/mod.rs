//! axum router assembly.

mod adsb;
mod analysis;
mod ais;
mod apt;
mod aprs;
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
        .route("/radio/record", post(radio::record))
        .route("/radio/recording", get(radio::download))
        .route("/radio/scan", post(radio::scan))
        .route("/radio/tx/key", post(radio::key_tx))
        .route("/radio/tx/unkey", post(radio::unkey_tx))
        .route("/radio/tx/log", get(radio::tx_log))
        .route("/radio/tx/say", post(radio::say))
        .route("/radio/transcript", get(radio::transcript))
        .route("/adsb/aircraft", get(adsb::aircraft))
        .route("/adsb/messages", get(adsb::messages))
        .route("/adsb/flight/{icao}", get(adsb::flight))
        .route("/ais/vessels", get(ais::vessels))
        .route("/ais/messages", get(ais::sentences))
        .route("/ais/vessel/{mmsi}", get(ais::vessel))
        .route("/aprs/stations", get(aprs::stations))
        .route("/aprs/packets", get(aprs::packets))
        .route("/aprs/tx", post(aprs::tx))
        .route("/apt/image", get(apt::image))
        .route("/apt/status", get(apt::status))
        .route("/analysis/spectrum", get(analysis::spectrum))
        .route("/analysis/status", get(analysis::status))
        .route("/analysis/record", post(analysis::record))
        .route("/analysis/recording", get(analysis::download))
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
        .route("/am-stations.json", get(webui::am_stations))
        .route("/apt-tle.json", get(webui::apt_tle))
        .route("/repeaters.json", get(webui::repeaters))
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
        let radio_mgr = RadioManager::new(radio_cfg.clone(), registry.clone(), None, std::env::temp_dir(), false, crate::voice::VoiceShared::new(&config));
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
            Ports { c2: 0, audio_out, audio_in: 0, beast: 0, ais_nmea: 0, aprs: 0 },
            Ipv4Addr::LOCALHOST.into(),
            registry,
            sessions,
            radio_cfg,
            radio_mgr,
            webrtc,
        );
        router(state)
    }

    /// Same as `app()`, but built as if started with `--enable-tx` — for the
    /// handful of tests exercising what's gated *behind* that flag, as
    /// opposed to the flag itself (which every other test's plain `app()`
    /// covers by leaving it off, the real default).
    async fn app_tx_enabled() -> axum::Router {
        let config = Config::parse_from(["lanline-server", "--no-beacon", "--enable-tx"]);
        let registry = Arc::new(DeviceRegistry::new()); // no device selected
        let radio_cfg = Arc::new(Mutex::new(RadioConfig::default_noaa()));
        let radio_mgr = RadioManager::new(radio_cfg.clone(), registry.clone(), None, std::env::temp_dir(), true, crate::voice::VoiceShared::new(&config));
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
            Ports { c2: 0, audio_out, audio_in: 0, beast: 0, ais_nmea: 0, aprs: 0 },
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

        for path in
            ["/nwr-stations.json", "/fm-stations.json", "/am-stations.json", "/apt-tle.json", "/repeaters.json"]
        {
            let res = app.clone().oneshot(get(path)).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert!(
                res.headers()
                    .get("content-type")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .contains("application/json"),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn am_mode_is_selectable() {
        let app = app().await;
        let (_, b) = send(
            &app,
            json_req("POST", "/api/v1/sessions", None, json!({ "client": { "name": "t" } })),
        )
        .await;
        let token = b["token"].as_str().unwrap().to_string();

        let (_, modes) = send(&app, get("/api/v1/modes")).await;
        assert!(modes.as_array().unwrap().iter().any(|m| m["id"] == "am"));

        let (s, b) = send(
            &app,
            json_req("PATCH", "/api/v1/radio", Some(&token), json!({ "mode": "am" })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["mode"], "am");
        assert_eq!(b["mode_params"]["channel_bw_hz"], 10000.0);

        // am needs an SDR like nbfm/wbfm -> 503 with no device selected
        let (s, b) = send(
            &app,
            json_req("POST", "/api/v1/radio/start", Some(&token), Value::Null),
        )
        .await;
        assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(b["error"]["code"], "device_unavailable");
    }

    #[tokio::test]
    async fn ham_mode_is_selectable() {
        let app = app().await;
        let (_, b) = send(
            &app,
            json_req("POST", "/api/v1/sessions", None, json!({ "client": { "name": "t" } })),
        )
        .await;
        let token = b["token"].as_str().unwrap().to_string();

        let (_, modes) = send(&app, get("/api/v1/modes")).await;
        assert!(modes.as_array().unwrap().iter().any(|m| m["id"] == "ham"));

        let (s, b) = send(
            &app,
            json_req(
                "PATCH",
                "/api/v1/radio",
                Some(&token),
                json!({ "mode": "ham", "frequency_hz": 146_520_000 }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["mode"], "ham");
        assert_eq!(b["mode_params"]["deviation_hz"], 5000.0);
        assert_eq!(b["frequency_hz"], 146_520_000);

        // ham needs an SDR like nbfm/wbfm/am/frs -> 503 with no device selected
        let (s, b) = send(
            &app,
            json_req("POST", "/api/v1/radio/start", Some(&token), Value::Null),
        )
        .await;
        assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(b["error"]["code"], "device_unavailable");

        // server capabilities advertise it
        let (_, srv) = send(&app, get("/api/v1/server")).await;
        assert!(srv["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "ham"));
    }

    #[tokio::test]
    async fn frs_mode_is_selectable() {
        let app = app().await;
        let (_, b) = send(
            &app,
            json_req("POST", "/api/v1/sessions", None, json!({ "client": { "name": "t" } })),
        )
        .await;
        let token = b["token"].as_str().unwrap().to_string();

        let (_, modes) = send(&app, get("/api/v1/modes")).await;
        assert!(modes.as_array().unwrap().iter().any(|m| m["id"] == "frs"));

        let (s, b) = send(
            &app,
            json_req("PATCH", "/api/v1/radio", Some(&token), json!({ "mode": "frs", "frequency_hz": 462_562_500 })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["mode"], "frs");
        assert_eq!(b["mode_params"]["channel_bw_hz"], 14000.0);
        assert_eq!(b["frequency_hz"], 462_562_500);

        // frs needs an SDR like nbfm/wbfm/am -> 503 with no device selected
        let (s, b) = send(
            &app,
            json_req("POST", "/api/v1/radio/start", Some(&token), Value::Null),
        )
        .await;
        assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(b["error"]["code"], "device_unavailable");
    }

    #[tokio::test]
    async fn tx_key_requires_a_tx_mode() {
        // TX enabled here specifically so this test isolates the mode check
        // from the (separately tested) --enable-tx gate.
        let app = app_tx_enabled().await;
        let (_, b) = send(
            &app,
            json_req("POST", "/api/v1/sessions", None, json!({ "client": { "name": "t" } })),
        )
        .await;
        let token = b["token"].as_str().unwrap().to_string();

        // Default mode is nbfm (RadioConfig::default_noaa) -> rejected even
        // with TX enabled.
        let (s, b) = send(
            &app,
            json_req("POST", "/api/v1/radio/tx/key", Some(&token), Value::Null),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(b["error"]["code"], "bad_request");

        // `ham` clears the mode check — it falls through to the
        // no-tx-capable-device check instead (the test harness has none).
        send(&app, json_req("PATCH", "/api/v1/radio", Some(&token), json!({ "mode": "ham" }))).await;
        let (s, b) = send(
            &app,
            json_req("POST", "/api/v1/radio/tx/key", Some(&token), Value::Null),
        )
        .await;
        assert_ne!(s, StatusCode::BAD_REQUEST, "ham should pass the mode check");
        assert_ne!(b["error"]["code"], "bad_request");
    }

    #[tokio::test]
    async fn tx_key_requires_enable_tx_flag() {
        // Plain app() — the real default (TX off) — proves the gate holds
        // even once every other precondition (mode) is satisfied.
        let app = app().await;
        let (_, b) = send(
            &app,
            json_req("POST", "/api/v1/sessions", None, json!({ "client": { "name": "t" } })),
        )
        .await;
        let token = b["token"].as_str().unwrap().to_string();

        send(
            &app,
            json_req("PATCH", "/api/v1/radio", Some(&token), json!({ "mode": "frs" })),
        )
        .await;

        let (s, b) = send(
            &app,
            json_req("POST", "/api/v1/radio/tx/key", Some(&token), Value::Null),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        assert_eq!(b["error"]["code"], "forbidden");
    }

    #[tokio::test]
    async fn tx_unkey_is_always_a_safe_no_op() {
        let app = app().await;
        let (_, b) = send(
            &app,
            json_req("POST", "/api/v1/sessions", None, json!({ "client": { "name": "t" } })),
        )
        .await;
        let token = b["token"].as_str().unwrap().to_string();

        let (s, b) = send(
            &app,
            json_req("POST", "/api/v1/radio/tx/unkey", Some(&token), Value::Null),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["keyed"], false);
    }

    #[tokio::test]
    async fn scan_start_needs_a_running_audio_mode_stop_is_a_no_op() {
        let app = app().await;
        let (_, b) = send(
            &app,
            json_req("POST", "/api/v1/sessions", None, json!({ "client": { "name": "t" } })),
        )
        .await;
        let token = b["token"].as_str().unwrap().to_string();

        // Nothing running -> 409, whatever channels are passed.
        let (s, b) = send(
            &app,
            json_req(
                "POST",
                "/api/v1/radio/scan",
                Some(&token),
                json!({ "action": "start", "channels": [{ "frequency_hz": 462_562_500.0 }] }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(b["error"]["code"], "conflict");

        // Stop is always safe.
        let (s, b) = send(
            &app,
            json_req("POST", "/api/v1/radio/scan", Some(&token), json!({ "action": "stop" })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["scanning"], false);

        // status carries a scan block.
        let (_, b) = send(&app, get("/api/v1/radio/status")).await;
        assert_eq!(b["scan"]["active"], false);
        assert_eq!(b["scan"]["total"], 0);
    }

    #[tokio::test]
    async fn adsb_endpoints_report_empty_when_idle() {
        let app = app().await;

        let (s, b) = send(&app, get("/api/v1/adsb/aircraft")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["aircraft_count"], 0);
        assert_eq!(b["running"], false);
        assert!(b["aircraft"].as_array().unwrap().is_empty());

        let (s, b) = send(&app, get("/api/v1/adsb/messages")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["count"], 0);

        // adsb is advertised as a mode + capability
        let (_, modes) = send(&app, get("/api/v1/modes")).await;
        assert!(modes.as_array().unwrap().iter().any(|m| m["id"] == "adsb"));
    }

    #[tokio::test]
    async fn ais_endpoints_report_empty_when_idle() {
        let app = app().await;

        let (s, b) = send(&app, get("/api/v1/ais/vessels")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["vessel_count"], 0);
        assert!(b["vessels"].as_array().unwrap().is_empty());

        let (s, b) = send(&app, get("/api/v1/ais/messages")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["count"], 0);

        let (_, modes) = send(&app, get("/api/v1/modes")).await;
        assert!(modes.as_array().unwrap().iter().any(|m| m["id"] == "ais"));
    }

    #[tokio::test]
    async fn apt_endpoints_report_empty_when_idle() {
        let app = app().await;

        let res = app.clone().oneshot(get("/api/v1/apt/image")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let ct = res.headers().get("content-type").unwrap().to_str().unwrap().to_string();
        assert!(ct.contains("application/octet-stream"), "got {ct}");
        let body = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        assert_eq!(body.len(), 8, "header only, no rows yet");
        let width = u32::from_le_bytes(body[0..4].try_into().unwrap());
        let height = u32::from_le_bytes(body[4..8].try_into().unwrap());
        assert_eq!(width, 1818); // 909 * 2 channels
        assert_eq!(height, 0);

        let (s, b) = send(&app, get("/api/v1/apt/status")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["lines"], 0);
        assert_eq!(b["width"], 1818);

        let (_, modes) = send(&app, get("/api/v1/modes")).await;
        assert!(modes.as_array().unwrap().iter().any(|m| m["id"] == "apt"));
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
