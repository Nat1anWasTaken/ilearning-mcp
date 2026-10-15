//! The path every tool call takes: argument parsing, the user's iLearning
//! account, execution and the call log.

use std::time::Instant;

use rmcp::{
    ErrorData,
    model::{CallToolResult, JsonObject},
};
use serde_json::Value;

use super::{
    log::{self, NewCall, Status},
    output::failure,
    tools::{self, Tool},
};
use crate::{files, links, oauth::AgentPrincipal, state::AppState};

/// Handles `tools/call`. Only an unknown tool or an internal failure is a
/// protocol error; everything else is a tool result the agent can read.
pub async fn call(
    state: &AppState,
    principal: AgentPrincipal,
    name: &str,
    args: JsonObject,
) -> Result<CallToolResult, ErrorData> {
    let tool = tools::find(name)
        .ok_or_else(|| ErrorData::invalid_params(format!("unknown tool: {name}"), None))?;
    run(state, principal, tool, Value::Object(args))
        .await
        .map_err(|err| {
            tracing::error!(error = ?err, "serving an MCP request");
            ErrorData::internal_error("internal error", None)
        })
}

/// What the agent is told when iLearning no longer accepts the user's token.
pub fn relink_message(state: &AppState) -> String {
    format!(
        "The user's iLearning sign-in has expired or was revoked. Ask the user to re-link \
         their iLearning account at {}/ and then try again.",
        state.config.public_url
    )
}

async fn run(
    state: &AppState,
    principal: AgentPrincipal,
    tool: &'static Tool,
    args: Value,
) -> sqlx::Result<CallToolResult> {
    let started = Instant::now();
    let request = match tool.parse(&args) {
        Ok(request) => request,
        Err(err) => {
            let entry = NewCall::unparsed(principal, tool.name, &args);
            return refuse(state, &entry, format!("Invalid arguments: {err}"), started).await;
        }
    };
    drop(args);
    let entry = NewCall::new(principal, tool.name, &request.log_arguments());
    let linked = match links::account(state, principal.user_id).await {
        Ok(Some(linked)) => linked,
        Ok(None) => return refuse(state, &entry, relink_message(state), started).await,
        Err(err) => {
            tracing::error!(error = ?err, "loading the iLearning account");
            return refuse(state, &entry, "internal error".into(), started).await;
        }
    };
    // Detached: rmcp drops this request's future when the client goes away,
    // but an action that started must run to its end and be logged.
    let state = state.clone();
    let task = tokio::spawn(async move {
        let id = log::start(&state.db, &entry).await?;
        let user_id = principal.user_id;
        let (outcome, status, result, error) = match request.run(linked.account).await {
            Ok(output) => {
                links::mark_ok(&state.db, user_id).await;
                let (outcome, result) =
                    output.into_parts(|file_url| files::link(&state, user_id, file_url));
                (outcome, Status::Ok, result, None)
            }
            Err(moodle::Error::InvalidToken) => {
                links::mark_invalid(&state.db, user_id, &linked.token_hash).await;
                let message = relink_message(&state);
                (failure(message.clone()), Status::Error, None, Some(message))
            }
            Err(err) => {
                let message = err.to_string();
                (failure(message.clone()), Status::Error, None, Some(message))
            }
        };
        let logged = log::finish(
            &state.db,
            id,
            status,
            result,
            error.as_deref(),
            started.elapsed(),
        )
        .await;
        if let Err(err) = logged {
            // The agent still gets the outcome.
            tracing::error!(%id, error = ?err, "logging the outcome of a tool call");
        }
        Ok(outcome)
    });
    task.await.unwrap_or_else(|err| {
        tracing::error!(tool = tool.name, error = ?err, "tool call task failed");
        Ok(failure("internal error"))
    })
}

/// Logs a call that ends without running.
async fn refuse(
    state: &AppState,
    entry: &NewCall,
    message: String,
    started: Instant,
) -> sqlx::Result<CallToolResult> {
    log::record(&state.db, entry, &message, started.elapsed()).await?;
    Ok(failure(message))
}
