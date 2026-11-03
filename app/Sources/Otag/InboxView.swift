import OtagKit
import SwiftUI

/// What the agents are waiting for, with the two buttons that answer them.
///
/// It sits at the top of the sidebar and is not there at all when nothing is
/// waiting: the whole point is that a question finds the user, rather than
/// the user going from pane to pane looking for one.
struct InboxView: View {
    let store: SessionStore
    /// Called to show the session a question came from.
    let reveal: (PaneID) -> Void

    var body: some View {
        ForEach(store.asks) { ask in
            AskRow(ask: ask, store: store, reveal: reveal)
        }
    }
}

/// One question.
private struct AskRow: View {
    let ask: AskView
    let store: SessionStore
    let reveal: (PaneID) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "questionmark.circle.fill")
                    .foregroundStyle(.tint)
                Text(who)
                    .font(.caption.weight(.medium))
                Spacer(minLength: 0)
                Button {
                    reveal(ask.pane)
                } label: {
                    Image(systemName: "arrow.right.circle")
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .help("Show this session")
            }

            Text(ask.summary)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(3)
                .fixedSize(horizontal: false, vertical: true)

            HStack(spacing: 6) {
                Button("Allow") { store.answer(ask, with: .allow) }
                    .controlSize(.small)
                Button("Deny") { store.answer(ask, with: .deny) }
                    .controlSize(.small)
                Spacer(minLength: 0)
            }
        }
        .padding(.vertical, 4)
    }

    private var who: String {
        let name = store.pane(ask.pane)?.workspace ?? ""
        let agent = ask.agent?.label ?? "a session"
        return name.isEmpty ? agent : "\(agent) · \(name)"
    }
}

/// What otag says about an answer it could not deliver.
///
/// An answer is typed into a running agent, so otag never claims one worked:
/// if the prompt is still up afterwards, this is what says so.
struct AnswerProblemView: View {
    let problem: String
    let dismiss: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "exclamationmark.triangle")
                .foregroundStyle(.secondary)
            Text(problem)
                .font(.caption)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            Button {
                dismiss()
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 9, weight: .bold))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
        }
        .padding(8)
        .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
        .padding(.horizontal, 10)
        .padding(.bottom, 6)
    }
}

/// One line across the bottom of the window: what otag just did.
///
/// A handoff, or something the night shift sent. It stays until it is
/// dismissed, because the thing it is reporting already happened.
struct NoticeBar: View {
    let notice: String
    let dismiss: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "info.circle")
                .foregroundStyle(.secondary)
            Text(notice)
                .font(.caption)
                .lineLimit(2)
            Spacer(minLength: 0)
            Button {
                dismiss()
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 9, weight: .bold))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }
}
