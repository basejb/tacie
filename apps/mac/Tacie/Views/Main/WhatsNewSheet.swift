import SwiftUI

/// What's new over the main window after an update (WhatsNew): the version's main feature as a small moving scene, the
/// rest as plain lines, See what's new and Got it.
struct WhatsNewSheet: View {
    let entry: WhatsNewEntry
    let notes: () -> Void
    let done: () -> Void
    /// Asked once here (TacieAccount.asksNews): the box's state when the sheet came up decides whether it shows.
    @State private var asksNews = TacieAccount.shared.asksNews
    @State private var news = false

    var body: some View {
        VStack(spacing: 0) {
            LitHead(channels: EyeRig.target(for: .listening, remote: false), height: 34).padding(.bottom, 10)
            Text("Tacie \(AppUpdate.installedVersion ?? entry.version)").uiFont(12, weight: .semibold).foregroundStyle(Theme.ink3)
            Text(entry.title).uiFont(18, weight: .bold).tracking(-0.3).foregroundStyle(Theme.ink)
                .multilineTextAlignment(.center).padding(.top, 4)
            Text(entry.detail).uiFont(12.5).foregroundStyle(Theme.ink2).multilineTextAlignment(.center).lineSpacing(2)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 5).padding(.bottom, 16)
            scene
            if !entry.more.isEmpty {
                Rectangle().fill(Theme.hairline).frame(height: 1).padding(.top, 16).padding(.bottom, 10)
                VStack(alignment: .leading, spacing: 7) {
                    ForEach(entry.more, id: \.self) { line in
                        HStack(spacing: 8) {
                            Circle().fill(Theme.ink4).frame(width: 4, height: 4)
                            Text(line).uiFont(12.5).foregroundStyle(Theme.ink2)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 8)
            }
            if asksNews {
                NewsConsentBox(on: $news)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 8).padding(.top, 16)
            }
            HStack {
                Button(action: notes) {
                    Text("바뀐 점 전체 보기").uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink3)
                        .padding(.horizontal, 6).frame(height: 34).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                Spacer()
                Button {
                    // a tick says yes; closing without one is a no only for someone who never chose, so a two-year-old
                    // yes left unticked lapses too
                    if asksNews { Task { _ = await TacieAccount.shared.setNews(news) } }
                    done()
                } label: {
                    Text("확인했어요").uiFont(13, weight: .semibold).foregroundStyle(Theme.onStrong)
                        .padding(.horizontal, 16).frame(height: 34)
                        .background(Capsule().fill(Theme.ink))
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .keyboardShortcut(.defaultAction)
            }
            .padding(.top, 14)
        }
        .padding(.horizontal, 24).padding(.top, 22).padding(.bottom, 14)
        .frame(width: 440)
        .background(RoundedRectangle(cornerRadius: 22, style: .continuous).fill(Theme.paper))
        .overlay(RoundedRectangle(cornerRadius: 22, style: .continuous).strokeBorder(Theme.modalEdge))
        .shadow(color: Theme.modalShadow, radius: 30, y: 12)
        .appearanceScheme()
        // the account may answer after the sheet is up: the box comes in then, and stays once shown
        .onChange(of: TacieAccount.shared.info) { if !asksNews { asksNews = TacieAccount.shared.asksNews } }
    }

    @ViewBuilder private var scene: some View {
        switch entry.scene {
        case .liveTranscript: LiveTranscriptScene()
        case .teamSources: TeamSourcesScene()
        case .moreSources: TeamSourcesScene.moreSources
        case .islandTranscript: IslandTranscriptScene()
        case .mcp: MCPAskScene()
        }
    }
}

/// 0.1.45: a question typed in Claude, "Tacie에서 찾는 중" with the face, then the answer with a meeting and a Notion
/// page under it (the landing's MCP card, site/src/components/WhatsNew.astro). Plays on a loop; with 움직임 줄이기 it is
/// the finished answer.
struct MCPAskScene: View {
    /// 0 empty, 1 question, 2 찾는 중, 3 찾았어요 and the answer.
    @State private var step = 0
    @Environment(\.accessibilityReduceMotion) private var systemReduce
    @Environment(\.forcedReduceMotion) private var forcedReduce
    @Environment(\.textScale) private var scale
    private var reduceMotion: Bool { systemReduce || forcedReduce }

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(spacing: 6) {
                Image("ServiceClaude").resizable().scaledToFit().frame(width: 13 * scale, height: 13 * scale)
                Text(verbatim: "Claude").font(.system(size: 11.5 * scale, weight: .semibold)).foregroundStyle(Theme.ink2)
            }
            Text("온보딩 체크리스트는 왜 뺐어?").font(.system(size: 12.5 * scale, weight: .medium)).foregroundStyle(Theme.ink)
                .padding(.horizontal, 10).padding(.vertical, 6)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Theme.fillHover))
                .frame(maxWidth: .infinity, alignment: .trailing)
                .opacity(step >= 1 ? 1 : 0).offset(y: step >= 1 ? 0 : 6)
            HStack(spacing: 6) {
                LitHead(channels: EyeRig.target(for: step >= 3 ? .happy : .thinking, remote: false), height: 13)
                Text(step >= 3 ? "Tacie에서 찾았어요" : "Tacie에서 찾는 중")
                    .font(.system(size: 11 * scale, weight: .medium)).foregroundStyle(Theme.ink3)
            }
            .padding(.horizontal, 8).frame(height: 22)
            .background(Capsule().fill(Theme.fill))
            .opacity(step >= 2 ? 1 : 0).offset(y: step >= 2 ? 0 : 6)
            VStack(alignment: .leading, spacing: 7) {
                Text("체크리스트가 있을 때 이탈이 더 많아서, 3월 제품 리뷰 회의에서 빼기로 했어요.")
                    .font(.system(size: 12.5 * scale, weight: .medium)).foregroundStyle(Theme.ink)
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 5) {
                    chip(Image(systemName: "person.2.fill"), String(localized: "3월 14일 · 제품 리뷰 회의"))
                    chip(Image("ServiceNotion").resizable(), String(localized: "온보딩 개선안"))
                }
            }
            .opacity(step >= 3 ? 1 : 0).offset(y: step >= 3 ? 0 : 6)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 14).padding(.vertical, 12)
        .frame(height: 178 * min(scale, 1.3))
        .clipped()
        .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill.opacity(0.6)))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
        .task(id: reduceMotion) { await play() }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Claude에서 물어본 질문에 Tacie가 회의 기록과 노션 페이지를 찾아 답하는 모습")
    }

    private func chip(_ icon: Image, _ name: String) -> some View {
        HStack(spacing: 4) {
            icon.scaledToFit().font(.system(size: 8 * scale)).foregroundStyle(Theme.ink3)
                .frame(width: 11 * scale, height: 11 * scale)
            Text(name).font(.system(size: 10.5 * scale)).foregroundStyle(Theme.ink2).lineLimit(1)
        }
        .padding(.horizontal, 7).frame(height: 20)
        .background(Capsule().fill(Theme.fill))
    }

    private func play() async {
        if reduceMotion { step = 3; return }
        while !Task.isCancelled {
            withAnimation(Theme.enter) { step = 0 }
            for (s, wait) in [(1, 700), (2, 900), (3, 1500)] {
                guard await pause(.milliseconds(wait)) else { return }
                withAnimation(Theme.enter) { step = s }
            }
            guard await pause(.milliseconds(3600)) else { return }
        }
    }

    private func pause(_ d: Duration) async -> Bool {
        try? await Task.sleep(for: d)
        return !Task.isCancelled
    }
}

/// 0.1.41: 대화 기록 under 화면 위 Tacie. The pill's 대화 기록 is pressed, the panel opens, the meeting's lines come in
/// as bubbles (나 on the right), a question gets "찾는 중" under it and then its answer with two sources. Plays on a
/// loop; with 움직임 줄이기 it is the finished panel.
struct IslandTranscriptScene: View {
    private struct Line: Identifiable {
        let id: Int
        let who: String
        let speaker: Int?  // nil: 나
        let text: String
    }

    private static let lines: [Line] = [
        Line(id: 0, who: String(localized: "나"), speaker: nil, text: String(localized: "재시도를 다섯 번으로 늘리면 어때요?")),
        Line(id: 1, who: String(localized: "지윤"), speaker: 1, text: String(localized: "예전에 왜 세 번으로 정했죠?")),
    ]

    /// 0 closed, 1 opened, 2 and 3 one more line each, 4 찾는 중, 5 the answer.
    @State private var step = 0
    @Environment(\.accessibilityReduceMotion) private var systemReduce
    @Environment(\.forcedReduceMotion) private var forcedReduce
    @Environment(\.textScale) private var scale
    private var reduceMotion: Bool { systemReduce || forcedReduce }

    var body: some View {
        VStack(spacing: 8) {
            pill
            VStack(alignment: .leading, spacing: 8) {
                ForEach(Self.lines) { l in
                    if step >= 2 + l.id { bubble(l).transition(.opacity.combined(with: .offset(y: 4))) }
                }
                if step == 4 {
                    Text("찾는 중").font(.system(size: 11 * scale, weight: .medium)).foregroundStyle(Theme.ink3)
                        .transition(.opacity)
                }
                if step >= 5 { answer.transition(.opacity) }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
            .frame(height: 196 * min(scale, 1.3), alignment: .bottom)
            .clipped()
            .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill.opacity(0.6)))
            .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
            .opacity(step >= 1 ? 1 : 0)
            .scaleEffect(step >= 1 ? 1 : 0.97, anchor: .top)
        }
        .task(id: reduceMotion) { await play() }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("화면 위 Tacie 아래로 대화 기록이 펼쳐지고, 질문 아래에 답이 붙는 모습")
    }

    private var pill: some View {
        HStack(spacing: 7) {
            LitHead(channels: EyeRig.target(for: .listening, remote: true), height: 15)
            Circle().fill(Theme.live).frame(width: 5, height: 5)
            Text("원격 회의").font(.system(size: 11 * scale, weight: .medium)).foregroundStyle(Theme.ink2)
            HStack(spacing: 3) {
                Image(systemName: "text.alignleft").font(.system(size: 8.5 * scale, weight: .semibold))
                Text("대화 기록").font(.system(size: 10.5 * scale, weight: .semibold))
                Image(systemName: "chevron.down").font(.system(size: 7 * scale, weight: .bold))
                    .rotationEffect(.degrees(step >= 1 ? 180 : 0))
            }
            .foregroundStyle(step >= 1 ? Theme.onStrong : Theme.ink2)
            .padding(.horizontal, 7).frame(height: 19)
            .background(Capsule().fill(step >= 1 ? Theme.ink : Theme.fill))
        }
        .padding(.leading, 7).padding(.trailing, 5).frame(height: 28)
        .background(Capsule().fill(Theme.paper))
        .overlay(Capsule().strokeBorder(Theme.hairline))
        .shadow(color: .black.opacity(0.06), radius: 6, y: 2)
    }

    private func bubble(_ l: Line) -> some View {
        let me = l.speaker == nil
        return VStack(alignment: me ? .trailing : .leading, spacing: 2) {
            Text(l.who).font(.system(size: 10.5 * scale, weight: .semibold))
                .foregroundStyle(l.speaker.map { AnyShapeStyle(Theme.speaker($0)) } ?? AnyShapeStyle(Theme.ink3))
            Text(l.text).font(.system(size: 12 * scale, weight: .medium)).foregroundStyle(Theme.ink)
                .padding(.horizontal, 9).padding(.vertical, 5)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(me ? Theme.fillHover : Theme.fill))
        }
        .frame(maxWidth: .infinity, alignment: me ? .trailing : .leading)
    }

    private var answer: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("네 번째부터 카드사가 차단을 걸어서 세 번으로 정했어요.").font(.system(size: 12 * scale, weight: .medium))
                .foregroundStyle(Theme.ink).fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 5) {
                source("ServiceJira", String(localized: "PAY-88 재시도 횟수"))
                source("ServiceConfluence", String(localized: "결제 정책"))
            }
        }
        .padding(9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background {
            RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Theme.fill)
            RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.hairline)
        }
    }

    private func source(_ logo: String, _ name: String) -> some View {
        HStack(spacing: 4) {
            Image(logo).resizable().scaledToFit().frame(width: 11 * scale, height: 11 * scale)
            Text(name).font(.system(size: 10.5 * scale)).foregroundStyle(Theme.ink2).lineLimit(1)
        }
        .padding(.horizontal, 7).frame(height: 20)
        .background(Capsule().fill(Theme.fill))
    }

    private func play() async {
        if reduceMotion { step = 5; return }
        while !Task.isCancelled {
            step = 0
            guard await pause(.milliseconds(900)) else { return }
            for s in 1...5 {
                withAnimation(Theme.enter) { step = s }
                guard await pause(.milliseconds(s == 1 ? 700 : s == 4 ? 1100 : 1000)) else { return }
            }
            guard await pause(.milliseconds(2600)) else { return }
            withAnimation(.easeOut(duration: 0.3)) { step = 0 }
        }
    }

    private func pause(_ d: Duration) async -> Bool {
        try? await Task.sleep(for: d)
        return !Task.isCancelled
    }
}

/// 대화 기록 as it fills during a meeting (docs/25): words come in light, turn dark a moment later, and the speaker's
/// name shows when the sentence ends. Plays on a loop; with 움직임 줄이기 it is one still frame.
struct LiveTranscriptScene: View {
    private struct Line: Identifiable {
        let id: Int
        let who: String
        let speaker: Int
        let words: [String]
    }

    private static let script: [Line] = [
        Line(id: 0, who: String(localized: "민호"), speaker: 0, words: String(localized: "예전에 왜 재시도를 세 번으로 정했죠?").split(separator: " ").map(String.init)),
        Line(id: 1, who: String(localized: "지윤"), speaker: 1, words: String(localized: "PG사 제한 때문이었던 걸로 기억해요.").split(separator: " ").map(String.init)),
        Line(id: 2, who: String(localized: "서연"), speaker: 2, words: String(localized: "맞아요, 그때 문서에 적어 뒀어요.").split(separator: " ").map(String.init)),
    ]
    /// Lines that fit the stage; an older one leaves when a new one comes.
    private static let shown = 2

    /// The line being said, how many of its words are in, how many of them are final, and whether it has its name.
    @State private var line = 0
    @State private var heard = 0
    @State private var final = 0
    @State private var named = false
    @Environment(\.accessibilityReduceMotion) private var systemReduce
    @Environment(\.forcedReduceMotion) private var forcedReduce
    @Environment(\.textScale) private var scale
    private var reduceMotion: Bool { systemReduce || forcedReduce }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            ForEach(visible) { l in
                VStack(alignment: .leading, spacing: 2) {
                    Text(l.who).font(.system(size: 11.5 * scale, weight: .semibold)).foregroundStyle(Theme.speaker(l.speaker))
                        .opacity(l.id < line || named ? 1 : 0)
                    words(of: l).font(.system(size: 14 * scale)).lineSpacing(3)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .transition(.opacity)
            }
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 18).padding(.vertical, 16)
        .frame(height: 178 * min(scale, 1.3))
        .clipped()
        .overlay(alignment: .topTrailing) {
            HStack(spacing: 5) {
                Circle().fill(Theme.live).frame(width: 6, height: 6)
                Text("받아쓰는 중").font(.system(size: 11 * scale, weight: .semibold)).foregroundStyle(Theme.live)
            }
            .padding(.top, 12).padding(.trailing, 14)
        }
        .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill.opacity(0.6)))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
        .task(id: reduceMotion) { await play() }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("회의 중 대화 기록에 말이 바로 적히는 모습")
    }

    private var visible: [Line] {
        let upTo = Array(Self.script.prefix(line + 1))
        return Array(upTo.suffix(Self.shown))
    }

    /// Final words dark, the ones still changing light, as on the meeting page (TranscriptDoc).
    private func words(of l: Line) -> Text {
        let inCount = l.id < line ? l.words.count : heard
        let finalCount = l.id < line ? l.words.count : final
        var text = Text("")
        for (i, w) in l.words.prefix(inCount).enumerated() {
            text = text + Text((i == 0 ? "" : " ") + w)
                .foregroundColor(Theme.ink.opacity(i < finalCount ? 0.88 : 0.3))
        }
        return text
    }

    private func play() async {
        if reduceMotion {
            line = 1; heard = 3; final = 2; named = false
            return
        }
        while !Task.isCancelled {
            for i in Self.script.indices {
                withAnimation(Theme.enter) { line = i; heard = 0; final = 0; named = false }
                let count = Self.script[i].words.count
                for n in 1...count {
                    withAnimation(.easeOut(duration: 0.2)) {
                        heard = n
                        final = max(0, n - 2)
                    }
                    guard await pause(.milliseconds(230 + Int.random(in: 0...160))) else { return }
                }
                guard await pause(.milliseconds(250)) else { return }
                withAnimation(.easeOut(duration: 0.4)) { final = count }
                guard await pause(.milliseconds(350)) else { return }
                withAnimation(.easeOut(duration: 0.4)) { named = true }
                guard await pause(.milliseconds(900)) else { return }
            }
            guard await pause(.milliseconds(2200)) else { return }
        }
    }

    private func pause(_ d: Duration) async -> Bool {
        try? await Task.sleep(for: d)
        return !Task.isCancelled
    }
}

/// A question in the meeting and the answer card that comes up under it, its sources from Jira and Confluence
/// (0.1.39). The question is said, the answer fades in, then the two sources come in one after the other. Plays on a
/// loop; with 움직임 줄이기 it is the finished card.
struct TeamSourcesScene: View {
    private struct Source: Identifiable {
        let id: Int
        let logo: String
        let name: String
    }

    var question = String(localized: "결제 웹훅이 두 번 들어오는 버그, 누가 보고 있어요?")
    var answer = String(localized: "지윤 님이 맡았어요. 이벤트 id로 중복을 거르기로 했어요.")
    private var sources: [Source] = [
        Source(id: 0, logo: "ServiceJira", name: String(localized: "KAN-5 · 결제 웹훅이 두 번 들어오는 버그")),
        Source(id: 1, logo: "ServiceConfluence", name: String(localized: "결제 구조 결정 기록")),
    ]
    var label = String(localized: "회의 중 질문에 Jira 이슈와 Confluence 페이지를 근거로 답이 뜨는 모습")

    /// 0.1.40: the same answer with an Asana task, a Dropbox file and an Obsidian note under it.
    static var moreSources: TeamSourcesScene {
        var s = TeamSourcesScene()
        s.question = String(localized: "결제 웹훅 중복, 어떻게 하기로 했죠?")
        s.answer = String(localized: "이벤트 id로 중복을 거르기로 했어요. 지윤 님이 맡았어요.")
        s.sources = [
            Source(id: 0, logo: "ServiceAsana", name: String(localized: "결제 웹훅 중복 처리")),
            Source(id: 1, logo: "ServiceDropbox", name: String(localized: "결제 구조 설계.pdf")),
            Source(id: 2, logo: "ServiceObsidian", name: String(localized: "11월 결제 회의 메모")),
        ]
        s.label = String(localized: "회의 중 질문에 Asana task, Dropbox 파일, Obsidian 노트를 근거로 답이 뜨는 모습")
        return s
    }

    /// 0 nothing, 1 question, 2 answer, 3… one more source each.
    @State private var step = 0
    @Environment(\.accessibilityReduceMotion) private var systemReduce
    @Environment(\.forcedReduceMotion) private var forcedReduce
    @Environment(\.textScale) private var scale
    private var reduceMotion: Bool { systemReduce || forcedReduce }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text("민호").font(.system(size: 11.5 * scale, weight: .semibold)).foregroundStyle(Theme.speaker(0))
                Text(question).font(.system(size: 14 * scale)).foregroundStyle(Theme.ink.opacity(0.88))
                    .fixedSize(horizontal: false, vertical: true)
            }
            .opacity(step >= 1 ? 1 : 0)
            VStack(alignment: .leading, spacing: 8) {
                Text(answer).font(.system(size: 13.5 * scale, weight: .medium)).foregroundStyle(Theme.ink)
                    .fixedSize(horizontal: false, vertical: true)
                VStack(alignment: .leading, spacing: 5) {
                    ForEach(sources) { s in
                        HStack(spacing: 6) {
                            Image(s.logo).resizable().scaledToFit().frame(width: 13 * scale, height: 13 * scale)
                            Text(s.name).font(.system(size: 11.5 * scale)).foregroundStyle(Theme.ink2).lineLimit(1)
                        }
                        .padding(.horizontal, 8).frame(height: 24 * min(scale, 1.3))
                        .background(Capsule().fill(Theme.fill))
                        .opacity(step >= 3 + s.id ? 1 : 0)
                        .offset(y: step >= 3 + s.id ? 0 : 4)
                    }
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.paper))
            .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
            .opacity(step >= 2 ? 1 : 0)
            .offset(y: step >= 2 ? 0 : 6)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 18).padding(.vertical, 16)
        .frame(height: (sources.count > 2 ? 208 : 178) * min(scale, 1.3))
        .clipped()
        .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill.opacity(0.6)))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
        .task(id: reduceMotion) { await play() }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
    }

    private func play() async {
        let last = 2 + sources.count
        if reduceMotion { step = last; return }
        while !Task.isCancelled {
            withAnimation(Theme.enter) { step = 0 }
            for (s, wait) in [(1, 600), (2, 1100)] + (3...last).map({ ($0, $0 == 3 ? 700 : 500) }) {
                guard await pause(.milliseconds(wait)) else { return }
                withAnimation(Theme.enter) { step = s }
            }
            guard await pause(.milliseconds(3600)) else { return }
        }
    }

    private func pause(_ d: Duration) async -> Bool {
        try? await Task.sleep(for: d)
        return !Task.isCancelled
    }
}
