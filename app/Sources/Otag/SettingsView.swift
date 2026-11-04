import OtagKit
import SwiftUI

/// The handful of things worth setting.
///
/// Deliberately short: otag's defaults are meant to be right, and a wall of
/// settings is how an app admits they are not. Everything here changes how
/// the app looks or how loudly it speaks — never what the daemon does, which
/// belongs to the daemon. The one exception is the button that offers to
/// match the agents' own themes, which says exactly what it would change and
/// does nothing until it is pressed.
enum Preference {
    /// Size of the terminal font, in points.
    static let fontSize = "terminalFontSize"
    /// Which terminal theme every pane uses.
    static let theme = "terminalTheme"
    /// Whether a session that is doing nothing is drawn dimmer.
    static let dimIdle = "dimIdleSessions"
    /// Whether the app posts notifications itself.
    static let notify = "postNotifications"
    /// The agent the new-session sheet offers, which is the last one used.
    static let lastAgent = "lastSessionAgent"
    /// Whether the user has been offered the agents' own themes once.
    static let offeredAgentThemes = "offeredAgentThemes"

    /// The size a terminal uses when nothing has been chosen.
    static let defaultFontSize: Double = 13
}

/// The settings window.
struct SettingsView: View {
    var body: some View {
        TabView {
            AppearanceSettings()
                .tabItem { Label("Appearance", systemImage: "paintpalette") }
            AttentionSettings()
                .tabItem { Label("Attention", systemImage: "bell") }
        }
        .frame(width: 480, height: 520)
    }
}

/// How otag looks: the theme every pane uses, and how big its text is.
private struct AppearanceSettings: View {
    @AppStorage(Preference.theme) private var theme = TerminalThemeChoice.system.rawValue
    @AppStorage(Preference.fontSize) private var fontSize = Preference.defaultFontSize
    @AppStorage(Preference.dimIdle) private var dimIdle = true

    @Environment(\.colorScheme) private var scheme
    @State private var offering = false

    var body: some View {
        Form {
            Section("Terminal theme") {
                Picker("Theme", selection: $theme) {
                    ForEach(TerminalThemeChoice.allCases) { choice in
                        Text(choice.label).tag(choice.rawValue)
                    }
                }
                .labelsHidden()
                .pickerStyle(.inline)

                ThemePreview(theme: resolved, fontSize: fontSize)
                    .padding(.vertical, 4)

                Text(resolved.detail + " Every session uses this theme, whatever is running in "
                    + "it — a Claude pane and a Codex pane are the same terminal. Changing it "
                    + "reaches every open session at once.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Section("Text") {
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
                Toggle("Dim sessions that are doing nothing", isOn: $dimIdle)
            }

            Section("The agents' own colours") {
                Text("Every pane already tells an agent what colour it is, so an agent that "
                    + "asks follows this theme by itself. Both agents also have a theme "
                    + "setting of their own, which otag can match — with your permission.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Match the agents to this theme…") { offering = true }
            }
        }
        .formStyle(.grouped)
        .sheet(isPresented: $offering) {
            AgentThemeDialog(theme: resolved)
        }
    }

    /// The theme this choice means in the appearance the window is in.
    private var resolved: TerminalTheme {
        TerminalThemeChoice.stored(theme).resolved(dark: scheme == .dark)
    }
}

/// How loudly otag speaks.
private struct AttentionSettings: View {
    @AppStorage(Preference.notify) private var notify = true

    var body: some View {
        Form {
            Section {
                Toggle("Notify me when a session needs me", isOn: $notify)
                Text("With notifications off, the Dock badge and the menu bar still show who "
                    + "is waiting. With the app closed, the daemon notifies instead.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .formStyle(.grouped)
    }
}

/// A theme, as it will actually look.
///
/// A real terminal screen rather than sixteen swatches: a prompt, a diff
/// block, bold and dim text, and the palette underneath. Those are the
/// things an agent draws, so those are the things worth seeing before
/// choosing.
struct ThemePreview: View {
    let theme: TerminalTheme
    let fontSize: Double

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            line([
                (6, "~/work/api"), (4, " main"), (15, " $ "), (15, "claude"),
            ])
            line([(5, "  ✻ "), (7, "Thinking about the parser…")])
            line([(2, "  + "), (2, "fn parse(line: &str) -> Option<Row> {")])
            line([(1, "  - "), (1, "fn parse(line: &str) -> Row {")])
            line([(3, "  ! "), (3, "3 tests need updating")])
            line([(4, "  → "), (12, "crates/otag-core/src/proto.rs"), (8, ":481")])

            HStack(spacing: 2) {
                ForEach(0 ..< TerminalTheme.ansiCount, id: \.self) { index in
                    Rectangle()
                        .fill(colour(index))
                        .frame(height: 8)
                }
            }
            .padding(.top, 5)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(colour(background: true), in: RoundedRectangle(cornerRadius: 6))
        .overlay {
            RoundedRectangle(cornerRadius: 6).strokeBorder(.quaternary)
        }
        .accessibilityLabel("\(theme.name) preview")
    }

    /// One line of the mock, as runs of coloured text.
    private func line(_ runs: [(Int, String)]) -> some View {
        HStack(spacing: 0) {
            ForEach(Array(runs.enumerated()), id: \.offset) { _, run in
                Text(run.1)
                    .foregroundStyle(colour(run.0))
            }
            Spacer(minLength: 0)
        }
        .font(.system(size: max(fontSize - 2, 8), design: .monospaced))
        .lineLimit(1)
    }

    private func colour(_ index: Int) -> Color {
        Self.swiftUI(theme.ansi[index])
    }

    private func colour(background: Bool) -> Color {
        Self.swiftUI(background ? theme.background : theme.foreground)
    }

    /// One `0xRRGGBB` as a SwiftUI colour, in sRGB so it is the number the
    /// terminal will actually draw.
    static func swiftUI(_ value: UInt32) -> Color {
        let parts = TerminalPalette.components(value)
        return Color(
            .sRGB,
            red: Double(parts.red) / 255,
            green: Double(parts.green) / 255,
            blue: Double(parts.blue) / 255
        )
    }
}
