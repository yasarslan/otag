import Foundation

/// One pane, watched as a stream of bytes.
///
/// A connection of its own per pane, because that is what the daemon's raw
/// attach is keyed by: one client, one pane, one stream. The daemon sends the
/// screen as escape sequences first and then the program's output unchanged,
/// so a terminal emulator that is handed everything in order ends up showing
/// exactly what the pane shows.
///
/// The app owns the size: whatever the view reports is sent on, and the
/// daemon's own layout then leaves this pane alone.
@MainActor
public final class PaneStream {
    /// The pane this follows.
    public let pane: PaneID

    /// Called with the escape sequences that put the whole screen in place.
    /// Everything the terminal had should be considered replaced.
    public var onScreen: ((PaneSize, [UInt8]) -> Void)?
    /// Called with each chunk of the program's own output.
    public var onOutput: (([UInt8]) -> Void)?
    /// Called when the pane's program has ended and nothing more is coming.
    public var onEnded: ((PaneExit?) -> Void)?
    /// Called when the stream could not be kept up, with a line to show.
    public var onFailure: ((String) -> Void)?
    /// Called when the daemon has no such pane, so there is nothing to wait
    /// for: the session it was following is gone for good.
    public var onGone: ((String) -> Void)?
    /// Called each time the pane has been picked up again after a break,
    /// just before its screen arrives.
    public var onReattached: (() -> Void)?

    private let socket: URL
    private var connection: DaemonConnection?
    private var listening: Task<Void, Never>?
    private var size: PaneSize?

    /// How long to wait before trying again, and the longest that gets.
    ///
    /// A daemon that has just been restarted is worth waiting for, but not
    /// worth hammering: the delay doubles up to a few seconds, which is
    /// faster than a person reaches for the window and slower than a loop.
    private static let firstRetry: Duration = .milliseconds(400)
    private static let longestRetry: Duration = .seconds(5)

    public init(pane: PaneID, socket: URL = Paths.socket()) {
        self.pane = pane
        self.socket = socket
    }

    /// Start following the pane, telling the daemon the size the view has.
    ///
    /// Following survives the daemon going away: the stream is picked up
    /// again when it comes back, and the pane's screen arrives with it.
    public func start(size: PaneSize?) {
        guard listening == nil else { return }
        self.size = size
        listening = Task { [weak self] in
            await self?.run()
        }
    }

    /// Stop following it. The pane and its program keep running.
    public func stop() {
        listening?.cancel()
        listening = nil
        if let connection {
            // Tell the daemon as well, so it stops sending before the socket
            // closes under it.
            try? connection.send(.detachPane(pane: pane))
            connection.close()
        }
        connection = nil
    }

    /// Send what the user typed.
    public func send(_ bytes: [UInt8]) {
        guard !bytes.isEmpty else { return }
        try? connection?.send(.paneInput(pane: pane, bytes: Data(bytes)))
    }

    /// Tell the daemon how large the view is drawing the pane.
    ///
    /// The size is remembered whether or not there is a connection to send
    /// it on, because it is also what the next attach asks for.
    public func resize(to size: PaneSize) {
        guard size != self.size else { return }
        self.size = size
        try? connection?.send(.paneResize(pane: pane, size: size))
    }

    /// Why one attempt at following the pane came to an end.
    private enum Outcome {
        /// The pane's program ended. Nothing more will ever come.
        case ended
        /// The daemon answered, and has no such pane.
        case gone(String)
        /// The connection went away without a word, which is what a daemon
        /// being restarted looks like.
        case dropped(String)
    }

    /// Follow the pane, picking it up again whenever the daemon comes back.
    private func run() async {
        var attempt = 0
        var said = false

        while !Task.isCancelled {
            switch await follow(reattaching: attempt > 0) {
            case .ended:
                return
            case let .gone(message):
                onGone?(message)
                return
            case let .dropped(message):
                if Task.isCancelled { return }
                if !said {
                    // Once, not once per attempt: a line per retry would
                    // fill the pane with otag talking about itself.
                    onFailure?("\(message) Reconnecting…")
                    said = true
                }
                attempt += 1
                try? await Task.sleep(for: delay(attempt))
            }
        }
    }

    /// How long to wait before the next attempt.
    ///
    /// Not private so that the growth can be checked without waiting for it.
    func delay(_ attempt: Int) -> Duration {
        var waiting = Self.firstRetry
        for _ in 1 ..< max(attempt, 1) {
            waiting += waiting
            if waiting >= Self.longestRetry { return Self.longestRetry }
        }
        return waiting
    }

    /// One attempt: connect, attach, and pass on everything that arrives.
    private func follow(reattaching: Bool) async -> Outcome {
        let connection: DaemonConnection
        do {
            connection = try DaemonConnection(socket: socket)
            self.connection = connection
            // The size the view has settled on, so the program is told the
            // truth about where it is drawing even across a restart.
            try connection.send(.attachPane(pane: pane, size: size))
        } catch {
            self.connection = nil
            return .dropped(error.localizedDescription)
        }

        if reattaching {
            onReattached?()
        }

        for await response in connection.responses {
            if Task.isCancelled { return .ended }
            switch response {
            case let .paneScreen(pane, size, bytes) where pane == self.pane:
                self.size = size
                onScreen?(size, [UInt8](bytes))
            case let .output(pane, bytes) where pane == self.pane:
                onOutput?([UInt8](bytes))
            case let .paneEnded(pane, exit) where pane == self.pane:
                onEnded?(exit)
                return .ended
            case let .error(message):
                // The daemon is there and will not give us this pane, which
                // is not something waiting fixes.
                return .gone(message)
            default:
                break
            }
        }

        if self.connection === connection { self.connection = nil }
        if Task.isCancelled { return .ended }
        // The daemon closed the connection without saying the pane ended,
        // which means the daemon itself went away.
        return .dropped(DaemonError.disconnected.localizedDescription)
    }
}
