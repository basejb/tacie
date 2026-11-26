import Foundation

/// A change the panel's AI proposed for the meeting on the page (docs/38 12절). The engine only proposes; the app
/// changes its meeting file when the person applies the card. `fix_words` puts the meant words in the places the heard
/// ones were found; `edit_ledger` changes and adds ledger entries.
struct PanelProposal: Codable, Equatable, Identifiable {
    static let fixWords = "fix_words", editLedger = "edit_ledger", nameSpeaker = "name_speaker", mergeSpeakers = "merge_speakers"
    static let moveLines = "move_lines", rewriteNotes = "rewrite_notes", changeKind = "change_kind", renameTitle = "rename_title"
    static let assignLines = "assign_lines"

    /// assign_lines: one line, the speaker it shows now, and the speaker the context says said it.
    struct Assigned: Codable, Equatable {
        let key: String
        var speaker: String? = nil
        var text: String? = nil
        let to: Speaker
    }

    /// move_lines: a line to move, as the engine read it.
    struct Moved: Codable, Equatable {
        let key: String
        var speaker: String? = nil
        var text: String? = nil
    }

    /// move_lines: whose words they are; `key` when one of the meeting's speakers, nil for a new name.
    struct Target: Codable, Equatable {
        var key: String? = nil
        let name: String
    }

    /// A speaker of the meeting: its key ("화자 3"), the name it showed when proposed, its transcript lines.
    struct Speaker: Codable, Equatable {
        let key: String
        let name: String
        var lines: Int? = nil
    }

    /// Where a place is: a transcript line (`line`), the title, a summary line, a topic's title or one of its points, or
    /// a ledger entry's words.
    struct At: Codable, Equatable, Hashable {
        var line: String? = nil
        var title: Bool? = nil
        var summary: Int? = nil
        var topic: Int? = nil
        var point: Int? = nil
        var ledger: Int? = nil

        var inTranscript: Bool { line != nil }
    }

    /// One place `word` was found: the `nth` time (from 0) these exact characters come in that text. `to` is what goes
    /// there, with the middle dot the record already uses.
    struct Place: Codable, Equatable {
        let at: At
        let nth: Int
        let before: String
        let word: String
        let after: String
        let to: String
    }

    /// A change to the ledger entry at `index`, made only while its words are still `was`.
    struct Change: Codable, Equatable {
        let index: Int
        let was: String
        var text: String? = nil
        var owner: String? = nil
        var due: String? = nil
        var done: Bool? = nil
        var remove: Bool? = nil
    }

    /// A new ledger entry (a to-do).
    struct Add: Codable, Equatable {
        let kind: String
        let text: String
        var owner: String? = nil
        var due: String? = nil
    }

    let id: String
    let kind: String
    let meetingId: String
    var heard: String? = nil
    var meant: String? = nil
    var places: [Place]? = nil
    var changes: [Change]? = nil
    var add: [Add]? = nil
    /// name_speaker: the speaker as the transcript shows it ("화자 1", or the name it goes by) and the name to give.
    var speaker: String? = nil
    var name: String? = nil
    /// name_speaker: the speaker's key ("화자 1").
    var key: String? = nil
    /// name_speaker: how many transcript lines the speaker has; merge_speakers: how many move.
    var lines: Int? = nil
    /// merge_speakers: the speakers that go, and the one they become.
    var from: [Speaker]? = nil
    var into: Speaker? = nil
    var moved: [Moved]? = nil
    var assigned: [Assigned]? = nil
    var target: Target? = nil
    /// rewrite_notes: what the person asked the notes to be ("요약을 더 자세히").
    var request: String? = nil
    /// change_kind: the new kind (`NotesKind` raw value).
    var notesKind: String? = nil
    /// rename_title: the new title and the one it had.
    var title: String? = nil
    var was: String? = nil
}

/// What the person chose on a proposal's card, kept with the turn so a reopened conversation shows it.
enum ProposalState: String, Codable, Equatable {
    case applied, undone, dismissed
}

/// What applying a proposal changed, kept to put it back (되돌리기): each field as it was before and as the apply left it.
/// A field changed again since (the person edited it, notes written again) is not put back.
struct ProposalUndo: Equatable {
    struct Line: Equatable {
        let key: String
        let text: String
        let heard: String?
        let after: String
    }
    var lines: [Line] = []
    var notes: Notes?
    var notesAfter: Notes?
    /// Notes were written again after the fix: the notes from before the fix come back with the lines.
    var rewrote = false
    /// name_speaker: the speakers' names before and after, put back only while nothing renamed them since.
    var naming: NamingChange?

    struct NamingChange: Equatable {
        let before: SavedMeeting.Naming
        let after: SavedMeeting.Naming
    }

    struct Notes: Equatable {
        var title: String?
        var titleEdited: Bool
        var kind: NotesKind?
        var kindSource: String?
        var notesTitle: String?
        var state: SavedMeeting.Notes
        var summary: [String]
        var topics: [NoteTopic]?
        var ledger: [LedgerEntry]

        init(_ m: SavedMeeting) {
            title = m.title
            titleEdited = m.titleEdited
            kind = m.kind
            kindSource = m.kindSource
            notesTitle = m.notesTitle
            state = m.notes
            summary = m.summary
            topics = m.topics
            ledger = m.ledger
        }

        func put(_ m: inout SavedMeeting) {
            m.title = title
            m.titleEdited = titleEdited
            m.kind = kind
            m.kindSource = kindSource
            m.notesTitle = notesTitle
            m.notes = state
            m.notesProblem = nil
            m.summary = summary
            m.topics = topics
            m.ledger = ledger
        }
    }
}

enum ProposalApply {
    /// Middle dots the transcript and the notes write differently ("경․경매", "경·경매"): the engine took them as one.
    static let dots: Set<Character> = ["·", "․", "‧", "・", "･"]

    /// Where in `text` the `nth` (from 0) non-overlapping `word` starts, or nil when it is not there that many times.
    static func range(of word: String, nth: Int, in text: String) -> Range<String.Index>? {
        guard !word.isEmpty else { return nil }
        var from = text.startIndex
        var n = 0
        while let r = text.range(of: word, range: from..<text.endIndex) {
            if n == nth { return r }
            n += 1
            from = r.upperBound
        }
        return nil
    }

    /// `text` with the places' words replaced, the later ones first so the earlier turns still count right. nil when a
    /// place is not there any more (the text changed since the engine read it): then nothing in it changes.
    static func replaced(_ text: String, _ places: [Place]) -> String? {
        var ranges: [(Range<String.Index>, String)] = []
        for p in places {
            guard let r = range(of: p.word, nth: p.nth, in: text) else { return nil }
            ranges.append((r, p.to))
        }
        var out = text
        for (r, to) in ranges.sorted(by: { $0.0.lowerBound > $1.0.lowerBound }) { out.replaceSubrange(r, with: to) }
        return out
    }

    typealias Place = PanelProposal.Place

    /// Puts the chosen places of a fix into the meeting. Notes texts keep `[[화자 2]]` marks, where the engine read names:
    /// a text whose word count differs between the two is left as it is. Returns what changed (for undo) and how many
    /// places were changed.
    static func fix(_ m: inout SavedMeeting, places: [Place]) -> (undo: ProposalUndo, changed: Int) {
        var undo = ProposalUndo()
        let notesBefore = ProposalUndo.Notes(m)
        var changed = 0
        var byText: [PanelProposal.At: [Place]] = [:]
        for p in places { byText[p.at, default: []].append(p) }

        for (at, ps) in byText {
            if let key = at.line {
                guard let i = m.transcript.firstIndex(where: { $0.key == key }),
                      let new = replaced(m.transcript[i].text, ps) else { continue }
                let l = m.transcript[i]
                undo.lines.append(.init(key: key, text: l.text, heard: l.heard, after: new))
                m.transcript[i].heard = l.heard ?? l.text
                m.transcript[i].text = new
                changed += ps.count
                continue
            }
            let names = m
            // the engine counted in the text as shown (names for marks); the raw text must hold the same count
            func edit(_ raw: String) -> String? {
                let same = ps.allSatisfy { count(of: $0.word, in: raw) == count(of: $0.word, in: names.shownRefs(raw)) }
                return same ? replaced(raw, ps) : nil
            }
            if at.title == true, let t = m.title, let new = edit(t) {
                m.title = new
                m.titleEdited = true
            } else if let i = at.summary, m.summary.indices.contains(i), let new = edit(m.summary[i]) {
                m.summary[i] = new
            } else if let i = at.topic, var topics = m.topics, topics.indices.contains(i) {
                let t = topics[i]
                if let j = at.point {
                    guard t.points.indices.contains(j), let new = edit(t.points[j]) else { continue }
                    var points = t.points
                    points[j] = new
                    topics[i] = NoteTopic(title: t.title, points: points)
                } else {
                    guard let new = edit(t.title) else { continue }
                    topics[i] = NoteTopic(title: new, points: t.points)
                }
                m.topics = topics
            } else if let i = at.ledger, m.ledger.indices.contains(i), let new = edit(m.ledger[i].text) {
                m.ledger[i].text = new
            } else {
                continue
            }
            changed += ps.count
        }
        let after = ProposalUndo.Notes(m)
        if after != notesBefore {
            undo.notes = notesBefore
            undo.notesAfter = after
        }
        undo.lines.sort { $0.key < $1.key }
        return (undo, changed)
    }

    static func count(of word: String, in text: String) -> Int {
        guard !word.isEmpty else { return 0 }
        return text.components(separatedBy: word).count - 1
    }

    /// Puts the ledger changes and additions into the meeting, each change only while its entry still reads `was`.
    /// Changed and added entries are the person's (`edited`), as on the page. Returns the undo and how many applied.
    static func ledger(_ m: inout SavedMeeting, changes: [PanelProposal.Change], add: [PanelProposal.Add]) -> (undo: ProposalUndo, changed: Int) {
        let before = ProposalUndo.Notes(m)
        var changed = 0
        var removing: [Int] = []
        for c in changes {
            guard m.ledger.indices.contains(c.index), m.shownRefs(m.ledger[c.index].text) == c.was || m.ledger[c.index].text == c.was else { continue }
            if c.remove == true {
                removing.append(c.index)
                changed += 1
                continue
            }
            if let t = c.text { m.ledger[c.index].text = m.keyedRefs(t); m.ledger[c.index].refs = nil }
            if let o = c.owner { m.ledger[c.index].owner = m.ownerKey(o) }
            if let d = c.due { m.ledger[c.index].due = d }
            if let done = c.done {
                m.ledger[c.index].done = done
                m.ledger[c.index].doneAt = done ? Date() : nil
            }
            m.ledger[c.index].edited = true
            changed += 1
        }
        for i in removing.sorted(by: >) { m.ledger.remove(at: i) }
        for a in add {
            m.ledger.append(LedgerEntry(kind: a.kind, text: m.keyedRefs(a.text), owner: a.owner.map(m.ownerKey), due: a.due, edited: true))
            changed += 1
        }
        var undo = ProposalUndo()
        if changed > 0 {
            undo.notes = before
            undo.notesAfter = ProposalUndo.Notes(m)
        }
        return (undo, changed)
    }

    /// Puts back what an apply changed, where it is still as the apply left it. True when anything came back.
    @discardableResult
    static func undo(_ m: inout SavedMeeting, _ u: ProposalUndo) -> Bool {
        var any = false
        for l in u.lines {
            guard let i = m.transcript.firstIndex(where: { $0.key == l.key }), m.transcript[i].text == l.after else { continue }
            m.transcript[i].text = l.text
            m.transcript[i].heard = l.heard
            any = true
        }
        if let before = u.notes, u.rewrote || ProposalUndo.Notes(m) == u.notesAfter {
            before.put(&m)
            any = true
        }
        return any
    }
}
