import SwiftUI

/// What Getting started shows about connections (docs/11 14절): what is connected, what could be, and how many questions
/// went without an answer.
struct FirstStepsInfo: Equatable {
    struct Connected: Equatable {
        let tool: ATool
        let label: String
    }
    var connected: [Connected] = []
    var unconnected: [ATool] = []
    /// Saved questions without an answer (kept without a connection, or nothing found).
    var unanswered = 0

    static func name(_ tool: ATool) -> String {
        switch tool {
        case .github: "GitHub"
        case .notion: String(localized: "노션")
        case .linear: "Linear"
        case .gdrive: "Google Drive"
        case .atlassian: "Jira · Confluence"
        case .asana: "Asana"
        case .dropbox: "Dropbox"
        case .notes: "Obsidian"
        }
    }

    static func logo(_ tool: ATool) -> String {
        switch tool {
        case .github: "ServiceGitHub"
        case .notion: "ServiceNotion"
        case .linear: "ServiceLinear"
        case .gdrive: "ServiceGoogleDrive"
        case .atlassian: "ServiceJira"
        case .asana: "ServiceAsana"
        case .dropbox: "ServiceDropbox"
        case .notes: "ServiceObsidian"
        }
    }
}

struct FirstStepsActions {
    var start: () -> Void = {}
    /// A row's ✕: this step leaves the list.
    var skip: (FirstStep) -> Void = { _ in }
    var connect: (ATool, Bool) -> Void = { _, _ in }
    var hide: () -> Void = {}
}

/// The list itself: on the empty main page, and in the sidebar footer's popover.
struct FirstStepsCard: View {
    let state: FirstStepsState
    let info: FirstStepsInfo
    let actions: FirstStepsActions
    var width: CGFloat = 560
    var framed = true

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Text("처음 써 보기").uiFont(13, weight: .semibold).foregroundStyle(Theme.ink)
                Text(verbatim: "\(FirstSteps.count(state))/\(FirstSteps.total(state))").uiFont(12, weight: .medium, digits: true).foregroundStyle(Theme.ink3)
                Spacer()
                FirstStepsBar(done: FirstSteps.count(state), total: FirstSteps.total(state))
                Button(action: actions.hide) {
                    Image(systemName: "xmark").uiFont(9.5, weight: .bold).foregroundStyle(Theme.ink3)
                        .frame(width: 22, height: 22).background(Theme.fill, in: Circle())
                }
                .buttonStyle(.plain)
                .help("숨기기")
                .accessibilityLabel("처음 써 보기 숨기기")
            }
            .padding(.top, 14).padding(.bottom, 2)
            ForEach(Array(FirstSteps.steps(state).enumerated()), id: \.element) { i, step in
                if i > 0 { line }
                row(step)
            }
        }
        .padding(.horizontal, 18).padding(.bottom, 4)
        .frame(width: width, alignment: .leading)
        .background {
            if framed { RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.ink.opacity(0.1)) }
        }
    }

    private var line: some View { Rectangle().fill(Theme.hairline).frame(height: 1) }

    @ViewBuilder private func row(_ step: FirstStep) -> some View {
        let done = FirstSteps.isDone(state, step)
        let skip = { actions.skip(step) }
        switch step {
        case .source:
            SourceStep(done: done, info: info, connect: actions.connect, skip: skip)
        case .meeting:
            StepRow(done: done, title: String(localized: "첫 회의 시작하기"),
                    sub: state.done[.meeting].map { String(localized: "\(Self.when($0)) 회의에서 했어요") }
                        ?? String(localized: "다음 회의 때 ⌃⌥⌘M을 누르면 Tacie가 들어요"),
                    button: String(localized: "회의 시작"), action: actions.start, skip: skip)
        case .card:
            StepRow(done: done, title: String(localized: "답 카드 열어 보기"),
                    sub: done ? String(localized: "근거 문서를 열어 봤어요") : String(localized: "회의 중 질문에 답이 뜨면 카드를 눌러 근거를 확인해요"),
                    button: nil, action: {}, skip: skip)
        case .ask:
            StepRow(done: done, title: String(localized: "회의 중 직접 물어보기"),
                    sub: done ? String(localized: "회의 중에 물어봤어요") : String(localized: "회의 중에 ⌃⌥⌘A를 누르고 물어보면 바로 찾아요"),
                    button: nil, action: {}, skip: skip)
        }
    }

    /// "오늘 오후 2:00", "11월 2일 오후 2:00".
    static func when(_ d: Date, now: Date = .now) -> String {
        let f = DateFormatter()
        f.locale = AppLocale.locale
        let today = Calendar.current.isDate(d, inSameDayAs: now)
        if AppLocale.isKorean {
            f.dateFormat = today ? "'오늘' a h:mm" : "M월 d일 a h:mm"
            return f.string(from: d)
        }
        // English: "today 2:00 PM", "Nov 2 at 2:00 PM" in the system's order
        f.dateFormat = AppLocale.pattern(ko: today ? "a h:mm" : "M월 d일 a h:mm", template: today ? "jmm" : "MMMdjmm")
        return today ? String(localized: "firstSteps.today", defaultValue: "오늘") + " " + f.string(from: d) : f.string(from: d)
    }
}

/// Four segments, green for each step done.
struct FirstStepsBar: View {
    let done: Int
    var total = 4
    var segment: CGFloat = 18
    var body: some View {
        HStack(spacing: 4) {
            ForEach(0..<total, id: \.self) { i in
                Capsule().fill(i < done ? Theme.live : Theme.ink.opacity(0.1)).frame(width: segment, height: 4)
            }
        }
        .accessibilityHidden(true)
    }
}

private struct StepCheck: View {
    let done: Bool
    var body: some View {
        ZStack {
            if done {
                Circle().fill(Theme.live)
                Image(systemName: "checkmark").font(.system(size: 9.5, weight: .bold)).foregroundStyle(.white)
            } else {
                Circle().strokeBorder(Theme.ink.opacity(0.25), lineWidth: 1.5)
            }
        }
        .frame(width: 18, height: 18)
        .accessibilityLabel(done ? String(localized: "했어요") : String(localized: "아직이에요"))
    }
}

private struct StepRow: View {
    let done: Bool
    let title: String
    let sub: String
    let button: String?
    let action: () -> Void
    let skip: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            StepCheck(done: done)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).uiFont(13.5, weight: .semibold).foregroundStyle(done ? Theme.ink.opacity(0.45) : Theme.ink)
                Text(sub).uiFont(12).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            if let button, !done {
                Button(action: action) {
                    Text(button).uiFont(12, weight: .semibold)
                        .foregroundStyle(Theme.onStrong)
                        .padding(.horizontal, 12).frame(height: 26)
                        .background(Theme.ink, in: Capsule())
                        .contentShape(Capsule())
                }
                .buttonStyle(PressStyle())
            }
            if !done { SkipButton(action: skip) }
        }
        .padding(.vertical, 11)
        .frame(minHeight: 56)
    }
}

/// 팀 자료 연결하기: carries what the onboarding connected and offers the rest.
private struct SourceStep: View {
    let done: Bool
    let info: FirstStepsInfo
    let connect: (ATool, Bool) -> Void
    let skip: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            StepCheck(done: done).padding(.top, 2)
            VStack(alignment: .leading, spacing: 8) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("팀 자료 연결하기").uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink)
                    Text(info.connected.isEmpty
                         ? String(localized: "연결해야 회의 중 질문에 답할 수 있어요. 그전까지는 질문을 적어 두고, 연결하면 다시 찾아요")
                         : String(localized: "연결한 곳이 많을수록 더 많은 질문에 답해요"))
                        .uiFont(12).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                }
                if !info.connected.isEmpty {
                    Wrap(spacing: 6) {
                        ForEach(info.connected, id: \.label) { c in
                            ToolChip(tool: c.tool, label: c.label, connected: true) { connect(c.tool, true) }
                        }
                    }
                }
                if info.unanswered > 0 && !info.unconnected.isEmpty {
                    HStack(spacing: 6) {
                        Image(systemName: "questionmark.bubble").uiFont(11, weight: .semibold)
                        Text("지난 회의에서 답을 못 찾은 질문 \(info.unanswered)개 · 연결하면 다시 찾아 봐요").uiFont(12, weight: .medium)
                    }
                    .foregroundStyle(Theme.ink2)
                }
                if !info.unconnected.isEmpty {
                    VStack(alignment: .leading, spacing: 6) {
                        if !info.connected.isEmpty {
                            Text("더 연결할 수 있는 곳").uiFont(11, weight: .semibold).foregroundStyle(Theme.ink3)
                        }
                        Wrap(spacing: 6) {
                            ForEach(info.unconnected, id: \.self) { tool in
                                ToolChip(tool: tool, label: FirstStepsInfo.name(tool), connected: false) { connect(tool, false) }
                            }
                        }
                    }
                }
            }
            Spacer(minLength: 0)
            if !done { SkipButton(action: skip).padding(.top, 2) }
        }
        .padding(.vertical, 14)
    }
}

/// One tool to connect (+) or connected (✓, opens 설정 → 연결): 처음 써 보기 and a meeting's answers use it.
struct ToolChip: View {
    @Environment(\.colorScheme) private var scheme
    let tool: ATool
    let label: String
    let connected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                Image(ServiceLogo.name(FirstStepsInfo.logo(tool), scheme)).resizable().scaledToFit().frame(width: 14, height: 14)
                Text(label).uiFont(12, weight: .medium).foregroundStyle(connected ? Theme.ink2 : Theme.ink).lineLimit(1)
                Image(systemName: connected ? "checkmark" : "plus").uiFont(9, weight: .bold)
                    .foregroundStyle(connected ? AnyShapeStyle(Theme.live) : AnyShapeStyle(Theme.ink3))
            }
            .padding(.horizontal, 10).frame(height: 28)
            .background(connected ? .clear : Theme.fill, in: Capsule())
            .overlay(Capsule().strokeBorder(Theme.ink.opacity(connected ? 0.1 : 0)))
            .contentShape(Capsule())
        }
        .buttonStyle(PressStyle())
        .help(connected ? String(localized: "설정 → 연결에서 봐요") : String(localized: "\(FirstStepsInfo.name(tool)) 연결"))
    }
}

/// ✕ at a row's end, like 남은 일's: the step leaves the list for good.
private struct SkipButton: View {
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            Image(systemName: "xmark").uiFont(9.5, weight: .bold).foregroundStyle(Theme.ink3)
                .frame(width: 22, height: 22).background(Theme.fill, in: Circle())
        }
        .buttonStyle(.plain)
        .help("목록에서 빼기")
        .accessibilityLabel("목록에서 빼기")
    }
}

/// The sidebar footer once there is a meeting: one row like 남은 일, the list in a popover, and the finish once.
struct FirstStepsFooterRow: View {
    let state: FirstStepsState
    let info: FirstStepsInfo
    let actions: FirstStepsActions
    let finished: () -> Void
    @State private var open = false

    var body: some View {
        if FirstSteps.showsFinish(state) {
            HStack(spacing: 8) {
                DoneMark(text: String(localized: "처음 써 보기를 다 했어요"))
                Spacer()
            }
            .padding(.horizontal, 14).frame(height: 36)
            .task {
                try? await Task.sleep(for: .seconds(2))
                withAnimation(Theme.settle) { finished() }
            }
        } else {
            Button { open.toggle() } label: {
                HStack(spacing: 8) {
                    FirstStepsBar(done: FirstSteps.count(state), total: FirstSteps.total(state), segment: 10)
                    Text("처음 써 보기 \(FirstSteps.count(state))/\(FirstSteps.total(state))").uiFont(12.5, weight: .medium).foregroundStyle(Theme.ink2)
                    Spacer()
                    Image(systemName: "chevron.up").uiFont(10, weight: .semibold).foregroundStyle(Theme.ink3)
                }
                .padding(.horizontal, 14).frame(height: 36).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .popover(isPresented: $open, arrowEdge: .top) {
                FirstStepsCard(state: state, info: info, actions: FirstStepsActions(
                    start: { open = false; actions.start() },
                    skip: actions.skip,
                    connect: { t, c in open = false; actions.connect(t, c) },
                    hide: { open = false; actions.hide() }), width: 500, framed: false)
                    .padding(.vertical, 4)
                    .appearanceScheme()
            }
        }
    }
}
