//! Pulls text out of file bytes: PDF, Word (docx), PowerPoint (pptx), Excel (xlsx) (issue #31).
//!
//! - Reads only with Rust crates, without launching outside programs (`pdf_oxide`, `office_oxide`,
//!   `tests/no_spawn.rs`).
//! - Office comes back as Markdown: headings are `#`, tables are `|` lines, slides and sheets are `##` headings. PDF
//!   joins the text page by page.
//! - Each file is read only up to `MAX_BYTES`. Reading happens on a separate thread, and the file is skipped once
//!   `TIMEOUT` passes. Threads cannot be stopped, so once `MAX_STUCK` unfinished threads remain, it stops reading
//!   (so it does not use up the CPU). A panic is also a failure of that one file.
//! - Files with no text (such as scanned PDFs) are `NoText`.
//!
//! Content is never put in errors or logs.

use std::io::Cursor;
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// 읽는 파일 하나의 최대 크기예요(20MB).
pub const MAX_BYTES: usize = 20 * 1024 * 1024;
/// 파일 하나를 읽는 시간의 상한이에요.
pub const TIMEOUT: Duration = Duration::from_secs(60);
/// 돌려주는 글자의 최대(문자 수)예요. 그 뒤는 버려요.
pub const MAX_CHARS: usize = 1_000_000;
/// 끝나지 않고 남은 읽기 스레드가 이만큼이면 더 읽지 않아요.
pub const MAX_STUCK: usize = 2;

/// 읽을 수 있는 종류예요.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Pdf,
    Docx,
    Pptx,
    Xlsx,
}

impl FileKind {
    /// MIME 형식의 종류예요(Drive·Dropbox·OneDrive가 주는 값).
    pub fn of_mime(mime: &str) -> Option<Self> {
        match mime {
            "application/pdf" => Some(Self::Pdf),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => Some(Self::Docx),
            "application/vnd.openxmlformats-officedocument.presentationml.presentation" => Some(Self::Pptx),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => Some(Self::Xlsx),
            _ => None,
        }
    }

    /// 확장자의 종류예요(대소문자는 가리지 않아요).
    pub fn of_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "pdf" => Some(Self::Pdf),
            "docx" => Some(Self::Docx),
            "pptx" => Some(Self::Pptx),
            "xlsx" => Some(Self::Xlsx),
            _ => None,
        }
    }

    /// 돌려주는 글이 Markdown인지예요(제목·표로 조각을 나눌 수 있어요).
    pub fn markdown(self) -> bool {
        self != Self::Pdf
    }
}

/// 글자를 뽑지 못한 이유예요. 로그에는 이 이름만 남겨요.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fail {
    /// `MAX_BYTES`보다 커요.
    TooLarge,
    /// 읽었는데 글자가 없어요(스캔한 PDF, 빈 문서).
    NoText,
    /// 파일이 깨졌거나 읽지 못하는 형식이에요(panic 포함).
    Broken,
    /// `TIMEOUT` 안에 끝나지 않았거나, 끝나지 않은 읽기가 많아 읽지 않았어요.
    Slow,
}

impl Fail {
    pub fn reason(self) -> &'static str {
        match self {
            Fail::TooLarge => "tooLarge",
            Fail::NoText => "noText",
            Fail::Broken => "unreadable",
            Fail::Slow => "slow",
        }
    }
}

static STUCK: AtomicUsize = AtomicUsize::new(0);
static LIMITS: Once = Once::new();

/// 파일 하나의 글자예요.
pub fn text(bytes: Vec<u8>, kind: FileKind) -> Result<String, Fail> {
    text_within(bytes, kind, TIMEOUT)
}

/// `text`와 같고 시간 상한만 달라요(테스트).
pub fn text_within(bytes: Vec<u8>, kind: FileKind, timeout: Duration) -> Result<String, Fail> {
    if bytes.len() > MAX_BYTES {
        return Err(Fail::TooLarge);
    }
    if STUCK.load(Ordering::SeqCst) >= MAX_STUCK {
        return Err(Fail::Slow);
    }
    LIMITS.call_once(|| {
        // 20MB 파일이 풀려서 커질 수 있는 만큼만 받아요(zip 폭탄)
        office_oxide::limits::set_max_package_bytes(256 << 20);
        office_oxide::limits::set_max_part_bytes(128 << 20);
    });
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("extract".into())
        .spawn(move || {
            let r = std::panic::catch_unwind(move || read(bytes, kind)).unwrap_or(Err(Fail::Broken));
            // 받는 쪽이 시간이 지나 떠났으면 이 스레드가 남은 것으로 세었어요
            if tx.send(r).is_err() {
                STUCK.fetch_sub(1, Ordering::SeqCst);
            }
        });
    if spawned.is_err() {
        return Err(Fail::Slow);
    }
    match rx.recv_timeout(timeout) {
        Ok(r) => r.map(|t| cap(&t)),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            STUCK.fetch_add(1, Ordering::SeqCst);
            // 그 사이 끝났으면 받은 것을 써요(세지 않아요)
            match rx.try_recv() {
                Ok(r) => {
                    STUCK.fetch_sub(1, Ordering::SeqCst);
                    r.map(|t| cap(&t))
                }
                Err(_) => Err(Fail::Slow),
            }
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(Fail::Broken),
    }
}

fn read(bytes: Vec<u8>, kind: FileKind) -> Result<String, Fail> {
    let out = match kind {
        FileKind::Pdf => {
            let d = pdf_oxide::PdfDocument::from_bytes(bytes).map_err(|_| Fail::Broken)?;
            let n = d.page_count().map_err(|_| Fail::Broken)?;
            let mut pages = Vec::with_capacity(n);
            for i in 0..n {
                // 쪽 하나가 깨져도 나머지 쪽은 써요
                if let Ok(t) = d.extract_text(i) {
                    pages.push(t);
                }
            }
            pages.join("\n\n")
        }
        FileKind::Docx | FileKind::Pptx | FileKind::Xlsx => {
            let format = match kind {
                FileKind::Docx => office_oxide::DocumentFormat::Docx,
                FileKind::Pptx => office_oxide::DocumentFormat::Pptx,
                _ => office_oxide::DocumentFormat::Xlsx,
            };
            office_oxide::Document::from_reader(Cursor::new(bytes), format)
                .map_err(|_| Fail::Broken)?
                .to_markdown()
        }
    };
    let out = out.replace("\r\n", "\n").replace('\r', "\n");
    if out.trim().is_empty() {
        Err(Fail::NoText)
    } else {
        Ok(out)
    }
}

fn cap(t: &str) -> String {
    match t.char_indices().nth(MAX_CHARS) {
        Some((at, _)) => t[..at].to_string(),
        None => t.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_mime_and_extension() {
        assert_eq!(FileKind::of_mime("application/pdf"), Some(FileKind::Pdf));
        assert_eq!(
            FileKind::of_mime("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
            Some(FileKind::Pptx)
        );
        assert_eq!(FileKind::of_mime("application/msword"), None);
        assert_eq!(FileKind::of_extension("XLSX"), Some(FileKind::Xlsx));
        assert_eq!(FileKind::of_extension("hwp"), None);
    }

    #[test]
    fn too_large_is_refused_before_reading() {
        assert_eq!(text(vec![0; MAX_BYTES + 1], FileKind::Pdf), Err(Fail::TooLarge));
    }

    #[test]
    fn broken_files_fail_alone() {
        assert_eq!(text(b"not a pdf".to_vec(), FileKind::Pdf), Err(Fail::Broken));
        assert_eq!(text(b"not a zip".to_vec(), FileKind::Docx), Err(Fail::Broken));
    }

    #[test]
    fn caps_long_text() {
        let long = "가".repeat(MAX_CHARS + 10);
        assert_eq!(cap(&long).chars().count(), MAX_CHARS);
    }
}
