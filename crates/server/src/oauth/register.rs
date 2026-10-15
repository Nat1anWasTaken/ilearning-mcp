//! Dynamic client registration (RFC 7591). Registration is open.

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{StatusCode, header},
    response::IntoResponse,
};
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::{
    OAuthError, SCOPE,
    client::{self, AuthMethod, Registration, Source},
};
use crate::{
    crypto::{hash_token, random_token},
    state::AppState,
};

#[derive(Serialize)]
struct RegistrationResponse {
    client_id: String,
    client_id_issued_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    /// `0`: the secret never expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    client_secret_expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logo_uri: Option<String>,
    redirect_uris: Vec<String>,
    grant_types: Vec<String>,
    response_types: [&'static str; 1],
    token_endpoint_auth_method: AuthMethod,
    scope: &'static str,
}

pub async fn register(
    State(state): State<AppState>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<impl IntoResponse, OAuthError> {
    let Json(metadata) =
        body.map_err(|err| OAuthError::invalid_client_metadata(err.body_text()))?;
    let registration = Registration::from_metadata(metadata, AuthMethod::ClientSecretBasic)?;
    if registration.auth_method == AuthMethod::PrivateKeyJwt {
        return Err(OAuthError::invalid_client_metadata(
            "private_key_jwt is supported only for clients identified by a metadata document",
        ));
    }

    let client_id = Uuid::new_v4().to_string();
    let secret = (registration.auth_method != AuthMethod::None).then(random_token);
    let secret_hash = secret.as_deref().map(hash_token);
    client::save(
        &state.db,
        &client_id,
        secret_hash.as_deref(),
        Source::Dcr,
        &registration,
    )
    .await?;
    tracing::info!(%client_id, name = ?registration.client_name, "registered OAuth client");

    let response = RegistrationResponse {
        client_id,
        client_id_issued_at: Utc::now().timestamp(),
        client_secret_expires_at: secret.as_ref().map(|_| 0),
        client_secret: secret,
        client_name: registration.client_name,
        client_uri: registration.client_uri,
        logo_uri: registration.logo_uri,
        redirect_uris: registration.redirect_uris,
        grant_types: registration.grant_types,
        response_types: ["code"],
        token_endpoint_auth_method: registration.auth_method,
        scope: SCOPE,
    };
    Ok((
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(response),
    ))
}
