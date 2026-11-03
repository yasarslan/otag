import Foundation

/// Where otag keeps its socket, and where the app finds the daemon binary.
///
/// The rules here mirror `otag-core`'s `paths.rs` on macOS, because both have
/// to agree on one socket: `OTAG_SOCKET` wins if it is set, and otherwise the
/// socket lives in otag's state directory.
public enum Paths {
    /// File name of the daemon's listening socket.
    public static let socketFileName = "otag.sock"

    /// Environment variable that overrides the socket path.
    public static let socketEnvironmentVariable = "OTAG_SOCKET"

    /// otag's state directory: `~/Library/Application Support/otag`.
    public static func stateDirectory(
        home: String = NSHomeDirectory()
    ) -> URL {
        URL(fileURLWithPath: home, isDirectory: true)
            .appendingPathComponent("Library", isDirectory: true)
            .appendingPathComponent("Application Support", isDirectory: true)
            .appendingPathComponent("otag", isDirectory: true)
    }

    /// The socket the daemon listens on.
    ///
    /// A relative `OTAG_SOCKET` is ignored rather than resolved against
    /// whatever directory the app happens to have been launched in, which is
    /// unknowable for a bundled app.
    public static func socket(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        home: String = NSHomeDirectory()
    ) -> URL {
        if let override = environment[socketEnvironmentVariable],
           override.hasPrefix("/") {
            return URL(fileURLWithPath: override)
        }
        return stateDirectory(home: home).appendingPathComponent(socketFileName)
    }

    /// The `otag` binary the app should run.
    ///
    /// The app bundle carries its own daemon in `Contents/Resources`, which
    /// is the only copy whose version is known to match. A development build
    /// has no bundle, so the usual places follow, and `PATH` last.
    public static func daemonBinary(
        bundle: Bundle = .main,
        fileManager: FileManager = .default,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> URL? {
        var candidates: [URL] = []
        if let bundled = bundle.url(forResource: "otag", withExtension: nil) {
            candidates.append(bundled)
        }
        // The binary this package is built next to, for `swift run` during
        // development.
        let alongside = bundle.bundleURL
            .deletingLastPathComponent()
            .appendingPathComponent("otag")
        candidates.append(alongside)

        if let home = environment["HOME"] {
            candidates.append(
                URL(fileURLWithPath: home)
                    .appendingPathComponent(".cargo/bin/otag")
            )
        }
        candidates.append(URL(fileURLWithPath: "/usr/local/bin/otag"))
        candidates.append(URL(fileURLWithPath: "/opt/homebrew/bin/otag"))

        for path in (environment["PATH"] ?? "").split(separator: ":") where !path.isEmpty {
            candidates.append(
                URL(fileURLWithPath: String(path), isDirectory: true)
                    .appendingPathComponent("otag")
            )
        }

        return candidates.first { fileManager.isExecutableFile(atPath: $0.path) }
    }
}
