//! What the agents are waiting for the user to answer.
//!
//! An agent that needs permission stops and asks on its own screen. With
//! several agents running, finding which one is asking — and what it is
//! asking — costs more attention than the answer itself. The inbox collects
//! the questions so they can be answered from one place, without looking at
//! the pane.
//!
//! The rules are deliberately small:
//!
//! - one pane has at most one unanswered question, because an agent that is
//!   waiting is not also asking something else;
//! - a newer question from the same pane replaces the older one, since the
//!   agent has clearly moved on;
//! - answering takes the question out of the inbox, and so does the agent
//!   carrying on by itself.
//!
//! A pane is named by its number rather than by `otag-core`'s `PaneId`: this
//! crate holds the reasoning, not the terminal plumbing, and does not depend
//! on the crate that runs the panes. What an answer *is*, and what has to be
//! typed to give it, belongs to the agent and lives in
//! [`otag_agents::approval`].

use std::time::SystemTime;

use otag_agents::AgentKind;

/// Identifier of one question, unique within a daemon run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AskId(u64);

impl AskId {
    /// Identifier of the first question a daemon collects.
    pub const FIRST: Self = Self(1);

    /// The identifier after this one.
    ///
    /// Saturates rather than wrapping, so an identifier is never reused and
    /// an answer can never reach a different question than the one it was
    /// given for.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    /// The identifier as a number, for the socket protocol.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The identifier with this number, as it arrives from a client.
    #[must_use]
    pub const fn from_number(number: u64) -> Self {
        Self(number)
    }
}

impl std::fmt::Display for AskId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// One question waiting for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// Which question this is.
    pub id: AskId,
    /// The pane the agent is running in, as its number.
    pub pane: u64,
    /// The agent asking, when otag knows which one it is.
    pub agent: Option<AgentKind>,
    /// The tool the agent wants to use, in the agent's own words.
    pub tool: Option<String>,
    /// One line saying what is waiting, for a list.
    pub summary: String,
    /// When the question arrived.
    pub asked_at: SystemTime,
}

/// Everything waiting for the user, oldest first.
#[derive(Debug, Clone)]
pub struct Inbox {
    next: AskId,
    asks: Vec<Ask>,
}

impl Default for Inbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Inbox {
    /// An empty inbox.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next: AskId::FIRST,
            asks: Vec::new(),
        }
    }

    /// Record that a pane's agent is asking something, and say which
    /// question that became.
    ///
    /// Anything that pane was already asking is dropped: an agent waiting
    /// for an answer cannot be waiting for a second one, so the newer
    /// question is the true one.
    pub fn ask(
        &mut self,
        pane: u64,
        agent: Option<AgentKind>,
        tool: Option<String>,
        summary: impl Into<String>,
    ) -> AskId {
        let id = self.next;
        self.next = id.next();
        self.asks.retain(|ask| ask.pane != pane);
        self.asks.push(Ask {
            id,
            pane,
            agent,
            tool,
            summary: summary.into(),
            asked_at: SystemTime::now(),
        });
        id
    }

    /// Everything waiting, oldest first.
    #[must_use]
    pub fn asks(&self) -> &[Ask] {
        &self.asks
    }

    /// How many questions are waiting.
    #[must_use]
    pub fn len(&self) -> usize {
        self.asks.len()
    }

    /// Whether nothing is waiting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.asks.is_empty()
    }

    /// The question with this identifier.
    #[must_use]
    pub fn get(&self, id: AskId) -> Option<&Ask> {
        self.asks.iter().find(|ask| ask.id == id)
    }

    /// What a pane is asking, if it is asking anything.
    #[must_use]
    pub fn of_pane(&self, pane: u64) -> Option<&Ask> {
        self.asks.iter().find(|ask| ask.pane == pane)
    }

    /// Take a question out, because it is being answered.
    ///
    /// The question leaves the inbox before the answer is delivered: a
    /// question answered twice would answer the agent's *next* prompt as
    /// well, which is exactly what must not happen.
    pub fn take(&mut self, id: AskId) -> Option<Ask> {
        let at = self.asks.iter().position(|ask| ask.id == id)?;
        Some(self.asks.remove(at))
    }

    /// Forget whatever a pane was asking, because it is no longer waiting —
    /// the user answered in the pane itself, or the pane is gone.
    ///
    /// Returns whether anything was waiting.
    pub fn clear_pane(&mut self, pane: u64) -> bool {
        let before = self.asks.len();
        self.asks.retain(|ask| ask.pane != pane);
        self.asks.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_inbox_is_empty() {
        let inbox = Inbox::new();
        assert!(inbox.is_empty());
        assert_eq!(inbox.len(), 0);
        assert!(inbox.asks().is_empty());
    }

    #[test]
    fn a_question_can_be_found_by_its_identifier_and_by_its_pane() {
        let mut inbox = Inbox::new();
        let id = inbox.ask(
            7,
            Some(AgentKind::Claude),
            Some("Bash".to_owned()),
            "Bash: ls",
        );

        let ask = inbox.get(id).expect("the question is there");
        assert_eq!(ask.pane, 7);
        assert_eq!(ask.agent, Some(AgentKind::Claude));
        assert_eq!(ask.tool.as_deref(), Some("Bash"));
        assert_eq!(ask.summary, "Bash: ls");
        assert_eq!(inbox.of_pane(7).map(|ask| ask.id), Some(id));
        assert_eq!(inbox.of_pane(8), None);
    }

    #[test]
    fn two_panes_wait_side_by_side() {
        let mut inbox = Inbox::new();
        let first = inbox.ask(1, None, None, "one");
        let second = inbox.ask(2, None, None, "two");

        assert_eq!(inbox.len(), 2);
        assert_eq!(
            inbox.asks().iter().map(|ask| ask.id).collect::<Vec<_>>(),
            vec![first, second],
            "the oldest question comes first"
        );
    }

    #[test]
    fn a_newer_question_from_one_pane_replaces_the_older_one() {
        // An agent that is waiting is not also asking something else.
        let mut inbox = Inbox::new();
        let first = inbox.ask(1, None, None, "old");
        let second = inbox.ask(1, None, None, "new");

        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox.get(first), None);
        assert_eq!(
            inbox.get(second).map(|ask| ask.summary.as_str()),
            Some("new")
        );
    }

    #[test]
    fn answering_takes_the_question_out() {
        let mut inbox = Inbox::new();
        let id = inbox.ask(1, None, None, "one");

        assert_eq!(inbox.take(id).map(|ask| ask.id), Some(id));
        assert!(inbox.is_empty());
        assert_eq!(
            inbox.take(id),
            None,
            "a question cannot be answered a second time"
        );
    }

    #[test]
    fn an_identifier_is_never_reused() {
        let mut inbox = Inbox::new();
        let first = inbox.ask(1, None, None, "one");
        inbox.take(first);
        let second = inbox.ask(1, None, None, "one again");

        assert_ne!(first, second);
    }

    #[test]
    fn a_pane_that_carries_on_by_itself_is_cleared() {
        let mut inbox = Inbox::new();
        inbox.ask(1, None, None, "one");
        inbox.ask(2, None, None, "two");

        assert!(inbox.clear_pane(1));
        assert_eq!(inbox.len(), 1);
        assert!(
            !inbox.clear_pane(1),
            "a pane that was not waiting changes nothing"
        );
    }

    #[test]
    fn an_identifier_survives_the_trip_through_the_protocol() {
        let id = AskId::FIRST.next().next();
        assert_eq!(AskId::from_number(id.get()), id);
        assert_eq!(id.to_string(), "3");
    }
}
