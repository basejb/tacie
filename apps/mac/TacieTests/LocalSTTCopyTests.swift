import XCTest
@testable import Tacie

@MainActor
final class LocalSTTCopyTests: XCTestCase {
    private let banned = ["받아쓰", "섬"]

    func testSettingsDetailPerPhase() {
        XCTAssertEqual(LocalSTTCopy.detail(.off),
                       "회의 소리를 음성 인식 서버로 보내요. 말이 끝나면 바로 대화 기록에 적히고, 영어 용어를 더 정확히 적어요.")
        XCTAssertEqual(LocalSTTCopy.detail(.ready), "회의 소리를 Mac 밖으로 보내지 않아요. 말이 끝나고 1초쯤 뒤에 적히고, 영어 용어를 더 자주 틀려요.")
        XCTAssertEqual(LocalSTTCopy.detail(.unsupported),
                       "회의 소리를 음성 인식 서버로 보내요. 온디바이스는 메모리가 16GB 이상인 Mac에서만 쓸 수 있어요.")
        XCTAssertEqual(LocalSTTCopy.detail(.downloading(done: 240_000_000, total: 570_000_000)),
                       "음성 인식 모델 받는 중 · 240MB / 570MB · 다 받기 전까지는 서버에서 음성 인식을 해요.")
        XCTAssertEqual(LocalSTTCopy.detail(.waiting),
                       "인터넷에 연결되면 이어서 받아요. 그 전까지는 서버에서 음성 인식을 해요.")
        XCTAssertEqual(LocalSTTCopy.detail(.needsSpace(1)), LocalSTTCopy.detail(.off))
    }

    func testSidebarRowPerPhase() {
        let d = LocalSTTCopy.sidebar(.downloading(done: 240_000_000, total: 570_000_000))
        XCTAssertEqual(d?.title, "음성 인식 모델 받는 중")
        XCTAssertEqual(d?.percent, "42%")
        XCTAssertEqual(d?.detail, "240MB / 570MB · 다 받으면 온디바이스 음성 인식으로 바뀌어요")
        XCTAssertEqual(d?.failed, false)
        let f = LocalSTTCopy.sidebar(.failed("네트워크 연결이 끊겼어요"))
        XCTAssertEqual(f?.title, "음성 인식 모델을 받지 못했어요")
        XCTAssertEqual(f?.detail, "네트워크 연결이 끊겼어요")
        XCTAssertEqual(f?.failed, true)
        XCTAssertEqual(LocalSTTCopy.sidebar(.failed("받지 못했어요"))?.detail, "연결을 확인하고 다시 받아 주세요")
        let w = LocalSTTCopy.sidebar(.waiting)
        XCTAssertEqual(w?.title, "음성 인식 모델 받기 대기 중")
        XCTAssertEqual(w?.detail, "인터넷에 연결되면 이어서 받아요")
        let hidden: [LocalSTT.Phase] = [.off, .unsupported, .needsSpace(1), .ready]
        for p in hidden { XCTAssertNil(LocalSTTCopy.sidebar(p)) }
        XCTAssertEqual(LocalSTTCopy.doneText, "온디바이스 음성 인식을 켰어요")
    }

    func testAlertsAndSpaceNumber() {
        XCTAssertEqual(LocalSTTCopy.consentTitle, "음성 인식 모델을 받을까요?")
        XCTAssertEqual(LocalSTTCopy.consentBody,
                       "온디바이스 음성 인식에는 0.6GB 모델이 필요해요. 다 받기 전에 시작한 회의는 지금처럼 서버에서 음성 인식을 해요.")
        XCTAssertEqual(LocalSTTCopy.spaceTitle, "저장 공간이 부족해요")
        XCTAssertEqual(LocalSTTCopy.spaceBody(more: 400_000_000),
                       "음성 인식 모델을 받으려면 0.4GB가 더 필요해요. 공간을 비운 뒤 다시 켜 주세요.")
        XCTAssertEqual(LocalSTTCopy.gb(400_000_001), "0.5GB")   // rounds up
        XCTAssertEqual(LocalSTTCopy.gb(1_000_000_000), "1.0GB")
        XCTAssertEqual(LocalSTTCopy.gb(0), "0.0GB")
    }

    func testMeetingLine() {
        XCTAssertEqual(LocalSTTCopy.meetingLine(.downloading(done: 240_000_000, total: 570_000_000)),
                       "이번 회의는 서버에서 음성 인식을 해요 · 음성 인식 모델 42%")
        XCTAssertEqual(LocalSTTCopy.meetingLine(.failed("x")), "이번 회의는 서버에서 음성 인식을 해요 · 음성 인식 모델을 받지 못했어요")
        XCTAssertNil(LocalSTTCopy.meetingLine(.ready))
        XCTAssertEqual(LocalSTTCopy.paused, "음성 인식이 잠시 멈췄어요")
    }

    func testNoBannedWordsAnywhere() {
        var all = LocalSTTCopy.allStatic
        let phases: [LocalSTT.Phase] = [.off, .unsupported, .needsSpace(400_000_000), .downloading(done: 1, total: 570_000_000),
                                        .waiting, .failed("네트워크 연결이 끊겼어요"), .ready]
        for p in phases {
            all.append(LocalSTTCopy.detail(p))
            if let r = LocalSTTCopy.sidebar(p) { all += [r.title, r.detail] }
            if let m = LocalSTTCopy.meetingLine(p) { all.append(m) }
        }
        for text in all { for word in banned { XCTAssertFalse(text.contains(word), "\(word) in \(text)") } }
    }

    func testNoticeOncePerMeetingAndAgainAfterRestart() {
        var n = STTMeetingNotice()
        let a = STTMeetingState(line: "line", starts: 1)
        XCTAssertEqual(n.note(stt: "server", practice: false, state: a), "line")
        XCTAssertNil(n.note(stt: "server", practice: false, state: a))          // same meeting, same download
        XCTAssertNil(n.note(stt: "local", practice: false, state: a))
        XCTAssertEqual(n.note(stt: "server", practice: false, state: STTMeetingState(line: "again", starts: 2)), "again")
        n.reset()                                                               // next meeting
        XCTAssertEqual(n.note(stt: "server", practice: false, state: a), "line")
    }

    func testNoNoticeInPracticeOrWhenNotNeeded() {
        var n = STTMeetingNotice()
        let a = STTMeetingState(line: "line", starts: 1)
        XCTAssertNil(n.note(stt: "server", practice: true, state: a))
        XCTAssertNil(n.note(stt: "server", practice: false, state: nil))        // switch off or model ready
        XCTAssertNil(n.note(stt: nil, practice: false, state: a))               // an older engine says nothing
        XCTAssertEqual(n.note(stt: "server", practice: false, state: a), "line")  // practice did not use it up
    }

    func testMeetingStateFollowsSwitchAndPhase() {
        let d = UserDefaults(suiteName: UUID().uuidString)!
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let pack = ModelPack(name: "stt", base: URL(string: "http://x/")!, dir: dir,
                             files: [ModelFile(name: "m.bin", size: 100, sha256: "g")])
        let s = LocalSTT(pack: pack, defaults: d, memory: 16 * 1024 * 1024 * 1024, free: { 100_000_000_000 },
                         fetch: { _, _ in try await Task.sleep(for: .seconds(5)) }, hash: { _ in "g" },
                         online: { true }, engine: { nil })
        XCTAssertNil(s.meetingState)               // off
        s.turnOn(consented: true)
        XCTAssertEqual(s.meetingState?.starts, 1)
        XCTAssertNotNil(s.meetingState?.line)
        s.turnOff()
        XCTAssertNil(s.meetingState)
    }
}
