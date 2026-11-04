import Foundation
import Testing

@testable import OtagKit

/// What a session is called.
///
/// The names are what makes the sidebar readable, so the rules that make
/// them are worth pinning down: a prompt goes in and one short line of
/// English comes out, or nothing at all.
@Suite("Session names")
struct SessionNameTests {
    @Test("a name is the first clause of the first prompt")
    func theUsualCase() {
        #expect(SessionName.from(prompt: "add a test for the transcript parser")
            == "Add a test for the transcript parser")
    }

    @Test("an empty opening is dropped")
    func fillerGoes() {
        let openings = [
            "can you add a changelog entry",
            "could you please add a changelog entry",
            "please add a changelog entry",
            "I want you to add a changelog entry",
            "let's add a changelog entry",
            "now add a changelog entry",
            "hey, add a changelog entry",
        ]
        for prompt in openings {
            #expect(
                SessionName.from(prompt: prompt) == "Add a changelog entry",
                "\"\(prompt)\" kept its opening"
            )
        }
    }

    @Test("a word that only begins like filler is left alone")
    func fillerIsWholeWordsOnly() {
        #expect(SessionName.from(prompt: "pleased to meet you") == "Pleased to meet you")
        #expect(SessionName.from(prompt: "nowhere does it say that") == "Nowhere does it say that")
    }

    @Test("the name is cut on a word, never mid-word")
    func cutOnAWord() throws {
        let long = "refactor the transcript parser so that every field is optional and "
            + "nothing panics"
        let name = try #require(SessionName.from(prompt: long))
        #expect(name.count <= SessionName.limit)
        #expect(!name.hasSuffix(" "))
        #expect(long.lowercased().hasPrefix(name.lowercased()), "it is a prefix of the prompt")
        #expect(name == "Refactor the transcript parser so that")
    }

    @Test("one enormous word is still a name")
    func oneVeryLongWord() throws {
        let name = try #require(SessionName.from(prompt: String(repeating: "a", count: 200)))
        #expect(name.count == SessionName.limit)
    }

    @Test("a fenced code block is not the name")
    func codeFencesAreSkipped() {
        let prompt = """
        ```
        fn main() {}
        ```
        make this compile
        """
        #expect(SessionName.from(prompt: prompt) == "Make this compile")
    }

    @Test("a line that is only a path is not the name")
    func pathsAreSkipped() {
        let prompt = """
        crates/otag-core/src/session.rs
        split this file up
        """
        #expect(SessionName.from(prompt: prompt) == "Split this file up")
        #expect(SessionName.from(prompt: "~/notes/todo.md\nread my notes") == "Read my notes")
    }

    @Test("a heading or a bullet is punctuation around the ask")
    func markdownIsStripped() {
        #expect(SessionName.from(prompt: "## fix the parser") == "Fix the parser")
        #expect(SessionName.from(prompt: "- fix the parser") == "Fix the parser")
    }

    @Test("trailing punctuation goes, and whitespace collapses")
    func tidying() {
        #expect(SessionName.from(prompt: "  fix   the    parser.  ") == "Fix the parser")
        #expect(SessionName.from(prompt: "why does this fail?") == "Why does this fail")
    }

    @Test("a name is sentence case, and words that are already shouting stay that way")
    func casing() {
        #expect(SessionName.from(prompt: "trim CLAUDE.md") == "Trim CLAUDE.md")
        #expect(SessionName.from(prompt: "PaneID should be a struct") == "PaneID should be a struct")
    }

    @Test("a prompt with nothing in it has no name")
    func nothingToName() {
        #expect(SessionName.from(prompt: "") == nil)
        #expect(SessionName.from(prompt: "   \n\n  ") == nil)
        #expect(SessionName.from(prompt: "!!! ???") == nil)
        #expect(SessionName.from(prompt: "please") == nil, "filler and nothing else")
        #expect(SessionName.from(prompt: "```\ncode only\n```") == nil)
    }

    @Test("a session falls back to its folder, then to what is running in it")
    func theFallbackOrder() {
        var pane = PaneInfo(id: 1, size: PaneSize(rows: 24, cols: 80))
        pane.agent = .claude
        pane.cwd = "/Users/someone/work/api-gateway"
        pane.workspace = "api-gateway"

        #expect(SessionName.of(pane: pane) == "api-gateway", "the folder it works in")

        pane.cwd = nil
        #expect(SessionName.of(pane: pane) == "api-gateway", "then the project")

        pane.workspace = ""
        #expect(SessionName.of(pane: pane) == "Claude", "then the agent")

        pane.agent = nil
        #expect(SessionName.of(pane: pane) == "shell")
    }

    @Test("the user's own name wins over everything")
    func renameWins() {
        var pane = PaneInfo(id: 1, size: PaneSize(rows: 24, cols: 80))
        pane.agent = .codex
        pane.cwd = "/work/api"

        #expect(
            SessionName.of(pane: pane, chosen: "the hard one", fromFirstPrompt: "Fix the parser")
                == "the hard one"
        )
        #expect(
            SessionName.of(pane: pane, chosen: "   ", fromFirstPrompt: "Fix the parser")
                == "Fix the parser",
            "a blank name is not a name"
        )
    }
}

/// The names, kept between runs.
@Suite("Remembering session names")
@MainActor
struct SessionNamesStoreTests {
    /// A store of its own, so a test never touches the real preferences.
    private func store() -> (SessionNames, UserDefaults, String) {
        let suite = "otag.names.test.\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        return (SessionNames(defaults: defaults), defaults, suite)
    }

    private func pane(_ id: PaneID, cwd: String = "/work/api") -> PaneInfo {
        var pane = PaneInfo(id: id, size: PaneSize(rows: 24, cols: 80))
        pane.agent = .claude
        pane.cwd = cwd
        return pane
    }

    @Test("a first prompt becomes the name, and later prompts do not")
    func firstPromptOnly() {
        let (names, _, suite) = store()
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        names.remember(firstPrompt: "add a parser test", for: 1)
        #expect(names.name(of: pane(1)) == "Add a parser test")

        names.remember(firstPrompt: "now do something else", for: 1)
        #expect(names.name(of: pane(1)) == "Add a parser test", "only the first counts")
    }

    @Test("a prompt with no name in it leaves the fallback alone")
    func anEmptyPrompt() {
        let (names, _, suite) = store()
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        names.remember(firstPrompt: "   ", for: 1)
        #expect(names.name(of: pane(1)) == "api", "still the folder")
        #expect(!names.isRenamed(1))
    }

    @Test("renaming wins, and an empty rename puts the old name back")
    func renaming() {
        let (names, _, suite) = store()
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        names.remember(firstPrompt: "add a parser test", for: 1)
        names.rename(1, to: "  the parser  ")
        #expect(names.name(of: pane(1)) == "the parser")
        #expect(names.isRenamed(1))

        names.rename(1, to: "")
        #expect(names.name(of: pane(1)) == "Add a parser test")
        #expect(!names.isRenamed(1))
    }

    @Test("names survive a restart")
    func persistence() {
        let (names, defaults, suite) = store()
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        names.remember(firstPrompt: "add a parser test", for: 1)
        names.rename(2, to: "the hard one")

        let reopened = SessionNames(defaults: defaults)
        #expect(reopened.name(of: pane(1)) == "Add a parser test")
        #expect(reopened.name(of: pane(2)) == "the hard one")
    }

    @Test("a session the daemon no longer runs is forgotten")
    func pruning() {
        let (names, defaults, suite) = store()
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        names.rename(1, to: "one")
        names.rename(2, to: "two")
        names.prune(to: [2])

        #expect(!names.isRenamed(1))
        #expect(names.isRenamed(2))

        let reopened = SessionNames(defaults: defaults)
        #expect(!reopened.isRenamed(1), "and it stays forgotten")
    }
}
