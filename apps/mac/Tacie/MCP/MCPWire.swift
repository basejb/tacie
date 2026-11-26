import Foundation

/// Why a request was not answered; the raw value is the wire code (mirrors engine/crates/tacie-mcp).
enum MCPError: String, Error, Equatable {
    case off, notReady = "not_ready", switching, limit, busy, network, key, down, model, badRequest = "bad_request"
}

enum MCPTool: String {
    case search, listMeetings = "list_meetings", listLedger = "list_ledger", readTranscript = "read_transcript"
}

enum MCPClient: String {
    case claudeDesktop = "claude_desktop", cursor, claudeCode = "claude_code", codex, other
}

struct MCPRequest {
    let tool: MCPTool
    let args: [String: Any]
    let client: MCPClient
}

/// The one-line JSON the `tacie-mcp` bridge and the app speak over the socket (docs/38 4절).
/// Request: `{"v":1,"tool":…,"args":{…},"client":…}`; reply: `{"v":1,"ok":true,"result":…}` or `{"v":1,"ok":false,"error":…}`.
enum MCPWire {
    static let version = 1
    static let maxRequest = 65_536
    static let maxResponse = 102_400

    static func decode(_ line: Data) -> Result<MCPRequest, MCPError> {
        guard line.count <= maxRequest,
              let obj = try? JSONSerialization.jsonObject(with: line) as? [String: Any],
              (obj["v"] as? Int) == version,
              let name = obj["tool"] as? String, let tool = MCPTool(rawValue: name)
        else { return .failure(.badRequest) }
        var args: [String: Any] = [:]
        if let raw = obj["args"] {
            guard let a = raw as? [String: Any] else { return .failure(.badRequest) }
            args = a
        }
        let client = (obj["client"] as? String).flatMap(MCPClient.init(rawValue:)) ?? .other
        return .success(MCPRequest(tool: tool, args: args, client: client))
    }

    /// A reply line. A result too big for one line is refused rather than cut.
    static func ok(_ result: Any) -> Data {
        let body: [String: Any] = ["v": version, "ok": true, "result": result]
        guard JSONSerialization.isValidJSONObject(body),
              var data = try? JSONSerialization.data(withJSONObject: body, options: [.withoutEscapingSlashes]) else {
            return fail(.badRequest)
        }
        data.append(0x0A)
        return data.count > maxResponse ? fail(.badRequest) : data
    }

    static func fail(_ e: MCPError) -> Data {
        let body: [String: Any] = ["v": version, "ok": false, "error": e.rawValue]
        var data = (try? JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])) ?? Data()
        data.append(0x0A)
        return data
    }
}
