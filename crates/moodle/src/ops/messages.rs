//! Notifications and private / group messaging.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::preview;
use crate::Account;
use crate::error::{Error, Result};
use crate::text::{Get, text_to_html};
use crate::types::{
    ConversationRequest, ConversationsRequest, NoParams, NotificationsRequest, SendMessageRequest,
};

/// Notifications fetched when filtering to unread ones client-side.
const UNREAD_SCAN: u32 = 100;
const NOTIFICATION_CHARS: usize = 2000;

impl Account {
    /// `get_notifications`.
    pub async fn get_notifications(&self, request: NotificationsRequest) -> Result<Value> {
        let userid = self.me().await?.userid;
        let limit = request.limit.clamp(1, 50);
        let fetch = if request.unread_only {
            UNREAD_SCAN
        } else {
            limit
        };
        let raw = self
            .call(
                "message_popup_get_popup_notifications",
                json!({ "useridto": userid, "newestfirst": true, "limit": fetch, "offset": 0 }),
            )
            .await?;
        let notifications: Vec<Value> = raw
            .list("notifications")
            .iter()
            .filter(|n| !request.unread_only || n.flag("read") != Some(true))
            .take(limit as usize)
            .map(notification)
            .collect();
        self.finish(json!({
            "unread_count": raw.int("unreadcount"),
            "notifications": notifications,
        }))
        .await
    }

    /// `mark_notifications_read`.
    pub async fn mark_notifications_read(&self, _request: NoParams) -> Result<Value> {
        let userid = self.me().await?.userid;
        self.call(
            "core_message_mark_all_notifications_as_read",
            json!({ "useridto": userid }),
        )
        .await?;
        Ok(json!({ "marked_read": true }))
    }

    /// `list_conversations`.
    pub async fn list_conversations(&self, request: ConversationsRequest) -> Result<Value> {
        let userid = self.me().await?.userid;
        let raw = self
            .call(
                "core_message_get_conversations",
                json!({ "userid": userid, "limitnum": request.limit.clamp(1, 50), "mergeself": true }),
            )
            .await?;
        let conversations: Vec<Value> = raw
            .list("conversations")
            .iter()
            .map(|c| {
                let last = c.list("messages").first();
                json!({
                    "id": c.int("id"),
                    "type": match c.int("type") {
                        Some(1) => "private",
                        Some(2) => "group",
                        Some(3) => "self",
                        _ => "other",
                    },
                    "name": c.html("name"),
                    "members": c.list("members").iter().map(member).collect::<Vec<_>>(),
                    "member_count": c.int("membercount"),
                    "unread": c.int("unreadcount").filter(|n| *n > 0),
                    "read": c.flag("isread"),
                    "muted": c.flag("ismuted").filter(|m| *m),
                    "last_message": last.map(|m| json!({
                        "from_user_id": m.int("useridfrom"),
                        "time": m.time("timecreated"),
                        "text": preview(m.html("text"), 300),
                    })),
                })
            })
            .collect();
        self.finish(json!({ "conversations": conversations })).await
    }

    /// `get_conversation_messages`.
    pub async fn get_conversation_messages(&self, request: ConversationRequest) -> Result<Value> {
        let me = self.me().await?;
        let raw = self
            .call(
                "core_message_get_conversation_messages",
                json!({
                    "currentuserid": me.userid,
                    "convid": request.conversation_id,
                    "limitnum": request.limit.clamp(1, 100),
                    "newest": true,
                }),
            )
            .await?;
        let mut names: HashMap<i64, String> = raw
            .list("members")
            .iter()
            .filter_map(|m| Some((m.int("id")?, m.str("fullname")?.to_owned())))
            .collect();
        names.insert(me.userid, format!("{} (me)", me.fullname));
        let mut messages: Vec<Value> = raw
            .list("messages")
            .iter()
            .map(|m| {
                let from = m.int("useridfrom");
                json!({
                    "id": m.int("id"),
                    "from_user_id": from,
                    "from": from.and_then(|id| names.get(&id)),
                    "time": m.time("timecreated"),
                    "text": m.html("text"),
                })
            })
            .collect();
        messages.reverse();
        self.finish(json!({
            "conversation_id": request.conversation_id,
            "members": raw.list("members").iter().map(member).collect::<Vec<_>>(),
            "messages": messages,
        }))
        .await
    }

    /// `send_message`.
    pub async fn send_message(&self, request: SendMessageRequest) -> Result<Value> {
        if request.text.trim().is_empty() {
            return Err(Error::NotAllowed("the message text is empty".into()));
        }
        let raw = self
            .call(
                "core_message_send_instant_messages",
                json!({ "messages": [{
                    "touserid": request.to_user_id,
                    "text": text_to_html(&request.text),
                    "textformat": 1,
                }] }),
            )
            .await?;
        let sent = raw
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| Error::decode("no result for the sent message"))?;
        if sent.int("msgid").is_none_or(|id| id <= 0) {
            return Err(Error::Moodle {
                errorcode: "messagenotsent".into(),
                message: sent
                    .str("errormessage")
                    .unwrap_or("the message could not be sent")
                    .to_owned(),
            });
        }
        self.finish(json!({
            "message_id": sent.int("msgid"),
            "conversation_id": sent.int("conversationid"),
            "time": sent.time("timecreated"),
        }))
        .await
    }
}

fn notification(n: &Value) -> Value {
    // `fullmessage` is the plain-text rendering; fall back to the HTML.
    let text = n
        .str("fullmessage")
        .map(|t| match n.int("fullmessageformat") {
            Some(1) => crate::text::html_to_text(t),
            _ => t.trim().to_owned(),
        })
        .filter(|t| !t.is_empty())
        .or_else(|| n.html("fullmessagehtml"))
        .or_else(|| n.html("smallmessage"));
    json!({
        "id": n.int("id"),
        "subject": n.html("subject"),
        "text": preview(text, NOTIFICATION_CHARS),
        "from_user_id": n.int("useridfrom").filter(|id| *id > 0),
        "time": n.time("timecreated"),
        "read": n.flag("read"),
        "url": n.str("contexturl"),
        "url_name": n.html("contexturlname"),
        "component": n.str("component"),
        "event": n.str("eventtype"),
    })
}

fn member(m: &Value) -> Value {
    json!({
        "id": m.int("id"),
        "fullname": m.str("fullname"),
        "deleted": m.flag("isdeleted").filter(|d| *d),
        "can_message": m.flag("canmessage"),
    })
}
