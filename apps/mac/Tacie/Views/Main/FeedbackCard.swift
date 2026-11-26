import Observation
import SwiftUI

/// Where the pointer is over the main window, in global coordinates; nil when it is outside. Only the feedback card's face
/// reads it, so a move redraws the face and nothing else.
@Observable
final class PointerTracker {
    var location: CGPoint?
}

/// The feedback card (FeedbackSurvey): Tacie's head looking at the pointer, "무엇이 더 좋아지면 좋겠어요?", six wishes to
/// pick, an optional line, 건너뛰기 and 보내기. On a send the form folds away, the head hops with happy eyes and a few
/// sparks, "고마워요" shows and slips away to a check (DoneMark), and the card leaves on its own.
struct FeedbackCard: View {
    enum Outcome: Equatable {
        case sent([FeedbackSurvey.Wish], String)
        case skipped
        case closed
    }

    let pointer: PointerTracker
    /// How many meetings have notes, for the line above the question.
    let notes: Int
    /// Called once: right away for a skip or a close, after the thank-you for a send.
    let finish: (Outcome) -> Void

    @State private var picked: [FeedbackSurvey.Wish]
    @State private var text: String
    @State private var sent: Bool
    /// Bumped on a send: the hop, the glow and the sparks run off it.
    @State private var cheer = 0
    @State private var sparksOut = false
    @FocusState private var writing: Bool
    @Environment(\.reducesMotion) private var reduceMotion

    /// `picked`, `text` and `sent` start the card further along, for renders.
    init(pointer: PointerTracker, notes: Int, picked: [FeedbackSurvey.Wish] = [], text: String = "", sent: Bool = false,
         finish: @escaping (Outcome) -> Void) {
        self.pointer = pointer
        self.notes = notes
        self.finish = finish
        _picked = State(initialValue: picked)
        _text = State(initialValue: text)
        _sent = State(initialValue: sent)
    }

    static let width: CGFloat = 340
    private static let head: CGFloat = 46

    private var canSend: Bool { !picked.isEmpty || !FeedbackSurvey.cleaned(text).isEmpty }

    var body: some View {
        VStack(spacing: 0) {
            hero
            if sent {
                DoneMark(text: String(localized: "고마워요. 다음 업데이트에 반영할게요"))
                    .frame(maxWidth: .infinity)
                    .padding(.top, 2)
                    .padding(.bottom, 18)
                    .transition(.opacity.combined(with: .offset(y: 8)))
            } else {
                form.transition(.opacity)
            }
        }
        .frame(width: Self.width)
        .foregroundStyle(Theme.ink)
        .background(Theme.raised, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(Theme.modalEdge))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(Theme.ink.opacity(0.06)))
        .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
        .shadow(color: Theme.modalShadow, radius: 22, y: 10)
        .onExitCommand { if !sent { finish(.closed) } }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Tacie 피드백")
    }

    // MARK: - head

    private var hero: some View {
        ZStack {
            // a warm wash under the head; the glow swells once on a send
            LinearGradient(colors: [Theme.eyeGlow.opacity(0.14), Theme.eyeGlow.opacity(0)], startPoint: .top, endPoint: .bottom)
                .mask(RadialGradient(colors: [.white, .white.opacity(0)], center: .init(x: 0.5, y: 0), startRadius: 0, endRadius: 200))
            sparks
            FaceFollowing(pointer: pointer, mood: sent ? .happy : .listening, height: Self.head)
                .keyframeAnimator(initialValue: Hop(), trigger: cheer) { face, hop in
                    face.scaleEffect(x: hop.sx, y: hop.sy, anchor: .bottom).offset(y: hop.y)
                        .background {
                            Ellipse().fill(RadialGradient(colors: [Theme.eyeGlow.opacity(0.55), Theme.eyeGlow.opacity(0)],
                                                          center: .center, startRadius: 0, endRadius: 70))
                                .frame(width: 150, height: 90)
                                .scaleEffect(hop.glowScale)
                                .opacity(hop.glow)
                        }
                } keyframes: { _ in
                    KeyframeTrack(\.y) {
                        CubicKeyframe(2, duration: 0.12)
                        SpringKeyframe(-14, duration: 0.2, spring: .snappy)
                        SpringKeyframe(0, duration: 0.42, spring: .bouncy)
                    }
                    KeyframeTrack(\.sx) {
                        CubicKeyframe(1.07, duration: 0.12)
                        CubicKeyframe(0.95, duration: 0.18)
                        SpringKeyframe(1, duration: 0.4, spring: .bouncy)
                    }
                    KeyframeTrack(\.sy) {
                        CubicKeyframe(0.9, duration: 0.12)
                        CubicKeyframe(1.06, duration: 0.18)
                        SpringKeyframe(1, duration: 0.4, spring: .bouncy)
                    }
                    KeyframeTrack(\.glow) {
                        LinearKeyframe(0, duration: 0.1)
                        CubicKeyframe(1, duration: 0.35)
                        CubicKeyframe(0, duration: 1.1)
                    }
                    KeyframeTrack(\.glowScale) {
                        LinearKeyframe(0.6, duration: 0.1)
                        CubicKeyframe(1.5, duration: 1.45)
                    }
                }
                .padding(.top, 6)
            if !sent {
                Button { finish(.closed) } label: {
                    Image(systemName: "xmark").font(.system(size: 10.5, weight: .bold)).foregroundStyle(Theme.ink2)
                        .frame(width: 24, height: 24)
                        .background(Theme.raised.opacity(0.7), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("닫기")
                .accessibilityLabel("닫기")
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
                .padding(10)
                .transition(.opacity)
            }
        }
        .frame(height: 92)
    }

    private struct Hop {
        var y: CGFloat = 0
        var sx: CGFloat = 1
        var sy: CGFloat = 1
        var glow: Double = 0
        var glowScale: CGFloat = 0.6
    }

    /// A few warm dots that burst up from behind the head on a send.
    private var sparks: some View {
        ZStack {
            ForEach(0..<9, id: \.self) { i in
                let a = -Double.pi / 2 + (Double(i) / 8 - 0.5) * 2.6
                let d = 52.0 + Double((i * 7) % 5) * 6
                Circle()
                    .fill(i % 3 == 0 ? Theme.eyeGlow : Theme.eyeCore)
                    .frame(width: i % 3 == 0 ? 6 : 4, height: i % 3 == 0 ? 6 : 4)
                    .offset(x: sparksOut ? cos(a) * d * 1.3 : 0, y: sparksOut ? sin(a) * d : 0)
                    .opacity(sparksOut ? 0 : (cheer > 0 ? 1 : 0))
                    .animation(.easeOut(duration: 0.9).delay(0.16 + Double(i % 4) * 0.03), value: sparksOut)
            }
        }
        .offset(y: 4)
        .allowsHitTesting(false)
    }

    // MARK: - form

    private var form: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Tacie로 회의 정리 \(notes)번째예요").uiFont(12).foregroundStyle(Theme.ink3)
            Text("무엇이 더 좋아지면 좋겠어요?").uiFont(15, weight: .semibold).padding(.top, 2)
            Wrap(spacing: 6) {
                ForEach(FeedbackSurvey.Wish.allCases) { w in chip(w) }
            }
            .padding(.top, 12)
            field.padding(.top, 10)
            HStack(spacing: 5) {
                Image(systemName: "lock").font(.system(size: 10, weight: .medium))
                Text("적은 글은 Tacie 팀만 봐요. 회의 내용은 같이 가지 않아요.").uiFont(11.5)
            }
            .foregroundStyle(Theme.ink3)
            .padding(.top, 8)
            HStack {
                Button("건너뛰기") { finish(.skipped) }
                    .buttonStyle(.plain).uiFont(13).foregroundStyle(Theme.ink3)
                Spacer()
                Button(action: send) {
                    HStack(spacing: 5) {
                        Text("보내기").uiFont(13, weight: .semibold)
                        Image(systemName: "arrow.right").font(.system(size: 11, weight: .semibold))
                    }
                    .foregroundStyle(Theme.onStrong)
                    .padding(.horizontal, 14).frame(height: 32)
                    .background(Theme.ink, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                    .opacity(canSend ? 1 : 0.35)
                    .contentShape(Rectangle())
                }
                .buttonStyle(CardPress())
                .disabled(!canSend)
                .keyboardShortcut(.return, modifiers: .command)
            }
            .padding(.top, 12)
        }
        .padding(.horizontal, 18)
        .padding(.top, 4)
        .padding(.bottom, 16)
    }

    private func chip(_ w: FeedbackSurvey.Wish) -> some View {
        let on = picked.contains(w)
        return Button {
            withAnimation(Theme.anim(Theme.snappy, reduce: reduceMotion)) {
                if on { picked.removeAll { $0 == w } } else { picked.append(w) }
            }
        } label: {
            HStack(spacing: 4) {
                if on { Image(systemName: "checkmark").font(.system(size: 9.5, weight: .bold)).transition(.scale.combined(with: .opacity)) }
                Text(w.label).uiFont(12.5)
            }
            .foregroundStyle(on ? Theme.onStrong : Theme.ink)
            .padding(.horizontal, 11).frame(height: 28)
            .background(on ? Theme.ink : Theme.fill, in: Capsule())
            .contentShape(Capsule())
        }
        .buttonStyle(CardPress())
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    private var field: some View {
        ZStack(alignment: .topLeading) {
            TextEditor(text: $text)
                .font(.system(size: 13))
                .scrollContentBackground(.hidden)
                .focused($writing)
                .padding(.horizontal, 7).padding(.vertical, 7)
            if text.isEmpty {
                Text("더 하고 싶은 말이 있으면 적어 주세요").uiFont(13).foregroundStyle(Theme.ink3)
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    .allowsHitTesting(false)
            }
        }
        .frame(height: 72)
        .background(writing ? Theme.raised : Theme.fill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(writing ? Theme.ink4 : .clear, lineWidth: 1.5))
        .overlay(alignment: .bottomTrailing) {
            Text(verbatim: "\(text.count)/\(FeedbackSurvey.textMax)").uiFont(11, digits: true).foregroundStyle(Theme.ink4)
                .padding(.trailing, 10).padding(.bottom, 7)
        }
        .onChange(of: text) { if text.count > FeedbackSurvey.textMax { text = String(text.prefix(FeedbackSurvey.textMax)) } }
        .animation(Theme.anim(Theme.snappy, reduce: reduceMotion), value: writing)
    }

    private func send() {
        guard canSend, !sent else { return }
        let outcome = Outcome.sent(picked, FeedbackSurvey.cleaned(text))
        writing = false
        withAnimation(Theme.anim(Theme.grow, reduce: reduceMotion)) { sent = true }
        if !reduceMotion {
            cheer += 1
            sparksOut = false
            DispatchQueue.main.async { sparksOut = true }
        }
        // the thank-you settles to its check at 1.4 s (DoneMark); the card leaves a beat after
        DispatchQueue.main.asyncAfter(deadline: .now() + 3.2) { finish(outcome) }
    }
}

/// The head, looking where the pointer is (the landing's gaze: tanh of the distance, 340 pt across, 300 pt down).
struct FaceFollowing: View {
    let pointer: PointerTracker
    let mood: EyeMood
    let height: CGFloat
    @State private var frame: CGRect = .zero

    var body: some View {
        CueEyes(mood: mood, height: height, gaze: gaze)
            .background(GeometryReader { g in
                Color.clear
                    .onAppear { frame = g.frame(in: .global) }
                    .onChange(of: g.frame(in: .global)) { _, f in frame = f }
            })
    }

    private var gaze: CGPoint? {
        guard let p = pointer.location, frame != .zero else { return nil }
        let dx = p.x - frame.midX, dy = p.y - frame.midY
        return CGPoint(x: tanh(dx / 340), y: tanh(dy / 300))
    }
}

/// A press that sinks a little and springs back.
struct CardPress: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.96 : 1)
            .animation(.spring(duration: 0.25, bounce: 0.4), value: configuration.isPressed)
    }
}
