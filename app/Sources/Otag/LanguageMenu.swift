import OtagKit
import SwiftUI

/// The language, from the toolbar.
///
/// Each language is listed in its own words — "Türkçe", not "Turkish" —
/// because somebody who has otag in a language they cannot read needs to
/// find their own in the list.
struct LanguageMenu: View {
    @State private var strings = Strings.shared

    var body: some View {
        Menu {
            Picker("", selection: Binding(
                get: { strings.language },
                set: { strings.language = $0 }
            )) {
                ForEach(Language.allCases) { language in
                    Text(language.endonym).tag(language)
                }
            }
            .pickerStyle(.inline)
            .labelsHidden()
        } label: {
            Label(Strings.t("toolbar.language"), systemImage: "globe")
                .labelStyle(.titleAndIcon)
                .foregroundStyle(.primary)
        }
        .menuStyle(.button)
        .labelStyle(.titleAndIcon)
        .chromeButtonStyle()
        .controlSize(.regular)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(Strings.t("toolbar.language"))
        .accessibilityLabel(Strings.t("toolbar.language"))
    }
}

/// The language, in Settings.
///
/// The same preference as the toolbar menu; a person who looks in Settings
/// for a language should find it there rather than being told to look in the
/// toolbar.
struct LanguageSettings: View {
    @State private var strings = Strings.shared

    var body: some View {
        Section(Strings.t("settings.language")) {
            Picker(Strings.t("settings.language"), selection: Binding(
                get: { strings.language },
                set: { strings.language = $0 }
            )) {
                ForEach(Language.allCases) { language in
                    Text(language.endonym).tag(language)
                }
            }
            .labelsHidden()
            .pickerStyle(.inline)

            Text(Strings.t("settings.language.detail"))
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
