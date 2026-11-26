import SwiftUI

/// The words of the proposal cards (docs/38 12-6절).
enum ProposalText {
    static func fixTitle(_ heard: String, _ meant: String) -> String {
        String(localized: "proposal.fix.title", defaultValue: "\(heard) → \(meant)")
    }
    static func counts(lines: Int, notes: Int) -> String {
        switch (lines, notes) {
        case (0, let n): return String(localized: "proposal.fix.notesOnly", defaultValue: "회의 정리에서 \(n)곳이 바뀌어요")
        case (let l, 0): return String(localized: "proposal.fix.linesOnly", defaultValue: "대화 기록에서 \(l)곳이 바뀌어요")
        default: return String(localized: "proposal.fix.both", defaultValue: "대화 기록에서 \(lines)곳, 회의 정리에서 \(notes)곳이 바뀌어요")
        }
    }
    static var rewrite: String { String(localized: "proposal.fix.rewrite", defaultValue: "고친 대화 기록으로 회의 정리 다시 쓰기") }
    static func remember(_ meant: String) -> String {
        String(localized: "proposal.fix.remember", defaultValue: "다음 회의 받아쓰기 용어에 '\(meant)' 넣기")
    }
    static func apply(_ n: Int) -> String { String(localized: "proposal.fix.apply", defaultValue: "\(n)곳 바꾸기") }
    static var applyLedger: String { String(localized: "proposal.ledger.apply", defaultValue: "바꾸기") }
    static var dismiss: String { String(localized: "proposal.dismiss", defaultValue: "취소") }
    static var undo: String { String(localized: "proposal.undo", defaultValue: "되돌리기") }
    static var applied: String { String(localized: "proposal.applied", defaultValue: "바꿨어요") }
    static var undone: String { String(localized: "proposal.undone", defaultValue: "되돌렸어요") }
    static var dismissed: String { String(localized: "proposal.dismissed", defaultValue: "바꾸지 않았어요") }
    static func more(_ n: Int) -> String { String(localized: "proposal.fix.more", defaultValue: "\(n)곳 더 보기") }
    static var ledgerTitle: String { String(localized: "proposal.ledger.title", defaultValue: "결정 사항과 할 일을 이렇게 바꿀게요") }
    static var removeMark: String { String(localized: "proposal.ledger.remove", defaultValue: "지워요") }
    static var doneMark: String { String(localized: "proposal.ledger.done", defaultValue: "끝낸 일로 옮겨요") }
    static var notDoneMark: String { String(localized: "proposal.ledger.notDone", defaultValue: "안 끝낸 일로 옮겨요") }
    static func owner(_ o: String) -> String { String(localized: "proposal.ledger.owner", defaultValue: "담당자 \(o)") }
    static func due(_ d: String) -> String { String(localized: "proposal.ledger.due", defaultValue: "기한 \(d)") }
    static func nameTitle(_ speaker: String, _ name: String) -> String {
        String(localized: "proposal.name.title", defaultValue: "\(speaker) → \(name)")
    }
    static func nameSub(_ lines: Int) -> String {
        String(localized: "proposal.name.sub", defaultValue: "대화 기록 \(lines)줄과 회의 정리에서 화자 이름을 바꿔요")
    }
    static func mergeTitle(_ from: [String], _ into: String) -> String {
        String(localized: "proposal.merge.title", defaultValue: "\(from.joined(separator: ", ")) → \(into)")
    }
    static func mergeSub(_ lines: Int) -> String {
        String(localized: "proposal.merge.sub", defaultValue: "화자를 같은 사람으로 합쳐요. 대화 기록 \(lines)줄의 화자가 바뀌어요")
    }
    static func moveTitle(_ n: Int, _ to: String) -> String {
        String(localized: "proposal.move.title", defaultValue: "대화 기록 \(n)줄의 화자 → \(to)")
    }
    static func assignTitle(_ n: Int) -> String {
        String(localized: "proposal.assign.title", defaultValue: "대화 기록 \(n)줄의 화자를 맥락으로 바로잡아요")
    }
    static var assignSub: String { String(localized: "proposal.assign.sub", defaultValue: "앞뒤 말로 짐작한 거라 틀린 줄은 체크를 풀어 주세요") }
    static func applyAssign(_ n: Int) -> String { String(localized: "proposal.assign.apply", defaultValue: "\(n)줄 바꾸기") }
    static var moveSub: String { String(localized: "proposal.move.sub", defaultValue: "이 줄들만 화자를 바꿔요") }
    static var applyMove: String { String(localized: "proposal.move.apply", defaultValue: "옮기기") }
    static var rewriteTitle: String { String(localized: "proposal.rewrite.title", defaultValue: "회의 정리 다시 쓰기") }
    static func rewriteSub(_ request: String?) -> String {
        let r = (request ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        return r.isEmpty ? String(localized: "proposal.rewrite.sub", defaultValue: "대화 기록으로 회의 정리를 다시 써요. 직접 고친 결정 사항과 할 일은 남아요")
            : String(localized: "proposal.rewrite.subAsked", defaultValue: "'\(r)' 요청대로 회의 정리를 다시 써요. 직접 고친 결정 사항과 할 일은 남아요")
    }
    static var applyRewrite: String { String(localized: "proposal.rewrite.apply", defaultValue: "다시 쓰기") }
    static func kindTitle(_ from: NotesKind?, _ to: NotesKind?) -> String {
        String(localized: "proposal.kind.title", defaultValue: "회의 정리 형식: \((from ?? .meeting).menuTitle) → \(to?.menuTitle ?? "")")
    }
    static var kindSub: String { String(localized: "proposal.kind.sub", defaultValue: "이 형식으로 회의 정리를 다시 써요") }
    static var applyKind: String { String(localized: "proposal.kind.apply", defaultValue: "정리 형식 바꾸기") }
    static var titleSub: String { String(localized: "proposal.title.sub", defaultValue: "회의 제목을 바꿔요") }
    static var applyTitle: String { String(localized: "proposal.title.apply", defaultValue: "제목 바꾸기") }
    static var applyMerge: String { String(localized: "proposal.merge.apply", defaultValue: "합치기") }
    static var applyName: String { String(localized: "proposal.name.apply", defaultValue: "이름 바꾸기") }
    static var newTodo: String { String(localized: "proposal.ledger.new", defaultValue: "새로 넣는 할 일") }

    /// Where a place is, as the card names it.
    static func label(_ at: PanelProposal.At, meeting: SavedMeeting?) -> String {
        if let key = at.line {
            guard let m = meeting, let l = m.transcript.first(where: { $0.key == key }) else { return "" }
            return m.lineName(l) ?? ""
        }
        if at.title == true { return String(localized: "proposal.at.title", defaultValue: "제목") }
        if let i = at.summary { return String(localized: "proposal.at.summary", defaultValue: "요약 \(i + 1)") }
        if let i = at.topic {
            return at.point == nil ? String(localized: "proposal.at.topic", defaultValue: "주제 \(i + 1)")
                : String(localized: "proposal.at.point", defaultValue: "주제 \(i + 1) 내용")
        }
        if at.ledger != nil { return String(localized: "proposal.at.ledger", defaultValue: "결정 사항·할 일") }
        return ""
    }
}

/// A proposal under a panel answer: what changes, the places to keep or leave out, then one black button. Once applied
/// the words show briefly and the check stays (`DoneMark`), with 되돌리기 beside it while the change can come back.
struct PanelProposalCard: View {
    let proposal: PanelProposal
    let turn: UUID
    let state: ProposalState?
    @ObservedObject var store: PanelStore
    let meeting: SavedMeeting?

    var body: some View {
        switch state {
        case .applied?:
            HStack(spacing: 10) {
                DoneMark(text: ProposalText.applied)
                if store.canUndo(proposal, turn: turn) {
                    Button(ProposalText.undo) { store.undo(proposal, turn: turn) }
                        .buttonStyle(GreyTextButton())
                }
            }
        case .undone?:
            Text(ProposalText.undone).uiFont(12.5).foregroundStyle(Theme.ink3)
        case .dismissed?:
            Text(ProposalText.dismissed).uiFont(12.5).foregroundStyle(Theme.ink3)
        case nil:
            if proposal.kind == PanelProposal.fixWords {
                FixCard(proposal: proposal, turn: turn, store: store, meeting: meeting)
            } else if proposal.kind == PanelProposal.nameSpeaker {
                NameCard(proposal: proposal, turn: turn, store: store)
            } else if proposal.kind == PanelProposal.mergeSpeakers {
                MergeCard(proposal: proposal, turn: turn, store: store)
            } else if proposal.kind == PanelProposal.assignLines {
                AssignCard(proposal: proposal, turn: turn, store: store)
            } else if proposal.kind == PanelProposal.moveLines {
                MoveCard(proposal: proposal, turn: turn, store: store)
            } else if proposal.kind == PanelProposal.rewriteNotes {
                SimpleCard(title: ProposalText.rewriteTitle, sub: ProposalText.rewriteSub(proposal.request), button: ProposalText.applyRewrite,
                           ready: store.canWriteAgain(proposal), apply: { store.applyRewrite(proposal, turn: turn) },
                           dismiss: { store.dismiss(proposal, turn: turn) })
            } else if proposal.kind == PanelProposal.changeKind {
                SimpleCard(title: ProposalText.kindTitle(meeting?.shownKind, proposal.notesKind.flatMap(NotesKind.init(rawValue:))),
                           sub: ProposalText.kindSub, button: ProposalText.applyKind,
                           ready: store.canWriteAgain(proposal) && proposal.notesKind.flatMap(NotesKind.init(rawValue:)) != meeting?.shownKind,
                           apply: { store.applyRewrite(proposal, turn: turn) }, dismiss: { store.dismiss(proposal, turn: turn) })
            } else if proposal.kind == PanelProposal.renameTitle {
                SimpleCard(title: ProposalText.nameTitle(proposal.was ?? "", proposal.title ?? ""), sub: ProposalText.titleSub,
                           button: ProposalText.applyTitle, ready: true, apply: { store.applyTitle(proposal, turn: turn) },
                           dismiss: { store.dismiss(proposal, turn: turn) })
            } else {
                LedgerCard(proposal: proposal, turn: turn, store: store, meeting: meeting)
            }
        }
    }
}

/// Grey words that act: the second choice beside a black pill.
struct GreyTextButton: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        Hovered { hover in
            configuration.label.uiFont(12.5, weight: .medium)
                .foregroundStyle(hover ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3))
                .padding(.horizontal, 6).frame(height: 28)
                .contentShape(Rectangle())
                .opacity(configuration.isPressed ? 0.6 : 1)
        }
    }
}

private struct Hovered<Content: View>: View {
    @ViewBuilder let content: (Bool) -> Content
    @State private var hover = false
    var body: some View { content(hover).onHover { hover = $0 } }
}

/// A small square check: black with a white tick when on, a hairline when off.
struct ProposalCheck: View {
    let on: Bool
    var body: some View {
        RoundedRectangle(cornerRadius: 4, style: .continuous)
            .fill(on ? Theme.ink : Color.clear)
            .overlay(RoundedRectangle(cornerRadius: 4, style: .continuous).strokeBorder(on ? Color.clear : Theme.ink.opacity(0.25), lineWidth: 1.2))
            .overlay { if on { Image(systemName: "checkmark").font(.system(size: 8.5, weight: .bold)).foregroundStyle(Theme.onStrong) } }
            .frame(width: 14, height: 14)
    }
}

/// A row with a check: the whole row toggles.
private struct CheckRow<Label: View>: View {
    @Binding var on: Bool
    @ViewBuilder let label: Label
    var body: some View {
        Button { on.toggle() } label: {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                ProposalCheck(on: on).alignmentGuide(.firstTextBaseline) { $0[.bottom] - 2.5 }
                label.frame(maxWidth: .infinity, alignment: .leading)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

private struct FixCard: View {
    static let firstShown = 5

    let proposal: PanelProposal
    let turn: UUID
    @ObservedObject var store: PanelStore
    let meeting: SavedMeeting?
    @State private var kept: Set<Int>
    @State private var rewrite = true
    @State private var remember = true
    @State private var all = false

    init(proposal: PanelProposal, turn: UUID, store: PanelStore, meeting: SavedMeeting?) {
        self.proposal = proposal
        self.turn = turn
        self.store = store
        self.meeting = meeting
        _kept = State(initialValue: Set((proposal.places ?? []).indices))
    }

    private var places: [PanelProposal.Place] { proposal.places ?? [] }
    private var chosen: [PanelProposal.Place] { places.indices.filter(kept.contains).map { places[$0] } }
    private var lines: Int { chosen.filter { $0.at.inTranscript }.count }
    /// Notes written again from a transcript that did not change would bring the heard word back.
    private var offersRewrite: Bool { places.contains { $0.at.inTranscript } && store.canRewrite(proposal) }

    var body: some View {
        let shown = all ? places.indices : places.indices.prefix(Self.firstShown)
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 3) {
                Text(ProposalText.fixTitle(proposal.heard ?? "", proposal.meant ?? "")).uiFont(13.5, weight: .semibold)
                    .foregroundStyle(Theme.ink).fixedSize(horizontal: false, vertical: true)
                Text(ProposalText.counts(lines: places.filter { $0.at.inTranscript }.count, notes: places.filter { !$0.at.inTranscript }.count))
                    .uiFont(12).foregroundStyle(Theme.ink3)
            }
            .padding(.top, 11).padding(.bottom, 6)
            ForEach(Array(shown), id: \.self) { i in
                let p = places[i]
                CheckRow(on: Binding(get: { kept.contains(i) }, set: { if $0 { kept.insert(i) } else { kept.remove(i) } })) {
                    VStack(alignment: .leading, spacing: 1) {
                        Text(ProposalText.label(p.at, meeting: meeting)).uiFont(11, weight: .semibold).foregroundStyle(Theme.ink3)
                        place(p, on: kept.contains(i)).uiFont(12.5).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                    }
                }
                .padding(.vertical, 6)
                .overlay(alignment: .top) { Rectangle().fill(Theme.hairline).frame(height: 1) }
            }
            if !all, places.count > Self.firstShown {
                Button(ProposalText.more(places.count - Self.firstShown)) { all = true }
                    .buttonStyle(GreyTextButton()).padding(.leading, -6)
            }
            VStack(alignment: .leading, spacing: 7) {
                if offersRewrite {
                    CheckRow(on: $rewrite) { Text(ProposalText.rewrite).uiFont(12.5).foregroundStyle(Theme.ink2) }
                        .disabled(lines == 0).opacity(lines == 0 ? 0.45 : 1)
                }
                if !(proposal.meant ?? "").isEmpty {
                    CheckRow(on: $remember) { Text(ProposalText.remember(proposal.meant ?? "")).uiFont(12.5).foregroundStyle(Theme.ink2) }
                }
            }
            .padding(.top, 10)
            .overlay(alignment: .top) { Rectangle().fill(Theme.hairline).frame(height: 1) }
            .padding(.top, 2)
            HStack(spacing: 6) {
                Button {
                    store.applyFix(proposal, turn: turn, choice: .init(places: chosen, rewrite: offersRewrite && rewrite, remember: remember))
                } label: { StrongButton(label: ProposalText.apply(chosen.count)) }
                    .buttonStyle(PressStyle())
                    .disabled(chosen.isEmpty).opacity(chosen.isEmpty ? 0.4 : 1)
                Button(ProposalText.dismiss) { store.dismiss(proposal, turn: turn) }.buttonStyle(GreyTextButton())
            }
            .padding(.top, 12).padding(.bottom, 12)
        }
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
    }

    /// "…앞 글 [은퇴→근태] 뒤 글…": the old word struck through and the new one in ink, or just the old one when left out.
    private func place(_ p: PanelProposal.Place, on: Bool) -> Text {
        // the engine gives at most 30 characters on each side: a full side was cut
        let before = p.before.count >= 30 ? "…" + p.before : p.before
        let after = p.after.count >= 30 ? p.after + "…" : p.after
        var t = Text(before).foregroundStyle(Theme.ink2)
        if on {
            t = t + Text(p.word).strikethrough().foregroundStyle(Theme.ink3) + Text(" " + p.to).fontWeight(.semibold).foregroundStyle(Theme.ink)
        } else {
            t = t + Text(p.word).fontWeight(.semibold).foregroundStyle(Theme.ink2)
        }
        return t + Text(after).foregroundStyle(Theme.ink2)
    }
}

private struct LedgerCard: View {
    let proposal: PanelProposal
    let turn: UUID
    @ObservedObject var store: PanelStore
    let meeting: SavedMeeting?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(ProposalText.ledgerTitle).uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink)
                .padding(.top, 11).padding(.bottom, 6)
            ForEach(Array((proposal.changes ?? []).enumerated()), id: \.offset) { _, c in
                row(was: c.was, to: c.text, marks: marks(c), removed: c.remove == true)
            }
            ForEach(Array((proposal.add ?? []).enumerated()), id: \.offset) { _, a in
                row(was: nil, to: a.text, marks: [ProposalText.newTodo] + [a.owner.map(ProposalText.owner), a.due.map(ProposalText.due)].compactMap { $0 },
                    removed: false)
            }
            HStack(spacing: 6) {
                Button { store.applyLedger(proposal, turn: turn) } label: { StrongButton(label: ProposalText.applyLedger) }
                    .buttonStyle(PressStyle())
                Button(ProposalText.dismiss) { store.dismiss(proposal, turn: turn) }.buttonStyle(GreyTextButton())
            }
            .padding(.top, 12).padding(.bottom, 12)
        }
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
    }

    private func marks(_ c: PanelProposal.Change) -> [String] {
        if c.remove == true { return [ProposalText.removeMark] }
        var out: [String] = []
        if let o = c.owner { out.append(ProposalText.owner(o)) }
        if let d = c.due { out.append(ProposalText.due(d)) }
        if let d = c.done { out.append(d ? ProposalText.doneMark : ProposalText.notDoneMark) }
        return out
    }

    private func row(was: String?, to: String?, marks: [String], removed: Bool) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            if let was {
                Text(was).uiFont(12.5).foregroundStyle(to != nil || removed ? AnyShapeStyle(Theme.ink3) : AnyShapeStyle(Theme.ink))
                    .strikethrough(to != nil || removed, color: Theme.ink.opacity(0.4))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let to {
                Text(to).uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink).fixedSize(horizontal: false, vertical: true)
            }
            if !marks.isEmpty {
                Text(marks.joined(separator: " · ")).uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 7)
        .overlay(alignment: .top) { Rectangle().fill(Theme.hairline).frame(height: 1) }
    }
}

/// A speaker's new name: the speaker as shown, the name, and that it shows across the whole meeting.
private struct NameCard: View {
    let proposal: PanelProposal
    let turn: UUID
    @ObservedObject var store: PanelStore

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 3) {
                Text(ProposalText.nameTitle(proposal.speaker ?? "", proposal.name ?? "")).uiFont(13.5, weight: .semibold)
                    .foregroundStyle(Theme.ink).fixedSize(horizontal: false, vertical: true)
                Text(ProposalText.nameSub(proposal.lines ?? 0)).uiFont(12).foregroundStyle(Theme.ink3)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.top, 11)
            HStack(spacing: 6) {
                Button { store.applyName(proposal, turn: turn) } label: { StrongButton(label: ProposalText.applyName) }
                    .buttonStyle(PressStyle())
                    .disabled(store.speakerKey(proposal) == nil).opacity(store.speakerKey(proposal) == nil ? 0.4 : 1)
                Button(ProposalText.dismiss) { store.dismiss(proposal, turn: turn) }.buttonStyle(GreyTextButton())
            }
            .padding(.top, 12).padding(.bottom, 12)
        }
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
    }
}

/// Speakers that are one person: the ones that go, the one they become, and how many lines move.
private struct MergeCard: View {
    /// "화자 3(서연)" when the speaker has a name: two speakers may show the same name.
    static func label(_ s: PanelProposal.Speaker) -> String { s.name == s.key || s.name.isEmpty ? s.key : "\(s.key)(\(s.name))" }

    let proposal: PanelProposal
    let turn: UUID
    @ObservedObject var store: PanelStore

    var body: some View {
        let ready = store.mergeKeys(proposal) != nil
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 3) {
                Text(ProposalText.mergeTitle((proposal.from ?? []).map(Self.label), proposal.into.map(Self.label) ?? ""))
                    .uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink).fixedSize(horizontal: false, vertical: true)
                Text(ProposalText.mergeSub(proposal.lines ?? 0)).uiFont(12).foregroundStyle(Theme.ink3)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.top, 11)
            HStack(spacing: 6) {
                Button { store.applyMerge(proposal, turn: turn) } label: { StrongButton(label: ProposalText.applyMerge) }
                    .buttonStyle(PressStyle())
                    .disabled(!ready).opacity(ready ? 1 : 0.4)
                Button(ProposalText.dismiss) { store.dismiss(proposal, turn: turn) }.buttonStyle(GreyTextButton())
            }
            .padding(.top, 12).padding(.bottom, 12)
        }
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
    }
}

/// A card with one change: what it is, a line under it, one black button and 취소.
private struct SimpleCard: View {
    let title: String
    let sub: String
    let button: String
    let ready: Bool
    let apply: () -> Void
    let dismiss: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title).uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink).fixedSize(horizontal: false, vertical: true)
                Text(sub).uiFont(12).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
            }
            .padding(.top, 11)
            HStack(spacing: 6) {
                Button(action: apply) { StrongButton(label: button) }
                    .buttonStyle(PressStyle())
                    .disabled(!ready).opacity(ready ? 1 : 0.4)
                Button(ProposalText.dismiss, action: dismiss).buttonStyle(GreyTextButton())
            }
            .padding(.top, 12).padding(.bottom, 12)
        }
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
    }
}

/// Lines said by someone else: each line as it reads, and whose words they become.
private struct MoveCard: View {
    let proposal: PanelProposal
    let turn: UUID
    @ObservedObject var store: PanelStore

    var body: some View {
        let lines = proposal.moved ?? []
        let ready = !store.movedKeys(proposal).isEmpty
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 3) {
                Text(ProposalText.moveTitle(lines.count, proposal.target?.name ?? "")).uiFont(13.5, weight: .semibold)
                    .foregroundStyle(Theme.ink).fixedSize(horizontal: false, vertical: true)
                Text(ProposalText.moveSub).uiFont(12).foregroundStyle(Theme.ink3)
            }
            .padding(.top, 11).padding(.bottom, 6)
            ForEach(Array(lines.enumerated()), id: \.offset) { _, l in
                VStack(alignment: .leading, spacing: 1) {
                    Text(l.speaker ?? "").uiFont(11, weight: .semibold).foregroundStyle(Theme.ink3)
                    Text(l.text ?? "").uiFont(12.5).foregroundStyle(Theme.ink2).lineLimit(2)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, 6)
                .overlay(alignment: .top) { Rectangle().fill(Theme.hairline).frame(height: 1) }
            }
            HStack(spacing: 6) {
                Button { store.applyMove(proposal, turn: turn) } label: { StrongButton(label: ProposalText.applyMove) }
                    .buttonStyle(PressStyle())
                    .disabled(!ready).opacity(ready ? 1 : 0.4)
                Button(ProposalText.dismiss) { store.dismiss(proposal, turn: turn) }.buttonStyle(GreyTextButton())
            }
            .padding(.top, 12).padding(.bottom, 12)
        }
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
    }
}

/// Lines a wrong speaker split made: each line, who it shows now and who the context says said it, with a check.
private struct AssignCard: View {
    let proposal: PanelProposal
    let turn: UUID
    @ObservedObject var store: PanelStore
    @State private var kept: Set<String>

    init(proposal: PanelProposal, turn: UUID, store: PanelStore) {
        self.proposal = proposal
        self.turn = turn
        self.store = store
        _kept = State(initialValue: Set((proposal.assigned ?? []).map(\.key)))
    }

    var body: some View {
        let rows = proposal.assigned ?? []
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 3) {
                Text(ProposalText.assignTitle(rows.count)).uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink)
                    .fixedSize(horizontal: false, vertical: true)
                Text(ProposalText.assignSub).uiFont(12).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
            }
            .padding(.top, 11).padding(.bottom, 6)
            ForEach(rows, id: \.key) { a in
                Button {
                    if kept.contains(a.key) { kept.remove(a.key) } else { kept.insert(a.key) }
                } label: {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        ProposalCheck(on: kept.contains(a.key)).alignmentGuide(.firstTextBaseline) { $0[.bottom] - 2.5 }
                        VStack(alignment: .leading, spacing: 1) {
                            (Text(a.speaker ?? "").strikethrough().foregroundStyle(Theme.ink3)
                             + Text(" → \(a.to.name)").fontWeight(.semibold).foregroundStyle(Theme.ink))
                                .uiFont(11.5)
                            Text(a.text ?? "").uiFont(12.5).foregroundStyle(Theme.ink2).lineLimit(2)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .padding(.vertical, 6)
                .overlay(alignment: .top) { Rectangle().fill(Theme.hairline).frame(height: 1) }
            }
            HStack(spacing: 6) {
                Button { store.applyAssign(proposal, turn: turn, keys: kept) } label: { StrongButton(label: ProposalText.applyAssign(kept.count)) }
                    .buttonStyle(PressStyle())
                    .disabled(kept.isEmpty).opacity(kept.isEmpty ? 0.4 : 1)
                Button(ProposalText.dismiss) { store.dismiss(proposal, turn: turn) }.buttonStyle(GreyTextButton())
            }
            .padding(.top, 12).padding(.bottom, 12)
        }
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
    }
}
