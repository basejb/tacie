import Foundation

/// When each editable part of an iPhone meeting was last changed, by the device that changed it
/// (design notes 3절). Top fields (`title`, `summary`, `topics`,
/// `kind`, `pinned`) have one stamp each; ledger entries one per id, and a removed entry a stamp in `ledgerGone`.
/// Stamps are UTC ISO 8601 strings with milliseconds, compared as strings like the server does.
/// On the wire and in the file the top fields sit flat beside `ledger` and `ledgerGone`, as the server keeps them.
struct SyncStamps: Codable, Equatable {
    var fields: [String: String] = [:]
    var ledger: [String: String] = [:]
    var ledgerGone: [String: String] = [:]

    /// The fields that carry a stamp, as the server's `meeting-merge.js` names them.
    static let topFields = ["title", "summary", "topics", "kind", "pinned"]

    private static let formatter: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        f.timeZone = TimeZone(identifier: "UTC")
        return f
    }()

    /// "2026-11-16T01:00:00.000Z", the same as JavaScript's `toISOString`.
    static func string(from date: Date) -> String { formatter.string(from: date) }

    /// The stamps after an edit from `old` to `new` at `at`: every top field that differs, every ledger entry that is new
    /// or differs (its `replaces` and `refs` stay on the device, so they do not count), and each id `new` no longer has.
    /// Ledger ids must be there already (`ensureLedgerIds`); an entry without one is not stamped.
    static func touch(old: SavedMeeting, new: SavedMeeting, at: Date) -> SyncStamps {
        var s = old.sync ?? SyncStamps()
        let now = string(from: at)
        if old.title != new.title || old.titleEdited != new.titleEdited { s.fields["title"] = now }
        if old.summary != new.summary { s.fields["summary"] = now }
        if old.topics != new.topics { s.fields["topics"] = now }
        if old.kind != new.kind || old.kindSource != new.kindSource { s.fields["kind"] = now }
        if (old.pinned ?? false) != (new.pinned ?? false) { s.fields["pinned"] = now }
        let before = Dictionary(old.ledger.compactMap { e in e.id.map { ($0, e.synced) } }, uniquingKeysWith: { a, _ in a })
        for e in new.ledger {
            guard let id = e.id else { continue }
            if before[id] != e.synced { s.ledger[id] = now }
        }
        let here = Set(new.ledger.compactMap(\.id))
        for id in before.keys where !here.contains(id) { s.ledgerGone[id] = now }
        return s
    }

    /// The stamps after the notes were written anew (다시 쓰기): what `touch` finds, and then every top field and every
    /// ledger id stamped `at`, so the new notes win over older edits made elsewhere (spec 4-4).
    static func rewritten(old: SavedMeeting, new: SavedMeeting, at: Date) -> SyncStamps {
        var s = touch(old: old, new: new, at: at)
        let now = string(from: at)
        for f in topFields { s.fields[f] = now }
        for id in new.ledger.compactMap(\.id) { s.ledger[id] = now }
        return s
    }

    /// The latest stamp of all, "" when there is none.
    var latest: String { (Array(fields.values) + Array(ledger.values) + Array(ledgerGone.values)).max() ?? "" }

    /// Whether any stamp here is later than the same one in `other`: an edit `other` has not seen yet.
    func newer(than other: SyncStamps) -> Bool {
        fields.contains { $0.value > (other.fields[$0.key] ?? "") }
            || ledger.contains { $0.value > (other.ledger[$0.key] ?? "") }
            || ledgerGone.contains { $0.value > (other.ledgerGone[$0.key] ?? "") }
    }

    private struct Key: CodingKey {
        var stringValue: String
        var intValue: Int? { nil }
        init(_ s: String) { stringValue = s }
        init?(stringValue: String) { self.stringValue = stringValue }
        init?(intValue: Int) { nil }
    }

    init(fields: [String: String] = [:], ledger: [String: String] = [:], ledgerGone: [String: String] = [:]) {
        self.fields = fields
        self.ledger = ledger
        self.ledgerGone = ledgerGone
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        for key in c.allKeys {
            switch key.stringValue {
            case "ledger": ledger = (try? c.decode([String: String].self, forKey: key)) ?? [:]
            case "ledgerGone": ledgerGone = (try? c.decode([String: String].self, forKey: key)) ?? [:]
            default: if let v = try? c.decode(String.self, forKey: key) { fields[key.stringValue] = v }
            }
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Key.self)
        for (k, v) in fields where k != "ledger" && k != "ledgerGone" { try c.encode(v, forKey: Key(k)) }
        try c.encode(ledger, forKey: Key("ledger"))
        try c.encode(ledgerGone, forKey: Key("ledgerGone"))
    }
}

extension LedgerEntry {
    /// The entry as it goes to the server: without the other meeting's decision it changed or this device's line keys.
    var synced: LedgerEntry {
        var e = self
        e.replaces = nil
        e.refs = nil
        return e
    }
}

extension SavedMeeting {
    /// Gives each ledger entry without an id (older files, fresh notes) one of its own.
    mutating func ensureLedgerIds() {
        for i in ledger.indices where ledger[i].id == nil { ledger[i].id = UUID().uuidString.lowercased() }
    }

    /// This copy (the file as it is now) with only what `edit` changed from `base` (the copy the edit started from) put on
    /// it: a page holding an older copy while a merge from the server landed must not write the older values back, or
    /// they would be stamped as new edits and undo the other device's. Ledger entries go by id; one `edit` removed is
    /// removed, one it added or changed is put in (added last when this copy no longer has it). All three must have
    /// their ledger ids already.
    func rebased(_ edit: SavedMeeting, from base: SavedMeeting) -> SavedMeeting {
        var m = self
        if base.title != edit.title || base.titleEdited != edit.titleEdited {
            m.title = edit.title
            m.titleEdited = edit.titleEdited
        }
        if base.summary != edit.summary { m.summary = edit.summary }
        if base.topics != edit.topics { m.topics = edit.topics }
        if base.kind != edit.kind || base.kindSource != edit.kindSource {
            m.kind = edit.kind
            m.kindSource = edit.kindSource
        }
        if base.pinned != edit.pinned { m.pinned = edit.pinned }
        let before = Dictionary(base.ledger.compactMap { e in e.id.map { ($0, e) } }, uniquingKeysWith: { a, _ in a })
        let after = Set(edit.ledger.compactMap(\.id))
        m.ledger.removeAll { e in e.id.map { before[$0] != nil && !after.contains($0) } ?? false }
        for e in edit.ledger {
            guard let id = e.id, before[id] != e else { continue }
            if let i = m.ledger.firstIndex(where: { $0.id == id }) { m.ledger[i] = e } else { m.ledger.append(e) }
        }
        return m
    }

    /// An iPhone meeting arriving from the server, before anything else is known of it.
    init(id: String, mode: String, start: Date) {
        self.id = id
        self.mode = mode
        self.start = start
    }
}
