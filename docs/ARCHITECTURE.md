# Architecture

Remote MCP server that lets AI agents (claude.ai, ChatGPT, Claude Code, …) use
CYCU i-learning 2.0 (Moodle 4.5.8, `https://ilearning.cycu.edu.tw`) on behalf
of each signed-in student. Multi-user. Structure mirrors
`~/Developments/telegram-mcp` (read its `docs/ARCHITECTURE.md` and code for any
pattern not spelled out here).

One Rust binary serves everything on one origin (`PUBLIC_URL`, prod
`https://ilearning-mcp.nath.tw`):

| Path | What | Auth |
| --- | --- | --- |
| `/mcp` | MCP Streamable HTTP endpoint (rmcp, stateless, JSON responses) | OAuth bearer |
| `/.well-known/*`, `/oauth/*` | OAuth 2.1 authorization server (copied from telegram-mcp) | public / client auth |
| `/auth/*`, `/api/*` | iLearning sign-in and dashboard JSON API | dashboard cookie `ilmcp_session` |
| `/files/{signed}` | short-lived file download proxy for agents | HMAC-signed URL |
| everything else | built React dashboard (`web/dist`, SPA fallback) | — |

```
crates/moodle   Moodle layer: REST client, link parsing, token renewal,
                files, agent-friendly operations + request types (JsonSchema).
                No HTTP-server/OAuth/MCP/DB knowledge.
crates/server   axum app (binary `ilearning-mcp`):
  auth/         iLearning sign-in (link flow), dashboard cookie sessions, CurrentUser
  oauth/        authorization server + bearer middleware (from telegram-mcp)
  mcp/          rmcp ServerHandler, tool registry, call pipeline, call log
  api/          dashboard JSON API
  links.rs      per-user iLearning token storage (sealed) + renewal task
  files.rs      /files/{signed} proxy
  web.rs        static SPA serving
migrations/     sqlx migrations (applied at startup)
web/            Vite + React 19 + shadcn (base-nova / Base UI) dashboard
```

There is **no permission/approval layer** (unlike telegram-mcp): every
authorized session may call every tool. Clients do their own confirmation
using the tools' `readOnlyHint`/`destructiveHint` annotations.

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

1. `POST /auth/ilearning/start {next}` creates a `link_attempts` row with a
   random `passport` (hex, 16 chars) and 15-min TTL, binds it to a pre-auth
   cookie `ilmcp_link` (random, hash stored), returns `{attempt_id, launch_url,
   expires_at}`.
2. The dashboard tells the user to open `launch_url`, sign in with M365, then
   right-click / long-press 「若應用程式沒有自動開啟，請點選這裡」 → copy link, and
   paste it back.
3. `POST /auth/ilearning/complete {attempt_id, pasted}`: check the attempt +
   cookie, `moodle::link::parse(wwwroot, passport, pasted)` (verifies md5
   signature), `site_info` with the token, allowlist check
   (`ALLOWED_MOODLE_USERNAMES`, comma-separated, empty = everyone), upsert
   `users` by `moodle_userid`, upsert `ilearning_links` (sealed token and
   private token; if the token differs from the previously stored one, the
   token is fresh → `expires_at = now + TOKEN_DURATION_DAYS (default 84)`,
   otherwise keep the known `expires_at`, or NULL for a first link), create a
   dashboard session cookie, delete the attempt, return `{redirect_to: next ||
   "/"}`.

## Persistence / renewal (`links.rs` background task, every 5 min)

- Normal operation uses only the token. Every successful call bumps
  `last_ok_at`. On `invalidtoken`, status → `needs_relink` (and the tool
  returns an error telling the agent to have the user re-link at
  `<PUBLIC_URL>/`).
- When `expires_at` is known, a private token exists, and
  `expires_at - now < 40 min`: status `renewing`, autologin (this logs the
  user's browser out of iLearning, once), keep the session alive (touch every
  10 min), and once `now > expires_at + 1 min` mint via `launch.php`. New
  token → store it, `expires_at = now + duration`, status `ok`. Same token
  still → keep waiting up to 2 h past `expires_at`, then give up: set
  `expires_at = NULL` (unknown) and status `ok`, so it falls back to re-link
  on the eventual `invalidtoken`. Failure → `needs_relink`.
- A per-user mutex (DashMap) serializes renewals. Respect the 6-minute
  autologin limit.

## Tools

The request types live in `moodle::types` (`#[derive(Deserialize, Serialize,
JsonSchema)]`, doc comments become descriptions). Each op is an
`Account` method returning `Result<serde_json::Value>` with compact,
agent-friendly JSON (strip HTML to text where large, drop noise fields,
include ids needed for follow-up calls, unix times → RFC 3339).

| Tool | Kind | `Account` method → Moodle functions |
| --- | --- | --- |
| `get_profile` | read | `get_profile` → `core_webservice_get_site_info` |
| `list_courses` | read | `list_courses(ListCoursesRequest{classification: inprogress\|past\|future\|all})` → `core_course_get_enrolled_courses_by_timeline_classification` |
| `get_course_contents` | read | `get_course_contents(CourseRequest{course_id})` → `core_course_get_contents` (sections → modules with id, modname, name, url, dates, file list with `fileurl`) |
| `get_upcoming_deadlines` | read | `get_upcoming_deadlines(DeadlinesRequest{days?: default 14, course_id?})` → `core_calendar_get_action_events_by_timesort` / `..._by_course` |
| `list_assignments` | read | `list_assignments(CourseIdsRequest{course_ids?})` → `mod_assign_get_assignments` |
| `get_assignment_status` | read | `get_assignment_status(AssignmentRequest{assignment_id})` → `mod_assign_get_submission_status` |
| `get_grades` | read | `get_grades(GradesRequest{course_id?})` → none: `gradereport_overview_get_course_grades`; course: `gradereport_user_get_grade_items` |
| `list_forum_discussions` | read | `list_forum_discussions(ForumRequest{forum_id, page?})` → `mod_forum_get_forum_discussions` |
| `get_discussion_posts` | read | `get_discussion_posts(DiscussionRequest{discussion_id})` → `mod_forum_get_discussion_posts` |
| `get_notifications` | read | `get_notifications(NotificationsRequest{limit?, unread_only?})` → `message_popup_get_popup_notifications` |
| `list_conversations` | read | `list_conversations(ConversationsRequest{limit?})` → `core_message_get_conversations` |
| `get_conversation_messages` | read | `get_conversation_messages(ConversationRequest{conversation_id, limit?})` → `core_message_get_conversation_messages` |
| `get_module` | read | `get_module(ModuleRequest{course_id, cmid})` → page/url/resource/label body: `mod_page_get_pages_by_courses`, `mod_url_get_urls_by_courses`, `mod_resource_get_resources_by_courses` (filter by `coursemodule`) |
| `read_file` | read | `read_file(ReadFileRequest{file_url, max_chars?, mode: text|link = text})` → `FileContent` (see below) |
| `submit_assignment` | write | `submit_assignment(SubmitAssignmentRequest{assignment_id, online_text?, files?: [{filename, text?, base64?}], submit_for_grading: bool = false})` → `upload.php` + `mod_assign_save_submission` (+ `mod_assign_submit_for_grading` with `acceptsubmissionstatement=1`) |
| `reply_forum_post` | write | `reply_forum_post(ReplyRequest{post_id, subject?, message})` → `mod_forum_add_discussion_post` |
| `create_forum_discussion` | write | `create_forum_discussion(NewDiscussionRequest{forum_id, subject, message})` → `mod_forum_add_discussion` |
| `send_message` | write | `send_message(SendMessageRequest{to_user_id, text})` → `core_message_send_instant_messages` |
| `mark_notifications_read` | write | `mark_notifications_read()` → `core_message_mark_all_notifications_as_read` (useridto = self) |

Not provided: quiz attempts, iLearning internal mail.

`read_file` only accepts URLs on the configured wwwroot. `FileContent`:
`Text {filename, mime, text, truncated}` for pdf (pdf-extract), docx/pptx/xlsx
(zip + quick-xml), txt/md/csv/html (html → text); `Image {filename, mime,
bytes}` for png/jpeg/gif/webp ≤ 5 MB; `Binary {filename, mime, size,
file_url}` otherwise (e.g. legacy .ppt/.doc), and for every file with
`mode: link` (the original bytes, without downloading the body). The server turns `Binary` into a
link `<PUBLIC_URL>/files/<signed>` (HMAC with a key derived from
`ENCRYPTION_KEY`, payload `{user_id, file_url, exp: now+1h}`) that streams via
`Account::download`.

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

## Database (Postgres 17, `migrations/0001_init.sql`)

Copy telegram-mcp's `dashboard_sessions`, `oauth_clients`, `oauth_requests`,
`oauth_codes`, `oauth_access_tokens`, `oauth_refresh_tokens` as is. Changed /
new tables:

- `users(id uuid pk, moodle_userid bigint unique not null, username text not
  null, fullname text not null, avatar_url text, created_at, last_login_at)`
- `ilearning_links(user_id uuid pk → users, token_sealed bytea, token_hash
  text, privatetoken_sealed bytea null, token_obtained_at timestamptz,
  expires_at timestamptz null, status text check in ('ok','renewing',
  'needs_relink'), last_ok_at timestamptz null, last_error text null,
  updated_at)` — sealed with AES-256-GCM, associated data = user_id.
- `link_attempts(id uuid pk, passport text, preauth_hash text, next text null,
  created_at, expires_at)`
- `agent_sessions(id uuid pk, user_id, client_id, name text, created_at,
  last_used_at, revoked_at)` — no permission columns.
- `tool_calls(id uuid pk, user_id, session_id, tool text, arguments jsonb,
  status text check in ('running','ok','error'), result jsonb null, error text
  null, duration_ms int null, created_at)` — arguments/results bounded and
  redacted like telegram-mcp (`base64` file bodies and long strings never
  stored).

## Dashboard API contract

All JSON, snake_case, RFC 3339 times, UUID strings. `401
{"error":"unauthorized"}` when signed out. Errors `{"error", "message"}`.
Mutating requests must be JSON and same-origin (telegram-mcp CSRF guard).

```ts
type User = { id: string; moodle_userid: number; username: string; fullname: string; avatar_url: string | null }
type LinkStatus = {
  status: "ok" | "renewing" | "needs_relink"
  token_obtained_at: string
  expires_at: string | null        // null = unknown
  last_ok_at: string | null
  last_error: string | null
  auto_renew: boolean              // has a private token and a known expiry
}
type LinkStart = { attempt_id: string; launch_url: string; expires_at: string }
type AgentSession = {
  id: string; name: string; client_id: string; client_name: string | null
  origin: string | null; created_at: string; last_used_at: string | null; call_count: number
}
type ToolInfo = { name: string; title: string; description: string; kind: "read" | "write" }
type ToolCall = {
  id: string; session_id: string; session_name: string; tool: string
  arguments: Record<string, unknown>; status: "running" | "ok" | "error"
  result: unknown | null; error: string | null; duration_ms: number | null; created_at: string
}
type OAuthRequest = {
  client_id: string; client_name: string | null; client_uri: string | null; logo_uri: string | null
  origin: string | null; redirect_uri: string; scope: string
  existing_session: { name: string } | null
}
```

| Method & path | Body | Response |
| --- | --- | --- |
| `POST /auth/ilearning/start` | `{ next?: string }` | `LinkStart` (sets `ilmcp_link` cookie; works signed in or out) |
| `POST /auth/ilearning/complete` | `{ attempt_id, pasted }` | `{ redirect_to }` (sets `ilmcp_session`); errors `invalid_link`, `link_expired`, `not_allowed`, `wrong_account` (signed in as a different Moodle user), `moodle_error` |
| `POST /auth/logout` | | 204 |
| `GET /api/me` | | `{ user: User, link: LinkStatus }` |
| `GET /api/tools` | | `ToolInfo[]` |
| `GET /api/sessions` | | `AgentSession[]` (active only) |
| `PATCH /api/sessions/:id` | `{ name }` | `AgentSession` |
| `DELETE /api/sessions/:id` | | 204 (revokes tokens, keeps the log) |
| `GET /api/tool-calls?session_id=&status=&tool=&before=&limit=` | | `{ items: ToolCall[], next_before: string \| null }` |
| `GET /api/tool-calls/:id` | | `ToolCall` |
| `GET /api/oauth/requests/:id` | | `OAuthRequest` |
| `POST /api/oauth/requests/:id` | `{ approve: boolean, name? }` | `{ redirect_to }` |

SPA routes: `/login` (link flow; also used for re-link while signed in),
`/consent/:id`, and inside the sidebar layout `/` (Overview: profile + link
status + re-link), `/sessions` (Connected clients, rename/revoke),
`/activity` (tool-call log + detail sheet), `/connect` (MCP URL +
claude.ai / ChatGPT / Claude Code instructions).

## Config (env)

`PUBLIC_URL`, `BIND_ADDR` (default `0.0.0.0:8080`), `DATABASE_URL`,
`ENCRYPTION_KEY` (base64 32 bytes), `ILEARNING_URL` (default
`https://ilearning.cycu.edu.tw`), `ALLOWED_MOODLE_USERNAMES` (optional),
`TOKEN_DURATION_DAYS` (default 84), `WEB_DIST`, `RUST_LOG`; prod compose adds
`POSTGRES_PASSWORD`. Dev Postgres on host port **5443**.

## Conventions

Same as telegram-mcp: runtime-checked sqlx queries (no compile-time macros),
only SHA-256 hashes of our own secrets stored, `//!` doc comment atop every
module, `AppError` for dashboard errors and RFC 6749 errors for OAuth,
conventional commits. Never log Moodle tokens, private tokens, or pasted link
contents.
