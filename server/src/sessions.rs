//! In-memory session store and the `AuthedSession` extractor.

use crate::error::ApiError;
use crate::model::*;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use std::collections::HashMap;
use std::sync::Mutex;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[derive(Clone)]
pub struct Session {
    pub id: Uuid,
    pub token: String,
    pub client: ClientInfo,
    pub created: OffsetDateTime,
    pub expires: OffsetDateTime,
    pub audio_state: AudioConnState,
}

impl Session {
    fn summary(&self) -> SessionSummary {
        SessionSummary {
            session_id: self.id,
            client: self.client.clone(),
            created: self.created,
            expires: self.expires,
            audio: SessionAudio { state: self.audio_state },
        }
    }
}

pub struct SessionStore {
    inner: Mutex<HashMap<Uuid, Session>>,
    ttl_s: u64,
    heartbeat_s: u64,
    max: usize,
}

impl SessionStore {
    pub fn new(heartbeat_s: u64, ttl_s: u64, max: usize) -> Self {
        Self { inner: Mutex::new(HashMap::new()), ttl_s, heartbeat_s, max }
    }

    pub fn create(&self, client: ClientInfo) -> Result<CreateSessionResponse, ApiError> {
        let mut map = self.inner.lock().unwrap();
        let now = OffsetDateTime::now_utc();
        map.retain(|_, s| s.expires > now);
        if map.len() >= self.max {
            return Err(ApiError::too_many_sessions());
        }
        let id = Uuid::new_v4();
        let token = random_token();
        let expires = now + Duration::seconds(self.ttl_s as i64);
        map.insert(
            id,
            Session {
                id,
                token: token.clone(),
                client,
                created: now,
                expires,
                audio_state: AudioConnState::Idle,
            },
        );
        Ok(CreateSessionResponse {
            session_id: id,
            token,
            heartbeat_interval_s: self.heartbeat_s,
            expires_in_s: self.ttl_s,
        })
    }

    /// Resolve a bearer token to a live session id.
    pub fn authenticate(&self, token: &str) -> Option<Uuid> {
        let now = OffsetDateTime::now_utc();
        let map = self.inner.lock().unwrap();
        map.values()
            .find(|s| s.token == token && s.expires > now)
            .map(|s| s.id)
    }

    pub fn get(&self, id: Uuid) -> Option<SessionSummary> {
        let now = OffsetDateTime::now_utc();
        let map = self.inner.lock().unwrap();
        map.get(&id).filter(|s| s.expires > now).map(Session::summary)
    }

    pub fn list(&self) -> Vec<SessionSummary> {
        let now = OffsetDateTime::now_utc();
        let map = self.inner.lock().unwrap();
        map.values().filter(|s| s.expires > now).map(Session::summary).collect()
    }

    pub fn heartbeat(&self, id: Uuid) -> Result<HeartbeatResponse, ApiError> {
        let mut map = self.inner.lock().unwrap();
        let now = OffsetDateTime::now_utc();
        let session = map.get_mut(&id).filter(|s| s.expires > now).ok_or_else(|| {
            ApiError::not_found("unknown or expired session")
        })?;
        session.expires = now + Duration::seconds(self.ttl_s as i64);
        Ok(HeartbeatResponse { expires_in_s: self.ttl_s })
    }

    pub fn delete(&self, id: Uuid) -> bool {
        self.inner.lock().unwrap().remove(&id).is_some()
    }

    pub fn set_audio_state(&self, id: Uuid, state: AudioConnState) {
        if let Some(s) = self.inner.lock().unwrap().get_mut(&id) {
            s.audio_state = state;
        }
    }

    pub fn audio_state(&self, id: Uuid) -> Option<AudioConnState> {
        self.inner.lock().unwrap().get(&id).map(|s| s.audio_state)
    }

    /// Drop expired sessions; returns the ids removed (for WebRTC teardown in
    /// later phases).
    pub fn reap(&self) -> Vec<Uuid> {
        let now = OffsetDateTime::now_utc();
        let mut map = self.inner.lock().unwrap();
        let dead: Vec<Uuid> =
            map.iter().filter(|(_, s)| s.expires <= now).map(|(id, _)| *id).collect();
        for id in &dead {
            map.remove(id);
        }
        dead
    }

    pub fn active_count(&self) -> usize {
        let now = OffsetDateTime::now_utc();
        self.inner.lock().unwrap().values().filter(|s| s.expires > now).count()
    }
}

fn random_token() -> String {
    let bytes: [u8; 24] = rand::random();
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("s_");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Extractor that authenticates the `Authorization: Bearer <token>` header
/// against the session store.
pub struct AuthedSession {
    pub id: Uuid,
}

impl FromRequestParts<AppState> for AuthedSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(ApiError::unauthorized)?;
        let token = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .ok_or_else(ApiError::unauthorized)?
            .trim();
        let id = state.sessions.authenticate(token).ok_or_else(ApiError::unauthorized)?;
        Ok(AuthedSession { id })
    }
}
