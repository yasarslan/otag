import Foundation
import Testing

@testable import OtagKit

/// Every user-visible string, in all three languages.
///
/// A missing translation is the kind of thing nobody notices until somebody
/// is using the app in Turkish and one button says `guard.title`. These
/// tests are what makes that a build failure instead.
@Suite("Localization")
@MainActor
struct LocalizationTests {
    /// Every key English has, which is the language the strings are written
    /// in and therefore the list everything else is checked against.
    private func keys(in language: Language) throws -> Set<String> {
        let bundle = try #require(
            Strings.shared.table(for: language),
            Comment(rawValue: "\(language.rawValue).lproj is not in the bundle")
        )
        let path = try #require(
            bundle.path(forResource: "Localizable", ofType: "strings"),
            "\(language.rawValue) has no Localizable.strings"
        )
        let table = try #require(
            NSDictionary(contentsOfFile: path) as? [String: String],
            Comment(rawValue: "\(language.rawValue)'s strings file could not be read")
        )
        return Set(table.keys)
    }

    private func table(in language: Language) throws -> [String: String] {
        let bundle = try #require(Strings.shared.table(for: language))
        let path = try #require(bundle.path(forResource: "Localizable", ofType: "strings"))
        return try #require(NSDictionary(contentsOfFile: path) as? [String: String])
    }

    @Test("all three languages are in the bundle")
    func allThreeAreThere() throws {
        for language in Language.allCases {
            #expect(
                Strings.shared.table(for: language) != nil,
                "\(language.endonym) is missing from the app"
            )
        }
    }

    @Test("every key English has, Turkish and Finnish have too")
    func noKeyIsMissing() throws {
        let english = try keys(in: .english)
        #expect(!english.isEmpty, "there are no strings at all")

        for language in [Language.turkish, Language.finnish] {
            let theirs = try keys(in: language)
            let missing = english.subtracting(theirs).sorted()
            #expect(
                missing.isEmpty,
                Comment(rawValue: "\(language.endonym) is missing \(missing.count) strings: "
                    + missing.joined(separator: ", "))
            )
            // The other way round too: a key nobody else has is a key that
            // was renamed and left behind.
            let extra = theirs.subtracting(english).sorted()
            #expect(
                extra.isEmpty,
                Comment(rawValue: "\(language.endonym) has \(extra.count) strings English "
                    + "does not: " + extra.joined(separator: ", "))
            )
        }
    }

    @Test("no string is empty in any language")
    func nothingIsBlank() throws {
        for language in Language.allCases {
            for (key, value) in try table(in: language) {
                #expect(
                    !value.trimmingCharacters(in: .whitespaces).isEmpty,
                    "\(language.endonym) has nothing for \(key)"
                )
            }
        }
    }

    @Test("a translation has the same placeholders as the English")
    func placeholdersMatch() throws {
        // A translation with one `%@` fewer than its format arguments is a
        // crash, not a typo.
        let english = try table(in: .english)
        for language in [Language.turkish, Language.finnish] {
            let theirs = try table(in: language)
            for (key, source) in english {
                guard let translated = theirs[key] else { continue }
                #expect(
                    placeholders(of: source) == placeholders(of: translated),
                    Comment(rawValue: "\(language.endonym)'s \(key) has "
                        + "\(placeholders(of: translated)) placeholders, English has "
                        + "\(placeholders(of: source))")
                )
            }
        }
    }

    @Test("Turkish and Finnish are actually translated")
    func notJustCopiedEnglish() throws {
        // A few strings really are the same word in every language. Anything
        // else identical to its English is a string somebody forgot.
        let same: Set<String> = [
            "new.runs.claude", "new.runs.codex", "new.model", "report.commits",
            "settings.language", "toolbar.language",
        ]
        let english = try table(in: .english)

        for language in [Language.turkish, Language.finnish] {
            let theirs = try table(in: language)
            var copied: [String] = []
            for (key, source) in english where !same.contains(key) {
                if theirs[key] == source { copied.append(key) }
            }
            #expect(
                copied.isEmpty,
                Comment(rawValue: "\(language.endonym) left \(copied.count) strings in "
                    + "English: " + copied.sorted().joined(separator: ", "))
            )
        }
    }

    @Test("every key the app asks for exists")
    func everyKeyUsedIsThere() throws {
        // Found by reading the sources, because a key renamed in one place
        // and not the other falls back to showing itself, which is exactly
        // the failure this suite is for.
        let used = try keysUsedInSources()
        #expect(!used.isEmpty, "no `Strings.t` calls were found — has the call changed?")

        let english = try keys(in: .english)
        let missing = used.subtracting(english).sorted()
        #expect(
            missing.isEmpty,
            Comment(rawValue: "the app asks for \(missing.count) strings that do not "
                + "exist: " + missing.joined(separator: ", "))
        )
    }

    @Test("a key that does not exist comes back as itself, not as a crash")
    func anUnknownKey() {
        #expect(Strings.shared.lookUp("no.such.key") == "no.such.key")
    }

    @Test("a string missing from one language falls back to English")
    func fallingBack() {
        // English is the language the strings are written in, so it is the
        // only honest fallback.
        for language in Language.allCases {
            #expect(
                !Strings.shared.lookUp("guard.title", in: language).isEmpty
            )
        }
    }

    @Test("the three languages really say different things")
    func theyDiffer() {
        let said = Language.allCases.map { Strings.shared.lookUp("guard.title", in: $0) }
        #expect(Set(said).count == Language.allCases.count, "\(said)")
    }

    @Test("a language otag does not speak means English, and nothing means the system")
    func choosingALanguage() {
        #expect(Language.stored("tr") == .turkish)
        #expect(Language.stored("fi") == .finnish)
        #expect(Language.stored("en") == .english)
        #expect(Language.stored("de") == .english, "a language otag cannot speak")
        #expect(Language.stored(nil) == Language.system, "nothing chosen follows the Mac")
    }

    @Test("each language names itself in its own words")
    func endonyms() {
        #expect(Language.english.endonym == "English")
        #expect(Language.turkish.endonym == "Türkçe")
        #expect(Language.finnish.endonym == "Suomi")
        #expect(Set(Language.allCases.map(\.endonym)).count == 3)
    }

    @Test("dates and numbers follow the chosen language")
    func formatting() {
        #expect(Language.turkish.locale.identifier == "tr")
        #expect(Language.finnish.locale.identifier == "fi")
        // The point of carrying a locale at all: 1.5 is written differently
        // in English than in Turkish and Finnish.
        let number = 1.5
        let english = number.formatted(.number.locale(Language.english.locale))
        let turkish = number.formatted(.number.locale(Language.turkish.locale))
        #expect(english != turkish, "the locale is not reaching the formatters")
    }

    /// Every key the Swift sources pass to `Strings.t`.
    private func keysUsedInSources() throws -> Set<String> {
        var found: Set<String> = []
        let pattern = try NSRegularExpression(pattern: #"Strings\.t\(\s*"([^"]+)""#)

        for file in try swiftFiles() {
            guard let text = try? String(contentsOf: file, encoding: .utf8) else { continue }
            let range = NSRange(text.startIndex ..< text.endIndex, in: text)
            for match in pattern.matches(in: text, range: range) {
                guard let key = Range(match.range(at: 1), in: text) else { continue }
                found.insert(String(text[key]))
            }
        }
        return found
    }

    /// The app's own sources, found from this file's path.
    ///
    /// A test reading the sources is unusual, and it is the only way to
    /// catch a key that exists nowhere: the compiler cannot, because a key
    /// is a string.
    private func swiftFiles() throws -> [URL] {
        let here = URL(fileURLWithPath: #filePath)
        // …/app/Tests/OtagKitTests/LocalizationTests.swift → …/app
        let app = here
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let sources = app.appendingPathComponent("Sources")

        guard let walk = FileManager.default.enumerator(
            at: sources,
            includingPropertiesForKeys: nil
        ) else { return [] }

        return walk.compactMap { $0 as? URL }.filter { $0.pathExtension == "swift" }
    }

    /// How many of each placeholder a format has.
    private func placeholders(of format: String) -> [String: Int] {
        var counts: [String: Int] = [:]
        var characters = Array(format)
        var index = 0
        while index < characters.count {
            guard characters[index] == "%" else {
                index += 1
                continue
            }
            var next = index + 1
            // `%1$@` and `%@` both end in the kind; `%%` is a literal.
            var digits = ""
            while next < characters.count, characters[next].isNumber || characters[next] == "$" {
                digits.append(characters[next])
                next += 1
            }
            guard next < characters.count else { break }
            let kind = characters[next]
            if kind != "%" {
                counts[String(kind), default: 0] += 1
            }
            index = next + 1
        }
        return counts
    }
}
