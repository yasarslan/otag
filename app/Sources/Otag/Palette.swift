import OtagKit
import SwiftUI

/// One thing the palette can do.
struct PaletteItem: Identifiable {
    let id = UUID()
    /// What it is called.
    let title: String
    /// A second line, when there is something worth saying.
    var detail: String = ""
    /// The shortcut it also has, for the user to learn.
    var shortcut: String = ""
    /// An SF Symbol.
    var symbol: String = "command"
    /// What to do.
    let run: () -> Void
}

/// Everything by name: `Cmd-K` for actions, `Cmd-P` for sessions.
///
/// One list, one field, no modes: what is typed filters, return runs the
/// first match. A palette that needs to be learned is a menu with extra
/// steps, so this one is only a faster way to what the menus already have.
struct PaletteView: View {
    let title: String
    let items: [PaletteItem]
    let dismiss: () -> Void

    @State private var query = ""
    @State private var selection: UUID?
    @FocusState private var fieldFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            TextField(title, text: $query)
                .textFieldStyle(.plain)
                .font(.title3)
                .padding(14)
                .focused($fieldFocused)
                .onSubmit { run(first) }

            Divider()

            if matches.isEmpty {
                Text("Nothing matches")
                    .foregroundStyle(.secondary)
                    .padding(20)
            } else {
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(matches) { item in
                            Row(item: item, selected: item.id == (selection ?? first?.id)) {
                                run(item)
                            }
                        }
                    }
                }
                .frame(maxHeight: 320)
            }
        }
        .frame(width: 480)
        .onAppear { fieldFocused = true }
    }

    /// What the typed text matches, in the order the caller gave.
    ///
    /// The letters have to appear in order but not together, which is what
    /// makes `nse` find "New Session".
    private var matches: [PaletteItem] {
        let needle = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !needle.isEmpty else { return items }
        return items.filter { item in
            let haystack = (item.title + " " + item.detail).lowercased()
            var remaining = Substring(needle)
            for character in haystack where character == remaining.first {
                remaining = remaining.dropFirst()
                if remaining.isEmpty { return true }
            }
            return remaining.isEmpty
        }
    }

    private var first: PaletteItem? { matches.first }

    private func run(_ item: PaletteItem?) {
        guard let item else { return }
        dismiss()
        item.run()
    }

    /// One row.
    private struct Row: View {
        let item: PaletteItem
        let selected: Bool
        let run: () -> Void

        @State private var hovering = false

        var body: some View {
            HStack(spacing: 10) {
                Image(systemName: item.symbol)
                    .foregroundStyle(.secondary)
                    .frame(width: 18)
                VStack(alignment: .leading, spacing: 1) {
                    Text(item.title)
                    if !item.detail.isEmpty {
                        Text(item.detail)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                Spacer(minLength: 6)
                if !item.shortcut.isEmpty {
                    Text(item.shortcut)
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 7)
            .background(selected || hovering ? AnyShapeStyle(.quaternary)
                : AnyShapeStyle(.clear))
            .contentShape(Rectangle())
            .onTapGesture(perform: run)
            .onHover { hovering = $0 }
        }
    }
}

/// Which palette is open.
enum PaletteKind: Identifiable {
    /// Every action by name.
    case commands
    /// Every session by name.
    case sessions

    var id: Int {
        switch self {
        case .commands: return 0
        case .sessions: return 1
        }
    }

    var title: String {
        switch self {
        case .commands: return "Run a command…"
        case .sessions: return "Go to a session…"
        }
    }
}
