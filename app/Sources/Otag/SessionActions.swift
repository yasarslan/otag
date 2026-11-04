import OtagKit
import SwiftUI

/// A session whose model the user is choosing.
///
/// A wrapper rather than the identifier itself, because `PaneID` is a plain
/// number and `sheet(item:)` needs something identifiable.
struct ChoosingModel: Identifiable {
    let id: PaneID
}

/// What there is to do with one session, in one place.
///
/// The same five actions appear above the session's terminal, in the
/// sidebar's context menu and in the palette. Writing them once means a
/// label can only be wrong in one place, and that the three paths cannot
/// drift apart.
@MainActor
struct SessionCommands {
    let store: SessionStore
    let workbench: Workbench
    let pane: PaneInfo
    /// Ask for a session beside this one, on this axis.
    let split: (SplitAxis) -> Void
    /// Ask for the model to continue this session on.
    let continueOnAnotherModel: () -> Void

    /// Named after where the work would go, which is what the user is
    /// choosing between.
    var handOverTitle: String {
        switch pane.agent {
        case .claude: return "Hand Off to Codex"
        case .codex: return "Hand Off to Claude Code"
        case nil: return "Hand Off"
        }
    }

    /// Whether this session can be handed anywhere at all.
    var canHandOver: Bool {
        pane.agent != nil && !isHandingOver
    }

    /// Whether a handoff for this session is already in flight.
    var isHandingOver: Bool {
        store.handingOver.contains(pane.id)
    }

    /// Whether its agent is close enough to its limit that the handoff is
    /// worth drawing attention to.
    var isRunningOut: Bool {
        store.isRunningOut(pane.agent)
    }

    func handOver() {
        guard canHandOver else { return }
        store.handOver(pane: pane.id, handoff: .agent(to: nil))
    }

    /// Stop showing this session. The daemon keeps running it.
    func close() {
        store.send(.detachPane(pane: pane.id))
        workbench.close(pane: pane.id)
    }
}

/// The actions, as menu items.
///
/// Used by the sidebar's context menu and by the overflow menu in a
/// session's header, so a right click and the header offer the same things
/// in the same words.
struct SessionActionItems: View {
    let commands: SessionCommands

    var body: some View {
        Button(commands.handOverTitle, systemImage: "arrow.left.arrow.right") {
            commands.handOver()
        }
        .disabled(!commands.canHandOver)

        Button("Continue on a Lighter Model…", systemImage: "arrow.down.right.circle") {
            commands.continueOnAnotherModel()
        }
        .disabled(commands.pane.agent == nil)

        Divider()

        Button("Split Right…", systemImage: "square.split.2x1") {
            commands.split(.horizontal)
        }
        Button("Split Down…", systemImage: "square.split.1x2") {
            commands.split(.vertical)
        }

        Divider()

        Button("Close This Session", systemImage: "xmark") {
            commands.close()
        }
    }
}

/// The strip above one session's terminal: what it is, and what to do to it.
///
/// Small and quiet — the terminal is the thing on this screen. The buttons
/// show their symbols only, with the words in their tooltips, except the
/// handoff when the agent is running out of room: then it says so.
struct SessionHeader: View {
    let commands: SessionCommands
    /// Whether this session has the keyboard, which only matters when it is
    /// sharing its tab.
    let focused: Bool

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: symbol)
                .font(.system(size: 11))
                .foregroundStyle(commands.pane.status.wantsAttention
                    ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
            Text(name)
                .font(.system(size: 11, weight: focused ? .medium : .regular))
                .lineLimit(1)
            if commands.pane.status != .idle {
                Text(commands.pane.status.label)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Spacer(minLength: 8)

            if commands.isHandingOver {
                Text("handing over…")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }

            // The one action that is ever urgent, so the only one that is
            // ever more than a symbol.
            Button(action: commands.handOver) {
                if commands.isRunningOut {
                    Label(commands.handOverTitle, systemImage: "arrow.left.arrow.right")
                        .font(.system(size: 11))
                } else {
                    Image(systemName: "arrow.left.arrow.right")
                }
            }
            .buttonStyle(.plain)
            .labelStyle(.titleAndIcon)
            .foregroundStyle(commands.isRunningOut ? AnyShapeStyle(.tint)
                : AnyShapeStyle(.secondary))
            .disabled(!commands.canHandOver)
            .help(commands.isRunningOut
                ? "\(commands.handOverTitle) — this agent is nearly out of room"
                : "\(commands.handOverTitle), with a package it can read")

            HeaderButton(
                symbol: "arrow.down.right.circle",
                help: "Continue this same session on another model",
                action: commands.continueOnAnotherModel
            )
            .disabled(commands.pane.agent == nil)

            HeaderButton(symbol: "square.split.2x1", help: "A session beside this one (⌘D)") {
                commands.split(.horizontal)
            }
            HeaderButton(symbol: "square.split.1x2", help: "A session below this one (⇧⌘D)") {
                commands.split(.vertical)
            }
            HeaderButton(symbol: "xmark", help: "Stop showing this session") {
                commands.close()
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(.bar)
        .overlay(alignment: .bottom) { Divider() }
    }

    /// What the session is called: its own title, or the agent in it.
    private var name: String {
        let title = commands.pane.title
        return title.isEmpty ? (commands.pane.agent?.label ?? "shell") : title
    }

    private var symbol: String {
        commands.pane.agent == nil ? "terminal" : "sparkles"
    }
}

/// One symbol in a session's header.
private struct HeaderButton: View {
    let symbol: String
    let help: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 11))
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
        .help(help)
    }
}

/// Asking which model to continue a session on.
///
/// A sheet rather than a popover, because it is reached from the header and
/// from a right click in the sidebar, and only one of those has a button to
/// hang a popover on.
struct LighterModelSheet: View {
    let store: SessionStore
    let pane: PaneID

    @Environment(\.dismiss) private var dismiss
    @State private var model = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Continue on another model")
                .font(.headline)
            Text("The conversation is kept; only the model changes. Leave this empty for "
                + "the agent's fallback, where otag has a verified one.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            TextField("Model", text: $model, prompt: Text("sonnet"))
                .textFieldStyle(.roundedBorder)
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Continue") {
                    store.handOver(
                        pane: pane,
                        handoff: .model(model.trimmingCharacters(in: .whitespaces))
                    )
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 400)
    }
}
