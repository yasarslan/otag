# otag

A multiplexer for AI coding agents, with a terminal client and a native
macOS app.

otag runs several coding agents side by side in panes, tabs and workspaces,
and keeps track of what each one is doing, what it has spent, and where it
left off. It shows which agent is waiting for you, how much of your usage
limits is gone and when it will run out, and it can hand a task from one
agent to another when one of them cannot carry on.

> **Status: pre-alpha.** Everything here works, and nothing is stable: there
> are no released binaries yet, and the socket protocol, the key bindings and
> the file formats may change between commits.

## What it does

- **Panes, tabs and workspaces.** Splits, a sidebar of workspaces, mouse
  focus, and sessions that keep running when you close the terminal.
- **Survives a restart.** Layout, working directories and what every pane had
  on screen come back after a reboot or a daemon restart — and a pane that was
  running an agent resumes that agent's session rather than starting over.
- **Says what each agent is doing.** `working`, `done`, `needs approval`,
  `error` or `looping`, on the pane's border, in the tab bar and in the
  sidebar, so one glance covers every workspace.
- **Live usage.** How much of each agent's five-hour and weekly windows is
  used, when they reset, and when they would be full at the rate of the last
  twenty minutes.
- **Handoff.** One key writes everything the next agent needs into a file and
  opens that agent beside the pane, or continues the same conversation on a
  lighter model.
- **A desktop notification** the moment an agent starts waiting for you —
  once per agent, not once a second.
- **A native macOS app** on top of the same daemon: one list of sessions, up
  to three of them side by side, Allow and Deny on the row that asked, the
  usage forecast, three terminal themes, and a night shift that works
  through a queue of prompts while you sleep. See [the app](#the-macos-app).

Supported agents: Claude Code and Codex CLI. otag reads the files they
already write; it never uploads anything, and it never reads credentials.

## Requirements

- macOS or Linux.
- A stable Rust toolchain, 1.88 or newer, to build it.
- For the macOS app: macOS 14 or newer and Swift 5.9 or newer (Xcode 15+).
- `git`, for the handoff package's summary of your checkout (optional).
- At least one of the supported agents, if you want otag to track one.

## Install from source

```sh
git clone https://github.com/yasarslan/otag
cd otag
cargo install --path crates/otag
```

That puts `otag` in `~/.cargo/bin`. To build without installing, run
`cargo build --release`; the binary lands in `target/release/otag`.

## Quick start

```sh
cd your-project
otag                      # opens a pane in this directory
```

Start an agent in the pane the way you normally would (`claude`, `codex`).
otag notices it within a second, names the pane after it, and starts tracking
what it is doing.

For usage numbers and immediate status — rather than status guessed from the
screen — let otag register itself with Claude Code:

```sh
otag install-hooks        # says what it will change, then asks
```

It adds one command per event it needs, takes over the status line (and keeps
running yours), asks for the status line to be re-run every 30 seconds so a
session that is sitting idle keeps reporting the account's limits, and leaves a
copy of the file as it was. An interval you chose yourself is left alone.
`otag uninstall-hooks` puts everything back.

The limits are account-wide, so otag counts what every Claude Code session on
the machine reports, including sessions you started in your own terminal.
Context and what each session spent are shown only for sessions running in an
otag pane, because that is what otag can attribute.

## The macOS app

The app is a client of the same daemon the terminal client talks to: the
daemon owns the panes and the agents, and both ways of looking at them see
the same thing at the same time. Quitting the app leaves every agent
running.

```sh
./scripts/bundle-app.sh     # builds the daemon and the app
open target/Otag.app
```

The first launch is three steps: what otag does and which agents it can
find, an offer to install Claude Code's hooks — showing exactly what it
would change, and doing nothing until you press the button — and your first
session.

There is one thing to learn: **sessions**. The sidebar lists them, and the
main area shows the one you picked, or two or three side by side.

- **The sidebar is the only navigator.** Sessions grouped by project, each
  with a colour for its agent, a name taken from its first prompt, and one
  word for what it is doing: Working, Needs you, Done or Idle. Click a
  session to see it, `⌘1`–`⌘9` to jump to one, double-click or right-click to
  rename it.
- **A question opens the row that asked it.** When an agent wants
  permission, its own row expands with the question and **Allow** / **Deny**
  (`⇧⌘Y` / `⇧⌘N`); the same two buttons are on the notification and in the
  menu bar. otag types only the keys the agent's own prompt offers, then
  checks the prompt is gone — an answer it cannot confirm says so rather than
  pretending.
- **One bar above a session**: its agent, project and status on the left,
  then a single primary button — **Switch to Codex** / **Switch to Claude**,
  which hands the work over with a package the other agent can read, and
  stands out once the current one is nearly out of room — and a single `⋯`
  menu for everything else: split, a lighter model, the night shift, rename,
  the morning report, close.
- **Sessions side by side.** Drag one out of the sidebar onto the left, the
  right or the bottom of the main area, or use `⋯ > Split with…`, `⌘D` or
  `⇧⌘D`. Up to three at once. Dividers drag, `⌘[` and `⌘]` move the keyboard,
  `⌘W` closes a session, and the arrangement is still there after a restart.
- **Limits in one card** at the foot of the sidebar: what each agent last
  reported and when it resets. Click it for the detail — a timeline of every
  reading, this session's context, and what each session has spent.
- **Night shift.** Give a session a queue of prompts: otag sends the next one
  only while the agent is idle and its window has room, stops at a threshold
  you choose, carries on when the window resets, and never types past a
  question. `⇧⌘R` shows what it did, with the commits that came of it.
- **One terminal, three themes.** Every session uses the same font, line
  height, paper and palette, whatever is running in it — a Claude pane and a
  Codex pane are the same terminal. Settings → Appearance offers **Glass
  Light**, **Glass Dark** and **Midnight** with a live preview, and follows
  your system appearance unless you choose. Each theme's colours are checked
  against its own background, so an agent's diffs and prompts stay readable.
  Every pane also answers an agent's "what colour are you?" query honestly,
  so an agent on its own auto theme follows otag's with nothing configured.
- `⌘K` is every session and command by name, and the menu bar item shows the
  limits and who is waiting even with the window closed.

The app is not signed with a developer identity yet, so macOS may need
"Open Anyway" in System Settings → Privacy & Security the first time, and
notifications may be refused for an unsigned build (the Dock badge and the
menu bar item still work).

## Keys

Every key is pressed after the prefix, `Ctrl-b`.

| Key | What it does |
|---|---|
| `%` | Split the focused pane left and right |
| `"` | Split it top and bottom |
| `h` `j` `k` `l` or arrows | Move the focus to the pane on that side |
| `o` or `Tab` | Focus the next pane |
| `H` `J` `K` `L` | Move the focused pane's edge |
| `c` | New tab |
| `n` `p` | Next, previous tab |
| `1`–`9` | Go to that tab |
| `w` | New workspace, for the directory otag was started in |
| `(` `)` | Previous, next workspace |
| `z` | Show or hide the sidebar |
| `g` | Hand the focused pane's work to the other agent |
| `m` | Continue the focused pane's session on another model |
| `d` | Detach: everything keeps running |
| `Ctrl-b` | Send `Ctrl-b` to the program in the pane |

A pane closes when the program in it ends. Clicking a pane, a tab or a
workspace selects it.

## Commands

```sh
otag                      # draw a pane in this terminal, starting the daemon
otag ls                   # workspaces, panes, the agent in each and its status
otag usage                # how much of each agent's limits is used
otag usage --json         # the same numbers for a script to read
otag handoff <pane>       # hand that pane's work to the other agent
otag handoff <pane> --model <name>
                          # keep the agent and the session, change the model
otag install-hooks        # register otag with Claude Code (asks first)
otag uninstall-hooks      # take it out again
otag ping                 # check the daemon, starting it if needed
otag version              # this binary's version, and the daemon's
otag kill-server          # stop the daemon and its panes
otag daemon               # run the daemon in the foreground, for debugging
```

## Configuration

There is no configuration file yet. These environment variables work today:

| Variable | What it does |
|---|---|
| `OTAG_SOCKET` | Path of the daemon's socket |
| `OTAG_LOG` | Log filter, for example `debug` or `otag_core=trace` |
| `OTAG_NO_NOTIFY` | Set it to turn desktop notifications off |

The daemon listens on a Unix socket only your user account can reach. Its
socket, its state database and its log live under
`~/Library/Application Support/otag/` on macOS, and under `$XDG_STATE_HOME`
(or `~/.local/state/otag/`) on Linux, with the socket in `$XDG_RUNTIME_DIR`
where there is one.

Handoff packages are written to `.otag/handoff/` inside the project they
belong to, and a worktree the app makes for a session goes to
`.otag/worktrees/<name>` on a branch called `otag/<name>`. otag adds
`/.otag/` to that repository's `.git/info/exclude` so none of it shows up as
untracked work. Your `.gitignore` is not touched.

The app keeps its own few settings — the terminal's font size, whether idle
sessions dim, whether it notifies — in its preferences, under
**Otag → Settings** (`⌘,`).

## Privacy

otag reads the session files the agents write, on your machine only. It
uploads nothing, sends nothing to any service, and never reads or logs API
keys or credentials. The one file outside its own directories it ever writes
is an agent's settings file, and only when you run `otag install-hooks`,
which shows what it will change, asks first, and keeps a backup.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). By participating you agree to the
[Code of Conduct](CODE_OF_CONDUCT.md). To report a security issue, follow
[SECURITY.md](SECURITY.md) rather than opening an issue.

## License

MIT. See [LICENSE](LICENSE).
