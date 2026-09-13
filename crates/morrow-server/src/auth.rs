use axum::http::{HeaderMap, StatusCode};
use std::sync::Arc;

use crate::state::ServerState;

/// Extract + verify the bearer token; returns the agent it belongs to.
/// The {agent} path segment must match the key's owner — keys are agent-scoped.
pub async fn authenticate(
    state: &Arc<ServerState>,
    headers: &HeaderMap,
    agent: &str,
) -> Result<(), (StatusCode, String)> {
    let presented = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                "missing Authorization: Bearer <key>".into(),
            )
        })?;

    match state.resolve_key(presented).await {
        Some(owner) if owner == agent => Ok(()),
        Some(other) => Err((
            StatusCode::FORBIDDEN,
            format!("this key belongs to '{other}', not '{agent}'"),
        )),
        None => Err((StatusCode::UNAUTHORIZED, "invalid api key".into())),
    }
}
