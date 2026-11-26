//! One Unix-socket connection per tool call: send one JSON line, read one JSON line, close.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

/// The app caps its reply at 100 KB; anything much longer is not from the app.
const MAX_REPLY: u64 = 256 * 1024;

#[derive(Debug)]
pub enum CallError {
    /// No socket, wrong owner, or nothing answers: the app is off or access is off.
    Unreachable,
    Timeout,
    /// The app answered something that is not one JSON line.
    Bad,
}

/// The folder and the socket must be real (not symlinks) and owned by the same user as `$HOME`.
fn trusted(path: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME").and_then(|h| fs::metadata(h).ok()) else {
        return false;
    };
    let Some(dir) = path.parent() else {
        return false;
    };
    let own = |p: &Path| {
        fs::symlink_metadata(p)
            .map(|m| !m.file_type().is_symlink() && m.uid() == home.uid())
            .unwrap_or(false)
    };
    own(dir) && own(path)
}

pub fn call(path: &Path, req: &Value, timeout: Duration) -> Result<Value, CallError> {
    if !trusted(path) {
        return Err(CallError::Unreachable);
    }
    let mut s = UnixStream::connect(path).map_err(|_| CallError::Unreachable)?;
    let _ = s.set_write_timeout(Some(timeout));
    let _ = s.set_read_timeout(Some(timeout));
    let mut line = req.to_string();
    line.push('\n');
    s.write_all(line.as_bytes()).map_err(|e| classify(&e))?;
    let mut reply = String::new();
    BufReader::new(s.take(MAX_REPLY))
        .read_line(&mut reply)
        .map_err(|e| classify(&e))?;
    if !reply.ends_with('\n') {
        return Err(CallError::Bad); // closed early, or too long
    }
    serde_json::from_str(&reply).map_err(|_| CallError::Bad)
}

fn classify(e: &std::io::Error) -> CallError {
    match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => CallError::Timeout,
        _ => CallError::Unreachable,
    }
}
