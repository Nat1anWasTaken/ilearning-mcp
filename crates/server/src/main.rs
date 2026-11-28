//! `ilearning-mcp`: a multi-user remote MCP server over CYCU i-learning 2.0,
//! built on mcp-backbone.

mod auth;
mod config;
mod files;
mod links;
mod service;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;
mod tools;

use mcp_backbone::migrate::Adoption;

use crate::{config::ILearningConfig, service::ILearningService};

/// Brings a database from before mcp-backbone into the framework's schema.
pub static ADOPTION: Adoption = Adoption {
    legacy_versions: &[1],
    before: include_str!("../../../migrations/adopt/before.sql"),
    after: include_str!("../../../migrations/adopt/after.sql"),
    service_baseline: None,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = ILearningConfig::from_env()?;
    // Sessions may send their write calls to the user's outbox; off for
    // each session until the user turns it on.
    mcp_backbone::App::new(ILearningService::new(config))
        .outbox(true)
        .push(true)
        .adopt(&ADOPTION)
        .serve()
        .await
}

#[cfg(test)]
mod adoption_tests {
    use chrono::{DateTime, TimeDelta, Utc};
    use mcp_backbone::{
        crypto::sealed,
        links::{LinkState, LinkStore, token_hash},
        migrate,
        sqlx::{self, PgPool, migrate::Migrator},
        testing::{TempDatabase, schema},
    };
    use serde_json::json;
    use uuid::Uuid;

    use super::ADOPTION;
    use crate::{links, testing};

    static LEGACY: Migrator = sqlx::migrate!("../../migrations/legacy");

    /// What `Config::for_tests` seals with.
    const KEY: [u8; 32] = [7; 32];

    /// A database from before mcp-backbone, with two linked users (one with a
    /// private token and a known expiry), a session and a logged call, is
    /// adopted with every row, matches a fresh schema, and its tokens still
    /// open.
    #[sqlx::test(migrations = false)]
    async fn legacy_databases_are_adopted(db: PgPool) {
        LEGACY.run(&db).await.unwrap();
        let renewable = Uuid::new_v4();
        let plain = Uuid::new_v4();
        let seal = |user_id: Uuid, secret: &str| {
            sealed::seal(&KEY, user_id.as_bytes(), secret.as_bytes()).unwrap()
        };
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO users (id, moodle_userid, username, fullname, avatar_url)
             VALUES ('{renewable}', 12345, 's1234', 'Ada Lovelace', 'https://pic.example/1'),
                    ('{plain}', 678, 's678', 'Bob', NULL);"
        )))
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO ilearning_links (user_id, token_sealed, token_hash, privatetoken_sealed,
                                          token_obtained_at, expires_at, status, last_ok_at)
             VALUES ($1, $2, $3, $4, now() - interval '3 days', now() + interval '80 days',
                     'ok', now())",
        )
        .bind(renewable)
        .bind(seal(renewable, "token-renewable"))
        .bind(token_hash("token-renewable"))
        .bind(seal(renewable, "private-renewable"))
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO ilearning_links (user_id, token_sealed, token_hash, status, last_error)
             VALUES ($1, $2, $3, 'needs_relink', 'iLearning rejected the token')",
        )
        .bind(plain)
        .bind(seal(plain, "token-plain"))
        .bind(token_hash("token-plain"))
        .execute(&db)
        .await
        .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO oauth_clients (client_id, redirect_uris, grant_types, source)
             VALUES ('client', ARRAY['https://agent.example/cb'], ARRAY['authorization_code'], 'dcr');
             INSERT INTO agent_sessions (id, user_id, client_id, name)
             VALUES ('00000000-0000-0000-0000-0000000000a1', '{renewable}', 'client', 'Claude');
             INSERT INTO tool_calls (user_id, agent_session_id, tool, arguments, status)
             VALUES ('{renewable}', '00000000-0000-0000-0000-0000000000a1', 'get_profile', '{{}}', 'ok');
             INSERT INTO dashboard_sessions (token_hash, user_id, expires_at)
             VALUES ('\\x01', '{renewable}', now() + interval '1 day');
             INSERT INTO link_attempts (passport, preauth_hash, next, expires_at)
             VALUES ('0123456789abcdef', '\\x02', '/', now() + interval '5 minutes');"
        )))
        .execute(&db)
        .await
        .unwrap();

        migrate::run(&db, None, Some(&ADOPTION)).await.unwrap();
        // Idempotent afterwards.
        migrate::run(&db, None, Some(&ADOPTION)).await.unwrap();

        let fresh = TempDatabase::new(&db).await;
        migrate::run(&fresh.pool, None, None).await.unwrap();
        assert_eq!(schema(&db).await, schema(&fresh.pool).await);
        fresh.drop().await;

        // Each user as `subject login name email`, the absent as `-`.
        let users: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, concat_ws(' ', subject, coalesce(login, '-'), coalesce(name, '-'),
                                  coalesce(email, '-'))
             FROM users ORDER BY subject",
        )
        .fetch_all(&db)
        .await
        .unwrap();
        assert_eq!(
            users,
            [
                (renewable, "12345 s1234 Ada Lovelace -".to_owned()),
                (plain, "678 s678 Bob -".to_owned()),
            ]
        );
        let (enabled, read_only, allowed, scope, outbox, ttl): (
            bool,
            bool,
            Option<Vec<String>>,
            serde_json::Value,
            bool,
            i32,
        ) = sqlx::query_as(
            "SELECT enabled, read_only, allowed_tools, scope, use_outbox, outbox_ttl_minutes
             FROM agent_sessions",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert!(enabled && !read_only && !outbox);
        assert_eq!((allowed, scope, ttl), (None, json!({}), 60));
        let counts: (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM tool_calls), (SELECT count(*) FROM dashboard_sessions),
                    (SELECT count(*) FROM link_attempts)",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(counts, (1, 1, 0));
        let legacy: bool = sqlx::query_scalar("SELECT to_regclass('ilearning_links') IS NULL")
            .fetch_one(&db)
            .await
            .unwrap();
        assert!(legacy, "the legacy link table is gone");

        // The sealed tokens still open, and the links read as before.
        let state = testing::state(db.clone()).await;
        let store = LinkStore::new(&state);
        let record = store.load(renewable).await.unwrap().unwrap();
        assert_eq!(
            record.secret(&state, links::TOKEN).unwrap(),
            "token-renewable"
        );
        assert_eq!(
            record.secret(&state, links::PRIVATETOKEN).unwrap(),
            "private-renewable"
        );
        let status = links::status(&state, renewable).await.unwrap().unwrap();
        assert_eq!(status.status, LinkState::Ok);
        assert!(status.auto_renew);
        assert!(status.expires_at.unwrap() > Utc::now() + TimeDelta::days(79));
        let three_days_ago = Utc::now() - TimeDelta::days(3);
        assert!(
            (status.token_obtained_at - three_days_ago).abs() < TimeDelta::minutes(1),
            "{}",
            status.token_obtained_at
        );
        let obtained: DateTime<Utc> =
            serde_json::from_value(record.metadata.0["token_obtained_at"].clone()).unwrap();
        assert_eq!(obtained, status.token_obtained_at);
        let linked = links::account(&state, &state.service.moodle, renewable)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(linked.token_hash, Some(token_hash("token-renewable")));

        let record = store.load(plain).await.unwrap().unwrap();
        assert_eq!(record.secret(&state, links::TOKEN).unwrap(), "token-plain");
        assert!(!record.has_secret(links::PRIVATETOKEN));
        let status = links::status(&state, plain).await.unwrap().unwrap();
        assert_eq!(status.status, LinkState::NeedsRelink);
        assert_eq!(
            status.last_error.as_deref(),
            Some("iLearning rejected the token")
        );
        assert!(!status.auto_renew && status.expires_at.is_none());
    }
}
