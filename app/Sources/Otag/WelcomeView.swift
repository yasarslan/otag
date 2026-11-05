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
                Text(Strings.t("welcome.title"))
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
                    Button(Strings.t("welcome.back")) { step -= 1 }
                }
                Spacer()
                switch step {
                case 0:
                    Button(Strings.t("welcome.continue")) { step = 1 }
                        .keyboardShortcut(.defaultAction)
                case 1:
                    Button(Strings.t("welcome.skip")) { step = 2 }
                        .keyboardShortcut(.cancelAction)
                    Button(Strings.t(installing
                        ? "welcome.hooks.installing" : "welcome.hooks.install")) { install() }
                        .keyboardShortcut(.defaultAction)
                        .disabled(installing || installed != nil)
                default:
                    Button(Strings.t("welcome.notNow"), action: dismiss)
                    Button(Strings.t("welcome.first.new"), action: start)
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
            Text(Strings.t("welcome.what"))
                .fixedSize(horizontal: false, vertical: true)
            Text(Strings.t("welcome.daemon"))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            VStack(alignment: .leading, spacing: Chrome.gap) {
                found("Claude Code", path: Self.which("claude"), agent: .claude)
                found("Codex", path: Self.which("codex"), agent: .codex)
            }
            .padding(.top, 4)

            Text(Strings.t("welcome.notFound"))
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// Step two: the hooks, and exactly what they change.
    private var hooks: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(Strings.t("welcome.hooks.title"))
                .font(.headline)
            Text(Strings.t("welcome.hooks.detail"))
                .fixedSize(horizontal: false, vertical: true)

            VStack(alignment: .leading, spacing: 4) {
                Text(Strings.t("welcome.hooks.changes"))
                    .font(.subheadline.weight(.semibold))
                Text(Strings.t("welcome.hooks.changes.detail"))
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
            Text(Strings.t("welcome.first.title"))
                .font(.headline)
            Text(Strings.t("welcome.first.detail"))
                .fixedSize(horizontal: false, vertical: true)
            Text(Strings.t("welcome.first.more"))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Text(Strings.t("welcome.first.again"))
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
            .accessibilityLabel(Strings.t("welcome.step", at + 1, count))
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
                Text(path ?? Strings.t("welcome.notOnPath"))
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
                        installed = text.isEmpty
                            ? Strings.t("welcome.hooks.installed") : lastLine(text)
                        step = 2
                    } else {
                        problem = text.isEmpty
                            ? Strings.t("welcome.hooks.failed")
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
