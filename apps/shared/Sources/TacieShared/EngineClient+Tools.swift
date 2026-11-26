import Foundation

extension EngineClient {
    /// The engine's `/tools/search`, `/tools/meetings` and `/tools/ledger` (docs/38 4절) for the local MCP bridge: the
    /// body goes as it is and the reply comes back as raw JSON. A non-200 reply throws `EngineError.http` with its
    /// `error_kind`.
    func tool(_ path: String, body: [String: Any], timeout: TimeInterval = 30) async throws -> Data {
        var req = request(path)
        req.timeoutInterval = timeout
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, resp) = try await session.data(for: req)
        try check(resp, data)
        return data
    }
}
