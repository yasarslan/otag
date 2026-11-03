import OtagKit
import SwiftUI

/// One session, on the detail side of the window.
///
/// The terminal itself arrives next; this is the frame around it — what the
/// session is, what it is doing, and what otag knows about it — so that the
/// window is useful as soon as the daemon has anything to say.
struct SessionView: View {
    let pane: PaneInfo

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            placeholder
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .navigationTitle(pane.title.isEmpty ? "Session \(pane.id)" : pane.title)
        .navigationSubtitle(pane.workspace)
    }

    private var header: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(pane.title.isEmpty ? "shell" : pane.title)
                    .font(.headline)
                HStack(spacing: 6) {
                    if let agent = pane.agent {
                        Text(agent.label)
                    }
                    Text(pane.status.label)
                    Text("\(pane.size.cols)×\(pane.size.rows)")
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
            if let exit = pane.exit {
                Label(ended(exit), systemImage: "stop.circle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    private var placeholder: some View {
        VStack(spacing: 8) {
            Image(systemName: "terminal")
                .font(.system(size: 30, weight: .light))
                .foregroundStyle(.tertiary)
            Text("The terminal view is not wired up yet")
                .foregroundStyle(.secondary)
            Text("Until it is, use this session in a terminal with `otag`.")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// How a program's end reads in one phrase.
    private func ended(_ exit: PaneExit) -> String {
        if let signal = exit.signal { return "killed by \(signal)" }
        if let code = exit.code { return code == 0 ? "finished" : "exited \(code)" }
        return "ended"
    }
}
