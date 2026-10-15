//! Per-user iLearning tokens: sealed storage, health bookkeeping and the
//! background task that renews tokens around their expiry.
//!
//! Normal operation only ever uses the token. Renewal needs an iLearning web
//! session, and the site allows one web login per user, so creating it logs
//! the user out of iLearning in their browser: it happens once per token
//! lifetime, shortly before the token expires.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, TimeDelta, Utc};
use dashmap::DashMap;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection, PgPool};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{config::Config, crypto::sealed, state::AppState};

/// How often the renewal task looks for tokens close to expiry.
const SCAN_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Start renewing this long before a token expires.
const RENEW_LEAD: TimeDelta = TimeDelta::minutes(40);
/// Mint a new token this long after the old one expired.
const MINT_DELAY: TimeDelta = TimeDelta::minutes(1);
/// Give up waiting for a new token this long after the old one expired.
const MINT_DEADLINE: TimeDelta = TimeDelta::hours(2);
/// Keep the web session alive at this interval (its idle timeout is 1 h).
const TOUCH_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Retry minting at this interval while iLearning returns the old token.
const MINT_RETRY: Duration = Duration::from_secs(5 * 60);
/// iLearning allows one autologin key per user every 6 minutes.
const AUTOLOGIN_INTERVAL: Duration = Duration::from_secs(6 * 60 + 10);
const AUTOLOGIN_ATTEMPTS: usize = 3;
/// `last_ok_at` is bumped at most this often.
const OK_THROTTLE_SECS: i32 = 60;

/// Per-user locks that serialize renewals, with the time of the user's last
/// autologin.
pub type RenewLocks = Arc<DashMap<Uuid, Arc<Mutex<Option<Instant>>>>>;

/// The user's iLearning link as the dashboard shows it (`LinkStatus`).
#[derive(Debug, Serialize, FromRow)]
pub struct LinkStatus {
    pub status: String,
    pub token_obtained_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_ok_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// Has a private token and a known expiry, so the server renews it.
    pub auto_renew: bool,
}

pub async fn status(db: &PgPool, user_id: Uuid) -> sqlx::Result<Option<LinkStatus>> {
    sqlx::query_as(
        "SELECT status, token_obtained_at, expires_at, last_ok_at, last_error,
                (privatetoken_sealed IS NOT NULL AND expires_at IS NOT NULL) AS auto_renew
         FROM ilearning_links WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

/// Hex SHA-256 of a token, to tell tokens apart without unsealing them.
pub fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn seal(config: &Config, user_id: Uuid, secret: &str) -> anyhow::Result<Vec<u8>> {
    sealed::seal(
        &config.encryption_key,
        user_id.as_bytes(),
        secret.as_bytes(),
    )
}

fn open(config: &Config, user_id: Uuid, sealed_value: &[u8]) -> anyhow::Result<String> {
    let plain = sealed::open(&config.encryption_key, user_id.as_bytes(), sealed_value)?;
    Ok(String::from_utf8(plain)?)
}

/// Stores the tokens from a sign-in link. A token that differs from the one
/// stored is fresh, so its expiry is known; the same token keeps the known
/// expiry, and a first link leaves it unknown (iLearning returns an existing
/// token until it expires).
pub async fn store_linked(
    tx: &mut PgConnection,
    config: &Config,
    user_id: Uuid,
    tokens: &moodle::Tokens,
) -> anyhow::Result<()> {
    let hash = token_hash(&tokens.token);
    let previous: Option<String> =
        sqlx::query_scalar("SELECT token_hash FROM ilearning_links WHERE user_id = $1 FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    let fresh_expiry = match &previous {
        Some(previous) if *previous != hash => Some(Utc::now() + config.token_duration()),
        _ => None,
    };
    let privatetoken = tokens
        .privatetoken
        .as_deref()
        .map(|secret| seal(config, user_id, secret))
        .transpose()?;
    sqlx::query(
        "INSERT INTO ilearning_links (user_id, token_sealed, token_hash, privatetoken_sealed,
                                      token_obtained_at, expires_at, status, last_ok_at)
         VALUES ($1, $2, $3, $4, now(), NULL, 'ok', now())
         ON CONFLICT (user_id) DO UPDATE SET
             token_sealed = EXCLUDED.token_sealed,
             token_hash = EXCLUDED.token_hash,
             privatetoken_sealed = COALESCE(EXCLUDED.privatetoken_sealed,
                                            ilearning_links.privatetoken_sealed),
             token_obtained_at = CASE WHEN ilearning_links.token_hash = EXCLUDED.token_hash
                                      THEN ilearning_links.token_obtained_at ELSE now() END,
             expires_at = CASE WHEN ilearning_links.token_hash = EXCLUDED.token_hash
                               THEN ilearning_links.expires_at ELSE $5 END,
             status = 'ok',
             last_ok_at = now(),
             last_error = NULL,
             updated_at = now()",
    )
    .bind(user_id)
    .bind(seal(config, user_id, &tokens.token)?)
    .bind(&hash)
    .bind(privatetoken)
    .bind(fresh_expiry)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// A user's iLearning account, ready for calls.
pub struct Linked {
    pub account: moodle::Account,
    /// Identifies the token in use, see [`mark_invalid`].
    pub token_hash: String,
}

/// The user's account, or `None` if they never linked one. A link marked
/// `needs_relink` is still tried: iLearning has the final word.
pub async fn account(state: &AppState, user_id: Uuid) -> anyhow::Result<Option<Linked>> {
    let row: Option<(Vec<u8>, String)> =
        sqlx::query_as("SELECT token_sealed, token_hash FROM ilearning_links WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((sealed_token, token_hash)) = row else {
        return Ok(None);
    };
    let token = open(&state.config, user_id, &sealed_token)?;
    Ok(Some(Linked {
        account: state.moodle.account(token),
        token_hash,
    }))
}

/// Records a successful call, at most once a minute.
pub async fn mark_ok(db: &PgPool, user_id: Uuid) {
    let result = sqlx::query(
        "UPDATE ilearning_links SET last_ok_at = now()
         WHERE user_id = $1
           AND (last_ok_at IS NULL OR last_ok_at < now() - make_interval(secs => $2))",
    )
    .bind(user_id)
    .bind(OK_THROTTLE_SECS)
    .execute(db)
    .await;
    if let Err(err) = result {
        tracing::warn!(%user_id, error = ?err, "recording a successful iLearning call");
    }
}

/// Records that iLearning rejected the token `token_hash` identifies, unless
/// it was replaced in the meantime.
pub async fn mark_invalid(db: &PgPool, user_id: Uuid, token_hash: &str) {
    let result = sqlx::query(
        "UPDATE ilearning_links
         SET status = 'needs_relink', last_error = 'iLearning rejected the token',
             updated_at = now()
         WHERE user_id = $1 AND token_hash = $2",
    )
    .bind(user_id)
    .bind(token_hash)
    .execute(db)
    .await;
    match result {
        Ok(done) if done.rows_affected() > 0 => {
            tracing::info!(%user_id, "iLearning token rejected; needs re-link");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(%user_id, error = ?err, "marking an iLearning link invalid"),
    }
}

/// Starts the renewal task for the lifetime of the process.
pub fn spawn_renewal(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SCAN_INTERVAL);
        loop {
            interval.tick().await;
            match due_for_renewal(&state.db).await {
                Ok(users) => {
                    for user_id in users {
                        let state = state.clone();
                        tokio::spawn(async move { renew_user(&state, user_id).await });
                    }
                }
                Err(err) => tracing::warn!(error = ?err, "looking for tokens to renew"),
            }
        }
    });
}

/// Users whose token expires soon (or did, while this process was down) and
/// can be renewed.
async fn due_for_renewal(db: &PgPool) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar(
        "SELECT user_id FROM ilearning_links
         WHERE status IN ('ok', 'renewing') AND privatetoken_sealed IS NOT NULL
           AND expires_at IS NOT NULL AND expires_at < $1",
    )
    .bind(Utc::now() + RENEW_LEAD)
    .fetch_all(db)
    .await
}

#[derive(FromRow)]
struct RenewRow {
    token_sealed: Vec<u8>,
    token_hash: String,
    privatetoken_sealed: Option<Vec<u8>>,
    expires_at: Option<DateTime<Utc>>,
    moodle_userid: i64,
}

/// How a renewal ended.
enum Renewed {
    /// A new token, stored.
    Token,
    /// iLearning kept returning the old token past the deadline.
    GaveUp,
    /// The link changed under us (re-linked or rejected); nothing to do.
    Superseded,
}

/// Renews one user's token unless a renewal for them is already running.
async fn renew_user(state: &AppState, user_id: Uuid) {
    let lock = state.renew_locks.entry(user_id).or_default().clone();
    let Ok(mut last_autologin) = lock.try_lock_owned() else {
        return;
    };
    tracing::info!(%user_id, "renewing iLearning token");
    match renew(state, user_id, &mut last_autologin).await {
        Ok(Renewed::Token) => tracing::info!(%user_id, "renewed iLearning token"),
        Ok(Renewed::GaveUp) => {
            tracing::warn!(%user_id, "iLearning kept the old token; expiry now unknown");
        }
        Ok(Renewed::Superseded) => {}
        Err(err) => {
            tracing::warn!(%user_id, error = %err, "renewing iLearning token failed");
            let result = sqlx::query(
                "UPDATE ilearning_links
                 SET status = 'needs_relink', last_error = $2, updated_at = now()
                 WHERE user_id = $1 AND status = 'renewing'",
            )
            .bind(user_id)
            .bind(format!("Automatic renewal failed: {err}"))
            .execute(&state.db)
            .await;
            if let Err(err) = result {
                tracing::warn!(%user_id, error = ?err, "recording a failed renewal");
            }
        }
    }
}

async fn renew(
    state: &AppState,
    user_id: Uuid,
    last_autologin: &mut Option<Instant>,
) -> anyhow::Result<Renewed> {
    let row: Option<RenewRow> = sqlx::query_as(
        "SELECT l.token_sealed, l.token_hash, l.privatetoken_sealed, l.expires_at,
                u.moodle_userid
         FROM ilearning_links l JOIN users u ON u.id = l.user_id
         WHERE l.user_id = $1 AND l.status IN ('ok', 'renewing')",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(row) = row else {
        return Ok(Renewed::Superseded);
    };
    let (Some(privatetoken_sealed), Some(expires_at)) = (&row.privatetoken_sealed, row.expires_at)
    else {
        return Ok(Renewed::Superseded);
    };
    if expires_at - Utc::now() >= RENEW_LEAD {
        return Ok(Renewed::Superseded);
    }
    let token = open(&state.config, user_id, &row.token_sealed)?;
    let privatetoken = open(&state.config, user_id, privatetoken_sealed)?;
    set_renewing(&state.db, user_id, &row.token_hash).await?;

    let session = autologin(
        state,
        &token,
        &privatetoken,
        row.moodle_userid,
        last_autologin,
    )
    .await?;
    let mut last_touch = Instant::now();
    let mut mint_at = expires_at + MINT_DELAY;
    loop {
        // Sleep until the next mint attempt or keep-alive, whichever is first.
        let until_mint = (mint_at - Utc::now()).to_std().unwrap_or_default();
        let until_touch = TOUCH_INTERVAL.saturating_sub(last_touch.elapsed());
        if until_touch < until_mint {
            tokio::time::sleep(until_touch).await;
            moodle::renew::touch(&state.moodle, &session).await?;
            last_touch = Instant::now();
            continue;
        }
        tokio::time::sleep(until_mint).await;
        if !still_renewing(&state.db, user_id, &row.token_hash).await? {
            return Ok(Renewed::Superseded);
        }
        let minted = moodle::renew::mint(&state.moodle, &session).await?;
        last_touch = Instant::now();
        if minted.token != token {
            store_renewed(state, user_id, &row.token_hash, &minted).await?;
            return Ok(Renewed::Token);
        }
        if Utc::now() > expires_at + MINT_DEADLINE {
            sqlx::query(
                "UPDATE ilearning_links SET status = 'ok', expires_at = NULL, updated_at = now()
                 WHERE user_id = $1 AND token_hash = $2",
            )
            .bind(user_id)
            .bind(&row.token_hash)
            .execute(&state.db)
            .await?;
            return Ok(Renewed::GaveUp);
        }
        mint_at = Utc::now() + TimeDelta::from_std(MINT_RETRY)?;
    }
}

/// Opens a web session, respecting iLearning's autologin rate limit.
async fn autologin(
    state: &AppState,
    token: &str,
    privatetoken: &str,
    moodle_userid: i64,
    last_autologin: &mut Option<Instant>,
) -> anyhow::Result<moodle::renew::WebSession> {
    for attempt in 1..=AUTOLOGIN_ATTEMPTS {
        if let Some(last) = *last_autologin {
            tokio::time::sleep(AUTOLOGIN_INTERVAL.saturating_sub(last.elapsed())).await;
        }
        *last_autologin = Some(Instant::now());
        match moodle::renew::autologin(&state.moodle, token, privatetoken, moodle_userid).await {
            Ok(session) => return Ok(session),
            Err(moodle::Error::RateLimited) if attempt < AUTOLOGIN_ATTEMPTS => continue,
            Err(err) => return Err(err.into()),
        }
    }
    unreachable!("the last attempt returns")
}

async fn set_renewing(db: &PgPool, user_id: Uuid, token_hash: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE ilearning_links SET status = 'renewing', updated_at = now()
         WHERE user_id = $1 AND token_hash = $2 AND status <> 'renewing'",
    )
    .bind(user_id)
    .bind(token_hash)
    .execute(db)
    .await?;
    Ok(())
}

/// Whether the link is still the one being renewed.
async fn still_renewing(db: &PgPool, user_id: Uuid, token_hash: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM ilearning_links
                        WHERE user_id = $1 AND token_hash = $2 AND status = 'renewing')",
    )
    .bind(user_id)
    .bind(token_hash)
    .fetch_one(db)
    .await
}

async fn store_renewed(
    state: &AppState,
    user_id: Uuid,
    old_hash: &str,
    tokens: &moodle::Tokens,
) -> anyhow::Result<()> {
    let privatetoken = tokens
        .privatetoken
        .as_deref()
        .map(|secret| seal(&state.config, user_id, secret))
        .transpose()?;
    sqlx::query(
        "UPDATE ilearning_links SET
             token_sealed = $3, token_hash = $4,
             privatetoken_sealed = COALESCE($5, privatetoken_sealed),
             token_obtained_at = now(), expires_at = $6, status = 'ok', last_error = NULL,
             updated_at = now()
         WHERE user_id = $1 AND token_hash = $2",
    )
    .bind(user_id)
    .bind(old_hash)
    .bind(seal(&state.config, user_id, &tokens.token)?)
    .bind(token_hash(&tokens.token))
    .bind(privatetoken)
    .bind(Utc::now() + state.config.token_duration())
    .execute(&state.db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::testing;

    fn tokens(token: &str) -> moodle::Tokens {
        moodle::Tokens {
            token: token.to_owned(),
            privatetoken: Some("p".repeat(64)),
        }
    }

    async fn expiry(db: &PgPool, user_id: Uuid) -> Option<DateTime<Utc>> {
        status(db, user_id).await.unwrap().unwrap().expires_at
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn relinking_learns_the_expiry_of_fresh_tokens(db: PgPool) {
        let state = testing::state(db.clone());
        let (user_id, _) = testing::sign_in(&state, 7).await;
        let store = |token: &'static str| {
            let state = state.clone();
            async move {
                let mut tx = state.db.begin().await.unwrap();
                store_linked(&mut tx, &state.config, user_id, &tokens(token))
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
            }
        };
        // sign_in links a token whose expiry is unknown.
        assert_eq!(expiry(&db, user_id).await, None);
        store("a".repeat(32).leak()).await;
        let known = expiry(&db, user_id).await.expect("fresh token");
        let days = (known - Utc::now()).num_days();
        assert!((83..=84).contains(&days), "{days}");
        // The same token again keeps the known expiry.
        store("a".repeat(32).leak()).await;
        assert_eq!(expiry(&db, user_id).await, Some(known));

        let linked = account(&state, user_id).await.unwrap().unwrap();
        assert_eq!(linked.token_hash, token_hash(&"a".repeat(32)));
        let status = status(&db, user_id).await.unwrap().unwrap();
        assert!(status.auto_renew);

        mark_invalid(&db, user_id, "stale").await;
        assert_eq!(
            super::status(&db, user_id).await.unwrap().unwrap().status,
            "ok"
        );
        mark_invalid(&db, user_id, &linked.token_hash).await;
        let status = super::status(&db, user_id).await.unwrap().unwrap();
        assert_eq!(status.status, "needs_relink");
        assert!(status.last_error.is_some());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn only_soon_expiring_renewable_links_are_due(db: PgPool) {
        let state = testing::state(db.clone());
        let (due, _) = testing::sign_in(&state, 1).await;
        let (later, _) = testing::sign_in(&state, 2).await;
        let (unknown, _) = testing::sign_in(&state, 3).await;
        for (user_id, minutes) in [(due, 30), (later, 60 * 24)] {
            sqlx::query(
                "UPDATE ilearning_links SET expires_at = now() + make_interval(mins => $2),
                        privatetoken_sealed = '\\x00'
                 WHERE user_id = $1",
            )
            .bind(user_id)
            .bind(minutes)
            .execute(&db)
            .await
            .unwrap();
        }
        let _ = unknown;
        assert_eq!(due_for_renewal(&db).await.unwrap(), vec![due]);
    }

    #[test]
    fn token_hash_is_hex_sha256() {
        assert_eq!(
            token_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
