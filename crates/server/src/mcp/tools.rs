//! The tool registry: one MCP tool per [`moodle::Account`] operation.

use std::{future::Future, sync::LazyLock};

use futures::future::BoxFuture;
use moodle::{Account, types::*};
use rmcp::model::{JsonObject, Tool as Definition, ToolAnnotations};
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::output::{IntoOutput, Output};

/// Whether a tool only reads or also changes something. Clients use the MCP
/// annotations derived from it to decide what to confirm with the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Read,
    Write,
}

/// What a tool does to the account; determines its [`Kind`] and MCP annotations.
#[derive(Clone, Copy)]
enum Effect {
    Read,
    /// Creates something new on every call (a post, a message).
    Additive,
    /// Changes state; repeating the call changes nothing more.
    Idempotent,
    /// Replaces or finalizes something the user cannot take back.
    Destructive,
}

/// A tool. Serializes as the dashboard's `ToolInfo`.
#[derive(Serialize)]
pub struct Tool {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub kind: Kind,
    #[serde(skip)]
    definition: Definition,
    #[serde(skip)]
    parse: Box<Parse>,
}

/// Parses tool arguments into a request.
type Parse = dyn Fn(&Value) -> serde_json::Result<Box<dyn Call>> + Send + Sync;

/// A tool request with valid arguments.
pub trait Call: Send + Sync {
    /// The request as the call log shows it (before the log bounds its size).
    fn log_arguments(&self) -> Value;

    fn run(self: Box<Self>, account: Account) -> BoxFuture<'static, moodle::Result<Output>>;
}

struct Prepared<R, F> {
    request: R,
    run: F,
}

impl<R, F, Fut, T> Call for Prepared<R, F>
where
    R: Serialize + Send + Sync + 'static,
    F: FnOnce(Account, R) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = moodle::Result<T>> + Send + 'static,
    T: IntoOutput,
{
    fn log_arguments(&self) -> Value {
        serde_json::to_value(&self.request).unwrap_or_default()
    }

    fn run(self: Box<Self>, account: Account) -> BoxFuture<'static, moodle::Result<Output>> {
        let Self { request, run } = *self;
        Box::pin(async move { Ok(run(account, request).await?.into_output()) })
    }
}

impl Tool {
    fn new<R, F, Fut, T>(
        name: &'static str,
        effect: Effect,
        title: &'static str,
        description: &'static str,
        run: F,
    ) -> Self
    where
        R: DeserializeOwned + Serialize + JsonSchema + Send + Sync + 'static,
        F: Fn(Account, R) -> Fut + Copy + Send + Sync + 'static,
        Fut: Future<Output = moodle::Result<T>> + Send + 'static,
        T: IntoOutput,
    {
        Self {
            name,
            title,
            description,
            kind: match effect {
                Effect::Read => Kind::Read,
                _ => Kind::Write,
            },
            definition: definition::<R>(name, title, description, effect.annotations(title)),
            parse: Box::new(move |args| {
                let request = R::deserialize(args)?;
                Ok(Box::new(Prepared { request, run }))
            }),
        }
    }

    pub fn definition(&self) -> Definition {
        self.definition.clone()
    }

    pub fn parse(&self, args: &Value) -> serde_json::Result<Box<dyn Call>> {
        (self.parse)(args)
    }
}

impl Effect {
    fn annotations(self, title: &str) -> ToolAnnotations {
        let annotations = ToolAnnotations::with_title(title).open_world(true);
        match self {
            Self::Read => annotations.read_only(true),
            write => annotations
                .read_only(false)
                .destructive(matches!(write, Self::Destructive))
                .idempotent(matches!(write, Self::Idempotent)),
        }
    }
}

fn definition<R: JsonSchema + 'static>(
    name: &'static str,
    title: &str,
    description: &'static str,
    annotations: ToolAnnotations,
) -> Definition {
    Definition::new(name, description, JsonObject::new())
        .with_title(title)
        .with_input_schema::<R>()
        .with_annotations(annotations)
}

static TOOLS: LazyLock<Vec<Tool>> = LazyLock::new(|| {
    use Effect::*;
    vec![
        Tool::new(
            "get_profile",
            Read,
            "Get my profile",
            "The signed-in student's iLearning profile: user ID, username, full name, picture \
             and site.",
            |a: Account, r: NoParams| async move { a.get_profile(r).await },
        ),
        Tool::new(
            "list_courses",
            Read,
            "List my courses",
            "The student's enrolled courses with their IDs, names, category and dates. \
             `classification` picks in-progress (default), past, future or all courses.",
            |a: Account, r: ListCoursesRequest| async move { a.list_courses(r).await },
        ),
        Tool::new(
            "get_course_contents",
            Read,
            "Get course contents",
            "A course's sections and their modules (cmid, type `modname`, name, URL, dates) \
             with attached files (`fileurl` for read_file).",
            |a: Account, r: CourseRequest| async move { a.get_course_contents(r).await },
        ),
        Tool::new(
            "get_upcoming_deadlines",
            Read,
            "Get upcoming deadlines",
            "Upcoming due dates and events (assignments, quizzes, …) in the next `days` days \
             (default 14), across all courses or for one course.",
            |a: Account, r: DeadlinesRequest| async move { a.get_upcoming_deadlines(r).await },
        ),
        Tool::new(
            "list_assignments",
            Read,
            "List assignments",
            "Assignments of the given courses (all enrolled courses by default) with their IDs, \
             due dates, descriptions and attachments.",
            |a: Account, r: CourseIdsRequest| async move { a.list_assignments(r).await },
        ),
        Tool::new(
            "get_assignment_status",
            Read,
            "Get assignment status",
            "One assignment's submission status for the student: submitted files and text, \
             whether it is graded, the grade and teacher feedback.",
            |a: Account, r: AssignmentRequest| async move { a.get_assignment_status(r).await },
        ),
        Tool::new(
            "get_grades",
            Read,
            "Get grades",
            "Without course_id, the student's overall grade in every course; with it, that \
             course's grade items with grades, ranges and feedback.",
            |a: Account, r: GradesRequest| async move { a.get_grades(r).await },
        ),
        Tool::new(
            "list_forum_discussions",
            Read,
            "List forum discussions",
            "The discussions of a forum (by forum instance ID from get_course_contents), \
             newest first, a page at a time.",
            |a: Account, r: ForumRequest| async move { a.list_forum_discussions(r).await },
        ),
        Tool::new(
            "get_discussion_posts",
            Read,
            "Get discussion posts",
            "All posts of a forum discussion with authors, times, text and attachments.",
            |a: Account, r: DiscussionRequest| async move { a.get_discussion_posts(r).await },
        ),
        Tool::new(
            "get_notifications",
            Read,
            "Get notifications",
            "The student's recent iLearning notifications (newest first), optionally only \
             unread ones.",
            |a: Account, r: NotificationsRequest| async move { a.get_notifications(r).await },
        ),
        Tool::new(
            "list_conversations",
            Read,
            "List conversations",
            "The student's private and group message conversations with their IDs, members \
             and latest message.",
            |a: Account, r: ConversationsRequest| async move { a.list_conversations(r).await },
        ),
        Tool::new(
            "get_conversation_messages",
            Read,
            "Get conversation messages",
            "Messages of one conversation, newest first.",
            |a: Account, r: ConversationRequest| async move { a.get_conversation_messages(r).await },
        ),
        Tool::new(
            "get_module",
            Read,
            "Get a module",
            "The content of one course module (by course ID and cmid): a page's text, a URL \
             module's link, a resource's files or a label's text.",
            |a: Account, r: ModuleRequest| async move { a.get_module(r).await },
        ),
        Tool::new(
            "read_file",
            Read,
            "Read a course file",
            "Reads an iLearning file by its `fileurl`: text of PDF, Word, PowerPoint, Excel and \
             text files (up to `max_chars`), images as images, and a download link valid for \
             one hour for anything else.",
            |a: Account, r: ReadFileRequest| async move { a.read_file(r).await },
        ),
        Tool::new(
            "submit_assignment",
            Destructive,
            "Submit an assignment",
            "Saves the student's submission for an assignment: online text and/or files \
             (`text` for plain-text files, `base64` for others). This replaces the current \
             draft. With submit_for_grading=true it is also submitted for grading, accepting \
             the submission statement, after which the student usually cannot change it. \
             Confirm with the user first.",
            |a: Account, r: SubmitAssignmentRequest| async move { a.submit_assignment(r).await },
        ),
        Tool::new(
            "reply_forum_post",
            Additive,
            "Reply to a forum post",
            "Posts a reply, as the student, to a forum post (by post ID). Visible to the \
             teacher and classmates.",
            |a: Account, r: ReplyRequest| async move { a.reply_forum_post(r).await },
        ),
        Tool::new(
            "create_forum_discussion",
            Additive,
            "Start a forum discussion",
            "Starts a new discussion, as the student, in a forum (by forum instance ID). \
             Visible to the teacher and classmates.",
            |a: Account, r: NewDiscussionRequest| async move { a.create_forum_discussion(r).await },
        ),
        Tool::new(
            "send_message",
            Additive,
            "Send a message",
            "Sends a private iLearning message, as the student, to a user (by user ID).",
            |a: Account, r: SendMessageRequest| async move { a.send_message(r).await },
        ),
        Tool::new(
            "mark_notifications_read",
            Idempotent,
            "Mark notifications read",
            "Marks all of the student's notifications as read.",
            |a: Account, r: NoParams| async move { a.mark_notifications_read(r).await },
        ),
    ]
});

pub fn all() -> impl Iterator<Item = &'static Tool> {
    TOOLS.iter()
}

pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotations_match_effects() {
        let definition = |name| find(name).unwrap().definition();
        let read = definition("get_profile").annotations.unwrap();
        assert_eq!(read.read_only_hint, Some(true));
        let submit = definition("submit_assignment").annotations.unwrap();
        assert_eq!(submit.read_only_hint, Some(false));
        assert_eq!(submit.destructive_hint, Some(true));
        let reply = definition("reply_forum_post").annotations.unwrap();
        assert_eq!(reply.destructive_hint, Some(false));
        assert_eq!(reply.idempotent_hint, Some(false));
        let mark = definition("mark_notifications_read").annotations.unwrap();
        assert_eq!(mark.idempotent_hint, Some(true));
        assert_eq!(mark.destructive_hint, Some(false));
    }

    #[test]
    fn every_tool_has_an_object_schema() {
        let names: Vec<_> = all().map(|tool| tool.name).collect();
        assert_eq!(names.len(), 19);
        for tool in all() {
            let definition = tool.definition();
            assert_eq!(definition.input_schema["type"], "object", "{}", tool.name);
            assert_eq!(definition.title.as_deref(), Some(tool.title));
        }
    }
}
