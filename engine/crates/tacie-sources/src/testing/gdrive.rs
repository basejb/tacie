//! A fake Google Drive (`GDriveApi`): holds files, shared drives and the change log. Fetch tests (`gdrive::fetch`)
//! and engine tests share it.
//!
//! - Change token n starts at the nth entry of the change log. `upsert`/`delete` record to the log; `vanish` does not
//!   (only reconcile notices it).
//! - Export text: Docs Markdown has two headings and one `data:` image, Slides is one line with a BOM and CRLF, Sheets
//!   is CRLF CSV.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::{Value, json};

use crate::error::{Kind, SourceError};
use crate::gdrive::api::{GDriveApi, Got};

pub const DOC: &str = "application/vnd.google-apps.document";
pub const SLIDES: &str = "application/vnd.google-apps.presentation";
pub const SHEET: &str = "application/vnd.google-apps.spreadsheet";

pub struct FakeDrive {
    /// `file(…, days_ago, …)`의 기준 시각이에요.
    pub now: DateTime<Utc>,
    pub files: Mutex<Vec<Value>>,
    pub drives: Mutex<Vec<Value>>,
    pub log: Mutex<Vec<Value>>,
    /// export 답을 바꿔요: 파일 id → (상태, 이유).
    pub fails: Mutex<HashMap<String, (u16, String)>>,
    /// Docs Markdown이 10MB를 넘는 파일 id예요.
    pub huge: Mutex<HashSet<String>>,
    pub reject_tokens: Mutex<bool>,
    /// export한 것이에요: `"<id> <mime>"`.
    pub exports: Mutex<Vec<String>>,
    /// export 글을 바꿔요: 파일 id → 글(형식과 상관없이 그대로예요).
    pub texts: Mutex<HashMap<String, String>>,
    /// 원본(`alt=media`)으로 내주는 바이트예요: 파일 id → 바이트. 없으면 404예요.
    pub media: Mutex<HashMap<String, Vec<u8>>>,
    /// 모든 호출이 401이에요.
    pub refuse: AtomicBool,
    /// 모든 호출이 한도예요.
    pub busy: AtomicBool,
}

fn iso(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn body(v: Value) -> Result<Got, SourceError> {
    Ok(Got::Body(v.to_string().into_bytes()))
}

fn q(query: &[(&str, String)], k: &str) -> Option<String> {
    query.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone())
}

impl FakeDrive {
    pub fn at(now: DateTime<Utc>) -> FakeDrive {
        FakeDrive {
            now,
            files: Mutex::default(),
            drives: Mutex::default(),
            log: Mutex::default(),
            fails: Mutex::default(),
            huge: Mutex::default(),
            reject_tokens: Mutex::default(),
            exports: Mutex::default(),
            texts: Mutex::default(),
            media: Mutex::default(),
            refuse: AtomicBool::new(false),
            busy: AtomicBool::new(false),
        }
    }

    /// 지금 시각 기준의 가짜예요(엔진 테스트).
    pub fn new() -> FakeDrive {
        Self::at(Utc::now())
    }

    pub fn t(&self, days_ago: i64) -> String {
        iso(self.now - Duration::days(days_ago))
    }

    /// 파일 하나를 더하거나 바꿔요. 내 드라이브 파일은 내가 주인이에요.
    pub fn file(&self, id: &str, mime: &str, days_ago: i64, drive: Option<&str>) {
        let mut f = json!({"id": id, "name": format!("name {id}"), "mimeType": mime,
            "modifiedTime": self.t(days_ago), "version": "1", "webViewLink": format!("https://docs.google.com/{id}"),
            "ownedByMe": drive.is_none(), "trashed": false, "capabilities": {"canDownload": true},
            "lastModifyingUser": {"displayName": "Kim"}});
        if let Some(d) = drive {
            f["driveId"] = json!(d);
        }
        self.upsert(f);
    }

    pub fn upsert(&self, f: Value) {
        let mut files = self.files.lock().unwrap();
        files.retain(|x| x["id"] != f["id"]);
        files.push(f.clone());
        self.log
            .lock()
            .unwrap()
            .push(json!({"changeType": "file", "fileId": f["id"], "removed": false, "file": f}));
    }

    pub fn edit(&self, id: &str, days_ago: i64) {
        let mut f = self.get(id).unwrap();
        f["modifiedTime"] = json!(self.t(days_ago));
        self.upsert(f);
    }

    pub fn get(&self, id: &str) -> Option<Value> {
        self.files
            .lock()
            .unwrap()
            .iter()
            .find(|f| f["id"] == id)
            .cloned()
    }

    pub fn delete(&self, id: &str) {
        self.files.lock().unwrap().retain(|f| f["id"] != id);
        self.log
            .lock()
            .unwrap()
            .push(json!({"changeType": "file", "fileId": id, "removed": true}));
    }

    /// 이 글을 가진 Docs 파일을 더해요(엔진 테스트).
    pub fn doc(&self, id: &str, name: &str, text: &str, drive: Option<&str>) {
        self.texts.lock().unwrap().insert(id.into(), text.into());
        self.file(id, DOC, 1, drive);
        let mut f = self.get(id).unwrap();
        f["name"] = json!(name);
        self.upsert(f);
    }

    /// 지우되 변경 기록에는 남기지 않아요(재조정만 알아요).
    pub fn vanish(&self, id: &str) {
        self.files.lock().unwrap().retain(|f| f["id"] != id);
    }
}

impl Default for FakeDrive {
    fn default() -> Self {
        Self::new()
    }
}

impl GDriveApi for FakeDrive {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Got, SourceError> {
        if self.refuse.load(Ordering::SeqCst) {
            return Err(SourceError::service(Kind::Key, "gdrive: refused"));
        }
        if self.busy.load(Ordering::SeqCst) {
            return Err(SourceError::service(Kind::Busy, "gdrive: rate limited"));
        }
        let drive = q(query, "driveId");
        match path {
            "drives" => body(json!({"drives": *self.drives.lock().unwrap()})),
            "changes/startPageToken" => {
                body(json!({"startPageToken": self.log.lock().unwrap().len().to_string()}))
            }
            "files" => {
                let files: Vec<Value> = self
                    .files
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|f| match &drive {
                        Some(d) => f["driveId"].as_str() == Some(d.as_str()),
                        None => f["driveId"].is_null(),
                    })
                    .cloned()
                    .collect();
                body(json!({"files": files}))
            }
            "changes" => {
                if *self.reject_tokens.lock().unwrap() {
                    return Ok(Got::Fail {
                        status: 410,
                        reason: String::new(),
                    });
                }
                let from: usize = q(query, "pageToken")
                    .and_then(|t| t.parse().ok())
                    .unwrap_or(0);
                let log = self.log.lock().unwrap();
                let changes: Vec<Value> = log[from.min(log.len())..]
                    .iter()
                    .filter(|c| {
                        let d = c["file"]["driveId"].as_str();
                        match &drive {
                            Some(want) => c["removed"] == true || d == Some(want.as_str()),
                            None => d.is_none(),
                        }
                    })
                    .cloned()
                    .collect();
                body(json!({"changes": changes, "newStartPageToken": log.len().to_string()}))
            }
            p if p.ends_with("/export") => {
                let id = p.trim_start_matches("files/").trim_end_matches("/export");
                let mime = q(query, "mimeType").unwrap_or_default();
                self.exports.lock().unwrap().push(format!("{id} {mime}"));
                if let Some((status, reason)) = self.fails.lock().unwrap().get(id) {
                    return Ok(Got::Fail {
                        status: *status,
                        reason: reason.clone(),
                    });
                }
                if mime == "text/markdown" && self.huge.lock().unwrap().contains(id) {
                    return Ok(Got::Fail {
                        status: 403,
                        reason: "exportSizeLimitExceeded".into(),
                    });
                }
                if let Some(t) = self.texts.lock().unwrap().get(id) {
                    return Ok(Got::Body(t.clone().into_bytes()));
                }
                let text = match mime.as_str() {
                    "text/markdown" => format!(
                        "# Title {id}\n\nbody ![][image1]\n\n## Part\n\nmore\n\n[image1]: <data:image/png;base64,AAAA>\n"
                    ),
                    "text/csv" => "a,b\r\n1,2\r\n".to_string(),
                    _ => format!("\u{feff}plain {id}\r\n"),
                };
                Ok(Got::Body(text.into_bytes()))
            }
            p if p.starts_with("files/") && q(query, "alt").as_deref() == Some("media") => {
                let id = p.trim_start_matches("files/");
                self.exports.lock().unwrap().push(format!("{id} media"));
                if let Some((status, reason)) = self.fails.lock().unwrap().get(id) {
                    return Ok(Got::Fail {
                        status: *status,
                        reason: reason.clone(),
                    });
                }
                match self.media.lock().unwrap().get(id) {
                    Some(b) => Ok(Got::Body(b.clone())),
                    None => Ok(Got::Fail {
                        status: 404,
                        reason: String::new(),
                    }),
                }
            }
            _ => Ok(Got::Fail {
                status: 404,
                reason: String::new(),
            }),
        }
    }
}
