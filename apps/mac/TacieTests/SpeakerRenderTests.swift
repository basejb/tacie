import XCTest
import SwiftUI

/// Draws the transcript (a name with its ⌄ as under the pointer) and the naming toasts to PNG for a look. Runs only when
/// TACIE_SPEAKER_RENDER_DIR names a folder (pass it as TEST_RUNNER_TACIE_SPEAKER_RENDER_DIR); nothing opens on screen.
@MainActor
final class SpeakerRenderTests: XCTestCase {
    private let base = Date(timeIntervalSince1970: 1_792_000_000)

    private func meeting() -> SavedMeeting {
        let said: [(String, String?, String)] = [
            ("화자 2", nil, "지난주 배포 뒤로 로그인 에러가 좀 늘었어요."),
            ("화자 1", nil, "얼마나요?"),
            ("화자 2", "앤디", "하루에 서른 건쯤이고, 거의 다 토큰 갱신 쪽이에요."),
            ("화자 2", nil, "그거 제가 지난번에 본 이슈랑 비슷한 것 같은데요. 갱신 토큰이 바뀌는 타이밍 문제였어요."),
            ("화자 2", nil, "그럼 재시도 횟수를 늘리면 줄어들까요?"),
            ("화자 1", nil, "일단 이번 주는 로그만 더 보죠."),
            ("화자 2", nil, "제가 대시보드에 갱신 실패만 따로 걸러 볼게요."),
        ]
        var m = SavedMeeting(mode: "inperson", start: base)
        m.transcript = said.enumerated().map { i, s in
            TranscriptLine(key: "e0:s1u\(i + 1)", at: base + Double(i * 40), speaker: nil, text: s.2, question: false,
                           voice: s.0, name: s.1)
        }
        m.voices = [VoiceEntry(key: "화자 1", name: "민호")]
        return m
    }

    private func write(_ rep: NSBitmapImageRep?, _ name: String, _ dir: URL) {
        guard let data = rep?.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
        try? data.write(to: dir.appendingPathComponent("\(name).png"))
    }

    private func transcript(_ m: SavedMeeting, _ name: String, _ dir: URL, lit: Int? = nil, scale: CGFloat = 1) {
        let page = TranscriptTextView.make()
        page.scale = scale
        page.canName = lit != nil
        page.named = lit
        page.show(TranscriptDoc.make(meeting: m, people: [], calendar: Calendar(identifier: .gregorian), unheard: [], scale: scale))
        page.layoutPage(width: 760)
        let host = NSView(frame: page.bounds)
        host.wantsLayer = true
        host.layer?.backgroundColor = NSColor.white.cgColor
        host.addSubview(page)
        guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { return XCTFail("no rep") }
        host.cacheDisplay(in: host.bounds, to: rep)
        write(rep, name, dir)
    }

    private func png<V: View>(_ view: V, _ name: String, _ dir: URL) {
        let r = ImageRenderer(content: view.padding(24).frame(width: 700).background(Color(white: 0.96)).environment(\.colorScheme, .light))
        r.scale = 2
        guard let tiff = r.nsImage?.tiffRepresentation else { return XCTFail("no image for \(name)") }
        write(NSBitmapImageRep(data: tiff), name, dir)
    }

    func testRender() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_SPEAKER_RENDER_DIR"] else { throw XCTSkip("render only") }
        let dir = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let m = meeting()
        transcript(m, "0-before", dir)
        // the ⌄ beside a name the pointer is on (drawn as when its namer is open), at the text sizes the page has
        transcript(m, "0-name-lit", dir, lit: 1)
        transcript(m, "0-name-lit-large", dir, lit: 1, scale: 1.3)
        // taken up again (이어서 듣기): the divider before the first turn after the gap
        var resumed = m
        resumed.resumes = [MeetingResume(stopped: base + 170, resumed: base + 190)]
        transcript(resumed, "5-resumed", dir)
        png(VStack(spacing: 12) {
            NamingToastView(toast: NamingToast(text: "이 말을 앤디로 바꿨어요"), undo: {}, gone: {})
            NamingToastView(toast: NamingToast(text: "화자 2의 말 3개를 앤디로 바꿨어요"), undo: {}, gone: {})
            NamingToastView(toast: NamingToast(text: "화자 2를 해리로 바꿨어요"), undo: {}, gone: {})
            NamingToastView(toast: NamingToast(text: "화자 4를 민호와 합쳤어요"), undo: {}, gone: {})
        }, "4-toasts", dir)
    }
}

/// The namer dates remembered people with the main window's calendar; the main window is not part of the tests.
enum MainView {
    static let calendar = Calendar(identifier: .gregorian)
}
