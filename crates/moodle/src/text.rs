//! Text helpers: HTML to plain text, multi-language tags, timestamps and
//! JSON compaction for agent-facing output.

use chrono::{DateTime, FixedOffset};
use serde_json::{Map, Value};

/// Offset used for timestamps in agent output (Asia/Taipei, no DST).
const SITE_OFFSET_SECS: i32 = 8 * 3600;

/// Converts HTML to readable plain text. Links are kept as footnotes.
pub(crate) fn html_to_text(html: &str) -> String {
    if !html.contains('<') && !html.contains('&') {
        return tidy(html);
    }
    let text = html2text::config::plain()
        .string_from_read(html.as_bytes(), 100_000)
        .unwrap_or_else(|_| html.to_owned());
    tidy(&text)
}

/// Trims trailing spaces on each line and collapses runs of blank lines.
pub(crate) fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.replace("\r\n", "\n").lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        out.push_str(line);
    }
    out
}

/// Plain text (from an agent) as HTML for Moodle editors: escaped, with
/// paragraphs and line breaks preserved.
pub(crate) fn text_to_html(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    escaped
        .replace("\r\n", "\n")
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| format!("<p>{}</p>", p.trim_matches('\n').replace('\n', "<br>")))
        .collect()
}

/// Unix seconds as RFC 3339 in site time; `None` for 0 / missing.
pub(crate) fn time(seconds: Option<i64>) -> Option<String> {
    let seconds = seconds.filter(|s| *s > 0)?;
    let offset = FixedOffset::east_opt(SITE_OFFSET_SECS)?;
    Some(
        DateTime::from_timestamp(seconds, 0)?
            .with_timezone(&offset)
            .to_rfc3339(),
    )
}

/// Resolves `{mlang xx}…{mlang}` blocks (filter_multilang2 syntax) to the
/// variant for `lang` (e.g. `zh_tw`), falling back to `other`, then to the
/// first variant. Moodle leaves these unresolved in some responses, such as
/// notification texts.
pub(crate) fn mlang(text: &str, lang: &str) -> String {
    const OPEN: &str = "{mlang ";
    const CLOSE: &str = "{mlang}";
    if !text.contains(OPEN) {
        return text.to_owned();
    }
    let want = normalize_lang(lang);
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        // Collect a group of adjacent blocks.
        let mut variants: Vec<(String, &str)> = Vec::new();
        let mut cursor = &rest[start..];
        while let Some(block) = cursor.strip_prefix(OPEN) {
            let Some(tag_end) = block.find('}') else {
                break;
            };
            let body = &block[tag_end + 1..];
            let Some(close) = body.find(CLOSE) else { break };
            variants.push((normalize_lang(&block[..tag_end]), &body[..close]));
            cursor = &body[close + CLOSE.len()..];
            let trimmed = cursor.trim_start();
            if !trimmed.starts_with(OPEN) {
                break;
            }
            cursor = trimmed;
        }
        if variants.is_empty() {
            // Malformed tag: keep it literally and move on.
            out.push_str(OPEN);
            rest = &rest[start + OPEN.len()..];
            continue;
        }
        let base = want.split('_').next().unwrap_or_default();
        let pick = variants
            .iter()
            .find(|(langs, _)| langs.split(',').any(|l| l.trim() == want))
            .or_else(|| {
                variants
                    .iter()
                    .find(|(langs, _)| langs.split(',').any(|l| l.trim() == base))
            })
            .or_else(|| variants.iter().find(|(langs, _)| langs == "other"))
            .unwrap_or(&variants[0]);
        out.push_str(pick.1);
        rest = cursor;
    }
    out.push_str(rest);
    out
}

fn normalize_lang(lang: &str) -> String {
    lang.trim().to_ascii_lowercase().replace('-', "_")
}

/// Final pass over agent output: resolves multi-language tags in every
/// string and drops null / empty-string fields.
pub(crate) fn finish(value: Value, lang: &str) -> Value {
    match value {
        Value::String(s) => Value::String(mlang(&s, lang)),
        Value::Array(items) => Value::Array(items.into_iter().map(|v| finish(v, lang)).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, v)| !v.is_null() && v.as_str() != Some(""))
                .map(|(k, v)| (k, finish(v, lang)))
                .collect::<Map<_, _>>(),
        ),
        other => other,
    }
}

/// Field accessors on raw Moodle JSON.
pub(crate) trait Get {
    fn str(&self, key: &str) -> Option<&str>;
    fn int(&self, key: &str) -> Option<i64>;
    fn flag(&self, key: &str) -> Option<bool>;
    fn list(&self, key: &str) -> &[Value];
    /// `key` as HTML converted to text (`None` when empty).
    fn html(&self, key: &str) -> Option<String>;
    /// `key` as unix seconds converted to RFC 3339.
    fn time(&self, key: &str) -> Option<String>;
}

impl Get for Value {
    fn str(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    fn int(&self, key: &str) -> Option<i64> {
        let value = self.get(key)?;
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
            .or_else(|| value.as_bool().map(i64::from))
    }

    fn flag(&self, key: &str) -> Option<bool> {
        let value = self.get(key)?;
        value.as_bool().or_else(|| self.int(key).map(|i| i != 0))
    }

    fn list(&self, key: &str) -> &[Value] {
        self.get(key)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn html(&self, key: &str) -> Option<String> {
        Some(html_to_text(self.str(key)?)).filter(|s| !s.is_empty())
    }

    fn time(&self, key: &str) -> Option<String> {
        time(self.int(key))
    }
}

/// Truncates `text` to at most `max` characters, on a char boundary.
pub(crate) fn truncate(text: &mut String, max: usize) -> bool {
    match text.char_indices().nth(max) {
        Some((index, _)) => {
            text.truncate(index);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn resolves_mlang_groups() {
        let name = "{mlang zh-tw}1151線性代數{mlang}{mlang other}1151Linear Algebra{mlang}";
        assert_eq!(mlang(name, "zh_tw"), "1151線性代數");
        assert_eq!(mlang(name, "en"), "1151Linear Algebra");
        let sentence = format!("message in course {name} today");
        assert_eq!(
            mlang(&sentence, "zh_tw"),
            "message in course 1151線性代數 today"
        );
        assert_eq!(mlang("{mlang en}A{mlang} {mlang ja}B{mlang}", "fr"), "A");
        assert_eq!(mlang("{mlang zh_tw}中{mlang}", "zh_tw"), "中");
        assert_eq!(mlang("broken {mlang en}x", "en"), "broken {mlang en}x");
        assert_eq!(mlang("plain", "en"), "plain");
    }

    #[test]
    fn converts_html() {
        assert_eq!(html_to_text("<p>a</p>\r\n<p>b&amp;c</p>"), "a\n\nb&c");
        assert_eq!(html_to_text("plain\n\n\n\ntext  "), "plain\n\ntext");
        let list = html_to_text("<ol><li>one</li><li>two</li></ol>");
        assert!(list.contains("one") && list.contains("two"));
    }

    #[test]
    fn escapes_agent_text() {
        assert_eq!(
            text_to_html("Hi <b>\nline2\n\npara & more"),
            "<p>Hi &lt;b&gt;<br>line2</p><p>para &amp; more</p>"
        );
    }

    #[test]
    fn formats_times() {
        assert_eq!(time(Some(1790772095)).unwrap(), "2026-09-30T20:41:35+08:00");
        assert_eq!(time(Some(0)), None);
        assert_eq!(time(None), None);
    }

    #[test]
    fn finishes_output() {
        let value = finish(
            json!({"a": null, "b": "", "c": "{mlang en}x{mlang}{mlang other}y{mlang}", "d": [{"e": null}]}),
            "en",
        );
        assert_eq!(value, json!({"c": "x", "d": [{}]}));
    }

    #[test]
    fn truncates_on_char_boundary() {
        let mut s = "中文字".to_owned();
        assert!(truncate(&mut s, 2));
        assert_eq!(s, "中文");
        assert!(!truncate(&mut s, 5));
    }
}
