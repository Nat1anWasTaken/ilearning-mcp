//! iLearning as an mcp-backbone service.

use axum::{
    Router,
    routing::{get, post},
};
use mcp_backbone::{Access, AppState, Core, Failure, Info, NoScope, Service, Tool};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    auth,
    config::ILearningConfig,
    files,
    links::{self, RenewLocks},
    tools,
};

pub static INFO: Info = Info {
    slug: "ilearning",
    name: "iLearning",
    version: env!("CARGO_PKG_VERSION"),
    // Served from the dashboard's static files (web/public).
    icon: Some("/ilearning.png"),
    // Kept from before the framework, so signed-in browsers stay signed in.
    cookie_prefix: Some("ilmcp"),
    instructions: INSTRUCTIONS,
    allowlist_env: &["ALLOWED_MOODLE_USERNAMES"],
    // Room for a `submit_assignment` with a few files, base64-encoded.
    max_request_bytes: 64 * 1024 * 1024,
};

const INSTRUCTIONS: &str = "\
These tools act as the user's own account on i-learning 2.0, the Moodle learning platform of \
Chung Yuan Christian University (CYCU). Use them only as the user asks.

Start with list_courses (course IDs), get_upcoming_deadlines (due work across courses) or \
get_notifications. get_course_contents lists a course's sections and modules (cmid, modname, \
file URLs); get_module reads a page/url/resource/label module; read_file extracts the text of \
a course file (PDF, Word, PowerPoint, Excel, text), returns images, or gives a short-lived \
download link for other files; read_file with mode=\"link\" gives that link for any file, to \
get its original bytes, and download_file returns those bytes inline (base64, in parts) for \
agents without network access. Assignments: list_assignments, then get_assignment_status for \
one assignment's submission and feedback. Forums: list_forum_discussions, then \
get_discussion_posts. get_grades without course_id gives the overview, with it the course's \
grade items. Messages: list_conversations, get_conversation_messages.

Write tools act as the student and are visible to teachers and classmates: \
submit_assignment, reply_forum_post, create_forum_discussion, send_message and \
mark_notifications_read. Confirm with the user before calling them. submit_assignment with \
submit_for_grading=true finalizes the submission so it can no longer be edited.

If a tool says the iLearning link expired, ask the user to re-link their account in the \
dashboard.";

/// What tools act with: the user's Moodle account, and what it takes to
/// hand out download links for files that cannot be shown inline.
pub struct Account {
    pub moodle: moodle::Account,
    pub files: files::Signer,
}

pub struct ILearningService {
    pub moodle: moodle::Moodle,
    pub config: ILearningConfig,
    /// Serializes token renewals per user, with the time of the user's last
    /// autologin.
    pub renew_locks: RenewLocks,
}

impl ILearningService {
    pub fn new(config: ILearningConfig) -> Self {
        Self {
            moodle: moodle::Moodle::new(config.ilearning_url.clone()),
            config,
            renew_locks: RenewLocks::default(),
        }
    }
}

impl Service for ILearningService {
    type Account = Account;
    type Error = moodle::Error;
    type Scope = NoScope;
    type Target = ();

    fn info(&self) -> &'static Info {
        &INFO
    }

    fn tools(&self) -> Vec<Tool<Self>> {
        tools::all()
    }

    async fn account(&self, core: &Core, user_id: Uuid) -> anyhow::Result<Access<Account>> {
        // A link flagged `needs_relink` is still tried: iLearning has the
        // final word.
        Ok(match links::account(core, &self.moodle, user_id).await? {
            Some(linked) => Access::Ready(linked),
            None => Access::NotLinked,
        })
    }

    fn classify(&self, error: &moodle::Error) -> Failure {
        match error {
            moodle::Error::InvalidToken => Failure::Unauthorized,
            _ => Failure::Other,
        }
    }

    async fn link_status(&self, core: &Core, user_id: Uuid) -> anyhow::Result<Value> {
        Ok(serde_json::to_value(links::status(core, user_id).await?)?)
    }

    fn relink_message(&self, core: &Core) -> String {
        format!(
            "The user's iLearning sign-in has expired or was revoked. Ask the user to re-link \
             their iLearning account at {}/ and then try again.",
            core.config.public_url
        )
    }

    fn not_linked_message(&self, core: &Core) -> String {
        self.relink_message(core)
    }

    fn routes(&self) -> Router<AppState<Self>> {
        Router::new()
            .route("/auth/ilearning/start", post(auth::start))
            .route("/auth/ilearning/complete", post(auth::complete))
            .route("/files/{signed}", get(files::download))
    }

    fn background(&self, state: &AppState<Self>) {
        links::spawn_renewal(state.clone());
    }
}

/// The account `links` hands out, bound to `user_id`.
pub fn account(core: &Core, moodle: moodle::Account, user_id: Uuid) -> Account {
    Account {
        moodle,
        files: files::Signer::new(&core.config, user_id),
    }
}
