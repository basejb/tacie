import Foundation

/// One meeting as kept on this Mac (design notes §5-3):
/// what was said, what was asked and found, and the notes made at the end. Never the sound.
struct SavedMeeting: Codable, Identifiable, Equatable {
    /// The summary, title and ledger made when the meeting ended.
    enum Notes: String, Codable { case waiting, done, failed, off }

    var version = 1
    let id: String
    let mode: String  // MeetingMode.rawValue: "inperson" | "remote"
    let start: Date
    var end: Date?  // nil while the meeting goes on (or the app died during it)
    var title: String?
    var titleEdited = false
    var notes: Notes = .waiting
    var notesProblem: String?  // why the notes failed, in the island's words
    /// The notes failed at the month's Tacie AI limit (NotesLimit.swift); nil for any other reason and in older files.
    var notesLimit: NotesLimit? = nil
    var summary: [String] = []
    /// The talk topic by topic; nil in meetings whose notes came before topics (their summary is three short lines).
    var topics: [NoteTopic]? = nil
    var ledger: [LedgerEntry] = []
    /// The kind the notes were written for; nil in older files and before the notes (read as a meeting).
    var kind: NotesKind? = nil
    /// "auto" when the engine picked the kind, "user" when the person did.
    var kindSource: String? = nil
    var questions: [SavedQuestion] = []
    var transcript: [TranscriptLine] = []
    /// Names given to the engine's speakers in this meeting; nil in meetings saved before speakers (MeetingVoices.swift).
    var voices: [VoiceEntry]? = nil
    /// Pinned to the top of the main window's list; nil (not false) when not, so older files read the same.
    var pinned: Bool? = nil
    /// Whether the sound of this meeting is kept; nil when it was not recorded (and in older files).
    var recording: Recording? = nil
    var recordingSeconds: Double? = nil
    /// Stretches of the recording whose utterance could not be transcribed.
    var unheard: [AudioSpan]? = nil
    /// Development build only: what the Soniox shadow transcription heard per utterance (docs/25 "그림자 모드").
    var shadow: [ShadowResult]? = nil
    /// The id of the meeting this one goes on from (a prepared meeting that began within two hours, docs/36 1-5절);
    /// "다음 회의 준비" reads the chain back along it. nil when not linked and in older files.
    var linkedFrom: String? = nil
    /// What the prep answer listed (아직 안 정한 것, 안 끝난 할 일) when this meeting was linked, and after its notes
    /// whether this meeting settled each (docs/36 1-6절); nil when not linked and in older files.
    var prepared: [PreparedItem]? = nil

    /// 연결 끊기: the link and the prepared items under it go together.
    mutating func cutLink() {
        linkedFrom = nil
        prepared = nil
    }

    /// The calendar event this meeting was linked to, and who it invited (docs/35 3-3); nil when none or in older files.
    var event: MeetingEvent? = nil
    var invited: [MeetingInvitee]? = nil
    /// The notes' own title, kept while a calendar title shows, so taking the event off brings it back.
    var notesTitle: String? = nil
    /// Each time the meeting was taken up again after it ended (이어서 듣기, docs/35 5절); nil in meetings never resumed.
    var resumes: [MeetingResume]? = nil
    /// "iphone" on a Mac copy of an iPhone meeting (the only files that talk to the server, MeetingSync.swift); nil for
    /// the device's own meetings and in older files.
    var source: String? = nil
    /// A Mac copy of an iPhone meeting: notes only, its transcript and recording stay on the iPhone.
    var fromIPhone: Bool { source == "iphone" }
    /// When each part of the notes was last edited, for merging with the server (SyncStamps); nil when never synced.
    var sync: SyncStamps? = nil

    /// The sound is gone: its word places and unheard stretches go with it, the text stays.
    mutating func dropRecording() {
        recording = .deleted
        unheard = nil
        for i in transcript.indices { transcript[i].words = nil }
    }

    /// Links the event. Its title becomes the meeting's when it says something ("Zoom 회의" does not) and the person
    /// has not named the meeting.
    mutating func link(_ event: MeetingEvent, invited: [MeetingInvitee], meaningful: Bool) {
        if !titleEdited, calendarTitled { title = notesTitle }
        self.event = event
        self.invited = invited
        if meaningful, !titleEdited { title = event.title }
    }

    /// Takes the event off; its title goes with it unless the person chose that title.
    mutating func unlink() {
        if !titleEdited, calendarTitled { title = notesTitle }
        event = nil
        invited = nil
    }

    /// The meeting's title is the linked event's (so the notes' title does not replace it).
    var calendarTitled: Bool { event.map { title == $0.title } ?? false }

    /// The invited people's names, trimmed, each once; people known only by email are left out.
    var invitedNames: [String] {
        var seen = Set<String>()
        return (invited ?? []).compactMap { $0.name?.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty && seen.insert($0).inserted }
    }

    /// The kind to show: a meeting unless the notes say otherwise.
    var shownKind: NotesKind { kind ?? .meeting }

    init(mode: String, start: Date) {
        self.id = Self.id(for: start)
        self.mode = mode
        self.start = start
    }

    /// The file name and the window's handle on a meeting: when it started, to the second.
    static func id(for start: Date, in zone: TimeZone = .current) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = zone
        f.dateFormat = "yyyyMMdd-HHmmss"
        return f.string(from: start)
    }
}

/// One item of a prep answer carried into the next meeting: `decided` and `how` stay nil until the notes are judged.
struct PreparedItem: Codable, Equatable {
    let text: String
    let kind: String
    var decided: Bool?
    var how: String?

    /// The 준비한 것은 어떻게 됐나요 block shows once the items were judged; a failed request leaves them unjudged and it hides.
    static func shows(_ items: [PreparedItem]?) -> Bool { items?.contains { $0.decided != nil } == true }
}

/// Whether a meeting's recording is on this Mac or was deleted afterwards.
enum Recording: String, Codable { case kept, deleted }

/// A stretch of the recording, in milliseconds from its start.
struct AudioSpan: Codable, Equatable {
    let start: Int
    let end: Int
}

/// One word and where it sits in the recording (milliseconds from its start).
struct SpokenWord: Codable, Equatable {
    let text: String
    let start: Int
    let end: Int

    /// The engine's words (seconds from the utterance's own WAV) placed in the whole recording.
    static func placed(_ words: [HeardWord], at startMs: Int) -> [SpokenWord] {
        words.map { SpokenWord(text: $0.text, start: startMs + Int(($0.start * 1000).rounded()), end: startMs + Int(($0.end * 1000).rounded())) }
    }
}

/// One item of the meeting's ledger: kind is decision, commitment or open_question (engine/crates/tacie-engine/src/notes.rs).
struct LedgerEntry: Codable, Equatable {
    let kind: String
    var text: String
    var owner: String? = nil  // commitments: a speaker key "화자 N", a name said for someone not in the meeting, nil; "me"/"them" in older files
    var due: String? = nil  // as said: "금요일까지"
    var done = false
    var doneAt: Date? = nil  // when it was checked, for 끝낸 일's order; nil in files from before, or unchecked
    /// Changed or added by the person on the meeting page: 다시 쓰기 keeps it. nil for the notes' own.
    var edited: Bool? = nil
    /// Keys of the transcript lines the notes wrote it from ("e0:s1u3"), for 대화 기록에서 보기; nil when added by hand or
    /// in files from before (docs/24 2절 2번).
    var refs: [String]? = nil
    /// A decision only: the past meeting's decision this one changed (docs/30 8절). nil when none, or in older files.
    var replaces: ReplacedDecision? = nil
    /// Its handle across devices, for merging edits entry by entry (MeetingSync.swift); nil in older files until
    /// `SavedMeeting.ensureLedgerIds` gives one.
    var id: String? = nil
}

/// A past meeting's decision that a later decision changed: its words as that meeting's notes have them (speakers as
/// names), the meeting to open, and when it started.
struct ReplacedDecision: Codable, Equatable {
    let text: String
    let meeting: String
    let title: String
    let at: Date
}

extension LedgerEntry {
    /// From the engine's item: an older engine's "나"/"상대" become "me"/"them"; a speaker key or a name stays as it is.
    init(item: LedgerItem) {
        let owner = item.owner == "나" ? "me" : item.owner == "상대" ? "them" : item.owner
        self.init(kind: item.kind, text: item.text, owner: owner, due: item.due)
    }
}

extension Card {
    /// What a meeting file keeps of a card: what the page shows and where a click goes, not the code it quotes
    /// or where the checkout lives on this Mac (spec §5-3: name, path, URL).
    var forRecord: Card {
        Card(id: id, path: path, kind: kind, source: source, byline: byline, url: url, lineStart: lineStart, lineEnd: lineEnd,
             title: title, snippet: "", score: nil, githubUrl: githubUrl, localPath: nil, chunk: chunk, repeat: nil)
    }
}

/// A question the meeting asked and what Tacie found for it.
struct SavedQuestion: Codable, Equatable, Identifiable {
    let id: String
    let question: String
    let asker: String?  // "me", "them", "typed"; nil in person (one microphone hears everyone)
    let at: Date
    let answer: String?
    let cards: [Card]
    let cited: [Int]
    let unanswered: Bool
    /// Heard while nothing was connected, so not searched: asked again once a service is (docs/11 4-1절 3번).
    var noSource: Bool? = nil
    /// Who asked, as the engine's speaker key ("화자 2"); nil before speakers or when typed.
    var voice: String? = nil
    /// The sources it was searched with (ResearchRules.keys); nil in records from before docs/11 14절.
    var searchedWith: [String]? = nil
    /// What a read of the meeting made of it once nothing was found (docs/24 4절); nil before that read or when it failed.
    var review: QuestionReview? = nil
    /// iPhone: the past meetings the answer stood on (지난 회의 묻기, docs/27 8-7); nil on the Mac.
    var pastMeetings: [PastMeetingRef]? = nil

    /// 문서로 남길 질문: no source answered it, and the read did not set it aside as not asking for a fact.
    var toDocument: Bool { unanswered && noSource != true && review?.verdict != QuestionReview.notQuestion }
    /// The question as the list shows it: rewritten to read on its own, when the read did.
    var shownQuestion: String { review?.question ?? question }
}

/// A past meeting an iPhone answer stood on: its id and title when it was cited.
struct PastMeetingRef: Codable, Equatable {
    let id: String
    let title: String
    /// When it started (ISO 8601), to tell meetings with the same title apart.
    var start: String? = nil
}

/// A question no source answered, read against the whole meeting (`POST /questions/review`, docs/24 4절 1–2번).
struct QuestionReview: Codable, Equatable {
    static let answered = "answered", doc = "doc", notQuestion = "not_question"
    /// answered (someone answered it in the meeting), doc (still open), not_question (small talk, an opinion asked, …)
    let verdict: String
    /// The question rewritten to read on its own; nil keeps the words as heard.
    var question: String?
    /// The answer said in the meeting, with who said it as `[[key]]` like the summary; answered only.
    var answer: String?
    /// Transcript keys of the lines with that answer.
    var refs: [String]?
}

/// One transcribed utterance. `key` is "e<epoch>:<utterance id>": a restarted engine counts from s1u1 again.
struct TranscriptLine: Codable, Equatable {
    let key: String
    let at: Date
    let speaker: String?  // "me", "them", nil: the channel; not shown since speakers
    /// The words as shown. The panel's 잘못 들린 말 고치기 may change them (docs/38 12절); `heard` keeps what was transcribed.
    var text: String
    var question: Bool
    var voice: String? = nil  // the engine's speaker key, "화자 2"; nil before speakers or without the voice model
    var name: String? = nil  // a name given to this line only (이 줄만)
    var audioAt: Int? = nil  // where the utterance starts in the recording (ms); nil when not recorded
    var words: [SpokenWord]? = nil  // its words in the recording; nil without timing or after the recording is deleted
    var translation: String? = nil  // in the app's language when said in another one (docs/30, dev build only for now)
    var translatedFrom: String? = nil  // the language it was said in ("en")
    /// The words as first transcribed, once a fix changed `text`; nil when never fixed (and in older files).
    var heard: String? = nil

    /// Only sound tags and no words ("[pause]", "[배경 소음] [silence]"): the engine's `skipped: "sound"`
    /// (`notes.rs` `is_sound_only`). Not shown in 대화 기록; meetings saved before 2026-11-10 still hold such lines.
    var isSoundOnly: Bool {
        guard text.contains("[") else { return false }
        let rest = text.replacingOccurrences(of: #"\[[^\]]*\]"#, with: " ", options: .regularExpression)
        return !rest.contains { $0.isLetter || $0.isNumber }
    }
}

/// A speaker of one meeting and what the person called them. `same` points at the key it was merged into.
struct VoiceEntry: Codable, Equatable {
    let key: String
    var name: String?
    var person: String?  // the remembered person this speaker is, when known
    var same: String?
    /// True when the app joined this speaker to `same` on its own, for being heard only briefly (`SmallSpeakers`):
    /// it lets go again when that no longer holds. A merge made by hand has none.
    var auto: Bool?
}

/// How long meetings stay on this Mac. Chosen in 개인정보; kept until deleted by default.
enum Retention: String, CaseIterable {
    case days30, days90, year, forever

    var days: Int? {
        switch self {
        case .days30: 30
        case .days90: 90
        case .year: 365
        case .forever: nil
        }
    }
}

extension SavedMeeting {
    /// The transcript keys of the lines the engine cites. A meeting's notes cite "s1u3" (the session's own ids) and notes
    /// written again cite the key the app sent ("e0:s1u3"). A bare id is a line of the last engine run only ("e<last>:"):
    /// a restarted engine counts from s1u1 again and the notes come from the session going on at the end, so an id
    /// it never reached is not an older run's line. Lines not in the transcript (joined into a later one) are left out;
    /// nil when none is left.
    func lineKeys(_ ids: [String]) -> [String]? {
        let last = transcript.last.flatMap { $0.key.split(separator: ":", maxSplits: 1).first }.map(String.init)
        let keys = ids.compactMap { id -> String? in
            if id.contains(":") { return transcript.contains { $0.key == id } ? id : nil }
            guard let last, transcript.contains(where: { $0.key == "\(last):\(id)" }) else { return nil }
            return "\(last):\(id)"
        }
        return keys.isEmpty ? nil : keys
    }

    /// The first transcript line a ledger entry was written from, for its 대화 기록에서 보기 chip.
    func sourceLine(_ entry: LedgerEntry) -> TranscriptLine? {
        guard let refs = entry.refs else { return nil }
        return transcript.filter { refs.contains($0.key) }.min { $0.at < $1.at }
    }
}

extension SavedMeeting {
    /// The transcript line a question was heard in: the line marked as a question nearest to when it was asked (within a
    /// minute); nil for a typed question or one whose line is gone.
    func questionLine(_ q: SavedQuestion) -> TranscriptLine? {
        transcript.filter { $0.question && abs($0.at.timeIntervalSince(q.at)) < 60 }
            .min { abs($0.at.timeIntervalSince(q.at)) < abs($1.at.timeIntervalSince(q.at)) }
    }
}

/// The calendar event a meeting was linked to (docs/35 3-3): enough to show it and to tell it apart, never its notes.
struct MeetingEvent: Codable, Equatable {
    var id: String
    var title: String
    var start: Date
    var end: Date
}

/// Someone the linked event invited. Only the name leaves this Mac (as a speech-recognition term); the email stays here.
struct MeetingInvitee: Codable, Equatable {
    var name: String?
    var email: String?
}

/// A gap in a meeting: it ended (by itself or by hand) at `stopped` and went on at `resumed`.
struct MeetingResume: Codable, Equatable {
    var stopped: Date
    var resumed: Date
}

extension SavedMeeting {
    /// How long after it ended a meeting can still be taken up again.
    static let resumeWindow: TimeInterval = 30 * 60

    /// An ended meeting less than `resumeWindow` ago (the page's 이어서 듣기). Never a Mac copy of an iPhone meeting:
    /// what it would go on from is on the iPhone.
    func resumable(now: Date) -> Bool {
        guard let end, !fromIPhone else { return false }
        return now.timeIntervalSince(end) >= 0 && now.timeIntervalSince(end) < Self.resumeWindow
    }

    /// Takes the ended meeting up again at `at`: it is going on (no end) and the gap is kept for the transcript and its length.
    mutating func reopen(at: Date) {
        guard let end else { return }
        resumes = (resumes ?? []) + [MeetingResume(stopped: end, resumed: at)]
        self.end = nil
    }

    /// The engine-session number for lines heard after a resume: one past every number the transcript used, so the
    /// new session's "s1u1" never meets an old line's key.
    var nextEpoch: Int {
        let used = transcript.compactMap { l -> Int? in
            guard l.key.hasPrefix("e"), let colon = l.key.firstIndex(of: ":") else { return nil }
            return Int(l.key[l.key.index(after: l.key.startIndex)..<colon])
        }
        return (used.max() ?? -1) + 1
    }

    /// Seconds the meeting was listened to: start to end (or `now`) less the gaps.
    func listenedSeconds(now: Date = Date()) -> TimeInterval {
        let gaps = (resumes ?? []).reduce(0) { $0 + max(0, $1.resumed.timeIntervalSince($1.stopped)) }
        return max(0, (end ?? now).timeIntervalSince(start) - gaps)
    }
}

