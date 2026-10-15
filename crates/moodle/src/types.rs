//! Request types for the agent tools.
//!
//! They double as MCP tool input schemas, so doc comments are written for
//! the agent: each struct's comment describes the tool, each field's comment
//! becomes the parameter description.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// For tools that take no arguments (`get_profile`,
/// `mark_notifications_read`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct NoParams {}

/// List the courses the user is enrolled in.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListCoursesRequest {
    /// Which courses: `inprogress` (current semester, default), `past`,
    /// `future` or `all`.
    #[serde(default)]
    pub classification: Classification,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Classification {
    #[default]
    Inprogress,
    Past,
    Future,
    All,
}

impl Classification {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inprogress => "inprogress",
            Self::Past => "past",
            Self::Future => "future",
            Self::All => "all",
        }
    }
}

/// Get a course's sections and activities (modules), including file
/// attachments with their `fileurl` for `read_file`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CourseRequest {
    /// Course ID, from `list_courses`.
    pub course_id: i64,
}

/// List upcoming deadlines and other events that need action (assignments
/// due, quizzes closing, …), soonest first. Items overdue by up to 7 days
/// are included and marked `overdue`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeadlinesRequest {
    /// How many days ahead to look (1-365). Default 14.
    #[serde(default = "default_days")]
    #[schemars(range(min = 1, max = 365))]
    pub days: u32,
    /// Only this course. Default: all courses.
    #[serde(default)]
    pub course_id: Option<i64>,
}

impl Default for DeadlinesRequest {
    fn default() -> Self {
        Self {
            days: default_days(),
            course_id: None,
        }
    }
}

/// List assignments with their due dates, instructions and submission
/// settings.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CourseIdsRequest {
    /// Only these courses. Default: all enrolled courses.
    #[serde(default)]
    pub course_ids: Option<Vec<i64>>,
}

/// Get the user's submission status, submitted files and text, grade and
/// feedback for an assignment.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AssignmentRequest {
    /// Assignment ID (the `id` from `list_assignments`, not the `cmid`).
    pub assignment_id: i64,
}

/// Get grades: the course totals of every course, or every grade item of
/// one course.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct GradesRequest {
    /// Show the grade items of this course. Default: course totals of all
    /// courses.
    #[serde(default)]
    pub course_id: Option<i64>,
}

/// List the discussions of a forum (e.g. a course's announcements), newest
/// activity first.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ForumRequest {
    /// Forum ID: the `instance` of a `forum` module in
    /// `get_course_contents`.
    pub forum_id: i64,
    /// Page number, from 0. 20 discussions per page. Default 0.
    #[serde(default)]
    pub page: u32,
}

/// Get all posts of a forum discussion, oldest first.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DiscussionRequest {
    /// Discussion ID, from `list_forum_discussions`.
    pub discussion_id: i64,
}

/// List the user's notifications (announcements, grading, mail alerts, …),
/// newest first.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct NotificationsRequest {
    /// Number of notifications (1-50). Default 20.
    #[serde(default = "default_limit")]
    #[schemars(range(min = 1, max = 50))]
    pub limit: u32,
    /// Only unread notifications. Default false.
    #[serde(default)]
    pub unread_only: bool,
}

impl Default for NotificationsRequest {
    fn default() -> Self {
        Self {
            limit: default_limit(),
            unread_only: false,
        }
    }
}

/// List the user's private and group message conversations, most recent
/// first.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ConversationsRequest {
    /// Number of conversations (1-50). Default 20.
    #[serde(default = "default_limit")]
    #[schemars(range(min = 1, max = 50))]
    pub limit: u32,
}

impl Default for ConversationsRequest {
    fn default() -> Self {
        Self {
            limit: default_limit(),
        }
    }
}

/// Get the latest messages of a conversation, oldest first.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ConversationRequest {
    /// Conversation ID, from `list_conversations`.
    pub conversation_id: i64,
    /// Number of most recent messages (1-100). Default 30.
    #[serde(default = "default_messages")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u32,
}

/// Get the body of a course module: a page's text, a URL's target, a
/// file resource's files, a label's text, or the description of any other
/// activity.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ModuleRequest {
    /// Course ID.
    pub course_id: i64,
    /// Course module ID (`cmid` from `get_course_contents`).
    pub cmid: i64,
}

/// Read a file from iLearning: text of PDF, Word, PowerPoint, Excel, text
/// and HTML files; images are returned as images; other files (videos,
/// legacy .ppt/.doc, archives) as a download link.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReadFileRequest {
    /// A `fileurl` from another tool's result (an iLearning
    /// `pluginfile.php` URL).
    pub file_url: String,
    /// Most characters of text to return. Default 100000.
    #[serde(default = "default_max_chars")]
    #[schemars(range(min = 1))]
    pub max_chars: u32,
}

/// Save the user's submission for an assignment: online text and/or files.
/// Files replace all previously submitted files. Optionally submit it for
/// grading, after which it usually cannot be changed.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SubmitAssignmentRequest {
    /// Assignment ID (the `id` from `list_assignments`).
    pub assignment_id: i64,
    /// Online text answer (plain text), for assignments that accept it.
    #[serde(default)]
    pub online_text: Option<String>,
    /// Files to submit, for assignments that accept file submissions.
    #[serde(default)]
    pub files: Vec<SubmissionFile>,
    /// Also submit for grading (accepting the submission statement).
    /// Default false: saved as a draft where the assignment allows drafts.
    #[serde(default)]
    pub submit_for_grading: bool,
}

/// A file to submit. Give exactly one of `text` or `base64`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SubmissionFile {
    /// File name including extension, e.g. `report.pdf`.
    pub filename: String,
    /// File content as UTF-8 text.
    #[serde(default)]
    pub text: Option<String>,
    /// File content, base64-encoded.
    #[serde(default)]
    pub base64: Option<String>,
}

/// Reply to a forum post.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReplyRequest {
    /// ID of the post to reply to, from `get_discussion_posts`.
    pub post_id: i64,
    /// Subject. Default: "Re: " and the parent's subject.
    #[serde(default)]
    pub subject: Option<String>,
    /// Message (plain text).
    pub message: String,
}

/// Start a new discussion in a forum.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct NewDiscussionRequest {
    /// Forum ID: the `instance` of a `forum` module.
    pub forum_id: i64,
    pub subject: String,
    /// Message (plain text).
    pub message: String,
}

/// Send a private message to another iLearning user.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SendMessageRequest {
    /// Recipient's user ID, e.g. a conversation member or a forum post
    /// author.
    pub to_user_id: i64,
    /// Message (plain text).
    pub text: String,
}

fn default_days() -> u32 {
    14
}

fn default_limit() -> u32 {
    20
}

fn default_messages() -> u32 {
    30
}

fn default_max_chars() -> u32 {
    100_000
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn applies_defaults() {
        let r: DeadlinesRequest = serde_json::from_value(json!({})).unwrap();
        assert_eq!((r.days, r.course_id), (14, None));
        let r: ListCoursesRequest = serde_json::from_value(json!({})).unwrap();
        assert_eq!(r.classification, Classification::Inprogress);
        let r: ListCoursesRequest =
            serde_json::from_value(json!({"classification": "past"})).unwrap();
        assert_eq!(r.classification.as_str(), "past");
        let r: ReadFileRequest = serde_json::from_value(json!({"file_url": "x"})).unwrap();
        assert_eq!(r.max_chars, 100_000);
        let r: SubmitAssignmentRequest =
            serde_json::from_value(json!({"assignment_id": 3})).unwrap();
        assert!(!r.submit_for_grading && r.files.is_empty() && r.online_text.is_none());
        let _: NoParams = serde_json::from_value(json!({})).unwrap();
    }

    #[test]
    fn schemas_carry_descriptions() {
        let schema = serde_json::to_value(schemars::schema_for!(ReplyRequest)).unwrap();
        assert_eq!(schema["description"], "Reply to a forum post.");
        assert!(schema["properties"]["post_id"]["description"].is_string());
        assert_eq!(schema["required"], json!(["post_id", "message"]));
    }
}
