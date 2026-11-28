//! Per-user iLearning tokens in the framework's link store, and the
//! background task that renews them around their expiry.
//!
//! The link's secrets are [`TOKEN`] (the mobile web service token) and,
//! when the sign-in link carried one, [`PRIVATETOKEN`]; its `expires_at` is
//! the token's expiry when known (`NULL` = unknown), and its metadata holds
//! `token_obtained_at`. The framework records each call's outcome on it.
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
use mcp_backbone::{
    AppState, Core, Linked,
    links::{LinkRecord, LinkState, LinkStore, NewLink, token_hash},
};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    config::ILearningConfig,
    service::{self, Account, ILearningService},
};

/// The web service token.
pub const TOKEN: &str = "token";
/// The private token that lets the server open a web session to renew the
/// token. Optional: not every sign-in link carries one.
pub const PRIVATETOKEN: &str = "privatetoken";

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

type State_ = AppState<ILearningService>;

/// Per-user locks that serialize renewals, with the time of the user's last
/// autologin.
pub type RenewLocks = Arc<DashMap<Uuid, Arc<Mutex<Option<Instant>>>>>;

/// The user's iLearning link as the dashboard shows it (`LinkStatus`).
#[derive(Debug, Serialize)]
pub struct LinkStatus {
    pub status: LinkState,
    pub token_obtained_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_ok_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// Has a private token and a known expiry, so the server renews it.
    pub auto_renew: bool,
}

/// When the link's token was obtained.
fn obtained_at(record: &LinkRecord) -> DateTime<Utc> {
    record
        .metadata
        .0
        .get("token_obtained_at")
        .and_then(|at| serde_json::from_value(at.clone()).ok())
        .unwrap_or(record.created_at)
}

/// The link, or `None` when the user never linked one.
pub async fn status(core: &Core, user_id: Uuid) -> anyhow::Result<Option<LinkStatus>> {
    let Some(record) = LinkStore::new(core).load(user_id).await? else {
        return Ok(None);
    };
    Ok(Some(LinkStatus {
        status: record.status,
        token_obtained_at: obtained_at(&record),
        expires_at: record.expires_at,
        last_ok_at: record.last_ok_at,
        last_error: record.last_error.clone(),
        auto_renew: record.has_secret(PRIVATETOKEN) && record.expires_at.is_some(),
    }))
}

/// Stores the tokens from a sign-in link. A token that differs from the one
/// stored is fresh, so its expiry is known; the same token keeps the known
/// expiry, and a first link leaves it unknown (iLearning returns an existing
/// token until it expires). A link without a private token keeps the one
/// stored.
pub async fn store_linked(
    core: &Core,
    config: &ILearningConfig,
    user_id: Uuid,
    tokens: &moodle::Tokens,
) -> anyhow::Result<()> {
    // Read and write the link as one step against concurrent sign-ins.
    let lock = core.user_lock(user_id);
    let _guard = lock.lock().await;
    let store = LinkStore::new(core);
    let hash = token_hash(&tokens.token);
    let previous = store.load(user_id).await?;
    let now = Utc::now();
    let (expires_at, obtained) = match &previous {
        Some(previous) if previous.token_hash == hash => {
            (previous.expires_at, obtained_at(previous))
        }
        Some(_) => (Some(now + config.token_duration()), now),
        None => (None, now),
    };
    let privatetoken = match &tokens.privatetoken {
        Some(privatetoken) => Some(privatetoken.clone()),
        None => previous
            .as_ref()
            .filter(|previous| previous.has_secret(PRIVATETOKEN))
            .and_then(|previous| match previous.secret(core, PRIVATETOKEN) {
                Ok(privatetoken) => Some(privatetoken),
                Err(err) => {
                    tracing::warn!(%user_id, error = ?err, "dropping an unreadable private token");
                    None
                }
            }),
    };
    let mut secrets = vec![(TOKEN, tokens.token.as_str())];
    if let Some(privatetoken) = &privatetoken {
        secrets.push((PRIVATETOKEN, privatetoken));
    }
    store
        .put(
            user_id,
            &NewLink {
                secrets: &secrets,
                token_hash: hash,
                expires_at,
                metadata: json!({ "token_obtained_at": obtained }),
            },
        )
        .await
}

/// The user's account, or `None` if they never linked one. A link marked
/// `needs_relink` is still returned: iLearning has the final word.
pub async fn account(
    core: &Core,
    moodle: &moodle::Moodle,
    user_id: Uuid,
) -> anyhow::Result<Option<Linked<Account>>> {
    let Some(record) = LinkStore::new(core).load(user_id).await? else {
        return Ok(None);
    };
    let token = record.secret(core, TOKEN)?;
    let account = service::account(core, moodle.account(token), user_id);
    Ok(Some(Linked::new(account, record.token_hash.clone())))
}

/// Starts the renewal task for the lifetime of the process.
pub fn spawn_renewal(state: State_) {
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
        "SELECT l.user_id FROM links l
         WHERE l.status IN ('ok', 'renewing') AND l.expires_at IS NOT NULL AND l.expires_at < $1
           AND EXISTS (SELECT 1 FROM link_secrets s
                       WHERE s.user_id = l.user_id AND s.name = $2)",
    )
    .bind(Utc::now() + RENEW_LEAD)
    .bind(PRIVATETOKEN)
    .fetch_all(db)
    .await
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
async fn renew_user(state: &State_, user_id: Uuid) {
    let lock = state
        .service
        .renew_locks
        .entry(user_id)
        .or_default()
        .clone();
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
                "UPDATE links
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
    state: &State_,
    user_id: Uuid,
    last_autologin: &mut Option<Instant>,
) -> anyhow::Result<Renewed> {
    let store = LinkStore::new(state);
    let Some(record) = store.load(user_id).await? else {
        return Ok(Renewed::Superseded);
    };
    if !record.is_usable() || !record.has_secret(PRIVATETOKEN) {
        return Ok(Renewed::Superseded);
    }
    let Some(expires_at) = record.expires_at else {
        return Ok(Renewed::Superseded);
    };
    if expires_at - Utc::now() >= RENEW_LEAD {
        return Ok(Renewed::Superseded);
    }
    let token = record.secret(state, TOKEN)?;
    let privatetoken = record.secret(state, PRIVATETOKEN)?;
    let moodle_userid = moodle_userid(&state.db, user_id).await?;
    let old_hash = &record.token_hash;
    set_renewing(&state.db, user_id, old_hash).await?;

    let moodle = &state.service.moodle;
    let session = autologin(moodle, &token, &privatetoken, moodle_userid, last_autologin).await?;
    let mut last_touch = Instant::now();
    let mut mint_at = expires_at + MINT_DELAY;
    loop {
        // Sleep until the next mint attempt or keep-alive, whichever is first.
        let until_mint = (mint_at - Utc::now()).to_std().unwrap_or_default();
        let until_touch = TOUCH_INTERVAL.saturating_sub(last_touch.elapsed());
        if until_touch < until_mint {
            tokio::time::sleep(until_touch).await;
            moodle::renew::touch(moodle, &session).await?;
            last_touch = Instant::now();
            continue;
        }
        tokio::time::sleep(until_mint).await;
        if !still_renewing(&state.db, user_id, old_hash).await? {
            return Ok(Renewed::Superseded);
        }
        let minted = moodle::renew::mint(moodle, &session).await?;
        last_touch = Instant::now();
        if minted.token != token {
            store_renewed(state, user_id, old_hash, &minted).await?;
            return Ok(Renewed::Token);
        }
        if Utc::now() > expires_at + MINT_DEADLINE {
            sqlx::query(
                "UPDATE links SET status = 'ok', expires_at = NULL, updated_at = now()
                 WHERE user_id = $1 AND token_hash = $2",
            )
            .bind(user_id)
            .bind(old_hash)
            .execute(&state.db)
            .await?;
            return Ok(Renewed::GaveUp);
        }
        mint_at = Utc::now() + TimeDelta::from_std(MINT_RETRY)?;
    }
}

/// The user's Moodle user ID, their identity's subject.
async fn moodle_userid(db: &PgPool, user_id: Uuid) -> anyhow::Result<i64> {
    let subject: String = sqlx::query_scalar("SELECT subject FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(db)
        .await?;
    Ok(subject.parse()?)
}

/// Opens a web session, respecting iLearning's autologin rate limit.
async fn autologin(
    moodle: &moodle::Moodle,
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
        match moodle::renew::autologin(moodle, token, privatetoken, moodle_userid).await {
            Ok(session) => return Ok(session),
            Err(moodle::Error::RateLimited) if attempt < AUTOLOGIN_ATTEMPTS => continue,
            Err(err) => return Err(err.into()),
        }
    }
    unreachable!("the last attempt returns")
}

async fn set_renewing(db: &PgPool, user_id: Uuid, token_hash: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE links SET status = 'renewing', updated_at = now()
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
        "SELECT EXISTS (SELECT 1 FROM links
                        WHERE user_id = $1 AND token_hash = $2 AND status = 'renewing')",
    )
    .bind(user_id)
    .bind(token_hash)
    .fetch_one(db)
    .await
}

/// Replaces the token hashed `old_hash` with a freshly minted one, whose
/// expiry is known. A missing private token keeps the stored one.
async fn store_renewed(
    state: &State_,
    user_id: Uuid,
    old_hash: &str,
    tokens: &moodle::Tokens,
) -> anyhow::Result<()> {
    let mut secrets = vec![(TOKEN, tokens.token.as_str())];
    if let Some(privatetoken) = &tokens.privatetoken {
        secrets.push((PRIVATETOKEN, privatetoken));
    }
    let now = Utc::now();
    LinkStore::new(state)
        .replace(
            user_id,
            old_hash,
            &NewLink {
                secrets: &secrets,
                token_hash: token_hash(&tokens.token),
                expires_at: Some(now + state.service.config.token_duration()),
                metadata: json!({ "token_obtained_at": now }),
            },
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::testing;

    fn tokens(token: &str, privatetoken: Option<&str>) -> moodle::Tokens {
        moodle::Tokens {
            token: token.to_owned(),
            privatetoken: privatetoken.map(str::to_owned),
        }
    }

    async fn link(state: &State_, user_id: Uuid) -> LinkStatus {
        status(state, user_id).await.unwrap().unwrap()
    }

    #[sqlx::test(migrations = false)]
    async fn relinking_learns_the_expiry_of_fresh_tokens(db: PgPool) {
        let state = testing::state(db.clone()).await;
        let (user_id, _) = testing::sign_in(&state, 7).await;
        let config = &state.service.config;
        let store = |token: &'static str, privatetoken: Option<&'static str>| {
            let state = state.clone();
            async move {
                store_linked(
                    &state,
                    &state.service.config,
                    user_id,
                    &tokens(token, privatetoken),
                )
                .await
                .unwrap();
            }
        };
        // sign_in links a token whose expiry is unknown.
        let first = link(&state, user_id).await;
        assert_eq!(first.expires_at, None);
        assert!(!first.auto_renew);

        let a = "a".repeat(32).leak();
        store(a, Some("p")).await;
        let fresh = link(&state, user_id).await;
        let known = fresh.expires_at.expect("fresh token");
        let days = (known - Utc::now()).num_days();
        assert!(
            (config.token_duration_days - 1..=config.token_duration_days).contains(&days),
            "{days}"
        );
        assert!(fresh.auto_renew);
        assert!(fresh.token_obtained_at >= first.token_obtained_at);

        // The same token again keeps the known expiry and when it was
        // obtained; without a private token, the stored one stays.
        store(a, None).await;
        let again = link(&state, user_id).await;
        assert_eq!(again.expires_at, Some(known));
        assert_eq!(again.token_obtained_at, fresh.token_obtained_at);
        assert!(again.auto_renew);
        let record = LinkStore::new(&state).load(user_id).await.unwrap().unwrap();
        assert_eq!(record.secret(&state, PRIVATETOKEN).unwrap(), "p");
        assert_eq!(record.secret(&state, TOKEN).unwrap(), *a);

        let linked = account(&state, &state.service.moodle, user_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(linked.token_hash, Some(token_hash(a)));
        assert_eq!(linked.account.files.user_id(), user_id);

        // A rejected token still reaches iLearning, which has the final word.
        let store_ = LinkStore::new(&state);
        assert!(!store_.mark_invalid(user_id, "stale", "x").await.unwrap());
        assert_eq!(link(&state, user_id).await.status, LinkState::Ok);
        assert!(
            store_
                .mark_invalid(user_id, &token_hash(a), "rejected")
                .await
                .unwrap()
        );
        let flagged = link(&state, user_id).await;
        assert_eq!(flagged.status, LinkState::NeedsRelink);
        assert!(flagged.last_error.is_some());
        assert!(
            account(&state, &state.service.moodle, user_id)
                .await
                .unwrap()
                .is_some()
        );

        // Linking again clears the flag.
        store("b".repeat(32).leak(), None).await;
        let relinked = link(&state, user_id).await;
        assert_eq!(relinked.status, LinkState::Ok);
        assert_eq!(relinked.last_error, None);
        assert!(relinked.auto_renew, "the private token is kept");
    }

    #[sqlx::test(migrations = false)]
    async fn only_soon_expiring_renewable_links_are_due(db: PgPool) {
        let state = testing::state(db.clone()).await;
        let (due, _) = testing::sign_in(&state, 1).await;
        let (later, _) = testing::sign_in(&state, 2).await;
        let (no_privatetoken, _) = testing::sign_in(&state, 3).await;
        let (unknown, _) = testing::sign_in(&state, 4).await;
        let (relink, _) = testing::sign_in(&state, 5).await;
        for user_id in [due, later, unknown, relink] {
            store_linked(
                &state,
                &state.service.config,
                user_id,
                &tokens(&format!("{user_id}"), Some("p")),
            )
            .await
            .unwrap();
        }
        for (user_id, minutes) in [
            (due, 30),
            (later, 60 * 24),
            (no_privatetoken, 30),
            (relink, 30),
        ] {
            sqlx::query(
                "UPDATE links SET expires_at = now() + make_interval(mins => $2) WHERE user_id = $1",
            )
            .bind(user_id)
            .bind(minutes)
            .execute(&db)
            .await
            .unwrap();
        }
        sqlx::query("UPDATE links SET expires_at = NULL WHERE user_id = $1")
            .bind(unknown)
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("UPDATE links SET status = 'needs_relink' WHERE user_id = $1")
            .bind(relink)
            .execute(&db)
            .await
            .unwrap();
        assert_eq!(due_for_renewal(&db).await.unwrap(), vec![due]);
        assert_eq!(moodle_userid(&db, due).await.unwrap(), 1);
    }

    #[sqlx::test(migrations = false)]
    async fn renewed_tokens_replace_only_the_one_renewed(db: PgPool) {
        let state = testing::state(db.clone()).await;
        let (user_id, _) = testing::sign_in(&state, 1).await;
        store_linked(
            &state,
            &state.service.config,
            user_id,
            &tokens("old", Some("p")),
        )
        .await
        .unwrap();
        let old = token_hash("old");
        set_renewing(&db, user_id, &old).await.unwrap();
        assert!(still_renewing(&db, user_id, &old).await.unwrap());
        assert_eq!(link(&state, user_id).await.status, LinkState::Renewing);

        store_renewed(&state, user_id, "stale", &tokens("other", None))
            .await
            .unwrap();
        assert!(still_renewing(&db, user_id, &old).await.unwrap());

        store_renewed(&state, user_id, &old, &tokens("new", None))
            .await
            .unwrap();
        assert!(!still_renewing(&db, user_id, &old).await.unwrap());
        let renewed = link(&state, user_id).await;
        assert_eq!(renewed.status, LinkState::Ok);
        assert!(renewed.expires_at.unwrap() > Utc::now() + TimeDelta::days(80));
        assert!(renewed.auto_renew);
        let record = LinkStore::new(&state).load(user_id).await.unwrap().unwrap();
        assert_eq!(record.token_hash, token_hash("new"));
        assert_eq!(record.secret(&state, TOKEN).unwrap(), "new");
        assert_eq!(record.secret(&state, PRIVATETOKEN).unwrap(), "p");
    }
}
