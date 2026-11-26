//! Address rules. The engine (`settings` in `tacie-engine`) and the Slack client use the same check.

/// 이 주소가 이 Mac 안(loopback)인지예요: http·https이고, 사용자 정보가 없고, 호스트가 `127.0.0.1`, `localhost`,
/// `[::1]` 중 하나예요(대소문자 무관). `127.0.0.1.evil.example`, `localhost@evil.example`, `127.0.0.2`는 아니에요.
pub fn is_loopback(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else {
        return false;
    };
    matches!(u.scheme(), "http" | "https")
        && u.username().is_empty()
        && u.password().is_none()
        // 파서가 IP는 정규형으로(`[0:0::1]` → `[::1]`), 도메인은 소문자로 바꿔 둬요.
        && u.host_str().is_some_and(|h| {
            ["127.0.0.1", "localhost", "[::1]"]
                .iter()
                .any(|ok| h.eq_ignore_ascii_case(ok))
        })
}

/// reqwest 오류의 글이에요: URL을 빼고(채널 id, 커서, 저장소 이름, 리다이렉트의 임시 토큰이 들어 있을 수 있어요)
/// 원인을 이어요. 슬랙과 GitHub 클라이언트가 같이 써요.
pub fn net_error(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut out = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}
