import OtagKit
import SwiftUI

/// One button in the toolbar.
///
/// All five share the same style and shape, because a
/// toolbar whose buttons differ is a toolbar you read every time instead of
/// learning once. Each one carries its full sentence twice: as a tooltip for
/// the mouse and as an accessibility label for everything else.
struct ToolbarIcon: View {
    let symbol: String
    let title: String
    /// The whole sentence, not the symbol's name.
    let label: String
    var enabled = true
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) {
                Image(systemName: symbol)
                    .frame(width: 18, height: 18)
                Text(title)
                    .foregroundStyle(.primary)
            }
            .fixedSize()
        }
        .chromeButtonStyle()
        .controlSize(.regular)
        .disabled(!enabled)
        .help(label)
        .accessibilityLabel(label)
    }
}

/// A toolbar button that opens something, rather than acting at once.
///
/// The same look as `ToolbarIcon`; the difference is only that it owns a
/// popover, and that the popover is anchored to it rather than to the window.
struct ToolbarPopoverIcon<Content: View>: View {
    let symbol: String
    let title: String
    let label: String
    var tinted = false
    var enabled = true
    @Binding var shown: Bool
    @ViewBuilder let content: Content

    var body: some View {
        Button { shown = true } label: {
            HStack(spacing: 5) {
                Image(systemName: symbol)
                    .frame(width: 18, height: 18)
                    .foregroundStyle(tinted ? AnyShapeStyle(.tint) : AnyShapeStyle(.primary))
                Text(title)
                    .foregroundStyle(.primary)
            }
            .fixedSize()
        }
        .chromeButtonStyle()
        .controlSize(.regular)
        .disabled(!enabled)
        .help(label)
        .accessibilityLabel(label)
        .popover(isPresented: $shown, arrowEdge: .bottom) { content }
    }
}
