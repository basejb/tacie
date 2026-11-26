//! Sorts failed calls into kinds the app can tell the user about.
//!
//! key (key missing or rejected), credit (no balance), network (no connection), busy (rate limit), down (service
//! failure or error response), model (the search model is not downloaded on this Mac yet; happens only inside the
//! engine).
//! Responses carry it as `error_kind` next to `error`.

use std::fmt;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Key,
    Credit,
    Network,
    Busy,
    Down,
    Model,
}

impl Kind {
    /// 종류의 이름이에요: 응답의 `error_kind`(serde)와 로그 줄이 같은 글자를 써요.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Key => "key",
            Kind::Credit => "credit",
            Kind::Network => "network",
            Kind::Busy => "busy",
            Kind::Down => "down",
            Kind::Model => "model",
        }
    }
}

/// 출처 일(파일, GitHub, 슬랙 같은 바깥 서비스)의 실패예요.
#[derive(Debug)]
pub enum SourceError {
    /// 읽은 JSON이 기대한 모양이 아니에요.
    Json(String),
    /// 종류를 아는 실패예요.
    Service {
        kind: Kind,
        message: String,
    },
    Other(String),
}

impl SourceError {
    pub fn service(kind: Kind, message: impl Into<String>) -> Self {
        SourceError::Service {
            kind,
            message: message.into(),
        }
    }

    /// 상태의 `error_kind`예요. 종류를 아는 실패 밖은 down이에요.
    pub fn kind(&self) -> Kind {
        match self {
            SourceError::Service { kind, .. } => *kind,
            _ => Kind::Down,
        }
    }

    /// 상태의 `error`예요: 메시지 그대로예요. 200자로 자르는 건 부르는 쪽이에요.
    pub fn detail(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceError::Json(m)
            | SourceError::Service { message: m, .. }
            | SourceError::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for SourceError {}

impl From<std::io::Error> for SourceError {
    fn from(e: std::io::Error) -> Self {
        SourceError::Other(e.to_string())
    }
}

impl From<anyhow::Error> for SourceError {
    fn from(e: anyhow::Error) -> Self {
        SourceError::Other(format!("{e:#}"))
    }
}
