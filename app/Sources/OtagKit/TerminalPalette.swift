import Foundation

/// The sixteen ANSI colours every otag pane draws with.
///
/// A pane is a pane: whether Claude Code, Codex or a shell is drawing into
/// it, the colours numbered 0 to 15 mean the same thing. Before this, each
/// pane took the emulator's built-in palette — fixed regardless of the system
/// appearance — while the background came from the appearance, so two agents
/// that lean on those colours to very different degrees did not look like the
/// same program.
///
/// The numbers live here, apart from the view that installs them, because
/// they are worth checking on their own: sixteen of them, in the order a
/// terminal numbers them, distinct from each other, and a bright half that
/// really is brighter than the normal half.
public enum TerminalPalette {
    /// How many colours a terminal palette has.
    public static let count = 16

    /// The palette for a dark pane, as `0xRRGGBB`.
    ///
    /// black, red, green, yellow, blue, magenta, cyan, white, then the eight
    /// bright ones. Chosen for contrast against a near-black background and
    /// deliberately muted: otag's rule is attention through thin borders, not
    /// through saturated colour.
    public static let dark: [UInt32] = [
        0x2B2B2B, 0xD4756A, 0x8FB573, 0xD9B365, 0x6FA8D4, 0xB48EAD, 0x74B0B5, 0xC9C9C4,
        0x5A5A5A, 0xE89489, 0xA8CE8F, 0xEFCE84, 0x8FC2EA, 0xC9A8C4, 0x92CBD0, 0xECECE6,
    ]

    /// The same sixteen for a light pane: darker, so that they are readable
    /// on white rather than washed out.
    public static let light: [UInt32] = [
        0x2B2B2B, 0xA3312A, 0x4C7A2F, 0x8A6400, 0x2A5F9E, 0x7A4C8A, 0x1F6F75, 0xBFBFBA,
        0x6B6B6B, 0xC2493F, 0x5F9140, 0xA37A12, 0x3D74B5, 0x8F5FA0, 0x2E8189, 0xE6E6E0,
    ]

    /// The palette for an appearance.
    public static func colors(dark isDark: Bool) -> [UInt32] {
        isDark ? dark : light
    }

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
    /// Used only to check the palettes against themselves: a bright colour
    /// that is not brighter than its normal counterpart would make bold text
    /// disappear.
    public static func luminance(_ value: UInt32) -> Double {
        let parts = components(value)
        let red = Double(parts.red) / 255
        let green = Double(parts.green) / 255
        let blue = Double(parts.blue) / 255
        return 0.2126 * red + 0.7152 * green + 0.0722 * blue
    }
}
