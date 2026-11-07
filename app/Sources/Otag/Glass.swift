import AppKit
import OtagKit
import SwiftUI

/// Shared window surfaces and dimensions.
enum Chrome {
    static let cardRadius: CGFloat = 12
    static let controlRadius: CGFloat = 8
    static let gap: CGFloat = 8
    static let gutter: CGFloat = 16
    static let headerHeight: CGFloat = 32

    static let sidebar = adaptive(light: 0xF1F2F4, dark: 0x20232A)
    static let header = adaptive(light: 0xE9EBEF, dark: 0x292D35)
    static let card = adaptive(light: 0xF8F9FA, dark: 0x2D313A)
    static let hairline = adaptive(light: 0xCCD0D7, dark: 0x383D46)
    static let splitHairline = Color(nsColor: TerminalProfile.colour(0x454B56))
    static let attention = adaptive(light: 0xA06C24, dark: 0xD5A45E)
    static let paneFocus = adaptive(light: 0x8E8E93, dark: 0x6E6E73)

    private static func adaptive(light: UInt32, dark: UInt32) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            return TerminalProfile.colour(isDark ? dark : light)
        })
    }
}

extension View {
    /// Sets the window and native title bar over the same opaque surface.
    @ViewBuilder
    func workspaceBackground() -> some View {
        if #available(macOS 15, *) {
            containerBackground(Chrome.header, for: .window)
                .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        } else {
            self
        }
    }

    func chromeBackground() -> some View {
        background(Chrome.header)
    }

    func cardBackground(radius: CGFloat = Chrome.cardRadius, floating: Bool = false) -> some View {
        modifier(CardSurface(radius: radius, floating: floating))
    }

    @ViewBuilder
    func primaryActionStyle(prominent: Bool) -> some View {
        if prominent {
            buttonStyle(.borderedProminent).controlSize(.small)
        } else {
            buttonStyle(.bordered).controlSize(.small)
        }
    }

    func chromeButtonStyle() -> some View {
        modifier(ToolbarButtonSurface())
    }
}

/// Only explicitly floating cards use glass, backed by a solid window.
private struct CardSurface: ViewModifier {
    let radius: CGFloat
    let floating: Bool
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: radius)
        if #available(macOS 26, *), floating, !reduceTransparency {
            content.glassEffect(.regular, in: shape)
        } else {
            content.background(Chrome.card, in: shape)
        }
    }
}

/// Toolbar buttons fall back to the system bordered style when glass is disabled.
private struct ToolbarButtonSurface: ViewModifier {
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

    func body(content: Content) -> some View {
        if #available(macOS 26, *), !reduceTransparency {
            content.buttonStyle(.glass).controlSize(.small)
        } else {
            content.buttonStyle(.bordered).controlSize(.small)
        }
    }
}

/// Which agent a session is running, as a colour.
///
/// The only place in the window the two agents are told apart at a glance,
/// which is why it is a colour and not a symbol: both agents used to get
/// `sparkles`, and two identical rows is the same as no information.
extension Optional where Wrapped == AgentKind {
    /// Claude orange, Codex green, and a shell in the quiet grey of
    /// everything else.
    var mark: Color {
        switch self {
        case .claude: return Color(red: 0.851, green: 0.467, blue: 0.341)
        case .codex: return Color(red: 0.063, green: 0.639, blue: 0.498)
        case .deepseek: return Color(red: 0.38, green: 0.49, blue: 0.73)
        case nil: return .secondary
        }
    }

}

/// The small rounded square in front of a session's name.
struct AgentMark: View {
    let agent: AgentKind?
    var size: CGFloat = 8

    var body: some View {
        RoundedRectangle(cornerRadius: size / 3)
            .fill(agent.mark)
            .frame(width: size, height: size)
            .accessibilityLabel(agent.name)
    }
}

/// What a session is doing, as a capsule of one or two words.
///
/// Four words and nothing else: Working, Needs you, Done, Idle. Tinted, not
/// filled; still, not animated. It is a state you glance at.
struct StatusPill: View {
    let status: PaneStatus

    var body: some View {
        Text(Strings.t(status.badge.key))
            .font(.system(size: 10, weight: .medium))
            .foregroundStyle(colour)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(colour.opacity(0.10), in: Capsule())
            .help(status.localizedPrecise)
            .accessibilityLabel(status.localizedPrecise)
    }

    private var colour: Color {
        switch status.badge {
        case .needsYou: return status == .error ? .red : Chrome.attention
        case .working, .done, .idle: return .secondary
        }
    }
}
