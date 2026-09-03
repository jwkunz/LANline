//! Phase-2 endpoints: defined for a stable contract, not yet implemented.

use crate::error::ApiError;

pub async fn stub() -> ApiError {
    ApiError::not_implemented("transmit / modulate is a phase 2 feature")
}
