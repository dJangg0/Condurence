//! Loopback HTTP transport for the MCP router.
//!
//! Bound to `127.0.0.1` on an ephemeral port at startup. Every request must carry the
//! per-run bearer token, and the `(run, instance)` pair must exist, so only processes the
//! orchestrator launched can reach it.

use std::net::TcpListener;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router as HttpRouter};
use serde_json::Value;

use super::router::Router;
use crate::error::AppResult;

/// Binds the loopback listener now so the port is known before any run starts.
pub fn bind_loopback() -> AppResult<(TcpListener, u16)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

pub fn serve(listener: TcpListener, router: Arc<Router>) -> AppResult<()> {
    listener.set_nonblocking(true)?;
    let app = HttpRouter::new()
        .route("/mcp/{run_id}/{instance_id}", post(handle))
        .with_state(router);

    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(e) => {
                eprintln!("MCP listener setup failed: {e}");
                return;
            }
        };
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("MCP server stopped: {e}");
        }
    });
    Ok(())
}

async fn handle(
    State(router): State<Arc<Router>>,
    Path((run_id, instance_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(message): Json<Value>,
) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if router.authorize(&run_id, &instance_id, token).is_err() {
        return (StatusCode::UNAUTHORIZED, "unknown run/instance or bad token").into_response();
    }

    match router.handle_message(&run_id, &instance_id, message).await {
        Ok(Some(reply)) => Json(reply).into_response(),
        // Notifications (no id) are acknowledged without a body.
        Ok(None) => StatusCode::ACCEPTED.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
