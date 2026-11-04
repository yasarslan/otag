import AppKit
import OtagKit
import SwiftTerm

/// One terminal look for every session, whatever is running in it.
///
/// A pane is a pane: the same font at the same size, the same line height,
/// the same background and the same sixteen ANSI colours, whether Claude
/// Code, Codex or a shell is drawing into it. Before this, each pane took
/// SwiftTerm's built-in palette, which is fixed regardless of the system
/// appearance, while the background came from the appearance — so the two
/// agents, which lean on those colours to very different degrees, did not
/// look like the same program.
///
/// Nothing here is configurable beyond the font size in Settings. An agent's
/// own colours are its own business; this is the surface it draws onto, and
/// one surface is the point.
///
/// ## What the agents can be told
///
/// otag does not write to an agent's configuration, so the only way to make
/// an agent fit the pane is through the terminal itself. Both panes are
/// started with `TERM=xterm-256color` and `COLORTERM=truecolor` by the
/// daemon, and a pane answers the OSC 10 and OSC 11 queries ("what are your
/// foreground and background?") with exactly the colours below, so an agent
/// that asks is told the truth rather than guessing from the terminal's name.
///
/// Codex's own `tui.theme` setting, and its `/theme` command, choose a
/// *syntax highlighting* theme for code blocks (the values are the usual
/// `.tmTheme` names); there is no supported setting that makes the rest of
/// its interface follow the terminal's palette. So there is nothing to set
/// on its side, and otag never touches `~/.codex` either way.
enum TerminalProfile {
    /// The font every pane uses, at the size the user chose.
    static func font(size: Double) -> NSFont {
        NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
    }

    /// How tall a line is, as a multiple of the font's own height.
    ///
    /// One is the font's natural height, which is tight for a terminal that
    /// is read all day. Slightly more is easier on the eye and, more to the
    /// point here, it is the *same* slightly more in every pane.
    static let lineSpacing: CGFloat = 1.1

    /// Put the whole profile onto a terminal view.
    static func apply(to view: TerminalView, size: Double) {
        view.font = font(size: size)
        view.lineSpacing = lineSpacing
        // The paper the agents draw on comes from the user's appearance, like
        // every other calm part of otag: nothing is painted that the system
        // has not chosen. These two are also what a pane answers an OSC 10 or
        // OSC 11 query with.
        view.nativeBackgroundColor = .textBackgroundColor
        view.nativeForegroundColor = .textColor
        view.installColors(ansiColors(dark: isDark(view)))
    }

    /// Whether this view is being drawn in a dark appearance.
    static func isDark(_ view: NSView) -> Bool {
        view.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
    }

    /// The sixteen ANSI colours, in the order a terminal numbers them.
    ///
    /// The numbers themselves live in `OtagKit.TerminalPalette`, where they
    /// can be checked on their own. One palette per appearance rather than
    /// one for both: the same red has to be readable on white and on
    /// near-black, and a single set cannot be.
    static func ansiColors(dark: Bool) -> [SwiftTerm.Color] {
        TerminalPalette.colors(dark: dark).map { value in
            let parts = TerminalPalette.components(value)
            return SwiftTerm.Color(red8: parts.red, green8: parts.green, blue8: parts.blue)
        }
    }
}
