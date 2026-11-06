import Foundation
import OtagKit
import SwiftUI

/// Confirmation comes from a daemon-generated list and a checked tree identity.
struct TeamUndoSheet: View {
    let socket: URL
    let pane: PaneID
    let step: UInt64
    @Environment(\.dismiss) private var dismiss
    @State private var preview: TeamUndoPreview?
    @State private var problem: String?
    @State private var busy = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(Strings.t("team.undo")).font(.headline)
            Text(Strings.t("team.undo.detail")).font(.callout).foregroundStyle(.secondary)
            if let preview {
                if preview.files.isEmpty { Text(Strings.t("team.undo.empty")) }
                else {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 4) {
                            ForEach(preview.files, id: \.self) { Text($0).font(.system(.body, design: .monospaced)) }
                        }.frame(maxWidth: .infinity, alignment: .leading)
                    }.frame(maxHeight: 240)
                }
            } else if problem == nil { ProgressView() }
            if let problem { Text(problem).foregroundStyle(.secondary).textSelection(.enabled) }
            HStack {
                Button(Strings.t("sheet.cancel")) { dismiss() }.keyboardShortcut(.cancelAction)
                Spacer()
                Button(Strings.t("team.undo"), role: .destructive) { send(confirming: true) }
                    .disabled(preview == nil || preview?.files.isEmpty == true || busy)
            }
        }.padding(20).frame(width: 500)
            .task { send(confirming: false) }
    }

    private func send(confirming: Bool) {
        busy = true
        problem = nil
        Task {
            do {
                let connection = try DaemonConnection(socket: socket)
                defer { connection.close(); busy = false }
                if confirming, let preview {
                    try connection.send(.teamUndoConfirm(team: preview.team, step: preview.step, expected: preview.expected))
                } else { try connection.send(.teamUndoPreview(pane: pane, step: step)) }
                for await response in connection.responses {
                    switch response {
                    case let .teamUndoPreview(value): preview = value; return
                    case .ok where confirming: dismiss(); return
                    case let .error(message): problem = message; preview = nil; return
                    default: continue
                    }
                }
                problem = DaemonError.disconnected.localizedDescription
            } catch { problem = error.localizedDescription; busy = false }
        }
    }
}
