import Foundation

/// The rules for putting a proposal card's change into its meeting (docs/38 12절, docs/40 6-4), the same on the Mac
/// panel and the iPhone meeting page. Each app passes how it changes a meeting file, writes the notes again and keeps
/// a remembered word. Speakers are named with no remembered person (`person: nil`); the Mac names them its own way.
@MainActor
struct ProposalApplier {
    /// The person's choices on a fix card: the places kept checked, whether to write the notes again and whether to
    /// remember the word.
    struct FixChoice: Equatable {
        var places: [PanelProposal.Place]
        var rewrite: Bool
        var remember: Bool
    }

    /// Changes the meeting with this id, when there is one (the change is not called otherwise).
    var update: (String, (inout SavedMeeting) -> Void) -> Void
    /// 회의 정리 다시 쓰기 from the meeting's transcript.
    var rewrite: (String) -> Void
    /// 다음 회의부터 알아듣기: the word goes to the account's terms.
    var remember: (String) -> Void
    /// Whether the notes may be written again for this meeting once its lines change.
    var canRewrite: (SavedMeeting) -> Bool
    /// 다시 쓰기 with the person's request from the panel (rewrite_notes, "한국어로", "더 짧게"). Without it a rewrite_notes
    /// card writes the notes as `rewrite` does, and the request is lost.
    var rewriteAsked: ((String, String?) -> Void)? = nil
    var place: APanelPlace

    /// Puts the proposal into its meeting. Returns what to put back for 되돌리기, or nil when nothing was applied (the
    /// meeting is gone, a fix's meeting is still going on, the title or speaker is not there).
    func apply(_ p: PanelProposal, choice: FixChoice?) -> ProposalUndo? {
        switch p.kind {
        case PanelProposal.fixWords: applyFix(p, choice: choice ?? FixChoice(places: p.places ?? [], rewrite: false, remember: false))
        case PanelProposal.editLedger: applyLedger(p)
        case PanelProposal.nameSpeaker: applyName(p)
        case PanelProposal.renameTitle: applyTitle(p)
        case PanelProposal.rewriteNotes: applyRewrite(p)
        default: nil
        }
    }

    /// Not while the notes are being written again (they would land over what comes back), and a name only while no one
    /// renamed the speakers since.
    func canUndo(_ u: ProposalUndo, in m: SavedMeeting) -> Bool {
        if let n = u.naming { return m.naming == n.after }
        return m.notes != .waiting
    }

    /// 되돌리기: what the apply changed and is still as it left it comes back. The remembered word stays.
    func undo(_ p: PanelProposal, _ u: ProposalUndo) {
        update(p.meetingId) { m in
            if let n = u.naming {
                if m.naming == n.after { m.restore(n.before) }
            } else {
                ProposalApply.undo(&m, u)
            }
        }
        Analytics.capture(.panelProposal(kind: AProposalKind(p.kind), action: .undone,
                                         places: 0, lines: u.lines.count, rewrite: false, remember: false, place: place))
    }

    // MARK: Each kind

    /// The chosen places into the meeting, then the notes again from the fixed transcript and the word into the
    /// account's terms when chosen.
    private func applyFix(_ p: PanelProposal, choice: FixChoice) -> ProposalUndo? {
        var result: (undo: ProposalUndo, changed: Int)?
        var before: SavedMeeting?
        update(p.meetingId) { m in
            guard m.end != nil else { return }
            before = m
            result = ProposalApply.fix(&m, places: choice.places)
        }
        guard var r = result, let m = before else { return nil }
        let lines = choice.places.filter { $0.at.inTranscript }.count
        let rewrites = choice.rewrite && lines > 0 && canRewrite(m)
        if rewrites {
            r.undo.rewrote = true
            r.undo.notes = r.undo.notes ?? ProposalUndo.Notes(m)
            rewrite(p.meetingId)
        }
        let remembers = choice.remember && !(p.meant ?? "").isEmpty
        if remembers, let w = p.meant { remember(w) }
        Analytics.capture(.panelProposal(kind: .fixWords, action: .applied, places: r.changed, lines: lines,
                                         rewrite: rewrites, remember: remembers, place: place))
        return r.undo
    }

    private func applyLedger(_ p: PanelProposal) -> ProposalUndo? {
        var result: (undo: ProposalUndo, changed: Int)?
        update(p.meetingId) { result = ProposalApply.ledger(&$0, changes: p.changes ?? [], add: p.add ?? []) }
        guard let r = result else { return nil }
        Analytics.capture(.panelProposal(kind: .editLedger, action: .applied, places: r.changed, lines: 0, rewrite: false, remember: false, place: place))
        return r.undo
    }

    /// The title the person picks: kept as theirs (`titleEdited`), as when renamed on the page.
    private func applyTitle(_ p: PanelProposal) -> ProposalUndo? {
        guard let title = p.title?.trimmingCharacters(in: .whitespacesAndNewlines), !title.isEmpty else { return nil }
        var undo: ProposalUndo?
        update(p.meetingId) { m in
            let before = ProposalUndo.Notes(m)
            m = m.renamed(title)
            undo = ProposalUndo(notes: before, notesAfter: ProposalUndo.Notes(m))
        }
        guard let undo else { return nil }
        Analytics.capture(.panelProposal(kind: .renameTitle, action: .applied, places: 1, lines: 0, rewrite: false, remember: false, place: place))
        return undo
    }

    /// Gives the speaker the name for the whole meeting. The notes keep the speaker's key, so they show it too.
    private func applyName(_ p: PanelProposal) -> ProposalUndo? {
        guard let name = p.name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty else { return nil }
        var undo: ProposalUndo?
        update(p.meetingId) { m in
            guard let key = Self.speakerKey(p, in: m) else { return }
            let before = m.naming
            m.rename(key: key, to: name, person: nil)
            undo = ProposalUndo(naming: .init(before: before, after: m.naming))
        }
        guard let undo else { return nil }
        Analytics.capture(.panelProposal(kind: .nameSpeaker, action: .applied, places: 1, lines: p.lines ?? 0, rewrite: false, remember: false, place: place))
        return undo
    }

    /// 다시 쓰기 of a meeting with its transcript and the person's request. The notes from before come back with 되돌리기.
    private func applyRewrite(_ p: PanelProposal) -> ProposalUndo? {
        var undo: ProposalUndo?
        update(p.meetingId) { m in
            guard canRewrite(m), m.notes != .waiting else { return }
            undo = ProposalUndo(notes: ProposalUndo.Notes(m), rewrote: true)
        }
        guard let undo else { return nil }
        if let rewriteAsked { rewriteAsked(p.meetingId, p.request) } else { rewrite(p.meetingId) }
        Analytics.capture(.panelProposal(kind: .rewriteNotes, action: .applied, places: 1, lines: 0, rewrite: true, remember: false, place: place))
        return undo
    }

    // MARK: Speakers

    /// The speaker key a name_speaker points at: a speaker key, or the name a speaker or a line's person goes by now.
    static func speakerKey(_ p: PanelProposal, in m: SavedMeeting) -> String? {
        if let k = p.key, speakerKeys(m).contains(k) { return k }
        guard let s = p.speaker else { return nil }
        let bare = s.filter { !$0.isWhitespace }
        if let k = (m.voices ?? []).map(\.key).first(where: { $0.filter { !$0.isWhitespace } == bare }) { return k }
        return m.refKey(s)
    }

    /// The meeting's speakers as keys: the named ones (`voices`) and every speaker its lines were heard from, which has
    /// no entry until it is named or merged.
    static func speakerKeys(_ m: SavedMeeting) -> Set<String> {
        Set((m.voices ?? []).map { m.resolved($0.key) } + m.transcript.compactMap { $0.voice.map(m.resolved) })
    }
}
