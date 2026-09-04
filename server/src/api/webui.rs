//! Serves the embedded web client so a plain browser can just open the
//! server's own address (no host to type). The bundle is a single inlined
//! `index.html` produced by `web/` (Vite + vite-plugin-singlefile); the three
//! station databases it fetches at runtime are embedded alongside it.
//!
//! `build.rs` stages these files from `web/dist` into `OUT_DIR`.

use axum::http::header;
use axum::response::{Html, IntoResponse, Response};

const INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/webui/index.html"));
const NWR_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/webui/nwr-stations.json"));
const FM_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/webui/fm-stations.json"));
const AM_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/webui/am-stations.json"));

/// `true` when a real bundle was embedded (vs. the build-time placeholder).
pub fn bundled() -> bool {
    INDEX_HTML.contains("id=\"app\"") || INDEX_HTML.contains("id=app")
}

pub async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

pub async fn nwr_stations() -> Response {
    json(NWR_JSON)
}

pub async fn fm_stations() -> Response {
    json(FM_JSON)
}

pub async fn am_stations() -> Response {
    json(AM_JSON)
}

fn json(body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        body,
    )
        .into_response()
}
