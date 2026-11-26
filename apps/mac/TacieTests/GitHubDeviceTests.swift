import XCTest
@testable import Tacie

final class GitHubDeviceTests: XCTestCase {
    private enum Reply { case body(String), fail(Error) }

    private final class Box: @unchecked Sendable {
        var replies: [Reply]
        var requests: [URLRequest] = []
        var slept: [Int] = []
        init(_ r: [Reply]) { replies = r }
    }

    private func device(_ box: Box) -> GitHubDevice {
        var d = GitHubDevice()
        d.send = { req in
            box.requests.append(req)
            guard !box.replies.isEmpty else { throw URLError(.badServerResponse) }
            switch box.replies.removeFirst() {
            case .body(let s): return Data(s.utf8)
            case .fail(let e): throw e
            }
        }
        d.sleep = { box.slept.append($0) }
        return d
    }

    private let code = DeviceCode(deviceCode: "d", userCode: "WDJB-MJHT",
                                  verificationURI: URL(string: "https://github.com/login/device")!,
                                  expiresIn: 900, interval: 5)

    private func bodyString(_ r: URLRequest) -> String { String(decoding: r.httpBody ?? Data(), as: UTF8.self) }

    func testStartParsesDeviceCode() async throws {
        let box = Box([.body(#"{"device_code":"d","user_code":"WDJB-MJHT","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#)])
        let got = try await device(box).start()
        XCTAssertEqual(got, code)
        let req = try XCTUnwrap(box.requests.first)
        XCTAssertEqual(req.httpMethod, "POST")
        XCTAssertEqual(req.url?.absoluteString, "https://github.com/login/device/code")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Accept"), "application/json")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Content-Type"), "application/x-www-form-urlencoded")
        let body = bodyString(req)
        XCTAssertTrue(body.contains("client_id="))
        XCTAssertTrue(body.contains("scope=repo%20read:org"), body)
    }

    func testStartMapsErrors() async {
        for (reply, want) in [(Reply.body(#"{"error":"device_flow_disabled"}"#), DeviceFlowError.unavailable),
                              (.body("<html>nope</html>"), .unavailable),
                              (.fail(URLError(.notConnectedToInternet)), .network)] {
            do { _ = try await device(Box([reply])).start(); XCTFail("should throw") }
            catch { XCTAssertEqual(error as? DeviceFlowError, want) }
        }
    }

    func testPollWaitsThenReturnsToken() async throws {
        let box = Box([.body(#"{"error":"authorization_pending"}"#),
                       .body(#"{"error":"slow_down"}"#),
                       .body(#"{"access_token":"gho_t","token_type":"bearer","refresh_token":"r"}"#)])
        let token = try await device(box).token(for: code)
        XCTAssertEqual(token, "gho_t")
        XCTAssertEqual(box.slept, [5, 5, 10])
        let body = bodyString(box.requests[0])
        XCTAssertTrue(body.contains("device_code=d"))
        XCTAssertTrue(body.contains("grant_type=urn:ietf:params:oauth:grant-type:device_code"), body)
    }

    func testPollStopsOnExpiredAndDenied() async {
        let cases: [(Reply, Error)] = [
            (.body(#"{"error":"expired_token"}"#), DeviceFlowError.expired),
            (.body(#"{"error":"access_denied"}"#), DeviceFlowError.denied),
            (.body(#"{"error":"incorrect_device_code"}"#), DeviceFlowError.unavailable),
            (.body("not json"), DeviceFlowError.unavailable),
            (.fail(URLError(.timedOut)), DeviceFlowError.network),
            (.fail(URLError(.cancelled)), CancellationError()),
        ]
        for (reply, want) in cases {
            do { _ = try await device(Box([reply])).token(for: code); XCTFail("should throw") }
            catch let e as DeviceFlowError { XCTAssertEqual(e, want as? DeviceFlowError) }
            catch { XCTAssertTrue(want is CancellationError, "\(error)") ; XCTAssertTrue(error is CancellationError) }
        }
    }

    func testPollGivesUpWhenCodeLifetimeIsSpent() async {
        let short = DeviceCode(deviceCode: "d", userCode: "U", verificationURI: code.verificationURI, expiresIn: 10, interval: 5)
        let box = Box(Array(repeating: .body(#"{"error":"authorization_pending"}"#), count: 10))
        do { _ = try await device(box).token(for: short); XCTFail("should throw") }
        catch { XCTAssertEqual(error as? DeviceFlowError, .expired) }
        XCTAssertEqual(box.slept, [5, 5])
    }

    func testIntervalIsAtLeastOneSecond() async throws {
        let box = Box([.body(#"{"device_code":"d","user_code":"U","verification_uri":"https://github.com/login/device","expires_in":900,"interval":0}"#)])
        let got = try await device(box).start()
        XCTAssertEqual(got.interval, 1)
        let poll = Box([.body(#"{"error":"slow_down","interval":0}"#), .body(#"{"access_token":"gho_t"}"#)])
        _ = try await device(poll).token(for: got)
        XCTAssertEqual(poll.slept, [1, 1])
    }
}
