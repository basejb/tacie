import XCTest
@testable import Tacie

final class LogReportTests: XCTestCase {
    private func tempDir() throws -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    func testFilteredDropsTextLines() {
        let input = "a\n[heard] s1u1 p=0.91 질문\nrefresh: embed\n[ask] context [..]\n[answer] context []\n"
            + "2026-10-23T09:00:00Z debug: ask 세션 어디서\n2026-10-23T09:00:01Z debug: search 결제\n2026-10-23T09:00:02Z debug: main\nengine ready"
        XCTAssertEqual(LogReport.filtered(input), "a\nrefresh: embed\n2026-10-23T09:00:02Z debug: main\nengine ready")
        XCTAssertEqual(LogReport.filtered("a\r\n[heard] s1u1 말\r\nb"), "a\r\nb")
    }

    func testStageCopiesOnlyLogs() throws {
        let logs = try tempDir(), out = try tempDir()
        let fm = FileManager.default
        try "x".write(to: logs.appendingPathComponent("app.log"), atomically: true, encoding: .utf8)
        try "[heard] 말\nok".write(to: logs.appendingPathComponent("engine.log"), atomically: true, encoding: .utf8)
        try "[ask] 물음\nold".write(to: logs.appendingPathComponent("engine.log.1"), atomically: true, encoding: .utf8)
        try fm.createDirectory(at: logs.appendingPathComponent("meetings"), withIntermediateDirectories: true)
        try "{}".write(to: logs.appendingPathComponent("meetings/a.json"), atomically: true, encoding: .utf8)
        try fm.createDirectory(at: logs.appendingPathComponent("reports"), withIntermediateDirectories: true)
        try "z".write(to: logs.appendingPathComponent("reports/old.zip"), atomically: true, encoding: .utf8)
        let staged = try LogReport.stage(logs: logs, into: out)
        XCTAssertEqual(staged.map(\.lastPathComponent), ["app.log", "engine.log", "engine.log.1"])
        XCTAssertEqual(try String(contentsOf: out.appendingPathComponent("engine.log"), encoding: .utf8), "ok")
        XCTAssertEqual(try String(contentsOf: out.appendingPathComponent("engine.log.1"), encoding: .utf8), "old")
        XCTAssertEqual(try fm.contentsOfDirectory(atPath: out.path).sorted(), ["app.log", "engine.log", "engine.log.1"])
    }

    func testStageSkipsMissingFiles() throws {
        let logs = try tempDir(), out = try tempDir()
        try "x".write(to: logs.appendingPathComponent("app.log"), atomically: true, encoding: .utf8)
        XCTAssertEqual(try LogReport.stage(logs: logs, into: out).map(\.lastPathComponent), ["app.log"])
    }
}
