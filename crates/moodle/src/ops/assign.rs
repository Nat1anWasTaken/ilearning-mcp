//! Assignments: listing, submission status, and submitting.

use base64::Engine;
use serde_json::{Map, Value, json};

use super::{files, preview};
use crate::Account;
use crate::error::{Error, Result};
use crate::files::UploadFile;
use crate::text::{Get, text_to_html};
use crate::types::{AssignmentRequest, CourseIdsRequest, SubmitAssignmentRequest};

impl Account {
    /// `list_assignments`.
    pub async fn list_assignments(&self, request: CourseIdsRequest) -> Result<Value> {
        let raw = self
            .call(
                "mod_assign_get_assignments",
                json!({ "courseids": request.course_ids.unwrap_or_default() }),
            )
            .await?;
        let courses: Vec<Value> = raw
            .list("courses")
            .iter()
            .filter(|c| !c.list("assignments").is_empty())
            .map(|c| {
                json!({
                    "id": c.int("id"),
                    "name": c.html("fullname"),
                    "assignments": c.list("assignments").iter().map(assignment).collect::<Vec<_>>(),
                })
            })
            .collect();
        let note = courses
            .is_empty()
            .then_some("no assignments found in these courses");
        self.finish(json!({ "courses": courses, "note": note }))
            .await
    }

    /// `get_assignment_status`.
    pub async fn get_assignment_status(&self, request: AssignmentRequest) -> Result<Value> {
        let raw = self
            .call(
                "mod_assign_get_submission_status",
                json!({ "assignid": request.assignment_id }),
            )
            .await?;
        self.finish(submission_status(request.assignment_id, &raw))
            .await
    }

    /// `submit_assignment`.
    pub async fn submit_assignment(&self, request: SubmitAssignmentRequest) -> Result<Value> {
        if request.online_text.is_none() && request.files.is_empty() && !request.submit_for_grading
        {
            return Err(Error::NotAllowed(
                "give online_text, files, or submit_for_grading".into(),
            ));
        }
        let mut uploads = Vec::with_capacity(request.files.len());
        for file in request.files {
            let bytes = match (file.text, file.base64) {
                (Some(text), None) => text.into_bytes(),
                (None, Some(encoded)) => {
                    let cleaned: String = encoded.split_whitespace().collect();
                    base64::engine::general_purpose::STANDARD
                        .decode(cleaned)
                        .map_err(|_| {
                            Error::NotAllowed(format!("{}: base64 is invalid", file.filename))
                        })?
                }
                _ => {
                    return Err(Error::NotAllowed(format!(
                        "{}: give exactly one of text or base64",
                        file.filename
                    )));
                }
            };
            if file.filename.trim().is_empty() || file.filename.contains(['/', '\\']) {
                return Err(Error::NotAllowed(format!(
                    "invalid file name {:?}",
                    file.filename
                )));
            }
            uploads.push(UploadFile {
                filename: file.filename,
                bytes,
            });
        }

        let mut plugindata = Map::new();
        if let Some(text) = &request.online_text {
            plugindata.insert(
                "onlinetext_editor".into(),
                json!({ "text": text_to_html(text), "format": 1, "itemid": 0 }),
            );
        }
        if !uploads.is_empty() {
            let itemid = self.upload_draft(uploads).await?;
            plugindata.insert("files_filemanager".into(), json!(itemid));
        }
        if !plugindata.is_empty() {
            let warnings = self
                .call(
                    "mod_assign_save_submission",
                    json!({ "assignmentid": request.assignment_id, "plugindata": plugindata }),
                )
                .await?;
            check_warnings(&warnings)?;
        }
        if request.submit_for_grading {
            let warnings = self
                .call(
                    "mod_assign_submit_for_grading",
                    json!({ "assignmentid": request.assignment_id, "acceptsubmissionstatement": true }),
                )
                .await?;
            check_warnings(&warnings)?;
        }
        let status = self
            .get_assignment_status(AssignmentRequest {
                assignment_id: request.assignment_id,
            })
            .await?;
        Ok(json!({
            "saved": !plugindata.is_empty(),
            "submitted_for_grading": request.submit_for_grading,
            "status": status,
        }))
    }
}

/// Fails with the first warning of a `{warnings: [...]}` / `[...]` result.
fn check_warnings(result: &Value) -> Result<()> {
    let warnings = match result {
        Value::Array(items) => items.as_slice(),
        other => other.list("warnings"),
    };
    match warnings.first() {
        None => Ok(()),
        Some(w) => Err(Error::Moodle {
            errorcode: w
                .str("warningcode")
                .or_else(|| w.str("item"))
                .unwrap_or("warning")
                .to_owned(),
            message: w.str("message").unwrap_or_default().to_owned(),
        }),
    }
}

fn assignment(a: &Value) -> Value {
    let configs = a.list("configs");
    let config = |plugin: &str, name: &str| {
        configs
            .iter()
            .find(|c| {
                c.str("subtype") == Some("assignsubmission")
                    && c.str("plugin") == Some(plugin)
                    && c.str("name") == Some(name)
            })
            .and_then(|c| c.str("value"))
    };
    let enabled = |plugin: &str| config(plugin, "enabled") == Some("1");
    let mut types = Vec::new();
    if enabled("onlinetext") {
        types.push("online_text");
    }
    if enabled("file") {
        types.push("file");
    }
    let file_rules = enabled("file").then(|| {
        json!({
            "max_files": config("file", "maxfilesubmissions").and_then(|v| v.parse::<i64>().ok()),
            "max_bytes": config("file", "maxsubmissionsizebytes").and_then(|v| v.parse::<i64>().ok()).filter(|b| *b > 0),
            "accepted_types": config("file", "filetypeslist"),
        })
    });
    let mut attachments = files(a.list("introattachments"));
    attachments.extend(files(a.list("introfiles")));
    json!({
        "id": a.int("id"),
        "cmid": a.int("cmid"),
        "name": a.html("name"),
        "allow_submissions_from": a.time("allowsubmissionsfromdate"),
        "due": a.time("duedate"),
        "cutoff": a.time("cutoffdate"),
        "time_limit_secs": a.int("timelimit").filter(|t| *t > 0),
        "max_grade": a.int("grade"),
        "submission_types": types,
        "file_rules": file_rules,
        "word_limit": config("onlinetext", "wordlimit").filter(|_| config("onlinetext", "wordlimitenabled") == Some("1")),
        "drafts": a.flag("submissiondrafts"),
        "requires_statement": a.flag("requiresubmissionstatement").filter(|r| *r),
        "statement": a.html("submissionstatement"),
        "team_submission": a.flag("teamsubmission").filter(|t| *t),
        "max_attempts": a.int("maxattempts").filter(|m| *m > 0),
        "instructions": preview(a.html("intro"), 2000),
        "attachments": (!attachments.is_empty()).then_some(attachments),
    })
}

fn plugin_files(plugins: &[Value]) -> Vec<Value> {
    plugins
        .iter()
        .flat_map(|p| p.list("fileareas"))
        .flat_map(|area| files(area.list("files")))
        .collect()
}

fn plugin_text(plugins: &[Value]) -> Option<String> {
    let texts: Vec<String> = plugins
        .iter()
        .flat_map(|p| p.list("editorfields"))
        .filter_map(|f| f.html("text"))
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n\n"))
}

fn submission_status(assignment_id: i64, raw: &Value) -> Value {
    let last = raw.get("lastattempt").unwrap_or(&Value::Null);
    let submission = last
        .get("submission")
        .filter(|s| !s.is_null())
        .or_else(|| last.get("teamsubmission").filter(|s| !s.is_null()))
        .unwrap_or(&Value::Null);
    let plugins = submission.list("plugins");
    let submitted_files = plugin_files(plugins);
    let feedback = raw.get("feedback").filter(|f| !f.is_null()).map(|f| {
        let plugins = f.list("plugins");
        let files = plugin_files(plugins);
        json!({
            "grade": f.html("gradefordisplay"),
            "graded_at": f.time("gradeddate"),
            "comments": plugin_text(plugins),
            "files": (!files.is_empty()).then_some(files),
        })
    });
    let data = raw.get("assignmentdata").unwrap_or(&Value::Null);
    let mut attachments: Vec<Value> = Vec::new();
    if let Some(groups) = data.get("attachments") {
        for key in ["intro", "activity"] {
            attachments.extend(files(groups.list(key)));
        }
    }
    json!({
        "assignment_id": assignment_id,
        "status": submission.str("status"),
        "grading_status": last.str("gradingstatus"),
        "submissions_enabled": last.flag("submissionsenabled"),
        "can_edit": last.flag("canedit"),
        "can_submit": last.flag("cansubmit"),
        "locked": last.flag("locked").filter(|l| *l),
        "extension_due": last.time("extensionduedate"),
        "modified": submission.time("timemodified"),
        "attempt": submission.int("attemptnumber").map(|n| n + 1),
        "online_text": plugin_text(plugins),
        "files": (!submitted_files.is_empty()).then_some(submitted_files),
        "feedback": feedback,
        "activity_instructions": data.html("activity"),
        "attachments": (!attachments.is_empty()).then_some(attachments),
        "warnings": raw.list("warnings").iter().filter_map(|w| w.str("message")).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_submission_status() {
        let raw = json!({
            "lastattempt": {
                "submission": {
                    "id": 9, "status": "submitted", "timemodified": 1790772095, "attemptnumber": 0,
                    "plugins": [
                        {"type": "file", "name": "File submissions", "fileareas": [{"area": "submission_files", "files": [
                            {"filename": "a.pdf", "filepath": "/", "filesize": 10, "fileurl": "https://x/webservice/pluginfile.php/1/a.pdf", "mimetype": "application/pdf", "timemodified": 1790772095}
                        ]}]},
                        {"type": "onlinetext", "name": "Online text", "editorfields": [{"name": "onlinetext", "text": "<p>answer</p>", "format": 1}]}
                    ]
                },
                "submissionsenabled": true, "cansubmit": false, "canedit": true, "gradingstatus": "notgraded", "locked": false
            },
            "feedback": {"gradefordisplay": "<span>90.00 / 100.00</span>", "gradeddate": 1790772095, "plugins": [
                {"type": "comments", "editorfields": [{"name": "comments", "text": "Good", "format": 1}]}
            ]},
            "warnings": []
        });
        let value = crate::text::finish(submission_status(5, &raw), "en");
        assert_eq!(value["status"], "submitted");
        assert_eq!(value["online_text"], "answer");
        assert_eq!(value["files"][0]["filename"], "a.pdf");
        assert_eq!(value["attempt"], 1);
        assert_eq!(value["feedback"]["grade"], "90.00 / 100.00");
        assert_eq!(value["feedback"]["comments"], "Good");
        assert!(value.get("locked").is_none());
    }

    #[test]
    fn reads_assignment_configs() {
        let a = json!({
            "id": 7, "cmid": 70, "name": "HW1", "duedate": 1790772095, "cutoffdate": 0, "grade": 100,
            "submissiondrafts": 1, "requiresubmissionstatement": 0, "intro": "<p>Do it</p>",
            "configs": [
                {"plugin": "file", "subtype": "assignsubmission", "name": "enabled", "value": "1"},
                {"plugin": "file", "subtype": "assignsubmission", "name": "maxfilesubmissions", "value": "3"},
                {"plugin": "file", "subtype": "assignsubmission", "name": "filetypeslist", "value": ".pdf"},
                {"plugin": "onlinetext", "subtype": "assignsubmission", "name": "enabled", "value": "0"},
                {"plugin": "comments", "subtype": "assignfeedback", "name": "enabled", "value": "1"}
            ]
        });
        let value = crate::text::finish(assignment(&a), "en");
        assert_eq!(value["submission_types"], json!(["file"]));
        assert_eq!(value["file_rules"]["max_files"], 3);
        assert_eq!(value["instructions"], "Do it");
        assert!(value.get("cutoff").is_none());
        assert_eq!(value["drafts"], true);
    }

    #[test]
    fn warnings_become_errors() {
        assert!(check_warnings(&json!([])).is_ok());
        assert!(check_warnings(&json!({"warnings": []})).is_ok());
        assert!(check_warnings(&Value::Null).is_ok());
        let error = check_warnings(&json!([{"item": "assign", "warningcode": "couldnotsavesubmission", "message": "nope"}]))
            .unwrap_err();
        assert!(
            matches!(error, Error::Moodle { errorcode, .. } if errorcode == "couldnotsavesubmission")
        );
    }
}
