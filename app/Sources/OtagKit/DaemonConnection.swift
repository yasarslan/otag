import Foundation

/// Everything that can go wrong talking to the daemon.
public enum DaemonError: LocalizedError {
    /// No daemon is listening on the socket.
    case notRunning(path: String)
    /// The connection could not be opened, with the system's reason.
    case cannotConnect(path: String, reason: String)
    /// The connection was open and then ended.
    case disconnected
    /// No `otag` binary could be found to start a daemon with.
    case noDaemonBinary
    /// The daemon was started but never began listening.
    case didNotStart(path: String)
    /// The daemon answered something unexpected.
    case unexpected(String)

    public var errorDescription: String? {
        switch self {
        case let .notRunning(path):
            return "No otag daemon is listening on \(path)."
        case let .cannotConnect(path, reason):
            return "Cannot connect to the otag daemon on \(path): \(reason)."
        case .disconnected:
            return "The otag daemon closed the connection."
        case .noDaemonBinary:
            return "Otag cannot find the otag command line tool to start the daemon with."
        case let .didNotStart(path):
            let logs = Paths.stateDirectory().appendingPathComponent("logs").path
            return "The otag daemon did not start listening on \(path). Its log is in \(logs)."
        case let .unexpected(what):
            return "The otag daemon answered something unexpected: \(what)."
        }
    }
}

/// One connection to the daemon: requests out, responses in.
///
/// Reading happens on a thread of its own and arrives as an `AsyncStream`,
/// because the daemon pushes messages the app never asked for — a pane's
/// output, an event — and neither direction may wait for the other.
public final class DaemonConnection: @unchecked Sendable {
    /// Where this connection goes, for error messages.
    public let socketPath: String

    private let descriptor: Int32
    private let writing = NSLock()
    private var closed = false

    /// Everything the daemon sends, in order.
    public let responses: AsyncStream<Response>
    private let continuation: AsyncStream<Response>.Continuation

    /// Connect to the daemon listening on `socket`.
    public init(socket: URL) throws {
        socketPath = socket.path

        let descriptor = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard descriptor >= 0 else {
            throw DaemonError.cannotConnect(
                path: socket.path,
                reason: String(cString: strerror(errno))
            )
        }

        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let pathBytes = Array(socket.path.utf8)
        let capacity = MemoryLayout.size(ofValue: address.sun_path)
        guard pathBytes.count < capacity else {
            Darwin.close(descriptor)
            throw DaemonError.cannotConnect(
                path: socket.path,
                reason: "the socket path is too long for this system"
            )
        }
        withUnsafeMutableBytes(of: &address.sun_path) { buffer in
            buffer.copyBytes(from: pathBytes)
        }

        let size = socklen_t(MemoryLayout<sockaddr_un>.size)
        let connected = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { address in
                Darwin.connect(descriptor, address, size)
            }
        }
        guard connected == 0 else {
            let reason = String(cString: strerror(errno))
            let code = errno
            Darwin.close(descriptor)
            if code == ENOENT || code == ECONNREFUSED {
                throw DaemonError.notRunning(path: socket.path)
            }
            throw DaemonError.cannotConnect(path: socket.path, reason: reason)
        }

        self.descriptor = descriptor
        var handed: AsyncStream<Response>.Continuation!
        responses = AsyncStream { handed = $0 }
        continuation = handed

        start()
    }

    deinit {
        close()
    }

    /// Send one request.
    public func send(_ request: Request) throws {
        let line = try Wire.encode(request)
        writing.lock()
        defer { writing.unlock() }
        guard !closed else { throw DaemonError.disconnected }

        try line.withUnsafeBytes { buffer in
            var sent = 0
            while sent < buffer.count {
                let written = Darwin.write(
                    descriptor,
                    buffer.baseAddress!.advanced(by: sent),
                    buffer.count - sent
                )
                if written > 0 {
                    sent += written
                    continue
                }
                if written < 0 && errno == EINTR {
                    continue
                }
                throw DaemonError.disconnected
            }
        }
    }

    /// Stop reading and close the socket.
    ///
    /// Safe to call more than once: the app closes a connection both when it
    /// is done with it and when the daemon has gone.
    public func close() {
        writing.lock()
        let wasOpen = !closed
        closed = true
        writing.unlock()

        guard wasOpen else { return }
        // `shutdown` wakes the reading thread, which then closes the
        // descriptor itself; closing it here would leave that thread reading
        // a descriptor that may have been handed to something else.
        Darwin.shutdown(descriptor, SHUT_RDWR)
    }

    /// Read lines on a thread of its own until the connection ends.
    private func start() {
        let thread = Thread { [descriptor, continuation] in
            var pending = Data()
            var buffer = [UInt8](repeating: 0, count: 64 * 1024)

            while true {
                let read = buffer.withUnsafeMutableBytes { raw in
                    Darwin.read(descriptor, raw.baseAddress, raw.count)
                }
                if read < 0 {
                    if errno == EINTR { continue }
                    break
                }
                if read == 0 { break }

                pending.append(contentsOf: buffer[0 ..< read])
                while let end = pending.firstIndex(of: 0x0A) {
                    let line = pending[pending.startIndex ..< end]
                    pending = pending[pending.index(after: end)...]
                    guard !line.isEmpty else { continue }
                    // A line this build cannot read is dropped: one bad
                    // message must not take the connection down.
                    if let response = try? Wire.decode(Data(line)) {
                        continuation.yield(response)
                    }
                }
            }

            Darwin.close(descriptor)
            continuation.finish()
        }
        thread.name = "otag.connection.read"
        thread.start()
    }
}
