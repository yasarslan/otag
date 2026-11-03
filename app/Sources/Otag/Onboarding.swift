import AppKit
import OtagKit
import SwiftUI

/// The first launch: find the agents, offer the hooks, done.
///
/// Three steps at most, and the only one that changes anything outside otag
/// is the hooks — which is why it shows exactly what it would do and does
/// nothing until the button is pressed. otag keeps a copy of the file it
/// changes and leaves the user's own status line working.
struct OnboardingView: View {
    /// Where the daemon is, for the command-line tool hint.
    let socket: URL
    let dismiss: () -> Void

    @State private var step = 0
    @State private var installing = false
    @State private var installed: String?
    @State private var problem: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "square.split.2x1")
                    .font(.title2)
                    .foregroundStyle(.tint)
                Text("Welcome to otag")
                    .font(.title2.weight(.semibold))
                Spacer()
            }
            .padding(20)

            Divider()

            Group {
                switch step {
                case 0: agents
                case 1: hooks
                default: done
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)

            Divider()
            HStack {
                if step > 0 {
                    Button("Back") { step -= 1 }
                }
                Spacer()
                if step < 2 {
                    Button(step == 1 ? "Skip" : "Continue") { step += 1 }
                        .keyboardShortcut(step == 1 ? .cancelAction : .defaultAction)
                }
                if step == 1 {
                    Button(installing ? "Installing…" : "Install hooks") { install() }
                        .keyboardShortcut(.defaultAction)
                        .disabled(installing || installed != nil)
                }
                if step == 2 {
                    Button("Start") { dismiss() }
                        .keyboardShortcut(.defaultAction)
                }
            }
            .padding(16)
        }
        .frame(width: 540)
    }

    /// Step one: what otag can see.
    private var agents: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("otag runs your agents in panes it owns, and shows you what they are doing.")
            VStack(alignment: .leading, spacing: 8) {
                found("Claude Code", path: Self.which("claude"))
                found("Codex", path: Self.which("codex"))
            }
            Text("An agent otag cannot find is not a problem — you can still run anything in "
                + "a session, and otag notices an agent as soon as one starts.")
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
            .background(.quaternary, in: RoundedRectangle(cornerRadius: 8))

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

    /// Step three: how to get around.
    private var done: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("That is everything.")
                .font(.headline)
            keys("⌘N", "start a session")
            keys("⌘K", "every command by name")
            keys("⌘P", "jump to a session")
            keys("⌘D / ⇧⌘D", "split the tab")
            keys("⌥⌘I", "usage, the forecast and the night shift")
            keys("⇧⌘Y / ⇧⌘N", "answer what an agent is asking")
            Text("Agents keep running when you quit otag: the daemon owns them, and this "
                + "window is only looking in.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(.top, 4)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func keys(_ shortcut: String, _ what: String) -> some View {
        HStack(spacing: 8) {
            Text(shortcut)
                .font(.system(.caption, design: .monospaced))
                .frame(width: 80, alignment: .leading)
            Text(what)
                .font(.caption)
            Spacer(minLength: 0)
        }
    }

    private func found(_ name: String, path: String?) -> some View {
        HStack(spacing: 8) {
            Image(systemName: path == nil ? "circle.dashed" : "checkmark.circle.fill")
                .foregroundStyle(path == nil ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.tint))
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

    /// Run `otag install-hooks --yes`, which is the one place otag changes a
    /// file of the user's — and only because this button was pressed.
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
