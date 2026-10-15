//! The REST web service transport: `POST /webservice/rest/server.php` with
//! PHP-style flattened form parameters.

use serde_json::Value;

use crate::Account;
use crate::error::{Error, Result};

impl Account {
    /// Calls a web service function. `params` must be a JSON object (or
    /// null); nested objects and arrays are flattened PHP-style
    /// (`courseids[0]=1`, `options[0][name]=x`) and booleans become `1`/`0`.
    ///
    /// Text is returned with Moodle filters applied (so multi-language names
    /// are resolved where Moodle formats them) and file URLs point at
    /// `/webservice/pluginfile.php`.
    pub async fn call(&self, wsfunction: &str, params: Value) -> Result<Value> {
        let mut form = vec![
            ("wstoken".to_owned(), self.token.to_string()),
            ("wsfunction".to_owned(), wsfunction.to_owned()),
            ("moodlewssettingfilter".to_owned(), "true".to_owned()),
            ("moodlewssettingfileurl".to_owned(), "true".to_owned()),
        ];
        match &params {
            Value::Null => {}
            Value::Object(_) => flatten("", &params, &mut form),
            _ => {
                return Err(Error::NotAllowed(
                    "web service parameters must be an object".into(),
                ));
            }
        }
        let mut url = self.moodle.endpoint("/webservice/rest/server.php");
        url.query_pairs_mut()
            .append_pair("moodlewsrestformat", "json")
            .append_pair("wsfunction", wsfunction);
        let response = self
            .moodle
            .http
            .post(url)
            .form(&form)
            .send()
            .await?
            .error_for_status()?;
        let body = response.text().await?;
        let value: Value = serde_json::from_str(&body).map_err(|error| {
            Error::Decode(format!("{wsfunction} did not return JSON ({error})"))
        })?;
        match Error::from_response(&value) {
            Some(error) => Err(error),
            None => Ok(value),
        }
    }
}

/// Flattens `value` into PHP form fields named after `prefix`. Nulls are
/// omitted (Moodle then applies the parameter's default).
pub(crate) fn flatten(prefix: &str, value: &Value, out: &mut Vec<(String, String)>) {
    let key = |name: &str| {
        if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}[{name}]")
        }
    };
    match value {
        Value::Null => {}
        Value::Bool(b) => out.push((prefix.to_owned(), if *b { "1" } else { "0" }.to_owned())),
        Value::Number(n) => out.push((prefix.to_owned(), n.to_string())),
        Value::String(s) => out.push((prefix.to_owned(), s.clone())),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                flatten(&key(&index.to_string()), item, out);
            }
        }
        Value::Object(map) => {
            for (name, item) in map {
                flatten(&key(name), item, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn flat(value: Value) -> Vec<(String, String)> {
        let mut out = Vec::new();
        flatten("", &value, &mut out);
        // Key order depends on serde_json's `preserve_order` feature.
        out.sort();
        out
    }

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut pairs: Vec<_> = items
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        pairs.sort();
        pairs
    }

    #[test]
    fn flattens_scalars() {
        assert_eq!(
            flat(json!({"courseid": 5, "name": "x", "newest": true, "old": false, "skip": null})),
            pairs(&[
                ("courseid", "5"),
                ("name", "x"),
                ("newest", "1"),
                ("old", "0")
            ])
        );
    }

    #[test]
    fn flattens_arrays_and_objects() {
        assert_eq!(
            flat(json!({
                "courseids": [3, 4],
                "options": [{"name": "discussionsubscribe", "value": true}],
                "plugindata": {"onlinetext_editor": {"text": "<p>hi</p>", "format": 1, "itemid": 0}}
            })),
            pairs(&[
                ("courseids[0]", "3"),
                ("courseids[1]", "4"),
                ("options[0][name]", "discussionsubscribe"),
                ("options[0][value]", "1"),
                ("plugindata[onlinetext_editor][text]", "<p>hi</p>"),
                ("plugindata[onlinetext_editor][format]", "1"),
                ("plugindata[onlinetext_editor][itemid]", "0"),
            ])
        );
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert!(flat(json!({})).is_empty());
        assert!(flat(json!({"ids": []})).is_empty());
    }
}
