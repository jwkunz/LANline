use crate::model::*;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;

pub async fn health(State(st): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok", uptime_s: st.uptime_s(), version: SERVER_VERSION })
}

pub async fn server_info(State(st): State<AppState>) -> Json<ServerInfo> {
    Json(ServerInfo {
        server_id: st.server_id,
        protocol_version: PROTOCOL_VERSION,
        version: SERVER_VERSION,
        hostname: st.hostname.clone(),
        time: now_utc(),
        ports: st.ports,
        capabilities: st.capabilities(),
        selected_device: st.registry.selected_summary(),
    })
}
