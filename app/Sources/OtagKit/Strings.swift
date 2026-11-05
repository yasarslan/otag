import Foundation
import Observation

/// A language otag speaks.
///
/// Three, and the list is deliberately short: a language otag offers is a
/// language somebody has to keep correct, and three kept correct beats
/// twelve half done.
public enum Language: String, Sendable, CaseIterable, Identifiable {
    case english = "en"
    case turkish = "tr"
    case finnish = "fi"

    public var id: String { rawValue }

    /// The one otag falls back to, and the one the strings are written in.
    public static let fallback = Language.english

    /// What this language calls itself.
    ///
    /// Its own name rather than the current language's word for it: someone
    /// who has otag in a language they cannot read needs to find their own
    /// in the list, and "Türkçe" is findable where "Turkish" is not.
    public var endonym: String {
        switch self {
        case .english: return "English"
        case .turkish: return "Türkçe"
        case .finnish: return "Suomi"
        }
    }

    /// The locale dates and numbers are formatted for.
    public var locale: Locale {
        Locale(identifier: rawValue)
    }

    /// The language a stored preference means.
    ///
    /// Nothing stored means the system's: somebody whose Mac is in Turkish
    /// should get Turkish without choosing it. A language otag does not
    /// speak means English.
    public static func stored(_ raw: String?) -> Language {
        if let raw, let chosen = Language(rawValue: raw) {
            return chosen
        }
        guard raw == nil || raw?.isEmpty == true else { return fallback }
        return system
    }

    /// The first of the user's preferred languages that otag speaks.
    public static var system: Language {
        for preferred in Locale.preferredLanguages {
            // "tr-TR" and "tr" both mean Turkish.
            let code = String(preferred.prefix(2))
            if let found = Language(rawValue: code) { return found }
        }
        return fallback
    }
}

/// Every user-visible string in otag.
///
/// One layer, one call shape: `Strings.t("guard.title")`. Nothing outside
/// this file and the `.strings` files beside it may hold a sentence a person
/// reads — that is what makes a missing translation a test failure rather
/// than a surprise in the window.
///
/// ## Why not a String Catalog
///
/// A `.xcstrings` catalog is compiled by `xcstringstool`, which is an Xcode
/// build phase. This package builds with plain `swift build`, and SwiftPM
/// copies a catalog into the bundle **verbatim** — every lookup would return
/// its own key. Verified before this was written. `.lproj/Localizable.strings`
/// is the same localization layer in a format this build understands, and
/// SwiftPM compiles and copies it natively.
///
/// ## Why not `String(localized:locale:)`
///
/// Because it ignores the `locale:` it is given when it chooses a table: it
/// returns whatever the bundle's own preferred localization is. Also
/// verified. Switching language without a restart therefore means looking
/// the key up in the chosen language's bundle, which is what `table(for:)`
/// does.
@MainActor
@Observable
public final class Strings {
    /// Where the chosen language is kept.
    public static let key = "language"

    /// The one the whole app reads.
    ///
    /// Observable, so changing `language` redraws every view that read a
    /// string through it. That is the whole mechanism for switching without
    /// a restart: there is no reloading and nothing to restart.
    public static let shared = Strings()

    /// The language otag is speaking.
    public var language: Language {
        didSet {
            guard language != oldValue else { return }
            defaults.set(language.rawValue, forKey: Self.key)
            // The number and date helpers are called from wherever a value
            // is drawn, so they are told rather than asked.
            Readable.format(for: language.locale)
        }
    }

    private let defaults: UserDefaults
    /// One bundle per language, found once.
    private var tables: [Language: Bundle] = [:]

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        language = Language.stored(defaults.string(forKey: Self.key))
        Readable.format(for: language.locale)
    }

    /// The locale dates and numbers are formatted for.
    public var locale: Locale {
        language.locale
    }

    /// One string, in the language otag is speaking.
    public static func t(_ key: String) -> String {
        shared.lookUp(key)
    }

    /// One string with values put into it.
    ///
    /// The placeholders are the translation's, so a language that needs them
    /// in another order can say `%2$@ … %1$@` and nothing here changes.
    public static func t(_ key: String, _ arguments: CVarArg...) -> String {
        String(format: shared.lookUp(key), locale: shared.locale, arguments: arguments)
    }

    /// One string in a named language, whatever otag is speaking.
    ///
    /// For the language menu, which lists every language in its own words,
    /// and for the tests.
    public func lookUp(_ key: String, in language: Language) -> String {
        guard let table = table(for: language) else { return key }
        // `value: nil` makes a missing key come back as the key itself,
        // which is what the completeness test looks for.
        let found = table.localizedString(forKey: key, value: nil, table: nil)
        guard found != key, !found.isEmpty else {
            // English is the language the strings are written in, so it is
            // the only honest fallback — and falling back is visible,
            // because the test that forbids a missing key runs on every
            // build.
            guard language != .fallback else { return key }
            return lookUp(key, in: .fallback)
        }
        return found
    }

    /// One string in the language otag is speaking.
    public func lookUp(_ key: String) -> String {
        lookUp(key, in: language)
    }

    /// The bundle holding one language's strings.
    public func table(for language: Language) -> Bundle? {
        if let found = tables[language] { return found }
        guard let path = Self.resources.path(forResource: language.rawValue, ofType: "lproj"),
              let bundle = Bundle(path: path)
        else { return nil }
        tables[language] = bundle
        return bundle
    }

    /// The bundle the `.lproj` folders live in.
    ///
    /// `Bundle.module` of this target, exposed so the completeness test can
    /// read the same files the app does rather than a copy of them.
    public static var resources: Bundle { .module }
}
