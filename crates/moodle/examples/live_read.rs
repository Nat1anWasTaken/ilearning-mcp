//! Read-only smoke test against the live site.
//!
//! Reads `{"wwwroot","token"}` from `.token.json` (or the path in the first
//! argument) and runs every read operation, printing result shapes rather
//! than content. Never calls write operations or renewal.
//!
//! ```sh
//! cargo run -p moodle --example live_read
//! ```

use moodle::types::*;
use moodle::{FileContent, Moodle};
use serde_json::Value;

/// Keys of an object and, for arrays, the length and first element's shape.
fn shape(value: &Value, depth: usize) -> String {
    match value {
        Value::Object(map) if depth > 0 => {
            let fields: Vec<String> = map
                .iter()
                .map(|(k, v)| match v {
                    Value::Object(_) | Value::Array(_) => format!("{k}: {}", shape(v, depth - 1)),
                    _ => k.clone(),
                })
                .collect();
            format!("{{{}}}", fields.join(", "))
        }
        Value::Array(items) if depth > 0 => match items.first() {
            Some(first) => format!("[{} × {}]", items.len(), shape(first, depth - 1)),
            None => "[]".into(),
        },
        Value::Object(_) => "{…}".into(),
        Value::Array(items) => format!("[{}]", items.len()),
        _ => "_".into(),
    }
}

/// Set `DUMP=<tool name prefix>` to print that result in full.
fn report(name: &str, result: &moodle::Result<Value>) {
    if let (Ok(prefix), Ok(value)) = (std::env::var("DUMP"), result)
        && name.starts_with(&prefix)
    {
        println!("{name}: {}\n", serde_json::to_string_pretty(value).unwrap());
        return;
    }
    match result {
        Ok(value) => println!("{name}: {}\n", shape(value, 4)),
        Err(error) => println!("{name}: ERROR {error}\n"),
    }
}

#[tokio::main]
async fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| ".token.json".into());
    let creds: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("token file"))
        .expect("token JSON");
    let wwwroot = url::Url::parse(creds["wwwroot"].as_str().unwrap()).unwrap();
    let account = Moodle::new(wwwroot).account(creds["token"].as_str().unwrap().to_owned());

    let info = account.site_info().await.expect("site_info");
    println!(
        "site_info: userid={} lang={} sitename={}\n",
        info.userid, info.lang, info.sitename
    );
    report("get_profile", &account.get_profile(NoParams {}).await);

    let courses = account.list_courses(ListCoursesRequest::default()).await;
    report("list_courses", &courses);
    let course_ids: Vec<i64> = courses
        .as_ref()
        .map(|c| {
            c["courses"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|c| c["id"].as_i64())
                .collect()
        })
        .unwrap_or_default();
    report(
        "list_courses(past)",
        &account
            .list_courses(ListCoursesRequest {
                classification: Classification::Past,
            })
            .await,
    );

    // Walk course contents, remembering interesting modules and files.
    let mut files: Vec<(String, String)> = Vec::new();
    let mut modules: Vec<(i64, i64, String)> = Vec::new();
    let mut forums: Vec<i64> = Vec::new();
    for (index, &course_id) in course_ids.iter().enumerate() {
        let contents = account
            .get_course_contents(CourseRequest { course_id })
            .await;
        if index == 0 {
            report("get_course_contents", &contents);
        }
        let Ok(contents) = contents else { continue };
        for section in contents["sections"].as_array().unwrap() {
            for module in section["modules"].as_array().unwrap() {
                let modname = module["modname"].as_str().unwrap_or_default().to_owned();
                let cmid = module["cmid"].as_i64().unwrap();
                if !modules.iter().any(|(_, _, m)| *m == modname) {
                    modules.push((course_id, cmid, modname.clone()));
                }
                if modname == "forum" {
                    forums.push(module["instance"].as_i64().unwrap());
                }
                for file in module["files"].as_array().into_iter().flatten() {
                    files.push((
                        file["filename"].as_str().unwrap_or_default().to_owned(),
                        file["fileurl"].as_str().unwrap_or_default().to_owned(),
                    ));
                }
            }
        }
    }
    for (course_id, cmid, modname) in &modules {
        report(
            &format!("get_module({modname})"),
            &account
                .get_module(ModuleRequest {
                    course_id: *course_id,
                    cmid: *cmid,
                })
                .await,
        );
    }

    report(
        "get_upcoming_deadlines",
        &account
            .get_upcoming_deadlines(DeadlinesRequest::default())
            .await,
    );
    if let Some(&course_id) = course_ids.first() {
        report(
            "get_upcoming_deadlines(course)",
            &account
                .get_upcoming_deadlines(DeadlinesRequest {
                    days: 60,
                    course_id: Some(course_id),
                })
                .await,
        );
    }
    report(
        "list_assignments",
        &account.list_assignments(CourseIdsRequest::default()).await,
    );
    report(
        "get_grades",
        &account.get_grades(GradesRequest::default()).await,
    );
    if let Some(&course_id) = course_ids.first() {
        report(
            "get_grades(course)",
            &account
                .get_grades(GradesRequest {
                    course_id: Some(course_id),
                })
                .await,
        );
    }
    let mut discussion = None;
    for &forum_id in &forums {
        let result = account
            .list_forum_discussions(ForumRequest { forum_id, page: 0 })
            .await;
        if let Ok(value) = &result
            && let Some(id) = value["discussions"][0]["discussion_id"].as_i64()
        {
            report("list_forum_discussions", &result);
            discussion = Some(id);
            break;
        }
    }
    if let Some(discussion_id) = discussion {
        report(
            "get_discussion_posts",
            &account
                .get_discussion_posts(DiscussionRequest { discussion_id })
                .await,
        );
    }
    report(
        "get_notifications",
        &account
            .get_notifications(NotificationsRequest::default())
            .await,
    );
    report(
        "get_notifications(unread)",
        &account
            .get_notifications(NotificationsRequest {
                limit: 5,
                unread_only: true,
            })
            .await,
    );
    let conversations = account
        .list_conversations(ConversationsRequest::default())
        .await;
    report("list_conversations", &conversations);
    if let Some(conversation_id) = conversations
        .as_ref()
        .ok()
        .and_then(|c| c["conversations"][0]["id"].as_i64())
    {
        report(
            "get_conversation_messages",
            &account
                .get_conversation_messages(ConversationRequest {
                    conversation_id,
                    limit: 5,
                })
                .await,
        );
    }

    // One file per extension.
    let mut seen = Vec::new();
    for (filename, file_url) in files {
        let extension = filename
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if seen.contains(&extension) {
            continue;
        }
        seen.push(extension.clone());
        let started = std::time::Instant::now();
        let result = account
            .read_file(ReadFileRequest {
                file_url,
                max_chars: 20_000,
                mode: ReadMode::Text,
            })
            .await;
        let elapsed = started.elapsed().as_millis();
        match result {
            Ok(FileContent::Text {
                mime,
                text,
                truncated,
                ..
            }) => {
                let head: String = text
                    .chars()
                    .take(80)
                    .collect::<String>()
                    .replace('\n', " ⏎ ");
                println!(
                    "read_file(.{extension}): Text mime={mime} chars={} truncated={truncated} {elapsed}ms | {head}",
                    text.chars().count()
                )
            }
            Ok(FileContent::Image { mime, bytes, .. }) => {
                println!(
                    "read_file(.{extension}): Image mime={mime} bytes={} {elapsed}ms",
                    bytes.len()
                )
            }
            Ok(FileContent::Binary { mime, size, .. }) => {
                println!("read_file(.{extension}): Binary mime={mime} size={size} {elapsed}ms")
            }
            Err(error) => println!("read_file(.{extension}): ERROR {error}"),
        }
    }
    let rejected = account
        .read_file(ReadFileRequest {
            file_url: "https://example.com/webservice/pluginfile.php/1/x.pdf".into(),
            max_chars: 10,
            mode: ReadMode::Text,
        })
        .await;
    println!(
        "read_file(foreign host): {:?}",
        rejected.err().map(|e| e.to_string())
    );
}
