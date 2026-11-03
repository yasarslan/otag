# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Handoff. `Ctrl-b g`, or `otag handoff <pane>`, writes everything the next
  agent needs — what the user asked, where the last agent left off, the files
  it changed, the commands it ran, the last errors on screen and what `git`
  says about the checkout — into `.otag/handoff/<when>-<from>-to-<to>.md`,
  then opens the other agent beside the pane and tells it to read that file.
  Nothing asks the agent being handed from, so it works when that agent is
  out of quota. The old pane stays open.
- `Ctrl-b m`, or `otag handoff <pane> --model <name>`, keeps the agent and
  the conversation and continues the same session on another model, which is
  what to do when one model's quota is gone but the account's is not.
- The usage widget offers the handoff as soon as a window reads 100%.
- otag's own files are kept out of `git status` through
  `.git/info/exclude`, never by touching a tracked `.gitignore`.
- A notice line on the tab bar: what a handoff did, or why otag would not do
  something. A refused request no longer ends the session.
- A usage widget at the bottom of the sidebar: how much of each agent's
  five-hour and weekly windows is gone, when the window resets, and — once
  there are three readings — when it would be full at the rate of the last
  twenty minutes. It turns amber at 70% and red at 90%, and nothing blinks.
- `otag usage` prints the same numbers, and `otag usage --json` prints them
  for a script or an agent to read, with what each pane has spent.
- The numbers come from the agents themselves: Claude Code reports them
  through otag's status line, Codex in its session files. An agent that
  reports no percentage is shown as a token count, marked `~`, because no
  one knows what such an account's allowance is.
- The usage history survives a restart, so a projection is available again
  immediately rather than after three more readings.
- otag notices the agents in its panes. A pane running Claude Code or Codex
  gets the agent's name on its border, and a mark saying whether it is
  working, done, looping, failing or waiting for permission — in the pane's
  border, in the tab bar and in the sidebar, so a workspace shows the pane
  that most wants you without opening it.
- `otag install-hooks` registers otag with Claude Code: a command on the
  events otag needs, and a status line. It says what it would change, asks
  before changing it, keeps a copy of the file as it was, and keeps your own
  status line working by running it and printing what it prints.
  `otag uninstall-hooks` takes otag's entries out again and puts your status
  line back.
- `otag hook <event>` is what the agents call. It reports what the agent is
  doing and exits immediately, whether or not otag is running, so it can
  never hold an agent up.
- Agent sessions are remembered, so a pane that comes back after a restart
  continues its conversation instead of starting a new one.
- `otag ls` says which agent is in a pane, what it is doing, and which
  session it would continue.
- Sessions survive the daemon. otag saves its workspaces, tabs, layouts and
  what each pane has shown, and puts them back when it starts again: after a
  `otag kill-server`, a reboot, or a crash, the panes are running in their
  own directories again with what they showed above a `── restored ──` line.
- Closing the terminal still leaves everything running, and `otag` reattaches
  to it; stopping the daemon now ends an attached otag without reporting an
  error, because the session is over rather than broken.
- Panes, tabs and workspaces. `Ctrl-b %` splits the focused pane left and
  right, `Ctrl-b "` splits it top and bottom, and a pane leaves the layout
  when its program ends, giving its space back to its neighbour.
- `Ctrl-b c` adds a tab and `Ctrl-b n`, `Ctrl-b p` and `Ctrl-b 1`-`9` choose
  one; `Ctrl-b w` adds a workspace for the directory otag was started in and
  `Ctrl-b (`, `Ctrl-b )` step between them.
- `Ctrl-b h`/`j`/`k`/`l` or the arrow keys move the focus to the pane on that
  side, `Ctrl-b o` to the next one, and `Ctrl-b H`/`J`/`K`/`L` move the
  focused pane's edges.
- A sidebar listing the workspaces and a bar listing the open tabs, both of
  which a click can choose from, as can a click on a pane. `Ctrl-b z` hides
  the sidebar and gives its columns to the panes.
- Mouse events reach the program in the focused pane when it has asked for
  them, so scrolling and clicking inside a full-screen program still work.
- A calm theme: muted colours, and the focused pane marked by the colour of
  its border rather than by anything bright.
- `otag ls` now also reports each pane's workspace and what it is running.
- `otag` draws a pane in your terminal: a shell runs inside otag, keys reach
  it, and resizing the terminal resizes the program's view. Full-screen
  programs such as editors and system monitors work.
- The client also compares the terminal's size with the pane's once a second,
  so a resize otag was never told about is still picked up.
- `Ctrl-b d` detaches, leaving the pane and its program running; attaching
  again shows the pane as it was. `Ctrl-b Ctrl-b` sends a literal `Ctrl-b` to
  the pane.
- `otag attach` does what plain `otag` does, and `otag ls` lists the panes
  the daemon is running.
- `otag` starts a background daemon if none is running and reports whether it
  answers, so `otag ping` prints `pong`.
- `otag daemon` runs the daemon in the foreground, for debugging and for
  service managers.
- `otag kill-server` stops the daemon; `otag version` reports its state.
- A Unix socket, reachable only by the owning user, carrying newline-delimited
  JSON requests. `OTAG_SOCKET` and `--socket` choose its path.
- Daemon logging to a daily-rolling file in otag's state directory, filtered by
  `OTAG_LOG` or `--log`.
