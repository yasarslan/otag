<p align="center">
  <img src="app/Resources/icon/otag-1024.png" width="88" height="88" alt="otag logo" />
</p>

<h1 align="center">otag</h1>
<p align="center"><strong>A local workspace for Claude Code and Codex CLI.</strong></p>
<p align="center">Run agents side by side, see which one needs your attention, and hand off work with its context.</p>

<p align="center">
  <a href="https://github.com/yasarslan/otag/releases/latest">
    <img src="https://img.shields.io/badge/Download-macOS%20·%20Apple%20silicon-111111?style=for-the-badge&logo=apple&logoColor=white" alt="Download for macOS">
  </a>
</p>

<p align="center">
  <a href="https://github.com/yasarslan/otag/releases/latest"><strong>Download</strong></a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="#under-the-hood">Under the hood</a> ·
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

## What you can do
- **Coordinate multiple agents:** Organize Claude Code and Codex into project-based workflows.
- **Delegate tasks:** Assign different roles and responsibilities across agents.
- **Switch seamlessly:** Transfer work between agents while preserving context.
- **Manage usage limits:** Monitor quotas and configure limit-aware handoffs.
- **Automate workflows:** Progress through team tasks and queued prompts with safety checks.
- **Keep your work organized:** Manage persistent sessions and isolated Git worktrees.

## Download

The macOS download supports Apple silicon Macs running macOS 14 or later. Install
and sign in to Claude Code, Codex, or both before starting a session.

1. Download the `.dmg` from [GitHub Releases](https://github.com/yasarslan/otag/releases/latest).
2. Open the disk image and drag **Otag** to **Applications**.
3. Try opening **Otag** from **Applications**. Because the app is not notarized,
   go to **System Settings → Privacy & Security → Open Anyway** after the first
   launch attempt, then confirm.

## Quick start

otag is **pre-alpha** and continues to evolve based on what comes up in day-to-day use.

To build from source and try agent tracking, install Git, a stable Rust toolchain (1.88 or newer), and at least one of the supported agent CLIs.

```sh
git clone https://github.com/yasarslan/otag.git
cd otag
```

### macOS app

Requires macOS 14 or newer and a Swift 6 toolchain (Xcode 16 or newer).

```sh
./scripts/bundle-app.sh
open target/Otag.app
```

Choose a project folder and start a Claude Code or Codex session. Use **Arrange** to see multiple sessions together, **Limits** to check reported usage, or **Switch agent** to hand off a task.

### Terminal client

On macOS or Linux, run this from the repository:

```sh
cargo install --locked --path crates/otag
otag
```

In an otag pane, run `claude` or `codex`. Press `Ctrl-b`, then `%` to split the pane, or `Ctrl-b`, then `d` to detach. Run `otag` again to reconnect. From a separate shell, run `otag ls` to list sessions or `otag usage` to see reported limits.

For Claude Code usage reporting, run `otag install-hooks` and review the proposed settings changes. The macOS app also offers this setup on first launch.

## Under the hood

A Rust daemon owns the terminal processes and persists session state in SQLite. The Ratatui terminal client and SwiftUI macOS app talk to it over a local Unix socket. otag reads local agent events and session logs for status and usage.

| Start here | What you’ll find |
| --- | --- |
| [`crates/otag-core/`](crates/otag-core/) | Terminal processes, panes, workspaces, and the socket protocol |
| [`crates/otag-agents/`](crates/otag-agents/) | Claude Code and Codex CLI integrations |
| [`app/`](app/) | SwiftUI app and its daemon client |

[Architecture notes](ARCHITECTURE.md) · [Build and test guide](CONTRIBUTING.md) · [MIT license](LICENSE)
