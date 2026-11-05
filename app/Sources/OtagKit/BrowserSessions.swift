import Foundation
import Observation

/// App-owned sessions. Their identifiers use the upper half of the pane ID
/// space so they can share a workbench without being sent to the daemon.
public struct BrowserSession: Codable, Equatable, Identifiable, Sendable {
    public let id: PaneID
    public var url: URL
    public let project: String
}

@MainActor
@Observable
public final class BrowserSessions {
    public private(set) var sessions: [BrowserSession]
    private let defaults: UserDefaults
    public static let key = "browserSessions"
    public static let defaultURL = URL(string: "http://localhost:3000")!

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        sessions = defaults.data(forKey: Self.key)
            .flatMap { try? JSONDecoder().decode([BrowserSession].self, from: $0) } ?? []
    }

    public func session(_ id: PaneID) -> BrowserSession? {
        sessions.first { $0.id == id }
    }

    @discardableResult
    public func create(project: String = "") -> PaneID {
        var id: PaneID
        repeat {
            id = PaneID.random(in: ((1 << 63) + 1)...PaneID.max)
        } while session(id) != nil
        sessions.append(BrowserSession(id: id, url: Self.defaultURL, project: project))
        save()
        return id
    }

    public func navigate(_ id: PaneID, to url: URL) {
        guard let index = sessions.firstIndex(where: { $0.id == id }),
              sessions[index].url != url else { return }
        sessions[index].url = url
        save()
    }

    public func close(_ id: PaneID) {
        sessions.removeAll { $0.id == id }
        save()
    }

    private func save() {
        guard let data = try? JSONEncoder().encode(sessions) else { return }
        defaults.set(data, forKey: Self.key)
    }

    /// A port or a local address uses HTTP; other bare hosts use HTTPS.
    /// Only web URLs are accepted. No script, file or external app schemes.
    public static func address(_ text: String) -> URL? {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return nil }
        let address: String
        if let port = UInt16(text), port > 0 {
            address = "http://localhost:\(port)"
        } else if text.contains("://") {
            address = text
        } else if text == "localhost" || text.hasPrefix("localhost:") || text.hasPrefix("localhost/")
            || text.hasPrefix("127.0.0.1") || text.hasPrefix("[::1]") {
            address = "http://\(text)"
        } else {
            address = "https://\(text)"
        }
        guard let url = URL(string: address), let host = url.host, !host.isEmpty,
              let scheme = url.scheme?.lowercased(), ["http", "https"].contains(scheme)
        else { return nil }
        return url
    }
}
