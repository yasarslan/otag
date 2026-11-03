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

    @Test("a refresh with nothing connected is not left spinning")
    func refreshingWithoutADaemon() {
        let store = SessionStore(
            socket: URL(fileURLWithPath: NSTemporaryDirectory() + "/otag-nowhere.sock")
        )
        store.refreshUsage()
        #expect(!store.refreshingUsage, "there is nobody to answer, so nothing is pending")
    }

    /// Wait for something to become true, or fail saying what did not.
    private func until(_ what: String, _ ready: () -> Bool) async throws {
        for _ in 0 ..< 100 {
            if ready() { return }
            try await Task.sleep(for: .milliseconds(100))
        }
        Issue.record("never saw \(what)")
    }
}
