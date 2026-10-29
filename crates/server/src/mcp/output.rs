//! Turning operation results into MCP tool results.

use base64::{Engine, engine::general_purpose::STANDARD};
use moodle::{FileChunk, FileContent};
use rmcp::model::{CallToolResult, ContentBlock, ResourceContents};
use serde_json::{Value, json};

use super::log;

/// The result of an iLearning tool call.
pub enum Output {
    Json(Value),
    File(FileContent),
    Bytes(FileChunk),
}

impl Output {
    /// The tool result, and what the call log keeps of it: everything but
    /// file contents. `link` turns the URL of a file that cannot be shown
    /// inline into a download link for the agent.
    pub fn into_parts(self, link: impl FnOnce(&str) -> String) -> (CallToolResult, Option<Value>) {
        match self {
            Self::Json(value) => {
                let text = render(&value);
                let logged = log::bounded(value, &text);
                (success(text), Some(logged))
            }
            Self::File(FileContent::Text {
                filename,
                mime,
                text,
                truncated,
            }) => {
                let header = json!({
                    "filename": filename,
                    "mime": mime,
                    "chars": text.chars().count(),
                    "truncated": truncated,
                });
                let result = CallToolResult::success(vec![
                    ContentBlock::text(header.to_string()),
                    ContentBlock::text(text),
                ]);
                (result, Some(header))
            }
            Self::File(FileContent::Image {
                filename,
                mime,
                bytes,
            }) => {
                let logged = json!({ "filename": filename, "mime": mime, "size": bytes.len() });
                let result = CallToolResult::success(vec![
                    ContentBlock::text(logged.to_string()),
                    ContentBlock::image(STANDARD.encode(&bytes), mime),
                ]);
                (result, Some(logged))
            }
            Self::File(FileContent::Binary {
                filename,
                mime,
                size,
                file_url,
            }) => {
                let download_url = link(&file_url);
                let logged = json!({ "filename": filename, "mime": mime, "size": size });
                let text = format!(
                    "{filename} ({mime}, {size} bytes) is not returned as text or an image. \
                     Download link to the original file, valid for one hour, to give the user \
                     or fetch: {download_url}"
                );
                (success(text), Some(logged))
            }
            Self::Bytes(FileChunk {
                filename,
                mime,
                file_url,
                size,
                offset,
                bytes,
                next_offset,
            }) => {
                let header = json!({
                    "filename": filename,
                    "mime": mime,
                    "size": size,
                    "offset": offset,
                    "length": bytes.len(),
                    "next_offset": next_offset,
                    "encoding": "base64",
                });
                let blob =
                    ResourceContents::blob(STANDARD.encode(&bytes), file_url).with_mime_type(mime);
                let result = CallToolResult::success(vec![
                    ContentBlock::text(header.to_string()),
                    ContentBlock::resource(blob),
                ]);
                (result, Some(header))
            }
        }
    }
}

/// How a successful result reads to the agent: pretty JSON.
pub fn render(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

pub fn success(text: String) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(text)])
}

/// A tool result the agent sees as a failure.
pub fn failure(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

pub trait IntoOutput {
    fn into_output(self) -> Output;
}

impl IntoOutput for Value {
    fn into_output(self) -> Output {
        Output::Json(self)
    }
}

impl IntoOutput for FileContent {
    fn into_output(self) -> Output {
        Output::File(self)
    }
}

impl IntoOutput for FileChunk {
    fn into_output(self) -> Output {
        Output::Bytes(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(result: &CallToolResult, index: usize) -> &str {
        &result.content[index].as_text().unwrap().text
    }

    fn no_link(_: &str) -> String {
        unreachable!("no link needed")
    }

    #[test]
    fn json_results_render_for_agents() {
        let (result, logged) = Output::Json(json!({ "id": 1 })).into_parts(no_link);
        assert_eq!(result.is_error, Some(false));
        assert_eq!(text(&result, 0), "{\n  \"id\": 1\n}");
        assert_eq!(logged, Some(json!({ "id": 1 })));
        assert_eq!(failure("nope").is_error, Some(true));
    }

    #[test]
    fn files_are_logged_without_their_content() {
        let file = FileContent::Text {
            filename: "a.pdf".into(),
            mime: "application/pdf".into(),
            text: "hello".into(),
            truncated: false,
        };
        let (result, logged) = Output::File(file).into_parts(no_link);
        assert_eq!(text(&result, 1), "hello");
        assert_eq!(
            logged,
            Some(
                json!({ "filename": "a.pdf", "mime": "application/pdf", "chars": 5, "truncated": false })
            )
        );

        let file = FileContent::Image {
            filename: "a.png".into(),
            mime: "image/png".into(),
            bytes: vec![1, 2, 3],
        };
        let (result, logged) = Output::File(file).into_parts(no_link);
        assert!(result.content[1].as_image().is_some());
        assert_eq!(logged.unwrap()["size"], 3);

        let file = FileContent::Binary {
            filename: "a.ppt".into(),
            mime: "application/vnd.ms-powerpoint".into(),
            size: 10,
            file_url: "https://ilearning.example/webservice/pluginfile.php/1/a.ppt".into(),
        };
        let (result, logged) =
            Output::File(file).into_parts(|url| format!("https://mcp.example/files/x?{url}"));
        assert!(text(&result, 0).contains("https://mcp.example/files/x?"));
        assert_eq!(logged.unwrap()["filename"], "a.ppt");

        let chunk = FileChunk {
            filename: "a.docx".into(),
            mime: "application/octet-stream".into(),
            file_url: "https://ilearning.example/webservice/pluginfile.php/1/a.docx".into(),
            size: Some(10),
            offset: 4,
            bytes: vec![1, 2, 3],
            next_offset: Some(7),
        };
        let (result, logged) = Output::Bytes(chunk).into_parts(no_link);
        let resource = result.content[1].as_resource().unwrap();
        let ResourceContents::BlobResourceContents { blob, .. } = &resource.resource else {
            panic!("not a blob");
        };
        assert_eq!(blob, "AQID");
        assert_eq!(
            logged,
            Some(json!({
                "filename": "a.docx", "mime": "application/octet-stream", "size": 10,
                "offset": 4, "length": 3, "next_offset": 7, "encoding": "base64",
            }))
        );
        assert_eq!(text(&result, 0), logged.unwrap().to_string());
    }
}
