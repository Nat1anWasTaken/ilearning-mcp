# Architecture

Remote MCP server that lets AI agents (claude.ai, ChatGPT, Claude Code, …) use
CYCU i-learning 2.0 (Moodle 4.5.8, `https://ilearning.cycu.edu.tw`) on behalf
of each signed-in student. Multi-user. Students sign in to the dashboard by
linking their iLearning account; the dashboard sets each connected agent's
permissions, holds the outbox of write calls waiting for approval, and shows
a log of every tool call.

The OAuth server for MCP clients, dashboard sessions, per-session
permissions (enabled, read-only, allowed tools, outbox), the tool pipeline,
the call log and the dashboard itself come from
[mcp-backbone](https://github.com/Nat1anWasTaken/mcp-backbone): the
`mcp-backbone` crate and the `@nat1an/mcp-backbone-dashboard` package. This
repository holds what is iLearning's. The framework's own documentation
covers its parts; this one covers iLearning.

One Rust binary serves everything on one origin (`PUBLIC_URL`, prod
`https://ilearning-mcp.nath.tw`):

| Path | What | Auth |
| --- | --- | --- |
| `/mcp` | MCP Streamable HTTP endpoint (the framework) | OAuth bearer |
| `/.well-known/*`, `/oauth/*` | OAuth 2.1 authorization server (the framework) | public / client auth |
| `/auth/ilearning/*` | iLearning sign-in (the link flow) | pre-auth cookie `ilmcp_link` |
| `/auth/logout`, `/api/*` | dashboard JSON API (the framework's) | dashboard cookie `ilmcp_session` |
| `/files/{signed}` | short-lived file download proxy for agents | HMAC-signed URL |
| everything else | built dashboard (`web/dist`, SPA fallback) | — |

```
crates/moodle   Moodle layer: REST client, link parsing, token renewal,
                files, agent-friendly operations + request types (JsonSchema).
                No HTTP-server/OAuth/MCP/DB knowledge.
crates/server   the mcp-backbone service (binary `ilearning-mcp`):
  service.rs    the `Service` impl: info + instructions, account, error classes,
                link status, routes, background renewal
  tools.rs      the tool registry
  auth.rs       iLearning sign-in (sign-in = link)
  links.rs      iLearning tokens in the framework's link store + the renewal task
  files.rs      file results for agents, and the /files/{signed} proxy
  config.rs     iLearning settings; testing.rs, a Moodle mock
migrations/     legacy/ (the schema before mcp-backbone) and adopt/ (its one-time adoption)
web/            the dashboard: the package, plus iLearning's login steps,
                overview cards, link health and icons page
```

## iLearning facts (verified 2026-10-15)

- Moodle 4.5.8+, mobile web service `moodle_mobile_app` enabled, 440 functions.
- Login is Microsoft 365 SSO only (OAuth2 issuer id `1`). No password login,
  so `/login/token.php` is unusable.
- `GET /admin/tool/mobile/launch.php?service=moodle_mobile_app&passport=<p>&urlscheme=moodlemobile&confirmed=1&oauthsso=1`
  runs the M365 login if needed, then renders an HTML page whose link
  `<a id="launchapp" href="moodlemobile://token=<base64>">` (text
  「若應用程式沒有自動開啟，請點選這裡」, English "click here to launch the app")
  carries `base64(md5(wwwroot + passport) ":::" token [":::" privatetoken])`.
  With `oauthsso=1` the private token (64 chars) is included. The token is 32
  hex chars. Without `confirmed=1` it 302s to `moodlemobile://` instead.
- Tokens expire at `timecreated + tokenduration` (Moodle default 12 weeks;
  real value unknown). `launch.php` returns the **existing** token until it
  expires, then creates a new one.
- REST: `POST /webservice/rest/server.php?moodlewsrestformat=json` with form
  fields `wstoken`, `wsfunction`, and PHP-style flattened params
  (`courseids[0]=1`, `options[0][name]=x`). Errors come back as HTTP 200 JSON
  `{"exception": "...", "errorcode": "...", "message": "..."}`; expired or
  revoked token → errorcode `invalidtoken` (also treat `accessexception` on
  token as invalid).
- File URLs from WS responses (`fileurl`) download with `?token=<token>`
  appended (`/webservice/pluginfile.php/...`). Uploads: multipart
  `POST /webservice/upload.php` (`token`, `filearea=draft`, `itemid=0`,
  file fields) → JSON array with `itemid`.
- Renewal: `tool_mobile_get_autologin_key` (form param `privatetoken`, never in
  the query string; User-Agent must contain `MoodleMobile`; at most once per
  6 min) → `{key, autologinurl}`; `GET autologinurl?userid=<id>&key=<key>`
  (no redirect follow) sets `MoodleSession` cookie; then `launch.php` with
  `confirmed=1` (no oauthsso) in that cookie session returns the link page →
  token + privatetoken. Session idle timeout is 3600 s; keep alive with AJAX
  `core_session_touch` (`POST /lib/ajax/service.php?sesskey=<sesskey>`, body
  `[{"index":0,"methodname":"core_session_touch","args":{}}]`; sesskey is in
  any HTML page as `"sesskey":"…"`).
- **The site limits concurrent web logins to 1**: an autologin logs the user
  out of iLearning in their browser. Token-based REST calls do not. So the
  server only creates a web session for renewal, around token expiry.
- `local_mail` (iLearning internal mail) is disabled for the mobile service.

## Linking / sign-in (the iLearning account *is* the user)

1. `POST /auth/ilearning/start {next}` starts a sign-in attempt
   (`mcp_backbone::auth::attempts`) whose sealed payload is a random
   `passport` (hex, 16 chars), with `next` (a same-origin path, else `/`) and
   a 15-min TTL. It sets the pre-auth cookie `ilmcp_link` (path
   `/auth/ilearning`, stored hashed) and returns `{attempt_id, launch_url,
   expires_at}`.
2. The dashboard tells the user to open `launch_url`, sign in with M365, then
   right-click / long-press 「若應用程式沒有自動開啟，請點選這裡」 → copy link, and
   paste it back.
3. `POST /auth/ilearning/complete {attempt_id, pasted}`: reads the attempt
   for this browser's cookie without consuming it (a mistyped paste can be
   retried), `moodle::link::parse(wwwroot, passport, pasted)` (verifies the
   md5 signature), `site_info` with the token, the allowlist
   (`ALLOWED_USERS`, or `ALLOWED_MOODLE_USERNAMES`; comma-separated
   usernames, empty = everyone), and, when already signed in, that it is the
   same Moodle user. Then it consumes the attempt (single use), upserts the
   user (`subject` = Moodle user ID, `login` = username, `name` = full name,
   `avatar_url` = picture), stores the tokens (`links::store_linked`, below),
   starts a dashboard session (`ilmcp_session`), clears `ilmcp_link` and
   returns `{redirect_to: next}`.

   Errors: `link_expired` 410 (no cookie, unknown, used or lapsed attempt),
   `invalid_link` 400 (not this attempt's link, or iLearning refuses the
   token), `moodle_error` 502, `not_allowed` 403, `wrong_account` 409.

## Persistence / renewal (`links.rs`)

- The framework's `links` row holds `token_hash` (hex SHA-256 of the token),
  `expires_at` (NULL = unknown), `status` (`ok | renewing | needs_relink`),
  `last_ok_at`, `last_error`, and as metadata `token_obtained_at`. The
  secrets are `token` and, when the link carried one, `privatetoken`
  (AES-256-GCM, AAD = user ID).
- `store_linked`: a token that differs from the stored one is fresh, so
  `expires_at = now + TOKEN_DURATION_DAYS (default 84)` and
  `token_obtained_at = now`; the same token keeps both; a first link leaves
  the expiry unknown (iLearning returns an existing token until it expires).
  A link without a private token keeps the stored one.
- Normal operation uses only the token. The framework bumps `last_ok_at` on
  success, and on `invalidtoken` (`Failure::Unauthorized`) flags the link
  `needs_relink` and tells the agent to have the user re-link at
  `<PUBLIC_URL>/`. A flagged link is still tried (`Service::account` returns
  it as ready): iLearning has the final word. There is no on-demand refresh.
- The renewal task (started by `Service::background`, every 5 min): when
  `expires_at` is known, a private token exists, and
  `expires_at - now < 40 min`: status `renewing`, autologin (this logs the
  user's browser out of iLearning, once; the Moodle user ID is
  `users.subject`), keep the session alive (touch every 10 min), and once
  `now > expires_at + 1 min` mint via `launch.php`. New token → replace it
  (`LinkStore::replace`, only if the link still holds the old one),
  `expires_at = now + duration`, status `ok`. Same token still → keep
  waiting up to 2 h past `expires_at`, then give up: set `expires_at = NULL`
  (unknown) and status `ok`, so it falls back to re-link on the eventual
  `invalidtoken`. Failure → `needs_relink`.
- A per-user mutex in the service (`RenewLocks`, a DashMap holding the last
  autologin time) serializes renewals. Respect the 6-minute autologin limit.

## Tools

The registry is `crates/server/src/tools.rs`; each tool has a dashboard
group (profile, courses, assignments, grades, forums, messages, files) and
an `Effect`: `R` Read, `A` Additive, `I` Idempotent, `D` Destructive. Write
tools (A, I, D) obey a session's read-only switch and, when the user turns it
on, wait in the outbox for approval. Write tools also name the arguments
outbox notifications describe them with (`Tool::summary`).

The request types live in `moodle::types` (`#[derive(Deserialize, Serialize,
JsonSchema)]`, doc comments become descriptions). Each op is an
`Account` method returning `Result<serde_json::Value>` with compact,
agent-friendly JSON (strip HTML to text where large, drop noise fields,
include ids needed for follow-up calls, unix times → RFC 3339).

| Tool | Effect | `Account` method → Moodle functions |
| --- | --- | --- |
| `get_profile` | R | `get_profile` → `core_webservice_get_site_info` |
| `list_courses` | R | `list_courses(ListCoursesRequest{classification: inprogress\|past\|future\|all})` → `core_course_get_enrolled_courses_by_timeline_classification` |
| `get_course_contents` | R | `get_course_contents(CourseRequest{course_id})` → `core_course_get_contents` (sections → modules with id, modname, name, url, dates, file list with `fileurl`) |
| `get_upcoming_deadlines` | R | `get_upcoming_deadlines(DeadlinesRequest{days?: default 14, course_id?})` → `core_calendar_get_action_events_by_timesort` / `..._by_course` |
| `list_assignments` | R | `list_assignments(CourseIdsRequest{course_ids?})` → `mod_assign_get_assignments` |
| `get_assignment_status` | R | `get_assignment_status(AssignmentRequest{assignment_id})` → `mod_assign_get_submission_status` |
| `get_grades` | R | `get_grades(GradesRequest{course_id?})` → none: `gradereport_overview_get_course_grades`; course: `gradereport_user_get_grade_items` |
| `list_forum_discussions` | R | `list_forum_discussions(ForumRequest{forum_id, page?})` → `mod_forum_get_forum_discussions` |
| `get_discussion_posts` | R | `get_discussion_posts(DiscussionRequest{discussion_id})` → `mod_forum_get_discussion_posts` |
| `get_notifications` | R | `get_notifications(NotificationsRequest{limit?, unread_only?})` → `message_popup_get_popup_notifications` |
| `list_conversations` | R | `list_conversations(ConversationsRequest{limit?})` → `core_message_get_conversations` |
| `get_conversation_messages` | R | `get_conversation_messages(ConversationRequest{conversation_id, limit?})` → `core_message_get_conversation_messages` |
| `get_module` | R | `get_module(ModuleRequest{course_id, cmid})` → page/url/resource/label body: `mod_page_get_pages_by_courses`, `mod_url_get_urls_by_courses`, `mod_resource_get_resources_by_courses` (filter by `coursemodule`) |
| `read_file` | R | `read_file(ReadFileRequest{file_url, max_chars?, mode: text|link = text})` → `FileContent` (see below) |
| `download_file` | R | `download_file(DownloadFileRequest{file_url, offset: u64 = 0, max_bytes: u32 = 1 MiB (≤ 4 MiB)})` → `FileChunk` (see below) |
| `submit_assignment` | D | `submit_assignment(SubmitAssignmentRequest{assignment_id, online_text?, files?: [{filename, text?, base64?}], submit_for_grading: bool = false})` → `upload.php` + `mod_assign_save_submission` (+ `mod_assign_submit_for_grading` with `acceptsubmissionstatement=1`) |
| `reply_forum_post` | A | `reply_forum_post(ReplyRequest{post_id, subject?, message})` → `mod_forum_add_discussion_post` |
| `create_forum_discussion` | A | `create_forum_discussion(NewDiscussionRequest{forum_id, subject, message})` → `mod_forum_add_discussion` |
| `send_message` | A | `send_message(SendMessageRequest{to_user_id, text})` → `core_message_send_instant_messages` |
| `mark_notifications_read` | I | `mark_notifications_read()` → `core_message_mark_all_notifications_as_read` (useridto = self) |

Not provided: quiz attempts, iLearning internal mail.

`read_file` only accepts URLs on the configured wwwroot. `FileContent`:
`Text {filename, mime, text, truncated}` for pdf (pdf-extract), docx/pptx/xlsx
(zip + quick-xml), txt/md/csv/html (html → text); `Image {filename, mime,
bytes}` for png/jpeg/gif/webp ≤ 5 MB; `Binary {filename, mime, size,
file_url}` otherwise (e.g. legacy .ppt/.doc), and for every file with
`mode: link` (the original bytes, without downloading the body). The server turns `Binary` into a
link `<PUBLIC_URL>/files/<signed>` (HMAC with a key derived from
`ENCRYPTION_KEY`, payload `{user_id, file_url, exp: now+1h}`) that streams via
`Account::download_response`, through `AppState::call`, so a rejected token
flags the link as a tool call would. `submit_assignment`'s request is
wrapped (`tools::SubmitAssignment`) so the call log and outbox list show file
bodies (`base64`) by length only, while the outbox keeps them in full to run
the call.

`download_file` is for agents that cannot fetch that link: it asks iLearning
for `Range: bytes=offset-(offset+max_bytes)` (or skips `offset` bytes of a 200
response) and returns `FileChunk {filename, mime, file_url, size?, offset,
bytes, next_offset?}`; the extra byte tells whether `next_offset` is set. The
server sends a JSON header and the bytes as an embedded base64 blob resource
(`Output::Content`, built in `files.rs`); the call log keeps only the header.

### `crates/moodle` public API (contract between the crates)

```rust
pub struct Moodle { /* reqwest::Client + wwwroot: Url */ }       // Clone, cheap
impl Moodle {
    pub fn new(wwwroot: Url) -> Self;
    pub fn wwwroot(&self) -> &Url;
    pub fn account(&self, token: String) -> Account;              // Account: Clone
    pub async fn renew(&self, creds: &RenewCreds, hooks: ...)     // see renew module
}
impl Account {
    pub async fn call(&self, wsfunction: &str, params: serde_json::Value) -> Result<serde_json::Value, Error>;
    pub async fn site_info(&self) -> Result<SiteInfo, Error>;     // userid, username, fullname, userpictureurl, siteurl
    pub async fn download(&self, file_url: &str) -> Result<Download, Error>; // {filename, mime, bytes}
    pub async fn upload_draft(&self, files: Vec<UploadFile>) -> Result<i64, Error>; // draft itemid
    // + one method per tool in the table above
}
pub mod link {
    pub fn new_passport() -> String;
    pub fn launch_url(wwwroot: &Url, passport: &str, oauthsso: bool) -> Url;
    pub fn parse(wwwroot: &Url, passport: &str, pasted: &str) -> Result<Tokens, Error>; // accepts the moodlemobile:// URL, or "token=..." , or raw base64; trims whitespace
}
pub struct Tokens { pub token: String, pub privatetoken: Option<String> }
pub mod renew {
    pub struct WebSession { /* cookie jar */ }
    pub async fn autologin(m: &Moodle, token: &str, privatetoken: &str, userid: i64) -> Result<WebSession, Error>;
    pub async fn touch(m: &Moodle, s: &WebSession) -> Result<(), Error>;
    pub async fn mint(m: &Moodle, s: &WebSession) -> Result<Tokens, Error>; // launch.php confirmed=1 in session
}
pub enum Error { InvalidToken, Moodle { errorcode: String, message: String }, InvalidLink(String),
                 RateLimited, Http(reqwest::Error), Decode(String), NotAllowed(String) }
pub enum FileContent { Text{..}, Image{..}, Binary{..} }
```

## Database

The framework's schema (its `0001_core.sql`); iLearning adds no tables of
its own. A database created before mcp-backbone (`_sqlx_migrations` at
version 1, `migrations/legacy/0001_init.sql`) is adopted once at startup, in
one transaction (`migrations/adopt`):

- `users`: `moodle_userid` becomes `subject` (as text), `username` `login`,
  `fullname` `name`; `email` and `profile` are added. User IDs are kept, so
  dashboard sessions (and `ilmcp_session` cookies) stay valid.
- `agent_sessions` gain the framework's permission columns with their
  defaults: enabled, not read-only, every tool, no outbox.
- `tool_calls` gain the outbox's statuses and columns; every row is kept.
- `ilearning_links` becomes a link with the same sealed bytes as the secrets
  `token` and `privatetoken`, `token_obtained_at` moving into its metadata.
- The legacy `link_attempts` is dropped (attempts live 15 minutes), and
  `_sqlx_migrations` is renamed `_legacy_sqlx_migrations`.

A test (`adoption_tests` in `main.rs`) adopts a seeded legacy database,
compares its schema with a fresh one and opens the tokens.

## Dashboard API contract

The framework's API (`/api/me`, `/api/meta`, sessions, tools, tool calls and
the outbox, the consent request, push) is documented with mcp-backbone.
What is iLearning's:

```ts
// GET /api/me → { user, link }. user.subject is the Moodle user ID (decimal
// string), user.login the username, user.name the full name.
type LinkStatus = {
  status: "ok" | "renewing" | "needs_relink"
  token_obtained_at: string
  expires_at: string | null        // null = unknown
  last_ok_at: string | null
  last_error: string | null
  auto_renew: boolean              // has a private token and a known expiry
} | null                           // never linked
type LinkStart = { attempt_id: string; launch_url: string; expires_at: string }
```

| Method & path | Body | Response |
| --- | --- | --- |
| `POST /auth/ilearning/start` | `{ next?: string }` | `LinkStart` (sets `ilmcp_link`; works signed in or out) |
| `POST /auth/ilearning/complete` | `{ attempt_id, pasted }` | `{ redirect_to }` (sets `ilmcp_session`); errors above |
| `GET /files/{signed}` | | the file, streamed; 403 bad or expired link, 404 not linked, 410 token rejected, 502 iLearning failed |

The dashboard routes are the package's, plus `/icons` (the server icon in
several sizes, from `web/public/icon-files`).

## Config (env)

The framework's: `PUBLIC_URL`, `BIND_ADDR` (default `0.0.0.0:8080`),
`DATABASE_URL`, `ENCRYPTION_KEY` (base64 32 bytes; seals the tokens and keys
the file links), `ALLOWED_USERS` (or the older `ALLOWED_MOODLE_USERNAMES`),
`VAPID_SUBJECT`, `WEB_DIST`, `RUST_LOG`. iLearning's: `ILEARNING_URL`
(default `https://ilearning.cycu.edu.tw`), `TOKEN_DURATION_DAYS` (default
84). Prod compose adds `POSTGRES_PASSWORD`. Dev Postgres on host port
**5443**.

For local development against a checkout of mcp-backbone, an untracked
`.cargo/config.toml` patches the crate to `../mcp-backbone/crates/mcp-backbone`.

## Conventions

Runtime-checked sqlx queries (no compile-time macros), only SHA-256 hashes
of our own secrets stored, `//!` doc comment atop every module, `AppError`
for dashboard errors, tool failures as tool results, conventional commits.
Never log Moodle tokens, private tokens, or pasted link contents.
