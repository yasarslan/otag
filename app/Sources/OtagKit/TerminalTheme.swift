import Foundation

/// One terminal look, used by every pane whatever is running in it.
///
/// A pane is a pane. The font, the size, the line height, the paper, the
/// cursor, the selection and all sixteen ANSI colours are the same whether
/// Claude Code, Codex or a shell is drawing into it. Two agents that lean on
/// those colours to very different degrees should still look like two windows
/// onto the same terminal.
///
/// The numbers live here, apart from the view that installs them, because
/// they are worth checking on their own: sixteen of them, in the order a
/// terminal numbers them, distinct from each other, a bright half that really
/// is brighter, and enough contrast against the theme's own paper that an
/// agent's diff blocks and highlighted prompts stay readable. All four of
/// those are tested.
public struct TerminalTheme: Identifiable, Sendable, Hashable {
    /// What the preference stores.
    public let id: String
    /// What Settings calls it.
    public let name: String
    /// One line in Settings saying what it is for.
    public let detail: String
    /// Whether this is a dark look, which is what an agent asking the
    /// terminal about its colours is effectively asking.
    public let isDark: Bool
    /// The paper, as `0xRRGGBB`.
    public let background: UInt32
    /// The ink.
    public let foreground: UInt32
    /// The cursor.
    public let cursor: UInt32
    /// Selected text's background.
    public let selection: UInt32
    /// The sixteen ANSI colours: black, red, green, yellow, blue, magenta,
    /// cyan, white, then the eight bright ones.
    public let ansi: [UInt32]

    /// How many colours a terminal palette has.
    public static let ansiCount = 16

    /// Daylight, and what a light system appearance gets.
    ///
    /// Deliberately muted and on the dark side of each hue: otag's rule is
    /// attention through thin borders, not saturated colour, and a colour
    /// has to carry meaning on near-white paper.
    public static let glassLight = TerminalTheme(
        id: "glass-light",
        name: "Glass Light",
        detail: "Near-white paper, muted ink. For daylight.",
        isDark: false,
        background: 0xFBFBFD,
        foreground: 0x1C1C1E,
        cursor: 0x1C1C1E,
        selection: 0xD4E3F7,
        ansi: [
            0x2E2E33, 0x8C2F28, 0x33662E, 0x7A5500,
            0x1F4F8F, 0x6B3B80, 0x1A6066, 0xB9B9B5,
            0x5F5F66, 0xA8453C, 0x45803E, 0x8F6A10,
            0x2A68B0, 0x82508F, 0x24767E, 0xE4E4DE,
        ]
    )

    /// What a dark system appearance gets.
    public static let glassDark = TerminalTheme(
        id: "glass-dark",
        name: "Glass Dark",
        detail: "Near-black paper, muted ink. The quiet default.",
        isDark: true,
        background: 0x1C1C1E,
        foreground: 0xE8E8ED,
        cursor: 0xE8E8ED,
        selection: 0x2F4A6B,
        ansi: [
            0x3A3A3F, 0xD4756A, 0x8FB573, 0xD9B365,
            0x6FA8D4, 0xB48EAD, 0x74B0B5, 0xC9C9C4,
            0x6A6A70, 0xE89489, 0xA8CE8F, 0xEFCE84,
            0x8FC2EA, 0xC9A8C4, 0x92CBD0, 0xECECE6,
        ]
    )

    /// Deep navy and high contrast, for a long session.
    ///
    /// Brighter and more saturated than Glass Dark: the same muted red that
    /// reads well on near-black loses too much against navy, so every colour
    /// here is pushed until it clears the same bar.
    public static let midnight = TerminalTheme(
        id: "midnight",
        name: "Midnight",
        detail: "Deep navy, high contrast. For a long session.",
        isDark: true,
        background: 0x0B1020,
        foreground: 0xE9F0FA,
        cursor: 0x79C0FF,
        selection: 0x1E3356,
        ansi: [
            0x1B2338, 0xFF7B72, 0x7EE787, 0xF2CC60,
            0x79C0FF, 0xD2A8FF, 0x76E3EA, 0xC3CCDA,
            0x4A5571, 0xFFA198, 0xA5F3AE, 0xFFE08A,
            0xA6D8FF, 0xE2C5FF, 0xA0F0F5, 0xF0F6FC,
        ]
    )

    /// Every built-in theme, in the order Settings offers them.
    public static let all: [TerminalTheme] = [glassLight, glassDark, midnight]

    /// The theme with this identifier, or nothing when a preference names
    /// one this build does not have.
    public static func named(_ id: String) -> TerminalTheme? {
        all.first { $0.id == id }
    }

    /// The colours numbered 0 to 15, which is what a terminal wants.
    public var ansiColours: [UInt32] { ansi }
}

/// Which terminal theme the user chose.
///
/// Following the system is the default, and it is the one most people should
/// leave alone: otag's panes then match the rest of their screen without
/// anyone picking anything.
public enum TerminalThemeChoice: String, Sendable, CaseIterable, Identifiable {
    /// Glass Light in a light appearance, Glass Dark in a dark one.
    case system
    case glassLight = "glass-light"
    case glassDark = "glass-dark"
    case midnight

    public var id: String { rawValue }

    /// What Settings calls it.
    public var label: String {
        switch self {
        case .system: return "Follow the system"
        case .glassLight: return TerminalTheme.glassLight.name
        case .glassDark: return TerminalTheme.glassDark.name
        case .midnight: return TerminalTheme.midnight.name
        }
    }

    /// The theme this means in the appearance the window is being drawn in.
    public func resolved(dark: Bool) -> TerminalTheme {
        switch self {
        case .system: return dark ? .glassDark : .glassLight
        case .glassLight: return .glassLight
        case .glassDark: return .glassDark
        case .midnight: return .midnight
        }
    }

    /// The choice a stored preference means, falling back to the system for
    /// anything this build does not recognise.
    public static func stored(_ raw: String?) -> TerminalThemeChoice {
        guard let raw, let choice = TerminalThemeChoice(rawValue: raw) else { return .system }
        return choice
    }
}

/// Arithmetic on the colours, kept apart so it can be checked on its own.
public enum TerminalPalette {
    /// One colour's three components, each 0 to 255.
    public static func components(_ value: UInt32) -> (red: UInt16, green: UInt16, blue: UInt16) {
        (
            red: UInt16((value >> 16) & 0xFF),
            green: UInt16((value >> 8) & 0xFF),
            blue: UInt16(value & 0xFF)
        )
    }

    /// How light a colour is, 0 to 1, by the usual perceptual weighting.
    ///
    /// Used only to check a palette against itself: a bright colour that is
    /// not brighter than its normal counterpart would make bold text
    /// disappear rather than stand out.
    public static func luminance(_ value: UInt32) -> Double {
        let parts = components(value)
        return 0.2126 * Double(parts.red) / 255
            + 0.7152 * Double(parts.green) / 255
            + 0.0722 * Double(parts.blue) / 255
    }

    /// A colour's relative luminance as the contrast formula defines it,
    /// with the sRGB transfer function undone.
    public static func relativeLuminance(_ value: UInt32) -> Double {
        let parts = components(value)
        func linear(_ component: UInt16) -> Double {
            let scaled = Double(component) / 255
            return scaled <= 0.03928
                ? scaled / 12.92
                : pow((scaled + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * linear(parts.red)
            + 0.7152 * linear(parts.green)
            + 0.0722 * linear(parts.blue)
    }

    /// The contrast ratio between two colours, 1 to 21.
    ///
    /// This is the number that decides whether an agent's diff block is
    /// readable, so it is the number the themes are tested against rather
    /// than anybody's opinion of the screenshot.
    public static func contrast(_ one: UInt32, _ other: UInt32) -> Double {
        let first = relativeLuminance(one)
        let second = relativeLuminance(other)
        let lighter = max(first, second)
        let darker = min(first, second)
        return (lighter + 0.05) / (darker + 0.05)
    }

    /// What body text has to clear against its own background.
    public static let textContrast = 7.0

    /// What a colour that carries meaning — red through cyan, and their
    /// bright halves — has to clear.
    public static let colourContrast = 4.5

    /// The colours that carry meaning, by their terminal numbers.
    ///
    /// Not the four greys: 0, 7, 8 and 15 are the paper-and-ink end of the
    /// palette, and a program using them against this background has
    /// chosen that, where red and green mean removed and added.
    public static let meaningfulColours = Array(1 ... 6) + Array(9 ... 14)
}
