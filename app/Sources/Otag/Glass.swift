import OtagKit
import SwiftUI

/// The window's chrome, and the one place that knows what the system can do.
///
/// macOS 26 draws chrome as Liquid Glass; macOS 14 and 15 draw it as a
/// material. The difference is real but it is not interesting to a view, so
/// it lives here: a sidebar asks for `.chromeBackground()` and gets whichever
/// the system has. No view in otag writes an availability check.
///
/// Glass is for **chrome only** — the sidebar, the header bar, the limits
/// card, the empty-state card. A terminal stays opaque: text an agent drew
/// must not have the desktop showing through it, and a pane paints its
/// theme's background flat.
enum Chrome {
    /// How round a card is.
    static let cardRadius: CGFloat = 12
    /// How round a control is.
    static let controlRadius: CGFloat = 8
    /// The gap between things that belong together.
    static let gap: CGFloat = 8
    /// The gap between things that do not.
    static let gutter: CGFloat = 16
    /// How tall a header bar is.
    static let headerHeight: CGFloat = 32
}

extension View {
    /// The background of a region of chrome that fills its own edge of the
    /// window: the sidebar, the header bar.
    @ViewBuilder
    func chromeBackground() -> some View {
        if #available(macOS 26, *) {
            background(.clear).glassEffect(.regular, in: Rectangle())
        } else {
            background(.regularMaterial)
        }
    }

    /// The background of a card that floats over something: the limits card,
    /// the empty state, a notice.
    @ViewBuilder
    func cardBackground(radius: CGFloat = Chrome.cardRadius) -> some View {
        if #available(macOS 26, *) {
            glassEffect(.regular, in: RoundedRectangle(cornerRadius: radius))
        } else {
            background(.regularMaterial, in: RoundedRectangle(cornerRadius: radius))
                .overlay {
                    RoundedRectangle(cornerRadius: radius)
                        .strokeBorder(.quaternary, lineWidth: 1)
                }
        }
    }

    /// The one primary button in a header bar.
    ///
    /// `prominent` when the agent is running out of room, which is the only
    /// time otag raises its voice. The same button either way, so nothing on
    /// the header ever moves.
    @ViewBuilder
    func primaryActionStyle(prominent: Bool) -> some View {
        if #available(macOS 26, *) {
            if prominent {
                buttonStyle(.glassProminent).controlSize(.small)
            } else {
                buttonStyle(.glass).controlSize(.small)
            }
        } else {
            if prominent {
                buttonStyle(.borderedProminent).controlSize(.small)
            } else {
                buttonStyle(.bordered).controlSize(.small)
            }
        }
    }

    /// A small round button that sits on chrome: the `⋯` menu, a close.
    @ViewBuilder
    func chromeButtonStyle() -> some View {
        if #available(macOS 26, *) {
            buttonStyle(.glass).controlSize(.small)
        } else {
            buttonStyle(.bordered).controlSize(.small)
        }
    }

    /// Groups several pieces of glass so they blend into each other rather
    /// than stacking up. Nothing on macOS 14 and 15, where there is no glass
    /// to blend.
    @ViewBuilder
    func glassGroup(spacing: CGFloat = Chrome.gap) -> some View {
        if #available(macOS 26, *) {
            GlassEffectContainer(spacing: spacing) { self }
        } else {
            self
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
            .background(colour.opacity(0.14), in: Capsule())
            .help(status.localizedPrecise)
            .accessibilityLabel(status.localizedPrecise)
    }

    private var colour: Color {
        switch status.badge {
        case .needsYou: return status == .error ? .red : .accentColor
        case .working: return .accentColor
        case .done: return .green
        case .idle: return .secondary
        }
    }
}
