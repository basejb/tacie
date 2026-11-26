import XCTest

@MainActor
final class MediaPauseTests: XCTestCase {
    override func tearDown() { UserDefaults.standard.removeObject(forKey: MediaPause.key) }

    func testPausesOnlyRunningPlayingPlayersAndResumesOnlyThem() {
        let m = MediaPause()
        var sent: [(String, String)] = []
        m.running = { $0 != "com.spotify.client" }  // Spotify not open: never sent anything
        m.run = { id, script in sent.append((id, script)); return script.hasPrefix("if player state is playing") ? "paused" : nil }
        m.pauseAll()
        XCTAssertEqual(m.paused, ["com.apple.Music"])
        XCTAssertFalse(sent.contains { $0.0 == "com.spotify.client" })
        sent = []
        m.resumeAll()
        XCTAssertEqual(sent.map(\.0), ["com.apple.Music"])
        XCTAssertTrue(sent[0].1.contains("play"))
        XCTAssertEqual(m.paused, [])
    }

    func testNothingPlayingMeansNothingToResume() {
        let m = MediaPause()
        var sent = 0
        m.running = { _ in true }
        m.run = { _, _ in sent += 1; return nil }  // not playing
        m.pauseAll()
        XCTAssertEqual(m.paused, [])
        sent = 0
        m.resumeAll()
        XCTAssertEqual(sent, 0)
    }

    func testOffDoesNothing() {
        UserDefaults.standard.set(false, forKey: MediaPause.key)
        let m = MediaPause()
        var sent = 0
        m.running = { _ in true }
        m.run = { _, _ in sent += 1; return "paused" }
        m.pauseAll()
        XCTAssertEqual(sent, 0)
    }
}
