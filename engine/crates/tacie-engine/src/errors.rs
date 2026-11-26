//! Sorts failed calls to outside services (STT, Jev, answer model) into kinds the app can tell the user about.
//!
//! key (missing or rejected key), credit (no balance), network (no connection), busy (rate limited), down (service
//! failure or error response).
//! Responses carry it as `error_kind` next to `error`.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

/// 종류는 하나로 정의해요: `tacie-sources`에 두고 여기서 다시 내보내요.
pub use tacie_sources::Kind;

#[derive(Debug)]
pub struct ServiceError {
    pub kind: Kind,
    pub message: String,
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ServiceError {}

/// 응답 상태로 종류를 정해요. key와 credit은 서비스("stt", "answer", "ledger", "jev")별 마지막 거절로 남겨서
/// 연결 화면이 키 파일 옆에 보여 줘요.
pub fn from_status(
    service: &str,
    status: u16,
    text: &str,
    last: &Mutex<HashMap<String, Kind>>,
) -> ServiceError {
    let kind = match status {
        401 | 403 => Kind::Key,
        402 => Kind::Credit,
        429 => Kind::Busy,
        _ => Kind::Down,
    };
    if matches!(kind, Kind::Key | Kind::Credit) {
        lock(last).insert(service.to_string(), kind);
    }
    ServiceError {
        kind,
        message: format!("{service} {status}: {text}"),
    }
}

/// 그 서비스가 다시 200으로 답했어요: 거절을 잊어요(새 키나 충전이 바로 보여요).
pub fn clear(service: &str, last: &Mutex<HashMap<String, Kind>>) {
    lock(last).remove(service);
}

/// 다른 스레드가 잡은 채 죽었어도 표는 그대로 써요(거절 기록 하나라 깨질 게 없어요).
fn lock(last: &Mutex<HashMap<String, Kind>>) -> std::sync::MutexGuard<'_, HashMap<String, Kind>> {
    last.lock().unwrap_or_else(|e| e.into_inner())
}
