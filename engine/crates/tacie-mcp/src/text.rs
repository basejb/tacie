//! What the AI app reads when a call fails (docs/38 section 6). English; the AI translates for the user.

const NOT_RUNNING: &str = "Tacie is not running, or its MCP is turned off. Open Tacie and turn it on in 설정 → 연결 → MCP (Settings → Connections → MCP).";
const LIMIT: &str = "This month's Tacie usage limit is reached. It resets next month.";
const STARTING: &str = "Tacie is starting up. Try again in a moment.";
const MODEL: &str = "Tacie is still downloading its search model. Try again later.";
const BUSY: &str = "Tacie is busy (a meeting may be running). Try again shortly.";
const SERVICE: &str = "Tacie could not reach its search service. Try again later.";
const BAD_REQUEST: &str = "Tacie could not read this request.";

/// The text for an app error code (`off`, `limit`, ...) or `unreachable` when no socket answers.
pub fn for_error(code: &str) -> &'static str {
    match code {
        "limit" => LIMIT,
        "not_ready" | "switching" => STARTING,
        "model" => MODEL,
        "busy" => BUSY,
        "network" | "down" | "key" => SERVICE,
        "bad_request" => BAD_REQUEST,
        // "off", "unreachable" and anything unknown
        _ => NOT_RUNNING,
    }
}
