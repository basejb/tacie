import XCTest
import SwiftUI
@testable import Tacie

/// The month's limit kept with a meeting (NotesLimit) and its block on the meeting page.
@MainActor
final class LimitNotesTests: XCTestCase {
    private func meeting(limit: NotesLimit?, recorded: Bool) -> SavedMeeting {
        var m = SavedMeeting(id: "m1", mode: "inperson", start: Date(timeIntervalSince1970: 1_800_000_000))
        m.end = m.start.addingTimeInterval(42 * 60)
        m.notes = .failed
        m.notesProblem = "이번 달 AI 사용량을 다 썼어요"
        m.notesLimit = limit
        if recorded { m.recording = .kept }
        return m
    }

    func testOlderFilesReadWithoutLimit() throws {
        let m = meeting(limit: .stopped, recorded: false)
        var json = try JSONSerialization.jsonObject(with: JSONEncoder().encode(m)) as! [String: Any]
        XCTAssertEqual(json["notesLimit"] as? String, "stopped")
        json["notesLimit"] = nil
        let old = try JSONDecoder().decode(SavedMeeting.self, from: JSONSerialization.data(withJSONObject: json))
        XCTAssertNil(old.notesLimit)
    }

    func testWords() {
        XCTAssertEqual(meeting(limit: .stopped, recorded: true).limitMinutes, 42)
        XCTAssertTrue(NotesLimitText.title(.stopped, ready: false).contains("회의를 끝냈어요"))
        XCTAssertTrue(NotesLimitText.title(.notes, ready: false).contains("요약을 쓰지 못했어요"))
        XCTAssertFalse(NotesLimitText.detail(.notes, ready: false, recorded: false, until: "14일").contains("녹음"))
        XCTAssertTrue(NotesLimitText.detail(.stopped, ready: false, recorded: true, until: "14일").contains("14일 뒤에"))
        // the server's month is UTC: 17 Nov → 1 Dec 00:00 UTC
        let now = ISO8601DateFormatter().date(from: "2026-11-17T05:00:00Z")!
        XCTAssertEqual(UsageMonth.nextMonth(now), ISO8601DateFormatter().date(from: "2026-12-01T00:00:00Z")!)
    }

    /// Draws the block in its three states to PNG for a look. Runs only when TACIE_LIMIT_RENDER_DIR names a folder
    /// (pass it as TEST_RUNNER_TACIE_LIMIT_RENDER_DIR); nothing opens on screen.
    func testRender() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_LIMIT_RENDER_DIR"] else { throw XCTSkip("no render dir") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let cases: [(String, NotesLimit, Double?, Bool)] = [("stopped", .stopped, 0, true), ("notes", .notes, 0, false), ("ready", .notes, 1, true)]
        for (name, limit, left, recorded) in cases {
            var m = SavedMeeting(id: "m1", mode: "inperson", start: Date(timeIntervalSince1970: 1_800_000_000))
            m.end = m.start.addingTimeInterval(42 * 60)
            m.notes = .failed
            m.notesLimit = limit
            if recorded { m.recording = .kept }
            let view = VStack(alignment: .leading, spacing: 12) {
                Text("요약").font(.system(size: 17, weight: .semibold))
                LimitNotesBlock(meeting: m, limit: limit, left: left, write: {}, usage: {})
            }
            .frame(width: 640).padding(32).background(Color.white)
            let r = ImageRenderer(content: view)
            r.scale = 2
            guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
                  let data = rep.representation(using: NSBitmapImageRep.FileType.png, properties: [:]) else { return XCTFail("no image") }
            try data.write(to: out.appendingPathComponent("limit-\(name).png"))
        }
    }
}
