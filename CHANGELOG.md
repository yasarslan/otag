# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `otag` starts a background daemon if none is running and reports whether it
  answers, so `otag` and `otag ping` both print `pong`.
- `otag daemon` runs the daemon in the foreground, for debugging and for
  service managers.
- `otag kill-server` stops the daemon; `otag ls` and `otag version` report its
  state.
- A Unix socket, reachable only by the owning user, carrying newline-delimited
  JSON requests. `OTAG_SOCKET` and `--socket` choose its path.
- Daemon logging to a daily-rolling file in otag's state directory, filtered by
  `OTAG_LOG` or `--log`.
