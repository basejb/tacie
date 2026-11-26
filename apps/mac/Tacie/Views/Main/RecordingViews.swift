import AppKit
import SwiftUI

/// The meeting page's recording (design notes 5-1, 5-2, 5-4, 5-6):
/// words that play from where they were said, the player floating at the bottom, and the unheard stretches.

/// How the transcript plays its recording; nil where the meeting has none to play (or is going on).
struct TranscriptPlayback {
    /// Starts the recording at a millisecond.
    let play: (Int) -> Void
    /// The word being heard now, read only by the words themselves.
    var focus: PlaybackFocus? = nil
    /// Another meeting is going on: nothing plays (its sound would reach the microphone).
    var dimmed = false
    var unheard: [AudioSpan] = []
    /// A word drawn as if the pointer were on it ("<line key>#<shown index>"), for renders.
    var hoverWord: String? = nil
}

/// "▶ 6:12" beside the word under the pointer.
struct TimeChip: View {
    let ms: Int
    /// Its height at text size 100%: the word under the pointer holds it this far up.
    static let height: CGFloat = 16

    @Environment(\.textScale) private var scale

    var body: some View {
        Text("▶ \(RecordingPlayer.timeLabel(ms))").uiFont(10, weight: .medium).monospacedDigit()
            .foregroundStyle(Theme.onStrong)
            .padding(.horizontal, 5).frame(height: Self.height * scale)
            .background(Theme.ink, in: RoundedRectangle(cornerRadius: 4, style: .continuous))
    }
}

/// A stretch the recording kept but whose words did not come back: where the speaker's dot would be stays empty,
/// "받아쓰지 못한 부분 · 0:42" (how long), and a click plays it.
struct UnheardRow: View {
    let span: AudioSpan
    let playback: TranscriptPlayback

    @State private var hover = false

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            Color.clear.frame(width: 26, height: 22)
            Text("받아쓰지 못한 부분 · \(RecordingPlayer.timeLabel(span.end - span.start))").uiFont(14.5)
                .foregroundStyle(Theme.ink3)
            if hover { TimeChip(ms: span.start) }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .background(Theme.ink.opacity(hover ? 0.03 : 0), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .contentShape(Rectangle())
        .onHover { hover = $0 && !playback.dimmed }
        .onTapGesture { playback.play(span.start) }
        .opacity(playback.dimmed ? 0.4 : 1)
        .allowsHitTesting(!playback.dimmed)
        .padding(.horizontal, -12)
    }
}

/// The player floating at the bottom of the meeting page. Before listening: "▶ 녹음 듣기 47:03", a click plays from the
/// start. Once playing: play/pause, who is speaking, the bar (click or drag to move), "6:12 / 47:03", and the speed.
/// While another meeting goes on it says when the sound can be heard.
struct PlayerCapsule: View {
    let meeting: SavedMeeting
    /// Read at the click, not when drawn: the files can be joined after the page looked.
    let files: () -> [URL]
    let player: RecordingPlayer
    var dimmed = false
    /// For the speakers' colours in the waveform.
    var people: [Person] = []

    /// Loudness along the recording, read once the sound plays; the plain bar shows until then.
    @State private var peaks: [Float]?
    @Environment(\.reducesMotion) private var reduceMotion

    private var mine: Bool { player.meetingID == meeting.id }

    var body: some View {
        Group {
            if dimmed {
                HStack(spacing: 9) {
                    PlayCircle(playing: false, size: 28).opacity(0.4)
                    Text("회의가 끝나면 들을 수 있어요").uiFont(12.5).foregroundStyle(Theme.ink3)
                }
                .padding(.leading, 6).padding(.trailing, 14).padding(.vertical, 6)
                .allowsHitTesting(false)
            } else if mine {
                full
            } else {
                Button { player.play(meeting: meeting.id, urls: files(), fromMs: 0) } label: {
                    HStack(spacing: 9) {
                        PlayCircle(playing: false, size: 28)
                        Text("녹음 듣기").uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink)
                        if let s = meeting.recordingSeconds {
                            Text(RecordingPlayer.timeLabel(Int(s * 1000))).uiFont(12).foregroundStyle(Theme.ink3).monospacedDigit()
                        }
                    }
                    .padding(.leading, 6).padding(.trailing, 14).padding(.vertical, 6)
                    .contentShape(Capsule())
                }
                .buttonStyle(PressStyle())
            }
        }
        // "▶ 녹음 듣기" grows into the whole player in place, without a bounce
        .animation(Theme.anim(Theme.settle, reduce: reduceMotion), value: mine)
        // the shadow belongs to the filled shape only, not to the edge line or the content
        .background { Capsule().fill(Theme.raised).shadow(color: Theme.ink.opacity(0.08), radius: 11, y: 6) }
        .overlay(Capsule().strokeBorder(Theme.ink.opacity(0.1)))
    }

    private var full: some View {
        HStack(spacing: 12) {
            Button { player.toggle() } label: { PlayCircle(playing: player.playing, size: 30) }
                .buttonStyle(PressStyle())
                .help(player.playing ? String(localized: "멈춰요") : String(localized: "이어서 들어요"))
            if let key = RecordingPlayer.speaker(in: meeting.transcript, atMs: player.positionMs) {
                // the next speaker's name crossfades in; the room around it follows
                Text(meeting.displayName(key: key)).uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink)
                    .lineLimit(1).fixedSize()
                    .id(key)
                    .transition(.opacity)
            }
            Group {
                if let peaks, !peaks.isEmpty {
                    WaveBar(peaks: peaks, tints: WaveTints(meeting: meeting, people: people, durationMs: player.durationMs),
                            position: player.positionMs, duration: player.durationMs) { player.seek(toMs: $0) }
                } else {
                    ProgressBar(position: player.positionMs, duration: player.durationMs) { player.seek(toMs: $0) }
                }
            }
            .frame(minWidth: 60, idealWidth: 220, maxWidth: 220)
            Text(verbatim: "\(RecordingPlayer.timeLabel(player.positionMs)) / \(RecordingPlayer.timeLabel(player.durationMs))")
                .uiFont(12).foregroundStyle(Theme.ink3).monospacedDigit().lineLimit(1).fixedSize()
            Button { withAnimation(Theme.anim(Theme.snappy, reduce: reduceMotion)) { player.nextRate() } } label: {
                Text(Self.rateLabel(player.rate)).uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink2).monospacedDigit()
                    .contentTransition(.numericText(value: Double(player.rate)))
                    .padding(.horizontal, 7).frame(minWidth: 34, minHeight: 22)
                    .background(Theme.ink.opacity(0.06), in: Capsule())
                    .contentShape(Capsule())
            }
            .buttonStyle(PressStyle())
            .help("빠르기")
        }
        .padding(.leading, 6).padding(.trailing, 10).padding(.vertical, 6)
        .animation(Theme.anim(Theme.settle, reduce: reduceMotion),
                   value: RecordingPlayer.speaker(in: meeting.transcript, atMs: player.positionMs))
        .task(id: player.meetingID) {
            guard mine, peaks == nil else { return }
            peaks = await Waveform.cachedPeaks(of: files())
        }
    }

    /// 1×, 1.5×, 2×
    static func rateLabel(_ r: Float) -> String {
        (r == r.rounded() ? String(Int(r)) : String(format: "%.1f", r)) + "×"
    }
}

/// The black round play / pause button.
struct PlayCircle: View {
    let playing: Bool
    var size: CGFloat = 30
    @Environment(\.reducesMotion) private var reduceMotion

    /// ▶ and ❚❚ both drawn, one shrinking away as the other grows in (two marks, not `.symbolEffect(.replace)`, which
    /// held the main thread on the collected page).
    var body: some View {
        ZStack {
            mark("play.fill", shown: !playing).offset(x: size * 0.035)
            mark("pause.fill", shown: playing)
        }
        .frame(width: size, height: size)
        .background(Theme.ink, in: Circle())
        .animation(Theme.anim(Theme.snappy, reduce: reduceMotion), value: playing)
    }

    private func mark(_ symbol: String, shown: Bool) -> some View {
        Image(systemName: symbol)
            .font(.system(size: size * 0.36, weight: .bold))
            .foregroundStyle(Theme.onStrong)
            .scaleEffect(shown || reduceMotion ? 1 : 0.5)
            .opacity(shown ? 1 : 0)
    }
}

/// How far the recording is; a click or a drag moves it there.
struct ProgressBar: View {
    let position: Int
    let duration: Int
    let seek: (Int) -> Void

    var body: some View {
        GeometryReader { g in
            let f = duration > 0 ? min(1, max(0, Double(position) / Double(duration))) : 0
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.ink.opacity(0.1)).frame(height: 4)
                Capsule().fill(Theme.ink).frame(width: max(4, g.size.width * f), height: 4)
            }
            .frame(maxHeight: .infinity)
            .contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 0).onChanged { v in
                guard duration > 0, g.size.width > 0 else { return }
                seek(Int(Double(duration) * min(1, max(0, v.location.x / g.size.width))))
            })
        }
        .frame(height: 18)
    }
}

/// What colours the waveform's steps: the meeting's lines and speakers, worked out again only when they change (the
/// bar redraws on every tick of the player).
struct WaveTints: Equatable {
    let meeting: SavedMeeting
    let people: [Person]
    let durationMs: Int

    func make() -> [Waveform.Tint] {
        let speakers = meeting.hasSpeakers
        var colors: [String: Int] = [:]
        return Waveform.tints(lines: meeting.transcript, unheard: meeting.unheard ?? [], durationMs: durationMs) { l in
            guard speakers, let k = meeting.owner(of: l) else { return nil }
            if let c = colors[k] { return c }
            let c = meeting.speakerColor(k, people: people)
            colors[k] = c
            return c
        }
    }
}

/// The waveform in place of the plain bar: one thin bar per stretch, as tall as it is loud, in the colour of who speaks
/// there. Heard bars are full, the rest light. A click or a drag moves the sound there, as on the plain bar.
struct WaveBar: View {
    let peaks: [Float]
    let tints: WaveTints
    let position: Int
    let duration: Int
    let seek: (Int) -> Void

    @State private var made: (key: WaveTints, tints: [Waveform.Tint])?

    private static let bar: CGFloat = 2, gap: CGFloat = 1.5

    var body: some View {
        let colours = made?.key == tints ? made!.tints : tints.make()
        GeometryReader { g in
            let f = duration > 0 ? min(1, max(0, Double(position) / Double(duration))) : 0
            Canvas { ctx, size in
                let count = max(1, Int((size.width + Self.gap) / (Self.bar + Self.gap)))
                let bars = Waveform.bars(peaks: peaks, tints: colours, count: count)
                for (j, b) in bars.enumerated() {
                    let x = CGFloat(j) * (Self.bar + Self.gap)
                    let h = max(2, CGFloat(b.height) * size.height)
                    let heard = (Double(j) + 0.5) / Double(bars.count) <= f
                    let rect = CGRect(x: x, y: (size.height - h) / 2, width: Self.bar, height: h)
                    ctx.fill(Path(roundedRect: rect, cornerRadius: Self.bar / 2), with: .color(Self.colour(b.tint, heard: heard)))
                }
            }
            .contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 0).onChanged { v in
                guard duration > 0, g.size.width > 0 else { return }
                seek(Int(Double(duration) * min(1, max(0, v.location.x / g.size.width))))
            })
        }
        .frame(height: 22)
        .onAppear { made = (tints, colours) }
        .onChange(of: tints) { _, t in made = (t, t.make()) }
    }

    private static func colour(_ t: Waveform.Tint, heard: Bool) -> Color {
        switch t {
        case .speaker(let i): return Theme.speaker(i).opacity(heard ? 1 : 0.32)
        case .voiceless: return Theme.ink.opacity(heard ? 0.9 : 0.2)
        case .quiet: return Theme.ink.opacity(heard ? 0.35 : 0.12)
        case .unheard: return Theme.ink.opacity(heard ? 0.25 : 0.1)
        }
    }
}

/// Space plays and pauses the meeting page's recording, unless someone is typing (a title, a speaker's name) or the
/// key goes to another window. Holding it down does not repeat.
struct SpaceToPlay: NSViewRepresentable {
    let action: () -> Void

    func makeNSView(context: Context) -> NSView {
        let v = NSView()
        context.coordinator.view = v
        context.coordinator.action = action
        context.coordinator.start()
        return v
    }

    func updateNSView(_ v: NSView, context: Context) { context.coordinator.action = action }

    static func dismantleNSView(_ v: NSView, coordinator: Coordinator) { coordinator.stop() }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator {
        weak var view: NSView?
        var action: () -> Void = {}
        private var monitor: Any?

        func start() {
            monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
                // only a field being typed in keeps the space: the page's text (selected, or clicked into on the 대화 기록
                // tab) is text too, and holding the keys there took the space away from the player
                guard let self, Self.isSpace(e), let w = self.view?.window, e.window === w, w.attachedSheet == nil,
                      !((w.firstResponder as? NSText)?.isEditable ?? false) else { return e }
                if !e.isARepeat { self.action() }
                return nil
            }
        }

        func stop() {
            if let monitor { NSEvent.removeMonitor(monitor) }
            monitor = nil
        }

        static func isSpace(_ e: NSEvent) -> Bool {
            e.keyCode == 49 && e.modifierFlags.intersection([.command, .control, .option, .shift]).isEmpty
        }
    }
}

/// Follows the player and tells the page which word is being heard, only when that changes, so the transcript does not
/// redraw on every tick.
struct CurrentWordWatch: View {
    let player: RecordingPlayer
    let lines: [TranscriptLine]
    let focus: PlaybackFocus

    var body: some View {
        Color.clear
            .onChange(of: player.positionMs, initial: true) { _, ms in
                focus.set(RecordingPlayer.currentWord(in: lines, atMs: ms).map { CurrentWord(line: $0.lineKey, word: $0.wordIndex) })
            }
            .onDisappear { focus.set(nil) }
    }
}
