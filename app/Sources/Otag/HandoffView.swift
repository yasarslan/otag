import OtagKit
import SwiftUI

/// Carrying a session's work somewhere else.
///
/// Two buttons, because there are two honest answers when an agent runs out
/// of road: hand the task to the other agent with a package it can read, or
/// keep the conversation and continue it on a lighter model. The old session
/// is left exactly as it is either way — its window may reset.
struct HandoffButtons: View {
    let store: SessionStore
    let pane: PaneInfo

    @State private var askingForModel = false
    @State private var model = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Button {
                    store.send(.handoff(pane: pane.id, handoff: .agent(to: nil)))
                } label: {
                    Label(handOverTitle, systemImage: "arrow.left.arrow.right")
                }
                .controlSize(.small)
                .help("Write a package and open the other agent beside this session")

                Button {
                    askingForModel = true
                } label: {
                    Label("Model…", systemImage: "arrow.down.right.circle")
                }
                .controlSize(.small)
                .help("Continue this same session on another model")

                Spacer(minLength: 0)
            }
            .disabled(pane.agent == nil)

            if pane.agent == nil {
                Text("Only a session running an agent can be handed over.")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
        .popover(isPresented: $askingForModel, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 8) {
                Text("Continue on another model")
                    .font(.subheadline.weight(.semibold))
                Text("The conversation is kept; only the model changes. Leave this empty for "
                    + "the agent's fallback, where otag has a verified one.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                TextField("model", text: $model, prompt: Text("sonnet"))
                    .textFieldStyle(.roundedBorder)
                HStack {
                    Spacer()
                    Button("Cancel") { askingForModel = false }
                    Button("Continue") {
                        store.send(.handoff(
                            pane: pane.id,
                            handoff: .model(model.trimmingCharacters(in: .whitespaces))
                        ))
                        askingForModel = false
                    }
                    .keyboardShortcut(.defaultAction)
                }
            }
            .padding(14)
            .frame(width: 320)
        }
    }

    /// Named after where the work would go, which is what the user is
    /// choosing between.
    private var handOverTitle: String {
        switch pane.agent {
        case .claude: return "Hand over to Codex"
        case .codex: return "Hand over to Claude Code"
        case nil: return "Hand over"
        }
    }
}
