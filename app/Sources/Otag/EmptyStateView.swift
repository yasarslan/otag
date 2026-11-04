import OtagKit
import SwiftUI

/// What fills the main area with nothing selected.
///
/// A card with one button and three lines, and each line names something
/// that is actually on screen: the list on the left, the button in a
/// session's header, and the dragging. Not a shortcut sheet — somebody
/// opening otag for the second time needs to know where things are, not
/// which keys exist.
struct EmptyStateView: View {
    let state: ConnectionState
    let newSession: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            switch state {
            case .connecting:
                Card {
                    ProgressView()
                    Text("Connecting to the otag daemon…")
                        .foregroundStyle(.secondary)
                }

            case .connected:
                Card {
                    Text("Start a session")
                        .font(.title2.weight(.semibold))

                    Button {
                        newSession()
                    } label: {
                        Label("New session", systemImage: "plus")
                    }
                    .primaryActionStyle(prominent: true)
                    .controlSize(.large)
                    .padding(.bottom, 4)

                    VStack(alignment: .leading, spacing: 10) {
                        line(
                            "sidebar.left",
                            "Your sessions are listed on the left. Click one to see it."
                        )
                        line(
                            "arrow.left.arrow.right",
                            "**Switch to Codex** hands the work to the other agent, "
                                + "with a package it can read."
                        )
                        line(
                            "square.split.2x1",
                            "Drag a session from the list onto the side of this area "
                                + "to put two side by side."
                        )
                    }
                    .frame(maxWidth: 340, alignment: .leading)
                }

            case let .failed(message):
                Card {
                    Image(systemName: "exclamationmark.triangle")
                        .font(.system(size: 30, weight: .light))
                        .foregroundStyle(.tertiary)
                    Text("Not connected")
                        .font(.title3)
                    Text(message)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: 340)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func line(_ symbol: String, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Image(systemName: symbol)
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(width: 16)
            Text(.init(text))
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// The one card the empty state ever is.
    private struct Card<Content: View>: View {
        @ViewBuilder let content: Content

        var body: some View {
            VStack(spacing: 14) {
                content
            }
            .padding(28)
            .frame(width: 420)
            .cardBackground()
        }
    }
}
