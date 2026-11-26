//! State, raw and cache JSON files: there are no half-written files, and one broken file does not keep crashing.
//! Swapping in folders lives here too.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;
use tacie_index::pyjson;

/// `read_json`이 읽은 것이에요.
#[derive(Debug, Clone, PartialEq)]
pub enum Stored {
    /// 파일이 없어요.
    Missing,
    /// 있는데 JSON이 아니에요(깨진 UTF-8 포함). stderr에 한 줄 남겨요.
    Unreadable,
    Value(Value),
}

/// 파일 이름 뒤에 붙인 옆자리예요(`x.json` → `x.json+tmp`).
fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// JSON을 `<이름>+tmp`에 먼저 쓰고(fsync) 바꿔 넣어요: 쓰다 죽어도 옛 파일은 온전해요. 부모 폴더를 만들어요.
/// 어떤 실패든 임시 파일을 지우고 Err예요. 바이트는 Python `json.dump(v, ensure_ascii=False, indent=indent)` 모양
/// 그대로예요(파일 형식은 바꾸지 않아요).
pub fn write_json(path: &Path, v: &Value, indent: Option<usize>) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let tmp = beside(path, "+tmp");
    let written = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(pyjson::to_string(v, indent).as_bytes())?;
        // 그냥 fsync예요. std의 sync_all은 macOS에서 F_FULLFSYNC라 작은 파일 하나에 수 ms가
        // 들어요(GitHub 원본은 항목마다 파일 하나예요).
        rustix::fs::fsync(&f)?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// 없으면 `Missing`, JSON이 아니면 `Unreadable`(stderr `store: unreadable {이름}`)이에요. 그 밖의 IO 오류(폴더,
/// 권한)는 Err예요.
pub fn read_json(path: &Path) -> std::io::Result<Stored> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Stored::Missing),
        Err(e) => return Err(e),
    };
    let parsed = String::from_utf8(bytes)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    Ok(match parsed {
        Some(v) => Stored::Value(v),
        None => {
            eprintln!("store: unreadable {}", name_of(path));
            Stored::Unreadable
        }
    })
}

/// 폴더 `new`를 `live` 자리에 두고 거기 있던 것을 지워요. 옛것은 잠시 `<live>+old`에 둬요.
pub fn swap_in(new: &Path, live: &Path) -> std::io::Result<()> {
    let old = beside(live, "+old");
    let _ = fs::remove_dir_all(&old);
    if live.exists() {
        fs::rename(live, &old)?;
    }
    fs::rename(new, live)?;
    let _ = fs::remove_dir_all(&old);
    Ok(())
}
