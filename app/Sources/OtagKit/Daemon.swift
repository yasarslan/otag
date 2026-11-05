import Foundation

/// Starting the daemon, and connecting to it.
///
/// The daemon is a separate, long-lived process that owns the panes and the
/// agents; the app is one of its clients. Quitting the app therefore leaves
/// the agents running, which is the point.
public enum Daemon {
    /// How long a freshly started daemon is given to begin listening.
    public static let startupTimeout: Duration = .seconds(10)

    /// Whether a daemon is listening on `socket`.
    public static func isRunning(socket: URL) -> Bool {
        do {
            let connection = try DaemonConnection(socket: socket)
            connection.close()
            return true
        } catch {
            return false
        }
    }

    /// Connect to the daemon, starting one if none is listening.
    public static func connect(socket: URL, startIfNeeded: Bool = true) async throws -> DaemonConnection {
        if let connection = try? DaemonConnection(socket: socket) {
            return connection
        }

        try Task.checkCancellation()
        guard startIfNeeded else { throw DaemonError.disconnected }
        try start(socket: socket)

        let deadline = ContinuousClock.now + startupTimeout
        while ContinuousClock.now < deadline {
            try Task.checkCancellation()
            if let connection = try? DaemonConnection(socket: socket) {
                return connection
            }
            try await Task.sleep(for: .milliseconds(100))
        }
        throw DaemonError.didNotStart(path: socket.path)
    }

    /// Start `otag daemon` as a process that outlives the app.
    public static func start(socket: URL) throws {
        guard let binary = Paths.daemonBinary() else {
            throw DaemonError.noDaemonBinary
        }
        let process = Process()
        process.executableURL = binary
        process.arguments = ["daemon", "--socket", socket.path]
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        // The daemon outlives the app on purpose: nothing signals it when the
        // app quits, so the agents keep running. It writes its own log, which
        // is where a failure to start can be read.
        process.environment = ProcessInfo.processInfo.environment
        try process.run()
    }
}
