import Foundation

/// Who is signed in and how much of the month is left (the Tacie account, docs/14). Signed out, only the Mac's name.
struct Account {
    let name: String
    let signedInWith: String?
    let usageLeft: Double?
    let resetsOn: Date?
    /// waiting (until approved), active, blocked; nil signed out
    var status: String? = nil

    /// `--account-name <name>` (development builds): the name shown instead of the account's own, for captures.
    static let captureName: String? = {
        let args = ProcessInfo.processInfo.arguments
        guard Build.isDev, let i = args.firstIndex(of: "--account-name"), args.indices.contains(i + 1) else { return nil }
        return args[i + 1]
    }()

    @MainActor static var current: Account {
        let a = TacieAccount.shared
        guard a.signedIn else {
            return Account(name: captureName ?? NSFullUserName(), signedInWith: nil, usageLeft: nil, resetsOn: nil)
        }
        let month = a.info?.month
        let left = month.map { $0.limitUsd > 0 ? 1 - $0.spentUsd / $0.limitUsd : 0 }
        return Account(name: captureName ?? a.info?.name ?? a.info?.email ?? NSFullUserName(), signedInWith: a.provider?.label ?? "Google", usageLeft: left,
                       resetsOn: nextMonth(Date()), status: a.info?.status)
    }

    /// The first day of next month (the server counts months in UTC).
    static func nextMonth(_ now: Date) -> Date { UsageMonth.nextMonth(now) }

    /// Why a meeting did not start or stopped at the month's limit, on the pill. The
    /// time left is the same words as 설정 → 계정's usage meter.
    static func limitLine(kept: Bool, now: Date = Date()) -> String {
        let t = SettingsText.untilReset(Account.nextMonth(now).timeIntervalSince(now))
        return kept
            ? String(localized: "meetingModel.limitStopped", defaultValue: "이번 달 AI 사용량을 다 써서 회의를 끝냈어요. 지금까지 대화 기록은 저장했어요. \(t) 뒤에 다시 쓸 수 있어요.")
            : String(localized: "meetingModel.limitNotStarted", defaultValue: "이번 달 AI 사용량을 다 써서 회의를 시작하지 못했어요. \(t) 뒤에 다시 쓸 수 있어요.")
    }

    /// Settings sidebar: "사용량 72% 남음".
    var usageLine: String {
        line { String(localized: "사용량 \($0)% 남음") }
    }

    /// Main window account row: "이번 달 사용량 72% 남음".
    var monthUsageLine: String {
        line { String(localized: "이번 달 사용량 \($0)% 남음") }
    }

    /// Each line is one key with its percent inside, so English can put the words around the number its own way.
    private func line(_ usage: (Int) -> String) -> String {
        if status == "waiting" { return String(localized: "승인을 기다리는 중") }
        if let usageLeft { return usage(Self.percent(usageLeft)) }
        return signedInWith == nil ? (Build.isDev ? String(localized: "개발 빌드") : String(localized: "로그인하지 않았어요"))
            : String(localized: "로그인함")
    }

    private static func percent(_ v: Double) -> Int { Int((min(max(v, 0), 1) * 100).rounded()) }
}

extension ASignInWith {
    /// The name on the buttons and in "… 계정으로 로그인했어요".
    var label: String {
        switch self {
        case .google: "Google"
        case .github: "GitHub"
        case .apple: "Apple"
        }
    }
}
