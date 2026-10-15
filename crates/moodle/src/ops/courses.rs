//! Courses, their contents, and single course modules.

use serde_json::{Value, json};

use super::{files, preview};
use crate::Account;
use crate::error::{Error, Result};
use crate::text::Get;
use crate::types::{CourseRequest, ListCoursesRequest, ModuleRequest};

/// Longest module description in course contents (full text via
/// `get_module`).
const DESCRIPTION_CHARS: usize = 600;

impl Account {
    /// `list_courses`.
    pub async fn list_courses(&self, request: ListCoursesRequest) -> Result<Value> {
        let raw = self
            .call(
                "core_course_get_enrolled_courses_by_timeline_classification",
                json!({ "classification": request.classification.as_str(), "limit": 0, "offset": 0 }),
            )
            .await?;
        let courses: Vec<Value> = raw
            .list("courses")
            .iter()
            .map(|c| {
                json!({
                    "id": c.int("id"),
                    "name": c.html("fullname"),
                    "shortname": c.html("shortname"),
                    "category": c.str("coursecategory"),
                    "start": c.time("startdate"),
                    "end": c.time("enddate"),
                    "progress_percent": c.get("progress").filter(|_| c.flag("hasprogress") == Some(true)),
                    "summary": preview(c.html("summary"), DESCRIPTION_CHARS),
                    "favourite": c.flag("isfavourite").filter(|f| *f),
                    "url": c.str("viewurl"),
                })
            })
            .collect();
        self.finish(json!({ "courses": courses })).await
    }

    /// `get_course_contents`.
    pub async fn get_course_contents(&self, request: CourseRequest) -> Result<Value> {
        let raw = self
            .call(
                "core_course_get_contents",
                json!({ "courseid": request.course_id }),
            )
            .await?;
        let sections: Vec<Value> = raw
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter(|s| !s.list("modules").is_empty() || s.html("summary").is_some())
            .map(|s| {
                json!({
                    "id": s.int("id"),
                    "section": s.int("section"),
                    "name": s.html("name"),
                    "summary": s.html("summary"),
                    "hidden": (s.flag("uservisible") == Some(false)).then_some(true),
                    "modules": s.list("modules").iter().map(|m| module(m, DESCRIPTION_CHARS)).collect::<Vec<_>>(),
                })
            })
            .collect();
        self.finish(json!({ "course_id": request.course_id, "sections": sections }))
            .await
    }

    /// `get_module`.
    pub async fn get_module(&self, request: ModuleRequest) -> Result<Value> {
        let raw = self
            .call(
                "core_course_get_contents",
                json!({
                    "courseid": request.course_id,
                    "options": [{ "name": "cmid", "value": request.cmid }],
                }),
            )
            .await?;
        let found = raw
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|s| s.list("modules"))
            .find(|m| m.int("id") == Some(request.cmid))
            .ok_or_else(|| Error::Moodle {
                errorcode: "modulenotfound".into(),
                message: format!(
                    "course {} has no module with cmid {} visible to you",
                    request.course_id, request.cmid
                ),
            })?;
        let mut out = module(found, usize::MAX);
        let instance = found.int("instance");
        let courseids = json!({ "courseids": [request.course_id] });
        let by_cmid = |list: &[Value]| -> Option<Value> {
            list.iter()
                .find(|item| item.int("coursemodule") == Some(request.cmid))
                .cloned()
        };
        match found.str("modname").unwrap_or_default() {
            "page" => {
                let pages = self
                    .call("mod_page_get_pages_by_courses", courseids)
                    .await?;
                if let Some(page) = by_cmid(pages.list("pages")) {
                    out["intro"] = json!(page.html("intro"));
                    out["content"] = json!(page.html("content"));
                    out["content_files"] = json!(files(page.list("contentfiles")));
                    out["modified"] = json!(page.time("timemodified"));
                }
            }
            "url" => {
                let urls = self.call("mod_url_get_urls_by_courses", courseids).await?;
                if let Some(url) = by_cmid(urls.list("urls")) {
                    out["intro"] = json!(url.html("intro"));
                    out["external_url"] = json!(url.str("externalurl"));
                }
            }
            "resource" => {
                let resources = self
                    .call("mod_resource_get_resources_by_courses", courseids)
                    .await?;
                if let Some(resource) = by_cmid(resources.list("resources")) {
                    out["intro"] = json!(resource.html("intro"));
                    out["files"] = json!(files(resource.list("contentfiles")));
                }
            }
            "assign" => {
                out["hint"] = json!(format!(
                    "use get_assignment_status with assignment_id {}",
                    instance.unwrap_or_default()
                ));
            }
            "forum" => {
                out["hint"] = json!(format!(
                    "use list_forum_discussions with forum_id {}",
                    instance.unwrap_or_default()
                ));
            }
            _ => {}
        }
        self.finish(out).await
    }
}

/// A course module as listed in course contents.
fn module(m: &Value, description_chars: usize) -> Value {
    let modname = m.str("modname").unwrap_or_default();
    let contents = m.list("contents");
    let external_url = (modname == "url")
        .then(|| contents.iter().find_map(|c| c.str("fileurl")))
        .flatten();
    let dates: Vec<Value> = m
        .list("dates")
        .iter()
        .map(|d| json!({ "label": d.str("label"), "time": d.time("timestamp") }))
        .collect();
    let subsection = (modname == "subsection")
        .then(|| {
            let data: Value = serde_json::from_str(m.str("customdata")?).ok()?;
            data.int("sectionid")
        })
        .flatten();
    let files = files(contents);
    json!({
        "cmid": m.int("id"),
        "modname": modname,
        "instance": m.int("instance"),
        "name": m.html("name"),
        "url": m.str("url"),
        "description": preview(m.html("description"), description_chars),
        "external_url": external_url,
        "section_id": subsection,
        "dates": (!dates.is_empty()).then_some(dates),
        "files": (!files.is_empty()).then_some(files),
        "available": (m.flag("uservisible") == Some(false)).then_some(false),
        "availability": m.html("availabilityinfo"),
        "completed": m.get("completiondata").and_then(|c| c.int("state")).map(|s| s > 0),
    })
}
