//! The tool registry: one MCP tool per [`moodle::Account`] operation.

use std::borrow::Cow;

use mcp_backbone::{
    Tool,
    rmcp::model::JsonObject,
    schemars::{JsonSchema, Schema, SchemaGenerator},
};
use moodle::types::*;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use crate::{
    files,
    service::{Account, ILearningService},
};

/// Every tool, grouped by the part of iLearning it belongs to.
pub fn all() -> Vec<Tool<ILearningService>> {
    use mcp_backbone::Effect::*;
    vec![
        Tool::new(
            "get_profile",
            Read,
            "Get my profile",
            "The signed-in student's iLearning profile: user ID, username, full name, picture \
             and site.",
        )
        .group("profile")
        .run(|a: Account, r: NoParams| async move { a.moodle.get_profile(r).await }),
        Tool::new(
            "list_courses",
            Read,
            "List my courses",
            "The student's enrolled courses with their IDs, names, category and dates. \
             `classification` picks in-progress (default), past, future or all courses.",
        )
        .group("courses")
        .run(|a: Account, r: ListCoursesRequest| async move { a.moodle.list_courses(r).await }),
        Tool::new(
            "get_course_contents",
            Read,
            "Get course contents",
            "A course's sections and their modules (cmid, type `modname`, name, URL, dates) \
             with attached files (`fileurl` for read_file).",
        )
        .group("courses")
        .run(|a: Account, r: CourseRequest| async move { a.moodle.get_course_contents(r).await }),
        Tool::new(
            "get_upcoming_deadlines",
            Read,
            "Get upcoming deadlines",
            "Upcoming due dates and events (assignments, quizzes, …) in the next `days` days \
             (default 14), across all courses or for one course.",
        )
        .group("courses")
        .run(|a: Account, r: DeadlinesRequest| async move {
            a.moodle.get_upcoming_deadlines(r).await
        }),
        Tool::new(
            "list_assignments",
            Read,
            "List assignments",
            "Assignments of the given courses (all enrolled courses by default) with their IDs, \
             due dates, descriptions and attachments.",
        )
        .group("assignments")
        .run(|a: Account, r: CourseIdsRequest| async move { a.moodle.list_assignments(r).await }),
        Tool::new(
            "get_assignment_status",
            Read,
            "Get assignment status",
            "One assignment's submission status for the student: submitted files and text, \
             whether it is graded, the grade and teacher feedback.",
        )
        .group("assignments")
        .run(|a: Account, r: AssignmentRequest| async move {
            a.moodle.get_assignment_status(r).await
        }),
        Tool::new(
            "get_grades",
            Read,
            "Get grades",
            "Without course_id, the student's overall grade in every course; with it, that \
             course's grade items with grades, ranges and feedback.",
        )
        .group("grades")
        .run(|a: Account, r: GradesRequest| async move { a.moodle.get_grades(r).await }),
        Tool::new(
            "list_forum_discussions",
            Read,
            "List forum discussions",
            "The discussions of a forum (by forum instance ID from get_course_contents), \
             newest first, a page at a time.",
        )
        .group("forums")
        .run(|a: Account, r: ForumRequest| async move {
            a.moodle.list_forum_discussions(r).await
        }),
        Tool::new(
            "get_discussion_posts",
            Read,
            "Get discussion posts",
            "All posts of a forum discussion with authors, times, text and attachments.",
        )
        .group("forums")
        .run(|a: Account, r: DiscussionRequest| async move {
            a.moodle.get_discussion_posts(r).await
        }),
        Tool::new(
            "get_notifications",
            Read,
            "Get notifications",
            "The student's recent iLearning notifications (newest first), optionally only \
             unread ones.",
        )
        .group("messages")
        .run(|a: Account, r: NotificationsRequest| async move {
            a.moodle.get_notifications(r).await
        }),
        Tool::new(
            "list_conversations",
            Read,
            "List conversations",
            "The student's private and group message conversations with their IDs, members \
             and latest message.",
        )
        .group("messages")
        .run(|a: Account, r: ConversationsRequest| async move {
            a.moodle.list_conversations(r).await
        }),
        Tool::new(
            "get_conversation_messages",
            Read,
            "Get conversation messages",
            "Messages of one conversation, newest first.",
        )
        .group("messages")
        .run(|a: Account, r: ConversationRequest| async move {
            a.moodle.get_conversation_messages(r).await
        }),
        Tool::new(
            "get_module",
            Read,
            "Get a module",
            "The content of one course module (by course ID and cmid): a page's text, a URL \
             module's link, a resource's files or a label's text.",
        )
        .group("courses")
        .run(|a: Account, r: ModuleRequest| async move { a.moodle.get_module(r).await }),
        Tool::new(
            "read_file",
            Read,
            "Read a course file",
            "Reads an iLearning file by its `fileurl`: text of PDF, Word, PowerPoint, Excel and \
             text files (up to `max_chars`), images as images, and a download link valid for \
             one hour for anything else. With mode=\"link\" it returns that download link for \
             any file, to get the original bytes (e.g. to convert a document, or when its text \
             misses equations, images or layout).",
        )
        .group("files")
        .run(|a: Account, r: ReadFileRequest| async move {
            let file = a.moodle.read_file(r).await?;
            Ok::<_, moodle::Error>(files::file_output(file, &a.files))
        }),
        Tool::new(
            "download_file",
            Read,
            "Download a course file",
            "Returns an iLearning file's original bytes by its `fileurl`, base64-encoded as an \
             embedded resource, for agents that cannot fetch the link from read_file with \
             mode=\"link\". Returns up to `max_bytes` from `offset`; while `next_offset` is not \
             null, call again with offset=next_offset and join the parts. Prefer read_file \
             for the text, and a download link when you can fetch URLs: base64 is large.",
        )
        .group("files")
        .run(|a: Account, r: DownloadFileRequest| async move {
            let chunk = a.moodle.download_file(r).await?;
            Ok::<_, moodle::Error>(files::chunk_output(chunk))
        }),
        Tool::new(
            "submit_assignment",
            Destructive,
            "Submit an assignment",
            "Saves the student's submission for an assignment: online text and/or files \
             (`text` for plain-text files, `base64` for others). This replaces the current \
             draft. With submit_for_grading=true it is also submitted for grading, accepting \
             the submission statement, after which the student usually cannot change it. \
             Confirm with the user first.",
        )
        .group("assignments")
        .summary(&["assignment_id"], &["online_text"])
        .run_custom(
            |a: Account, r: SubmitAssignment, _| async move {
                a.moodle.submit_assignment(r.0).await
            },
            SubmitAssignment::edit,
            SubmitAssignment::store,
        ),
        Tool::new(
            "reply_forum_post",
            Additive,
            "Reply to a forum post",
            "Posts a reply, as the student, to a forum post (by post ID). Visible to the \
             teacher and classmates.",
        )
        .group("forums")
        .summary(&["post_id"], &["message"])
        .run(|a: Account, r: ReplyRequest| async move { a.moodle.reply_forum_post(r).await }),
        Tool::new(
            "create_forum_discussion",
            Additive,
            "Start a forum discussion",
            "Starts a new discussion, as the student, in a forum (by forum instance ID). \
             Visible to the teacher and classmates.",
        )
        .group("forums")
        .summary(&["subject", "forum_id"], &["message"])
        .run(|a: Account, r: NewDiscussionRequest| async move {
            a.moodle.create_forum_discussion(r).await
        }),
        Tool::new(
            "send_message",
            Additive,
            "Send a message",
            "Sends a private iLearning message, as the student, to a user (by user ID).",
        )
        .group("messages")
        .summary(&["to_user_id"], &["text"])
        .run(|a: Account, r: SendMessageRequest| async move { a.moodle.send_message(r).await }),
        Tool::new(
            "mark_notifications_read",
            Idempotent,
            "Mark notifications read",
            "Marks all of the student's notifications as read.",
        )
        .group("messages")
        .run(|a: Account, r: NoParams| async move { a.moodle.mark_notifications_read(r).await }),
    ]
}

/// `submit_assignment`'s request. It serializes, as the call log and the
/// outbox show it, with file bodies (`base64`) replaced by their length;
/// [`Self::store`] keeps them for the outbox to run the call.
#[derive(Deserialize)]
#[serde(transparent)]
pub struct SubmitAssignment(pub SubmitAssignmentRequest);

impl SubmitAssignment {
    /// The request in full.
    fn store(&self) -> Value {
        serde_json::to_value(&self.0).unwrap_or_default()
    }

    /// The request with the user's edits merged over it in full.
    fn edit(&self, patch: &JsonObject) -> Result<Self, String> {
        let Value::Object(mut args) = self.store() else {
            return Err("this request cannot be edited".into());
        };
        args.extend(patch.clone());
        serde_json::from_value(Value::Object(args)).map_err(|err| err.to_string())
    }
}

impl Serialize for SubmitAssignment {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut value = self.store();
        if let Some(files) = value.get_mut("files").and_then(Value::as_array_mut) {
            for file in files {
                if let Some(body) = file.get_mut("base64")
                    && let Some(text) = body.as_str()
                {
                    *body = Value::String(format!("[{} characters]", text.chars().count()));
                }
            }
        }
        value.serialize(serializer)
    }
}

impl JsonSchema for SubmitAssignment {
    fn inline_schema() -> bool {
        SubmitAssignmentRequest::inline_schema()
    }

    fn schema_name() -> Cow<'static, str> {
        SubmitAssignmentRequest::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        SubmitAssignmentRequest::schema_id()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        SubmitAssignmentRequest::json_schema(generator)
    }
}

#[cfg(test)]
mod tests {
    use mcp_backbone::Effect;
    use serde_json::json;

    use super::*;

    fn tool(name: &str) -> Tool<ILearningService> {
        all().into_iter().find(|tool| tool.name == name).unwrap()
    }

    #[test]
    fn annotations_match_effects() {
        let read = tool("get_profile").definition().annotations.unwrap();
        assert_eq!(read.read_only_hint, Some(true));
        let submit = tool("submit_assignment").definition().annotations.unwrap();
        assert_eq!(submit.read_only_hint, Some(false));
        assert_eq!(submit.destructive_hint, Some(true));
        let reply = tool("reply_forum_post").definition().annotations.unwrap();
        assert_eq!(reply.destructive_hint, Some(false));
        assert_eq!(reply.idempotent_hint, Some(false));
        let mark = tool("mark_notifications_read")
            .definition()
            .annotations
            .unwrap();
        assert_eq!(mark.idempotent_hint, Some(true));
        assert_eq!(mark.destructive_hint, Some(false));
    }

    #[test]
    fn writes_are_the_tools_that_change_something() {
        let writes: Vec<_> = all()
            .into_iter()
            .filter(|tool| tool.kind == Effect::Additive.kind())
            .map(|tool| tool.name)
            .collect();
        assert_eq!(
            writes,
            [
                "submit_assignment",
                "reply_forum_post",
                "create_forum_discussion",
                "send_message",
                "mark_notifications_read",
            ]
        );
    }

    #[test]
    fn every_tool_has_an_object_schema() {
        let tools = all();
        assert_eq!(tools.len(), 20);
        for tool in &tools {
            let definition = tool.definition();
            assert_eq!(definition.input_schema["type"], "object", "{}", tool.name);
            assert_eq!(definition.title.as_deref(), Some(tool.title));
            assert!(tool.group.is_some(), "{}", tool.name);
        }
        let submit = tool("submit_assignment").definition();
        assert!(submit.input_schema["properties"]["files"].is_object());
        assert!(
            submit.input_schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("assignment_id"))
        );
    }

    #[test]
    fn file_bodies_are_logged_by_length_but_kept_to_run() {
        let request: SubmitAssignment = serde_json::from_value(json!({
            "assignment_id": 3,
            "files": [
                { "filename": "a.bin", "base64": "QUJD".repeat(10) },
                { "filename": "b.txt", "text": "hello" },
            ],
        }))
        .unwrap();
        let logged = serde_json::to_value(&request).unwrap();
        assert_eq!(logged["files"][0]["base64"], "[40 characters]");
        assert_eq!(logged["files"][1]["text"], "hello");
        assert_eq!(request.store()["files"][0]["base64"], "QUJD".repeat(10));

        let mut patch = JsonObject::new();
        patch.insert("submit_for_grading".into(), json!(true));
        let edited = request.edit(&patch).unwrap();
        assert!(edited.0.submit_for_grading);
        assert_eq!(
            edited.0.files[0].base64.as_deref(),
            Some(&*"QUJD".repeat(10))
        );
        patch.insert("assignment_id".into(), json!("x"));
        assert!(request.edit(&patch).is_err());
    }
}
