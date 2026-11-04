import AppKit
import OtagKit
import SwiftUI

/// The first launch, in three steps: what otag does, the hooks, and a
/// session.
///
/// The only step that changes anything outside otag is the hooks — which is
/// why it shows exactly what it would do and does nothing until the button is
/// pressed. otag keeps a copy of the file it changes and leaves the user's
/// own status line working.
///
/// The old last step was eight keyboard shortcuts, which is a list nobody
/// reads. The last step is now the thing they came here to do.
struct WelcomeView: View {
    /// Where the daemon is, for finding the agents.
    let socket: URL
    /// Done, and start a session.
    let start: () -> Void
    /// Done, and do not.
    let dismiss: () -> Void

    @State private var step = 0
    @State private var installing = false
    @State private var installed: String?
    @State private var problem: String?

    /// How many steps there are.
    private static let steps = 3

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "square.split.2x1")
                    .font(.title2)
                    .foregroundStyle(.tint)
                Text("Welcome to otag")
                    .font(.title2.weight(.semibold))
                Spacer()
                Dots(of: Self.steps, at: step)
            }
            .padding(20)

            Divider()

            Group {
                switch step {
                case 0: what
                case 1: hooks
                default: first
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, minHeight: 220, alignment: .topLeading)

            Divider()
            HStack {
                if step > 0 {
                    Button("Back") { step -= 1 }
                }
                Spacer()
                switch step {
                case 0:
                    Button("Continue") { step = 1 }
                        .keyboardShortcut(.defaultAction)
                case 1:
                    Button("Skip") { step = 2 }
                        .keyboardShortcut(.cancelAction)
                    Button(installing ? "Installing…" : "Install hooks") { install() }
                        .keyboardShortcut(.defaultAction)
                        .disabled(installing || installed != nil)
                default:
                    Button("Not now", action: dismiss)
                    Button("New session…", action: start)
                        .keyboardShortcut(.defaultAction)
                }
            }
            .padding(Chrome.gutter)
        }
        .frame(width: 540)
    }

    /// Step one: what otag is.
    private var what: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("otag runs your agents in sessions it owns, and shows you what each one is "
                + "doing — side by side, with the limits in front of you.")
                .fixedSize(horizontal: false, vertical: true)
            Text("The agents keep running when you quit otag: a daemon owns them, and this "
                + "window is looking in.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            VStack(alignment: .leading, spacing: Chrome.gap) {
                found("Claude Code", path: Self.which("claude"), agent: .claude)
                found("Codex", path: Self.which("codex"), agent: .codex)
            }
            .padding(.top, 4)

            Text("An agent otag cannot find is not a problem — you can run anything in a "
                + "session, and otag notices an agent as soon as one starts.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// Step two: the hooks, and exactly what they change.
    private var hooks: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Claude Code can tell otag what it is doing.")
                .font(.headline)
            Text("With its hooks installed, otag knows when an agent is working, done or "
                + "waiting for permission — and how much of your limits is left — instead of "
                + "guessing from the screen.")
                .fixedSize(horizontal: false, vertical: true)

            VStack(alignment: .leading, spacing: 4) {
                Text("What this changes")
                    .font(.subheadline.weight(.semibold))
                Text("~/.claude/settings.json gains an `otag hook …` command on six events "
                    + "and a status line that runs otag's and then yours. A copy of the file "
                    + "is kept first, and `otag uninstall-hooks` puts it back.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(10)
            .background(.quaternary, in: RoundedRectangle(cornerRadius: Chrome.controlRadius))

            if let installed {
                Label(installed, systemImage: "checkmark.circle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            if let problem {
                Label(problem, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    /// Step three: the thing they came here to do.
    private var first: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Start your first session.")
                .font(.headline)
            Text("Choose a project folder and an agent, and otag runs it. Your sessions "
                + "appear in the list on the left; clicking one shows it.")
                .fixedSize(horizontal: false, vertical: true)
            Text("Drag a session from that list onto the side of the window to put two side "
                + "by side, and use the button above a session to hand its work to the other "
                + "agent.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Text("You can see this screen again from the Help menu.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(.top, 4)
        }
    }

    /// Which step of how many, as dots.
    private struct Dots: View {
        let count: Int
        let at: Int

        init(of count: Int, at: Int) {
            self.count = count
            self.at = at
        }

        var body: some View {
            HStack(spacing: 5) {
                ForEach(0 ..< count, id: \.self) { index in
                    Circle()
                        .fill(index == at ? AnyShapeStyle(.tint) : AnyShapeStyle(.quaternary))
                        .frame(width: 6, height: 6)
                }
            }
            .accessibilityLabel("Step \(at + 1) of \(count)")
        }
    }

    private func found(_ name: String, path: String?, agent: AgentKind) -> some View {
        HStack(spacing: Chrome.gap) {
            if path == nil {
                Image(systemName: "circle.dashed")
                    .foregroundStyle(.tertiary)
                    .frame(width: 14)
            } else {
                AgentMark(agent: agent, size: 10)
                    .frame(width: 14)
            }
            VStack(alignment: .leading, spacing: 1) {
                Text(name)
                Text(path ?? "not on your PATH")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.head)
            }
            Spacer(minLength: 0)
        }
    }

    /// Run `otag install-hooks --yes`, which is the one place this screen
    /// changes a file of the user's — and only because this button was
    /// pressed.
    private func install() {
        guard let binary = Paths.daemonBinary() else {
            problem = DaemonError.noDaemonBinary.localizedDescription
            return
        }
        installing = true
        problem = nil

        Task.detached {
            let process = Process()
            process.executableURL = binary
            process.arguments = ["install-hooks", "--yes"]
            let output = Pipe()
            process.standardOutput = output
            process.standardError = output

            do {
                try process.run()
                let data = output.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()
                let text = String(decoding: data, as: UTF8.self)
                    .trimmingCharacters(in: .whitespacesAndNewlines)

                await MainActor.run {
                    installing = false
                    if process.terminationStatus == 0 {
                        installed = text.isEmpty ? "Hooks installed." : lastLine(text)
                        step = 2
                    } else {
                        problem = text.isEmpty
                            ? "otag could not install the hooks."
                            : lastLine(text)
                    }
                }
            } catch {
                await MainActor.run {
                    installing = false
                    problem = error.localizedDescription
                }
            }
        }
    }

    /// The last thing the command said, which is its conclusion.
    private func lastLine(_ text: String) -> String {
        text.split(separator: "\n").last.map(String.init) ?? text
    }

    /// Where a program is, if it is on the PATH the app inherited.
    static func which(_ program: String) -> String? {
        let environment = ProcessInfo.processInfo.environment
        var directories = (environment["PATH"] ?? "").split(separator: ":").map(String.init)
        // A bundled app often inherits a bare PATH, so the usual places are
        // looked at too.
        directories.append(contentsOf: [
            "/usr/local/bin", "/opt/homebrew/bin",
            (environment["HOME"] ?? "") + "/.local/bin",
            (environment["HOME"] ?? "") + "/.bun/bin",
            (environment["HOME"] ?? "") + "/.volta/bin",
        ])

        for directory in directories where !directory.isEmpty {
            let candidate = URL(fileURLWithPath: directory, isDirectory: true)
                .appendingPathComponent(program)
            if FileManager.default.isExecutableFile(atPath: candidate.path) {
                return candidate.path
            }
        }
        return nil
    }
}
