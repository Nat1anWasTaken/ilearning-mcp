//! Shared application state.

use std::sync::Arc;

use sqlx::PgPool;

use crate::{config::Config, links::RenewLocks, oauth::ClientAssertions};

/// Shared application state. Every field is cheap to clone.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: PgPool,
    pub moodle: moodle::Moodle,
    pub client_assertions: Arc<ClientAssertions>,
    pub renew_locks: RenewLocks,
}
