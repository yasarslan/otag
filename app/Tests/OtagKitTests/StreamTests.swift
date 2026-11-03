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
struct Daemonless {
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
    private let process: Process

    init() throws {
        let binary = try #require(Self.binary)
        // Short on purpose: a unix socket's whole path has to fit in about
        // a hundred characters, and the temporary directory uses most of it.
        let suffix = String(UUID().uuidString.prefix(8))
        home = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("otag-\(suffix)")
        project = home.appendingPathComponent("project")
        socket = home.appendingPathComponent("otag.sock")
        try FileManager.default.createDirectory(at: project, withIntermediateDirectories: true)

        process = Process()
        process.executableURL = binary
        process.arguments = ["daemon", "--socket", socket.path]
        // A home of its own, so the daemon's state — and the sessions it
        // would restore — is this test's and nobody else's.
        process.environment = [
            "HOME": home.path,
            "PATH": ProcessInfo.processInfo.environment["PATH"] ?? "/usr/bin:/bin",
            "OTAG_NO_NOTIFY": "1",
        ]
        // Kept, so a daemon that will not start can say why.
        let log = home.appendingPathComponent("daemon.out")
        FileManager.default.createFile(atPath: log.path, contents: nil)
        let output = try FileHandle(forWritingTo: log)
        process.standardOutput = output
        process.standardError = output
        try process.run()
    }

    /// Whatever the daemon printed, for a failure message.
    var output: String {
        (try? String(contentsOf: home.appendingPathComponent("daemon.out"), encoding: .utf8)) ?? ""
    }

    func stop() {
        process.terminate()
        try? FileManager.default.removeItem(at: home)
    }
}
