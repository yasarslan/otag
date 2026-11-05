import OtagKit
import SwiftUI

/// The sides and the bottom of the main area, as places to drop a session.
///
/// Only drawn while something is being dragged over them: an app that always
/// shows its drop targets is an app shouting about its own implementation.
/// They are what makes side-by-side sessions findable without knowing that
/// `⋯ > Split with…` exists.
///
/// Three bands, not four. "Above" is the one nobody wants, and the top of the
/// area is where the header bar already is.
struct DropEdges: View {
    let workbench: Workbench
    let store: SessionStore
    let browsers: BrowserSessions

    /// How much of the area each band claims.
    private static let band: Double = 0.22

    @State private var targeted: DropEdge?

    var body: some View {
        GeometryReader { geometry in
            ZStack {
                ForEach(DropEdge.allCases, id: \.self) { edge in
                    let size = rect(edge, in: geometry.size)
                    Rectangle()
                        .fill(targeted == edge ? AnyShapeStyle(.tint.opacity(0.22))
                            : AnyShapeStyle(.clear))
                        .overlay {
                            if targeted == edge {
                                RoundedRectangle(cornerRadius: Chrome.controlRadius)
                                    .strokeBorder(.tint, lineWidth: 2)
                            }
                        }
                        .frame(width: size.width, height: size.height)
                        .position(x: size.midX, y: size.midY)
                        .dropDestination(for: DraggedSession.self) { items, _ in
                            drop(items, on: edge)
                        } isTargeted: { over in
                            targeted = over ? edge : (targeted == edge ? nil : targeted)
                        }
                }
            }
            .animation(.easeOut(duration: 0.15), value: targeted)
        }
    }

    /// Where one band sits in an area of this size.
    private func rect(_ edge: DropEdge, in size: CGSize) -> CGRect {
        let across = size.width * Self.band
        let down = size.height * Self.band
        switch edge {
        case .leading:
            return CGRect(x: 0, y: 0, width: across, height: size.height)
        case .trailing:
            return CGRect(x: size.width - across, y: 0, width: across, height: size.height)
        case .bottom:
            return CGRect(
                x: across,
                y: size.height - down,
                width: size.width - 2 * across,
                height: down
            )
        }
    }

    private func drop(_ items: [DraggedSession], on edge: DropEdge) -> Bool {
        targeted = nil
        // Only a session the daemon is actually running: a stale identifier
        // would put an empty terminal on screen.
        guard let session = items.first, (store.pane(session.pane) != nil || browsers.session(session.pane) != nil) else { return false }
        workbench.place(pane: session.pane, on: edge)
        return true
    }
}
