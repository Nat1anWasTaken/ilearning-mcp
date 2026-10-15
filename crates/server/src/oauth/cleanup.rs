//! Hourly purge of expired credentials, sign-in attempts and abandoned registrations.

use std::time::Duration;

use sqlx::PgPool;

const INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Expired credentials and abandoned registrations, by table.
const PURGES: [(&str, &str); 7] = [
    (
        "oauth_requests",
        "DELETE FROM oauth_requests WHERE expires_at < now()",
    ),
    (
        "oauth_codes",
        "DELETE FROM oauth_codes WHERE expires_at < now()",
    ),
    (
        "oauth_access_tokens",
        "DELETE FROM oauth_access_tokens WHERE expires_at < now()",
    ),
    (
        "oauth_refresh_tokens",
        "DELETE FROM oauth_refresh_tokens WHERE expires_at < now()",
    ),
    (
        "link_attempts",
        "DELETE FROM link_attempts WHERE expires_at < now()",
    ),
    (
        "dashboard_sessions",
        "DELETE FROM dashboard_sessions WHERE expires_at < now()",
    ),
    (
        "oauth_clients",
        "DELETE FROM oauth_clients c
         WHERE c.created_at < now() - interval '1 day'
           AND NOT EXISTS (SELECT 1 FROM agent_sessions s WHERE s.client_id = c.client_id)",
    ),
];

/// Runs the purges now and then hourly, for the lifetime of the process.
pub fn spawn(db: PgPool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(INTERVAL);
        loop {
            interval.tick().await;
            for (table, sql) in PURGES {
                match sqlx::query(sql).execute(&db).await {
                    Ok(done) if done.rows_affected() > 0 => {
                        tracing::debug!(
                            table,
                            deleted = done.rows_affected(),
                            "purged expired rows"
                        );
                    }
                    Ok(_) => {}
                    Err(err) => tracing::warn!(table, error = ?err, "purging expired rows"),
                }
            }
        }
    });
}
