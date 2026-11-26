//! The token for gh mode (spec 6-3). In development builds it borrows the user's `gh` login token once with
//! `gh auth token`. token and none modes never call this. The token is only returned, never printed, and never put
//! in failure text.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

use crate::error::{Kind, SourceError};
use crate::py;

/// `gh auth token`을 기다리는 시간이에요(gh가 키체인을 읽느라 느릴 수 있어요).
pub const GH_TOKEN_TIMEOUT: Duration = Duration::from_secs(5);
/// gh 토큰을 얻지 못했을 때의 글이에요(key).
pub const NOT_LOGGED_IN: &str = "gh is not logged in";

/// gh 모드의 토큰을 주는 곳이에요. 실패는 key `gh is not logged in`이에요.
pub trait GhToken: Send + Sync {
    fn token(&self) -> Result<String, SourceError>;
}

pub(crate) fn not_logged_in() -> SourceError {
    SourceError::service(Kind::Key, NOT_LOGGED_IN)
}

/// gh가 토큰을 주지 못한 오류인지예요(key `gh is not logged in`). 이 글을 만드는 곳은 이 모듈과 `GitHub`의 토큰
/// 빌리기뿐이에요.
pub fn is_not_logged_in(e: &SourceError) -> bool {
    matches!(e, SourceError::Service { kind: Kind::Key, message } if message == NOT_LOGGED_IN)
}

/// 진짜 `gh auth token`이에요: 이 프로세스의 환경을 물려받고(`GH_TOKEN`·`GITHUB_TOKEN`도 gh가 따라요), stdin은
/// 비우고, `GH_TOKEN_TIMEOUT`까지 기다려 stdout의 앞뒤 공백을 떼요. gh가 없거나, 0이 아닌 종료, 빈 출력, 시간 초과는
/// 모두 key예요.
#[derive(Debug, Clone)]
pub struct SystemGhToken {
    program: String,
    args: Vec<String>,
    timeout: Duration,
}

impl Default for SystemGhToken {
    fn default() -> Self {
        SystemGhToken::new()
    }
}

impl SystemGhToken {
    pub fn new() -> Self {
        SystemGhToken {
            program: "gh".to_string(),
            args: vec!["auth".to_string(), "token".to_string()],
            timeout: GH_TOKEN_TIMEOUT,
        }
    }

    /// 다른 프로그램을 같은 규칙으로 띄워요(테스트용: `sh -c …`로 느린 gh, stdin 읽기 따위를 흉내 내요).
    pub fn with_program(program: &str, args: &[&str], timeout: Duration) -> Self {
        SystemGhToken {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            timeout,
        }
    }
}

impl GhToken for SystemGhToken {
    fn token(&self) -> Result<String, SourceError> {
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| not_logged_in())?;
        let mut pipe = child.stdout.take().ok_or_else(not_logged_in)?;
        let deadline = Instant::now() + self.timeout;
        // stdout을 따로 읽어서 출력이 커도 기다리기가 막히지 않아요
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
        let status = match child.wait_timeout(self.timeout) {
            Ok(Some(status)) => status,
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(not_logged_in());
            }
        };
        // gh가 끝나도 손자 프로세스가 파이프를 쥐고 있을 수 있어서, 읽기도 같은 시각까지만 기다려요
        let left = deadline.saturating_duration_since(Instant::now());
        let out = rx.recv_timeout(left).map_err(|_| not_logged_in())?;
        if !status.success() {
            return Err(not_logged_in());
        }
        let text = String::from_utf8(out).map_err(|_| not_logged_in())?;
        let token = py::strip(&text);
        if token.is_empty() {
            return Err(not_logged_in());
        }
        Ok(token.to_string())
    }
}
