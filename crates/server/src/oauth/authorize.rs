//! The authorization endpoint. Validates the request and hands it to the
//! dashboard's consent page (see [`super::consent`]).

use anyhow::Context;
use axum::{
    extract::{OriginalUri, Query, State},
    http::{StatusCode, Uri},
    response::{IntoResponse, Redirect, Response},
};
use chrono::{TimeDelta, Utc};
use serde::Deserialize;
use url::form_urlencoded;
use uuid::Uuid;

use super::{
    OAuthError, SCOPE, check_resource, cimd,
    client::{self, ResolveError},
    pkce, redirect_uri,
};
use crate::{auth::CurrentUser, error::AppError, state::AppState};

const REQUEST_TTL: TimeDelta = TimeDelta::minutes(10);

#[derive(Deserialize)]
pub struct AuthorizeParams {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    state: Option<String>,
    resource: Option<String>,
}

pub async fn authorize(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    user: Option<CurrentUser>,
    Query(params): Query<AuthorizeParams>,
) -> Result<Response, AppError> {
    // Until the redirect URI is known to belong to the client, errors are
    // shown to the user instead of being sent to it.
    let Some(client_id) = params.client_id.as_deref() else {
        return Ok(invalid_request_page("client_id is missing"));
    };
    // Resolving a metadata-document client fetches a URL and stores it, so
    // anonymous requests must not get that far.
    let document_url = cimd::document_url(client_id);
    if user.is_none() && document_url.is_some() {
        return Ok(login_redirect(&uri));
    }
    let client = match client::resolve(&state.db, client_id, document_url.as_ref()).await {
        Ok(client) => client,
        Err(ResolveError::Database(err)) => return Err(err.into()),
        Err(err) => return Ok(invalid_request_page(&err.to_string())),
    };
    let Some(redirect_uri) = params.redirect_uri.as_deref() else {
        return Ok(invalid_request_page("redirect_uri is missing"));
    };
    if !client
        .redirect_uris
        .iter()
        .any(|registered| redirect_uri::matches(registered, redirect_uri))
    {
        return Ok(invalid_request_page(
            "redirect_uri is not registered for this client",
        ));
    }

    let reject = |error: OAuthError| -> Result<Response, AppError> {
        let mut response = vec![
            ("error", error.code()),
            ("error_description", error.description()),
            ("iss", state.config.public_url.as_str()),
        ];
        response.extend(params.state.as_deref().map(|s| ("state", s)));
        let location =
            redirect_uri::with_params(redirect_uri, response).context("building error redirect")?;
        Ok(Redirect::to(&location).into_response())
    };
    if params.response_type.as_deref() != Some("code") {
        return reject(OAuthError::unsupported_response_type());
    }
    let Some(code_challenge) = params
        .code_challenge
        .as_deref()
        .filter(|challenge| pkce::is_valid_challenge(challenge))
    else {
        return reject(OAuthError::invalid_request(
            "a PKCE S256 code_challenge is required",
        ));
    };
    if params.code_challenge_method.as_deref() != Some("S256") {
        return reject(OAuthError::invalid_request(
            "code_challenge_method must be S256",
        ));
    }
    if let Err(error) = check_resource(&state.config, params.resource.as_deref()) {
        return reject(error);
    }

    if user.is_none() {
        return Ok(login_redirect(&uri));
    }

    let request_id: Uuid = sqlx::query_scalar(
        "INSERT INTO oauth_requests
             (client_id, redirect_uri, state, scope, resource, code_challenge, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(&client.client_id)
    .bind(redirect_uri)
    .bind(&params.state)
    .bind(SCOPE)
    .bind(&params.resource)
    .bind(code_challenge)
    .bind(Utc::now() + REQUEST_TTL)
    .fetch_one(&state.db)
    .await?;
    Ok(Redirect::to(&format!("/consent/{request_id}")).into_response())
}

/// Sends the browser to sign in, then back to this authorization request.
fn login_redirect(uri: &Uri) -> Response {
    let this_url = uri.path_and_query().map_or("/", |pq| pq.as_str());
    let next: String = form_urlencoded::byte_serialize(this_url.as_bytes()).collect();
    Redirect::to(&format!("/login?next={next}")).into_response()
}

fn invalid_request_page(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format!("Invalid authorization request: {message}"),
    )
        .into_response()
}
