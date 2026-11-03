import Foundation
import Testing

@testable import OtagKit

/// The app and the daemon have to agree on one socket, or the app starts a
/// second daemon and nothing the user does lands where they expect.
@Suite("Where the socket is")
struct PathsTests {
    @Test("the socket lives in otag's state directory, as the daemon's rules say")
    func defaultSocket() {
        let socket = Paths.socket(environment: [:], home: "/Users/example")
        #expect(
            socket.path == "/Users/example/Library/Application Support/otag/otag.sock"
        )
    }

    @Test("OTAG_SOCKET wins")
    func overrideWins() {
        let socket = Paths.socket(
            environment: ["OTAG_SOCKET": "/tmp/somewhere/otag.sock"],
            home: "/Users/example"
        )
        #expect(socket.path == "/tmp/somewhere/otag.sock")
    }

    @Test("a relative OTAG_SOCKET is ignored rather than guessed at")
    func relativeOverrideIsIgnored() {
        // A bundled app has no meaningful working directory to resolve it
        // against, and the daemon ignores relative values too.
        let socket = Paths.socket(
            environment: ["OTAG_SOCKET": "otag.sock"],
            home: "/Users/example"
        )
        #expect(
            socket.path == "/Users/example/Library/Application Support/otag/otag.sock"
        )
    }
}
