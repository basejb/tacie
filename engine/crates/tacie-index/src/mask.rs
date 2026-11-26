//! Which chunks may be found: only those outside the masked paths that are open to everyone or to this person.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use crate::Chunk;

/// 색인에서 뺄 경로예요.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mask {
    /// `docs/superpowers` 아래 전부
    Superpowers,
    /// `docs/superpowers/plans` 아래만
    Plans,
    /// 가리지 않아요
    None,
}

impl Mask {
    fn prefix(self) -> Option<&'static str> {
        match self {
            Mask::Superpowers => Some("docs/superpowers"),
            Mask::Plans => Some("docs/superpowers/plans"),
            Mask::None => None,
        }
    }
}

impl FromStr for Mask {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "superpowers" => Ok(Mask::Superpowers),
            "plans" => Ok(Mask::Plans),
            "none" => Ok(Mask::None),
            other => anyhow::bail!("unknown mask {other:?} (superpowers, plans or none)"),
        }
    }
}

/// `FromStr`이 받는 이름 그대로예요(`/health`의 `"mask"`).
impl fmt::Display for Mask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mask::Superpowers => "superpowers",
            Mask::Plans => "plans",
            Mask::None => "none",
        })
    }
}

/// 조각마다 찾아도 되는지예요. acl 키가 없거나 `"*"`를 담으면 모두에게, 아니면 viewer와 겹칠 때만 열려요.
/// `"acl": null`은 읽을 때 빈 목록이 되어 아무도 못 봐요(chunk.rs).
pub fn allowed(chunks: &[Chunk], mask: Mask, viewer: &HashSet<String>) -> Vec<bool> {
    let prefix = mask.prefix();
    chunks
        .iter()
        .map(|c| {
            let outside = prefix.is_none_or(|p| !c.path.starts_with(p));
            let seen = match &c.acl {
                None => true,
                Some(acl) => acl.iter().any(|p| p == "*" || viewer.contains(p)),
            };
            outside && seen
        })
        .collect()
}
