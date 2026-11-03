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

    private let socket: URL
    private var connection: DaemonConnection?
    private var listening: Task<Void, Never>?
    private var size: PaneSize?

    public init(pane: PaneID, socket: URL = Paths.socket()) {
        self.pane = pane
        self.socket = socket
    }

    /// Start following the pane, telling the daemon the size the view has.
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
    public func resize(to size: PaneSize) {
        guard size != self.size else { return }
        self.size = size
        try? connection?.send(.paneResize(pane: pane, size: size))
    }

    private func run() async {
        do {
            let connection = try DaemonConnection(socket: socket)
            self.connection = connection
            try connection.send(.attachPane(pane: pane, size: size))

            for await response in connection.responses {
                if Task.isCancelled { return }
                switch response {
                case let .paneScreen(pane, size, bytes) where pane == self.pane:
                    self.size = size
                    onScreen?(size, [UInt8](bytes))
                case let .output(pane, bytes) where pane == self.pane:
                    onOutput?([UInt8](bytes))
                case let .paneEnded(pane, exit) where pane == self.pane:
                    onEnded?(exit)
                    return
                case let .error(message):
                    onFailure?(message)
                    return
                default:
                    break
                }
            }
            // The daemon closed the connection without saying the pane
            // ended, which means the daemon itself went away.
            if !Task.isCancelled {
                onFailure?(DaemonError.disconnected.localizedDescription)
            }
        } catch {
            onFailure?(error.localizedDescription)
        }
    }
}
