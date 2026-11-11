# Parvi

Parvi is the operator's agent workspace: terminals, files, work, brain, host and
missions in one Rust binary, used through a native app and the `parvi` CLI.
`VISION.md` says what it is for and `ARCHITECTURE.md` how it is built.

## The operator works on this machine while you do

- The operator's tmux sessions survive everything you do. Tests start their own
  tmux servers and Parvi nodes through `kit/testkit` and act only on those,
  never on the real fleet or the real Tailscale. A test needs only Rust and
  tmux: any other program (`bd`, `gh`, a harness) is a fake the test writes.
- Other agents run at the same time. Pick unique scratch names and ports.
- Build and test the crates you changed, through `heavy run`. CI runs the whole
  workspace. Chase a flake by reading the code and writing a deterministic
  test, not by rerunning.

## Code

- Work inside your area's directory. If a change seems to need another area's
  internals or a new central list, the design is wrong: stop and say so.
- In the native app every visual value is a token, and stores stay apart from
  views.
- Land a rename or reshape whole: no shims, aliases or dead code.
- Errors carry a code and a message in the one shape `kit/core` defines.

## Checks

Before pushing: `cargo fmt --check`, then `cargo clippy -- -D warnings` and
`PARVI_REQUIRE_TMUX=1 cargo test` for the crates you changed (`-p <crate>`).
If `tmux` on `PATH` is a wrapper that refuses `kill-server`, set `PARVI_TMUX` to
the real binary.
