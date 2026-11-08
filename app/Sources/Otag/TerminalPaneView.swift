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
struct TerminalPaneView: View {
    let pane: PaneID
    let socket: URL
    let focused: Bool
    let dimmed: Bool
    let onEnded: (PaneExit?) -> Void
    let onGone: (String) -> Void
    let onFocus: () -> Void

    @AppStorage(Preference.theme) private var theme = TerminalThemeChoice.glassDark.rawValue

    var body: some View {
        TerminalSurface(
            pane: pane, socket: socket, focused: focused,
            onEnded: onEnded, onGone: onGone, onFocus: onFocus
        )
        .opacity(dimmed ? 0.9 : 1)
        .padding(14)
        .background(Color(nsColor: TerminalProfile.colour(TerminalProfile.theme(for: theme).background)))
    }
}

private struct TerminalSurface: NSViewRepresentable {
    /// The pane to show.
    let pane: PaneID
    /// Where the daemon is.
    let socket: URL
    /// Whether this pane has the keyboard in its tab.
    let focused: Bool
    /// Called when the pane's program ends, so the tab can close it.
    let onEnded: (PaneExit?) -> Void
    /// Called when the daemon has no such pane any more, so the tab can say
    /// so instead of showing a terminal nothing is behind.
    let onGone: (String) -> Void
    /// Called when the user clicks into this pane.
    let onFocus: () -> Void

    @Environment(\.windowVisible) private var windowVisible

    @AppStorage(Preference.fontSize) private var fontSize = Preference.defaultFontSize
    @AppStorage(Preference.theme) private var theme = TerminalThemeChoice.glassDark.rawValue

    func makeCoordinator() -> Coordinator {
        Coordinator(
            pane: pane,
            socket: socket,
            onEnded: onEnded,
            onGone: onGone,
            onFocus: onFocus
        )
    }

    func makeNSView(context: Context) -> TerminalView {
        let view = SteadyTerminalView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        view.terminalDelegate = context.coordinator
        // One profile for every pane, whatever is running in it: see
        // `TerminalProfile`. The agents differ enough already.
        let theme = TerminalProfile.theme(for: self.theme)
        context.coordinator.drawn = theme
        TerminalProfile.apply(to: view, theme: theme, size: fontSize)
        // Leave contrast headroom for the inactive pane's 10% dimming.
        view.minimumContrastRatio = 5.5
        view.setCursorStyle(.steadyBlock)
        view.allowMouseReporting = true
        view.suspendsRenderingWhenNotVisible = true

        context.coordinator.attach(to: view, active: windowVisible)
        return view
    }

    func updateNSView(_ view: TerminalView, context: Context) {
        context.coordinator.onEnded = onEnded
        context.coordinator.onGone = onGone
        context.coordinator.onFocus = onFocus
        context.coordinator.setActive(windowVisible)
        // Changing the font changes how many cells fit, and the view reports
        // the new size, which the daemon then tells the program. The theme
        // is watched the same way, which is what makes choosing one in
        // Settings reach every pane that is already open — and what makes a
        // switch between light and dark follow, for anyone who left the
        // choice on the system.
        let theme = TerminalProfile.theme(for: self.theme)
        if view.font.pointSize != fontSize || context.coordinator.drawn != theme {
            context.coordinator.drawn = theme
            TerminalProfile.apply(to: view, theme: theme, size: fontSize)
        }
        if focused, view.window?.firstResponder !== view {
            view.window?.makeFirstResponder(view)
        }
    }

    static func dismantleNSView(_ view: TerminalView, coordinator: Coordinator) {
        coordinator.stop()
        closeUI(view)
    }

    private static func closeUI(_ view: TerminalView) {
        // SwiftTerm can defer teardown while a submitted GPU frame finishes.
        // Retry only that teardown; there is no repeating cleanup timer.
        if !view.updateUiClosed() {
            DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(16)) {
                closeUI(view)
            }
        }
    }

    /// Carries bytes between the daemon and one terminal view.
    @MainActor
    final class Coordinator: NSObject, TerminalViewDelegate {
        private let stream: PaneStream
        private weak var view: TerminalView?
        private var active = false
        private var pending: [UInt8] = []
        private var flushWork: DispatchWorkItem?
        /// The theme the view was last drawn in, so that a change reinstalls
        /// the palette and nothing else does.
        var drawn: TerminalTheme?
        var onEnded: (PaneExit?) -> Void
        var onGone: (String) -> Void
        var onFocus: () -> Void

        init(
            pane: PaneID,
            socket: URL,
            onEnded: @escaping (PaneExit?) -> Void,
            onGone: @escaping (String) -> Void,
            onFocus: @escaping () -> Void
        ) {
            stream = PaneStream(pane: pane, socket: socket)
            self.onEnded = onEnded
            self.onGone = onGone
            self.onFocus = onFocus
            super.init()
        }

        /// Start following the pane and feeding this view.
        func attach(to view: TerminalView, active: Bool) {
            self.view = view

            stream.onScreen = { [weak self] size, bytes in
                guard let self, let view = self.view else { return }
                cancelFlush()
                // The bytes clear the screen themselves and then draw it, so
                // feeding them is enough; the size is the daemon agreeing
                // with what this view asked for.
                _ = size
                view.feed(byteArray: bytes[...])
            }
            stream.onOutput = { [weak self] bytes in
                self?.enqueue(bytes)
            }
            stream.onEnded = { [weak self] exit in
                self?.flush()
                self?.onEnded(exit)
            }
            stream.onFailure = { [weak self] message in
                self?.flush()
                // Said on the pane itself, dimmed: it is otag speaking, not
                // the program.
                let text = "\r\n\u{1b}[2m-- \(message) --\u{1b}[0m\r\n"
                self?.view?.feed(text: text)
            }
            stream.onGone = { [weak self] message in
                self?.onGone(message)
            }

            setActive(active)
        }

        /// A hidden pane detaches from raw output. The daemon retains its
        /// screen, and a new attach replays that screen on becoming visible.
        func setActive(_ active: Bool) {
            guard self.active != active else { return }
            self.active = active
            guard active, let view else {
                cancelFlush()
                stream.stop()
                return
            }
            let size = view.terminalDimensions
            stream.start(size: PaneSize(
                rows: UInt16(max(size.rows, 1)),
                cols: UInt16(max(size.cols, 1))
            ))
        }

        private func enqueue(_ bytes: [UInt8]) {
            guard active else { return }
            pending.append(contentsOf: bytes)
            if pending.count >= 64 * 1024 {
                flush()
            } else if flushWork == nil {
                let work = DispatchWorkItem { [weak self] in self?.flush() }
                flushWork = work
                DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(16), execute: work)
            }
        }

        private func flush() {
            flushWork?.cancel()
            flushWork = nil
            guard !pending.isEmpty else { return }
            view?.feed(byteArray: pending[...])
            pending.removeAll(keepingCapacity: true)
        }

        private func cancelFlush() {
            flushWork?.cancel()
            flushWork = nil
            pending.removeAll(keepingCapacity: true)
        }

        func stop() {
            active = false
            cancelFlush()
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

/// Keeps the cursor shape requested by the program with a steady presentation.
private final class SteadyTerminalView: TerminalView {
    nonisolated override func cursorStyleChanged(source: Terminal, newStyle: CursorStyle) {
        let steadyStyle: CursorStyle
        switch newStyle {
        case .blinkBlock, .steadyBlock: steadyStyle = .steadyBlock
        case .blinkUnderline, .steadyUnderline: steadyStyle = .steadyUnderline
        case .blinkBar, .steadyBar: steadyStyle = .steadyBar
        }
        super.cursorStyleChanged(source: source, newStyle: steadyStyle)
    }
}
