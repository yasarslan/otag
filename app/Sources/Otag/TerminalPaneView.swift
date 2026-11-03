import AppKit
import OtagKit
import SwiftTerm
import SwiftUI

/// One pane's terminal.
///
/// SwiftTerm does the emulation and the drawing; the daemon's bytes go in and
/// the user's keys come out. Nothing here interprets the stream: a screen
/// message replaces what the view has, output is appended, and the size the
/// view settles on is sent back so the program is told the truth.
struct TerminalPaneView: NSViewRepresentable {
    /// The pane to show.
    let pane: PaneID
    /// Where the daemon is.
    let socket: URL
    /// Whether this pane has the keyboard in its tab.
    let focused: Bool
    /// Called when the pane's program ends, so the tab can close it.
    let onEnded: (PaneExit?) -> Void
    /// Called when the user clicks into this pane.
    let onFocus: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(pane: pane, socket: socket, onEnded: onEnded, onFocus: onFocus)
    }

    func makeNSView(context: Context) -> TerminalView {
        let view = TerminalView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        view.terminalDelegate = context.coordinator
        view.font = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
        // The terminal takes its colours from the user's appearance, like
        // every other calm part of otag: nothing is painted that the system
        // has not chosen.
        view.nativeBackgroundColor = .textBackgroundColor
        view.nativeForegroundColor = .textColor
        view.allowMouseReporting = true

        context.coordinator.attach(to: view)
        return view
    }

    func updateNSView(_ view: TerminalView, context: Context) {
        context.coordinator.onEnded = onEnded
        context.coordinator.onFocus = onFocus
        if focused, view.window?.firstResponder !== view {
            view.window?.makeFirstResponder(view)
        }
    }

    static func dismantleNSView(_ view: TerminalView, coordinator: Coordinator) {
        coordinator.stop()
    }

    /// Carries bytes between the daemon and one terminal view.
    @MainActor
    final class Coordinator: NSObject, TerminalViewDelegate {
        private let stream: PaneStream
        private weak var view: TerminalView?
        var onEnded: (PaneExit?) -> Void
        var onFocus: () -> Void

        init(
            pane: PaneID,
            socket: URL,
            onEnded: @escaping (PaneExit?) -> Void,
            onFocus: @escaping () -> Void
        ) {
            stream = PaneStream(pane: pane, socket: socket)
            self.onEnded = onEnded
            self.onFocus = onFocus
            super.init()
        }

        /// Start following the pane and feeding this view.
        func attach(to view: TerminalView) {
            self.view = view

            stream.onScreen = { [weak view] size, bytes in
                guard let view else { return }
                // The bytes clear the screen themselves and then draw it, so
                // feeding them is enough; the size is the daemon agreeing
                // with what this view asked for.
                _ = size
                view.feed(byteArray: bytes[...])
            }
            stream.onOutput = { [weak view] bytes in
                view?.feed(byteArray: bytes[...])
            }
            stream.onEnded = { [weak self] exit in
                self?.onEnded(exit)
            }
            stream.onFailure = { [weak view] message in
                // Said on the pane itself, dimmed: it is otag speaking, not
                // the program.
                let text = "\r\n\u{1b}[2m-- \(message) --\u{1b}[0m\r\n"
                view?.feed(text: text)
            }

            let size = view.terminalDimensions
            stream.start(size: PaneSize(
                rows: UInt16(max(size.rows, 1)),
                cols: UInt16(max(size.cols, 1))
            ))
        }

        func stop() {
            stream.stop()
        }

        // MARK: TerminalViewDelegate

        func send(source: TerminalView, data: ArraySlice<UInt8>) {
            onFocus()
            stream.send(Array(data))
        }

        func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
            stream.resize(to: PaneSize(
                rows: UInt16(max(newRows, 1)),
                cols: UInt16(max(newCols, 1))
            ))
        }

        func setTerminalTitle(source: TerminalView, title: String) {
            // The daemon says what a pane is running; a title the program
            // sets is not otag's to believe.
        }

        func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}

        func scrolled(source: TerminalView, position: Double) {}

        func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {
            guard let url = URL(string: link), let scheme = url.scheme,
                  ["http", "https", "mailto", "file"].contains(scheme.lowercased())
            else {
                // Anything else is not otag's to open on a program's word.
                return
            }
            NSWorkspace.shared.open(url)
        }

        func bell(source: TerminalView) {
            NSSound.beep()
        }

        func clipboardCopy(source: TerminalView, content: Data) {
            guard let text = String(data: content, encoding: .utf8) else { return }
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
        }

        func iTermContent(source: TerminalView, content: ArraySlice<UInt8>) {}

        func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}
    }
}
