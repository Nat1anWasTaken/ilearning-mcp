//! The MCP endpoint at `/mcp`: tool registry, call pipeline and call log.

mod log;
mod output;
mod pipeline;
mod server;
#[cfg(test)]
mod tests;
mod tools;

use std::sync::Arc;

use axum::{
    Router,
    http::{HeaderName, Method, header},
    middleware,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use tower_http::cors::{Any, CorsLayer};

use crate::{oauth, state::AppState};

pub use log::{
    Filter as ToolCallFilter, Status as ToolCallStatus, ToolCall, find as find_tool_call,
    list as list_tool_calls, settle_interrupted as settle_interrupted_calls,
};
pub use tools::{Tool, all as all_tools};

/// Room for a `submit_assignment` with a few files, base64-encoded.
const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;

/// The MCP endpoint, protected by OAuth bearer tokens.
pub fn router(state: AppState) -> Router<AppState> {
    // Stateless with plain JSON responses: the server only answers tool calls
    // and never sends requests or notifications of its own, so nothing needs
    // a session or an SSE stream. Any request can be served by any process,
    // including after a restart, and nothing depends on proxies (Cloudflare)
    // passing streams through unbuffered.
    //
    // rmcp only accepts loopback `Host` headers by default (a DNS rebinding
    // guard for local servers), which would reject every request in
    // production, so allow the public host instead. Origins are not checked:
    // requests authenticate with bearer tokens, which browsers never attach
    // on their own.
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts([state.config.public_host()])
        .with_max_request_body_bytes(MAX_REQUEST_BYTES);
    let handler_state = state.clone();
    let service = StreamableHttpService::new(
        move || Ok(server::ILearningServer::new(handler_state.clone())),
        Arc::new(NeverSessionManager::default()),
        config,
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, oauth::require_agent))
        // Outermost, so preflights skip authentication and browser clients
        // (e.g. MCP Inspector) can read the 401 challenge.
        .layer(cors())
}

fn cors() -> CorsLayer {
    let mcp_header = HeaderName::from_static;
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            mcp_header("mcp-session-id"),
            mcp_header("mcp-protocol-version"),
            mcp_header("last-event-id"),
            // Sent with every request from 2026-07-28 on (SEP-2243).
            mcp_header("mcp-method"),
            mcp_header("mcp-name"),
        ])
        .expose_headers([mcp_header("mcp-session-id"), header::WWW_AUTHENTICATE])
}
