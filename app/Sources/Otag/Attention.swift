import AppKit
import OtagKit
import SwiftUI
import UserNotifications

/// Reaching the user when the window is not in front of them.
///
/// Three ways, in order of how hard they are to miss: a number on the Dock
/// icon, a notification with Allow and Deny on it, and a menu bar item that
/// is there even with the window closed. All three say the same thing — how
/// many agents are waiting — and none of them repeats itself: a notification
/// goes out the moment an agent starts waiting, and not again until it stops.
@MainActor
@Observable
final class Attention: NSObject, UNUserNotificationCenterDelegate {
    /// The questions already announced, so none is announced twice.
    private var announced: Set<UInt64> = []
    /// Whether the system let otag post notifications.
    private var allowed = false
    /// Set when a notification's button was pressed, for the app to carry
    /// out on the next turn of the main loop.
    var pending: (ask: UInt64, answer: Answer)?

    /// Identifier of the category carrying the Allow and Deny buttons.
    private static let category = "otag.ask"
    private static let allowAction = "otag.allow"
    private static let denyAction = "otag.deny"

    /// Ask for permission to notify, and register the two buttons.
    ///
    /// A system that refuses — or an app that is not allowed to ask — is not
    /// an error: the Dock badge and the menu bar item still work.
    func prepare() {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        center.setNotificationCategories([
            UNNotificationCategory(
                identifier: Self.category,
                actions: [
                    UNNotificationAction(
                        identifier: Self.allowAction,
                        title: "Allow",
                        options: []
                    ),
                    UNNotificationAction(
                        identifier: Self.denyAction,
                        title: "Deny",
                        options: []
                    ),
                ],
                intentIdentifiers: [],
                options: []
            ),
        ])
        center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
            Task { @MainActor in self.allowed = granted }
        }
    }

    /// Put the number of waiting sessions on the Dock icon, and announce the
    /// questions that are new.
    func update(asks: [AskView], waiting: Int, store: SessionStore) {
        NSApp.dockTile.badgeLabel = waiting > 0 ? "\(waiting)" : nil

        let current = Set(asks.map(\.id))
        // A question that has been answered can be announced again if it
        // somehow comes back, but only as a new question.
        announced.formIntersection(current)

        for ask in asks where !announced.contains(ask.id) {
            announced.insert(ask.id)
            announce(ask, store: store)
        }
    }

    /// Tell the user about one question.
    private func announce(_ ask: AskView, store: SessionStore) {
        let wanted = UserDefaults.standard.object(forKey: Preference.notify) as? Bool ?? true
        guard allowed, wanted else { return }

        let content = UNMutableNotificationContent()
        let project = store.pane(ask.pane)?.workspace ?? ""
        content.title = ask.agent?.label ?? "A session"
        content.subtitle = project
        content.body = ask.summary
        content.categoryIdentifier = Self.category
        content.userInfo = ["ask": ask.id]
        content.sound = nil

        let request = UNNotificationRequest(
            identifier: "otag.ask.\(ask.id)",
            content: content,
            trigger: nil
        )
        UNUserNotificationCenter.current().add(request)
    }

    /// Take a notification's Allow or Deny back into the app.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let info = response.notification.request.content.userInfo
        let identifier = response.actionIdentifier
        let ask = (info["ask"] as? UInt64) ?? (info["ask"] as? NSNumber)?.uint64Value

        Task { @MainActor in
            defer { completionHandler() }
            guard let ask else { return }
            switch identifier {
            case Self.allowAction:
                pending = (ask, .allow)
            case Self.denyAction:
                pending = (ask, .deny)
            default:
                // The notification itself was clicked: bring the window
                // forward rather than answering anything.
                NSApp.activate(ignoringOtherApps: true)
            }
        }
    }

    /// Show otag's own notifications even while the app is in front: the
    /// window may be on another space or behind something else.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions)
            -> Void
    ) {
        completionHandler([.banner, .list])
    }
}

/// The menu bar item: the limits, and who is waiting.
///
/// Visible with the window closed, which is the point — an agent that needs
/// permission should be findable without going looking for the app.
struct MenuBarView: View {
    let store: SessionStore

    var body: some View {
        if store.waiting > 0 {
            Text(store.waiting == 1 ? "1 session needs you" : "\(store.waiting) sessions need you")
            Divider()
            ForEach(store.asks) { ask in
                Button("Allow: \(short(ask.summary))") { store.answer(ask, with: .allow) }
                Button("Deny: \(short(ask.summary))") { store.answer(ask, with: .deny) }
                Divider()
            }
        } else {
            Text("Nothing is waiting")
            Divider()
        }

        ForEach(store.usage.rows) { row in
            Text("\(row.agent.label) \(row.window.label) · \(row.usedPercent)%")
        }
        if !store.usage.rows.isEmpty {
            Divider()
        }

        Button("Open Otag") {
            NSApp.activate(ignoringOtherApps: true)
            // The window may have been closed rather than hidden.
            for window in NSApp.windows where window.canBecomeMain {
                window.makeKeyAndOrderFront(nil)
                break
            }
        }
        Button("Quit Otag") { NSApp.terminate(nil) }
    }

    /// A question, short enough for a menu.
    private func short(_ summary: String) -> String {
        let limit = 40
        guard summary.count > limit else { return summary }
        return summary.prefix(limit).trimmingCharacters(in: .whitespaces) + "…"
    }
}

/// What the menu bar item shows when it is not open.
struct MenuBarLabel: View {
    let store: SessionStore

    var body: some View {
        if store.waiting > 0 {
            Label("\(store.waiting)", systemImage: "questionmark.circle.fill")
        } else if let worst = store.usage.rows.max(by: { $0.usedTenths < $1.usedTenths }) {
            Text("\(worst.usedPercent)%")
        } else {
            Image(systemName: "square.split.2x1")
        }
    }
}
