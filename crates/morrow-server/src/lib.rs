mod auth;
mod handlers;
mod state;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::routing::{any, delete, get, post};
use axum::Router;

pub use state::ServerState;

/// Start the morrow API server. Blocks until the process is killed.
pub async fn serve(host: &str, port: u16) -> Result<()> {
    let config = Arc::new(morrow_core::Config::load()?);
    let store = morrow_core::CharacterStore::new();
    let state = ServerState::new(config, store);

    // built-in scheduler: run due tasks every 30s while serving
    {
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                if let Err(e) = crate::state::tick_active(&st).await {
                    eprintln!("[morrow-server] scheduler error: {e}");
                }
            }
        });
    }

    let app = Router::new()
        .route("/health", get(handlers::health))
        .route("/v1/{agent}/chat", post(handlers::chat))
        .route(
            "/v1/{agent}/messages",
            get(handlers::history).delete(handlers::clear_history),
        )
        .route("/v1/{agent}", get(handlers::info))
        .route("/v1/{agent}/settings", post(handlers::settings))
        .route(
            "/v1/{agent}/memory",
            get(handlers::list_memory)
                .post(handlers::add_memory)
                .delete(handlers::delete_memory),
        )
        .route(
            "/v1/{agent}/schedules",
            get(handlers::list_schedules).post(handlers::add_schedule),
        )
        .route(
            "/v1/{agent}/schedules/{id}",
            delete(handlers::remove_schedule),
        )
        .route(
            "/v1/{agent}/keys",
            get(handlers::list_keys).post(handlers::issue_key),
        )
        .route("/v1/{agent}/keys/{id}", delete(handlers::revoke_key))
        .route("/v1/{agent}/tick", any(not_found))
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .with_state(state);

    let addr = SocketAddr::new(host.parse()?, port);
    println!("morrow api listening on http://{addr}");
    println!("endpoints: /health, /v1/{{agent}}/chat (POST), memory, schedules, keys ...");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn not_found() -> &'static str {
    "not found"
}
