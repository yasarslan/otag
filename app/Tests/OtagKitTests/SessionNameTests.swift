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

    @Test("a session with nothing asked of it is the agent and a number")
    func theFallback() {
        // Never the folder. Three sessions in one project used to be the
        // folder's name three times over, which is the same as no names.
        var pane = PaneInfo(id: 1, workspace: "api-gateway", agent: .claude)
        pane.cwd = "/Users/someone/work/api-gateway"

        #expect(SessionName.of(pane: pane, ordinal: 2) == "Claude 2")
        #expect(SessionName.of(pane: pane, ordinal: nil) == "Claude")

        pane.agent = .codex
        #expect(SessionName.of(pane: pane, ordinal: 1) == "Codex 1")

        pane.agent = nil
        #expect(SessionName.of(pane: pane, ordinal: 3) == "Shell 3")
    }

    @Test("the numbers count within an agent, in the daemon's order")
    func numbering() {
        let panes = [
            PaneInfo(id: 1, agent: .claude),
            PaneInfo(id: 2, agent: .codex),
            PaneInfo(id: 3, agent: .claude),
            PaneInfo(id: 4, agent: nil),
            PaneInfo(id: 5, agent: .codex),
        ]
        let ordinals = SessionName.ordinals(of: panes)
        #expect(ordinals[1] == 1)
        #expect(ordinals[3] == 2, "the second Claude session, not the third session")
        #expect(ordinals[2] == 1)
        #expect(ordinals[5] == 2)
        #expect(ordinals[4] == 1, "a shell is counted among the shells")
    }

    @Test("the first prompt the daemon reports is the name")
    func theDaemonsPrompt() {
        let pane = PaneInfo(
            id: 1,
            workspace: "api",
            agent: .claude,
            firstPrompt: "please fix the transcript parser"
        )
        #expect(SessionName.of(pane: pane, ordinal: 1) == "Fix the transcript parser")
    }

    @Test("a first prompt with no name in it falls through to the number")
    func anUnusablePrompt() {
        let pane = PaneInfo(id: 1, agent: .codex, firstPrompt: "```\ncode only\n```")
        #expect(SessionName.of(pane: pane, ordinal: 2) == "Codex 2")
    }

    @Test("the user's own name wins over everything")
    func renameWins() {
        let pane = PaneInfo(
            id: 1,
            workspace: "api",
            agent: .codex,
            firstPrompt: "fix the parser"
        )

        #expect(SessionName.of(pane: pane, chosen: "the hard one", ordinal: 1) == "the hard one")
        #expect(
            SessionName.of(pane: pane, chosen: "   ", ordinal: 1) == "Fix the parser",
            "a blank name is not a name"
        )
    }
}

/// The names the user has given, kept between runs.
@Suite("Remembering session names")
@MainActor
struct SessionNamesStoreTests {
    /// A store of its own, so a test never touches the real preferences.
    private func store() -> (SessionNames, UserDefaults, String) {
        let suite = "otag.names.test.\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        return (SessionNames(defaults: defaults), defaults, suite)
    }

    private func pane(_ id: PaneID, prompt: String? = nil) -> PaneInfo {
        PaneInfo(id: id, workspace: "api", cwd: "/work/api", agent: .claude, firstPrompt: prompt)
    }

    @Test("the daemon's first prompt names a session, with no help from here")
    func namesComeFromTheDaemon() {
        let (names, _, suite) = store()
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        let asked = pane(1, prompt: "add a parser test")
        names.took([asked])
        #expect(names.name(of: asked) == "Add a parser test")
    }

    @Test("sessions in one folder are told apart by a number, not by the folder")
    func sessionsInOneFolder() {
        let (names, _, suite) = store()
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        // What the bug looked like: three sessions in one project, all
        // called after the project.
        let panes = [pane(1), pane(2), pane(3)]
        names.took(panes)

        let shown = panes.map(names.name(of:))
        #expect(shown == ["Claude 1", "Claude 2", "Claude 3"])
        #expect(Set(shown).count == 3, "no two sessions may read the same")
        #expect(!shown.contains("api"), "the folder is not a name")
    }

    @Test("renaming wins, and an empty rename puts the old name back")
    func renaming() {
        let (names, _, suite) = store()
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        let asked = pane(1, prompt: "add a parser test")
        names.took([asked])
        names.rename(1, to: "  the parser  ")
        #expect(names.name(of: asked) == "the parser")
        #expect(names.isRenamed(1))

        names.rename(1, to: "")
        #expect(names.name(of: asked) == "Add a parser test")
        #expect(!names.isRenamed(1))
    }

    @Test("renames survive a restart")
    func persistence() {
        let (names, defaults, suite) = store()
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        names.rename(2, to: "the hard one")

        let reopened = SessionNames(defaults: defaults)
        reopened.took([pane(2)])
        #expect(reopened.name(of: pane(2)) == "the hard one")
    }

    @Test("a session the daemon no longer runs is forgotten")
    func pruning() {
        let (names, defaults, suite) = store()
        defer { UserDefaults().removePersistentDomain(forName: suite) }

        names.rename(1, to: "one")
        names.rename(2, to: "two")
        names.took([pane(2)])

        #expect(!names.isRenamed(1))
        #expect(names.isRenamed(2))

        let reopened = SessionNames(defaults: defaults)
        #expect(!reopened.isRenamed(1), "and it stays forgotten")
    }
}
