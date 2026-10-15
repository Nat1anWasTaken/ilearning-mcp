//! The rmcp [`ServerHandler`]: server info, `tools/list` and `tools/call`.

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, Icon, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool as Definition,
    },
    service::RequestContext,
};

use super::{pipeline, tools};
use crate::{oauth::AgentPrincipal, state::AppState};

const INSTRUCTIONS: &str = "\
These tools act as the user's own account on i-learning 2.0, the Moodle learning platform of \
Chung Yuan Christian University (CYCU). Use them only as the user asks.

Start with list_courses (course IDs), get_upcoming_deadlines (due work across courses) or \
get_notifications. get_course_contents lists a course's sections and modules (cmid, modname, \
file URLs); get_module reads a page/url/resource/label module; read_file extracts the text of \
a course file (PDF, Word, PowerPoint, Excel, text), returns images, or gives a short-lived \
download link for other files. Assignments: list_assignments, then get_assignment_status for \
one assignment's submission and feedback. Forums: list_forum_discussions, then \
get_discussion_posts. get_grades without course_id gives the overview, with it the course's \
grade items. Messages: list_conversations, get_conversation_messages.

Write tools act as the student and are visible to teachers and classmates: \
submit_assignment, reply_forum_post, create_forum_discussion, send_message and \
mark_notifications_read. Confirm with the user before calling them. submit_assignment with \
submit_for_grading=true finalizes the submission so it can no longer be edited.

If a tool says the iLearning link expired, ask the user to re-link their account in the \
dashboard.";

/// The MCP server for one request. Stateless: every request is authenticated
/// on its own.
pub struct ILearningServer {
    state: AppState,
}

impl ILearningServer {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl ServerHandler for ILearningServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("ilearning", env!("CARGO_PKG_VERSION"))
                    .with_title("iLearning")
                    .with_icons(vec![
                        // Served from the dashboard's static files (web/public).
                        Icon::new(format!("{}/ilearning.png", self.state.config.public_url))
                            .with_mime_type("image/png")
                            .with_sizes(vec!["512x512".into()]),
                    ]),
            )
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // Every session may use every tool; the bearer middleware already
        // rejected revoked ones.
        let listed = tools::all().map(tools::Tool::definition).collect();
        Ok(ListToolsResult::with_all_items(listed))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let principal = principal(&context)?;
        let args = request.arguments.unwrap_or_default();
        pipeline::call(&self.state, principal, &request.name, args)
            .await
            .map(Into::into)
    }

    fn get_tool(&self, name: &str) -> Option<Definition> {
        tools::find(name).map(tools::Tool::definition)
    }
}

/// The agent the bearer middleware authenticated; rmcp passes the HTTP
/// request parts through.
fn principal(context: &RequestContext<RoleServer>) -> Result<AgentPrincipal, ErrorData> {
    context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<AgentPrincipal>())
        .copied()
        .ok_or_else(|| ErrorData::internal_error("request is not authenticated", None))
}
