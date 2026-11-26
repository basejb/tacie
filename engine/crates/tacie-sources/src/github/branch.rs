//! Branch name check. Without a process, it accepts and rejects the same names as
//! `git check-ref-format "refs/heads/<이름>"` (git `check_refname_format`, no flags). The expected-value table is
//! `tests/fixtures/branch_names.json`.

/// git이 받는 브랜치 이름이고 옵션으로 읽히지 않아요. 비었거나 `-`로 시작하면 false예요(지금 앞 검사 그대로).
/// 그 밖에는 `refs/heads/<이름>`에 git의 규칙을 그대로 적용해요: `/`로 나눈 조각은 비거나 `.`으로 시작하거나
/// `.lock`으로 끝나면 안 되고, 전체에 `..`, `@{`, 제어 문자(0x00–0x1f, 0x7f), 공백, `~ ^ : ? * [ \`가 있거나
/// `.`으로 끝나면 안 돼요. (`@` 하나는 `refs/heads/@`라서 git도 받아요.)
pub fn valid_branch(name: &str) -> bool {
    if name.is_empty() || name.starts_with('-') {
        return false;
    }
    if name.ends_with('.') || name.contains("..") || name.contains("@{") {
        return false;
    }
    if name.chars().any(|c| {
        c.is_ascii_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    }) {
        return false;
    }
    name.split('/')
        .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}
