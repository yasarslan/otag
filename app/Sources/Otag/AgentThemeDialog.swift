import OtagKit
import SwiftUI

/// Asking whether otag may match the agents' own themes to this one.
///
/// The only dialog in otag that changes a file outside otag, so it is the
/// only one that shows the exact lines it would write, names the files, says
/// a backup is kept, and does nothing at all until the button is pressed.
/// Cancel is the default action.
struct AgentThemeDialog: View {
    let theme: TerminalTheme

    @Environment(\.dismiss) private var dismiss
    @State private var plan: AgentTheme.Plan?
    @State private var outcome: AgentTheme.Outcome?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "paintpalette")
                    .font(.title3)
                    .foregroundStyle(.tint)
                Text("Match the agents to \(theme.name)")
                    .font(.headline)
                Spacer()
            }
            .padding(20)

            Divider()

            Group {
                if let outcome {
                    done(outcome)
                } else if let plan {
                    offer(plan)
                } else {
                    ProgressView().frame(maxWidth: .infinity)
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .topLeading)

            Divider()

            HStack {
                Spacer()
                if outcome != nil {
                    Button("Done") { dismiss() }
                        .keyboardShortcut(.defaultAction)
                } else {
                    Button("Cancel", role: .cancel) { dismiss() }
                        .keyboardShortcut(.cancelAction)
                    Button("Change these files") { apply() }
                        .disabled(plan?.isEmpty ?? true)
                }
            }
            .padding(Chrome.gutter)
        }
        .frame(width: 520)
        .onAppear { plan = AgentTheme.plan(for: theme, home: Self.home) }
    }

    /// What would change.
    private func offer(_ plan: AgentTheme.Plan) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            if plan.isEmpty {
                Text("There is nothing to change.")
                    .font(.callout)
            } else {
                Text("otag would make exactly these changes, and nothing else. A timestamped "
                    + "copy of each file is kept first.")
                    .font(.callout)
                    .fixedSize(horizontal: false, vertical: true)

                ForEach(plan.changes) { change in
                    VStack(alignment: .leading, spacing: 4) {
                        HStack(spacing: 6) {
                            AgentMark(agent: change.agent)
                            Text(change.file)
                                .font(.system(.caption, design: .monospaced))
                        }
                        if let from = change.from {
                            Text("\(change.key) = \"\(from)\"")
                                .font(.system(.caption, design: .monospaced))
                                .foregroundStyle(.secondary)
                                .strikethrough()
                        }
                        Text(change.line)
                            .font(.system(.caption, design: .monospaced))
                        Text(change.effect)
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(
                        .quaternary,
                        in: RoundedRectangle(cornerRadius: Chrome.controlRadius)
                    )
                }
            }

            ForEach(plan.refusals) { refusal in
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Image(systemName: "hand.raised")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Text(refusal.reason)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            Text("Your panes already answer an agent's \"what colour are you?\" query with "
                + "this theme, so this is only about the agents' own accents. Nothing here "
                + "is needed for otag to work.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// What happened.
    private func done(_ outcome: AgentTheme.Outcome) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            if !outcome.written.isEmpty {
                Label(
                    outcome.written.count == 1
                        ? "One file changed." : "\(outcome.written.count) files changed.",
                    systemImage: "checkmark.circle"
                )
                .font(.callout)
                Text("A running agent may need restarting before it picks the theme up.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            ForEach(outcome.backups, id: \.self) { path in
                Text("Backup: \(path)")
                    .font(.system(.caption2, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.head)
            }
            ForEach(outcome.problems, id: \.self) { problem in
                Label(problem, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func apply() {
        guard let plan else { return }
        outcome = AgentTheme.apply(plan, home: Self.home)
    }

    /// The user's home directory, which is where both agents keep their
    /// configuration. A parameter everywhere below this so that a test never
    /// goes near the real one.
    private static var home: URL {
        URL(fileURLWithPath: NSHomeDirectory(), isDirectory: true)
    }
}
