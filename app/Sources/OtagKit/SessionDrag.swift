import CoreTransferable
import Foundation

/// A session being dragged out of the sidebar.
///
/// Carried as text with a marker in front of it, so that dropping anything
/// else — a file, a line from a browser — on the terminal area does nothing
/// rather than being read as a session identifier.
public struct DraggedSession: Transferable, Codable, Sendable {
    public let pane: PaneID

    public init(pane: PaneID) {
        self.pane = pane
    }

    /// What the drag carries, and what a drop has to recognise.
    public static let marker = "otag-session:"

    public static var transferRepresentation: some TransferRepresentation {
        ProxyRepresentation(
            exporting: { session in "\(marker)\(session.pane)" },
            importing: { text in
                guard let pane = Self.pane(in: text) else {
                    throw CocoaError(.fileReadCorruptFile)
                }
                return DraggedSession(pane: pane)
            }
        )
    }

    /// The session a dragged string names, or nothing when it names none.
    public static func pane(in text: String) -> PaneID? {
        guard text.hasPrefix(marker) else { return nil }
        return PaneID(text.dropFirst(marker.count))
    }
}
