//! Discovery documents: protected resource (RFC 9728) and authorization
//! server (RFC 8414) metadata.

use axum::{Json, extract::State};
use serde_json::{Value, json};

use super::{
    SCOPE,
    client::{AuthMethod, SUPPORTED_GRANT_TYPES},
    client_assertion::SIGNING_ALGORITHMS,
};
use crate::state::AppState;

pub async fn protected_resource(State(state): State<AppState>) -> Json<Value> {
    let config = &state.config;
    Json(json!({
        "resource": config.mcp_resource(),
        "authorization_servers": [config.public_url],
        "scopes_supported": [SCOPE],
        "bearer_methods_supported": ["header"],
        "resource_name": "iLearning MCP",
    }))
}

pub async fn authorization_server(State(state): State<AppState>) -> Json<Value> {
    let issuer = &state.config.public_url;
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/oauth/token"),
        "registration_endpoint": format!("{issuer}/oauth/register"),
        "revocation_endpoint": format!("{issuer}/oauth/revoke"),
        "scopes_supported": [SCOPE],
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": SUPPORTED_GRANT_TYPES,
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": AuthMethod::ALL,
        "token_endpoint_auth_signing_alg_values_supported": SIGNING_ALGORITHMS,
        "revocation_endpoint_auth_methods_supported": AuthMethod::ALL,
        "revocation_endpoint_auth_signing_alg_values_supported": SIGNING_ALGORITHMS,
        "client_id_metadata_document_supported": true,
        "authorization_response_iss_parameter_supported": true,
    }))
}
