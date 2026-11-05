# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Handoff sessions inherit their source session's name with the receiving
  agent in parentheses, and keep its nickname, instead of being named after
  otag's instruction to read the handoff package.
- Choosing a session in the app's sidebar now shows that session. It changed
  which session had the keyboard but went on drawing the previous one's
  terminal: a terminal view holds one session's byte stream, and when the
  window replaced one session with another the view was reused rather than
  rebuilt, so the stream stayed attached to the session that had gone.
- A session's header no longer says the same thing twice. A session named
  after its folder repeated that folder's name and left out the separators
  between the three things it says; it now reads `name · project · status`,
  with the project left out when it is already what the session is called.
- `Otag.app` shipped with **no strings at all**: SwiftPM puts a target's
  resources in a bundle beside the binary, and `scripts/bundle-app.sh` never
  copied it, so every label in a bundled build would have shown its own key.
  Nested bundles are now copied and signed before the app itself, so the
  outer signature verifies.

### Added

- **A toolbar of five buttons**, one job each: Arrange, Limit guard, Switch
  agent, Language, New session. One size, one style, each with its whole
  sentence as a tooltip and as an accessibility label; a button that would do
  nothing is disabled rather than hidden.
- **Arrange** lays out the sessions of one project evenly: one fills the
  window, two are halves, three is one half beside two stacked quarters (three
  columns of a terminal are too narrow to read), four is a 2×2 grid, and more
  than four shows the four most recently used. Press it again for the single
  session you had. The window now shows up to four sessions rather than three.
- **Nicknames.** A word for the job a session is doing — "backend", "tests" —
  shown under its name in the sidebar and edited in the row itself. The daemon
  keeps it, so it survives a restart and every client sees the same one.
- **Limit guard.** Watch a session's limits and choose what happens when they
  run out: wait, type a resume prompt once the window resets, or hand the work
  to the other agent — which falls back to waiting when that agent is over the
  same threshold. Several sessions can be guarded at once, each with its own
  threshold. The daemon owns it, so it keeps working with every window closed.
  It stops a session **after its current turn**: neither agent can be
  suspended — a hook can only deny a tool call, which would lie to the agent,
  and an interrupt throws away the turn — so otag never interrupts work in
  progress and never answers an agent for you. A shield on the row and the
  header says which sessions are guarded and which have been stopped.
- **Switch agent** is one popover with both honest answers: hand the work to
  the other agent with its package, or change the model and keep the
  conversation. The model lists are what each tool accepts rather than a
  remembered list — Claude Code's documented `--model` aliases, and for Codex
  the list its own client keeps in `~/.codex/models_cache.json` — with a
  free-text field so neither list can go stale and strand a model.
- **English, Turkish and Finnish**, from the toolbar or Settings, applied at
  once with no restart. Every user-visible string is in one place and nothing
  else in the app holds one; dates and numbers are formatted for the chosen
  language. An agent's own terminal output is never translated. A test checks
  that no key is missing from any language, that placeholders match so a
  translation cannot crash a format, and that every key the app asks for
  exists.
- `set_nickname` and `guard` on the socket, and `nickname` and `guard` on
  every pane the daemon reports.

- **Otag.app**, a native macOS window onto the same daemon: it starts the
  daemon if none is running and lists every session, grouped by project, with
  what each one is doing. Build it with `scripts/bundle-app.sh`. The terminal
  client is unchanged and stays the way to use otag on Linux and over SSH.
- The app has one thing to learn: **sessions**. The sidebar lists them and
  the main area shows the one you picked, or up to three side by side. There
  is no tab bar and no second navigator.
- The app shows the sessions themselves: a real terminal per session with
  colours, scrollback, mouse reporting, copy and paste, resized by the
  window rather than by otag's own layout. Drag a session from the sidebar
  onto the left, the right or the bottom of the main area, or use
  `⋯ > Split with…`, `Cmd-D` or `Cmd-Shift-D`. Dividers drag, `Cmd-[` and
  `Cmd-]` move the keyboard, `Cmd-W` closes a session, and `Cmd-1`–`Cmd-9`
  jump to the first nine. A fourth session closes the one you had not looked
  at for longest and says so.
- Every session is called something you recognise: a name taken from the
  first thing it was asked, editable from `⋯ > Rename…` or a right click.
  The daemon carries that prompt — from the one a client started the session
  with, or from the agent's own transcript for a session otag did not start —
  so a session started from the terminal client is named too. A session
  nobody has asked anything yet is its agent and a number ("Claude 1",
  "Codex 2"), never the folder: every session in one project used to be
  named after that project, which is the same as having no names at all.
- An agent writes its own injected context into its transcript as a message
  from the user — every Codex session opens with an `<environment_context>`
  block — so otag skips those and names a session after the first thing a
  person actually typed.
- `first_prompt` on every pane the daemon reports, and `terminal_appearance`
  on the socket: a client says whether it draws its panes on dark or light
  paper, and otag sets `COLORFGBG` in every pane it starts from then on. A
  program that reads that variable rather than asking the terminal is
  therefore told the truth from its first line. A pane already running is
  left alone, because a process's environment cannot be changed from
  outside it.
- One bar above a session, and it is only a label:
  `nickname · name · project · status`, with a close button and nothing else.
  Everything you can do to a session is a toolbar button or a right click.
- A new-session sheet (`Cmd-N`): a project folder, Claude Code / Codex /
  shell, an optional model, an optional first prompt, and an optional git
  worktree of the session's own so that two agents cannot collide in one
  checkout. A worktree that cannot be made stops the session rather than
  quietly sharing the checkout.
- `Cmd-K` is every session and every command by name: one field, one list,
  no modes, and the shortcut shown beside each so the palette teaches the
  menus rather than replacing them. Nothing is only there.
- A welcome screen on the first launch, in three steps: what otag does and
  which agents it can find, exactly what installing Claude Code's hooks
  would change — installed only when you press the button — and your first
  session.
- An empty state that explains the window rather than apologising for being
  empty: one button and three lines naming the sidebar, the switch button
  and dragging to split.
- A settings window (`Cmd-,`) with the few things worth setting: the
  terminal theme, its font size, whether idle sessions dim, and whether the
  app notifies.
- **Three terminal themes**, the same in every pane: **Glass Light**, **Glass
  Dark** and **Midnight** (deep navy, high contrast), chosen in Settings →
  Appearance with a live preview, and following the system appearance unless
  you choose one. Changing the theme reaches every open session at once.
  Each theme sets the font, line height, paper, cursor, selection and all
  sixteen ANSI colours, so a Claude pane and a Codex pane are the same
  terminal. Every colour that carries meaning is checked against its own
  background — the agents' diff blocks and highlighted prompts stay readable
  in all three.
- Every pane answers an agent's `OSC 10` and `OSC 11` queries with the active
  theme's colours, so an agent on its own auto theme follows otag's theme
  with nothing configured and nothing written anywhere.
- Settings can also offer, once and only when you press the button, to match
  the agents' own theme settings: Claude Code's `theme` and Codex's
  `[tui] theme`. It shows the exact lines it would write, names both files,
  keeps a timestamped backup of each, changes one key per file, keeps a
  `-daltonized` or `-ansi` variant rather than replacing it with the plain
  one, and never touches a custom theme.
- Liquid Glass chrome on macOS 26 — the sidebar, the header bar, the cards and
  the popovers — falling back to materials on macOS 14 and 15. Terminals stay
  opaque: text an agent drew never has the desktop showing through it.
- A session doing nothing, beside others, is drawn slightly dimmer, so the
  eye goes to what is working. Nothing blinks and nothing moves.
- An app icon, drawn by `scripts/make-icon.swift` at build time rather than
  shipped as a binary asset.
- **Night shift**: give a session a queue of prompts and otag works through
  it for you. It sends the next one only while the agent is idle and its
  window has room, stops at a threshold you choose (90% by default), carries
  on when the window resets, and never types past a question or an error.
  Every prompt is pasted and submitted the way a person would, and written
  down as it happens.
- A morning report (`Cmd-Shift-R`): per session, how many prompts otag sent,
  what it did and when, the commits made in that session's directory since
  the night started, and what the session spent.
- Handoff from the app: one button hands the work to the other agent with a
  package it can read, another continues the same conversation on a
  different model. Both report what happened in a line across the bottom of
  the window, and every client is told, not only the one that asked.
- `night_shift`, `morning_report` and `handoff` on the socket, and every
  pane now says what the night shift is doing for it.
- The app shows how much of the limits is gone in one card at the foot of
  the sidebar: a thin meter per window with the percentage, when the window
  resets, and whether otag is talking to the daemon. Clicking it opens a
  popover with the same numbers, a timeline chart of every reading otag has
  kept, this session's context and what each session has spent — there is no
  separate inspector panel.
- A context meter for the session with the keyboard, drawn only when otag
  knows how large that model's context is: a share of an unknown total would
  be a guess.
- `usage_history` on the socket: the readings behind the numbers, which the
  daemon already keeps across restarts, so a chart can be drawn the moment a
  client connects.
- The app finds you when its window does not: a session waiting for
  permission opens up in the sidebar under its own name, with the question
  and **Allow** and **Deny** under it
  (`Cmd-Shift-Y` / `Cmd-Shift-N` answer the oldest), the Dock icon
  carries the number of sessions waiting, a notification with the same two
  buttons goes out the moment one starts waiting, and a menu bar item shows
  the limits and who is waiting even with the window closed.
- An answer otag could not deliver says so instead of pretending: otag types
  the keys the agent's own prompt offers, waits for the prompt to go away,
  and tells you to look at the session if it has not.
- otag notices a question whether or not the agent has hooks: a pane whose
  screen is asking gets an entry in the inbox, read off that screen, and the
  entry goes again the moment the pane moves on.
- The daemon's own desktop notifications stand down while the app is
  connected, so one question is one notification.
- `start_session` on the socket, which is what that sheet sends: any client
  can now start a session with an agent, a model, a first prompt and a
  worktree in one request.

- A second way to watch a pane, for clients that run a terminal emulator of
  their own: `attach_pane` answers with the escape sequences that recreate
  the pane's screen and then forwards the program's output unchanged, and
  sends the screen again rather than a stream with a hole in it when a client
  falls behind or the pane is resized.
- One subscription that pushes everything a client needs to know — panes and
  their statuses, usage, questions from the agents, handoffs — instead of
  polling for it.
- An inbox of what the agents are waiting for, and answering one of those
  questions without going to its pane. otag types only the keys the agent's
  own prompt offers, only while that prompt is on screen, and then checks
  that it is gone: an answer it cannot confirm is reported as not delivered
  rather than assumed.
- Bytes on the socket now travel as base64, which is a third of the size of
  the list of numbers they used to be. A list of numbers is still read.
- A desktop notification the moment an agent starts waiting for you: once
  per agent per situation, never once a second, and silent on a system that
  has no way to show one. `OTAG_NO_NOTIFY` turns them off.
- A README written for people using otag rather than building it: what it
  does, what it needs, how to install it, a quick start, the whole key
  table, the commands, the environment variables and what otag reads.
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

### Added

- A refresh button beside the usage numbers — in the limits card's popover
  and in the menu bar item — that asks the daemon for them again.
  Its tooltip says when the last reading arrived and which session otag
  heard it from, naming the project when it is a session otag is running.
  The app also re-reads them every minute while it is open.

### Added

- Sessions side by side, and findable without knowing a shortcut. Drag a
  session out of the sidebar and onto the left, right, top or bottom edge of
  the terminal area to put it there — the edges appear only while something
  is over them — or use the split buttons in a session's header, or `Cmd-D`
  and `Cmd-Shift-D`. `Cmd-[` and `Cmd-]` move the keyboard between the
  sessions sharing a tab, `Cmd-Shift-[` and `Cmd-Shift-]` still step between
  tabs. Dividers drag, the session with the keyboard keeps its thin ring,
  and a session already on screen is moved rather than shown twice.
- The arrangement survives a restart: quitting with three sessions side by
  side and reopening gives three sessions side by side, with the dividers
  where they were left and the same one holding the keyboard. The daemon
  remembers the sessions; how they were arranged is the one piece of state
  the app owns, so it is the one piece it writes down itself. A saved
  arrangement is checked against the sessions the daemon is actually running
  before anything is drawn, so one naming a session that is over comes back
  without it rather than as an empty terminal.

### Changed

- One terminal look for every session. A pane is a pane: the same font at the
  same size, the same line height, the same background and the same sixteen
  ANSI colours, whether Claude Code, Codex or a shell is drawing into it.
  Each pane used to take the emulator's built-in palette, which does not
  follow the system appearance, while its background did — so two agents
  that lean on those colours to very different degrees did not look like the
  same program. There is now one palette per appearance, chosen for contrast
  against the pane's own background and reinstalled when the appearance
  changes, and a pane answers an agent asking what its colours are with
  exactly those. otag does not write to any agent's configuration.

- The limits are shown as one line per window, everywhere — the limits card
  and its popover, the menu bar, the terminal client and `otag usage`: who,
  which window,
  what the agent last reported, and when that window resets as a time of day
  (`Claude 5h 69% resets 02:40`, `Claude week 34% resets 10 Nov 06:00`).
  Gone with it: the "full in" forecast, "updated X ago", the dimming of a
  reading nobody had confirmed recently, and every line of hint text. A
  reading the user can check against the agent itself is worth more than a
  reading with otag's own arithmetic beside it. The burn rate is still
  measured — the handoff and the night shift decide on it — and no longer
  shown. There is no supported way to ask Claude Code for the account's
  limits without spending tokens, so otag never does: the numbers come from
  what the sessions report.
- A window whose reset has passed reads 0% until a reading of the new one
  arrives, because the allowance is back and that is what zero means. A
  window otag has never had a reading of reads `–`. The menu bar shows the
  worst percentage any window last reported, with no cutoff for age: a
  number that vanishes on a timer reads as otag losing track.
- The app asks the daemon for the numbers again every 30 seconds rather than
  every minute, matching the interval the agents' status lines run on.
- `otag install-hooks` asks Claude Code to re-run the status line every 30
  seconds, so a session that is sitting idle keeps reporting the account's
  limits instead of going quiet until someone types. An interval already in
  the settings file is left as it is, and it survives otag taking the status
  line over and handing it back.

### Changed

- The frequent actions are where the sessions are, not only behind
  `Cmd-K`. Every session now has a thin header above its terminal with the
  handoff (named after where the work would go — "Hand Off to Codex"),
  continuing on a lighter model, splitting right and down, and closing; the
  same five are on a right click in the sidebar and in the palette, written
  once so they cannot drift apart. When a session's agent is at the 90% the
  night shift stops at, its handoff button says what it does and takes the
  accent colour instead of sitting there as a symbol.

### Fixed

- The new-session sheet starts where the user is: the project is filled in
  from the session on screen — or, with nothing on screen, from the newest
  one — and the agent is whichever was used last. Both can still be changed;
  this only means a second session in the project in front of you is one
  return key away rather than a trip through a file dialog. Sessions now
  report the directory they are working in (`cwd` on a pane), which is what
  made this possible: a workspace's name is a folder name, not a path.

- A Codex session no longer reads as "working" for the rest of its life. Its
  status came only from the words on its screen while it was busy, and a
  screen matching none of them left the status as it was — so a session that
  had answered and was waiting at its prompt was still listed as working.
  otag now reads the turn events Codex writes into its own rollout file,
  which is the reliable answer and needs no hooks, and also recognises the
  screen of a session waiting for a prompt. A question an agent puts on its
  screen is heard even moments after it reported for itself, so nothing
  about this can hide one.

- The line a handoff leaves across the bottom of the window goes by itself.
  "claude is taking over from codex, reading …" describes the session that
  is now in the window, so it has nothing left to say once that session is
  on screen — and it used to stay there for the rest of the day. It now goes
  as soon as the session it opened appears, and after eight seconds either
  way. The close button still works; it is just no longer the only way.

- A handoff asked for twice no longer opens two sessions on one task. A
  handoff takes a moment — a transcript to read, `git` to ask, a package to
  write — and the button that starts one sat there the whole time, so a
  second click opened a second agent. The daemon now refuses a second
  handoff from the same session while one is in flight, and for a short
  while after, saying why; the app's button goes quiet until the daemon has
  answered. A handoff that was refused leaves nothing behind and can be
  asked for again at once.

- A new session in a project that is already open joins it instead of making
  a second entry called "<project> 2". Sessions are matched by the project
  the directory belongs to rather than by its name: a trailing separator, a
  path that goes through a symbolic link, and a git worktree all find the
  entry the user already has, and a worktree session is listed under the
  repository it was cut from. Two different checkouts that happen to share a
  folder name are still told apart by the number, which is what it is for.

- A reading is no longer undone by a session that knows less. Readings arrive
  from every session on the machine, and an idle or resumed session re-runs
  its status line on a timer and reports the snapshot it was handed when it
  last answered — which can name a window that ended days ago. otag took
  whichever reading arrived last whenever the two named different resets, so
  one such session turned a current "5h 69%, resets 02:40" into "windows have
  reset" with no percentage at all. Readings are now ranked by the window
  they describe rather than by when they arrived: a later reset is a later
  window, and a reading of a window that is over can never replace one of the
  window running. A status line payload that carries no limits — from another
  session, from a resumed one before its first reply, from an account on an
  API key, or with its windows null — still changes nothing at all.

- The weekly forecast is measured over a day of readings rather than the
  last twenty minutes, so a busy afternoon no longer says the week will be
  gone by the evening — a weekly window a third full read "full in 4h38m".
  The five-hour window still follows the last twenty minutes, which is the
  sitting it is in. Neither measurement reaches back past the start of the
  window it is measuring, and the weekly window is projected only when otag
  knows when it resets and the projection lands before then.
- The refresh button is always there, not only when the numbers have gone
  stale, and it has a hit area rather than a ten-point glyph to aim at. It
  turns while otag is re-reading and says "Updated" when the answer lands,
  and it stops turning when the answer arrives — whether or not the numbers
  in it changed — or within five seconds if none does.
- A reading of a window that is still running no longer disappears because
  another window has reset, or because nobody has confirmed it for a while.
  One window being over said "waiting for fresh data" for the whole widget
  and hid a perfectly good number next to it.
- The app's terminals come back by themselves when the daemon is restarted.
  Each pane is picked up again — with its restored screen, at the size the
  view is drawing it — instead of showing "the otag daemon closed the
  connection" and staying dead. A pane the daemon no longer has says so over
  its last screen, with a button to close it, rather than quietly swallowing
  what is typed into it.
- Usage is read from every Claude Code session on the machine, not only from
  the ones running inside otag. A session in your own terminal reports the
  same account's limits, so its numbers now reach the meters too; what is
  attributed to a session — its context and what it spent — still needs the
  session to be running in an otag pane.
- The meters no longer fall behind the agent's own `/usage`. A session that
  has been idle for an hour keeps reporting the percentage it was last told,
  so the reading that arrived last is not the one that knows most: within a
  window otag now takes the highest reading, and a reading of a later window
  always replaces one of the window before it. Each window also says which
  session the number came from.
- A window that has already reset no longer shows the percentage it ended on
  as if it were current. Everywhere the numbers appear — the sidebar, the
  app, its limits popover and its menu bar item, and `otag usage` — such a
  window
  says it has reset and is waiting for fresh data, with no percentage and no
  forecast, until the agent reports the new window.
- Every reading now carries how old it is, so a number nobody has refreshed
  for a while is shown as `updated 3h ago` and drawn dimmed rather than as
  the state of things right now.
- otag no longer paints a background of its own. Everything it draws keeps
  the terminal's own background and foreground, so a light terminal no longer
  shows dark bands behind an agent's own interface.
- A reset more than a day away is written in days (`resets 6d 17h`) instead
  of as a count of hours nobody reads.
- The last thing a program prints always reaches the screen before otag
  reports that the program ended, so a crash message is no longer lost on
  the way out.
- Watching a pane whose program has just ended shows what it printed and
  then that it ended, instead of answering that the pane is gone. A program
  that prints one line and exits is no longer a blank window.
