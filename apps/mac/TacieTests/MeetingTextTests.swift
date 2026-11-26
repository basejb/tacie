import XCTest

final class MeetingTextTests: XCTestCase {
    private let seoul = TimeZone(identifier: "Asia/Seoul")!
    private lazy var cal: Calendar = {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = seoul
        c.firstWeekday = 2  // weeks start on Monday
        return c
    }()

    private func kst(_ h: Int, _ m: Int = 0, _ s: Int = 0, day: Int = 23, month: Int = 10, year: Int = 2026) -> Date {
        var c = DateComponents(year: year, month: month, day: day, hour: h, minute: m, second: s)
        c.timeZone = seoul
        return Calendar(identifier: .gregorian).date(from: c)!
    }

    private var now: Date { kst(16) }  // Friday 2026-10-23

    private func meeting(_ start: Date, minutes: Int? = 47, mode: String = "remote", title: String? = nil) -> SavedMeeting {
        var m = SavedMeeting(mode: mode, start: start)
        m.end = minutes.map { start.addingTimeInterval(Double($0) * 60) }
        m.title = title
        return m
    }

    private func line(_ key: String, _ at: Date, _ text: String, question: Bool = false) -> TranscriptLine {
        TranscriptLine(key: key, at: at, speaker: "them", text: text, question: question)
    }

    private let t0 = Date(timeIntervalSince1970: 1_790_000_000)

    private func w(_ text: String, _ start: Int, _ end: Int) -> SpokenWord { SpokenWord(text: text, start: start, end: end) }

    private func line(_ key: String, audioAt: Int?, words: [SpokenWord]?) -> TranscriptLine {
        var l = TranscriptLine(key: key, at: t0, speaker: "them", text: key, question: false)
        l.audioAt = audioAt
        l.words = words
        return l
    }

    func testMergedLineKeepsEarlierWords() {
        var m = SavedMeeting(mode: "remote", start: t0)
        m.addLine(line("e1:s1u1", audioAt: 1000, words: [w("링크는", 1000, 1400)]), replacing: nil)
        m.addLine(line("e1:s1u2", audioAt: 3000, words: [w("7일이에요", 3000, 3600)]), replacing: "e1:s1u1")
        XCTAssertEqual(m.transcript.count, 1)
        XCTAssertEqual(m.transcript[0].audioAt, 1000)
        XCTAssertEqual(m.transcript[0].words?.map(\.text), ["링크는", "7일이에요"])
    }

    /// A line the engine joins to the one before keeps both halves' translations (docs/30).
    func testMergedLineKeepsBothTranslations() {
        var m = SavedMeeting(mode: "remote", start: t0)
        var a = line("e1:s1u1", audioAt: 1000, words: nil)
        a.translation = "알겠어요."; a.translatedFrom = "en"
        var b = line("e1:s1u2", audioAt: 3000, words: nil)
        b.translation = "간격은 그대로 두죠."; b.translatedFrom = "en"
        m.addLine(a, replacing: nil)
        m.addLine(b, replacing: "e1:s1u1")
        XCTAssertEqual(m.transcript[0].translation, "알겠어요. 간격은 그대로 두죠.")
        XCTAssertEqual(m.transcript[0].translatedFrom, "en")
    }

    func testMergedLineWithOnlyOneSideWordsKeepsThem() {
        var m = SavedMeeting(mode: "remote", start: t0)
        m.addLine(line("a", audioAt: 1000, words: nil), replacing: nil)
        m.addLine(line("b", audioAt: 3000, words: [w("x", 3000, 3100)]), replacing: "a")
        XCTAssertEqual(m.transcript[0].words?.map(\.text), ["x"])
        XCTAssertEqual(m.transcript[0].audioAt, 1000)
    }

    func testKeepsRecordedMeetingWithoutLines() {
        var m = SavedMeeting(mode: "inperson", start: t0)
        XCTAssertFalse(m.keep)
        m.recording = .kept; m.recordingSeconds = 42
        XCTAssertTrue(m.keep)
        m.recordingSeconds = 0; XCTAssertFalse(m.keep)
    }

    func testPlacedWordsAreMsFromRecordingStart() {
        XCTAssertEqual(SpokenWord.placed([HeardWord(text: "a", start: 0.12, end: 0.5)], at: 2000), [SpokenWord(text: "a", start: 2120, end: 2500)])
    }

    func testDropRecording() {
        var m = SavedMeeting(mode: "remote", start: t0)
        m.addLine(line("a", audioAt: 1000, words: [w("x", 1000, 1100)]), replacing: nil)
        m.recording = .kept; m.recordingSeconds = 5; m.unheard = [AudioSpan(start: 0, end: 500)]
        m.dropRecording()
        XCTAssertEqual(m.recording, .deleted)
        XCTAssertNil(m.unheard)
        XCTAssertNil(m.transcript[0].words)
        XCTAssertEqual(m.transcript[0].text, "a")
    }

    func testAddLineKeepsTimeOrderAndReplacesJoinedHalf() {
        var m = meeting(kst(10))
        m.addLine(line("e0:s1u1", kst(10, 0, 0), "그 설정은"), replacing: nil)
        m.addLine(line("e0:s1u3", kst(10, 0, 5), "어디서 바꿔요?"), replacing: nil)
        m.addLine(line("e0:s1u2", kst(10, 0, 2), "네"), replacing: nil)  // came back late
        XCTAssertEqual(m.transcript.map(\.key), ["e0:s1u1", "e0:s1u2", "e0:s1u3"])
        m.addLine(line("e0:s1u4", kst(10, 0, 0), "그 설정은 어디서 바꿔요?", question: true), replacing: "e0:s1u1")
        XCTAssertEqual(m.transcript.map(\.key), ["e0:s1u4", "e0:s1u2", "e0:s1u3"])
    }

    func testLinesFromARestartedEngineKeepTheirOwnKeys() {
        var m = meeting(kst(10))
        m.addLine(line("e0:s1u1", kst(10, 1), "앞 엔진의 첫 말"), replacing: nil)
        m.addLine(line("e1:s1u1", kst(10, 5), "새 엔진의 첫 말"), replacing: nil)
        XCTAssertEqual(m.transcript.count, 2)
        m.addLine(line("e1:s1u2", kst(10, 5), "새 엔진의 첫 말 이어서"), replacing: "e1:s1u1")
        XCTAssertEqual(m.transcript.map(\.key), ["e0:s1u1", "e1:s1u2"])
    }

    func testKeepNeedsWords() {
        var m = meeting(kst(10))
        m.questions = [SavedQuestion(id: "m1", question: "배포 전 검사는?", asker: "typed", at: kst(10, 1), answer: nil,
                                     cards: [], cited: [], unanswered: false)]
        XCTAssertFalse(m.keep)
        m.addLine(line("e0:s1u1", kst(10, 2), "시작할게요"), replacing: nil)
        XCTAssertTrue(m.keep)
    }

    func testMatchesSearchesEverything() {
        var m = meeting(kst(14), title: "결제 실패 재시도 정책")
        m.questions = [SavedQuestion(id: "v1", question: "실패 알림은 어디서 보내요?", asker: "me", at: kst(14, 14),
                                     answer: "PR #1284에서 큐를 나눴어요.", cards: [], cited: [], unanswered: false)]
        m.addLine(line("e0:s1u1", kst(14, 2), "재시도 간격부터 볼까요"), replacing: nil)
        XCTAssertTrue(m.matches("pr #1284"))
        XCTAssertTrue(m.matches("재시도 간격"))
        XCTAssertTrue(m.matches("결제"))
        XCTAssertTrue(m.matches(""))
        XCTAssertFalse(m.matches("환불"))
    }

    func testDisplayTitleFallsBack() {
        var m = meeting(kst(14), title: "결제 실패 재시도 정책")
        XCTAssertEqual(m.displayTitle(calendar: cal), "결제 실패 재시도 정책")
        m.title = nil
        m.notes = .waiting
        XCTAssertEqual(m.displayTitle(calendar: cal), "원격 회의")
        m.notes = .failed
        XCTAssertEqual(m.displayTitle(calendar: cal), "오후 2:00 원격 회의")
    }

    func testMetaLines() {
        XCTAssertEqual(meeting(kst(14)).meta(now: now, calendar: cal), "오후 2:00 · 47분 · 원격")
        XCTAssertEqual(meeting(kst(9, 30, day: 21), minutes: 61).meta(now: now, calendar: cal), "10월 21일 수요일 · 61분 · 원격")
        let live = meeting(kst(15, 30), minutes: nil)
        XCTAssertEqual(live.meta(now: kst(15, 42), calendar: cal), "오후 3:30부터 · 12분째")
        XCTAssertEqual(meeting(kst(11), minutes: 0, mode: "inperson").meta(now: now, calendar: cal), "오전 11:00 · 1분 · 대면")
    }

    func testHeaderLine() {
        XCTAssertEqual(meeting(kst(14)).header(calendar: cal), "10월 23일 금요일 오후 2:00 · 원격 회의 · 47분")
    }

    func testGroupsByDay() {
        let ms = [
            meeting(kst(14), title: "오늘 1"), meeting(kst(11), title: "오늘 2"),
            meeting(kst(16, day: 22), title: "어제"),
            meeting(kst(10, day: 20), title: "이번 주"),
            meeting(kst(10, day: 15), title: "지난주"),
            meeting(kst(10, day: 2), title: "9월"),
            meeting(kst(10, day: 2, month: 9), title: "9월 회의"),
            meeting(kst(10, day: 5, month: 12, year: 2025), title: "작년"),
        ]
        let groups = MeetingGroups.group(ms, now: now, calendar: cal)
        XCTAssertEqual(groups.map(\.title), ["오늘", "어제", "이번 주", "지난주", "10월", "9월", "2025년"])
        XCTAssertEqual(groups[0].meetings.compactMap(\.title), ["오늘 1", "오늘 2"])
    }

    func testGroupsOpenAndFull() {
        XCTAssertEqual(MeetingGroups.open, [.today, .yesterday, .thisWeek, .lastWeek])
        XCTAssertTrue(MeetingGroups.full.contains(.thisWeek))
        XCTAssertFalse(MeetingGroups.full.contains(.lastWeek))  // last week's rows are one line
    }

    func testCollectedGathersNewestFirstAndSkipsLive() {
        var old = meeting(kst(10, day: 20), title: "지난 회의")
        old.ledger = [LedgerEntry(kind: "decision", text: "3번까지 재시도해요."),
                      LedgerEntry(kind: "commitment", text: "PR 올리기", owner: "me"),
                      LedgerEntry(kind: "commitment", text: "문서 쓰기", done: true)]
        old.questions = [
            SavedQuestion(id: "a", question: "dead letter는 누가 봐요?", asker: nil, at: kst(10, day: 20),
                          answer: nil, cards: [], cited: [], unanswered: true),
            SavedQuestion(id: "b", question: "연결 전 질문", asker: nil, at: kst(10, day: 20),
                          answer: nil, cards: [], cited: [], unanswered: true, noSource: true),
        ]
        var recent = meeting(kst(14), title: "오늘 회의")
        recent.ledger = [LedgerEntry(kind: "commitment", text: "측정하기")]
        var live = meeting(kst(15), minutes: nil, title: "회의 중")
        live.ledger = [LedgerEntry(kind: "commitment", text: "아직 아님")]
        let all = [old, live, recent]

        XCTAssertEqual(Collected.todos(all, done: false).map(\.meeting.title), ["오늘 회의", "지난 회의"])
        XCTAssertEqual(Collected.todos(all, done: true).map(\.indices), [[2]])
        XCTAssertEqual(Collected.decisions(all).map(\.meeting.title), ["지난 회의"])
        XCTAssertEqual(Collected.questions(all).flatMap { $0.questions.map(\.id) }, ["a"])  // not the one heard before connecting
        XCTAssertEqual(Collected.count(.todos, in: all), 2)
        XCTAssertNil(Collected.count(.decisions, in: all))
        XCTAssertEqual(Collected.count(.questions, in: all), 1)
    }

    func testPinnedIsOptionalInFiles() throws {
        var m = meeting(kst(14), title: "고정")
        let plain = try JSONEncoder().encode(m)
        XCTAssertFalse(String(decoding: plain, as: UTF8.self).contains("pinned"))
        m.pinned = true
        let back = try JSONDecoder().decode(SavedMeeting.self, from: JSONEncoder().encode(m))
        XCTAssertEqual(back.pinned, true)
    }

    func testMarkdownHasSectionsAndChecks() {
        var m = meeting(kst(14), title: "결제 실패 재시도 정책")
        m.notes = .done
        m.summary = ["결제 실패는 3번까지 자동으로 다시 시도해요."]
        m.ledger = [LedgerEntry(kind: "decision", text: "자동 재시도는 최대 3번까지 해요."),
                    LedgerEntry(kind: "commitment", text: "재시도 간격을 바꾸는 PR 올리기", owner: "me", due: "금요일까지"),
                    LedgerEntry(kind: "commitment", text: "알림 문구 초안 공유하기", owner: "them", done: true)]
        m.questions = [
            SavedQuestion(id: "v1", question: "실패 알림은 어디서 보내요?", asker: "me", at: kst(14, 14),
                          answer: "이메일만 보내요. [1]", cards: [], cited: [], unanswered: false),
            SavedQuestion(id: "v2", question: "이중 결제가 난 적이 있었나요?", asker: "them", at: kst(14, 19),
                          answer: nil, cards: [], cited: [], unanswered: true),
        ]
        let md = m.markdown(calendar: cal)
        for part in ["# 결제 실패 재시도 정책", "## 요약", "## 결정과 할 일", "### 결정 사항", "### 할 일",
                     "- [ ] 재시도 간격을 바꾸는 PR 올리기", "- [x] 알림 문구 초안 공유하기", "## Tacie의 답",
                     "이메일만 보내요.", "## 문서로 남길 질문", "- 이중 결제가 난 적이 있었나요?"] {
            XCTAssertTrue(md.contains(part), part)
        }
        XCTAssertFalse(md.contains("[1]"))
        XCTAssertFalse(md.contains("### 남은 논의"))  // empty: left out
    }

    func testMarkdownWithTopicsHasASummaryParagraphAndTheFlow() {
        var m = meeting(kst(14), title: "결제 실패 재시도 정책")
        m.notes = .done
        m.summary = ["재시도 정책을 정하려고 모였어요.", "세 번까지로 했어요."]
        m.topics = [NoteTopic(title: "재시도 횟수", points: ["민지가 재시도가 많다고 꺼냈어요.", "세 번까지로 했어요."]),
                    NoteTopic(title: "알림 문구", points: ["문구는 다음에 봐요.", "발송 시각은 미정."], roles: ["discussion", "open"]),
                    NoteTopic(title: "환불", points: ["환불 기준을 다시 봐야 함."], roles: ["open"])]
        m.ledger = [LedgerEntry(kind: "decision", text: "재시도는 세 번까지로 했어요. 다섯 번이면 차단돼서예요.")]
        let md = m.markdown(calendar: cal)
        // results first, then the talk topic by topic (docs/24 7-3); an open point says so once
        XCTAssertTrue(md.contains("## 요약\n재시도 정책을 정하려고 모였어요. 세 번까지로 했어요.\n\n## 결정과 할 일"), md)
        XCTAssertTrue(md.contains("\n\n## 논의 흐름\n"
                                  + "### 1. 재시도 횟수\n- 민지가 재시도가 많다고 꺼냈어요.\n- 세 번까지로 했어요.\n"
                                  + "### 2. 알림 문구\n- 문구는 다음에 봐요.\n- 발송 시각은 미정.\n"
                                  + "### 3. 환불\n- 환불 기준을 다시 봐야 함. (미정)\n"), md)
        XCTAssertLessThan(md.range(of: "## 결정과 할 일")!.lowerBound, md.range(of: "## 논의 흐름")!.lowerBound)
        XCTAssertTrue(m.matches("알림 문구"))
        // older notes (no topics) keep their summary as lines
        m.topics = nil
        XCTAssertTrue(m.markdown(calendar: cal).contains("## 요약\n- 재시도 정책을 정하려고 모였어요.\n- 세 번까지로 했어요.\n"))
        XCTAssertFalse(m.markdown(calendar: cal).contains("## 논의 흐름"))
    }

    func testOldMeetingDecodesAsMeeting() throws {
        let m = meeting(kst(14), title: "x")
        let plain = try JSONEncoder().encode(m)
        XCTAssertFalse(String(decoding: plain, as: UTF8.self).contains("kind"))
        let back = try JSONDecoder().decode(SavedMeeting.self, from: plain)
        XCTAssertNil(back.kind)
        XCTAssertEqual(back.shownKind, .meeting)
    }

    func testInterviewMarkdownUsesItsHeadings() {
        var m = meeting(kst(14), title: "디자인 스튜디오 대표 인터뷰")
        m.notes = .done
        m.kind = .interview
        m.summary = ["창업 과정을 들음."]
        m.topics = [NoteTopic(title: "왜 패션으로 옮겼나요?", points: ["반응이 좋았다고 함."])]
        m.ledger = [LedgerEntry(kind: "fact", text: "거래액 30억 원임."),
                    LedgerEntry(kind: "quote", text: "여러 번 던져 봐요.")]
        let md = m.markdown(calendar: cal)
        XCTAssertTrue(md.contains("## 질문과 답\n### 1. 왜 패션으로 옮겼나요?\n- 반응이 좋았다고 함.\n\n## 핵심 숫자·사실\n- 거래액 30억 원임.\n\n## 기억할 말\n- 여러 번 던져 봐요.\n"), md)
        XCTAssertFalse(md.contains("결정"), md)
        XCTAssertFalse(md.contains("논의 흐름"), md)
    }

    func testTalkMarkdownUsesItsHeadings() {
        var m = meeting(kst(14), title: "임베딩 기초")
        m.notes = .done
        m.kind = .talk
        m.summary = ["임베딩을 설명함."]
        m.topics = [NoteTopic(title: "벡터", points: ["글을 숫자로 바꿈."])]
        m.ledger = [LedgerEntry(kind: "concept", text: "임베딩: 글을 벡터로 바꾼 것."),
                    LedgerEntry(kind: "takeaway", text: "같은 모델로 묻고 찾기.")]
        let md = m.markdown(calendar: cal)
        for part in ["## 내용 흐름", "## 핵심 개념\n- 임베딩: 글을 벡터로 바꾼 것.", "## 기억할 점\n- 같은 모델로 묻고 찾기."] {
            XCTAssertTrue(md.contains(part), part)
        }
    }

    func testNotesKindWords() {
        XCTAssertEqual(NotesKind.allCases.map(\.button), ["회의", "인터뷰", "강의"])
        XCTAssertEqual(NotesKind.allCases.map(\.menuTitle), ["회의", "인터뷰·대담", "강의·발표"])
        XCTAssertEqual(NotesKind.interview.menuDetail, "질문과 답, 핵심 숫자, 기억할 말")
        XCTAssertEqual(NotesKind.meeting.sections.map(\.kind), ["decision", "commitment", "open_question"])
        XCTAssertEqual(NotesKind.talk.sections.map(\.title), ["핵심 개념", "기억할 점"])
        XCTAssertEqual(NotesKind.meeting.writing(rewriting: false), "요약, 논의 흐름, 결정 사항, 할 일을 쓰고 있어요. 보통 1분 안에 끝나요.")
        XCTAssertEqual(NotesKind.interview.writing(rewriting: true), "인터뷰 형식으로 다시 쓰고 있어요. 보통 1분 안에 끝나요.")
        XCTAssertEqual(NotesKind.meeting.writing(rewriting: true), "회의 형식으로 다시 쓰고 있어요. 보통 1분 안에 끝나요.")
    }

    func testTopicsAreOptionalInFiles() throws {
        var m = meeting(kst(14), title: "x")
        let old = try JSONDecoder().decode(SavedMeeting.self, from: JSONEncoder().encode(m))
        XCTAssertNil(old.topics)
        m.apply(title: nil, summary: ["요약"], topics: [NoteTopic(title: "주제", points: ["말했어요."])], entries: [])
        let back = try JSONDecoder().decode(SavedMeeting.self, from: JSONEncoder().encode(m))
        XCTAssertEqual(back.topics, [NoteTopic(title: "주제", points: ["말했어요."])])
    }

    func testFileNameHasDateAndSafeTitle() {
        XCTAssertEqual(meeting(kst(14), title: "API v2/v3: 정리").fileName(calendar: cal), "2026-10-23 API v2 v3 정리.md")
        XCTAssertEqual(meeting(kst(14)).fileName(calendar: cal), "2026-10-23 원격 회의.md")  // notes not in yet
    }

    func testTranscriptTextIsSpeakersAndWordsOnly() {
        var m = meeting(kst(14), title: "결제 실패 재시도 정책")
        m.transcript = [
            TranscriptLine(key: "e0:s1u1", at: kst(14, 1), speaker: nil, text: "재시도는 몇 번까지 해요?", question: true, voice: "화자 1"),
            TranscriptLine(key: "e0:s1u2", at: kst(14, 1, 20), speaker: nil, text: "알림도 궁금해요.", question: false, voice: "화자 1"),
            TranscriptLine(key: "e0:s1u3", at: kst(14, 2), speaker: nil, text: "세 번까지예요.", question: false, voice: "화자 2", name: "지수"),
        ]
        m.questions = [SavedQuestion(id: "e0:s1u1", question: "재시도는 몇 번까지 해요?", asker: "them", at: kst(14, 1),
                                     answer: "세 번이에요. [1]", cards: [], cited: [], unanswered: false)]
        // a turn per speaker, as the page groups them; no times, 질문 tags or Tacie's answers
        XCTAssertEqual(m.transcriptText(calendar: cal), """
            결제 실패 재시도 정책
            \(m.header(calendar: cal))

            화자 1: 재시도는 몇 번까지 해요? 알림도 궁금해요.
            지수: 세 번까지예요.

            """)
        // the saved file has it after the notes, turns a blank line apart; a copy of the notes does not
        XCTAssertTrue(m.markdown(calendar: cal, transcript: true)
            .hasSuffix("\n## 대화 기록\n화자 1: 재시도는 몇 번까지 해요? 알림도 궁금해요.\n\n지수: 세 번까지예요.\n"))
        XCTAssertFalse(m.markdown(calendar: cal).contains("## 대화 기록"))
    }

    func testTranscriptTextBeforeSpeakersIsLines() {
        var m = meeting(kst(14), title: "x")
        m.transcript = [line("a", kst(14, 1), "시작할게요"), line("b", kst(14, 2), "네")]
        XCTAssertTrue(m.transcriptText(calendar: cal).hasSuffix("\n\n시작할게요\n네\n"))
    }

    func testMarkdownListsAttendees() {
        var m = meeting(kst(14), title: "결제 실패 재시도 정책")
        m.transcript = [
            TranscriptLine(key: "e0:s1u1", at: kst(14, 1), speaker: nil, text: "시작할게요", question: false, voice: "화자 1"),
            TranscriptLine(key: "e0:s1u2", at: kst(14, 2), speaker: nil, text: "재시도는 몇 번까지 해요?", question: true, voice: "화자 2"),
            TranscriptLine(key: "e0:s1u3", at: kst(14, 3), speaker: nil, text: "찾아볼게요", question: false, voice: "화자 3"),
        ]
        m.rename(key: "화자 1", to: "민재", person: nil)
        m.ledger = [LedgerEntry(kind: "commitment", text: "PR 올리기", owner: "화자 1", due: "금요일까지"),
                    LedgerEntry(kind: "commitment", text: "문구 공유", owner: "팀장")]
        m.questions = [SavedQuestion(id: "v1", question: "재시도는 몇 번까지 해요?", asker: nil, at: kst(14, 2),
                                     answer: "최대 2번이에요. [1]", cards: [], cited: [], unanswered: false, voice: "화자 2")]
        let md = m.markdown(calendar: cal)
        let lines = md.components(separatedBy: "\n")
        XCTAssertEqual(lines[2], "참석: 민재, 화자 2, 화자 3", md)
        XCTAssertTrue(md.contains("- **재시도는 몇 번까지 해요?** 최대 2번이에요. (화자 2)"), md)
        XCTAssertTrue(md.contains("- [ ] PR 올리기 (민재 · 금요일까지)"), md)
        XCTAssertTrue(md.contains("- [ ] 문구 공유 (팀장)"), md)
    }

    func testMarkdownOldMeetingHasNoAttendeeLine() {
        var m = meeting(kst(14), title: "결제 실패 재시도 정책")
        m.transcript = [line("e0:s1u1", kst(14, 1), "시작할게요")]
        m.ledger = [LedgerEntry(kind: "commitment", text: "PR 올리기", owner: "me")]
        m.questions = [SavedQuestion(id: "v1", question: "재시도는?", asker: "them", at: kst(14, 2),
                                     answer: "2번이에요.", cards: [], cited: [], unanswered: false)]
        let md = m.markdown(calendar: cal)
        XCTAssertFalse(md.contains("참석:"), md)
        XCTAssertTrue(md.contains("- **재시도는?** 2번이에요.\n"), md)
        XCTAssertTrue(md.contains("- [ ] PR 올리기 (나)"), md)
    }

    func testRenamedTrimsAndIgnoresBlank() {
        let m = meeting(kst(14), title: "결제 실패 재시도 정책")
        let r = m.renamed("  결제 정책 ")
        XCTAssertEqual(r.title, "결제 정책")
        XCTAssertTrue(r.titleEdited)
        XCTAssertEqual(m.renamed("   "), m)
    }

    func testApplyNotesKeepsEditedTitleAndChecks() {
        var m = meeting(kst(14), title: "내가 붙인 제목")
        m.titleEdited = true
        m.notes = .failed
        m.notesProblem = "인터넷 연결이 끊겼어요"
        m.ledger = [LedgerEntry(kind: "commitment", text: "PR 올리기", done: true)]
        m.apply(title: "새 제목", summary: ["요약 한 줄"],
                entries: [LedgerEntry(kind: "commitment", text: "PR 올리기"), LedgerEntry(kind: "commitment", text: "문구 공유")])
        XCTAssertEqual(m.title, "내가 붙인 제목")
        XCTAssertEqual(m.summary, ["요약 한 줄"])
        XCTAssertEqual(m.ledger.map(\.done), [true, false])
        XCTAssertEqual(m.notes, .done)
        XCTAssertNil(m.notesProblem)
        var fresh = meeting(kst(15))
        fresh.apply(title: "새 제목", summary: [], entries: [])
        XCTAssertEqual(fresh.title, "새 제목")
    }

    func testEmptyNotesDoNotWipeWrittenOnes() {
        var m = meeting(kst(14), title: "배포 회의")
        m.summary = ["목요일에 배포해요."]
        m.ledger = [LedgerEntry(kind: "decision", text: "목요일 배포")]
        m.notes = .waiting
        m.apply(title: "다른 제목", summary: [], entries: [])
        XCTAssertEqual(m.title, "배포 회의")
        XCTAssertEqual(m.summary, ["목요일에 배포해요."])
        XCTAssertEqual(m.ledger.map(\.text), ["목요일 배포"])
        XCTAssertEqual(m.notes, .done)
    }

    func testSelectionAfterDeleting() {
        XCTAssertEqual(MeetingGroups.selection(afterDeleting: "b", in: ["a", "b", "c"]), "c")
        XCTAssertEqual(MeetingGroups.selection(afterDeleting: "c", in: ["a", "b", "c"]), "b")
        XCTAssertNil(MeetingGroups.selection(afterDeleting: "a", in: ["a"]))
    }

    func testQuestionsKeptWithoutAConnectionHaveTheirOwnSection() {
        var m = SavedMeeting(mode: "inperson", start: kst(10, 0))
        m.questions = [
            SavedQuestion(id: "v1", question: "배포는 어떻게 해요?", asker: nil, at: kst(10, 1), answer: nil, cards: [], cited: [],
                          unanswered: true, noSource: true),
            SavedQuestion(id: "v2", question: "롤백은 어디서 해요?", asker: nil, at: kst(10, 2), answer: nil, cards: [], cited: [],
                          unanswered: true),
        ]
        let md = m.markdown(calendar: cal)
        XCTAssertTrue(md.contains("## 연결하면 답할 질문\n- 배포는 어떻게 해요?"), md)
        XCTAssertTrue(md.contains("## 문서로 남길 질문\n- 롤백은 어디서 해요?"), md)
    }

    // MARK: calendar link (docs/35 3-3)

    private func linked(meaningful: Bool, title: String = "주간 제품 회의") -> SavedMeeting {
        var m = SavedMeeting(mode: "remote", start: kst(14))
        m.link(MeetingEvent(id: "ev", title: title, start: kst(14), end: kst(15)), invited: [], meaningful: meaningful)
        return m
    }

    func testLinkSetsTitleOnlyWhenMeaningful() {
        XCTAssertEqual(linked(meaningful: true).title, "주간 제품 회의")
        XCTAssertNil(linked(meaningful: false, title: "Zoom 회의").title)
        XCTAssertEqual(linked(meaningful: false, title: "Zoom 회의").event?.title, "Zoom 회의")
    }

    func testUserTitleWinsOverEvent() {
        var m = SavedMeeting(mode: "remote", start: kst(14)).renamed("내 제목")
        m.link(MeetingEvent(id: "ev", title: "주간 제품 회의", start: kst(14), end: kst(15)), invited: [], meaningful: true)
        XCTAssertEqual(m.title, "내 제목")
    }

    func testNotesDoNotOverwriteCalendarTitle() {
        var m = linked(meaningful: true)
        m.apply(title: "모델 제목", summary: ["요약"], entries: [])
        XCTAssertEqual(m.title, "주간 제품 회의")
        XCTAssertEqual(m.summary, ["요약"])
    }

    func testNotesFillTitleWhenEventGeneric() {
        var m = linked(meaningful: false, title: "Zoom 회의")
        m.apply(title: "모델 제목", summary: ["요약"], entries: [])
        XCTAssertEqual(m.title, "모델 제목")
    }

    func testUnlinkClearsOnlyCalendarTitle() {
        var m = linked(meaningful: true)
        m.unlink()
        XCTAssertNil(m.title)
        XCTAssertNil(m.event)
        var n = linked(meaningful: false, title: "Zoom 회의")
        n.apply(title: "모델 제목", summary: ["요약"], entries: [])
        n.unlink()
        XCTAssertEqual(n.title, "모델 제목")
    }

    /// Review I1: the notes' title is kept aside, so taking the event off (or swapping in a generic one) brings it back.
    func testUnlinkBringsBackNotesTitle() {
        var m = linked(meaningful: true)
        m.apply(title: "로드맵 우선순위 결정", summary: ["요약"], entries: [])
        XCTAssertEqual(m.title, "주간 제품 회의")
        m.unlink()
        XCTAssertEqual(m.title, "로드맵 우선순위 결정")
        var n = linked(meaningful: true)
        n.apply(title: "로드맵 우선순위 결정", summary: ["요약"], entries: [])
        n.link(MeetingEvent(id: "z", title: "Zoom 회의", start: kst(14), end: kst(15)), invited: [], meaningful: false)
        XCTAssertEqual(n.title, "로드맵 우선순위 결정")
    }

    func testInvitedNamesSkipsEmailOnlyAndDuplicates() {
        var m = SavedMeeting(mode: "remote", start: kst(14))
        m.link(MeetingEvent(id: "ev", title: "회의", start: kst(14), end: kst(15)),
               invited: [MeetingInvitee(name: " 정하늘 ", email: "a@x.com"), MeetingInvitee(name: nil, email: "b@x.com"),
                         MeetingInvitee(name: "", email: "c@x.com"), MeetingInvitee(name: "정하늘", email: nil),
                         MeetingInvitee(name: "윤가람", email: nil)],
               meaningful: false)
        XCTAssertEqual(m.invitedNames, ["정하늘", "윤가람"])
        XCTAssertEqual(SavedMeeting(mode: "remote", start: kst(14)).invitedNames, [])
    }
}

final class NotesDecodingTests: XCTestCase {
    func testLedgerDecodesTitleSummaryOwnerDue() throws {
        let json = #"{"title": "결제 실패 재시도 정책", "summary": ["3번까지 다시 시도해요."], "items": [{"kind": "commitment", "text": "PR 올리기", "utterance_ids": ["s1u1"], "owner": "나", "due": "금요일까지"}], "dropped": 0}"#
        let l = try EngineClient.engineDecoder.decode(Ledger.self, from: Data(json.utf8))
        XCTAssertEqual(l.title, "결제 실패 재시도 정책")
        XCTAssertEqual(l.summary, ["3번까지 다시 시도해요."])
        XCTAssertEqual(l.items.first?.owner, "나")
        XCTAssertEqual(l.items.first?.due, "금요일까지")
        let old = try EngineClient.engineDecoder.decode(Ledger.self, from: Data(#"{"items": []}"#.utf8))
        XCTAssertNil(old.title)
        XCTAssertNil(old.summary)
        XCTAssertNil(old.topics)
        let topics = try EngineClient.engineDecoder.decode(Ledger.self, from: Data(
            #"{"items": [], "topics": [{"title": "재시도 횟수", "points": ["세 번까지로 했어요."]}]}"#.utf8))
        XCTAssertEqual(topics.topics, [NoteTopic(title: "재시도 횟수", points: ["세 번까지로 했어요."])])
    }

    func testLedgerEntryFromItemMapsOwner() {
        func item(_ owner: String?) -> LedgerItem {
            LedgerItem(kind: "commitment", text: "PR 올리기", utteranceIds: ["s1u1"], owner: owner, due: "금요일까지")
        }
        XCTAssertEqual(LedgerEntry(item: item("나")).owner, "me")
        XCTAssertEqual(LedgerEntry(item: item("상대")).owner, "them")
        XCTAssertNil(LedgerEntry(item: item(nil)).owner)
        XCTAssertEqual(LedgerEntry(item: item("화자 2")).owner, "화자 2")  // a speaker key stays a key
        XCTAssertEqual(LedgerEntry(item: item("팀장")).owner, "팀장")  // someone outside the meeting: kept as said
        XCTAssertEqual(LedgerEntry(item: item("나")).due, "금요일까지")
        XCTAssertFalse(LedgerEntry(item: item("나")).done)
    }
}

final class LateResultTests: XCTestCase {
    private let base = Date(timeIntervalSince1970: 1_792_000_000)

    private func q(_ id: String, _ offset: TimeInterval, answer: String? = nil) -> SavedQuestion {
        SavedQuestion(id: id, question: "질문 \(id)", asker: "them", at: base + offset, answer: answer, cards: [], cited: [], unanswered: false)
    }

    /// An answer that lands after the meeting ended replaces its question; a question caught at the very end joins in time order.
    func testSetQuestionReplacesByIdAndKeepsTimeOrder() {
        var m = SavedMeeting(mode: "remote", start: base)
        m.questions = [q("v1", 10), q("v3", 30)]
        m.setQuestion(q("v3", 30, answer: "늦게 온 답"))
        XCTAssertEqual(m.questions.map(\.answer), [nil, "늦게 온 답"])
        m.setQuestion(q("v2", 20))
        XCTAssertEqual(m.questions.map(\.id), ["v1", "v2", "v3"])
    }

    /// Meeting files keep what the page shows of a card, not the code it quotes.
    func testCardForRecordDropsCodeText() {
        let c = Card(id: "c1", path: "apps/api/a.ts", kind: "code", source: "repo", byline: nil, url: nil, lineStart: 3, lineEnd: 9,
                     title: "a.ts", snippet: "const SECRET_RULE = 1", score: 0.9, githubUrl: "https://github.com/x/y/blob/dev/a.ts#L3",
                     localPath: "/Users/me/widgets/apps/api/a.ts", chunk: 7, repeat: true)
        let r = c.forRecord
        XCTAssertEqual(r.snippet, "")
        XCTAssertNil(r.localPath)
        XCTAssertNil(r.score)
        XCTAssertEqual(r.name, c.name)
        XCTAssertEqual(r.link, c.link)
        XCTAssertEqual(r.chunk, 7)
        XCTAssertEqual(r.symbol, c.symbol)
    }
}
