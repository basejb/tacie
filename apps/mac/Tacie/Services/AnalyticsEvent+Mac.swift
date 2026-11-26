import Foundation

// The Mac's own types turned into the usage-number values (AnalyticsEvent.swift, shared with the iPhone).

extension AMeetingApp {
    init(_ app: MeetingApp) { self = AMeetingApp(rawValue: app.rawValue) ?? .browser }
}

extension PanelScopeKind {
    init(_ scope: PanelScope) {
        switch scope {
        case .meeting: self = .meeting
        case .all: self = .all
        }
    }
}

extension AWish: Identifiable {
    var id: String { rawValue }
    var label: String {
        switch self {
        case .faster: String(localized: "답이 더 빨랐으면")
        case .accurate: String(localized: "답이 더 정확했으면")
        case .catch: String(localized: "질문을 놓치지 않았으면")
        case .sources: String(localized: "연결할 자료가 더 있었으면")
        case .notes: String(localized: "정리가 더 자세했으면")
        case .todos: String(localized: "할 일을 더 잘 챙겼으면")
        }
    }
}
