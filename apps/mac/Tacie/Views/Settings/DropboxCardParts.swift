import SwiftUI

/// What a Dropbox card says (docs/33 5절, 5-1절), the same in the onboarding and in Settings → Connections. Built from
/// plain values so the render tests can draw every state without an engine (DropboxCardRenderTests).
@MainActor
struct DropboxStatus {
    enum State: Equatable {
        case off  // not connected: 연결
        case waiting  // the approval page is open
        case failed(String)  // denied, expired, file access not allowed…: 다시 연결
        case lost  // Dropbox refused the refresh token: 다시 연결 (found files stay for seven days)
        case unreadable  // the keychain could not be read: whether there is a connection is unknown, no button
        case syncing(String)  // "Dropbox 파일 받는 중 · 40%"
        case connected
    }

    let link: DropboxLink
    let info: IndexState.DropboxInfo?
    /// The running sync's step line (IndexKeeper.stepLine), nil when no sync runs.
    var stepLine: String? = nil
    var now = Date()

    var state: State {
        switch link.phase {
        case .waiting: return .waiting
        case .failed(let why) where !link.connected: return .failed(why)
        default: break
        }
        if link.unreadable { return .unreadable }
        guard link.connected else { return .off }
        // the engine's lost_since: the connection is lost even if this app has not seen it yet
        if link.lost || info?.lostSince != nil { return .lost }
        // a sync with nothing from Dropbox yet is its first one
        if let line = stepLine, info?.files == nil { return .syncing(line) }
        return .connected
    }

    /// "연결됨 · me@example.com · 파일 1,240개", or why not (SettingsText.dropboxLine).
    var line: String {
        if link.unreadable { return SettingsText.dropboxUnreadable }
        return SettingsText.dropboxLine(info, lost: link.lost, lostAt: link.lostAt, now: now)
    }

    /// Who is connected, for the tile's first line.
    var name: String { link.account.flatMap { $0.isEmpty ? nil : $0 } ?? String(localized: "연결됨") }

    /// "파일 1,240개" when there is something found (the tile's second line).
    var counts: String? {
        guard let files = info?.files, files > 0 else { return nil }
        return String(localized: "파일 \(files)개")
    }

    /// The first fetch is not all in yet (a big Dropbox comes over several runs).
    var growing: Bool { info?.initialDone == false }

    /// "마지막으로 받은 때: 10월 31일 오후 3:20" when Dropbox could not be reached on the last sync.
    var lastFetched: String? {
        guard let info, ["network", "busy", "down"].contains(info.errorKind ?? ""), (info.files ?? 0) > 0,
              let at = info.fetchedAt.flatMap(Self.iso) else { return nil }
        return String(localized: "마지막으로 받은 때: \(at.formatted(.dateTime.month().day().hour().minute().locale(AppLocale.locale)))")
    }

    private static func iso(_ s: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f.date(from: s) ?? ISO8601DateFormatter().date(from: s)
    }
}

extension DropboxStatus.State {
    var stage: SourceStage {
        switch self {
        case .off: .off
        case .waiting: .waiting
        case .failed: .failed
        case .lost, .unreadable: .lost
        case .syncing: .syncing
        case .connected: .connected
        }
    }
}

/// What the Dropbox views can do; the keeper-backed wrappers fill it (DropboxCard.swift).
struct DropboxActions {
    var connect: () -> Void = {}
    var cancel: () -> Void = {}
    var pickFolders: () -> Void = {}
    var disconnect: () -> Void = {}
}

/// The onboarding's Dropbox tile (SourcesStep), after Asana's.
struct DropboxTileBody: View {
    let status: DropboxStatus
    var actions = DropboxActions()

    var body: some View {
        let state = status.state
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                // on the logo tile's own light ground, like the settings card, so the blue mark keeps its contrast in dark mode
                Image("ServiceDropbox").resizable().scaledToFit().frame(width: 13, height: 13)
                    .frame(width: 20, height: 20)
                    .background(Theme.logoTile, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 5, style: .continuous).stroke(Theme.ink.opacity(0.1)))
                VStack(alignment: .leading, spacing: 1) {
                    Text(verbatim: "Dropbox").uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink)
                    Text("Paper 문서와 파일").uiFont(11.5).foregroundStyle(Theme.ink3).lineLimit(1)
                }
                Spacer(minLength: 0)
                if state == .connected {
                    TileMenu { Button("폴더 고르기", action: actions.pickFolders) }
                }
            }
            switch state {
            case .off:
                Button("연결", action: actions.connect).buttonStyle(InkButtonStyle())
            case .waiting:
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.mini)
                        Text("브라우저에서 기다리는 중").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                        Spacer(minLength: 0)
                        Button("취소", action: actions.cancel).buttonStyle(.plain)
                            .uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink3)
                    }
                    Text("Dropbox에서 허용해 주세요. Tacie는 읽기만 해요.")
                        .uiFont(11).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                }
            case .failed(let why):
                problem(why)
            case .lost:
                problem(status.line)
            case .unreadable:
                Text(status.line).uiFont(11.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            case .syncing(let line):
                HStack(spacing: 6) {
                    ProgressView().controlSize(.mini)
                    Text("동기화 중 · \(line)").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                        .lineLimit(2).fixedSize(horizontal: false, vertical: true)
                }
            case .connected:
                if let counts = status.counts {
                    TileStatusLines(name: status.name,
                                    detail: status.growing ? String(localized: "\(counts) · 더 받는 중") : counts,
                                    growing: status.growing)
                } else {
                    // nothing to fetch, or not reached: the line needs its whole width
                    VStack(alignment: .leading, spacing: 3) {
                        TileStatusLines(name: status.name)
                        Text(status.line).uiFont(11).foregroundStyle(Theme.ink3)
                            .fixedSize(horizontal: false, vertical: true)
                            .padding(.leading, 17)
                    }
                }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 100, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.paper))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
            .strokeBorder(state == .connected ? Theme.ink.opacity(0.35) : Theme.ink.opacity(0.1)))
    }

    private func problem(_ why: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(why).uiFont(11.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            Button("다시 연결", action: actions.connect).buttonStyle(InkButtonStyle())
        }
    }
}

/// 설정 → 연결's Dropbox card, under Asana's.
struct DropboxCardBody: View {
    let status: DropboxStatus
    var actions = DropboxActions()
    @State private var justConnected = false

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            LogoTile(logo: "ServiceDropbox")
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: "Dropbox").uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                switch status.state {
                case .off:
                    Text("연결되지 않았어요").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .waiting:
                    Text("브라우저에서 Dropbox에 로그인하고 허용해 주세요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                        .fixedSize(horizontal: false, vertical: true)
                case .failed(let why):
                    Text(why).uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                case .lost, .unreadable:
                    Text(status.line).uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                case .syncing(let line):
                    Text("동기화 중 · \(line)").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .connected:
                    Text(status.line).uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                    if let last = status.lastFetched {
                        Text(last).uiFont(12).foregroundStyle(Theme.ink3)
                    }
                }
            }
            Spacer(minLength: 8)
            if justConnected {
                DoneMark(text: String(localized: "연결했어요")).padding(.top, -5)
            } else {
                switch status.state {
                case .off:
                    Button(action: actions.connect) { OutlineButton(label: String(localized: "연결")) }.buttonStyle(PressStyle())
                case .waiting:
                    Button(action: actions.cancel) { OutlineButton(label: String(localized: "취소")) }.buttonStyle(PressStyle())
                case .unreadable:
                    // connecting again would write over an item that may still be there: the 30-minute timer reads it again
                    EmptyView()
                case .failed, .lost:
                    Button(action: actions.connect) { OutlineButton(label: String(localized: "다시 연결")) }.buttonStyle(PressStyle())
                case .syncing, .connected:
                    Menu {
                        if status.state == .connected { Button("폴더 고르기", action: actions.pickFolders) }
                        Button("연결 해제", role: .destructive, action: actions.disconnect)
                    } label: {
                        Image(systemName: "ellipsis").uiFont(13, weight: .semibold).foregroundStyle(Theme.ink3)
                            .frame(width: 24, height: 20)
                            .contentShape(Rectangle())
                    }
                    .menuStyle(.button)
                    .buttonStyle(.plain)
                    .menuIndicator(.hidden)
                    .fixedSize()
                    .padding(.top, 2)
                }
            }
            // the first sync's wait, at the right edge where 연결했어요's check will close it
            if !justConnected, case .syncing(let line) = status.state {
                SyncRing(fraction: SyncRing.fraction(in: line)).padding(.top, -5)
            }
        }
        .sourceCardMotion(status.state.stage, justConnected: $justConnected)
        .frame(maxWidth: .infinity, alignment: .leading)  // a cell of 설정 → 연결's two columns, which draw the row
    }
}

/// 폴더 고르기: the top folders (shared and team folders too) with their file counts. A folder turned off is left out of
/// search on the next sync; only what is turned off is remembered, so new folders come in on their own (docs/33 5절).
struct DropboxFoldersBody: View {
    /// nil while the engine is asked.
    let folders: [DropboxFolder]?
    @Binding var on: Set<String>
    /// The folders could not be asked for (the engine did not answer).
    var unreachable = false
    /// The list scrolls past 300 points; the render tests draw it flat (ImageRenderer leaves a ScrollView blank).
    var scrolls = true
    var cancel: () -> Void = {}
    var save: () -> Void = {}

    private var empty: Bool { folders.map { $0.isEmpty } ?? false }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("폴더 고르기").uiFont(16, weight: .semibold).foregroundStyle(Theme.ink)
            Text("끈 폴더의 파일은 찾지 않아요. 새로 생긴 폴더는 켜진 채로 들어와요.")
                .uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            Group {
                if let folders {
                    if unreachable {
                        Text("Dropbox 폴더 목록을 불러오지 못했어요. 잠시 뒤에 다시 열어 주세요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, minHeight: 60, alignment: .leading)
                    } else if empty {
                        Text("고를 폴더가 없어요. Dropbox 목록을 받으면 여기에 보여요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, minHeight: 60, alignment: .leading)
                    } else {
                        let list = VStack(spacing: 2) { ForEach(folders) { row($0) } }
                        if scrolls {
                            ScrollView { list }
                                .scrollIndicators(.never)
                                .frame(maxHeight: 300)
                                .fixedSize(horizontal: false, vertical: true)
                        } else {
                            list
                        }
                    }
                } else {
                    ProgressView().controlSize(.small).frame(maxWidth: .infinity, minHeight: 60)
                }
            }
            .padding(.top, 14)
            HStack(spacing: 8) {
                Spacer(minLength: 8)
                Button(action: cancel) { OutlineButton(label: String(localized: "취소")) }
                    .buttonStyle(PressStyle()).keyboardShortcut(.cancelAction)
                let off = folders == nil || empty || unreachable
                Button("저장", action: save)
                    .buttonStyle(InkButtonStyle())
                    .keyboardShortcut(.defaultAction)
                    .disabled(off)
                    .opacity(off ? 0.35 : 1)
            }
            .padding(.top, 18)
        }
        .padding(22)
        .frame(width: 420)
        .background(Theme.paper)
    }

    /// One folder: an ink check when it is searched, its name ("팀 공간" under a team folder's), how many files. The
    /// whole row toggles.
    private func row(_ f: DropboxFolder) -> some View {
        let isOn = on.contains(f.id)
        return Button {
            if isOn { on.remove(f.id) } else { on.insert(f.id) }
        } label: {
            HStack(spacing: 10) {
                ZStack {
                    RoundedRectangle(cornerRadius: 5, style: .continuous)
                        .fill(isOn ? Theme.ink : .clear)
                    RoundedRectangle(cornerRadius: 5, style: .continuous)
                        .strokeBorder(Theme.ink.opacity(isOn ? 0 : 0.25), lineWidth: 1.2)
                    Image(systemName: "checkmark").uiFont(9.5, weight: .bold).foregroundStyle(Theme.onStrong)
                        .opacity(isOn ? 1 : 0)
                }
                .frame(width: 16, height: 16)
                VStack(alignment: .leading, spacing: 1) {
                    Text(verbatim: f.name).uiFont(13.5, weight: .medium)
                        .foregroundStyle(isOn ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3)).lineLimit(1)
                    if f.teamFolder {
                        Text("팀 공간").uiFont(11.5).foregroundStyle(Theme.ink3)
                    }
                }
                Spacer(minLength: 8)
                Text(String(localized: "파일 \(f.files)개")).uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize()
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .frame(minHeight: 34)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}
