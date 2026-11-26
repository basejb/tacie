import SwiftUI

/// The connect card (ConnectPrompt): Tacie's head looking at the pointer, how many questions went unanswered with only
/// past meetings to search, the tools that could answer them, Connect your services and Use notes only. Use notes only
/// settles to a check (DoneMark) and the card leaves on its own; Connect your services and the close button end it at
/// once.
struct ConnectPromptCard: View {
    let pointer: PointerTracker
    let counts: ConnectPrompt.Counts
    /// Called once: at once for connect or close, after the check for 기록만.
    let finish: (ConnectPrompt.Choice) -> Void

    @State private var kept: Bool
    @Environment(\.reducesMotion) private var reduceMotion

    /// `kept` starts the card on its check, for renders.
    init(pointer: PointerTracker, counts: ConnectPrompt.Counts, kept: Bool = false, finish: @escaping (ConnectPrompt.Choice) -> Void) {
        self.pointer = pointer
        self.counts = counts
        self.finish = finish
        _kept = State(initialValue: kept)
    }

    private static let head: CGFloat = 46

    /// The tools on the chips, as 설정 → 연결 names them.
    private static let tools: [(logo: String, name: String)] = [
        ("ServiceGitHub", "GitHub"), ("ServiceNotion", String(localized: "노션")), ("ServiceLinear", "Linear"),
        ("ServiceJira", "Jira · Confluence"), ("ServiceGoogleDrive", "Google Drive"),
    ]

    var body: some View {
        VStack(spacing: 0) {
            hero
            if kept {
                DoneMark(text: String(localized: "이제 기록만 해요. 다음 회의를 시작할 때 다시 바꿀 수 있어요"))
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 4)
                    .padding(.horizontal, 18)
                    .padding(.top, 2)
                    .padding(.bottom, 18)
                    .transition(.opacity.combined(with: .offset(y: 8)))
            } else {
                form.transition(.opacity)
            }
        }
        .frame(width: FeedbackCard.width)
        .foregroundStyle(Theme.ink)
        .background(Theme.raised, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(Theme.modalEdge))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(Theme.ink.opacity(0.06)))
        .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
        .shadow(color: Theme.modalShadow, radius: 22, y: 10)
        .onExitCommand { if !kept { finish(.close) } }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("팀 자료 연결 안내")
    }

    private var hero: some View {
        ZStack {
            // the same warm wash under the head as the feedback card
            LinearGradient(colors: [Theme.eyeGlow.opacity(0.14), Theme.eyeGlow.opacity(0)], startPoint: .top, endPoint: .bottom)
                .mask(RadialGradient(colors: [.white, .white.opacity(0)], center: .init(x: 0.5, y: 0), startRadius: 0, endRadius: 200))
            FaceFollowing(pointer: pointer, mood: kept ? .happy : .listening, height: Self.head)
                .padding(.top, 6)
            if !kept {
                Button { finish(.close) } label: {
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

    private var form: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("회의를 \(counts.meetings)번 마쳤어요").uiFont(12).foregroundStyle(Theme.ink3)
            Text("지금은 지난 회의에서만 답을 찾아요").uiFont(15, weight: .semibold).padding(.top, 2)
            Text("지난 회의에서 나온 질문 \(counts.unanswered)개는 답을 찾지 못했어요. 팀 자료를 연결하면 그 자료에서도 찾아요.")
                .uiFont(12.5).foregroundStyle(Theme.ink2).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            Wrap(spacing: 6) {
                ForEach(Self.tools, id: \.logo) { chip($0.logo, $0.name) }
            }
            .padding(.top, 12)
            HStack {
                Button(String(localized: "기록만 할게요"), action: keep)
                    .buttonStyle(.plain).uiFont(13).foregroundStyle(Theme.ink3)
                Spacer()
                Button { finish(.connect) } label: {
                    Text("연결하러 가기").uiFont(13, weight: .semibold)
                        .foregroundStyle(Theme.onStrong)
                        .padding(.horizontal, 15).frame(height: 32)
                        .background(Theme.ink, in: Capsule())
                        .contentShape(Capsule())
                }
                .buttonStyle(CardPress())
                .keyboardShortcut(.return, modifiers: [])
            }
            .padding(.top, 16)
            Text("기록만 모드에서는 답 카드를 띄우지 않고 녹음, 대화 기록, 회의 정리만 해요.")
                .uiFont(11.5).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 10)
        }
        .padding(.horizontal, 18)
        .padding(.top, 4)
        .padding(.bottom, 16)
    }

    /// A tool's mark on its light tile (the official marks are drawn for a light ground) and its name.
    private func chip(_ logo: String, _ name: String) -> some View {
        HStack(spacing: 5) {
            Image(logo).resizable().scaledToFit().frame(width: 11, height: 11)
                .frame(width: 16, height: 16)
                .background(Theme.logoTile, in: RoundedRectangle(cornerRadius: 4, style: .continuous))
            Text(verbatim: name).uiFont(12, weight: .medium)
        }
        .padding(.leading, 5).padding(.trailing, 10).frame(height: 26)
        .background(Theme.fill, in: Capsule())
        .accessibilityElement(children: .combine)
    }

    private func keep() {
        guard !kept else { return }
        withAnimation(Theme.anim(Theme.grow, reduce: reduceMotion)) { kept = true }
        // the words settle to their check at 1.4 s (DoneMark); the card leaves a beat after
        DispatchQueue.main.asyncAfter(deadline: .now() + 3.2) { finish(.recordOnly) }
    }
}
