import XCTest
import SwiftUI

/// A note's lines in the transcript (docs/24 2절 2번): the engine's ids as transcript keys, and how the lit lines and the
/// "View in transcript" chip look. The drawing runs only when TACIE_NOTES_RENDER_DIR names a folder (pass it as
/// TEST_RUNNER_TACIE_NOTES_RENDER_DIR); nothing opens on screen.
@MainActor
final class NotesSourceTests: XCTestCase {
    private let base = Date(timeIntervalSince1970: 1_792_000_000)

    private func meeting(_ said: [(String, String)]) -> SavedMeeting {
        var m = SavedMeeting(mode: "inperson", start: base)
        m.transcript = said.enumerated().map { i, s in
            TranscriptLine(key: s.0, at: base + Double(i * 40), speaker: nil, text: s.1, question: false, voice: "화자 \(i % 2 + 1)")
        }
        return m
    }

    func testLineKeysTakeTheLastSessionsLineForABareId() {
        let m = meeting([("e0:s1u1", "가"), ("e0:s1u2", "나"), ("e1:s1u1", "다")])
        // a bare id is the session going on at the end (a restarted engine counts from s1u1 again); a key stays as sent
        XCTAssertEqual(m.lineKeys(["s1u1", "e0:s1u2"]), ["e1:s1u1", "e0:s1u2"])
        // lines not in the transcript (joined into a later one) go; none left is nil
        XCTAssertEqual(m.lineKeys(["s1u9", "e0:s1u1"]), ["e0:s1u1"])
        XCTAssertNil(m.lineKeys(["s1u9"]))
        XCTAssertNil(m.lineKeys([]))
        // a bare id the last run never reached is not the older run's line
        XCTAssertNil(m.lineKeys(["s1u2"]))
    }

    func testAQuestionMadeAgainKeepsItsReview() {
        var m = meeting([("e0:s1u1", "가")])
        m.questions = [question("v1", "그거 뭐죠?", review: QuestionReview(verdict: QuestionReview.notQuestion))]
        // searched again with a new source and still not found: rebuilt without its review
        m.setQuestion(question("v1", "그거 뭐죠?", review: nil))
        XCTAssertEqual(m.questions[0].review?.verdict, QuestionReview.notQuestion)
        XCTAssertFalse(m.questions[0].toDocument)
    }

    func testSourceLineIsTheEarliestCitedLine() {
        let m = meeting([("e0:s1u1", "가"), ("e0:s1u2", "나"), ("e0:s1u3", "다")])
        var e = LedgerEntry(kind: "decision", text: "정함")
        XCTAssertNil(m.sourceLine(e))
        e.refs = ["e0:s1u3", "e0:s1u2"]
        XCTAssertEqual(m.sourceLine(e)?.key, "e0:s1u2")
    }

    func testRefsSurviveSavingAndOldFilesReadWithout() throws {
        var e = LedgerEntry(kind: "decision", text: "정함")
        e.refs = ["e0:s1u2"]
        let back = try JSONDecoder().decode(LedgerEntry.self, from: JSONEncoder().encode(e))
        XCTAssertEqual(back.refs, ["e0:s1u2"])
        let old = try JSONDecoder().decode(LedgerEntry.self, from: Data(#"{"kind":"decision","text":"정함","done":false}"#.utf8))
        XCTAssertNil(old.refs)
    }

    private func question(_ id: String, _ text: String, review: QuestionReview?, unanswered: Bool = true) -> SavedQuestion {
        var q = SavedQuestion(id: id, question: text, asker: nil, at: base, answer: nil, cards: [], cited: [], unanswered: unanswered)
        q.review = review
        return q
    }

    func testQuestionsToDocumentDropSmallTalkAndReadRewritten() {
        var m = meeting([("e0:s1u1", "회익급 알아듣기 기준이 몇이었죠?"), ("e0:s1u2", "0.8이요")])
        m.voices = [VoiceEntry(key: "화자 2", name: "민호")]
        m.transcript[0].question = true
        m.questions = [
            question("v1", "회익급 알아듣기 기준이 몇이었죠?",
                     review: QuestionReview(verdict: QuestionReview.answered, question: "회의 끝 알아듣기 기준은 몇인가요?",
                                            answer: "[[화자 2]]: 0.8이라고 답함.", refs: ["e0:s1u2"])),
            question("v2", "다들 커피 챙겼어요?", review: QuestionReview(verdict: QuestionReview.notQuestion)),
            question("v3", "데이터베이스 접근 되나 안 되나?", review: nil),
            question("v4", "찾은 질문", review: nil, unanswered: false),
        ]
        XCTAssertEqual(m.questions.filter(\.toDocument).map(\.id), ["v1", "v3"])
        XCTAssertEqual(m.toDocumentLines, ["- 회의 끝 알아듣기 기준은 몇인가요?", "  - 회의에서 나온 답: 민호: 0.8이라고 답함.",
                                           "- 데이터베이스 접근 되나 안 되나?"])
        XCTAssertEqual(m.questionLine(m.questions[0])?.key, "e0:s1u1")
    }

    func testReviewSurvivesSavingAndOldQuestionsReadWithout() throws {
        let q = question("v1", "x", review: QuestionReview(verdict: QuestionReview.doc, question: "x인가요?"))
        XCTAssertEqual(try JSONDecoder().decode(SavedQuestion.self, from: JSONEncoder().encode(q)).review, q.review)
        let old = try JSONDecoder().decode(SavedQuestion.self, from: Data(#"""
            {"id":"v1","question":"x","at":0,"cards":[],"cited":[],"unanswered":true}
            """#.utf8))
        XCTAssertNil(old.review)
        XCTAssertTrue(old.toDocument)
    }

    func testRender() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_NOTES_RENDER_DIR"] else { throw XCTSkip("render only") }
        let dir = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let m = meeting([
            ("e0:s1u1", "재시도 횟수는 몇 번으로 할까요? 지금은 다섯 번이에요."),
            ("e0:s1u2", "다섯 번이면 결제사 쪽에서 차단이 걸린 적이 있어요. 지난달에 두 번 그랬어요."),
            ("e0:s1u3", "그럼 세 번으로 줄이죠. 세 번이면 차단 기준 안쪽이에요."),
            ("e0:s1u4", "네, 세 번으로 가요."),
            ("e0:s1u5", "제가 오늘 설정 바꿔서 올릴게요."),
        ])
        // the transcript as a 대화 기록에서 보기 leaves it: the cited lines lit
        let page = TranscriptTextView.make()
        page.show(TranscriptDoc.make(meeting: m, people: [], calendar: Calendar(identifier: .gregorian), unheard: [], scale: 1))
        page.layoutPage(width: 680)
        page.marked = ["e0:s1u2", "e0:s1u3"]
        let host = NSView(frame: page.bounds)
        host.wantsLayer = true
        host.layer?.backgroundColor = NSColor.white.cgColor
        host.addSubview(page)
        let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: rep)
        try XCTUnwrap(rep.representation(using: .png, properties: [:])).write(to: dir.appendingPathComponent("transcript-lit.png"))

        // a ledger line under the pointer, as LedgerBullet lays it out: the chip before the bin
        let row = HStack(alignment: .firstTextBaseline, spacing: 10) {
            Circle().fill(Theme.ink.opacity(0.55)).frame(width: 4.5, height: 4.5).offset(y: -3.5)
            Text("재시도는 3회까지로 정함. 5회면 결제사 차단에 걸려서 뺌.").uiFont(15).foregroundStyle(Theme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 2) {
                LedgerSourceChip(at: base + 40, shown: true) {}
                LedgerBin(shown: true) {}
            }
            .alignmentGuide(.firstTextBaseline) { $0[VerticalAlignment.center] + 5 }
        }
        .padding(.horizontal, 10).padding(.vertical, 6)
        .background(Theme.ink.opacity(0.035), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        let r = ImageRenderer(content: VStack(alignment: .leading, spacing: 24) { row; missingRows }.padding(24).frame(width: 720)
            .background(Color.white).environment(\.colorScheme, .light))
        r.scale = 2
        let tiff = try XCTUnwrap(r.nsImage?.tiffRepresentation)
        try XCTUnwrap(NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]))
            .write(to: dir.appendingPathComponent("ledger-row.png"))
    }

    /// 문서로 남길 질문 rows as the meeting page lays them out (the asker on the right is left out).
    private var missingRows: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("문서로 남길 질문").uiFont(17, weight: .semibold).foregroundStyle(Theme.ink).padding(.bottom, 14)
            ForEach(Array([("회의 끝 알아듣기 기준은 몇으로 정해져 있나요?", nil),
                           ("직원들 백업 파일 받기는 전환 며칠 전부터 시작하나요?", "지훈: 2주 전쯤일 거라고 했지만 확실하지 않음."),
                           ("지금 데이터베이스에 접근할 수 있나요?", nil)].enumerated()), id: \.offset) { _, row in
                VStack(spacing: 0) {
                    Rectangle().fill(Theme.hairline).frame(height: 1)
                    HStack(alignment: .firstTextBaseline) {
                        VStack(alignment: .leading, spacing: 5) {
                            Text(row.0).uiFont(15, weight: .medium).foregroundStyle(Theme.ink)
                            if let a = row.1 { Text("회의에서 나온 답 · \(a)").uiFont(13.5).foregroundStyle(Theme.ink2) }
                        }
                        Spacer(minLength: 16)
                        Text("화자 2 · 오후 2:19").uiFont(12).foregroundStyle(Theme.ink3)
                    }
                    .padding(.vertical, 12)
                }
            }
        }
    }
}
