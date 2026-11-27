import AppKit
import SwiftUI

/// The home page's line after signing out (#56): an account's meetings are hidden until it signs in again, so the app
/// does not look empty for no reason. Meetings made while signed out show as usual.
struct SignedOutNotice: View {
    let signIn: () -> Void

    static func shows(signedIn: Bool, hasAccount: Bool, accountsHere: Bool) -> Bool {
        !signedIn && hasAccount && accountsHere
    }

    var body: some View {
        HStack(alignment: .center, spacing: 14) {
            VStack(alignment: .leading, spacing: 3) {
                Text("이 계정의 회의는 로그인하면 다시 보여요").uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                Text("지금은 로그인하지 않은 상태예요. 로그인하지 않고 한 회의는 여기에 보여요.")
                    .uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            Button(String(localized: "로그인"), action: signIn).buttonStyle(InkButtonStyle())
        }
        .padding(.horizontal, 18).padding(.vertical, 14)
        .background(Theme.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.hairline))
    }
}

/// The home page's line after meetings came over from an earlier open-source build (OpenEditionImport): once, until
/// closed. Keys never come over, so someone who used a key is told where to paste it again.
struct OpenImportNotice: View {
    @AppStorage(OpenEditionImport.noticeKey) private var shown = false
    let openAI: () -> Void

    var body: some View {
        if shown {
            let key = OpenEditionImport.usedKey()
            HStack(alignment: .center, spacing: 14) {
                VStack(alignment: .leading, spacing: 3) {
                    Text("예전에 쓰던 Tacie의 회의를 가져왔어요").uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                    if key {
                        Text("API 키를 설정 → AI에서 다시 붙여 넣어 주세요.")
                            .uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 8)
                if key {
                    Button(String(localized: "설정 열기")) {
                        shown = false
                        openAI()
                    }.buttonStyle(InkButtonStyle())
                }
                Button(String(localized: "닫기")) { shown = false }.buttonStyle(GhostButtonStyle())
            }
            .padding(.horizontal, 18).padding(.vertical, 14)
            .background(Theme.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.hairline))
            .padding(.top, 18)
        }
    }
}

/// The open-source build's last update (#56): Tacie is one app now. Only in the open build that updates itself (the
/// GitHub release DMG); a build from source has no feed and never shows it. The new app brings the meetings and
/// settings over on its first launch (OpenEditionImport); keys stay behind.
struct OneAppNotice: View {
    static let closedKey = "oneApp.noticeClosed"
    static let download = URL(string: "https://tacie.app/download/Tacie.dmg")!
    @AppStorage(OneAppNotice.closedKey) private var closed = false
    var open: (URL) -> Void = { NSWorkspace.shared.open($0) }

    static func shows(open: Bool, feed: URL?) -> Bool { open && feed != nil }

    var body: some View {
        if Self.shows(open: Build.isOpen, feed: BuildValues.updateFeed), !closed {
            HStack(alignment: .center, spacing: 14) {
                VStack(alignment: .leading, spacing: 3) {
                    Text("이제 Tacie는 앱 하나예요").uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                    Text("새 Tacie를 받으면 예전 회의와 설정을 가져와요. API 키는 다시 붙여 넣어 주세요. 이 앱의 업데이트는 여기서 끝나요.")
                        .uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 8)
                Button(String(localized: "받기")) { open(Self.download) }.buttonStyle(InkButtonStyle())
                Button(String(localized: "닫기")) { closed = true }.buttonStyle(GhostButtonStyle())
            }
            .padding(.horizontal, 18).padding(.vertical, 14)
            .background(Theme.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.hairline))
            .padding(.top, 18)
        }
    }
}
