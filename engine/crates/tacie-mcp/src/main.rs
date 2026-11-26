//! `tacie-mcp`: a tiny MCP stdio server bundled in the Tacie app (docs/38 section 2-1). AI apps launch it; each tool
//! call opens one connection to the app's Unix socket. It starts no process and never starts the app or the engine.

mod protocol;
mod socket;
mod text;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut socket_path = None;
    while let Some(a) = args.next() {
        if a == "--socket" {
            socket_path = args.next().map(PathBuf::from);
        }
    }
    let path = socket_path.unwrap_or_else(default_socket);
    let mut server = protocol::Server::new(&path);
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = server.handle(&line)
            && writeln!(out, "{reply}").and_then(|_| out.flush()).is_err()
        {
            break;
        }
    }
}

/// `~/Library/Application Support/<folder>/mcp/mcp.sock`, the folder of the app this binary sits in (`support_folder`).
fn default_socket() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let id = std::env::current_exe()
        .ok()
        .and_then(|e| bundle_id(&e))
        .unwrap_or_default();
    home.join("Library/Application Support")
        .join(support_folder(&id))
        .join("mcp/mcp.sock")
}

/// The app's own folder by its bundle id, as the app names it (Build.names in apps/shared): the official app, its
/// development build, the open edition and its development build each keep their own.
fn support_folder(bundle_id: &str) -> &'static str {
    match bundle_id {
        "ai.tacie.app.dev" => "Tacie Dev",
        "ai.tacie.open" => "Tacie Open",
        "ai.tacie.open.dev" => "Tacie Open Dev",
        _ => "Tacie",
    }
}

/// The executable sits in `<App>.app/Contents/MacOS`; the plist is `Contents/Info.plist`.
fn bundle_id(exe: &Path) -> Option<String> {
    let plist = exe.parent()?.parent()?.join("Info.plist");
    let bytes = std::fs::read(plist).ok()?;
    let s = String::from_utf8_lossy(&bytes);
    let rest = &s[s.find("<key>CFBundleIdentifier</key>")?..];
    let (a, b) = (rest.find("<string>")?, rest.find("</string>")?);
    (a + 8 <= b).then(|| rest[a + 8..b].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::support_folder;

    #[test]
    fn support_folder_maps_four_ids() {
        assert_eq!(support_folder("ai.tacie.app"), "Tacie");
        assert_eq!(support_folder("ai.tacie.app.dev"), "Tacie Dev");
        assert_eq!(support_folder("ai.tacie.open"), "Tacie Open");
        assert_eq!(support_folder("ai.tacie.open.dev"), "Tacie Open Dev");
        assert_eq!(support_folder("x.y"), "Tacie");
    }
}
