import OtagKit
import SwiftUI

/// Credentials are local view state until the user explicitly saves or tests them.
struct APIProviderPopover: View {
    let store: SessionStore
    @State private var key = ""
    @State private var model = ""
    @State private var busy = false
    @State private var message: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(Strings.t("api.title")).font(.headline)
            Picker(Strings.t("api.provider"), selection: .constant(store.apiProvider.provider)) {
                Text(AgentKind.deepseek.label).tag(store.apiProvider.provider)
            }
            SecureField(Strings.t("api.key"), text: $key)
                .textFieldStyle(.roundedBorder)
            Text(Strings.t(store.apiProvider.configured ? "api.key.saved" : "api.needKey"))
                .font(.caption).foregroundStyle(.secondary)
            Picker(Strings.t("api.model"), selection: $model) {
                ForEach(store.apiProvider.models, id: \.self) { Text($0).tag($0) }
            }
            Text(Strings.t("api.privacy")).font(.caption).foregroundStyle(.secondary)
            if let message { Text(message).font(.callout).foregroundStyle(.secondary) }
            HStack {
                Button(Strings.t("api.test")) { configure(test: true) }
                Spacer()
                if store.apiProvider.configured {
                    Button(Strings.t("api.remove"), role: .destructive) { remove() }
                }
                Button(Strings.t("api.save")) { configure(test: false) }
            }.disabled(busy || store.apiProvider.models.isEmpty || model.isEmpty)
            if busy { ProgressView().controlSize(.small) }
        }.padding(20).frame(width: 380)
            .onAppear {
                model = store.apiProvider.model
                store.send(.apiProvider(model: nil, refresh: false))
            }
            .onChange(of: store.apiProvider.model) { _, selected in if model.isEmpty { model = selected } }
            .onDisappear { key = "" }
    }

    private func configure(test: Bool) {
        message = nil
        let entered = key.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            if !entered.isEmpty {
                try ProviderKeychain.save(entered, service: store.apiProvider.keychainService, account: store.apiProvider.keychainAccount)
                key = ""
            } else if !store.apiProvider.configured {
                message = Strings.t("api.needKey"); return
            }
        } catch { message = Strings.t("api.key.error"); return }
        busy = true
        Task {
            defer { busy = false }
            do {
                let connection = try DaemonConnection(socket: store.socket)
                defer { connection.close() }
                try connection.send(.apiProvider(model: model, refresh: test))
                for await response in connection.responses {
                    switch response {
                    case let .apiProvider(info):
                        store.acceptProvider(info)
                        message = Strings.t(test ? (info.connected == true ? "api.test.ok" : "api.test.failed") : "api.saved")
                        return
                    case .error: message = Strings.t("api.test.failed"); return
                    default: continue
                    }
                }
                message = Strings.t("api.test.failed")
            } catch { message = Strings.t("api.test.failed") }
        }
    }

    private func remove() {
        do {
            try ProviderKeychain.remove(service: store.apiProvider.keychainService, account: store.apiProvider.keychainAccount)
            key = ""
            store.send(.apiProvider(model: nil, refresh: false))
            message = Strings.t("api.removed")
        } catch { message = Strings.t("api.key.error") }
    }
}
