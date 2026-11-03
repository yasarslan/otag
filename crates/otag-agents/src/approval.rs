//! Answering an agent's permission prompt from outside the pane.
//!
//! Agents ask for permission on their own screen, and otag answers by typing
//! into that screen: there is no other way in. What to type is part of each
//! agent's interface, so it lives with that agent's adapter
//! ([`crate::AgentAdapter::answer_keys`]).
//!
//! Two rules keep this honest. The keys must be the ones the prompt itself
//! offers — a number from the list, or the `esc` the prompt names — and
//! nothing is sent to a pane whose screen does not currently show a prompt.
//! Whoever sends the keys checks afterwards that the prompt is gone, because
//! typing into an agent that was not asking anything is exactly the kind of
//! mistake that cannot be taken back.

/// What a user answers a prompt with.
///
/// There is no catch-all variant: an answer otag does not understand must be
/// refused, not guessed at, because the guess would be typed into a running
/// agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// Let the agent do it, this once.
    Allow,
    /// Do not.
    Deny,
}

impl Answer {
    /// Whether this answer lets the agent carry on.
    #[must_use]
    pub const fn allows(self) -> bool {
        matches!(self, Self::Allow)
    }
}

impl std::fmt::Display for Answer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        })
    }
}

/// `Esc`, which both agents' prompts name as the way to refuse.
pub const ESCAPE: &[u8] = b"\x1b";

/// The first option of a prompt, followed by a return.
///
/// The prompts are lists whose first entry is the plain yes. Typing the
/// number picks it; the return confirms it for a prompt that waits for one,
/// and lands harmlessly in an empty input box for a prompt that does not.
pub const FIRST_OPTION: &[u8] = b"1\r";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_says_what_it_is() {
        assert_eq!(Answer::Allow.to_string(), "allow");
        assert_eq!(Answer::Deny.to_string(), "deny");
        assert!(Answer::Allow.allows());
        assert!(!Answer::Deny.allows());
    }
}
