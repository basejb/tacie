import SwiftUI
import XCTest

final class MeetingResumeTests: XCTestCase {
    private var cal: Calendar = {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "Asia/Seoul")!
        c.locale = Locale(identifier: "ko_KR")
        return c
    }()

    private func at(_ h: Int, _ m: Int = 0) -> Date {
        cal.date(from: DateComponents(year: 2026, month: 11, day: 14, hour: h, minute: m))!
    }

    private func ended(_ start: Date, _ end: Date) -> SavedMeeting {
        var m = SavedMeeting(mode: "remote", start: start)
        m.end = end
        return m
    }

    func testResumableForThirtyMinutes() {
        let m = ended(at(14), at(14, 47))
        XCTAssertTrue(m.resumable(now: at(14, 50)))
        XCTAssertTrue(m.resumable(now: at(15, 16)))
        XCTAssertFalse(m.resumable(now: at(15, 17)))
        XCTAssertFalse(SavedMeeting(mode: "remote", start: at(14)).resumable(now: at(14, 5)))  // going on
    }

    func testReopenKeepsTheGapAndLengthLeavesItOut() {
        var m = ended(at(14), at(14, 40))
        m.reopen(at: at(14, 50))
        XCTAssertNil(m.end)
        XCTAssertEqual(m.resumes, [MeetingResume(stopped: at(14, 40), resumed: at(14, 50))])
        m.end = at(15, 0)
        XCTAssertEqual(m.listenedSeconds(), 50 * 60)
        XCTAssertEqual(m.listenedMinutes(now: at(16)), 50)
    }

    func testNextEpochGoesPastEveryKey() {
        var m = SavedMeeting(mode: "remote", start: at(14))
        XCTAssertEqual(m.nextEpoch, 0)
        m.transcript = [TranscriptLine(key: "e0:s1u1", at: at(14), speaker: nil, text: "a", question: false),
                        TranscriptLine(key: "e2:s1u4", at: at(14), speaker: nil, text: "b", question: false)]
        XCTAssertEqual(m.nextEpoch, 3)
    }

    func testHeaderSaysHowLongAgoWhileResumable() {
        let m = ended(at(14), at(14, 47))
        XCTAssertTrue(m.pageHeader(calendar: cal, now: at(14, 50)).hasSuffix("47분 · 3분 전에 끝났어요"))
        XCTAssertTrue(m.pageHeader(calendar: cal, now: at(16)).hasSuffix("47분"))
        // copies never carry it: it is true only for a while
        XCTAssertTrue(m.header(calendar: cal, now: at(14, 50)).hasSuffix("47분"))
    }

    @MainActor
    func testRenderResumeCapsule() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_RESUME_RENDER_DIR"] else { throw XCTSkip("render only") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        for dark in [false, true] {
            AppearanceMode.isDark = dark
            let v = HStack(spacing: 10) { ResumeCapsule {} }.padding(30).background(dark ? Color(white: 0.11) : Theme.paper)
                .environment(\.colorScheme, dark ? .dark : .light)
            let r = ImageRenderer(content: v)
            r.scale = 2
            if let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
               let data = rep.representation(using: .png, properties: [:]) {
                try data.write(to: out.appendingPathComponent("capsule\(dark ? "-dark" : "").png"))
            }
            AppearanceMode.isDark = false
        }
    }
}
