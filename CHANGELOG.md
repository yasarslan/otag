# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `otag` draws a pane in your terminal: a shell runs inside otag, keys reach
  it, and resizing the terminal resizes the program's view. Full-screen
  programs such as editors and system monitors work.
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
