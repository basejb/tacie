import XCTest
import SwiftUI
@testable import Tacie

final class ConnectPromptTests: XCTestCase {
    private func meeting(ended: Bool = true, notes: SavedMeeting.Notes = .done, unanswered: Int = 0, answered: Int = 0) -> SavedMeeting {
        var m = SavedMeeting(mode: "remote", start: Date())
        if ended { m.end = Date() }
        m.notes = notes
        let q = { (i: Int, open: Bool) in
            SavedQuestion(id: "\(open)-\(i)", question: "q", asker: nil, at: Date(), answer: nil, cards: [], cited: [], unanswered: open)
        }
        m.questions = (0..<unanswered).map { q($0, true) } + (0..<answered).map { q($0, false) }
        return m
    }

    private func show(done: Bool = false, meetings: Int = 3, unanswered: Int = 11, teamSource: Bool? = false,
                      recordOnly: Bool = false, page: SavedMeeting? = nil, running: Bool = false, covered: Bool = false) -> Bool {
        ConnectPrompt.shouldShow(done: done, counts: .init(meetings: meetings, unanswered: unanswered), teamSource: teamSource,
                                 recordOnly: recordOnly, page: page ?? meeting(), running: running, covered: covered)
    }

    func testCountsFinishedMeetingsAndTheirUnansweredQuestions() {
        let ms = [meeting(unanswered: 4, answered: 1), meeting(unanswered: 2), meeting(ended: false, unanswered: 5)]
        XCTAssertEqual(ConnectPrompt.counts(ms), .init(meetings: 2, unanswered: 6))
    }

    func testComesUpAfterThreeMeetingsWithUnansweredQuestionsAndNothingConnected() {
        XCTAssertTrue(show())
        XCTAssertFalse(show(meetings: 2))
        XCTAssertFalse(show(unanswered: 0))
    }

    func testNotWhenSomethingIsConnectedOrNotYetKnown() {
        XCTAssertFalse(show(teamSource: true))
        XCTAssertFalse(show(teamSource: nil))
    }

    func testNotForNotesOnlyPeopleOrOnceAnswered() {
        XCTAssertFalse(show(recordOnly: true))
        XCTAssertFalse(show(done: true))
    }

    func testOnlyOverAFinishedNotesPageOutsideMeetingsAndModals() {
        XCTAssertFalse(show(page: meeting(ended: false)))
        XCTAssertFalse(show(page: meeting(notes: .waiting)))
        XCTAssertFalse(ConnectPrompt.shouldShow(done: false, counts: .init(meetings: 3, unanswered: 1), teamSource: false,
                                                recordOnly: false, page: nil, running: false, covered: false))
        XCTAssertFalse(show(running: true))
        XCTAssertFalse(show(covered: true))
    }

    func testAnsweredIsRemembered() {
        let d = UserDefaults(suiteName: "ConnectPromptTests")!
        d.removePersistentDomain(forName: "ConnectPromptTests")
        XCTAssertFalse(ConnectPrompt.load(d))
        ConnectPrompt.save(true, d)
        XCTAssertTrue(ConnectPrompt.load(d))
    }
}

/// Draws the connect card to PNG for a look. Runs only when TACIE_CONNECT_RENDER_DIR names a folder (pass it as
/// TEST_RUNNER_TACIE_CONNECT_RENDER_DIR); nothing opens on screen.
@MainActor
final class ConnectPromptCardRenderTests: XCTestCase {
    private func png<V: View>(_ view: V, _ name: String, scheme: ColorScheme, to out: URL) {
        let framed = view.padding(28).background(Theme.paper).environment(\.colorScheme, scheme)
        let r = ImageRenderer(content: framed)
        r.scale = 2
        guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
        try? data.write(to: out.appendingPathComponent("\(name).png"))
    }

    func testRenderStates() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_CONNECT_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let p = PointerTracker()
        let counts = ConnectPrompt.Counts(meetings: 3, unanswered: 11)
        for scheme in [ColorScheme.light, .dark] {
            let tag = scheme == .light ? "light" : "dark"
            png(ConnectPromptCard(pointer: p, counts: counts) { _ in }, "1-ask-\(tag)", scheme: scheme, to: out)
            png(ConnectPromptCard(pointer: p, counts: counts, kept: true) { _ in }, "2-kept-\(tag)", scheme: scheme, to: out)
        }
    }
}
