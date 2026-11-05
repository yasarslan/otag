import AppKit
import SwiftUI

/// Window notifications drive suspension; no visibility polling or activity
/// assertions that would prevent App Nap. Count all main windows so hiding
/// one does not suspend usage refresh in another.
struct WindowVisibility: NSViewRepresentable {
    let changed: (Bool, Bool) -> Void

    func makeNSView(context: Context) -> VisibilityProbe {
        let view = VisibilityProbe()
        view.changed = changed
        return view
    }

    func updateNSView(_ view: VisibilityProbe, context: Context) {
        view.changed = changed
    }

    static func dismantleNSView(_ view: VisibilityProbe, coordinator: ()) {
        view.stop()
    }
}

@MainActor
final class VisibilityProbe: NSView {
    private static var visible: Set<ObjectIdentifier> = []
    var changed: ((Bool, Bool) -> Void)?
    private var observers: [NSObjectProtocol] = []
    private var lastVisible: Bool?
    private var stopped = false

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard !stopped else { return }
        removeObservers()
        if let window {
            for name in [NSWindow.didChangeOcclusionStateNotification,
                         NSWindow.didMiniaturizeNotification,
                         NSWindow.didDeminiaturizeNotification,
                         NSWindow.willCloseNotification] {
                observe(name, object: window)
            }
            for name in [NSApplication.didHideNotification, NSApplication.didUnhideNotification] {
                observe(name, object: NSApp)
            }
        }
        // SwiftUI may still be installing the view hierarchy.
        Task { @MainActor [weak self] in self?.refresh() }
    }

    private func observe(_ name: Notification.Name, object: AnyObject) {
        observers.append(NotificationCenter.default.addObserver(forName: name, object: object, queue: .main) {
            [weak self] notification in
            Task { @MainActor [weak self] in
                if notification.name == NSWindow.willCloseNotification {
                    self?.report(false)
                } else {
                    self?.refresh()
                }
            }
        })
    }

    private func refresh() {
        guard !stopped else { return }
        let visible = window.map {
            !NSApp.isHidden && !$0.isMiniaturized && $0.occlusionState.contains(.visible)
        } ?? false
        report(visible)
    }

    private func report(_ visible: Bool) {
        if visible { Self.visible.insert(ObjectIdentifier(self)) }
        else { Self.visible.remove(ObjectIdentifier(self)) }
        guard lastVisible != visible else { return }
        lastVisible = visible
        changed?(visible, !Self.visible.isEmpty)
    }

    private func removeObservers() {
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
        observers.removeAll()
    }

    func stop() {
        stopped = true
        removeObservers()
        report(false)
        changed = nil
    }
}

private struct WindowVisibleKey: EnvironmentKey {
    static let defaultValue = true
}

extension EnvironmentValues {
    var windowVisible: Bool {
        get { self[WindowVisibleKey.self] }
        set { self[WindowVisibleKey.self] = newValue }
    }
}
