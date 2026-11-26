import Foundation
import UserNotifications

/// The macOS notification that goes with the pill's "A meeting started": seen while the user looks at the meeting app,
/// and left in Notification Center. Start listening on it does what the pill's button does. It goes away with the
/// pill's question. Permission is asked the first time a meeting app starts, not before.
@MainActor
final class MeetingNotice: NSObject, UNUserNotificationCenterDelegate {
    var onAccept: () -> Void = {}
    /// A click on "회의를 마쳤어요": open that meeting's page (where 이어서 듣기 is).
    var onOpenMeeting: (String) -> Void = { _ in }

    private static let category = "meeting-start"
    nonisolated private static let acceptAction = "listen"
    private static let id = "meeting-start"
    private var center: UNUserNotificationCenter? {
        // a bare test binary has no bundle to post from
        Bundle.main.bundleIdentifier == nil ? nil : .current()
    }

    func start() {
        guard let center else { return }
        center.delegate = self
        let listen = UNNotificationAction(identifier: Self.acceptAction, title: String(localized: "듣기 시작"), options: [])
        center.setNotificationCategories([UNNotificationCategory(identifier: Self.category, actions: [listen],
                                                                 intentIdentifiers: [], options: [])])
    }

    /// The pill's question came (`app`) or went (nil).
    func show(_ app: MeetingApp?) {
        guard let center else { return }
        center.removeDeliveredNotifications(withIdentifiers: [Self.id])
        center.removePendingNotificationRequests(withIdentifiers: [Self.id])
        guard let app else { return }
        Log.info("meeting notice: asking")
        center.requestAuthorization(options: [.alert, .sound]) { granted, error in
            guard granted else {
                Log.info("meeting notice not allowed\(error.map { ": \($0.localizedDescription)" } ?? "")")
                return
            }
            Log.info("meeting notice: allowed")
            let content = UNMutableNotificationContent()
            content.title = app.started
            content.body = String(localized: "Tacie가 회의를 들으며 질문에 답할 수 있어요. 누르기 전에는 아무것도 듣지 않아요.")
            content.categoryIdentifier = Self.category
            center.add(UNNotificationRequest(identifier: Self.id, content: content, trigger: nil)) { error in
                if let error { Log.info("meeting notice failed: \(error.localizedDescription)") }
            }
        }
    }

    /// Tacie ended `m` by itself (docs/35 5-4): why, its title, and that it can be taken up again. Never asks for
    /// permission on its own: only shown once the meeting-start notice was allowed.
    func showEnded(_ m: SavedMeeting, reason: AEndReason) {
        guard let center else { return }
        center.getNotificationSettings { settings in
            guard settings.authorizationStatus == .authorized else { return }
            let content = UNMutableNotificationContent()
            content.title = Self.endedTitle(reason)
            content.body = String(localized: "\(m.displayTitle(calendar: .current)) · 이 회의를 이어 갈 수 있어요")
            content.userInfo = ["meeting": m.id]
            center.add(UNNotificationRequest(identifier: "meeting-ended", content: content, trigger: nil)) { error in
                if let error { Log.info("meeting ended notice failed: \(error.localizedDescription)") }
            }
            Log.info("meeting ended notice: \(reason.rawValue)")
        }
    }

    /// The meeting was taken up again (or is gone): its "회의를 마쳤어요" no longer holds.
    func clearEnded() {
        center?.removeDeliveredNotifications(withIdentifiers: ["meeting-ended"])
    }

    static func endedTitle(_ reason: AEndReason) -> String {
        switch reason {
        case .callEnded: String(localized: "통화가 끝나서 회의를 마쳤어요")
        case .heardEnd: String(localized: "끝내자는 말을 듣고 회의를 마쳤어요")
        default: String(localized: "한동안 조용해서 회의를 마쳤어요")
        }
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                            withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .list])
    }

    /// 듣기 시작 starts listening. A click on the notification itself only brings the pill's question into view.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                            withCompletionHandler completionHandler: @escaping () -> Void) {
        let accept = response.actionIdentifier == Self.acceptAction
        let meeting = response.notification.request.content.userInfo["meeting"] as? String
        Task { @MainActor in
            if let meeting { self.onOpenMeeting(meeting) }
            if accept { self.onAccept() }
            completionHandler()
        }
    }
}
