import OtagKit
import SwiftUI

/// Something the window is asking the user about one session.
///
/// `sheet(item:)` wants something identifiable, and all four of these are
/// "this session, plus which question", so they are one type.
struct SessionPrompt: Identifiable {
    enum Kind {
        /// Which model to continue on.
        case model
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

/// The one strip above a session: what it is, and the two controls it has.
///
/// Left: the agent, the project and what it is doing, as plain text. Right:
/// **one** primary button — the handoff — and **one** `⋯` menu. Nothing else
/// goes here, ever. The strip before this had six symbol buttons and a
/// tooltip for each, which is six things to learn in the place a person is
/// trying to read a terminal.
struct SessionHeaderBar: View {
    let actions: SessionActions
    /// Every session the daemon is running, for `Split with…`.
    let everything: [PaneInfo]
    /// Whether this session has the keyboard, which only matters when it is
    /// sharing the window.
    let focused: Bool
    /// Whether it is sharing the window at all.
    let sharing: Bool
    /// Ask the window something about this session.
    let ask: (SessionPrompt.Kind) -> Void

    var body: some View {
        HStack(spacing: Chrome.gap) {
            AgentMark(agent: actions.pane.agent)

            Text(actions.name)
                .font(.system(size: 12, weight: focused && sharing ? .semibold : .medium))
                .lineLimit(1)
                .truncationMode(.middle)

            if !actions.pane.workspace.isEmpty {
                Text(actions.pane.workspace)
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Text(actions.pane.status.precise)
                .font(.system(size: 12))
                .foregroundStyle(actions.pane.status.badge.wantsAttention
                    ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
                .lineLimit(1)

            if actions.isOnNightShift {
                Image(systemName: "moon.stars")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .help("The night shift is working on this session")
            }

            Spacer(minLength: Chrome.gap)

            if actions.isHandingOff {
                Text("switching…")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            } else if actions.canHandOff {
                Button(actions.handOffTitle) { actions.handOff() }
                    .primaryActionStyle(prominent: actions.isRunningOut)
                    .help(actions.isRunningOut
                        ? "\(actions.handOffTitle) — this agent is nearly out of room"
                        : "\(actions.handOffTitle), with a package it can read")
            }

            Menu {
                SessionMenuItems(actions: actions, everything: everything, ask: ask)
            } label: {
                Image(systemName: "ellipsis")
            }
            .menuStyle(.button)
            .chromeButtonStyle()
            .menuIndicator(.hidden)
            .fixedSize()
            .help("Everything else about this session")
        }
        .padding(.horizontal, Chrome.gutter)
        .frame(height: Chrome.headerHeight)
        .chromeBackground()
        .overlay(alignment: .bottom) { Divider() }
    }
}

/// The `⋯` menu, and the sidebar's right click.
///
/// One list in one order, used in both places, so a right click and the
/// header can never drift apart. `Split with…` is the only item the sidebar
/// leaves out, because splitting is relative to a session that is showing.
struct SessionMenuItems: View {
    let actions: SessionActions
    let everything: [PaneInfo]
    let ask: (SessionPrompt.Kind) -> Void
    /// Whether this is the header's menu, which can offer a split.
    var canSplit = true

    var body: some View {
        if canSplit {
            Button("Split with…", systemImage: "square.split.2x1") { ask(.split) }
                .disabled(actions.splitCandidates(among: everything).isEmpty)
        }

        Button("Lighter model…", systemImage: "arrow.down.right.circle") { ask(.model) }
            .disabled(actions.pane.agent == nil)

        if actions.isOnNightShift {
            Button("Stop the night shift", systemImage: "moon.zzz") { actions.stopNightShift() }
        } else {
            Button("Night shift…", systemImage: "moon.stars") { ask(.nightShift) }
                .disabled(actions.pane.agent == nil)
        }

        Divider()

        Button("Rename…", systemImage: "pencil") { ask(.rename) }
        Button("Morning report…", systemImage: "sunrise") { actions.askForMorningReport() }

        Divider()

        Button("Close", systemImage: "xmark") { actions.close() }
    }
}

/// The sidebar's right click: the same list, without the split.
struct SessionContextMenu: View {
    let actions: SessionActions
    let ask: (SessionPrompt.Kind) -> Void

    var body: some View {
        Button(actions.handOffTitle, systemImage: "arrow.left.arrow.right") {
            actions.handOff()
        }
        .disabled(!actions.canHandOff)

        SessionMenuItems(actions: actions, everything: [], ask: ask, canSplit: false)
    }
}
