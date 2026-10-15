//! Operations an agent can run as the user, one [`Account`] method per
//! tool. Each returns compact JSON: HTML turned into text, unix times into
//! RFC 3339 (site time, +08:00), multi-language tags resolved, noise fields
//! dropped, and the IDs and `fileurl`s needed for follow-up calls kept.

mod assign;
mod calendar;
mod courses;
mod forum;
mod grades;
mod messages;

use serde_json::{Value, json};

use crate::Account;
use crate::error::Result;
use crate::text::{self, Get};
use crate::types::NoParams;

impl Account {
    /// `get_profile`: who the user is.
    pub async fn get_profile(&self, _request: NoParams) -> Result<Value> {
        let info = self.site_info().await?;
        self.finish(json!({
            "userid": info.userid,
            "username": info.username,
            "fullname": info.fullname,
            "site": info.sitename,
            "site_url": info.siteurl,
            "lang": info.lang,
            "picture_url": info.userpictureurl,
        }))
        .await
    }

    /// Final pass over a tool result (see [`text::finish`]).
    pub(crate) async fn finish(&self, value: Value) -> Result<Value> {
        let lang = self.me().await?.lang.clone();
        Ok(text::finish(value, &lang))
    }
}

/// File entries (`contents`, `contentfiles`, `attachments`, …) as
/// `{filename, fileurl, …}`; skips non-file entries such as URLs.
pub(crate) fn files(list: &[Value]) -> Vec<Value> {
    list.iter()
        .filter(|f| f.str("type").is_none_or(|t| t == "file") && f.str("fileurl").is_some())
        .map(|f| {
            json!({
                "filename": f.str("filename"),
                "path": f.str("filepath").filter(|p| *p != "/"),
                "fileurl": f.str("fileurl"),
                "filesize": f.int("filesize"),
                "mimetype": f.str("mimetype"),
                "modified": f.time("timemodified"),
            })
        })
        .collect()
}

/// `text`, cut to `max` characters with an ellipsis.
pub(crate) fn preview(text: Option<String>, max: usize) -> Option<String> {
    let mut text = text?;
    if text::truncate(&mut text, max) {
        text.push('…');
    }
    Some(text)
}
