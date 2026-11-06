import OtagKit
import SwiftUI

/// Something the window is asking the user about one session.
///
/// `sheet(item:)` wants something identifiable, and all of these are "this
/// session, plus which question", so they are one type.
struct SessionPrompt: Identifiable {
    enum Kind {
        /// Which session to put beside this one.
        case split
        /// What to call it.
        case rename
        /// What to queue overnight.
        case nightShift
    }

    let pane: PaneID
    let kind: Kind

    var id: String { "\(pane)-\(kind)" }
}

/// The one strip above a session: what it is, and a close button.
///
/// It used to hold six symbol buttons; then one primary button and a `⋯`.
/// Now it holds no menu at all. Everything that was in that menu went
/// somewhere a person can see it — the toolbar's two popovers, the sidebar's
/// right click, the Session menu — and what is left here is the one thing a
/// header should be: a label saying which session this is.
///
/// `nickname · name · project · status`, and nothing twice.
struct SessionHeaderBar: View {
    let actions: SessionActions
    /// Whether this session has the keyboard, which only matters when it is
    /// sharing the window.
    let focused: Bool
    /// Whether it is sharing the window at all.
    let sharing: Bool

    var body: some View {
        HStack(spacing: Chrome.gap) {
            AgentMark(agent: actions.pane.agent)

            ForEach(Array(parts.enumerated()), id: \.offset) { index, part in
                if index > 0 { Separator() }
                Text(part.text)
                    .font(.system(size: 12, weight: part.weight))
                    .foregroundStyle(part.style)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            if actions.isGuarded {
                Image(systemName: "shield.fill")
                    .font(.system(size: 10))
                    .foregroundStyle(guardIsHolding ? AnyShapeStyle(.tint)
                        : AnyShapeStyle(.secondary))
                    .help(Strings.t(guardIsHolding
                        ? "guard.help.stopped" : "guard.help.header"))
            }
            if actions.isOnNightShift {
                Image(systemName: "moon.stars")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .help(Strings.t("header.nightshift"))
            }

            Spacer(minLength: Chrome.gap)

            if let team = actions.pane.team {
                if team.current {
                    if team.waiting {
                        Button(Strings.t("team.startStep")) { advance(team) }
                    } else if team.manual || actions.pane.status == .idle || actions.pane.status == .done {
                        Button(Strings.t("team.next")) { advance(team) }
                        if team.index == 2 {
                            Button(Strings.t("team.back")) { advance(team, back: true) }
                        }
                    }
                } else {
                    Text(Strings.t("team.waiting")).font(.caption).foregroundStyle(.secondary)
                }
            }

            Button(action: actions.close) {
                Image(systemName: "xmark")
                    .font(.system(size: 10, weight: .semibold))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .help(Strings.t("header.close"))
            .accessibilityLabel(Strings.t("header.close.label"))
        }
        .padding(.horizontal, Chrome.gutter)
        .frame(height: Chrome.headerHeight)
        .chromeBackground()
        .overlay(alignment: .bottom) { Divider() }
        .help(actions.pane.agent.name)
    }

    private func advance(_ team: TeamMembership, back: Bool = false) {
        actions.store.send(.teamAdvance(pane: actions.pane.id, revision: team.revision, back: back))
    }

    /// One piece of the header's line.
    private struct Part {
        let text: String
        var weight: Font.Weight = .regular
        var style: AnyShapeStyle = AnyShapeStyle(.secondary)
    }

    /// What the header says, with nothing said twice.
    ///
    /// A session named after its folder used to read the folder's name
    /// twice; anything that repeats a part already shown is left out, and
    /// the comparison ignores case so a rename differing only in capitals
    /// cannot slip through.
    private var parts: [Part] {
        var shown: [Part] = []
        func add(_ text: String, weight: Font.Weight = .regular, style: AnyShapeStyle) {
            let wanted = text.trimmingCharacters(in: .whitespaces)
            guard !wanted.isEmpty else { return }
            guard !shown.contains(where: {
                $0.text.caseInsensitiveCompare(wanted) == .orderedSame
            }) else { return }
            shown.append(Part(text: wanted, weight: weight, style: style))
        }

        add(actions.nickname, weight: .semibold, style: AnyShapeStyle(.primary))
        add(
            actions.name,
            weight: actions.nickname.isEmpty
                ? (focused && sharing ? .semibold : .medium) : .regular,
            style: AnyShapeStyle(actions.nickname.isEmpty ? .primary : .secondary)
        )
        add(actions.pane.workspace, style: AnyShapeStyle(.secondary))
        add(
            actions.pane.status.localizedPrecise,
            style: actions.pane.status.badge.wantsAttention
                ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary)
        )
        return shown
    }

    /// Whether the guard has actually stopped this session, rather than only
    /// watching it.
    private var guardIsHolding: Bool {
        guard let plan = actions.pane.guardPlan else { return false }
        return plan.paused || plan.handedOff
    }
}

/// The dot between two pieces of a header.
private struct Separator: View {
    var body: some View {
        Text(verbatim: "·")
            .font(.system(size: 12))
            .foregroundStyle(.tertiary)
            .accessibilityHidden(true)
    }
}

/// The sidebar's right click.
///
/// What is left after the header's menu went: the two names, putting a
/// session beside this one, the night shift, and closing. Everything else is
/// a toolbar button now.
struct SessionContextMenu: View {
    let actions: SessionActions
    let everything: [PaneInfo]
    let ask: (SessionPrompt.Kind) -> Void
    /// Start editing the nickname in the row itself.
    let setNickname: () -> Void

    var body: some View {
        Button(
            Strings.t(actions.nickname.isEmpty ? "menu.nickname.set" : "menu.nickname.change"),
            systemImage: "tag"
        ) { setNickname() }

        Button(Strings.t("menu.rename"), systemImage: "pencil") { ask(.rename) }

        Divider()

        Button(Strings.t("menu.split"), systemImage: "square.split.2x1") { ask(.split) }
            .disabled(actions.splitCandidates(among: everything).isEmpty)

        if actions.isOnNightShift {
            Button(Strings.t("menu.nightshift.stop"), systemImage: "moon.zzz") {
                actions.stopNightShift()
            }
        } else {
            Button(Strings.t("menu.nightshift"), systemImage: "moon.stars") {
                ask(.nightShift)
            }
            .disabled(actions.pane.agent == nil)
        }

        if actions.isGuarded {
            Button(Strings.t("guard.stop.one"), systemImage: "shield.slash") {
                actions.guardLimits(nil)
            }
        }

        Divider()

        Button(Strings.t("menu.close"), systemImage: "xmark") { actions.close() }
    }
}
