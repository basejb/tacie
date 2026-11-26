import Foundation
import Observation

/// Remembered attendees, kept apart from the meetings in `~/Library/Application Support/Tacie/people`:
/// `people.json` holds the people, `meetings/<meetingId>.json` each meeting's voices until the meeting is deleted.
/// Meeting files keep the names given in them; forgetting people never touches those.
@Observable
@MainActor
final class VoiceVault {
    private(set) var people: [Person]
    private var root: URL
    private let enabled: () -> Bool
    /// How many people were ever made here. A new person's colour is this mod 6, so colours go round in order
    /// and a forgotten person's colour is not handed straight to the next one.
    private var created: Int
    /// Goes up whenever a meeting's voices or dismissed suggestions change on disk, so pages that read them look again.
    private(set) var revision = 0

    /// The remembered people of the account in use (AccountSpace).
    nonisolated static var standardRoot: URL { AccountSpace.inUse.people }

    /// 개인정보 > 참석자 기억하기. On unless the person turned it off.
    nonisolated static var enabled: Bool {
        UserDefaults.standard.object(forKey: "rememberPeople") as? Bool ?? true
    }

    init(root: URL = VoiceVault.standardRoot, enabled: @escaping () -> Bool = { VoiceVault.enabled }) {
        self.root = root
        self.enabled = enabled
        let url = root.appendingPathComponent("people.json")
        let file = Self.read(PeopleFile.self, at: url)
        if file == nil, FileManager.default.fileExists(atPath: url.path) { Self.keepAside(url) }
        people = file?.people ?? []
        created = file?.created ?? 0
    }

    /// Another account's people (AccountSpace): read from its folder; nothing of the previous one stays in memory.
    func reopen(root: URL) {
        let url = root.appendingPathComponent("people.json")
        let file = Self.read(PeopleFile.self, at: url)
        if file == nil, FileManager.default.fileExists(atPath: url.path) { Self.keepAside(url) }
        self.root = root
        people = file?.people ?? []
        created = file?.created ?? 0
        revision += 1
    }

    /// A people file that cannot be read goes to `people.json.bak` before anything writes a new one over it.
    private static func keepAside(_ url: URL) {
        let bak = url.appendingPathExtension("bak")
        do {
            if FileManager.default.fileExists(atPath: bak.path) { try FileManager.default.removeItem(at: bak) }
            try FileManager.default.moveItem(at: url, to: bak)
            Log.info("people file unreadable: kept as \(bak.lastPathComponent)")
        } catch {
            Log.info("people file unreadable, not moved: \(error.localizedDescription)")
        }
    }

    /// Something 기억한 참석자 모두 지우기 would delete: remembered people, or a meeting's voices kept so its speakers can
    /// still be named (kept for unnamed speakers too).
    var hasStoredData: Bool {
        if !people.isEmpty { return true }
        let dir = root.appendingPathComponent("meetings", isDirectory: true)
        let names = (try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []
        return names.contains { $0.hasSuffix(".json") }
    }

    // MARK: Meetings' voices

    /// Keeps a meeting's voices (replacing what was kept for it). Nothing is kept while remembering is off.
    func save(prints: [MeetingPrint], meeting: String) {
        guard enabled() else { return }
        var file = meetingFile(meeting) ?? MeetingFile(prints: [], absorbed: [:])
        file.prints = prints
        write(file, to: meetingURL(meeting))
        revision += 1
    }

    func prints(meeting: String) -> [MeetingPrint] { meetingFile(meeting)?.prints ?? [] }

    /// The suggestions (SpeakerSuggest.pair) dismissed with ✕ in this meeting.
    func dismissed(meeting: String) -> Set<String> { Set(meetingFile(meeting)?.dismissed ?? []) }

    /// ✕ on a suggestion: that speaker of the meeting is not suggested anyone again. Kept with the meeting's voices.
    func dismiss(_ pair: String, meeting: String) {
        guard enabled(), var file = meetingFile(meeting) else { return }
        file.dismissed = Array(Set(file.dismissed ?? []).union([pair])).sorted()
        write(file, to: meetingURL(meeting))
        revision += 1
    }

    func forget(meeting: String) {
        remove(meetingURL(meeting))
    }

    /// Every meeting's voices, for when every meeting is deleted. The people stay.
    func forgetMeetings() {
        remove(root.appendingPathComponent("meetings", isDirectory: true))
    }

    // MARK: People

    /// Remembers the speaker `key` of `meeting` (with any speakers merged into it) as `person`, or as a new person
    /// called `name`. Returns the person's id, or nil when remembering is off, there is no voice to remember, or `person`
    /// was forgotten (a link left from before is not a reason to make them again).
    /// A voice already taken into that person is not added again, and a meeting counts once.
    func remember(key: String, name: String, person: String?, meeting: SavedMeeting) -> String? {
        guard enabled() else { return nil }
        var file = meetingFile(meeting.id) ?? MeetingFile(prints: [], absorbed: [:])
        let target = meeting.resolved(key)
        let ours = file.prints.filter { meeting.resolved($0.key) == target }

        let id: String
        if let person, let i = people.firstIndex(where: { $0.id == person }) {
            id = person
            let taken = file.absorbed[id] ?? []
            let new = ours.filter { !taken.contains($0.key) }
            var p = people[i]
            if file.absorbed[id] == nil { p.meetings += 1 }
            if let v = Self.merge([(p.vector, p.seconds)] + new.map { ($0.vector, $0.seconds) }) { p.vector = v }
            p.seconds += new.reduce(0) { $0 + $1.seconds }
            p.last = max(p.last, meeting.start)
            people[i] = p
        } else if person != nil {
            return nil
        } else {
            let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !n.isEmpty, let v = Self.merge(ours.map { ($0.vector, $0.seconds) }) else { return nil }
            id = UUID().uuidString
            people.append(Person(id: id, name: n, vector: v, seconds: ours.reduce(0) { $0 + $1.seconds }, meetings: 1,
                                 last: meeting.start, color: created % SavedMeeting.speakerColors))
            created += 1
        }
        file.absorbed[id, default: []].formUnion(ours.map(\.key))
        write(file, to: meetingURL(meeting.id))
        writePeople()
        return id
    }

    /// The name used from the next meeting on. Past meetings keep theirs. A blank name changes nothing.
    func rename(person: String, to name: String) {
        let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !n.isEmpty, let i = people.firstIndex(where: { $0.id == person }) else { return }
        people[i].name = n
        writePeople()
    }

    func forget(person: String) {
        people.removeAll { $0.id == person }
        writePeople()
    }

    /// 기억한 참석자 모두 지우기: the whole folder, people and meetings' voices. False when something could not be deleted.
    @discardableResult
    func removeAll() -> Bool {
        do {
            if FileManager.default.fileExists(atPath: root.path) { try FileManager.default.removeItem(at: root) }
            people = []
            created = 0
            return true
        } catch {
            Log.info("people delete failed: \(error.localizedDescription)")
            let file = Self.read(PeopleFile.self, at: root.appendingPathComponent("people.json"))
            people = file?.people ?? []
            created = file?.created ?? 0
            return false
        }
    }

    // MARK: Undo

    /// The people and one meeting's voices as they are now, so a naming can be undone (되돌리기) with what it remembered.
    struct Snapshot {
        fileprivate let people: [Person]
        fileprivate let created: Int
        fileprivate let meeting: String
        fileprivate let file: MeetingFile?
    }

    func snapshot(meeting: String) -> Snapshot {
        Snapshot(people: people, created: created, meeting: meeting, file: meetingFile(meeting))
    }

    /// Puts the people and that meeting's voices back as they were. Nothing changes while remembering is off.
    func restore(_ s: Snapshot) {
        guard enabled() else { return }
        if people != s.people || created != s.created {
            people = s.people
            created = s.created
            writePeople()
        }
        if meetingFile(s.meeting) != s.file {
            if let f = s.file { write(f, to: meetingURL(s.meeting)) } else { remove(meetingURL(s.meeting)) }
            revision += 1
        }
    }

    // MARK: Files

    private struct PeopleFile: Codable {
        var created: Int
        var people: [Person]
    }

    fileprivate struct MeetingFile: Codable, Equatable {
        var prints: [MeetingPrint]
        var absorbed: [String: Set<String>]  // person id → the keys of this meeting already taken into them
        var dismissed: [String]? = nil  // SpeakerSuggest.pair of suggestions dismissed with ✕
    }

    /// Like MeetingStore: sorted keys, ISO 8601 dates. Not pretty-printed: a vector is 512 numbers.
    private static let encoder: JSONEncoder = {
        let e = JSONEncoder()
        e.dateEncodingStrategy = .iso8601
        e.outputFormatting = [.sortedKeys]
        return e
    }()

    private static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .iso8601
        return d
    }()

    /// The voices weighted by seconds, then made length 1. Nil when there is nothing to merge.
    nonisolated static func merge(_ parts: [([Float], Float)]) -> [Float]? {
        let parts = parts.filter { !$0.0.isEmpty && $0.1 > 0 }
        guard let n = parts.first?.0.count else { return nil }
        var sum = [Float](repeating: 0, count: n)
        for (v, s) in parts where v.count == n {
            for i in 0..<n { sum[i] += v[i] * s }
        }
        let norm = sum.reduce(0) { $0 + $1 * $1 }.squareRoot()
        guard norm > 0 else { return nil }
        return sum.map { $0 / norm }
    }

    private static func read<T: Decodable>(_ type: T.Type, at url: URL) -> T? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        guard let value = try? decoder.decode(type, from: data) else {
            Log.info("people file skipped: \(url.lastPathComponent)")
            return nil
        }
        return value
    }

    private func meetingURL(_ id: String) -> URL { root.appendingPathComponent("meetings/\(id).json") }

    private func meetingFile(_ id: String) -> MeetingFile? { Self.read(MeetingFile.self, at: meetingURL(id)) }

    private func writePeople() {
        write(PeopleFile(created: created, people: people), to: root.appendingPathComponent("people.json"))
    }

    private func write<T: Encodable>(_ value: T, to url: URL) {
        do {
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Self.encoder.encode(value).write(to: url, options: .atomic)
        } catch {
            Log.info("people write failed: \(error.localizedDescription)")
        }
    }

    private func remove(_ url: URL) {
        do { try FileManager.default.removeItem(at: url) } catch CocoaError.fileNoSuchFile {} catch {
            Log.info("people delete failed: \(error.localizedDescription)")
        }
    }
}
