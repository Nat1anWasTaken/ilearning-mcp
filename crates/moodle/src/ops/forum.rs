//! Forums: discussions, posts, replying and starting discussions.

use serde_json::{Value, json};

use super::{files, preview};
use crate::Account;
use crate::error::{Error, Result};
use crate::text::{Get, text_to_html};
use crate::types::{DiscussionRequest, ForumRequest, NewDiscussionRequest, ReplyRequest};

const PER_PAGE: u32 = 20;

impl Account {
    /// `list_forum_discussions`.
    pub async fn list_forum_discussions(&self, request: ForumRequest) -> Result<Value> {
        let raw = self
            .call(
                "mod_forum_get_forum_discussions",
                json!({ "forumid": request.forum_id, "page": request.page, "perpage": PER_PAGE }),
            )
            .await?;
        let discussions: Vec<Value> = raw
            .list("discussions")
            .iter()
            .map(|d| {
                let attachments = files(d.list("attachments"));
                json!({
                    "discussion_id": d.int("discussion"),
                    "first_post_id": d.int("id"),
                    "subject": d.html("subject").or_else(|| d.html("name")),
                    "author": d.str("userfullname"),
                    "author_id": d.int("userid"),
                    "created": d.time("created"),
                    "last_activity": d.time("timemodified"),
                    "replies": d.int("numreplies"),
                    "unread": d.int("numunread").filter(|n| *n > 0),
                    "pinned": d.flag("pinned").filter(|p| *p),
                    "locked": d.flag("locked").filter(|l| *l),
                    "can_reply": d.flag("canreply"),
                    "preview": preview(d.html("message"), 300),
                    "attachments": (!attachments.is_empty()).then_some(attachments),
                })
            })
            .collect();
        let has_more = discussions.len() as u32 == PER_PAGE;
        self.finish(json!({
            "forum_id": request.forum_id,
            "page": request.page,
            "discussions": discussions,
            "next_page": has_more.then_some(request.page + 1),
        }))
        .await
    }

    /// `get_discussion_posts`.
    pub async fn get_discussion_posts(&self, request: DiscussionRequest) -> Result<Value> {
        let raw = self
            .call(
                "mod_forum_get_discussion_posts",
                json!({ "discussionid": request.discussion_id, "sortby": "created", "sortdirection": "ASC" }),
            )
            .await?;
        let posts: Vec<Value> = raw.list("posts").iter().map(post).collect();
        self.finish(json!({
            "discussion_id": request.discussion_id,
            "forum_id": raw.int("forumid"),
            "course_id": raw.int("courseid"),
            "posts": posts,
        }))
        .await
    }

    /// `reply_forum_post`.
    pub async fn reply_forum_post(&self, request: ReplyRequest) -> Result<Value> {
        let subject = match request.subject.filter(|s| !s.trim().is_empty()) {
            Some(subject) => subject,
            None => {
                let parent = self
                    .call(
                        "mod_forum_get_discussion_post",
                        json!({ "postid": request.post_id }),
                    )
                    .await?;
                let post = parent.get("post").unwrap_or(&Value::Null);
                post.html("replysubject")
                    .or_else(|| post.html("subject").map(|s| format!("Re: {s}")))
                    .ok_or_else(|| Error::decode("the parent post has no subject"))?
            }
        };
        let raw = self
            .call(
                "mod_forum_add_discussion_post",
                json!({
                    "postid": request.post_id,
                    "subject": subject,
                    "message": text_to_html(&request.message),
                    "messageformat": 1,
                }),
            )
            .await?;
        let post_id = raw
            .int("postid")
            .ok_or_else(|| Error::decode("no post id in reply result"))?;
        self.finish(json!({
            "post_id": post_id,
            "subject": subject,
            "discussion_id": raw.get("post").and_then(|p| p.int("discussionid")),
            "messages": raw.list("messages").iter().filter_map(|m| m.str("message")).collect::<Vec<_>>(),
        }))
        .await
    }

    /// `create_forum_discussion`.
    pub async fn create_forum_discussion(&self, request: NewDiscussionRequest) -> Result<Value> {
        let raw = self
            .call(
                "mod_forum_add_discussion",
                json!({
                    "forumid": request.forum_id,
                    "subject": request.subject,
                    "message": text_to_html(&request.message),
                }),
            )
            .await?;
        let discussion_id = raw
            .int("discussionid")
            .ok_or_else(|| Error::decode("no discussion id in result"))?;
        self.finish(json!({ "discussion_id": discussion_id, "forum_id": request.forum_id }))
            .await
    }
}

fn post(p: &Value) -> Value {
    let author = p.get("author").unwrap_or(&Value::Null);
    let mut attachments = files(p.list("attachments"));
    attachments.extend(files(p.list("messageinlinefiles")));
    json!({
        "id": p.int("id"),
        "parent_id": p.int("parentid"),
        "subject": p.html("subject"),
        "author": author.str("fullname"),
        "author_id": author.int("id"),
        "created": p.time("timecreated"),
        "modified": p.time("timemodified").filter(|_| p.int("timemodified") != p.int("timecreated")),
        "message": p.html("message"),
        "private_reply": p.flag("isprivatereply").filter(|x| *x),
        "deleted": p.flag("isdeleted").filter(|x| *x),
        "can_reply": p.get("capabilities").and_then(|c| c.flag("reply")),
        "attachments": (!attachments.is_empty()).then_some(attachments),
    })
}
