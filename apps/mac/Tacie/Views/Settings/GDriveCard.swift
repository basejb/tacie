import SwiftUI

/// What a Google Drive card says (docs/19 8절, 8-1절), the same in the onboarding and in Settings → Connections.
@MainActor
struct GDriveStatus {
    enum State: Equatable {
        case off  // not connected: 연결
        case waiting  // the approval page is open
        case failed(String)  // 다시 연결
        case lost  // Google refused the refresh token: 다시 연결 (found files stay for seven days)
        case syncing(String)  // "Google Drive 문서 받는 중 40%"
        case connected
    }

    let keeper: IndexKeeper
    private var link: GDriveLink { keeper.gdrive }

    var state: State {
        switch link.phase {
        case .waiting: return .waiting
        case .failed(let why) where !link.connected: return .failed(why)
        default: break
        }
        guard link.connected else { return .off }
        // the engine's lost_since: the connection is lost even if this app has not seen it yet
        if link.lost || keeper.gdriveInfo?.lostSince != nil { return .lost }
        // a sync with nothing from Drive yet is Drive's first one
        if let line = keeper.stepLine, keeper.gdriveInfo?.files == nil { return .syncing(line) }
        return .connected
    }

    /// "연결됨 · me@gmail.com · 문서 1,240개", or why not (SettingsText.gdriveLine).
    var line: String { SettingsText.gdriveLine(keeper.gdriveInfo, lost: link.lost, now: Date()) }

    /// "마지막으로 받은 때: 10월 31일 오후 3:20" when Drive could not be reached on the last sync.
    var lastFetched: String? {
        guard let info = keeper.gdriveInfo, ["network", "busy", "down"].contains(info.errorKind ?? ""),
              let at = info.fetchedAt.flatMap(Self.iso) else { return nil }
        return String(localized: "마지막으로 받은 때: \(at.formatted(.dateTime.month().day().hour().minute().locale(AppLocale.locale)))")
    }

    /// The account is in shared drives: the picker is worth offering.
    var hasDrives: Bool { (keeper.gdriveInfo?.drives ?? 0) > 0 }

    private static func iso(_ s: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f.date(from: s) ?? ISO8601DateFormatter().date(from: s)
    }
}

/// The onboarding's Google Drive tile (SourcesStep), next to Linear's.
struct GDriveTile: View {
    let keeper: IndexKeeper
    @State private var pickDrives = false

    var body: some View {
        let status = GDriveStatus(keeper: keeper)
        let state = status.state
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image("ServiceGoogleDrive").resizable().scaledToFit().frame(width: 17, height: 17)
                    .frame(width: 20, height: 20)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Google Drive").uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink)
                    Text("Google 문서와 파일").uiFont(11.5).foregroundStyle(Theme.ink3).lineLimit(1)
                }
                Spacer(minLength: 0)
                if state == .connected, status.hasDrives {
                    TileMenu { Button("공유 드라이브 고르기") { pickDrives = true } }
                }
            }
            switch state {
            case .off:
                Button("연결") { keeper.gdrive.connect() }.buttonStyle(InkButtonStyle())
            case .waiting:
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.mini)
                        Text("브라우저에서 기다리는 중").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                        Spacer(minLength: 0)
                        Button("취소") { keeper.gdrive.cancel() }.buttonStyle(.plain)
                            .uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink3)
                    }
                    Text("계정을 고르고 허용해 주세요. Tacie는 읽기만 해요.")
                        .uiFont(11).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                }
            case .failed(let why):
                problem(why)
            case .lost:
                problem(status.line)
            case .syncing(let line):
                HStack(spacing: 6) {
                    ProgressView().controlSize(.mini)
                    Text("동기화 중 · \(line)").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                        .lineLimit(1)
                }
            case .connected:
                if let files = keeper.gdriveInfo?.files, files > 0 {
                    let count = String(localized: "문서 \(files)개")
                    let growing = keeper.gdriveInfo?.initialDone == false
                    TileStatusLines(name: keeper.gdrive.account ?? String(localized: "연결됨"),
                                    detail: growing ? String(localized: "\(count) · 더 받는 중") : count, growing: growing)
                } else {
                    TileStatusLines(name: keeper.gdrive.account ?? String(localized: "연결됨"), detail: status.line)
                }
            }
        }
        .sheet(isPresented: $pickDrives) { GDriveDrivesSheet(keeper: keeper) }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 100, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.paper))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
            .strokeBorder(state == .connected ? Theme.ink.opacity(0.35) : Theme.ink.opacity(0.1)))
    }

    private func problem(_ why: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(why).uiFont(11.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            Button("다시 연결") { keeper.gdrive.connect() }.buttonStyle(InkButtonStyle())
        }
    }
}

/// 설정 → 연결's Google Drive card, under Linear's.
struct GDriveSettingsCard: View {
    let keeper: IndexKeeper
    @State private var askDisconnect = false
    @State private var justConnected = false
    @State private var pickDrives = false

    var body: some View {
        let status = GDriveStatus(keeper: keeper)
        HStack(alignment: .top, spacing: 12) {
            LogoTile(logo: "ServiceGoogleDrive")
            VStack(alignment: .leading, spacing: 3) {
                Text("Google Drive").uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                switch status.state {
                case .off:
                    Text("연결되지 않았어요").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .waiting:
                    Text("브라우저에서 계정을 고르고 허용해 주세요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .failed(let why):
                    Text(why).uiFont(12.5).foregroundStyle(Theme.ink2)
                case .lost:
                    Text(status.line).uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                case .syncing(let line):
                    Text("동기화 중 · \(line)").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .connected:
                    Text(status.line).uiFont(12.5).foregroundStyle(Theme.ink2)
                    if let last = status.lastFetched {
                        Text(last).uiFont(12).foregroundStyle(Theme.ink3)
                    }
                }
            }
            Spacer()
            if justConnected {
                DoneMark(text: String(localized: "연결했어요")).padding(.top, -5)
            } else {
                switch status.state {
                case .off:
                    Button { keeper.gdrive.connect() } label: { OutlineButton(label: String(localized: "연결")) }.buttonStyle(PressStyle())
                case .waiting:
                    Button { keeper.gdrive.cancel() } label: { OutlineButton(label: String(localized: "취소")) }.buttonStyle(PressStyle())
                case .failed, .lost:
                    Button { keeper.gdrive.connect() } label: { OutlineButton(label: String(localized: "다시 연결")) }.buttonStyle(PressStyle())
                case .syncing, .connected:
                    Menu {
                        if status.hasDrives { Button("공유 드라이브 고르기") { pickDrives = true } }
                        Button("연결 해제", role: .destructive) { askDisconnect = true }
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
        .alert("Google Drive 연결을 해제할까요?", isPresented: $askDisconnect) {
            Button("연결 해제", role: .destructive) {
                Analytics.capture(.sourceDisconnected(tool: .gdrive))
                Task { await keeper.gdrive.disconnect() }
            }
            Button("취소", role: .cancel) {}
        } message: {
            Text("받아 둔 Drive 자료를 이 Mac에서 지우고, Google 쪽 연결도 끊어요.")
        }
        .sheet(isPresented: $pickDrives) { GDriveDrivesSheet(keeper: keeper) }
    }
}

/// 공유 드라이브 고르기: the shared drives with their file counts. 내 드라이브 is always searched; turned-off drives leave
/// search on the next sync; new drives come in on their own.
struct GDriveDrivesSheet: View {
    let keeper: IndexKeeper
    @Environment(\.dismiss) private var dismiss
    @State private var drives: [GDriveDrive] = []
    @State private var on: Set<String> = []
    @State private var loading = true

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("공유 드라이브 고르기").uiFont(16, weight: .semibold).foregroundStyle(Theme.ink)
            Text("내 드라이브는 늘 찾아요. 끈 공유 드라이브의 문서는 찾지 않고, 새로 들어간 드라이브는 저절로 들어와요.")
                .uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            Group {
                if loading {
                    ProgressView().controlSize(.small).frame(maxWidth: .infinity, minHeight: 60)
                } else if drives.isEmpty {
                    Text("속한 공유 드라이브가 없어요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                        .frame(maxWidth: .infinity, minHeight: 60, alignment: .leading)
                } else {
                    ScrollView {
                        VStack(spacing: 2) { ForEach(drives) { row($0) } }
                    }
                    .scrollIndicators(.never)
                    .frame(maxHeight: 280)
                    .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(.top, 14)
            HStack(spacing: 8) {
                Spacer()
                Button { dismiss() } label: { OutlineButton(label: String(localized: "취소")) }
                    .buttonStyle(PressStyle()).keyboardShortcut(.cancelAction)
                Button("저장") {
                    let muted = drives.map(\.id).filter { !on.contains($0) }
                    Task { await keeper.gdrive.setMuted(muted) }
                    dismiss()
                }
                .buttonStyle(InkButtonStyle())
                .keyboardShortcut(.defaultAction)
                .disabled(loading || drives.isEmpty)
                .opacity(loading || drives.isEmpty ? 0.35 : 1)
            }
            .padding(.top, 18)
        }
        .padding(22)
        .frame(width: 400)
        .background(Theme.paper)
        .task {
            drives = (try? await keeper.gdriveDrives()) ?? []
            let muted = Set(keeper.gdrive.saved?.muted ?? [])
            on = Set(drives.map(\.id)).subtracting(muted)
            loading = false
        }
    }

    /// One drive: its name, how many documents, and an ink check when it is searched. The whole row toggles.
    private func row(_ d: GDriveDrive) -> some View {
        let isOn = on.contains(d.id)
        return Button {
            if isOn { on.remove(d.id) } else { on.insert(d.id) }
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
                Text(d.name).uiFont(13.5, weight: .medium).foregroundStyle(isOn ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3)).lineLimit(1)
                Spacer(minLength: 8)
                Text(String(localized: "문서 \(d.files)개"))
                    .uiFont(12.5).foregroundStyle(Theme.ink3)
            }
            .padding(.horizontal, 10)
            .frame(height: 36)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

extension GDriveStatus.State {
    var stage: SourceStage {
        switch self {
        case .off: .off
        case .waiting: .waiting
        case .failed: .failed
        case .lost: .lost
        case .syncing: .syncing
        case .connected: .connected
        }
    }
}
