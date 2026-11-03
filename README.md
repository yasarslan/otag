# otag

A terminal-native multiplexer for AI coding agents.

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

Supported agents: Claude Code and Codex CLI. otag reads the files they
already write; it never uploads anything, and it never reads credentials.

## Requirements

- macOS or Linux.
- A stable Rust toolchain, 1.88 or newer, to build it.
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
running yours), and leaves a copy of the file as it was. `otag uninstall-hooks`
puts everything back.

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
belong to, and otag adds `/.otag/` to that repository's `.git/info/exclude`
so they never show up as untracked work. Your `.gitignore` is not touched.

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
