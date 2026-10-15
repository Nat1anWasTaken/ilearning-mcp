//! Upcoming deadlines from the calendar's action events.

use serde_json::{Value, json};
use url::Url;

use super::preview;
use crate::Account;
use crate::error::Result;
use crate::text::{self, Get};
use crate::types::DeadlinesRequest;

const OVERDUE_WINDOW_SECS: i64 = 7 * 86_400;
const MAX_EVENTS: u32 = 50;

impl Account {
    /// `get_upcoming_deadlines`.
    pub async fn get_upcoming_deadlines(&self, request: DeadlinesRequest) -> Result<Value> {
        let now = chrono::Utc::now().timestamp();
        let from = now - OVERDUE_WINDOW_SECS;
        let to = now + i64::from(request.days.clamp(1, 365)) * 86_400;
        let raw = match request.course_id {
            Some(course_id) => {
                self.call(
                    "core_calendar_get_action_events_by_course",
                    json!({ "courseid": course_id, "timesortfrom": from, "timesortto": to, "limitnum": MAX_EVENTS }),
                )
                .await?
            }
            None => {
                self.call(
                    "core_calendar_get_action_events_by_timesort",
                    json!({
                        "timesortfrom": from, "timesortto": to, "limitnum": MAX_EVENTS,
                        "limittononsuspendedevents": true,
                    }),
                )
                .await?
            }
        };
        let events: Vec<Value> = raw.list("events").iter().map(event).collect();
        self.finish(json!({
            "from": text::time(Some(from)),
            "to": text::time(Some(to)),
            "events": events,
        }))
        .await
    }
}

fn event(e: &Value) -> Value {
    let url = e.str("url");
    // Activity URLs are `…/mod/<name>/view.php?id=<cmid>`.
    let cmid = url
        .and_then(|u| Url::parse(u).ok())
        .filter(|u| u.path().ends_with("/view.php") && u.path().contains("/mod/"))
        .and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "id")
                .and_then(|(_, v)| v.parse::<i64>().ok())
        });
    let course = e
        .get("course")
        .map(|c| json!({ "id": c.int("id"), "name": c.html("fullname") }));
    let action = e.get("action");
    json!({
        "id": e.int("id"),
        "name": e.html("name"),
        "activity": e.html("activityname"),
        "type": e.str("modulename"),
        "event": e.str("activitystr"),
        "course": course,
        "time": e.time("timesort"),
        "overdue": e.flag("overdue").filter(|o| *o),
        "action": action.and_then(|a| a.str("name")),
        "actionable": action.and_then(|a| a.flag("actionable")),
        "cmid": cmid,
        "instance": e.int("instance"),
        "url": url,
        "description": preview(e.html("description"), 500),
    })
}
