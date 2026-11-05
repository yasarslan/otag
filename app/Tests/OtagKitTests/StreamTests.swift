import Foundation
import Testing

@testable import OtagKit

/// The app against a real daemon.
///
/// Everything else in these tests is about reading the daemon's words; this
/// is about the one thing that cannot be checked by reading — that a pane's
/// bytes actually arrive, and that what is typed reaches the program. It runs
/// a daemon of its own, in a home directory of its own, so it touches
/// nothing the user has.
///
/// Skipped rather than failed when the daemon binary has not been built:
/// `swift test` on its own cannot build Rust.
@Suite("A real daemon", .enabled(if: Daemonless.binary != nil))
@MainActor
struct StreamTests {
    @Test("a session the app starts sends its screen and takes what is typed")
    func startingAndWatchingASession() async throws {
        let daemon = try Daemonless()
        defer { daemon.stop() }

        // Starting a session: what the new-session sheet sends.
        let connection: DaemonConnection
        do {
            connection = try await connect(to: daemon.socket)
        } catch {
            Issue.record("the daemon did not start: \(error). It printed:\n\(daemon.output)")
            return
        }
        defer { connection.close() }
        try connection.send(.startSession(StartSession(cwd: daemon.project.path)))

        var started: PaneID?
        for await response in connection.responses {
            if case let .sessionStarted(pane, _, _) = response {
                started = pane
                break
            }
            if case let .error(message) = response {
                Issue.record("the daemon refused to start a session: \(message)")
                return
            }
        }
        let pane = try #require(started, "the daemon started a session")

        // Watching it: what a terminal view does.
        let stream = PaneStream(pane: pane, socket: daemon.socket)
        let screens = Collected()
        stream.onScreen = { size, bytes in screens.screen(size: size, bytes: bytes) }
        stream.onOutput = { bytes in screens.output(bytes) }
        stream.start(size: PaneSize(rows: 24, cols: 80))
        defer { stream.stop() }

        try await screens.wait(for: "the pane's screen") { $0.sawScreen }
        #expect(screens.size == PaneSize(rows: 24, cols: 80),
                "the daemon takes the size the view reported")

        // Typing: a command the shell will echo back.
        stream.send(Array("printf 'from the app\\n'\r".utf8))
        try await screens.wait(for: "the program's output") {
            $0.text.contains("from the app")
        }
    }

    @Test("a night shift plan reaches the daemon and comes back on the pane")
    func nightShiftRoundTrip() async throws {
        let daemon = try Daemonless()
        defer { daemon.stop() }

        let connection: DaemonConnection
        do {
            connection = try await connect(to: daemon.socket)
        } catch {
            Issue.record("the daemon did not start: \(error). It printed:\n\(daemon.output)")
            return
        }
        defer { connection.close() }

        try connection.send(.startSession(StartSession(cwd: daemon.project.path)))
        var started: PaneID?
        for await response in connection.responses {
            if case let .sessionStarted(pane, _, _) = response {
                started = pane
                break
            }
        }
        let pane = try #require(started)

        try connection.send(.nightShift(
            pane: pane,
            plan: NightShiftPlan(pauseAt: 80, resumePrompt: "carry on", queue: ["one", "two"])
        ))
        // The queue is not sent anywhere: this pane runs a shell, not an
        // agent, and otag only works for sessions it knows an agent in.
        try connection.send(.listPanes)

        for await response in connection.responses {
            guard case let .panes(panes) = response else { continue }
            let shift = try #require(
                panes.first { $0.id == pane }?.nightShift,
                "the pane carries what the night shift is doing"
            )
            #expect(shift.pauseAt == 80)
            #expect(shift.queued == 2)
            #expect(shift.sent == 0)
            #expect(shift.resumePrompt == "carry on")
            return
        }
    }

    @Test("the morning report says what otag did, and nothing it did not")
    func morningReport() async throws {
        let daemon = try Daemonless()
        defer { daemon.stop() }

        let connection: DaemonConnection
        do {
            connection = try await connect(to: daemon.socket)
        } catch {
            Issue.record("the daemon did not start: \(error). It printed:\n\(daemon.output)")
            return
        }
        defer { connection.close() }

        try connection.send(.morningReport)
        for await response in connection.responses {
            guard case let .morningReport(sessions) = response else { continue }
            #expect(sessions.isEmpty, "nothing has been queued, so there is nothing to report")
            return
        }
    }

    /// Connect, waiting for a daemon that is still starting.
    private func connect(to socket: URL) async throws -> DaemonConnection {
        for _ in 0 ..< 100 {
            if let connection = try? DaemonConnection(socket: socket) {
                return connection
            }
            try await Task.sleep(for: .milliseconds(100))
        }
        throw DaemonError.didNotStart(path: socket.path)
    }
}

/// Everything a terminal view would have been handed, kept for assertions.
@MainActor
final class Collected {
    private(set) var sawScreen = false
    private(set) var size: PaneSize?
    private(set) var text = ""

    func screen(size: PaneSize, bytes: [UInt8]) {
        sawScreen = true
        self.size = size
        text += String(decoding: bytes, as: UTF8.self)
    }

    func output(_ bytes: [UInt8]) {
        text += String(decoding: bytes, as: UTF8.self)
    }

    /// Forget what has arrived so far, to watch for what comes next.
    func forget() {
        text = ""
    }

    /// Wait until `ready` holds, or fail saying what did arrive.
    func wait(for what: String, until ready: (Collected) -> Bool) async throws {
        for _ in 0 ..< 100 {
            if ready(self) { return }
            try await Task.sleep(for: .milliseconds(100))
        }
        Issue.record("never saw \(what). What arrived was:\n\(text)")
    }
}

/// A daemon of its own, in a home directory of its own.
final class Daemonless {
    /// The `otag` binary a development build leaves in `target/debug`.
    static let binary: URL? = {
        // .../app/Tests/OtagKitTests/StreamTests.swift → the repository root.
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        for configuration in ["debug", "release"] {
            let candidate = root
                .appendingPathComponent("target/\(configuration)/otag")
            if FileManager.default.isExecutableFile(atPath: candidate.path) {
                return candidate
            }
        }
        return nil
    }()

    let home: URL
    let project: URL
    let socket: URL
    private var process: Process

    init() throws {
        let binary = try #require(Self.binary)
        // Short on purpose: a unix socket's whole path has to fit in about
        // a hundred characters, and the temporary directory uses most of it.
        let suffix = String(UUID().uuidString.prefix(8))
        home = URL(fileURLWithPath: ProcessInfo.processInfo.environment["OTAG_TEST_RUN_DIR"] ?? NSTemporaryDirectory())
            .appendingPathComponent("otag-\(suffix)")
        project = home.appendingPathComponent("project")
        socket = home.appendingPathComponent("otag.sock")
        try FileManager.default.createDirectory(at: project, withIntermediateDirectories: true)

        _ = binary
        process = try Self.spawn(home: home, socket: socket)
    }

    /// One daemon process, with a home of its own so its state — and the
    /// sessions it would restore — is this test's and nobody else's.
    private static func spawn(home: URL, socket: URL) throws -> Process {
        let process = Process()
        process.executableURL = try #require(Self.binary)
        process.arguments = ["daemon", "--socket", socket.path,
                             "--state", home.appendingPathComponent("state").path,
                             "--exit-with-parent", String(ProcessInfo.processInfo.processIdentifier)]
        process.environment = [
            "HOME": home.path,
            "SHELL": "/bin/sh",
            "PATH": ProcessInfo.processInfo.environment["PATH"] ?? "/usr/bin:/bin",
            "OTAG_NO_NOTIFY": "1",
            "OTAG_STATE_DIR": home.appendingPathComponent("state").path,
            "XDG_STATE_HOME": home.appendingPathComponent("xdg-state").path,
            "XDG_RUNTIME_DIR": home.appendingPathComponent("run").path,
        ]
        // Kept, so a daemon that will not start can say why. Appended to, so
        // a restart does not throw away what the first one said.
        let log = home.appendingPathComponent("daemon.out")
        if !FileManager.default.fileExists(atPath: log.path) {
            FileManager.default.createFile(atPath: log.path, contents: nil)
        }
        let output = try FileHandle(forWritingTo: log)
        output.seekToEndOfFile()
        process.standardOutput = output
        process.standardError = output
        try process.run()
        output.closeFile()
        return process
    }

    /// Stop the daemon and start another one on the same socket, with the
    /// same state directory, which is what an upgrade looks like.
    func restart() throws {
        stopProcess()
        process = try Self.spawn(home: home, socket: socket)
    }

    /// Whatever the daemon printed, for a failure message.
    var output: String {
        (try? String(contentsOf: home.appendingPathComponent("daemon.out"), encoding: .utf8)) ?? ""
    }

    func stop() {
        stopProcess()
        try? FileManager.default.removeItem(at: home)
    }

    private func stopProcess() {
        guard process.isRunning else { return }
        process.terminate()
        let deadline = Date().addingTimeInterval(5)
        while process.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.02) }
        if process.isRunning {
            kill(process.processIdentifier, SIGKILL)
            Issue.record("test daemon ignored shutdown and required SIGKILL")
        }
        process.waitUntilExit()
        #expect(!process.isRunning, "test daemon must be gone before fixture cleanup")
    }

    deinit { stop() }
}
