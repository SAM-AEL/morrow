//! HTTP handlers: per-character chat, memory, schedules and API keys.
//!
//! Auth is agent-scoped bearer tokens. Bootstrap rule: a character with no
//! enabled keys is open (so you can issue the first key); once keys exist,
//! every request must present one owned by the `{agent}` in the path.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use morrow_agent::AgentEvent;

use crate::state::ServerState;

type ApiResult = (StatusCode, Json<serde_json::Value>);

fn ok(v: serde_json::Value) -> ApiResult {
    (StatusCode::OK, Json(v))
}

fn err(code: StatusCode, msg: impl Into<String>) -> ApiResult {
    (code, Json(json!({ "error": msg.into() })))
}

/// Bootstrap-friendly auth: open while the character has no keys.
async fn ensure_auth(
    state: &Arc<ServerState>,
    headers: &HeaderMap,
    agent: &str,
) -> Result<(), (StatusCode, String)> {
    let has_keys = state
        .store
        .load(agent)
        .ok()
        .flatten()
        .map(|c| c.api_keys.iter().any(|k| k.enabled))
        .unwrap_or(false);
    if !has_keys {
        return Ok(());
    }
    crate::auth::authenticate(state, headers, agent).await
}

pub async fn health() -> impl IntoResponse {
    "ok"
}

// ---- chat ---------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct ChatBody {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}

pub async fn chat(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(agent): Path<String>,
    Json(body): Json<ChatBody>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    let text = body
        .message
        .or(body.prompt)
        .or(body.text)
        .unwrap_or_default();
    if text.trim().is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "missing 'message' (or 'prompt'/'text')",
        );
    }
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut runner = arc.lock().await;
    let mut noop = |_: AgentEvent| {};
    match runner.chat(&text, &mut noop).await {
        Ok(reply) => ok(json!({
            "agent": agent,
            "reply": reply,
            "usage": runner.total_usage,
        })),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

// ---- history ------------------------------------------------------------

pub async fn history(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(agent): Path<String>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let runner = arc.lock().await;
    ok(json!({ "agent": agent, "messages": runner.character.messages }))
}

pub async fn clear_history(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(agent): Path<String>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut runner = arc.lock().await;
    let n = runner.character.messages.len();
    runner.character.messages.clear();
    if let Err(e) = runner.save_character() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    ok(json!({ "agent": agent, "cleared": n }))
}

// ---- character info / settings ------------------------------------------

pub async fn info(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(agent): Path<String>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let runner = arc.lock().await;
    let c = &runner.character;
    ok(json!({
        "name": c.name,
        "mode": format!("{:?}", c.mode).to_lowercase(),
        "model": c.model,
        "persona": c.persona,
        "memories": c.memories.len(),
        "messages": c.messages.len(),
        "integrations": c.integrations.keys().collect::<Vec<_>>(),
        "schedules": c.schedules.len(),
        "updated_at": c.updated_at,
    }))
}

#[derive(Debug, Deserialize, Default)]
pub struct SettingsBody {
    #[serde(default)]
    pub persona: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
}

pub async fn settings(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(agent): Path<String>,
    Json(body): Json<SettingsBody>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut runner = arc.lock().await;
    if let Some(p) = body.persona {
        runner.character.persona = p;
    }
    if let Some(m) = body.model {
        runner.character.model = Some(m);
    }
    if let Some(mode) = body.mode {
        match mode.to_ascii_lowercase().as_str() {
            "autonomous" | "auto" => runner.character.mode = morrow_core::Mode::Autonomous,
            "approval" | "ask" => runner.character.mode = morrow_core::Mode::Approval,
            other => return err(StatusCode::BAD_REQUEST, format!("unknown mode '{other}'")),
        }
    }
    if let Err(e) = runner.save_character() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    ok(json!({
        "agent": agent,
        "mode": format!("{:?}", runner.character.mode).to_lowercase(),
        "model": runner.character.model,
    }))
}

// ---- memory --------------------------------------------------------------

pub async fn list_memory(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(agent): Path<String>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let runner = arc.lock().await;
    ok(json!({ "agent": agent, "memories": runner.character.memories }))
}

#[derive(Debug, Deserialize)]
pub struct AddMemoryBody {
    pub text: String,
}

pub async fn add_memory(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(agent): Path<String>,
    Json(body): Json<AddMemoryBody>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    if body.text.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "missing 'text'");
    }
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut runner = arc.lock().await;
    runner.character.remember(body.text.clone());
    if let Err(e) = runner.save_character() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    ok(json!({ "agent": agent, "memories": runner.character.memories.len() }))
}

#[derive(Debug, Deserialize, Default)]
pub struct DeleteMemoryBody {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub query: Option<String>,
}

pub async fn delete_memory(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(agent): Path<String>,
    body: Option<Json<DeleteMemoryBody>>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut runner = arc.lock().await;
    let removed = match body.map(|b| b.0) {
        Some(DeleteMemoryBody { id: Some(id), .. }) => {
            let before = runner.character.memories.len();
            runner.character.memories.retain(|m| m.id != id);
            before - runner.character.memories.len()
        }
        Some(DeleteMemoryBody { query: Some(q), .. }) => {
            let q = q.to_lowercase();
            let before = runner.character.memories.len();
            runner
                .character
                .memories
                .retain(|m| !m.text.to_lowercase().contains(&q));
            before - runner.character.memories.len()
        }
        _ => {
            let n = runner.character.memories.len();
            runner.character.memories.clear();
            n
        }
    };
    runner.character.touch();
    if let Err(e) = runner.save_character() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    ok(json!({ "agent": agent, "removed": removed }))
}

// ---- schedules ------------------------------------------------------------

pub async fn list_schedules(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(agent): Path<String>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let runner = arc.lock().await;
    ok(json!({ "agent": agent, "schedules": runner.character.schedules }))
}

#[derive(Debug, Deserialize)]
pub struct AddScheduleBody {
    pub when: String,
    pub task: String,
}

pub async fn add_schedule(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(agent): Path<String>,
    Json(body): Json<AddScheduleBody>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    if body.when.trim().is_empty() || body.task.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "need both 'when' and 'task'");
    }
    let sched = match morrow_core::Schedule::new(&body.when, &body.task) {
        Ok(s) => s,
        Err(e) => return err(StatusCode::BAD_REQUEST, e.to_string()),
    };
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut runner = arc.lock().await;
    if runner.character.schedules.len() >= 100 {
        return err(StatusCode::CONFLICT, "schedule limit (100) reached");
    }
    let id = sched.id.clone();
    runner.character.schedules.push(sched);
    if let Err(e) = runner.save_character() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    ok(json!({ "agent": agent, "id": id }))
}

pub async fn remove_schedule(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path((agent, id)): Path<(String, String)>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut runner = arc.lock().await;
    let before = runner.character.schedules.len();
    runner.character.schedules.retain(|s| s.id != id);
    if runner.character.schedules.len() == before {
        return err(StatusCode::NOT_FOUND, format!("no schedule '{id}'"));
    }
    if let Err(e) = runner.save_character() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    ok(json!({ "agent": agent, "removed": id }))
}

// ---- api keys --------------------------------------------------------------

pub async fn list_keys(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(agent): Path<String>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let runner = arc.lock().await;
    let keys: Vec<_> = runner
        .character
        .api_keys
        .iter()
        .map(|k| {
            json!({
                "id": k.id,
                "label": k.label,
                "created_at": k.created_at,
                "last_used": k.last_used,
                "enabled": k.enabled,
            })
        })
        .collect();
    ok(json!({ "agent": agent, "keys": keys }))
}

#[derive(Debug, Deserialize, Default)]
pub struct IssueKeyBody {
    #[serde(default)]
    pub label: Option<String>,
}

pub async fn issue_key(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(agent): Path<String>,
    body: Option<Json<IssueKeyBody>>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut runner = arc.lock().await;
    let label = body
        .and_then(|b| b.0.label)
        .unwrap_or_else(|| "api".to_string());
    let (entry, plaintext) = match runner.character.issue_key(&label) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    if let Err(e) = runner.save_character() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    ok(json!({
        "agent": agent,
        "id": entry.id,
        "label": entry.label,
        "key": plaintext,
        "note": "shown once — store it now",
    }))
}

pub async fn revoke_key(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path((agent, id)): Path<(String, String)>,
) -> ApiResult {
    if let Err((code, msg)) = ensure_auth(&state, &headers, &agent).await {
        return err(code, msg);
    }
    let arc = match state.runner_for(&agent).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut runner = arc.lock().await;
    if !runner.character.revoke_key(&id) {
        return err(StatusCode::NOT_FOUND, format!("no key '{id}'"));
    }
    if let Err(e) = runner.save_character() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    ok(json!({ "agent": agent, "revoked": id }))
}
