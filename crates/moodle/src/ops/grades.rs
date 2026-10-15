//! Grades: course totals overview and per-course grade items.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::Account;
use crate::error::Result;
use crate::text::Get;
use crate::types::GradesRequest;

impl Account {
    /// `get_grades`.
    pub async fn get_grades(&self, request: GradesRequest) -> Result<Value> {
        let userid = self.me().await?.userid;
        let Some(course_id) = request.course_id else {
            let (grades, courses) = tokio::try_join!(
                self.call("gradereport_overview_get_course_grades", json!({})),
                self.call("core_enrol_get_users_courses", json!({ "userid": userid })),
            )?;
            let names: HashMap<i64, String> = courses
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| Some((c.int("id")?, c.html("fullname")?)))
                .collect();
            let courses: Vec<Value> = grades
                .list("grades")
                .iter()
                .map(|g| {
                    let id = g.int("courseid");
                    json!({
                        "course_id": id,
                        "course": id.and_then(|id| names.get(&id)),
                        "grade": g.str("grade").filter(|s| *s != "-"),
                        "rank": g.int("rank"),
                    })
                })
                .collect();
            return self
                .finish(json!({ "courses": courses, "note": "courses without a grade yet have no `grade`" }))
                .await;
        };
        // Students must pass their own user ID; without it Moodle checks
        // the capability to view everyone's grades.
        let raw = self
            .call(
                "gradereport_user_get_grade_items",
                json!({ "courseid": course_id, "userid": userid }),
            )
            .await?;
        let report = raw
            .list("usergrades")
            .first()
            .cloned()
            .unwrap_or(Value::Null);
        let items: Vec<Value> = report.list("gradeitems").iter().map(grade_item).collect();
        self.finish(json!({ "course_id": course_id, "items": items }))
            .await
    }
}

fn grade_item(item: &Value) -> Value {
    let kind = item.str("itemtype").unwrap_or_default();
    let name = item.html("itemname").unwrap_or_else(|| {
        match kind {
            "course" => "Course total",
            "category" => "Category total",
            _ => "",
        }
        .to_owned()
    });
    let formatted = |key: &str| item.html(key).filter(|s| s != "-");
    json!({
        "id": item.int("id"),
        "name": name,
        "type": kind,
        "module": item.str("itemmodule"),
        "instance": item.int("iteminstance").filter(|_| kind == "mod"),
        "cmid": item.int("cmid"),
        "grade": formatted("gradeformatted"),
        "range": formatted("rangeformatted"),
        "percentage": formatted("percentageformatted"),
        "weight": formatted("weightformatted"),
        "feedback": item.html("feedback"),
        "graded_at": item.time("gradedategraded"),
        "submitted_at": item.time("gradedatesubmitted"),
        "hidden": item.flag("gradeishidden").filter(|h| *h),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_total_items() {
        let item = json!({"id": 1, "itemname": null, "itemtype": "course", "iteminstance": 3,
            "gradeformatted": "-", "rangeformatted": "0&ndash;100", "percentageformatted": "-", "feedback": ""});
        let value = crate::text::finish(grade_item(&item), "en");
        assert_eq!(value["name"], "Course total");
        assert_eq!(value["range"], "0–100");
        assert!(value.get("grade").is_none() && value.get("instance").is_none());
    }
}
