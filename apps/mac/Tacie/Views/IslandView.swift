import SwiftUI

/// The listening island: a small pill at the top of the screen with Tacie's head at its left end. A caught question widens it
/// (who asked what), and the answer card grows out underneath, then folds back. One answer at a time;
/// the rest wait in this meeting's history.
struct IslandView: View {
    @Bindable var model: MeetingModel
    var onStart: (MeetingMode) -> Void = { _ in }
    /// The main window, at this question when one is given.
    var onOpenMain: (String?) -> Void = { _ in }
    /// Puts the island away until the next meeting or 직접 묻기 (the same as turning off 화면 위 Tacie 보이기).
    var onHide: () -> Void = {}
    var onSize: (CGSize) -> Void = { _ in }
    /// What is drawn (the glass of the pill, open button, tabs and card, all together) and the pill, in the island's
    /// own space: the window keeps them on one screen and drags by the pill.
    var onDrawn: (CGRect, CGRect) -> Void = { _, _ in }
    @Namespace private var glass
    @Environment(\.reducesMotion) private var reduceMotion
    /// The pill's width without hover: its left edge stays here while hovering widens it.
    @State private var restingWidth: CGFloat?
    @State private var drawn = DrawnFrames()

    var body: some View {
        GlassGroup {
            VStack(spacing: 8) {
                pillRow.zIndex(1)  // over a card opening out of it
                // above the card, so the tabs stay put while the card under them changes height
                // (under it, a shorter answer moved the next tab under the pointer and hovering looped)
                if model.opened != nil, !model.trail.isEmpty {
                    Trail(model: model)
                        .surface(Capsule())
                        .glassID("trail", in: glass)
                        .drawn("trail", in: drawn)
                        .transition(inOut(.opacity, .opacity))
                }
                if let notice = model.recordNotice, model.shown == .nothing, model.sttNotice == nil {
                    RecordNoticeLine(notice: notice) { model.copyRecordNoticeNow() }
                        .padding(.leading, 14).padding(.trailing, notice == .ask ? 5 : 14).frame(height: 32)
                        .surface(Capsule())
                        .glassID("recordNotice", in: glass)
                        .drawn("recordNotice", in: drawn)
                        .transition(inOut(.opacity, .opacity))
                }
                if let line = model.sttNotice, model.shown == .nothing {
                    Text(line).font(Theme.chip).foregroundStyle(Theme.ink2).lineLimit(1).fixedSize()
                        .padding(.horizontal, 14).frame(height: 28)
                        .surface(Capsule())
                        .glassID("sttNotice", in: glass)
                        .drawn("sttNotice", in: drawn)
                        .transition(inOut(.opacity, .opacity))
                }
                below
            }
        }
        .padding(.top, 4)
        .padding([.horizontal, .bottom], Theme.shadowRoom)
        .fixedSize()
        .coordinateSpace(.named(IslandSpace.name))
        .onAppear { drawn.report = onDrawn }
        .onHover { model.hover($0) }
        .onExitCommand { model.escape() }
        // appearance may overshoot a little (grow); sizes that report state settle without it; leaving is on
        // the transitions below, quicker than arriving. 움직임 줄이기 turns every one into a short fade.
        .animation(anim(Theme.peek), value: model.peeking)
        .animation(anim(Theme.grow), value: model.shown)
        .animation(anim(Theme.settle), value: model.asking)
        .animation(anim(Theme.settle), value: model.sttNotice)
        .animation(anim(Theme.settle), value: model.recordNotice)
        .animation(anim(Theme.settle), value: model.net)
        .animation(anim(Theme.settle), value: model.presenting)
        .animation(anim(Theme.settle), value: model.current?.phase)
        .animation(anim(Theme.settle), value: model.current?.answer)
        .onGeometryChange(for: CGSize.self) { $0.size } action: { onSize($0) }
        .appearanceScheme()
    }

    /// The pill, and beside it (outside, so the pill keeps its width) the button into the main window on the right and,
    /// out of a meeting, the close button on the left (in a meeting stop ends it: hiding a pill that still listens
    /// would look like listening in secret). Hovering widens the pill to the right only (pause and stop come in at its
    /// end) and the open button rides its right edge: the eyes and the clock stay under the pointer. The row keeps room for both on each side, so its width (and the
    /// island's center) never changes on hover.
    private var pillRow: some View {
        let open = model.pillHovered && !model.asking && model.closingAt == nil
        let close = open && !model.isRunning
        return Pill(model: model, onOpenMain: openMain, hovered: model.pillHovered)
            .surface(Capsule())
            .glassID("pill", in: glass)
            .drawn("pill", in: drawn)
            .overlay(alignment: .trailing) {
                // The glass follows layout, not opacity or scale (at opacity 0 it still drew a grey circle; a scale
                // transition left it full size while the white veil grew from a dot). So the circle's frame grows from
                // nothing at the pill's edge to 38 pt, 10 pt out, and the glass container draws it pulling away.
                OpenMainButton(shown: open) {
                    let question = model.current?.id
                    model.close()
                    openMain(question)
                }
                .frame(width: open ? Theme.pillHeight : 0, height: open ? Theme.pillHeight : 0)
                .surface(Circle())
                .glassID("open", in: glass)
                .drawn("open", in: drawn)
                .allowsHitTesting(open)
                // a beat after the pill starts to widen, with the one overshoot; going, at once and without one
                .animation(open ? anim(Theme.grow.delay(0.05)) : anim(Theme.vanish), value: open)
                .alignmentGuide(.trailing) { d in d[.leading] - (open ? Self.openGap : 0) }
            }
            .overlay(alignment: .leading) {
                // the open button's mirror: grows from the pill's left edge, 10 pt out
                CloseIslandButton(shown: close) {
                    model.close()
                    onHide()
                }
                .frame(width: close ? Theme.pillHeight : 0, height: close ? Theme.pillHeight : 0)
                .surface(Circle())
                .glassID("close", in: glass)
                .drawn("close", in: drawn)
                .allowsHitTesting(close)
                .animation(close ? anim(Theme.grow.delay(0.05)) : anim(Theme.vanish), value: close)
                .alignmentGuide(.leading) { d in d[.trailing] + (close ? Self.openGap : 0) }
            }
            .fixedSize()
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { w in
                if !keepsWidth { restingWidth = w }
            }
            // the width before the pointer came, kept only while hovering adds pause and stop; else none, so the row
            // follows the pill in the same animation (a kept width caught up a layout pass later, in a jump, when the
            // ask field opened)
            .frame(width: keepsWidth ? restingWidth : nil, alignment: .leading)
            .padding(.horizontal, Self.hoverReach)
            // the pill's width and the button's place move together: a width that reports state, no overshoot,
            // and the room goes back quicker than it is taken
            .animation(model.pillHovered ? anim(Theme.settle) : anim(Theme.snappy), value: model.pillHovered)
            .contentShape(Rectangle())
            .onHover { model.hoverPill($0) }
    }

    /// Hovering adds 기록만 하기, pause and stop, so the row holds its width. Typing with the pointer on the pill grows the field,
    /// and a kept width clipped its right end.
    private var keepsWidth: Bool {
        var failed = false
        if case .failed = model.state { failed = true }
        return model.pillHovered && IslandSizing.widensOnHover(asking: model.asking, closing: model.closingAt != nil,
                                                               hasQuestion: model.current != nil, failed: failed,
                                                               running: model.isRunning)
    }

    /// Room on the right for 기록만 하기, pause and stop (three 24 pt circles after 8 pt gaps), the gap and the open
    /// button; the same on the left keeps the resting pill centered.
    private static let hoverReach: CGFloat = 3 * (24 + 8) + openGap + Theme.pillHeight
    private static let openGap: CGFloat = 10

    @ViewBuilder private var below: some View {
        Group {
            switch model.shown {
            case .nothing:
                if model.transcriptOpen, model.isRunning {
                    IslandTranscript(model: model, onOpenMain: { openMain(nil) })
                        .surface(RoundedRectangle(cornerRadius: Theme.radiusCard, style: .continuous))
                        .glassID("card", in: glass)
                        .drawn("transcript", in: drawn)
                }
            case .chooser:
                Chooser(model: model, onStart: onStart, onOpenMain: onOpenMain)
                    .surface(RoundedRectangle(cornerRadius: Theme.radiusCard, style: .continuous))
                    .glassID("card", in: glass)
                    .drawn("chooser", in: drawn)
            case .history:
                History(model: model, onOpenMain: onOpenMain)
                    .surface(RoundedRectangle(cornerRadius: Theme.radiusCard, style: .continuous))
                    .glassID("card", in: glass)
                    .drawn("history", in: drawn)
            case .answer:
                if let b = model.current {
                    AnswerCard(block: b, mode: model.mode, large: model.presenting,
                               onClose: { model.close() },
                               onLarge: { model.presenting.toggle() },
                               onConnect: { model.connectFromHint() },
                               onContinue: { continueInMain(b) })
                        .surface(RoundedRectangle(cornerRadius: Theme.radiusCard, style: .continuous))
                        .glassID("card", in: glass)
                        .drawn("answer", in: drawn)
                }
            }
        }
        .transition(fromPill)
    }

    /// The card (and the chooser, history, 대화 기록) opens out of the pill: it starts as the pill's own shape over
    /// the pill and opens down into place, then folds back the same way, quicker. 움직임 줄이기: a plain fade.
    private var fromPill: AnyTransition {
        guard !reduceMotion else { return .opacity.animation(Theme.reduced) }
        let pill = CGSize(width: restingWidth ?? 200, height: Theme.pillHeight)
        let lift = Theme.pillHeight + 8  // the VStack's spacing
        let open = PillEmerge(progress: 1, pill: pill, lift: lift)
        let shut = PillEmerge(progress: 0, pill: pill, lift: lift)
        return .asymmetric(insertion: .modifier(active: shut, identity: open).animation(Theme.grow),
                           removal: .modifier(active: shut, identity: open).animation(Theme.exit))
    }

    private func anim(_ a: Animation) -> Animation { Theme.anim(a, reduce: reduceMotion) }

    /// The main window from the island (the pill, its open button, 메인 창에서 보기): 대화 기록 folds away, since the
    /// meeting page shows the same lines and the island would cover it. The hotkey and the menu keep it open.
    private func openMain(_ question: String?) {
        if model.transcriptOpen { withAnimation(anim(Theme.settle)) { model.closeTranscript() } }
        onOpenMain(question)
    }

    /// 메인 창에서 이어서 물어보기: the card closes, the main window opens at its meeting with the panel seeded by it.
    private func continueInMain(_ b: MeetingModel.Block) {
        if let panel = model.panel { PanelEntries.continueIsland(question: b.question, answer: b.answer, cards: b.cards, in: panel) }
        model.close()
        openMain(b.id)
    }

    /// Coming in on `grow`, leaving on the quicker `exit`; with 움직임 줄이기 both are a plain fade.
    private func inOut(_ insertion: AnyTransition, _ removal: AnyTransition) -> AnyTransition {
        guard !reduceMotion else { return .opacity.animation(Theme.reduced) }
        return .asymmetric(insertion: insertion.animation(Theme.grow), removal: removal.animation(Theme.exit))
    }
}

// MARK: pill

private struct Pill: View {
    @Bindable var model: MeetingModel
    let onOpenMain: (String?) -> Void
    let hovered: Bool
    @FocusState private var focused: Bool
    @Environment(\.reducesMotion) private var reduceMotion
    @Environment(\.displayScale) private var displayScale

    var body: some View {
        HStack(spacing: 10) {
            CueEyes(mood: mood, level: model.level, remote: model.isRunning && model.mode == .remote,
                    tint: mood == .failed ? .error : .warm, height: Self.headHeight)
            // one kind of content gives way to the next on the spot (stacked, so the pill never holds both side by side)
            ZStack(alignment: .leading) {
                content.id(contentKind).transition(PillSwap.transition(reduce: reduceMotion, scale: displayScale))
            }
            .animation(Theme.anim(Theme.settle, reduce: reduceMotion), value: contentKind)
        }
        .padding(.leading, 10)
        .padding(.trailing, trailingPad)
        .frame(height: Theme.pillHeight)
        .contentShape(Capsule())
        .onTapGesture { tap() }
    }

    private var trailingPad: CGFloat { model.asking || model.isRunning || (model.prompt != nil && !model.promptFolded) ? 8 : 16 }
    /// The head fills the pill but for 7 pt above and below.
    private static let headHeight: CGFloat = 24

    private var mood: EyeMood {
        if model.asking && model.current == nil { return .listening }
        if let b = model.current {
            switch b.phase {
            case .finding: return b.cards.isEmpty ? .heard : .thinking
            case .failed: return .failed
            case .found:
                if b.unanswered || b.fallback { return .unsure }
                return b.answerDone ? .happy : .thinking
            }
        }
        if model.closingAt != nil { return .heard }
        if model.prompt != nil && !model.promptFolded && !model.isRunning { return .heard }
        if model.trouble != nil || model.net == .offline { return .failed }
        if model.micNote != nil { return .paused }
        switch model.state {
        case .listening: return .listening
        case .starting: return .starting
        case .failed: return .failed
        case .paused: return .paused
        case .idle: return .asleep
        }
    }

    /// Which of `content`'s kinds is showing; a new one swaps in with `PillSwap` (a new question counts as new).
    private var contentKind: String {
        if model.asking { return "ask" }
        if model.closingAt != nil { return "closing" }
        if let b = model.current { return "question:\(b.id)" }
        if case .failed = model.state { return "failed" }
        if model.isRunning { return "running" }
        if model.prompt != nil && !model.promptFolded { return "prompt" }
        return "start"
    }

    @ViewBuilder private var content: some View {
        if model.asking {
            askField
        } else if let deadline = model.closingAt {
            closing(deadline)
        } else if let b = model.current {
            why(b)
        } else if case .failed = model.state {
            (model.limitReached ? Text("이번 달 사용량을 다 썼어요") : Text("듣기를 멈췄어요 · 눌러서 보기")).font(Theme.chip).foregroundStyle(Theme.ink2)
        } else if model.isRunning {
            running
        } else if let app = model.prompt, !model.promptFolded {
            prompt(app)
        } else {
            HStack(spacing: 8) {
                if let app = model.prompt { FoldedPrompt(app: app) { model.unfoldPrompt() } }
                Text("회의 시작").font(Theme.chip).foregroundStyle(Theme.ink2)
                Image(systemName: "chevron.down").font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.ink4)
                    .rotationEffect(.degrees(model.shown == .chooser ? 180 : 0))
            }
        }
    }

    /// Why the card appeared: who asked, and the question itself.
    private func why(_ b: MeetingModel.Block) -> some View {
        HStack(spacing: 8) {
            if let a = model.asker(b) {
                // the speaker who asked: their dot and name, a step lighter while they go by "화자 N"
                HStack(spacing: 5) {
                    SpeakerDot(name: a.name, key: a.key, color: a.color, size: 18)
                    Text(a.name).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(a.name == a.key ? AnyShapeStyle(Theme.ink3) : AnyShapeStyle(Theme.ink2))
                        .lineLimit(1).fixedSize()
                }
            } else {
                Text(origin(b)).font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.ink3).fixedSize()
            }
            Text(b.question).font(Theme.question).foregroundStyle(Theme.ink).lineLimit(1).truncationMode(.tail)
                .frame(maxWidth: Theme.cardWidth - 150, alignment: .leading)
                .fixedSize(horizontal: true, vertical: false)
            if model.islandBlocks.count > 1 { historyChip }
        }
    }

    private func origin(_ b: MeetingModel.Block) -> String {
        switch b.origin {
        case .voice: return String(localized: "방금 들은 질문")  // no speaker (no voice model); one with a speaker shows its dot and name (asker)
        case .typed: return String(localized: "직접 물었어요")
        case .slack(let channel, let author): return "\(channel) · \(author)"
        }
    }

    /// "오늘은 여기까지 하죠" was heard: the countdown, a way to keep going, and the stop that needs no waiting.
    private func closing(_ deadline: Date) -> some View {
        HStack(spacing: 8) {
            TimelineView(.periodic(from: .now, by: 1)) { ctx in
                Text(MeetingEnd.line(until: deadline, now: ctx.date)).font(Theme.chip).monospacedDigit()
                    .foregroundStyle(Theme.ink2).lineLimit(1).fixedSize()
            }
            Button { model.keepGoing(by: .button) } label: {
                Text("계속 듣기").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.ink2)
                    .padding(.horizontal, 9).frame(height: 24).background(Theme.fill, in: Capsule()).fixedSize()
            }
            .buttonStyle(.plain)
            .help("회의를 이어서 들어요. 질문이 나오거나 잠깐 조용해도 회의를 끝내지 않아요.")
            PillIcon(symbol: "stop.fill", help: "지금 끝내기 ⌃⌥⌘M") { model.stop(by: .countdown) }
        }
    }

    /// A meeting app began: what began, a way in, and a way to put it away. Nothing records until 듣기 시작.
    private func prompt(_ app: MeetingApp) -> some View {
        MeetingPrompt(app: app, accept: { model.acceptPrompt() }, dismiss: { model.dismissPrompt() })
    }

    // a call that may be silent outranks a missing microphone: with both, nothing at all may be heard
    private var warning: String? { model.trouble ?? model.callNote ?? model.micNote ?? model.sttNote ?? model.quietNote }

    private var warningHelp: String {
        if model.asksToChooseAI { return String(localized: "녹음은 계속돼요. 고르면 받아쓰기가 시작돼요.") }
        if model.trouble != nil { return String(localized: "계속 듣고 있어요. 다시 되면 이 줄이 사라져요.") }
        if model.callNote != nil {
            return String(localized: "상대 말이 안 들리면 설정 → 일반의 처음 안내 다시 보기에서 회의 앱 소리를 확인해 주세요.")
        }
        if model.micNote == nil, model.sttNote != nil { return String(localized: "마이크 소리는 계속 듣고 있어요. 서버로 보내지 않고 다시 되면 이 줄이 사라져요.") }
        if model.micNote == nil, model.quietNote != nil { return QuietMeeting.help }
        guard model.micNote != nil else { return "" }
        return model.mode == .inPerson ? String(localized: "마이크나 이어폰을 연결하면 이어서 들어요.") : String(localized: "마이크나 이어폰을 연결하면 내 목소리도 들어요.")
    }

    private var running: some View {
        HStack(spacing: 8) {
            Circle().fill(warning != nil || model.net == .offline ? Theme.warn : model.state == .listening ? Theme.live : Theme.ink4)
                .frame(width: 6, height: 6)
                .scaleEffect(model.state == .listening && warning == nil && model.net != .offline ? 1 + CGFloat(model.level) * 0.5 : 1)
            // a failing service or a missing microphone replaces the mode name; the line clears once it works again
            Text(warning ?? (model.state == .paused ? String(localized: "일시정지") : model.state == .starting ? String(localized: "준비 중") : model.mode?.short ?? ""))
                .font(Theme.chip).foregroundStyle(Theme.ink2).lineLimit(1).fixedSize()
                .help(warningHelp)
            if model.asksToChooseAI {
                // open edition with nothing chosen (2-D): one way in, to settings → AI; recording goes on
                Button { model.openAISettings() } label: {
                    Text("고르기").font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.ink)
                        .padding(.horizontal, 9).frame(height: 22)
                        .background(Theme.ink.opacity(0.08), in: Capsule())
                }
                .buttonStyle(.plain)
            }
            if let start = model.startedAt {
                // a fixed width: a timeline's ideal size is not its text's, and the history chip got squeezed. With the
                // signal bars after it, the width follows the digits (0:15, 12:52), so the bars sit next to the time
                // instead of after an empty slot
                TimelineView(.periodic(from: .now, by: 1)) { ctx in
                    let t = elapsed(from: start, to: ctx.date)
                    Text(t).font(Theme.chip).monospacedDigit().foregroundStyle(Theme.ink3)
                        .frame(width: model.net != .ok ? Self.timeWidth(t) : 38, alignment: .leading)
                }
                .fixedSize()
            }
            if model.net != .ok {
                NetBars(level: model.net)
                    .help(model.netHelp)
                    // last in the row, the bars need the room a round button would have had (the pill keeps 8 pt
                    // for one): about the gap the eyes have on the left
                    .padding(.trailing, hovered || model.recordOnly || !model.islandBlocks.isEmpty ? 0 : 4)
                    .transition(.opacity)
            }
            if model.mode == .remote && !Capture.allowed {
                Image(systemName: "eye.slash").font(.system(size: 10.5, weight: .semibold)).foregroundStyle(Theme.ink3)
                    .help("사내 코드가 공유 화면으로 새지 않게, 화면 공유와 녹화에는 안 보여요.")
            }
            if model.recordOnly && !hovered {
                Text("기록만").font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.ink3)
                    .padding(.horizontal, 8).frame(height: 22)
                    .background(Theme.fill, in: Capsule())
                    .fixedSize()
                    .help("답 카드를 띄우지 않고 녹음, 대화 기록, 회의 정리만 해요.")
            }
            if hovered {
                PillIcon(symbol: model.recordOnly ? "text.bubble" : "waveform",
                         help: model.recordOnly ? "답 카드로 바꾸기 · 이 회의만 바뀌어요" : "기록만으로 바꾸기 · 이 회의만 바뀌어요") { model.toggleRecordOnly() }
                    .transition(budIn(0))
                PillIcon(symbol: model.state == .paused ? "play.fill" : "pause.fill", help: "잠깐 멈추기 / 다시 듣기") { model.togglePause() }
                    .transition(budIn(1))
                PillIcon(symbol: "stop.fill", help: "회의 끝내기 ⌃⌥⌘M") { model.stop(by: .island) }
                    .transition(budIn(2))
            }
            if model.isRunning { transcriptChip }
            // with 대화 기록 open the answers are in it: the history would list them twice
            if !model.islandBlocks.isEmpty, !model.transcriptOpen { historyChip }
        }
    }

    /// "대화 기록 ⌄": the meeting's lines under the pill (docs/04 3-1); dark while open, like the history chip.
    private var transcriptChip: some View {
        Button { withAnimation(Theme.anim(Theme.settle, reduce: reduceMotion)) { model.toggleTranscript() } } label: {
            HStack(spacing: 4) {
                Image(systemName: "text.alignleft").font(.system(size: 10, weight: .semibold))
                Text("대화 기록").font(.system(size: 12, weight: .semibold))
                Image(systemName: "chevron.down").font(.system(size: 8.5, weight: .bold))
                    .rotationEffect(.degrees(model.transcriptOpen ? 180 : 0))
            }
            .foregroundStyle(model.transcriptOpen ? Theme.onStrong : Theme.ink2)
            .padding(.horizontal, 9)
            .frame(height: 24)
            .background(model.transcriptOpen ? Theme.ink : Theme.fill, in: Capsule())
            .fixedSize()
        }
        .buttonStyle(.plain)
        .help("대화 기록 펼치기 / 접기")
    }

    /// "This meeting · 3": earlier answers, one tap away.
    private var historyChip: some View {
        Button { model.toggle(.history) } label: {
            HStack(spacing: 4) {
                Image(systemName: "square.stack").font(.system(size: 10, weight: .semibold))
                Text("\(model.islandBlocks.count)").font(.system(size: 12, weight: .semibold)).monospacedDigit()
                    // one more caught: the digit rolls up
                    .contentTransition(reduceMotion ? .identity : .numericText(countsDown: false))
            }
            .foregroundStyle(model.shown == .history ? .white : Theme.ink2)
            .padding(.horizontal, 9)
            .frame(height: 24)
            .background(model.shown == .history ? Theme.ink : Theme.fill, in: Capsule())
            .fixedSize()
            .animation(Theme.anim(Theme.snappy, reduce: reduceMotion), value: model.islandBlocks.count)
        }
        .buttonStyle(.plain)
        .help("이번 회의에서 찾은 것")
    }

    private var askField: some View {
        HStack(spacing: 8) {
            TextField("", text: $model.query)
                .textFieldStyle(.plain)
                .font(.system(size: 14.5, weight: .medium))
                .foregroundStyle(Theme.ink)
                // drawn here in ink: the system placeholder colour follows the glass, white over dark windows,
                // and macOS ignores a colour on the prompt
                .background(alignment: .leading) {
                    if model.query.isEmpty {
                        Text("무엇이든 물어보세요").font(.system(size: 14.5, weight: .medium)).foregroundStyle(Theme.ink3)
                            .allowsHitTesting(false)
                    }
                }
                .focused($focused)
                .onSubmit { model.ask() }
                .frame(width: Theme.cardWidth - 120)
                .onAppear { focused = true }
            if model.query.isEmpty {
                Keycap(text: "esc")
            } else {
                Button { model.ask() } label: {
                    Image(systemName: "arrow.up").font(.system(size: 11, weight: .bold)).foregroundStyle(Theme.onStrong)
                        .frame(width: 24, height: 24).background(Theme.ink, in: Circle())
                }
                .buttonStyle(.plain)
            }
        }
    }

    /// Pause and stop on hover: each swells out of a blur a beat after the pill starts to widen, the second one a beat
    /// after the first. They leave together and at once (vanish, no blur), gone before the pill's end passes them.
    private func budIn(_ index: Int) -> AnyTransition {
        guard !reduceMotion else { return .opacity.animation(Theme.reduced) }
        return .asymmetric(
            insertion: .modifier(active: Bud(active: true), identity: Bud(active: false))
                .animation(Theme.grow.delay(Theme.stagger(index + 1))),
            removal: .modifier(active: Bud(active: true, blur: 0, scale: 0.8), identity: Bud(active: false))
                .animation(Theme.vanish))
    }

    private func tap() {
        switch PillTap.action(asking: model.asking, closing: model.closingAt != nil, current: model.current?.id,
                              running: model.isRunning) {
        case .nothing: break
        case .chooser: model.toggle(.chooser)
        case .main(let question):
            model.close()  // the main window takes over; the card folds back into the pill
            onOpenMain(question)
        }
    }

    /// The width of an elapsed time in the pill's figures: the same for every value with as many characters.
    static func timeWidth(_ t: String) -> CGFloat {
        let font = NSFont.monospacedDigitSystemFont(ofSize: 12.5, weight: .medium)
        let template = String(t.map { $0.isNumber ? "0" : $0 })
        return ceil((template as NSString).size(withAttributes: [.font: font]).width)
    }

    private func elapsed(from a: Date, to b: Date) -> String {
        let s = Int(b.timeIntervalSince(a))
        return String(format: "%d:%02d", s / 60, s % 60)
    }
}

/// Half size, clear and blurred: where a pill control starts when it buds in, and ends when it goes.
private struct Bud: ViewModifier {
    let active: Bool
    var blur: CGFloat = 4
    var scale: CGFloat = 0.5
    func body(content: Content) -> some View {
        content
            .scaleEffect(active ? scale : 1)
            .blur(radius: active ? blur : 0)
            .opacity(active ? 0 : 1)
    }
}

/// The pill's question when a meeting app starts. It stays until the meeting is over, so it arrives with a nudge the
/// eye catches from the meeting app: the app's icon pops in and the button pulses twice.
private struct MeetingPrompt: View {
    let app: MeetingApp
    let accept: () -> Void
    let dismiss: () -> Void
    @State private var arrived = false
    @State private var pulse = false
    @State private var hover = false

    var body: some View {
        HStack(spacing: 8) {
            Group {
                if let icon = app.icon {
                    Image(nsImage: icon).resizable().interpolation(.high)
                } else {
                    Image(systemName: "globe").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.ink2)
                }
            }
            .frame(width: 18, height: 18)
            .scaleEffect(arrived ? 1 : 0.2)
            .opacity(arrived ? 1 : 0)
            Text(app.started).font(Theme.chip).foregroundStyle(Theme.ink2).lineLimit(1).fixedSize()
            Button(action: accept) {
                Text("듣기 시작").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.ink)
                    .padding(.horizontal, 10).frame(height: 24)
                    .background(hover ? Theme.fillHover : Theme.fill, in: Capsule())
                    .overlay(Capsule().strokeBorder(Theme.live.opacity(pulse ? 0.9 : 0), lineWidth: 1.5))
                    .scaleEffect(pulse ? 1.06 : 1)
                    .fixedSize()
            }
            .buttonStyle(.plain)
            .onHover { hover = $0 }
            .help("원격 회의로 듣기 시작해요. 누르기 전에는 아무것도 듣지 않아요.")
            PillIcon(symbol: "xmark", help: "이번 회의는 듣지 않기", action: dismiss)
        }
        .task(id: app) {
            arrived = false
            pulse = false
            withAnimation(.spring(response: 0.35, dampingFraction: 0.55)) { arrived = true }
            for _ in 0..<2 {
                try? await Task.sleep(for: .milliseconds(450))
                withAnimation(.easeOut(duration: 0.22)) { pulse = true }
                try? await Task.sleep(for: .milliseconds(380))
                withAnimation(.easeIn(duration: 0.3)) { pulse = false }
            }
        }
    }
}

/// The question folded after a minute: the meeting app's icon before 회의 시작; it opens the question again.
private struct FoldedPrompt: View {
    let app: MeetingApp
    let open: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: open) {
            Group {
                if let icon = app.icon {
                    Image(nsImage: icon).resizable().interpolation(.high)
                } else {
                    Image(systemName: "globe").font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.ink2)
                }
            }
            .frame(width: 16, height: 16)
            .padding(4)
            .background(hover ? Theme.fillHover : .clear, in: Circle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .help(String(localized: "\(app.started) · 눌러서 듣기 시작"))
        .transition(.scale(scale: 0.3).combined(with: .opacity))
    }
}

private struct PillIcon: View {
    let symbol: String
    let help: LocalizedStringKey
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol).font(.system(size: 9, weight: .bold)).foregroundStyle(hover ? Theme.ink : Theme.ink2)
                .frame(width: 24, height: 24)
                .background(hover ? Theme.fillHover : Theme.fill, in: Circle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .help(help)
    }
}

/// Beside the pill while the pointer is on it: the main window, at the answer on screen if there is one.
private struct OpenMainButton: View {
    let shown: Bool
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            Image(systemName: "arrow.up.left.and.arrow.down.right")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(hover ? Theme.ink : Theme.ink2)
                .scaleEffect(shown ? 1 : 0.3)
                .opacity(shown ? 1 : 0)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .contentShape(Circle())
        }
        .buttonStyle(PressStyle())
        .onHover { hover = $0 }
        .help("메인 창 열기 ⌃⌥⌘T")
        .accessibilityLabel("메인 창 열기")
    }
}

/// Left of the pill while the pointer is on it, out of a meeting: puts the island away.
private struct CloseIslandButton: View {
    let shown: Bool
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            Image(systemName: "xmark")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(hover ? Theme.ink : Theme.ink2)
                .scaleEffect(shown ? 1 : 0.3)
                .opacity(shown ? 1 : 0)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .contentShape(Circle())
        }
        .buttonStyle(PressStyle())
        .onHover { hover = $0 }
        .help("화면 위 Tacie 닫기")
        .accessibilityLabel("화면 위 Tacie 닫기")
    }
}

// MARK: trail

/// The last few questions under the card, oldest first. Hovering one shows its answer, so an answer is not lost
/// when the next question comes; clicking keeps it open. A question heard while reading waits here, marked.
private struct Trail: View {
    let model: MeetingModel
    @Namespace private var ns
    @Environment(\.reducesMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 2) {
            ForEach(model.trail) { b in
                TrailTab(block: b, on: b.id == model.current?.id, waiting: b.id == model.opened && model.peeking != nil,
                         ns: ns, look: { model.look(at: b.id) }, keep: { model.open(b.id) })
            }
        }
        .padding(4)
        // one black capsule glides to the tab being read instead of each tab lighting up
        .animation(Theme.anim(Theme.glide, reduce: reduceMotion), value: model.current?.id)
    }
}

private struct TrailTab: View {
    let block: MeetingModel.Block
    let on: Bool
    let waiting: Bool  // opened behind the answer being read
    let ns: Namespace.ID
    let look: () -> Void
    let keep: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: keep) {
            Text(block.question).font(.system(size: 12, weight: .medium)).lineLimit(1).truncationMode(.tail)
                .frame(maxWidth: 96, alignment: .leading)
                .fixedSize(horizontal: true, vertical: false)
                .foregroundStyle(on ? Theme.onStrong : hover ? Theme.ink : Theme.ink2)
                .padding(.horizontal, 10)
                .frame(height: 26)
                .background {
                    if on {
                        Capsule().fill(Theme.ink).matchedGeometryEffect(id: "trail-on", in: ns)
                    } else if hover {
                        Capsule().fill(Theme.fillHover)
                    }
                }
                // a badge, not inline: a tab that changes width would slide its neighbours under the pointer
                .overlay(alignment: .topTrailing) {
                    if waiting { Circle().fill(Theme.live).frame(width: 7, height: 7).offset(x: -1, y: 1) }
                }
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .onHover { inside in
            hover = inside
            if inside { look() }
        }
        .help(block.question)
    }
}

// MARK: chooser

/// Before a meeting: which kind it is decides what we listen to. Asking directly works without one.
private struct Chooser: View {
    let model: MeetingModel
    let onStart: (MeetingMode) -> Void
    let onOpenMain: (String?) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if case .failed(let msg) = model.state {
                Text(msg).font(.system(size: 13)).foregroundStyle(Theme.ink)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.warn.opacity(0.08), in: RoundedRectangle(cornerRadius: Theme.radiusInner))
            }
            if model.limitReached {
                // nothing would be transcribed this month: no meeting to start, only where the usage is
                HStack(spacing: 10) {
                    Spacer()
                    Button("닫기") { model.dismissFailure(); model.close() }
                        .buttonStyle(.plain).font(Theme.button).foregroundStyle(Theme.ink3)
                    Button("사용량 보기") { model.close(); model.dismissFailure(); model.openAccount() }
                        .buttonStyle(InkButtonStyle())
                }
            } else {
                choices
            }
        }
        .padding(18)
        .frame(width: Theme.cardWidth, alignment: .leading)
    }

    @ViewBuilder private var choices: some View {
        Text("어떤 회의인가요?").font(.system(size: 13.5, weight: .semibold)).foregroundStyle(Theme.ink2)
        MeetingModeTiles(onStart: onStart)
        RecordOnlyRow()
        HStack(spacing: 10) {
            Button { model.beginAsking() } label: {
                HStack(spacing: 8) {
                    Text("직접 묻기")
                    Text("⌃⌥⌘A").font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.ink3)
                }
            }
            .buttonStyle(GhostButtonStyle())
            .fixedSize()
            // the way into the main window from a card that is already open, in words (the pill itself starts meetings here)
            Button { model.close(); onOpenMain(nil) } label: {
                HStack(spacing: 8) {
                    Text("메인 창 열기")
                    Text("⌃⌥⌘T").font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.ink3)
                }
            }
            .buttonStyle(GhostButtonStyle())
            .fixedSize()  // the Slack line gives way first: English cut this to "Open main…"
            Spacer()
            Label("슬랙 채널 연결 · 준비 중", systemImage: "number")
                .font(.system(size: 12)).foregroundStyle(Theme.ink4)
        }
    }
}

// MARK: history

/// This meeting's questions, newest first. Unanswered ones are marked: they become doc candidates.
private struct History: View {
    let model: MeetingModel
    let onOpenMain: (String?) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("이번 회의 · 질문 \(model.islandBlocks.count)개").font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.ink3)
                .padding(.horizontal, 10)
                .padding(.bottom, 2)
            ScrollView {
                VStack(spacing: 2) {
                    ForEach(model.islandBlocks) { b in HistoryRow(block: b, asker: model.asker(b)) { model.open(b.id) } }
                }
            }
            .scrollIndicators(.never)
            .frame(maxHeight: 340)
            .fixedSize(horizontal: false, vertical: true)
            Button { model.close(); onOpenMain(nil) } label: {
                HStack(spacing: 6) {
                    Text("메인 창에서 보기")
                    Image(systemName: "arrow.up.right").font(.system(size: 10, weight: .semibold))
                }
            }
            .buttonStyle(GhostButtonStyle())
            .padding(.horizontal, 6)
            .padding(.top, 6)
        }
        .padding(10)
        .padding(.top, 6)
        .frame(width: Theme.cardWidth, alignment: .leading)
    }
}

private struct HistoryRow: View {
    let block: MeetingModel.Block
    /// The speaker who asked, as in the meeting record; nil for typed, Slack, or no voice model.
    let asker: (key: String, name: String, color: Int)?
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                if let a = asker {
                    SpeakerDot(name: a.name, key: a.key, color: a.color, size: 16)
                } else {
                    Image(systemName: symbol).font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.ink3).frame(width: 16)
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text(block.question).font(.system(size: 13.5, weight: .medium)).foregroundStyle(Theme.ink).lineLimit(1)
                    Text(detail).font(.system(size: 12)).foregroundStyle(Theme.ink3).lineLimit(1)
                }
                Spacer(minLength: 8)
                if block.unanswered {
                    Text(block.waitingSync != nil ? String(localized: "준비 중") : block.noSource ? String(localized: "연결 전") : String(localized: "못 찾았어요")).font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.ink2)
                        .padding(.horizontal, 8).frame(height: 20).background(Theme.fill, in: Capsule())
                }
                Text(block.at, format: .dateTime.hour().minute()).font(.system(size: 11.5)).monospacedDigit().foregroundStyle(Theme.ink4)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(hover ? Theme.fill : .clear, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(PressStyle())
        .onHover { hover = $0 }
    }

    private var symbol: String {
        switch block.origin {
        case .voice: return "waveform"
        case .typed: return "keyboard"
        case .slack: return "number"
        }
    }

    private var detail: String {
        if case .finding = block.phase { return String(localized: "찾는 중") }
        if block.waitingSync != nil { return String(localized: "동기화가 끝나면 찾아요") }
        if block.noSource { return String(localized: "연결하면 다시 찾아요") }
        guard let top = block.cards.first else { return String(localized: "찾은 곳이 없어요") }
        return top.name
    }
}

// MARK: Liquid Glass on macOS 26, plain views before it

private struct GlassGroup<Content: View>: View {
    @ViewBuilder let content: Content
    var body: some View {
        if #available(macOS 26.0, *) {
            GlassEffectContainer(spacing: 8) { content }
        } else {
            content
        }
    }
}

private extension View {
    @ViewBuilder func glassID(_ id: String, in ns: Namespace.ID) -> some View {
        if #available(macOS 26.0, *) {
            self.glassEffectID(id, in: ns)
        } else {
            self.softShadow()
        }
    }
}

private enum IslandSpace {
    static let name = "island"
}

private extension View {
    /// Reports this glass's frame in the island's space while it is on screen, under `id`.
    func drawn(_ id: String, in frames: DrawnFrames) -> some View {
        onGeometryChange(for: CGRect.self) { $0.frame(in: .named(IslandSpace.name)) } action: { frames.set(id, $0) }
            .onDisappear { frames.set(id, nil) }
    }
}

/// The glass frames, passed on in the layout pass that reports them, next to the island's size: through a @State and
/// onChange they came a pass late, and the window was placed for the island's old shape.
@MainActor
private final class DrawnFrames {
    var report: (CGRect, CGRect) -> Void = { _, _ in }
    private var frames: [String: CGRect] = [:]

    func set(_ id: String, _ frame: CGRect?) {
        guard frames[id] != frame else { return }
        frames[id] = frame
        let shown = frames.values.filter { $0.width > 0 && $0.height > 0 }
        report(shown.reduce(CGRect.null) { $0.union($1) }, frames["pill"] ?? .null)
    }
}

/// The pill's words changing kind (listening → a caught question → back): the old words blur and lift away quickly,
/// the new blur in from just below. On a 1x monitor only the blur and fade, no move (a move ends off the pixel grid
/// there and the text stays smudged, see `Theme.arrive`); with 움직임 줄이기 a plain fade.
private struct PillSwap: ViewModifier {
    var y: CGFloat
    var blur: CGFloat
    var opacity: Double
    func body(content: Content) -> some View { content.blur(radius: blur).offset(y: y).opacity(opacity) }

    static func transition(reduce: Bool, scale: CGFloat) -> AnyTransition {
        guard !reduce else { return .opacity }
        let move: CGFloat = scale >= 2 ? 5 : 0
        let shown = PillSwap(y: 0, blur: 0, opacity: 1)
        return .asymmetric(insertion: .modifier(active: PillSwap(y: move, blur: 4, opacity: 0), identity: shown),
                           removal: .modifier(active: PillSwap(y: -move, blur: 4, opacity: 0), identity: shown)
                               .animation(.easeOut(duration: 0.16)))
    }
}

/// `fromPill`'s frames: at 0 the view is masked to the pill's size and shape and lifted onto the pill; at 1 it is
/// itself. The mask stops at full size while the lift may overshoot with `grow`.
private struct PillEmerge: ViewModifier, Animatable {
    var progress: CGFloat
    let pill: CGSize
    let lift: CGFloat
    var animatableData: CGFloat {
        get { progress }
        set { progress = newValue }
    }

    func body(content: Content) -> some View {
        let p = min(max(progress, 0), 1)
        content
            .mask(alignment: .top) {
                GeometryReader { g in
                    let w = pill.width + (g.size.width - pill.width) * p
                    let h = pill.height + (g.size.height - pill.height) * p
                    let r = pill.height / 2 + (Theme.radiusCard - pill.height / 2) * p
                    RoundedRectangle(cornerRadius: r, style: .continuous)
                        .frame(width: max(0, w), height: max(0, h))
                        .frame(maxWidth: .infinity, alignment: .top)
                }
            }
            .offset(y: -lift * (1 - progress))
            .opacity(Double(min(1, p * 3)))
    }
}
