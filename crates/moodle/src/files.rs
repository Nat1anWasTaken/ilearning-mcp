//! File transfer: downloads from `pluginfile.php`, uploads to the user's
//! draft area, and turning documents into text for agents.

use std::io::{Cursor, Read};

use reqwest::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use reqwest::multipart::{Form, Part};
use serde::Serialize;
use serde_json::Value;
use url::Url;

use crate::Account;
use crate::error::{Error, Result};
use crate::text::{self, Get};
use crate::types::ReadFileRequest;

/// Largest image [`Account::read_file`] returns inline.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// Largest file [`Account::read_file`] downloads to extract text.
const MAX_EXTRACT_BYTES: u64 = 50 * 1024 * 1024;

/// A downloaded file.
#[derive(Clone)]
pub struct Download {
    pub filename: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for Download {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Download")
            .field("filename", &self.filename)
            .field("mime", &self.mime)
            .field("size", &self.bytes.len())
            .finish()
    }
}

/// A file to put in the draft area.
#[derive(Clone, Debug)]
pub struct UploadFile {
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// What [`Account::read_file`] made of a file.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileContent {
    /// Extracted text (pdf, docx, pptx, xlsx, txt, md, csv, html, …).
    Text {
        filename: String,
        mime: String,
        text: String,
        /// Whether `text` was cut at `max_chars`.
        truncated: bool,
    },
    /// An image small enough to hand to the agent.
    Image {
        filename: String,
        mime: String,
        #[serde(skip)]
        bytes: Vec<u8>,
    },
    /// Anything else; the caller should offer a download link.
    Binary {
        filename: String,
        mime: String,
        size: u64,
        file_url: String,
    },
}

impl Account {
    /// Downloads a file from a `fileurl` in a web service response.
    pub async fn download(&self, file_url: &str) -> Result<Download> {
        let response = self.download_response(file_url).await?;
        let (filename, mime) = describe(&response);
        let bytes = response.bytes().await?.to_vec();
        Ok(Download {
            filename,
            mime,
            bytes,
        })
    }

    /// Starts downloading a file, for streaming. Only URLs on the site's
    /// `pluginfile.php` are allowed; the token is added here. Use
    /// [`file_name_and_mime`] to describe the response.
    pub async fn download_response(&self, file_url: &str) -> Result<reqwest::Response> {
        let url = self.file_url(file_url)?;
        let response = self.moodle.http.get(url).send().await?.error_for_status()?;
        let is_json = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/json"));
        if is_json && response.headers().get(CONTENT_DISPOSITION).is_none() {
            // pluginfile.php reports errors as JSON with HTTP 200.
            let body: Value = response.json().await?;
            return Err(Error::from_response(&body)
                .unwrap_or_else(|| Error::decode("file download returned JSON")));
        }
        Ok(response)
    }

    /// The download URL for `file_url` with the token: rejects other
    /// hosts and non-file endpoints, maps `/pluginfile.php` to
    /// `/webservice/pluginfile.php`, and replaces any existing `token`.
    fn file_url(&self, file_url: &str) -> Result<Url> {
        let not_allowed = || Error::NotAllowed(format!("{file_url} is not an iLearning file URL"));
        let mut url = Url::parse(file_url.trim()).map_err(|_| not_allowed())?;
        let root = self.moodle.wwwroot();
        let base = root.path().trim_end_matches('/');
        if url.origin() != root.origin() || !url.path().starts_with(&format!("{base}/")) {
            return Err(not_allowed());
        }
        let local = url.path()[base.len()..].to_owned();
        let local = if local.starts_with("/pluginfile.php/")
            || local.starts_with("/tokenpluginfile.php/")
        {
            let (_, tail) = local.split_once("pluginfile.php").expect("checked prefix");
            format!("/webservice/pluginfile.php{tail}")
        } else if local.starts_with("/webservice/pluginfile.php/") {
            local
        } else {
            return Err(not_allowed());
        };
        url.set_path(&format!("{base}{local}"));
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(k, _)| k != "token")
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        url.query_pairs_mut()
            .clear()
            .extend_pairs(pairs)
            .append_pair("token", &self.token);
        Ok(url)
    }

    /// Uploads files into one new draft area and returns its item id.
    pub async fn upload_draft(&self, files: Vec<UploadFile>) -> Result<i64> {
        if files.is_empty() {
            return Err(Error::NotAllowed("no files to upload".into()));
        }
        let mut itemid = 0;
        for file in files {
            let mime = mime_guess::from_path(&file.filename)
                .first_or_octet_stream()
                .to_string();
            let part = Part::bytes(file.bytes)
                .file_name(file.filename.clone())
                .mime_str(&mime)
                .map_err(Error::decode)?;
            let form = Form::new()
                .text("token", self.token.to_string())
                .text("filearea", "draft")
                .text("itemid", itemid.to_string())
                .part("file_1", part);
            let body: Value = self
                .moodle
                .http
                .post(self.moodle.endpoint("/webservice/upload.php"))
                .multipart(form)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            if let Some(error) = Error::from_response(&body) {
                return Err(error);
            }
            let uploaded = body
                .as_array()
                .and_then(|a| a.first())
                .ok_or_else(|| Error::decode("upload returned no file"))?;
            itemid = uploaded
                .int("itemid")
                .ok_or_else(|| Error::decode("upload returned no itemid"))?;
        }
        Ok(itemid)
    }

    /// Reads a file for an agent: text for documents, the bytes for small
    /// images, otherwise a description to link to.
    pub async fn read_file(&self, request: ReadFileRequest) -> Result<FileContent> {
        let response = self.download_response(&request.file_url).await?;
        let (filename, mime) = describe(&response);
        let size = response.content_length();
        let kind = Kind::of(&filename, &mime);
        let limit = match kind {
            Kind::Image => MAX_IMAGE_BYTES as u64,
            Kind::Binary => 0,
            _ => MAX_EXTRACT_BYTES,
        };
        if kind == Kind::Binary || size.is_some_and(|s| s > limit) {
            return Ok(FileContent::Binary {
                filename,
                mime,
                size: size.unwrap_or_default(),
                file_url: request.file_url,
            });
        }
        let bytes = response.bytes().await?.to_vec();
        if bytes.len() as u64 > limit {
            return Ok(FileContent::Binary {
                filename,
                mime,
                size: bytes.len() as u64,
                file_url: request.file_url,
            });
        }
        if kind == Kind::Image {
            return Ok(FileContent::Image {
                filename,
                mime,
                bytes,
            });
        }
        let size = bytes.len() as u64;
        let extracted = tokio::task::spawn_blocking(move || extract(kind, &bytes))
            .await
            .map_err(|_| Error::Decode("the document could not be read".into()))?;
        let Some(mut text) = extracted else {
            return Ok(FileContent::Binary {
                filename,
                mime,
                size,
                file_url: request.file_url,
            });
        };
        let truncated = text::truncate(&mut text, request.max_chars.max(1) as usize);
        Ok(FileContent::Text {
            filename,
            mime,
            text,
            truncated,
        })
    }
}

/// File name and MIME type of a download response, from
/// `Content-Disposition` / `Content-Type`, else the URL path.
pub fn file_name_and_mime(response: &reqwest::Response) -> (String, String) {
    describe(response)
}

fn describe(response: &reqwest::Response) -> (String, String) {
    let filename = response
        .headers()
        .get(CONTENT_DISPOSITION)
        .and_then(|v| disposition_filename(v.as_bytes()))
        .or_else(|| {
            let segment = response.url().path_segments()?.next_back()?.to_owned();
            let decoded = percent_encoding::percent_decode_str(&segment)
                .decode_utf8_lossy()
                .into_owned();
            Some(decoded).filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "file".to_owned());
    let mime = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or_default().trim().to_owned())
        .filter(|m| {
            !m.is_empty() && m != "application/octet-stream" && m != "application/force-download"
        })
        .unwrap_or_else(|| {
            mime_guess::from_path(&filename)
                .first_or_octet_stream()
                .to_string()
        });
    (filename, mime)
}

/// The file name in a `Content-Disposition` header. Moodle sends raw UTF-8
/// in `filename="…"`; `filename*=UTF-8''…` is preferred when present.
fn disposition_filename(header: &[u8]) -> Option<String> {
    let header = String::from_utf8_lossy(header);
    let mut plain = None;
    for param in header.split(';').map(str::trim) {
        let Some((key, value)) = param.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "filename*" => {
                let encoded = value.trim().trim_matches('"');
                let encoded = encoded.splitn(3, '\'').nth(2).unwrap_or(encoded);
                let name = percent_encoding::percent_decode_str(encoded)
                    .decode_utf8_lossy()
                    .into_owned();
                if !name.is_empty() {
                    return Some(name);
                }
            }
            "filename" => plain = Some(value.trim().trim_matches('"').to_owned()),
            _ => {}
        }
    }
    plain
        .map(|name| {
            name.rsplit(['/', '\\'])
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .filter(|name| !name.is_empty())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Pdf,
    Docx,
    Pptx,
    Xlsx,
    Html,
    Text,
    Image,
    Binary,
}

impl Kind {
    fn of(filename: &str, mime: &str) -> Self {
        let extension = filename
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        match extension.as_str() {
            "pdf" => return Self::Pdf,
            "docx" => return Self::Docx,
            "pptx" => return Self::Pptx,
            "xlsx" => return Self::Xlsx,
            "html" | "htm" => return Self::Html,
            "txt" | "md" | "markdown" | "csv" | "tsv" | "json" | "xml" | "py" | "c" | "h"
            | "cpp" | "java" | "js" | "ts" | "rs" | "ipynb" | "tex" | "log" | "srt" | "vtt" => {
                return Self::Text;
            }
            "png" | "jpg" | "jpeg" | "gif" | "webp" => return Self::Image,
            _ => {}
        }
        match mime {
            "application/pdf" => Self::Pdf,
            "text/html" => Self::Html,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp" => Self::Image,
            m if m.starts_with("text/") => Self::Text,
            _ => Self::Binary,
        }
    }
}

/// Extracts text, or `None` if the document cannot be read.
fn extract(kind: Kind, bytes: &[u8]) -> Option<String> {
    let text = match kind {
        Kind::Pdf => pdf_text(bytes)?,
        Kind::Docx => docx_text(bytes)?,
        Kind::Pptx => pptx_text(bytes)?,
        Kind::Xlsx => xlsx_text(bytes)?,
        Kind::Html => text::html_to_text(&String::from_utf8_lossy(bytes)),
        Kind::Text => String::from_utf8_lossy(bytes).into_owned(),
        Kind::Image | Kind::Binary => return None,
    };
    Some(text)
}

fn pdf_text(bytes: &[u8]) -> Option<String> {
    // pdf-extract panics on some malformed files.
    let pages = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem_by_pages(bytes))
        .ok()?
        .ok()?;
    let mut out = String::new();
    for (index, page) in pages.iter().enumerate() {
        let page = page.trim();
        if page.is_empty() {
            continue;
        }
        if pages.len() > 1 {
            out.push_str(&format!("--- Page {} ---\n", index + 1));
        }
        out.push_str(page);
        out.push_str("\n\n");
    }
    Some(text::tidy(&out))
}

type Zip<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

fn open_zip(bytes: &[u8]) -> Option<Zip<'_>> {
    zip::ZipArchive::new(Cursor::new(bytes)).ok()
}

fn zip_entry(archive: &mut Zip<'_>, name: &str) -> Option<String> {
    let mut entry = archive.by_name(name).ok()?;
    let mut text = String::new();
    entry.read_to_string(&mut text).ok()?;
    Some(text)
}

/// Text runs of an OOXML part: `<w:t>`/`<a:t>` contents, with paragraph
/// (`<w:p>`, `<a:p>`) and break / tab elements turned into whitespace.
fn ooxml_text(xml: &str) -> String {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if e.local_name().as_ref() == "t" {
                    in_text = true;
                }
            }
            Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                "br" | "cr" => out.push('\n'),
                "tab" => out.push('\t'),
                _ => {}
            },
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "p" => out.push('\n'),
                "tc" => out.push('\t'),
                _ => {}
            },
            Ok(Event::Text(t)) if in_text => {
                out.push_str(&t.xml10_content());
            }
            Ok(Event::GeneralRef(r)) if in_text => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    out.push(c);
                } else {
                    out.push_str(match r.as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    // Layout tabs and spaces pile up in documents; keep single separators.
    let lines: Vec<String> = out
        .lines()
        .map(|line| {
            let mut collapsed = String::with_capacity(line.len());
            let mut previous = ' ';
            for c in line.trim().chars() {
                let is_gap = c == '\t' || c == ' ' || c == '\u{3000}';
                if is_gap && (previous == '\t' || (c == ' ' && previous == ' ')) {
                    continue;
                }
                if c == '\t' && previous == ' ' {
                    collapsed.pop();
                }
                collapsed.push(c);
                previous = c;
            }
            collapsed
        })
        .collect();
    text::tidy(&lines.join("\n"))
}

fn docx_text(bytes: &[u8]) -> Option<String> {
    let mut archive = open_zip(bytes)?;
    Some(ooxml_text(&zip_entry(&mut archive, "word/document.xml")?))
}

fn pptx_text(bytes: &[u8]) -> Option<String> {
    let mut archive = open_zip(bytes)?;
    let slide_number = |name: &str| -> Option<u32> {
        name.strip_prefix("ppt/slides/slide")?
            .strip_suffix(".xml")?
            .parse()
            .ok()
    };
    let mut slides: Vec<(u32, String)> = archive
        .file_names()
        .filter_map(|name| name.ok())
        .filter_map(|name| Some((slide_number(&name)?, name.into_owned())))
        .collect();
    slides.sort();
    let mut out = String::new();
    for (number, name) in slides {
        let text = ooxml_text(&zip_entry(&mut archive, &name)?);
        out.push_str(&format!("--- Slide {number} ---\n"));
        if !text.is_empty() {
            out.push_str(&text);
            out.push('\n');
        }
        let notes = format!("ppt/notesSlides/notesSlide{number}.xml");
        if let Some(notes) = zip_entry(&mut archive, &notes) {
            let notes = ooxml_text(&notes);
            // Notes pages repeat the slide number as their only text.
            if !notes.is_empty() && notes != number.to_string() {
                out.push_str(&format!("Notes: {notes}\n"));
            }
        }
        out.push('\n');
    }
    Some(out.trim_end().to_owned())
}

fn xlsx_text(bytes: &[u8]) -> Option<String> {
    use quick_xml::events::Event;
    let mut archive = open_zip(bytes)?;
    let shared: Vec<String> = zip_entry(&mut archive, "xl/sharedStrings.xml")
        .map(|xml| {
            let mut reader = quick_xml::Reader::from_str(&xml);
            let mut strings = Vec::new();
            let mut current = String::new();
            let mut in_text = false;
            loop {
                match reader.read_event() {
                    Ok(Event::Start(e)) if e.local_name().as_ref() == "t" => in_text = true,
                    Ok(Event::End(e)) => match e.local_name().as_ref() {
                        "t" => in_text = false,
                        "si" => strings.push(std::mem::take(&mut current)),
                        _ => {}
                    },
                    Ok(Event::Text(t)) if in_text => {
                        current.push_str(&t.xml10_content());
                    }
                    Ok(Event::GeneralRef(r)) if in_text => {
                        if let Ok(Some(c)) = r.resolve_char_ref() {
                            current.push(c);
                        } else if let Some(c) = predefined_entity(r.as_ref()) {
                            current.push(c);
                        }
                    }
                    Ok(Event::Eof) | Err(_) => break,
                    _ => {}
                }
            }
            strings
        })
        .unwrap_or_default();
    let sheet_number = |name: &str| -> Option<u32> {
        name.strip_prefix("xl/worksheets/sheet")?
            .strip_suffix(".xml")?
            .parse()
            .ok()
    };
    let mut sheets: Vec<(u32, String)> = archive
        .file_names()
        .filter_map(|name| name.ok())
        .filter_map(|name| Some((sheet_number(&name)?, name.into_owned())))
        .collect();
    sheets.sort();
    let mut out = String::new();
    for (number, name) in sheets {
        let xml = zip_entry(&mut archive, &name)?;
        out.push_str(&format!("--- Sheet {number} ---\n"));
        let mut reader = quick_xml::Reader::from_str(&xml);
        let mut row: Vec<String> = Vec::new();
        let mut cell_type = String::new();
        let mut value = String::new();
        let mut in_value = false;
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => match e.local_name().as_ref() {
                    "c" => {
                        cell_type = e
                            .try_get_attribute("t")
                            .ok()
                            .flatten()
                            .map(|a| a.value.into_owned())
                            .unwrap_or_default();
                        value.clear();
                    }
                    "v" | "t" => in_value = true,
                    _ => {}
                },
                Ok(Event::End(e)) => match e.local_name().as_ref() {
                    "v" | "t" => in_value = false,
                    "c" => {
                        let cell = if cell_type == "s" {
                            value
                                .trim()
                                .parse::<usize>()
                                .ok()
                                .and_then(|i| shared.get(i).cloned())
                                .unwrap_or_default()
                        } else {
                            value.clone()
                        };
                        row.push(cell);
                    }
                    "row" => {
                        while row.last().is_some_and(String::is_empty) {
                            row.pop();
                        }
                        if !row.is_empty() {
                            out.push_str(&row.join("\t"));
                            out.push('\n');
                        }
                        row.clear();
                    }
                    _ => {}
                },
                Ok(Event::Text(t)) if in_value => {
                    value.push_str(&t.xml10_content());
                }
                Ok(Event::GeneralRef(r)) if in_value => {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        value.push(c);
                    } else if let Some(c) = predefined_entity(r.as_ref()) {
                        value.push(c);
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
        out.push('\n');
    }
    Some(out.trim_end().to_owned())
}

fn predefined_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::Moodle;

    fn account() -> Account {
        Moodle::new(Url::parse("https://ilearning.cycu.edu.tw").unwrap()).account("TOKEN".into())
    }

    fn zip_of(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn builds_download_urls() {
        let a = account();
        assert_eq!(
            a.file_url("https://ilearning.cycu.edu.tw/webservice/pluginfile.php/1/mod_resource/content/0/a%20b.pdf?forcedownload=1")
                .unwrap()
                .as_str(),
            "https://ilearning.cycu.edu.tw/webservice/pluginfile.php/1/mod_resource/content/0/a%20b.pdf?forcedownload=1&token=TOKEN"
        );
        assert_eq!(
            a.file_url("https://ilearning.cycu.edu.tw/pluginfile.php/2/mod_label/intro/x.png")
                .unwrap()
                .as_str(),
            "https://ilearning.cycu.edu.tw/webservice/pluginfile.php/2/mod_label/intro/x.png?token=TOKEN"
        );
        assert_eq!(
            a.file_url("https://ilearning.cycu.edu.tw/webservice/pluginfile.php/3/f.txt?token=old")
                .unwrap()
                .as_str(),
            "https://ilearning.cycu.edu.tw/webservice/pluginfile.php/3/f.txt?token=TOKEN"
        );
        for bad in [
            "https://evil.example.com/webservice/pluginfile.php/1/a.pdf",
            "http://ilearning.cycu.edu.tw/webservice/pluginfile.php/1/a.pdf",
            "https://ilearning.cycu.edu.tw/mod/page/view.php?id=1",
            "https://ilearning.cycu.edu.tw.evil.com/webservice/pluginfile.php/1/a.pdf",
            "not a url",
        ] {
            assert!(
                matches!(a.file_url(bad), Err(Error::NotAllowed(_))),
                "{bad}"
            );
        }
    }

    #[tokio::test]
    async fn transport_errors_hide_the_token() {
        let account =
            Moodle::new(Url::parse("http://127.0.0.1:9").unwrap()).account("SECRETTOKEN".into());
        let error = account
            .download("http://127.0.0.1:9/webservice/pluginfile.php/1/a.pdf")
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Http(_)));
        assert!(!error.to_string().contains("SECRETTOKEN"), "{error}");
        assert!(!format!("{error:?}").contains("SECRETTOKEN"));
    }

    #[test]
    fn reads_content_disposition() {
        assert_eq!(
            disposition_filename("inline; filename=\"115-1理工及電資學院DM-A群.pdf\"".as_bytes())
                .as_deref(),
            Some("115-1理工及電資學院DM-A群.pdf")
        );
        assert_eq!(
            disposition_filename(b"attachment; filename=\"a.pdf\"; filename*=UTF-8''%E4%B8%AD.pdf")
                .as_deref(),
            Some("中.pdf")
        );
        assert_eq!(disposition_filename(b"inline"), None);
    }

    #[test]
    fn classifies_files() {
        assert_eq!(Kind::of("a.PDF", "application/octet-stream"), Kind::Pdf);
        assert_eq!(Kind::of("a.pptx", ""), Kind::Pptx);
        assert_eq!(
            Kind::of("a.ppt", "application/vnd.ms-powerpoint"),
            Kind::Binary
        );
        assert_eq!(Kind::of("index.html", "text/html"), Kind::Html);
        assert_eq!(Kind::of("photo", "image/png"), Kind::Image);
        assert_eq!(Kind::of("lecture.mp4", "video/mp4"), Kind::Binary);
    }

    #[test]
    fn extracts_docx() {
        let docx = zip_of(&[(
            "word/document.xml",
            r#"<?xml version="1.0"?><w:document xmlns:w="w"><w:body>
<w:p><w:r><w:t>Hello</w:t></w:r><w:r><w:t xml:space="preserve"> world &amp; co</w:t></w:r></w:p>
<w:p><w:r><w:t>第二段</w:t><w:br/><w:t>line</w:t></w:r></w:p></w:body></w:document>"#,
        )]);
        assert_eq!(
            extract(Kind::Docx, &docx).unwrap(),
            "Hello world & co\n第二段\nline"
        );
    }

    #[test]
    fn extracts_pptx_in_slide_order() {
        let slide = |text: &str| {
            format!(
                r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
            )
        };
        let pptx = zip_of(&[
            ("ppt/slides/slide10.xml", &slide("Ten")),
            ("ppt/slides/slide2.xml", &slide("Two")),
            ("ppt/slides/slide1.xml", &slide("One")),
            ("ppt/notesSlides/notesSlide2.xml", &slide("Speaker note")),
            ("ppt/slides/_rels/slide1.xml.rels", "<x/>"),
        ]);
        assert_eq!(
            extract(Kind::Pptx, &pptx).unwrap(),
            "--- Slide 1 ---\nOne\n\n--- Slide 2 ---\nTwo\nNotes: Speaker note\n\n--- Slide 10 ---\nTen"
        );
    }

    #[test]
    fn extracts_xlsx() {
        let xlsx = zip_of(&[
            (
                "xl/sharedStrings.xml",
                r#"<sst><si><t>Name</t></si><si><t>Score</t></si><si><r><t>A</t></r><r><t>&amp;B</t></r></si></sst>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row><row r="2"><c t="s"><v>2</v></c><c><v>95</v></c></row><row r="3"><c t="inlineStr"><is><t>inline</t></is></c></row></sheetData></worksheet>"#,
            ),
        ]);
        assert_eq!(
            extract(Kind::Xlsx, &xlsx).unwrap(),
            "--- Sheet 1 ---\nName\tScore\nA&B\t95\ninline"
        );
    }

    #[test]
    fn unreadable_documents_yield_none() {
        assert_eq!(extract(Kind::Docx, b"not a zip"), None);
        assert_eq!(extract(Kind::Pdf, b"not a pdf"), None);
    }
}
