import AppKit
import OtagKit
import SwiftUI
import WebKit

/// Retain navigation history across layout changes, without eagerly creating
/// web views for saved sessions. Closing a session drops its only owner.
@MainActor
final class BrowserWebViews {
    private var pages: [PaneID: BrowserPage] = [:]

    func page(for session: BrowserSession, sessions: BrowserSessions) -> BrowserPage {
        if let page = pages[session.id] { return page }
        let page = BrowserPage(url: session.url) { [weak sessions] url in
            sessions?.navigate(session.id, to: url)
        }
        pages[session.id] = page
        return page
    }

    func retain(_ ids: Set<PaneID>) {
        for id in Array(pages.keys) where !ids.contains(id) {
            pages.removeValue(forKey: id)?.stop()
        }
    }
}

@MainActor
@Observable
final class BrowserPage: NSObject, WKNavigationDelegate, WKUIDelegate {
    let webView: WKWebView
    var address: String
    var canGoBack = false
    var canGoForward = false
    var problem: String?
    private let navigated: (URL) -> Void
    private var observations: [NSKeyValueObservation] = []

    init(url: URL, navigated: @escaping (URL) -> Void) {
        webView = WKWebView(frame: .zero)
        address = url.absoluteString
        self.navigated = navigated
        super.init()
        webView.navigationDelegate = self
        webView.uiDelegate = self
        observations = [
            webView.observe(\.canGoBack, options: [.initial, .new]) { [weak self] view, _ in
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    canGoBack = webView.canGoBack
                }
            },
            webView.observe(\.canGoForward, options: [.initial, .new]) { [weak self] view, _ in
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    canGoForward = webView.canGoForward
                }
            },
            webView.observe(\.url, options: [.new]) { [weak self] view, _ in
                Task { @MainActor [weak self] in
                    guard let self, let url = webView.url else { return }
                    address = url.absoluteString
                    self.navigated(url)
                }
            },
        ]
        webView.load(URLRequest(url: url))
    }

    func go() {
        guard let url = BrowserSessions.address(address) else {
            problem = Strings.t("browser.invalidAddress")
            return
        }
        problem = nil
        address = url.absoluteString
        navigated(url)
        webView.load(URLRequest(url: url))
    }

    func stop() {
        webView.stopLoading()
        webView.navigationDelegate = nil
        webView.uiDelegate = nil
        observations.removeAll()
    }

    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        let scheme = navigationAction.request.url?.scheme?.lowercased()
        decisionHandler(scheme == "http" || scheme == "https" || scheme == "about" ? .allow : .cancel)
    }

    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        if navigationAction.targetFrame == nil, let url = navigationAction.request.url,
           ["http", "https"].contains(url.scheme?.lowercased() ?? "") {
            webView.load(URLRequest(url: url))
        }
        return nil
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!,
                 withError error: Error) {
        if (error as NSError).code != NSURLErrorCancelled { problem = error.localizedDescription }
    }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        problem = nil
    }
}

struct BrowserPaneView: View {
    @Environment(\.pixelLength) private var pixelLength
    @Bindable var page: BrowserPage
    let close: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                Button { page.webView.goBack() } label: {
                    Image(systemName: "chevron.left")
                }
                .disabled(!page.canGoBack)
                .help(Strings.t("browser.back"))
                .accessibilityLabel(Strings.t("browser.back"))
                Button { page.webView.goForward() } label: {
                    Image(systemName: "chevron.right")
                }
                .disabled(!page.canGoForward)
                .help(Strings.t("browser.forward"))
                .accessibilityLabel(Strings.t("browser.forward"))
                Button { page.webView.reload() } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .help(Strings.t("browser.reload"))
                .accessibilityLabel(Strings.t("browser.reload"))
                TextField(Strings.t("browser.address"), text: $page.address)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { page.go() }
                Button {
                    if let url = page.webView.url ?? BrowserSessions.address(page.address) {
                        NSWorkspace.shared.open(url)
                    }
                } label: {
                    Image(systemName: "arrow.up.forward.square")
                }
                .help(Strings.t("browser.external"))
                .accessibilityLabel(Strings.t("browser.external"))
                Button(action: close) { Image(systemName: "xmark") }
                    .help(Strings.t("menu.closeSession"))
                    .accessibilityLabel(Strings.t("menu.closeSession"))
            }
            .buttonStyle(.borderless)
            .padding(.horizontal, Chrome.gutter)
            .frame(height: Chrome.headerHeight)
            .chromeBackground()
            .overlay(alignment: .bottom) {
                Chrome.hairline.frame(height: pixelLength).allowsHitTesting(false)
            }
            if let problem = page.problem {
                Text(problem)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(6)
            }
            BrowserWebView(page: page)
        }
    }
}

private struct BrowserWebView: NSViewRepresentable {
    let page: BrowserPage
    func makeNSView(context: Context) -> WKWebView { page.webView }
    func updateNSView(_ view: WKWebView, context: Context) {}
}
