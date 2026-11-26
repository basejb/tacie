import SwiftUI

/// What a Notion card says (docs/13 1절, 1-1절), the same in the onboarding and in Settings → Connections.
@MainActor
struct NotionStatus {
    enum State: Equatable {
        case off  // not connected: 연결
        case waiting  // the approval page is open
        case failed(String)  // 다시 연결
        case lost  // Notion refused the token and a new one could not be had: 다시 연결
        case syncing(String)  // "노션 페이지 받는 중 40%"
        case connected
    }

    let keeper: IndexKeeper
    private var link: NotionLink { keeper.notion }

    var state: State {
        switch link.phase {
        case .waiting: return .waiting
        case .failed(let why) where !link.connected: return .failed(why)
        default: break
        }
        guard link.connected else { return .off }
        if link.lost { return .lost }
        // a sync with nothing from Notion yet is Notion's first one
        if let line = keeper.stepLine, keeper.info?.notion?.pages == nil { return .syncing(line) }
        return .connected
    }

    /// The tile's second line: "페이지 120개", or nil before the first fetch.
    var pagesLine: String? {
        keeper.info?.notion?.pages.map { $0 == 0 ? String(localized: "고른 페이지가 없어요") : String(localized: "페이지 \($0)개") }
    }

    /// "팀 워크스페이스 · 페이지 120개", or what is known of it.
    var connectedLine: String {
        var parts: [String] = []
        if let w = link.workspace { parts.append(w) }
        if let n = keeper.info?.notion?.pages { parts.append(n == 0 ? String(localized: "고른 페이지가 없어요") : String(localized: "페이지 \(n)개")) }
        return parts.isEmpty ? String(localized: "연결됨") : parts.joined(separator: " · ")
    }
}

/// The onboarding's Notion tile (SourcesStep), next to GitHub's.
struct NotionTile: View {
    let keeper: IndexKeeper
    @State private var pasting = false

    var body: some View {
        let status = NotionStatus(keeper: keeper)
        let state = status.state
        let link = keeper.notion
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image("ServiceNotion").resizable().scaledToFit().frame(width: 17, height: 17)
                    .frame(width: 20, height: 20)
                VStack(alignment: .leading, spacing: 1) {
                    Text("노션").uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink)
                    Text("팀 문서와 위키").uiFont(11.5).foregroundStyle(Theme.ink3)
                }
                Spacer(minLength: 0)
                if state == .connected {
                    TileMenu { Button("페이지 고르기") { TokenCopy.pickNotionPages(keeper: keeper) } }
                }
            }
            switch state {
            case .off:
                Button("연결") { TokenCopy.start(.notion, keeper: keeper, paste: $pasting) }.buttonStyle(InkButtonStyle())
            case .waiting:
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.mini)
                        Text("브라우저에서 연결을 기다리고 있어요").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                        Spacer(minLength: 0)
                        Button("취소") { keeper.notion.cancel() }.buttonStyle(.plain)
                            .uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink3)
                    }
                    Text("Tacie가 볼 페이지를 고르고 허용해 주세요.")
                        .uiFont(11).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                }
            case .failed(let why):
                problem(why)
            case .lost:
                problem(String(localized: "노션 연결이 끊겼어요. 받아 둔 페이지로 계속 찾아요."))
            case .syncing(let line):
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.mini)
                        Text("동기화 중 · \(line)").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                            .lineLimit(1)
                    }
                    if keeper.firstSync != nil {
                        Text("처음 동기화가 끝나야 답할 수 있어요. 페이지가 많으면 오래 걸려요.")
                            .uiFont(11).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                    }
                }
            case .connected:
                // nothing picked yet: picking is the next thing to do, so it comes out of the menu
                TileStatusLines(name: link.workspace ?? String(localized: "연결됨"), detail: status.pagesLine,
                                pick: keeper.info?.notion?.pages == 0 ? (String(localized: "페이지 고르기"), { TokenCopy.pickNotionPages(keeper: keeper) }) : nil)
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 100, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.paper))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
            .strokeBorder(state == .connected ? Theme.ink.opacity(0.35) : Theme.ink.opacity(0.1)))
        .sheet(isPresented: $pasting) { TokenConnectSheet(source: .notion, keeper: keeper) }
    }

    private func problem(_ why: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(why).uiFont(11.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            Button("다시 연결") { TokenCopy.start(.notion, keeper: keeper, paste: $pasting) }.buttonStyle(InkButtonStyle())
        }
    }
}

/// 설정 → 연결's Notion card, under GitHub's.
struct NotionSettingsCard: View {
    let keeper: IndexKeeper
    @State private var askDisconnect = false
    @State private var justConnected = false
    @State private var pasting = false

    var body: some View {
        let status = NotionStatus(keeper: keeper)
        HStack(alignment: .top, spacing: 12) {
            LogoTile(logo: "ServiceNotion")
            VStack(alignment: .leading, spacing: 3) {
                Text("노션").uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                switch status.state {
                case .off:
                    Text("연결되지 않았어요").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .waiting:
                    Text("브라우저에서 Tacie가 볼 페이지를 고르고 허용해 주세요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .failed(let why):
                    Text(why).uiFont(12.5).foregroundStyle(Theme.ink2)
                case .lost:
                    Text("노션 연결이 끊겼어요. 받아 둔 페이지로 계속 찾아요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .syncing(let line):
                    Text("동기화 중 · \(line)").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .connected:
                    Text(status.connectedLine).uiFont(12.5).foregroundStyle(Theme.ink2)
                }
            }
            Spacer()
            if justConnected {
                DoneMark(text: String(localized: "연결했어요")).padding(.top, -5)
            } else {
                switch status.state {
                case .off:
                    Button { TokenCopy.start(.notion, keeper: keeper, paste: $pasting) } label: { OutlineButton(label: String(localized: "연결")) }.buttonStyle(PressStyle())
                case .waiting:
                    Button { keeper.notion.cancel() } label: { OutlineButton(label: String(localized: "취소")) }.buttonStyle(PressStyle())
                case .failed, .lost:
                    Button { TokenCopy.start(.notion, keeper: keeper, paste: $pasting) } label: { OutlineButton(label: String(localized: "다시 연결")) }.buttonStyle(PressStyle())
                case .syncing, .connected:
                    Menu {
                        Button("페이지 고르기") { TokenCopy.pickNotionPages(keeper: keeper) }
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
        .alert("노션 연결을 해제할까요?", isPresented: $askDisconnect) {
            Button("연결 해제", role: .destructive) {
                Analytics.capture(.sourceDisconnected(tool: .notion))
                Task { await keeper.notion.disconnect() }
            }
            Button("취소", role: .cancel) {}
        } message: {
            Text("받아 둔 노션 페이지를 이 Mac에서 지우고, 더는 노션에서 찾지 않아요. 노션 쪽 접근은 노션의 설정 → 연결에서 Tacie를 지우면 끊겨요.")
        }
        .sheet(isPresented: $pasting) { TokenConnectSheet(source: .notion, keeper: keeper) }
    }
}

extension NotionStatus.State {
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
