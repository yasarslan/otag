import AppKit
import OtagKit
import SwiftTerm

/// One terminal look for every session, whatever is running in it.
///
/// A pane is a pane: the same font at the same size, the same line height,
/// the same paper, the same cursor and the same sixteen ANSI colours, whether
/// Claude Code, Codex or a shell is drawing into it. Before this, each pane
/// took the emulator's built-in palette — fixed regardless of anything — while
/// the background came from the system appearance, so two agents that lean on
/// those colours to very different degrees did not look like the same program.
///
/// The numbers live in `OtagKit.TerminalTheme`, where they can be checked on
/// their own: three themes, sixteen colours each, and a tested contrast ratio
/// against each theme's own paper.
///
/// ## What the agents are told
///
/// otag does not write to an agent's configuration unless the user presses
/// the button in Settings that says it will (see `AgentTheme`). The mechanism
/// that needs no permission is the terminal itself: every pane is started
/// with `TERM=xterm-256color` and `COLORTERM=truecolor` by the daemon, and a
/// pane answers the `OSC 10` and `OSC 11` queries — "what are your foreground
/// and background?" — with exactly the theme's own colours. An agent that
/// asks is told the truth rather than guessing from the terminal's name, and
/// follows the otag theme with nothing configured.
///
/// Codex probes a terminal's default colours at startup for exactly this
/// reason. Claude Code's own auto theme detects the terminal's background the
/// same way.
enum TerminalProfile {
    /// The font every pane uses, at the size the user chose.
    static func font(size: Double) -> NSFont {
        NSFont(name: "SFMono-Regular", size: size)
            ?? NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
    }

    /// How tall a line is, as a multiple of the font's own height.
    ///
    /// One is the font's natural height, which is tight for a terminal that
    /// is read all day. Slightly more is easier on the eye and, more to the
    /// point here, it is the *same* slightly more in every pane.
    static let lineSpacing: CGFloat = 1.1

    /// Put a whole theme onto a terminal view.
    ///
    /// Everything about the look is set here and nothing is left to the
    /// emulator's defaults, which is what makes two panes identical.
    static func apply(to view: TerminalView, theme: TerminalTheme, size: Double) {
        view.font = font(size: size)
        view.lineSpacing = lineSpacing
        // The paper the agents draw on, and what a pane answers an OSC 10 or
        // OSC 11 query with.
        view.nativeBackgroundColor = colour(theme.background)
        view.nativeForegroundColor = colour(theme.foreground)
        view.caretColor = colour(theme.cursor)
        view.selectedTextBackgroundColor = colour(theme.selection)
        view.installColors(ansi(theme))
    }

    /// Keeps legacy settings readable while offering the two dark palettes.
    static func choice(for stored: String) -> TerminalThemeChoice {
        TerminalThemeChoice.stored(stored) == .midnight ? .midnight : .glassDark
    }

    static func theme(for stored: String) -> TerminalTheme {
        choice(for: stored).resolved(dark: true)
    }

    /// A theme's sixteen colours, in the order a terminal numbers them.
    static func ansi(_ theme: TerminalTheme) -> [SwiftTerm.Color] {
        theme.ansi.map { value in
            let parts = TerminalPalette.components(value)
            return SwiftTerm.Color(red8: parts.red, green8: parts.green, blue8: parts.blue)
        }
    }

    /// One `0xRRGGBB` as a colour AppKit understands.
    static func colour(_ value: UInt32) -> NSColor {
        let parts = TerminalPalette.components(value)
        return NSColor(
            srgbRed: CGFloat(parts.red) / 255,
            green: CGFloat(parts.green) / 255,
            blue: CGFloat(parts.blue) / 255,
            alpha: 1
        )
    }
}
