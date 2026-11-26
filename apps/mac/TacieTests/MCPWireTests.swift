import XCTest

final class MCPWireTests: XCTestCase {
    private func failure(_ d: Data) -> MCPError? {
        if case .failure(let e) = MCPWire.decode(d) { return e }
        return nil
    }

    private func line(_ s: String) -> Data { Data(s.utf8) }

    func testDecodesARequest() throws {
        let r = try MCPWire.decode(line(#"{"v":1,"tool":"list_meetings","args":{"limit":5},"client":"claude_desktop"}"#)).get()
        XCTAssertEqual(r.tool, .listMeetings)
        XCTAssertEqual(r.client, .claudeDesktop)
        XCTAssertEqual(r.args["limit"] as? Int, 5)
    }

    func testMissingArgsAndUnknownClientAreAllowed() throws {
        let r = try MCPWire.decode(line(#"{"v":1,"tool":"search","client":"someone"}\#n"#)).get()
        XCTAssertEqual(r.tool, .search)
        XCTAssertEqual(r.client, .other)
        XCTAssertTrue(r.args.isEmpty)
    }

    func testRejectsBadRequests() {
        let bad = [
            #"{"v":2,"tool":"search","args":{},"client":"other"}"#,
            #"{"tool":"search","args":{},"client":"other"}"#,
            #"{"v":1,"tool":"delete_all","args":{},"client":"other"}"#,
            #"{"v":1,"tool":"search","args":[1],"client":"other"}"#,
            #"{"v":1,"tool":"search","args":"x","client":"other"}"#,
            "not json", "[1]", "",
        ]
        for b in bad {
            XCTAssertEqual(failure(line(b)), .badRequest, b)
        }
        let big = #"{"v":1,"tool":"search","args":{"q":""# + String(repeating: "a", count: MCPWire.maxRequest) + #""},"client":"other"}"#
        XCTAssertEqual(failure(line(big)), .badRequest)
    }

    func testOkAndFailEndWithNewline() throws {
        let ok = MCPWire.ok(["cards": [["title": "a"]]])
        XCTAssertEqual(ok.last, 0x0A)
        let o = try XCTUnwrap(JSONSerialization.jsonObject(with: ok) as? [String: Any])
        XCTAssertEqual(o["v"] as? Int, 1)
        XCTAssertEqual(o["ok"] as? Bool, true)
        XCTAssertNotNil(o["result"])

        let fail = MCPWire.fail(.notReady)
        XCTAssertEqual(fail.last, 0x0A)
        let f = try XCTUnwrap(JSONSerialization.jsonObject(with: fail) as? [String: Any])
        XCTAssertEqual(f["ok"] as? Bool, false)
        XCTAssertEqual(f["error"] as? String, "not_ready")
    }

    func testOversizedResultBecomesBadRequest() {
        let out = MCPWire.ok(["text": String(repeating: "a", count: MCPWire.maxResponse)])
        XCTAssertEqual(out, MCPWire.fail(.badRequest))
        XCTAssertLessThanOrEqual(out.count, MCPWire.maxResponse)
    }

    func testErrorCodes() {
        XCTAssertEqual(MCPError.badRequest.rawValue, "bad_request")
        XCTAssertEqual(MCPError.switching.rawValue, "switching")
    }
}
