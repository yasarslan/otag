import Foundation
import Testing

@testable import OtagKit

/// Asking the daemon for the numbers again.
///
/// The button that does this has to show that it did something, which means
/// the store has to say when a re-read is in flight and when one landed.
@Suite("Re-reading the usage numbers", .enabled(if: Daemonless.binary != nil))
@MainActor
struct RefreshTests {
    @Test("a refresh is in flight until the daemon answers")
    func refreshingAndLanding() async throws {
        let daemon = try Daemonless()
        defer { daemon.stop() }

        let store = SessionStore(socket: daemon.socket)
        store.start()
        defer { store.disconnect() }

        try await until("the store connects") { store.state == .connected }
        #expect(!store.refreshingUsage, "nothing has been asked for yet")

        store.refreshUsage()
        #expect(store.refreshingUsage, "the button has something to show at once")

        try await until("the daemon's answer") { !store.refreshingUsage }
        #expect(store.usageUpdatedAt != nil, "and the moment it landed is remembered")
    }

    @Test("a report that says the same as the last one still ends the refresh")
    func anUnchangedReportStillLands() async throws {
        let daemon = try Daemonless()
        defer { daemon.stop() }

        let store = SessionStore(socket: daemon.socket)
        store.start()
        defer { store.disconnect() }
        try await until("the store connects") { store.state == .connected }

        // Twice over, with nothing happening in between: the second answer
        // carries exactly what the first did. The icon has to stop turning
        // on the answer arriving, not on the numbers being different.
        for round in 1 ... 2 {
            store.refreshUsage()
            #expect(store.refreshingUsage, "round \(round) is in flight")
            try await until("the answer to round \(round)") { !store.refreshingUsage }
        }
        #expect(store.usageUpdatedAt != nil)
    }

    @Test("a daemon that never answers does not leave the refresh in flight")
    func aSilentDaemonTimesOut() async throws {
        // A socket that accepts and then says nothing, which is the one case
        // the timeout exists for. A real daemon always answers.
        let silent = try SilentSocket()
        defer { silent.stop() }

        let store = SessionStore(socket: silent.url)
        store.usageTimeout = .milliseconds(300)
        store.start()
        defer { store.disconnect() }
        try await until("the store connects") { store.state == .connected }

        store.refreshUsage()
        #expect(store.refreshingUsage, "the request went out")

        try await until("the app to stop waiting") { !store.refreshingUsage }
        #expect(store.usageUpdatedAt == nil, "nothing arrived, so nothing was updated")
    }

    @Test("a refresh with nothing connected is not left spinning")
    func refreshingWithoutADaemon() {
        let store = SessionStore(
            socket: URL(fileURLWithPath: NSTemporaryDirectory() + "/otag-nowhere.sock")
        )
        store.refreshUsage()
        #expect(!store.refreshingUsage, "there is nobody to answer, so nothing is pending")
    }

    /// Wait for something to become true, or fail saying what did not.
    @discardableResult
    private func until(_ what: String, _ ready: () -> Bool) async throws {
        for _ in 0 ..< 100 {
            if ready() { return }
            try await Task.sleep(for: .milliseconds(100))
        }
        Issue.record("never saw \(what)")
    }
}

/// A unix socket that accepts connections and never says anything.
///
/// Stands in for a daemon that has stopped answering: the app is connected,
/// its request goes out, and nothing comes back.
final class SilentSocket {
    let url: URL
    private let listener: Int32
    private let accepting: DispatchQueue

    init() throws {
        let suffix = String(UUID().uuidString.prefix(8))
        url = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("otag-silent-\(suffix).sock")

        listener = socket(AF_UNIX, SOCK_STREAM, 0)
        guard listener >= 0 else {
            throw DaemonError.cannotConnect(path: url.path, reason: "no socket")
        }

        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(url.path.utf8)
        withUnsafeMutableBytes(of: &address.sun_path) { buffer in
            buffer.copyBytes(from: bytes)
        }
        let size = socklen_t(MemoryLayout<sockaddr_un>.size)
        let descriptor = listener
        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { address in
                bind(descriptor, address, size)
            }
        }
        guard bound == 0, listen(descriptor, 4) == 0 else {
            Darwin.close(descriptor)
            throw DaemonError.cannotConnect(path: url.path, reason: "cannot listen")
        }

        // Accept whatever connects and hold it open, saying nothing.
        accepting = DispatchQueue(label: "otag.silent")
        accepting.async {
            var held: [Int32] = []
            while true {
                let accepted = accept(descriptor, nil, nil)
                if accepted < 0 { break }
                held.append(accepted)
            }
            for connection in held { Darwin.close(connection) }
        }
    }

    func stop() {
        Darwin.close(listener)
        try? FileManager.default.removeItem(at: url)
    }
}
