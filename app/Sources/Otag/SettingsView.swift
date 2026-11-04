import OtagKit
import SwiftUI

/// The handful of things worth setting.
///
/// Deliberately short: otag's defaults are meant to be right, and a wall of
/// settings is how an app admits they are not. Everything here changes how
/// the app looks or how loudly it speaks — never what the daemon does, which
/// belongs to the daemon.
enum Preference {
    /// Size of the terminal font, in points.
    static let fontSize = "terminalFontSize"
    /// Whether a session that is doing nothing is drawn dimmer.
    static let dimIdle = "dimIdleSessions"
    /// Whether the app posts notifications itself.
    static let notify = "postNotifications"
    /// The agent the new-session sheet offers, which is the last one used.
    static let lastAgent = "lastSessionAgent"

    /// The size a terminal uses when nothing has been chosen.
    static let defaultFontSize: Double = 13
}

/// The settings window.
struct SettingsView: View {
    @AppStorage(Preference.fontSize) private var fontSize = Preference.defaultFontSize
    @AppStorage(Preference.dimIdle) private var dimIdle = true
    @AppStorage(Preference.notify) private var notify = true

    var body: some View {
        Form {
            Section("Terminal") {
                HStack {
                    Text("Font size")
                    Spacer()
                    Text("\(Int(fontSize)) pt")
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                }
                Slider(value: $fontSize, in: 9 ... 20, step: 1)
                Text("Sessions are resized to fit, so the agent is told the truth about how "
                    + "much room it has.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section("Attention") {
                Toggle("Dim sessions that are doing nothing", isOn: $dimIdle)
                Toggle("Notify me when a session needs me", isOn: $notify)
                Text("With notifications off, the Dock badge and the menu bar still show who "
                    + "is waiting. With the app closed, the daemon notifies instead.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section("Appearance") {
                Text("otag follows the system's light and dark appearance, and takes the "
                    + "terminal's colours from it.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .frame(width: 420, height: 420)
    }
}
