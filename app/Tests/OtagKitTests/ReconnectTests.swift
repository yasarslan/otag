import Foundation
import Testing

@testable import OtagKit

/// Following a pane across a daemon that goes away.
///
/// A daemon restart used to leave every terminal in the app dead, with one
/// line of otag's saying the connection closed. These check the two things
/// that have to be true instead: a pane that is still there is picked up
/// again with its screen, and a pane that is not is reported as gone rather
/// than waited for forever.
@Suite("Reconnecting to the daemon", .enabled(if: Daemonless.binary != nil))
@MainActor
struct ReconnectTests {
    @Test("a missing custom socket never starts the installed daemon")
    func missingFixtureDoesNotAutostart() async throws {
        let daemon = try Daemonless()
        defer { daemon.stop() }
        let connection = try await connect(to: daemon.socket)
        connection.close()
        let store = SessionStore(socket: daemon.socket)
        store.start()
        defer { store.disconnect() }
        daemon.stop()
        try await Task.sleep(for: .seconds(3))
        #expect(!FileManager.default.fileExists(atPath: daemon.socket.path),
                "reconnection must not create a replacement daemon for a test fixture")
    }
    @Test("a pane is picked up again, with its screen, after the daemon restarts")
    func reattachesAfterARestart() async throws {
        let daemon = try Daemonless()
        defer { daemon.stop() }

        let connection = try await connect(to: daemon.socket)
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

        let stream = PaneStream(pane: pane, socket: daemon.socket)
        let screens = Collected()
        let reattached = Counter()
        var goneSaid: String?
        stream.onScreen = { size, bytes in screens.screen(size: size, bytes: bytes) }
        stream.onOutput = { bytes in screens.output(bytes) }
        stream.onReattached = { reattached.add() }
        stream.onGone = { message in goneSaid = message }
        stream.start(size: PaneSize(rows: 24, cols: 80))
        defer { stream.stop() }

        try await screens.wait(for: "the pane's screen") { $0.sawScreen }
        stream.send(Array("printf 'before the restart\\n'\r".utf8))
        try await screens.wait(for: "the program's output") {
            $0.text.contains("before the restart")
        }

        // The daemon goes away and comes back, which is what an upgrade or a
        // `kill-server` looks like from here.
        connection.close()
        try daemon.restart()

        try await screens.wait(for: "the pane being picked up again") { _ in
            reattached.count > 0
        }
        #expect(goneSaid == nil, "the pane is still there, so nothing is gone")

        // A live stream, not just a connection: what is typed reaches the
        // program the daemon restored, and its output comes back.
        screens.forget()
        for _ in 0 ..< 50 {
            stream.send(Array("printf 'after the restart\\n'\r".utf8))
            if screens.text.contains("after the restart") { break }
            try await Task.sleep(for: .milliseconds(100))
        }
        #expect(
            screens.text.contains("after the restart"),
            "the pane takes what is typed again. What arrived was:\n\(screens.text)"
        )
    }

    @Test("a pane the daemon does not have is reported gone, not waited for")
    func reportsAPaneThatIsGone() async throws {
        let daemon = try Daemonless()
        defer { daemon.stop() }
        // Wait for the daemon, so the refusal comes from a daemon that is up
        // rather than from a socket nobody is listening on.
        let connection = try await connect(to: daemon.socket)
        defer { connection.close() }

        let stream = PaneStream(pane: 9_999, socket: daemon.socket)
        let gone = Message()
        stream.onGone = { message in gone.said(message) }
        stream.start(size: PaneSize(rows: 24, cols: 80))
        defer { stream.stop() }

        try await gone.wait()
        #expect(gone.text?.isEmpty == false, "the daemon's own reason is passed on")
    }

    @Test("waiting for the daemon backs off, and stops growing")
    func backoffGrows() {
        let stream = PaneStream(pane: 1, socket: URL(fileURLWithPath: "/nowhere/otag.sock"))

        #expect(stream.delay(1) == .milliseconds(400))
        #expect(stream.delay(2) == .milliseconds(800))
        #expect(stream.delay(3) == .milliseconds(1_600))
        #expect(stream.delay(9) == .seconds(5), "a daemon that never comes back is not hammered")
        #expect(stream.delay(0) == .milliseconds(400), "and nothing waits for no time at all")
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

/// How many times something happened, across the main actor.
@MainActor
final class Counter {
    private(set) var count = 0
    func add() { count += 1 }
}

/// One line the stream reported, once it has.
@MainActor
final class Message {
    private(set) var text: String?

    func said(_ message: String) { text = message }

    func wait() async throws {
        for _ in 0 ..< 100 {
            if text != nil { return }
            try await Task.sleep(for: .milliseconds(100))
        }
        Issue.record("nothing was ever reported")
    }
}
