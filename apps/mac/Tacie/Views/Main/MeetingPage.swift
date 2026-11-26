import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// One meeting, summary first (design notes §5-2): who took part, then two tabs: Notes (summary, what the meeting
/// decided, the answers found, the questions to write down) and Transcript, the transcript in turns (design notes 5-1).
struct MeetingPage: View {
    let meeting: SavedMeeting
    let live: Bool
    /// While live: what the pill says in place of 대면·원격 (a service failing, no microphone).
    var warning: String? = nil
    /// While live: the sentence being said (Soniox), at the end of 대화 기록.
    var liveLine: LiveLine? = nil
    let library: MeetingLibrary
    @Bindable var selection: MainSelection
    let ids: [String]
    let actions: MainActions
    /// Remembered attendees: a speaker named as one of them keeps their colour.
    var people: [Person] = []
    /// Naming speakers from the page (spec 5-3); none where the page is only drawn.
    var editing: SpeakerEditing? = nil
    /// Another meeting is going on: this one's recording does not play meanwhile (meeting-recording spec 5-4).
    var running = false
    /// Whether the recording opens, once checked; given only where the page is drawn on its own (renders).
    var checkedSound: RecordingState? = nil
    /// A word drawn as if the pointer were on it, for renders.
    var hoverWord: String? = nil
    /// What is connected and what could be, for the line that connects over questions kept without a source.
    var sources: FirstStepsInfo? = nil
    /// The first sync's progress while it runs: kept questions wait for it instead of a connection.
    var syncLine: String? = nil
    /// A tool's chip on that line: (tool, already connected).
    var connect: (ATool, Bool) -> Void = { _, _ in }
    /// The main window's panel, for the entrances on this page (docs/36 1-1절); nil leaves them out.
    var panel: PanelStore? = nil
    /// 회의 (the page of every meeting), from the top row's ‹ 회의; nil leaves it out.
    var back: (() -> Void)? = nil

    @State private var copied: String?
    /// The ledger row the panel asked for: scrolled to, and lit for a moment.
    @State private var ledgerFocus: String?
    @State private var markedRow: Int?
    @State private var markTurn = 0
    /// The kept recording's files, checked off the page's body (opening them reads the disk).
    @State private var sound: RecordingState?
    /// The word being heard; only the words read it.
    @State private var heard = PlaybackFocus()
    @Environment(RecordingPlayer.self) private var player: RecordingPlayer?
    /// The to-do just added, whose words take the caret.
    @State private var newTodo: Int?
    @State private var renaming = false
    @State private var deleting = false
    @State private var deletingSound = false
    @State private var eventMenu = false
    @State private var eventHover = false
    @Environment(\.reducesMotion) private var reduceMotion
    @Namespace private var tabLine
    @Environment(\.displayScale) private var displayScale
    /// One selection over the 정리 tab's text, from the summary down through the questions: a drag runs across them all,
    /// ⌘A and ⌘C take them as one piece.
    @State private var text = TextSelectionGroup()
    /// 정리 or 대화 기록; until a pick, a meeting going on shows its transcript and an ended one its notes.
    @State private var tab: PageTab?
    /// The transcript tab's scroll, for its jump buttons.
    @State private var transcriptScroll = TranscriptScroll()
    /// 번역 함께 / 원문만 on the transcript tab (docs/30); this Mac's choice for every meeting
    @AppStorage("transcriptTranslations") private var showTranslations = true

    enum PageTab { case notes, transcript }

    private var cal: Calendar { MainView.calendar }
    private var found: [SavedQuestion] { meeting.questions.filter { !$0.unanswered } }
    private var missing: [SavedQuestion] { meeting.questions.filter(\.toDocument) }
    /// Heard before anything was connected: searched again once a service is.
    private var kept: [SavedQuestion] { meeting.questions.filter { $0.noSource == true } }

    /// The question to scroll to, when it is on this page (asked for by a click on the island).
    /// A question asked for by the island, or a ledger row asked for by the panel.
    private var focus: Binding<String?> {
        let question = meeting.questions.contains { $0.id == selection.question }
        return Binding(get: { (question ? selection.question : nil) ?? ledgerFocus },
                       set: { v in
                           guard v == nil else { return }
                           if question { selection.question = nil }
                           ledgerFocus = nil
                       })
    }

    private var hasTranscript: Bool {
        !meeting.transcript.isEmpty || !(playback?.unheard.isEmpty ?? true) || (live && liveLine != nil)
    }
    /// An iPhone meeting keeps its 대화 기록 tab: it says where the transcript is.
    private var hasTabs: Bool { hasTranscript || meeting.fromIPhone }
    private var shown: PageTab { hasTabs ? tab ?? (live ? .transcript : .notes) : .notes }

    var body: some View {
        let _ = text.set(shown == .notes ? paragraphs : [], section: 0)
        Group {
            if shown == .transcript && !meeting.fromIPhone {
                // its own scroll view, the text view's: the row above, not over it, so the page's edges are the view's
                VStack(spacing: 0) {
                    toolbar
                    TranscriptPage(meeting: meeting, scroll: transcriptScroll, people: people, calendar: cal, editing: speakerEditing, playback: playback,
                                   bottom: capsuleURLs == nil ? 60 : 96, live: live ? liveLine : nil,
                                   translations: showTranslations) {
                        VStack(alignment: .leading, spacing: 0) {
                            linkBanner
                            header
                            tabBar
                            if let note = soundNote { Text(note).uiFont(12).foregroundStyle(Theme.ink3).padding(.bottom, 16) }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    // as on the 정리 tab: above the player's line when it is there
                    .overlay(alignment: .bottomTrailing) {
                        if transcriptScroll.shown.long {
                            JumpButtons(up: transcriptScroll.shown.up, down: transcriptScroll.shown.down) { transcriptScroll.jump?($0) }
                                .padding(.trailing, 16).padding(.bottom, capsuleURLs == nil ? 16 : 68)
                                .transition(.opacity)
                        }
                    }
                    // listening, scrolled away from the word being heard: a way back to it, over the player
                    .overlay(alignment: .bottom) {
                        if let player, player.meetingID == meeting.id, player.playing, transcriptScroll.heard != 0 {
                            Button { transcriptScroll.toHeard?() } label: {
                                HStack(spacing: 6) {
                                    Image(systemName: transcriptScroll.heard < 0 ? "arrow.up" : "arrow.down").uiFont(11, weight: .semibold)
                                    Text("지금 듣는 곳으로").uiFont(13, weight: .medium)
                                }
                                .foregroundStyle(Theme.ink)
                                .padding(.horizontal, 14).frame(height: 32)
                                .background { Capsule().fill(Theme.raised).shadow(color: Theme.ink.opacity(0.08), radius: 11, y: 6) }
                                .overlay(Capsule().strokeBorder(Theme.ink.opacity(0.1)))
                                .contentShape(Capsule())
                            }
                            .buttonStyle(PressStyle())
                            .padding(.bottom, 76)
                            .transition(.opacity)
                        }
                    }
                    .animation(.easeOut(duration: 0.18), value: transcriptScroll.shown)
                    .animation(.easeOut(duration: 0.18), value: transcriptScroll.heard)
                }
            } else {
                // the jump buttons stand above the player's line when it is there, so a narrow window never puts them on it
                TitleRowPage(focus: focus, jumps: capsuleURLs == nil ? 16 : 68) {
                    toolbar
                } content: {
                    VStack(alignment: .leading, spacing: 0) {
                        linkBanner
                        header
                        if hasTabs { tabBar }
                        if shown == .transcript, let note = IPhoneMeetingUI.transcriptNote(for: meeting) {
                            TranscriptElsewhere(text: note)
                        } else {
                            if live && MeetingModel.notesOn { liveNote }
                            notes
                            if !found.isEmpty || !kept.isEmpty { answers }
                            if !missing.isEmpty { missingSection }
                        }
                    }
                    .padding(.horizontal, 56)
                    .padding(.top, 14)
                    .padding(.bottom, capsuleURLs == nil && !meeting.resumable(now: Date()) ? 60 : 96)
                    .frame(maxWidth: 792, alignment: .leading)
                    .frame(maxWidth: .infinity)
                    // a drag can start anywhere on the page, the margins either side of the column too, not only on the letters
                    .background { SelectionPad(group: text) }
                }
            }
        }
        // a source in the main window's panel: its ledger row lit on 정리, or else the transcript at its lines
        .onChange(of: selection.jump.mark, initial: true) {
            guard let mark = selection.jump.take(for: meeting.id) else { return }
            if let i = mark.ledgerIndex, meeting.ledger.indices.contains(i) {
                markLedger(i)
            } else if !mark.keys.isEmpty {
                showSource(mark.keys)
            }
        }
        // a click on a question in the island scrolls to it, on the 정리 tab
        .onChange(of: selection.question, initial: true) { _, q in
            if q != nil, focus.wrappedValue != nil { tab = .notes }
        }
        .overlay(alignment: .bottom) {
            // 녹음 듣기, and beside it 이어서 듣기 while the meeting can be taken up again (checked every 30 s)
            TimelineView(.periodic(from: .now, by: 30)) { t in
                let canResume = !live && !running && meeting.resumable(now: t.date)
                HStack(spacing: 10) {
                    if let urls = capsuleURLs, let player {
                        PlayerCapsule(meeting: meeting, files: { filesNow(else: urls) }, player: player, dimmed: running, people: people)
                    }
                    if canResume { ResumeCapsule { actions.resume(meeting.id) } }
                }
                .padding(.horizontal, 16).padding(.bottom, 16)
            }
        }
        .overlay(alignment: .bottom) {
            if let editing, let toast = editing.toast {
                NamingToastView(toast: toast, undo: editing.undo) { editing.dropToast(toast) }
                    .padding(.bottom, capsuleURLs == nil ? 16 : 72)
                    .transition(.opacity.combined(with: .offset(y: 6)))
            }
        }
        .animation(.easeOut(duration: 0.18), value: editing?.toast)
        .background {
            if let urls = capsuleURLs, let player, !running {
                let id = meeting.id
                SpaceToPlay { [library, meeting] in
                    if player.meetingID == id { player.toggle(); return }
                    let now = library.filesToPlay(meeting)
                    player.play(meeting: id, urls: now.isEmpty ? urls : now, fromMs: 0)
                }
            }
        }
        .background {
            if let player, player.meetingID == meeting.id {
                CurrentWordWatch(player: player, lines: meeting.transcript, focus: heard)
            }
        }
        // the file of a meeting going on is still being written: nothing to check until it ends
        .task(id: soundKey) { if !live { sound = checkedSound ?? library.checkedRecordingState(meeting) } }
        // leaving the page (another meeting, a collected page, the window going) stops its sound
        .onDisappear { if player?.meetingID == meeting.id { player?.stop() } }
        .confirmationDialog("이 회의를 지울까요?", isPresented: $deleting) {
            Button("지우기", role: .destructive) {
                Analytics.capture(.meetingDeleted)
                selection.id = MeetingGroups.selection(afterDeleting: meeting.id, in: ids)
                library.remove(meeting.id)
            }
            Button("취소", role: .cancel) {}
        } message: {
            Text(IPhoneMeetingUI.deleteMessage(for: meeting))
        }
    }

    /// Naming speakers, where the meeting allows it (not on an iPhone meeting).
    private var speakerEditing: SpeakerEditing? { IPhoneMeetingUI.allows(.rename, for: meeting) ? editing : nil }

    // MARK: top row

    private var toolbar: some View {
        HStack(spacing: 2) {
            if let back {
                Button(action: back) {
                    HStack(spacing: 3) {
                        Image(systemName: "chevron.left").uiFont(11, weight: .semibold)
                        Text(String(localized: "mainView.meetings", defaultValue: "회의")).uiFont(13)
                    }
                    .foregroundStyle(Theme.ink2)
                    .padding(.horizontal, 8).frame(height: 28)
                    .contentShape(Rectangle())
                }
                .buttonStyle(PressStyle())
                .help("모든 회의로 돌아가요")
            }
            Spacer()
            if let panel { AskEntryButton(store: panel, meetingID: meeting.id).padding(.trailing, 6) }
            if live {
                // a meeting going on can only be ended here, not copied or deleted
                HStack(spacing: 8) {
                    Button(action: actions.stop) { OutlineButton(label: String(localized: "meetingPage.end", defaultValue: "끝내기")) }.buttonStyle(PressStyle())
                    Keycap(text: "⌃⌥⌘M")
                }
                .padding(.trailing, 6)
            } else {
                if let panel, PanelEntries.showsPrep(meeting) {
                    PrepEntryButton(store: panel, meeting: meeting).padding(.trailing, 6)
                }
                Button { copy(meeting.markdown(calendar: cal), as: "page"); Analytics.capture(.notesExported(how: .copy, part: .page)) } label: {
                    IconButton(symbol: copied == "page" ? "checkmark" : "doc.on.doc")
                }
                .buttonStyle(PressStyle())
                .help("회의 정리를 Markdown으로 복사해요")
                ShareLink(item: meeting.markdown(calendar: cal),
                          subject: Text(meeting.displayTitle(calendar: cal)),
                          preview: SharePreview(meeting.displayTitle(calendar: cal), image: Image(nsImage: NSApp.applicationIconImage))) {
                    IconButton(symbol: "square.and.arrow.up")
                }
                .buttonStyle(PressStyle())
                .simultaneousGesture(TapGesture().onEnded { Analytics.capture(.notesExported(how: .share, part: .page)) })
                .help("회의 정리를 메일·메모·메시지로 보내요")
                Menu {
                    Button(meeting.pinned == true ? String(localized: "고정 풀기") : String(localized: "목록 위에 고정")) {
                        Analytics.capture(.meetingPinned(on: meeting.pinned != true))
                        library.update(meeting.id) { $0.pinned = $0.pinned == true ? nil : true }
                    }
                    Button("Markdown 파일로 저장…", action: save)
                    if capsuleURLs != nil && IPhoneMeetingUI.allows(.deleteRecording, for: meeting) {
                        Button("녹음만 지우기…", role: .destructive) { deletingSound = true }
                    }
                    Button("이 회의 지우기…", role: .destructive) { deleting = true }
                    if Build.isDev { devItems }
                } label: {
                    IconButton(symbol: "ellipsis")
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
            }
        }
        .padding(.horizontal, 14)
        .frame(height: 52)
        // on the row, not the page: the page already carries the meeting's own dialog
        .confirmationDialog("녹음만 지울까요?", isPresented: $deletingSound) {
            Button("지우기", role: .destructive, action: deleteSound)
            Button("취소", role: .cancel) {}
        } message: {
            Text("받아쓴 대화와 정리는 그대로 남고, 소리만 이 Mac에서 사라져요.")
        }
    }

    /// Keeps the text and notes; the page then says "녹음을 지웠어요" (meeting-recording spec 5-5).
    private func deleteSound() {
        if player?.meetingID == meeting.id { player?.stop() }
        sound = nil
        library.deleteRecording(meeting.id)
    }

    // MARK: header

    /// Where this meeting goes on from, with a way to cut the link; nothing when that meeting is gone.
    @ViewBuilder private var linkBanner: some View {
        if PanelFeatures.current.meetingLink, let id = meeting.linkedFrom, let from = library.meetings.first(where: { $0.id == id }) {
            LinkBanner(from: from, calendar: cal) { Analytics.capture(.meetingLinked(cut: true)); library.update(meeting.id) { $0.cutLink() } }
        }
    }

    @ViewBuilder private var header: some View {
        HStack(spacing: 8) {
            if live && meeting.recording == .kept {
                HStack(spacing: 5) {
                    Circle().fill(Theme.warn).frame(width: 7, height: 7)
                    Text("녹음 중").uiFont(12.5, weight: .semibold).foregroundStyle(Theme.warn)
                    Text("·").uiFont(12.5).foregroundStyle(Theme.ink3)
                }
                .padding(.trailing, -3)  // "녹음 중 · 오늘 …": the dot sits a space from each side
            } else if live {
                LiveDot(warn: warning != nil)
            }
            if live {
                TimelineView(.periodic(from: .now, by: 1)) { t in
                    Text(liveLine(t.date)).uiFont(12.5).foregroundStyle(Theme.ink2).monospacedDigit()
                }
                if let warning {
                    Text("· \(warning)").uiFont(12.5, weight: .medium).foregroundStyle(Theme.warn).lineLimit(1)
                }
            } else {
                TimelineView(.periodic(from: .now, by: 30)) { t in
                    Text(meeting.pageHeader(calendar: cal, now: t.date)).uiFont(12.5).foregroundStyle(Theme.ink3)
                }
                if meeting.fromIPhone { IPhoneChip() }
                if aiWrote { AITag() }
                if showsKind {
                    NotesKindButton(kind: meeting.shownKind, hasEdits: NotesFlow.hasPersonsWork(meeting)) { k in
                        actions.changeKind(meeting.id, k)
                    }
                }
            }
        }
        .padding(.bottom, 10)
        // grey only until the title comes; a meeting going on keeps its title until it ends (the menu does the same)
        EditableTitle(text: meeting.displayTitle(calendar: cal),
                      grey: meeting.title == nil && (live || meeting.notes == .waiting),
                      editable: !live, editing: $renaming) { title in
            Analytics.capture(.meetingRenamed)
            library.update(meeting.id) { $0 = $0.renamed(title) }
        }
        .padding(.bottom, meeting.event != nil ? 8 : meeting.hasSpeakers ? 18 : 30)
        if let event = meeting.event { eventLine(event) }
        if meeting.hasSpeakers {
            // above the tabs: a suggestion's card (화자 추천) hangs down over them
            PeopleStrip(meeting: meeting, people: people, editing: speakerEditing).padding(.bottom, 36).zIndex(1)
        }
    }

    /// The linked calendar event (docs/35 3-3); pressed, the day's events to link instead, or 일정 떼어 내기.
    private func eventLine(_ event: MeetingEvent) -> some View {
        MeetingEventLine(event: event, invited: meeting.invited?.count ?? 0, calendar: cal, lit: eventHover || eventMenu)
            .onHover { eventHover = $0 && !live }
            .onTapGesture { if !live { eventMenu = true } }
            .popover(isPresented: $eventMenu, arrowEdge: .bottom) {
                MeetingEventMenu(day: meeting.start, linked: event.id, events: CalendarStore.shared.events(on: meeting.start, calendar: cal),
                                 calendar: cal, pick: { e in
                    eventMenu = false
                    library.update(meeting.id) {
                        $0.link(MeetingEvent(id: e.id, title: e.title, start: e.start, end: e.end), invited: e.invited,
                                meaningful: CalendarLink.meaningful(e.title))
                    }
                }, unlink: {
                    eventMenu = false
                    library.update(meeting.id) { $0.unlink() }
                })
            }
            .padding(.bottom, meeting.hasSpeakers ? 16 : 28)
    }

    /// "오늘 오후 3:30 · 원격 회의 · 듣는 중 12:04"
    private func liveLine(_ now: Date) -> String {
        let s = max(0, Int(now.timeIntervalSince(meeting.start)))
        let clock = s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
        return String(localized: "오늘 \(SavedMeeting.format("a h:mm", meeting.start, cal)) · \(meeting.modeTitle) · 듣는 중 \(clock)")
    }

    /// The kind button: on written notes with a transcript to write again from, and while a picked kind is written.
    private var showsKind: Bool {
        !live && !meeting.transcript.isEmpty && IPhoneMeetingUI.allows(.rewrite, for: meeting)
            && (meeting.notes == .done || (meeting.notes == .waiting && meeting.kindSource == "user"))
    }

    /// Anything on the page the model wrote: the notes, or an answer line.
    private var aiWrote: Bool {
        !meeting.summary.isEmpty || !meeting.ledger.isEmpty || meeting.questions.contains { $0.answer != nil }
    }

    private var liveNote: some View {
        HStack(spacing: 8) {
            Image(systemName: "text.alignleft").uiFont(12, weight: .medium)
            Text("회의가 끝나면 요약, 결정 사항, 할 일을 정리해 둬요.").uiFont(13.5)
        }
        .foregroundStyle(Theme.ink2)
        .padding(.horizontal, 14)
        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
        .background(Theme.ink.opacity(0.035), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .padding(.bottom, 40)
    }

    // MARK: notes

    @ViewBuilder private var notes: some View {
        if !live && !hasTranscript && meeting.notes != .waiting && !meeting.fromIPhone {
            // nothing was transcribed: say so and why, instead of a page with only its title
            nothingHeard.transition(arrive)
        } else if !live {
            Group {
                switch meeting.notes {
                case .waiting:
                    writing.transition(.asymmetric(insertion: .opacity.animation(Theme.enter), removal: .opacity.animation(Theme.exit)))
                case .failed:
                    failed.transition(arrive)
                case .done where notesEmpty:
                    emptyNotes.transition(arrive)
                case .done:
                    if PanelFeatures.current.meetingLink, PreparedItem.shows(meeting.prepared), let items = meeting.prepared { PreparedBlock(items: items).transition(arrive) }
                    if !meeting.summary.isEmpty { summary.transition(arrive) }
                    // a meeting shows its results (decisions, to-dos, open questions) before the talk topic by topic
                    // (docs/24 7-3); an interview or a talk keeps its flow first
                    if !resultsFirst, !(meeting.topics ?? []).isEmpty { topics.transition(arrive) }
                    if !meeting.ledger.isEmpty { ledger.transition(arrive) }
                    if resultsFirst, !(meeting.topics ?? []).isEmpty { topics.transition(arrive) }
                case .off: EmptyView()
                }
            }
            // the bars fade out, the text rises 4pt into place, the height settles
            .animation(Theme.anim(Theme.settle, reduce: reduceMotion), value: meeting.notes)
        }
    }

    private var arrive: AnyTransition {
        Theme.arrive(y: 4, reduce: reduceMotion, scale: displayScale)
    }

    private var summary: some View {
        VStack(alignment: .leading, spacing: 0) {
            DocHeading(title: String(localized: "요약"), titleView: heading("h.summary", String(localized: "요약")))
            if meeting.topics == nil {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(Array(meeting.shownSummary.enumerated()), id: \.offset) { i, s in bullet(s, id: "summary.\(i)") }
                }
            } else {
                // a few sentences on the whole meeting: one paragraph, as it pastes
                SelectableText(id: "summary.0", text: summaryParagraph, group: text, size: 15, lineSpacing: 4, color: Theme.ink)
            }
        }
        .padding(.bottom, 44)
    }

    private var summaryParagraph: String { meeting.shownSummary.joined(separator: " ") }

    /// How the talk went, topic by topic in the order it came up: "1. 알림 설정 화면", then its sentences.
    private var topics: some View {
        VStack(alignment: .leading, spacing: 0) {
            DocHeading(title: meeting.shownKind.flowTitle, titleView: heading("h.topics", meeting.shownKind.flowTitle))
            VStack(alignment: .leading, spacing: 22) {
                ForEach(Array((meeting.shownTopics ?? []).enumerated()), id: \.offset) { i, t in
                    VStack(alignment: .leading, spacing: 9) {
                        SelectableText(id: "t.\(i)", text: "\(i + 1). \(t.title)", group: text, size: 15, weight: .semibold,
                                       lineSpacing: 0, color: Theme.ink)
                        ForEach(Array(t.points.enumerated()), id: \.offset) { j, p in point(p, role: t.role(j), id: "t.\(i).\(j)") }
                    }
                }
            }
        }
        .padding(.bottom, 44)
    }

    private var writing: some View {
        VStack(alignment: .leading, spacing: 0) {
            DocHeading(title: String(localized: "요약"))
            VStack(alignment: .leading, spacing: 12) {
                Skeleton(width: 560)
                Skeleton(width: 610, pulseDelay: Theme.stagger(1, step: 0.08))
                Skeleton(width: 380, pulseDelay: Theme.stagger(2, step: 0.08))
            }
            .padding(.vertical, 4)
            HStack(spacing: 7) {
                ProgressView().controlSize(.small).scaleEffect(0.8)
                Text(meeting.shownKind.writing(rewriting: meeting.kindSource == "user")).uiFont(13).foregroundStyle(Theme.ink3)
            }
            .padding(.top, 14)
        }
        .padding(.bottom, 44)
    }

    /// A meeting that ended with no line in it (in person, the sound was often in earphones, out of the microphone's reach).
    private var nothingHeard: some View {
        VStack(alignment: .leading, spacing: 0) {
            DocHeading(title: String(localized: "요약"))
            Text("이 회의에서는 받아쓴 말이 없어서 요약을 쓰지 못했어요.").uiFont(14).foregroundStyle(Theme.ink2)
            Text(QuietMeeting.emptyPage(inPerson: meeting.mode == MeetingMode.inPerson.rawValue, recorded: capsuleURLs != nil))
                .uiFont(13).foregroundStyle(Theme.ink3).lineSpacing(3).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 8)
        }
        .padding(.bottom, 44)
    }

    /// Notes that came back with nothing in them: no summary, topic, item or prepared block.
    private var notesEmpty: Bool {
        meeting.summary.isEmpty && (meeting.topics ?? []).isEmpty && meeting.ledger.isEmpty
            && !(PanelFeatures.current.meetingLink && PreparedItem.shows(meeting.prepared) && meeting.prepared != nil)
    }

    /// Notes that are done but empty said nothing before (a blank page under 정리, 오픈소스판 e2e 2026-11-25): no AI chosen,
    /// so every stretch went untranscribed, or a model that wrote nothing usable.
    private var emptyNotes: some View {
        VStack(alignment: .leading, spacing: 0) {
            DocHeading(title: String(localized: "요약"))
            Text(EmptyNotes.line(transcribed: !meeting.transcript.isEmpty, open: Build.isOpen, mode: AIMode.saved()))
                .uiFont(14).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
            if !meeting.transcript.isEmpty && IPhoneMeetingUI.allows(.rewrite, for: meeting) {
                Button { actions.rewriteNotes(meeting.id) } label: { OutlineButton(label: String(localized: "meetingPage.tryAgain", defaultValue: "다시 쓰기"), icon: "arrow.clockwise") }
                    .buttonStyle(PressStyle())
                    .padding(.top, 12)
            }
        }
        .padding(.bottom, 44)
    }

    private var failed: some View {
        VStack(alignment: .leading, spacing: 0) {
            DocHeading(title: String(localized: "요약"))
            if let limit = meeting.notesLimit, !meeting.transcript.isEmpty {
                LimitNotesBlock(meeting: meeting, limit: limit, left: accountLeft, write: { actions.rewriteNotes(meeting.id) },
                                usage: { selection.settings = .account })
                    // the month may have turned (or the limit been raised) since the server last said
                    .task { await TacieAccount.shared.refresh() }
            } else {
                failedLine
            }
        }
        .padding(.bottom, 44)
    }

    /// This account's month left as the server last said, for the limit block; nil when it has not said.
    private var accountLeft: Double? {
        guard let m = TacieAccount.shared.info?.month, m.limitUsd > 0 else { return nil }
        return min(max(1 - m.spentUsd / m.limitUsd, 0), 1)
    }

    private var failedLine: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(String(localized: "요약을 쓰지 못했어요.") + (meeting.notesProblem.map { " \($0)." } ?? ""))
                .uiFont(14).foregroundStyle(Theme.ink3)
            if !meeting.transcript.isEmpty && IPhoneMeetingUI.allows(.rewrite, for: meeting) {
                Button { actions.rewriteNotes(meeting.id) } label: { OutlineButton(label: String(localized: "meetingPage.tryAgain", defaultValue: "다시 쓰기"), icon: "arrow.clockwise") }
                    .buttonStyle(PressStyle())
                    .padding(.top, 12)
            }
        }
    }

    @ViewBuilder private var ledger: some View {
        if meeting.shownKind == .meeting { meetingLedger } else { kindSections }
    }

    /// An interview's or a talk's lists: each kind under its own heading, no to-dos to add (docs/23 3-5).
    private var kindSections: some View {
        let sections = meeting.shownKind.sections.filter { s in meeting.ledger.contains { $0.kind == s.kind } }
        return VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(sections.enumerated()), id: \.offset) { n, section in
                VStack(alignment: .leading, spacing: 0) {
                    DocHeading(title: section.title, titleView: heading("k.\(section.kind)", section.title)) {
                        if n == 0 {
                            Button { copy(ledgerText, as: "ledger"); Analytics.capture(.notesExported(how: .copy, part: .ledger)) } label: {
                                OutlineButton(label: copied == "ledger" ? String(localized: "복사했어요") : String(localized: "목록 복사"), icon: copied == "ledger" ? "checkmark" : "doc.on.doc")
                            }
                            .buttonStyle(PressStyle())
                        }
                    }
                    VStack(alignment: .leading, spacing: 9) {
                        ForEach(meeting.ledger.indices.filter { meeting.ledger[$0].kind == section.kind }, id: \.self) { i in
                            LedgerBullet(meeting: meeting, index: i, library: library, source: showSource, why: whyAction(i))
                                .modifier(LedgerMarkStyle(on: markedRow == i)).id(Self.ledgerID(i))
                        }
                    }
                }
                .padding(.bottom, 44)
            }
        }
    }

    private var meetingLedger: some View {
        VStack(alignment: .leading, spacing: 0) {
            DocHeading(title: String(localized: "결정과 할 일"), titleView: heading("h.ledger", String(localized: "결정과 할 일"))) {
                Button { copy(ledgerText, as: "ledger"); Analytics.capture(.notesExported(how: .copy, part: .ledger)) } label: {
                    OutlineButton(label: copied == "ledger" ? String(localized: "복사했어요") : String(localized: "목록 복사"), icon: copied == "ledger" ? "checkmark" : "doc.on.doc")
                }
                .buttonStyle(PressStyle())
            }
            ForEach(Self.ledgerKinds, id: \.0) { kind, title in
                let items = meeting.ledger.indices.filter { meeting.ledger[$0].kind == kind }
                if !items.isEmpty {
                    SelectableText(id: "k.\(kind)", text: title, group: text, size: 13, weight: .semibold, lineSpacing: 0,
                                   color: Theme.ink.opacity(displayScale >= 2 ? 0.5 : 0.62))
                        .padding(.bottom, 9)
                    VStack(alignment: .leading, spacing: kind == "commitment" ? 11 : 9) {
                        ForEach(items, id: \.self) { i in
                            if kind == "commitment" {
                                TodoLine(meeting: meeting, index: i, library: library, people: people, editable: true,
                                         focusOnAppear: i == newTodo, source: showSource)
                                    .modifier(LedgerMarkStyle(on: markedRow == i)).id(Self.ledgerID(i))
                                    .transition(.opacity.combined(with: .offset(y: -4)))
                            } else {
                                LedgerBullet(meeting: meeting, index: i, library: library, source: showSource, why: whyAction(i),
                                             openMeeting: { selection.id = $0; selection.question = nil })
                                    .modifier(LedgerMarkStyle(on: markedRow == i)).id(Self.ledgerID(i))
                            }
                        }
                        if kind == "commitment" { addTodo }
                    }
                    .todoColumns()
                    .padding(.bottom, 22)
                }
            }
            // no to-dos yet: one can still be written down
            if !meeting.ledger.contains(where: { $0.kind == "commitment" }) {
                VStack(alignment: .leading, spacing: 9) {
                    Text("할 일").uiFont(13, weight: .semibold).foregroundStyle(Theme.ink.opacity(displayScale >= 2 ? 0.5 : 0.62))
                    addTodo
                }
                .padding(.bottom, 22)
            }
        }
        .padding(.bottom, 22)
    }

    /// A ledger row asked for by the panel: the 정리 tab, scrolled to the row, lit for a moment like 대화 기록에서 보기's lines.
    private func markLedger(_ i: Int) {
        withAnimation(Theme.anim(Theme.glide, reduce: reduceMotion)) { tab = .notes }
        ledgerFocus = Self.ledgerID(i)
        markedRow = i
        markTurn += 1
        let turn = markTurn
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.4) {
            if markTurn == turn { withAnimation(.easeOut(duration: 0.4)) { markedRow = nil } }
        }
    }

    /// "이유 보기" on a decision row; nil without a panel.
    private func whyAction(_ i: Int) -> (() -> Void)? {
        guard let panel else { return nil }
        let m = meeting
        return { PanelEntries.why(m, index: i, in: panel) }
    }

    static func ledgerID(_ i: Int) -> String { "ledger.\(i)" }

    /// 대화 기록에서 보기: the transcript tab, scrolled to the lines a note was written from and lighting them a moment.
    private func showSource(_ keys: [String]) {
        transcriptScroll.pending = keys
        withAnimation(Theme.anim(Theme.glide, reduce: reduceMotion)) { tab = .transcript }
    }

    private static let ledgerKinds = [("decision", String(localized: "결정 사항")), ("commitment", String(localized: "할 일")),
                                       ("open_question", String(localized: "남은 논의"))]

    /// "＋ 할 일 더하기": a new empty to-do with the caret in it (left empty, it goes again).
    private var addTodo: some View {
        AddTodoButton {
            // the new row fades down into place and the button below makes room (only adding animates: a row taken out
            // shifts the indices, so a fade there would land on the wrong row)
            withAnimation(Theme.anim(Theme.settle, reduce: reduceMotion)) { newTodo = LedgerEdit.addTodo(library, meeting) }
            // only the row as it first shows takes the caret; a later row at the same place does not
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { newTodo = nil }
        }
            .padding(.top, 2)
    }

    /// A section heading as a paragraph of the page's selection.
    private func heading(_ id: String, _ title: String) -> AnyView {
        AnyView(SelectableText(id: id, text: title, group: text, size: 17, weight: .semibold, lineSpacing: 0, color: Theme.ink)
            .fixedSize())
    }

    private var resultsFirst: Bool { meeting.shownKind == .meeting }

    /// A line of a topic: what was settled reads a little heavier, what was left open carries a 미정 tag before it.
    private func point(_ s: String, role: String?, id: String) -> Bullet {
        let words = SelectableText(id: id, text: s, group: text, size: 15, weight: role == "conclusion" ? .medium : .regular,
                                   lineSpacing: 4, color: Theme.ink)
        guard role == "open" else { return Bullet(text: s, words: AnyView(words)) }
        return Bullet(text: s, words: AnyView(HStack(alignment: .firstTextBaseline, spacing: 7) {
            SoonTag(text: SavedMeeting.openMark)
            words
        }))
    }

    /// A summary or ledger line as a paragraph of the page's selection.
    private func bullet(_ s: String, id: String) -> Bullet {
        Bullet(text: s, words: AnyView(SelectableText(id: id, text: s, group: text, size: 15, lineSpacing: 4, color: Theme.ink)))
    }

    /// The page's text above the transcript in reading order, as it shows now: the paragraphs of the selection.
    private var paragraphs: [(id: String, text: String)] {
        var out: [(id: String, text: String)] = []
        if !live && meeting.notes == .done {
            if !meeting.summary.isEmpty {
                out.append(("h.summary", String(localized: "요약")))
                if meeting.topics == nil {
                    for (i, s) in meeting.shownSummary.enumerated() { out.append(("summary.\(i)", s)) }
                } else {
                    out.append(("summary.0", summaryParagraph))
                }
            }
            // the same order as the page: a meeting's topics come after its results
            func flow() {
                guard let topics = meeting.shownTopics, !topics.isEmpty else { return }
                out.append(("h.topics", meeting.shownKind.flowTitle))
                for (i, t) in topics.enumerated() {
                    out.append(("t.\(i)", "\(i + 1). \(t.title)"))
                    for (j, p) in t.points.enumerated() { out.append(("t.\(i).\(j)", p)) }
                }
            }
            if !resultsFirst { flow() }
            if !meeting.ledger.isEmpty && meeting.shownKind != .meeting {
                for (kind, title) in meeting.shownKind.sections where meeting.ledger.contains(where: { $0.kind == kind }) {
                    out.append(("k.\(kind)", title))
                }
            } else if !meeting.ledger.isEmpty {
                out.append(("h.ledger", String(localized: "결정과 할 일")))
                for (kind, title) in Self.ledgerKinds {
                    let items = meeting.ledger.indices.filter { meeting.ledger[$0].kind == kind }
                    if items.isEmpty { continue }
                    out.append(("k.\(kind)", title))
                }
            }
            if resultsFirst { flow() }
        }
        if !found.isEmpty || !kept.isEmpty {
            out.append(("h.answers", String(localized: "Tacie의 답")))
            for q in found {
                out.append(("q.\(q.id)", q.question))
                if let a = q.answer { out.append(("a.\(q.id)", Citations.selectable(a, size: 14.5).string)) }
            }
            for q in kept { out.append(("q.\(q.id)", q.question)) }
        }
        if !missing.isEmpty {
            out.append(("h.missing", String(localized: "문서로 남길 질문")))
            for q in missing {
                out.append(("q.\(q.id)", q.shownQuestion))
                if let a = meeting.heardAnswer(q) { out.append(("m.\(q.id)", String(localized: "회의에서 나온 답 · \(a)"))) }
            }
        }
        return out
    }

    private var ledgerText: String {
        var m = meeting
        m.summary = []
        m.topics = nil
        m.questions = []
        let md = m.markdown(calendar: cal)
        guard m.shownKind == .meeting else {
            // an interview's or a talk's lists start at their first section; the copy keeps their headings
            let first = m.shownKind.sections.first { s in m.ledger.contains { $0.kind == s.kind } }
            return first.flatMap { md.range(of: "## \($0.title)\n") }.map { String(md[$0.lowerBound...]) } ?? md
        }
        return md.components(separatedBy: SavedMeeting.decisionsHeading + "\n").last ?? md
    }

    // MARK: questions

    private var answers: some View {
        VStack(alignment: .leading, spacing: 0) {
            DocHeading(title: String(localized: "Tacie의 답"), count: found.count + kept.count, titleView: heading("h.answers", String(localized: "Tacie의 답")))
            if !kept.isEmpty { connectLine.padding(.bottom, 24) }
            VStack(alignment: .leading, spacing: 26) {
                ForEach(found) { q in answer(q) }
                ForEach(Array(kept.enumerated()), id: \.element.id) { i, q in keptAnswer(q, i) }
            }
        }
        .padding(.bottom, 44)
    }

    private func answer(_ q: SavedQuestion) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            asker(q)
            SelectableText(id: "q.\(q.id)", text: q.question, group: text, size: 15, weight: .semibold, lineSpacing: 0,
                           color: Theme.ink)
            if let a = q.answer {
                SelectableText(id: "a.\(q.id)", text: Citations.selectable(a, size: 14.5).string, group: text, size: 14.5,
                               lineSpacing: 3, color: Theme.ink2, styled: { Citations.selectable(a, size: $0) })
            } else if q.cards.isEmpty {
                // just after the end a late answer may still come; after that, nothing will
                Text(live || meeting.notes == .waiting ? String(localized: "찾는 중이에요") : String(localized: "찾지 못했어요")).uiFont(14.5).foregroundStyle(Theme.ink3)
            }
            if !q.cards.isEmpty {
                Wrap(spacing: 6) {
                    ForEach(Array(q.cards.enumerated()), id: \.element.id) { i, c in
                        Button { CardActions.open(c, place: .main) } label: { FileChip(n: i + 1, card: c) }
                            .buttonStyle(.plain)
                            .opacity(quiet(c, in: q) || MeetingCards.gone(c) ? 0.5 : 1)
                            .help(MeetingCards.gone(c) ? String(localized: "지운 회의라 열 수 없어요") : c.path)
                    }
                }
                .padding(.top, 3)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .id(q.id)
    }

    /// Cards the answer line does not cite read quieter, as in the island.
    private func quiet(_ c: Card, in q: SavedQuestion) -> Bool {
        q.answer != nil && !q.cited.isEmpty && !(c.chunk.map(q.cited.contains) ?? false)
    }

    /// Over questions kept without a source: connect right here, or how far the first sync is. Like 처음 써 보기's row.
    private var connectLine: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(syncLine == nil ? String(localized: "팀 자료를 연결하면 이 질문들에 답해요") : String(localized: "자료를 받고 있어요"))
                .uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink)
            Text(syncLine.map { String(localized: "끝나면 이 질문들로 찾아서 여기에 채워요 · \($0)") }
                 ?? String(localized: "회의에서 나온 질문 \(kept.count)개를 적어 뒀어요. 연결하면 바로 찾아서 여기에 채워요."))
                .uiFont(12).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 2)
            if let sources, !(sources.connected.isEmpty && sources.unconnected.isEmpty) {
                Wrap(spacing: 6) {
                    ForEach(sources.connected, id: \.label) { c in
                        ToolChip(tool: c.tool, label: c.label, connected: true) { connect(c.tool, true) }
                    }
                    ForEach(sources.unconnected, id: \.self) { tool in
                        ToolChip(tool: tool, label: FirstStepsInfo.name(tool), connected: false) { connect(tool, false) }
                    }
                }
                .padding(.top, 11)
            }
        }
        .padding(.horizontal, 16).padding(.vertical, 14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background { RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.ink.opacity(0.1)) }
    }

    /// A question kept without a source, where its answer will go: the answer line is an empty bar until then.
    private func keptAnswer(_ q: SavedQuestion, _ i: Int) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            asker(q)
            SelectableText(id: "q.\(q.id)", text: q.question, group: text, size: 15, weight: .semibold, lineSpacing: 0,
                           color: Theme.ink)
            // still, not pulsing: nothing is being written, the place is only empty
            Capsule().fill(Theme.ink.opacity(0.045)).frame(maxWidth: [300, 220, 260][i % 3]).frame(height: 11)
                .padding(.top, 3)
                .accessibilityLabel("연결하면 답이 와요")
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .id(q.id)
    }

    private var missingSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            DocHeading(title: String(localized: "문서로 남길 질문"), count: missing.count, titleView: heading("h.missing", String(localized: "문서로 남길 질문"))) {
                Button {
                    copy(meeting.toDocumentLines.joined(separator: "\n"), as: "missing")
                    Analytics.capture(.notesExported(how: .copy, part: .missing))
                } label: {
                    OutlineButton(label: copied == "missing" ? String(localized: "복사했어요") : String(localized: "목록 복사"), icon: copied == "missing" ? "checkmark" : "doc.on.doc")
                }
                .buttonStyle(PressStyle())
            }
            Text("답을 못 찾았거나 여러 곳에 흩어져 있던 질문이에요. 문서로 남겨 두면 다음에 찾을 수 있어요.").uiFont(13).foregroundStyle(Theme.ink3)
                .padding(.top, -6).padding(.bottom, 10)
            ForEach(missing) { q in
                VStack(spacing: 0) {
                    Rectangle().fill(Theme.hairline).frame(height: 1)
                    HStack(alignment: .firstTextBaseline) {
                        VStack(alignment: .leading, spacing: 5) {
                            SelectableText(id: "q.\(q.id)", text: q.shownQuestion, group: text, size: 15, weight: .medium, lineSpacing: 0,
                                           color: Theme.ink)
                            // said in the meeting but written nowhere Tacie looks: what to put in the document
                            if let a = meeting.heardAnswer(q) {
                                SelectableText(id: "m.\(q.id)", text: String(localized: "회의에서 나온 답 · \(a)"), group: text, size: 13.5, lineSpacing: 2,
                                               color: Theme.ink2)
                            }
                        }
                        Spacer(minLength: 16)
                        asker(q)
                    }
                    .padding(.vertical, 12)
                }
                .id(q.id)
            }
        }
        .padding(.bottom, 44)
    }

    private func asker(_ q: SavedQuestion) -> some View {
        QuestionAsker(meeting: meeting, question: q, people: people)
    }

    // MARK: transcript

    /// 정리 · 대화 기록, under the people: the transcript on its own tab, so the notes open at once and the transcript is
    /// one text to select in.
    private var hasTranslations: Bool { meeting.transcript.contains { $0.translation?.isEmpty == false } }

    private var tabBar: some View {
        HStack(spacing: 22) {
            tabButton(.notes, String(localized: "정리"), count: nil)
            tabButton(.transcript, String(localized: "대화 기록"), count: meeting.transcript.count)
            Spacer(minLength: 0)
        }
        // on the row, not in it: taller than the tabs, it would push the row down on this tab only.
        // A meeting going on has it too: what was heard so far
        .overlay(alignment: .bottomTrailing) {
            if shown == .transcript && !meeting.transcript.isEmpty {
              HStack(spacing: 8) {
                // only on a meeting that has translations (docs/30)
                if hasTranslations {
                    ChoiceStrip(choices: [(true, String(localized: "translation.both", defaultValue: "번역 함께")),
                                          (false, String(localized: "translation.original", defaultValue: "원문만"))],
                                value: $showTranslations)
                }
                Button {
                    copy(meeting.transcriptText(calendar: cal), as: "transcript")
                    Analytics.capture(.notesExported(how: .copy, part: .transcript))
                } label: {
                    OutlineButton(label: copied == "transcript" ? String(localized: "복사했어요") : String(localized: "대화 기록 복사"),
                                  icon: copied == "transcript" ? "checkmark" : "doc.on.doc")
                }
                .buttonStyle(PressStyle())
              }
              .padding(.bottom, 7)
            }
        }
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.ink.opacity(0.08)).frame(height: 1) }
        .padding(.bottom, 32)
    }

    private func tabButton(_ t: PageTab, _ title: String, count: Int?) -> some View {
        let on = shown == t
        return Button {
            // one underline slides between the tabs, like the selection in the meeting list (docs/03 9번)
            withAnimation(Theme.anim(Theme.glide, reduce: reduceMotion)) { tab = t }
        } label: {
            HStack(spacing: 5) {
                Text(title).uiFont(14.5, weight: .semibold).foregroundStyle(on ? Theme.ink : Theme.ink2)
                if let count, count > 0 { Text(verbatim: "\(count)").uiFont(13, weight: .medium).foregroundStyle(Theme.ink4).monospacedDigit() }
            }
            .padding(.bottom, 10)
            .overlay(alignment: .bottom) {
                if on { Rectangle().fill(Theme.ink).frame(height: 2).matchedGeometryEffect(id: "tab-on", in: tabLine) }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: recording (meeting-recording spec 5)

    /// What the sound is: no recording and a deleted one need no disk; a kept one is checked by the task.
    private var soundState: RecordingState? {
        switch meeting.recording {
        case nil: return checkedSound ?? RecordingState.none
        case .deleted?: return checkedSound ?? RecordingState.deleted
        case .kept?: return sound ?? checkedSound ?? (live ? nil : library.knownRecordingState(meeting))
        }
    }

    /// Checked again when the recording changes (closed, joined, deleted).
    private var soundKey: String { RecordingState.checkKey(live: live, meeting: meeting, audioRevision: library.audioRevision(meeting.id)) }

    /// The files to play, read again at the tap; `seen` (what the page checked) only when nothing opens now.
    private func filesNow(else seen: [URL]) -> [URL] {
        let now = library.filesToPlay(meeting)
        return now.isEmpty ? seen : now
    }

    /// The files the player plays, while the page can play them: an ended meeting with its sound.
    private var capsuleURLs: [URL]? {
        guard !live, IPhoneMeetingUI.allows(.listen, for: meeting), case .available(let urls)? = soundState else { return nil }
        return urls
    }

    /// Beside "대화 기록": why nothing plays.
    private var soundNote: String? {
        guard !live else { return nil }
        switch soundState {
        case RecordingState.none?: return String(localized: "녹음하지 않은 회의예요")
        case .deleted?: return String(localized: "녹음을 지웠어요")
        default: return nil
        }
    }

    private var playback: TranscriptPlayback? {
        guard let urls = capsuleURLs else { return nil }
        let id = meeting.id
        let library = library, meeting = meeting
        return TranscriptPlayback(play: { [player] ms in
                                      let now = library.filesToPlay(meeting)
                                      player?.play(meeting: id, urls: now.isEmpty ? urls : now, fromMs: ms)
                                  },
                                  focus: heard, dimmed: running,
                                  unheard: meeting.unheard ?? [], hoverWord: hoverWord)
    }

    private func save() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = meeting.fileName(calendar: cal)
        panel.allowedContentTypes = [UTType(filenameExtension: "md") ?? .plainText]
        panel.canCreateDirectories = true
        guard panel.runModal() == .OK, let url = panel.url else { return }
        guard (try? meeting.markdown(calendar: cal, transcript: true).write(to: url, atomically: true, encoding: .utf8)) != nil else { return }
        Analytics.capture(.notesExported(how: .file, part: .page))
    }

    /// Development builds only: the recording's files in Finder and every kept meeting as one Markdown text, for debugging.
    @ViewBuilder private var devItems: some View {
        Divider()
        if let urls = capsuleURLs {
            // development only: not translated (docs/28), so verbatim
            Button { NSWorkspace.shared.activateFileViewerSelecting(urls) } label: { Text(verbatim: "개발: 녹음 파일 Finder에서 보기") }
        }
        Button {
            copy(library.meetings.map { $0.markdown(calendar: cal) }.joined(separator: "\n\n---\n\n"), as: "all")
        } label: { Text(verbatim: "개발: 모든 회의 Markdown 복사") }
    }

    private func copy(_ text: String, as what: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        // the label crossfades and the button's width follows, both ways (docs/03 Arc 6번)
        let anim = Theme.anim(Theme.snappy, reduce: reduceMotion)
        withAnimation(anim) { copied = what }
        Task {
            try? await Task.sleep(for: .seconds(1.5))
            if copied == what { withAnimation(anim) { copied = nil } }
        }
    }
}
