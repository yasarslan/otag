# otag

A terminal-native multiplexer for AI coding agents.

otag runs many coding agents side by side in panes and workspaces, and keeps
track of what they cost and where they left off: live usage and limit
information, sessions that survive a reboot, and a single place to answer every
agent's permission prompts.

> **Status: pre-alpha.** The daemon, the socket protocol and the command-line
> interface exist; panes, the user interface and the agent integrations do not
> yet. Nothing is stable, and there are no released binaries. Expect breaking
> changes between commits.

## Build from source

Requires a stable Rust toolchain (1.88 or newer) on macOS or Linux.

```sh
git clone https://github.com/yasarslan/otag
cd otag
cargo build --release
```

The binary lands in `target/release/otag`.

## Usage

```sh
otag               # start the daemon if it is not running, then check on it
otag ping          # same, printing `pong` when the daemon answers
otag ls            # list workspaces and panes
otag version       # version of this binary and of the running daemon
otag kill-server   # stop the daemon
otag daemon        # run the daemon in the foreground, for debugging
```

The daemon listens on a Unix socket that only your user account can reach.
`OTAG_SOCKET` or `--socket` chooses its path; by default it lives under
`~/Library/Application Support/otag/` on macOS and in `$XDG_RUNTIME_DIR/otag/`
on Linux. `OTAG_LOG` or `--log` sets the log filter; the daemon writes its log
next to its state.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). By participating you agree to the
[Code of Conduct](CODE_OF_CONDUCT.md). To report a security issue, follow
[SECURITY.md](SECURITY.md) rather than opening an issue.

## License

MIT. See [LICENSE](LICENSE).
