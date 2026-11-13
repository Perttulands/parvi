# Parvi

Parvi is the operator's agent workspace: terminals, files, work, brain, host and
missions in one Rust binary, used through a native app and the `parvi` CLI.
`VISION.md` says what it is for, `DESIGN.md` what good looks like in the app and
`ARCHITECTURE.md` how it is built.

## The operator works on this machine while you do

- The operator's tmux sessions survive everything you do. Tests start their own
  tmux servers and Parvi nodes through `kit/testkit` and act only on those,
  never on the real fleet or the real Tailscale. A test needs only Rust and
  tmux: any other program (`bd`, `gh`, a harness) is a fake the test writes.
- Other agents run at the same time. Pick unique scratch names and ports.
- Build and test through `heavy run --memory <GiB>`.

## Owning a piece

You own an outcome, such as a page being good on its journeys, and keep going
until it is. Fix what you find where it lives, in any directory. Push to main
when the crates you changed pass the checks; CI runs the whole workspace, and a
red main is fixed forward by whoever broke it. Tell the other owners only when
you change a `contract` or `kit/` interface they use. File a bead only for work
you will not finish yourself.

For anything on screen, run the app against a node of your own on a private
display and look at the screenshot. A passing test does not show that a page
is good.

## Code

- `ARCHITECTURE.md` lists the boundaries the build holds. If a change needs to
  break one, the design is wrong: stop and say so.
- In the native app every visual value is a token, and stores stay apart from
  views.
- Land a rename or reshape whole: no shims, aliases or dead code.
- Errors carry a code and a message in the one shape `kit/core` defines.

## Checks

Before pushing: `cargo fmt --check`, then `cargo clippy -- -D warnings` and
`PARVI_REQUIRE_TMUX=1 cargo test` for the crates you changed (`-p <crate>`).
If `tmux` on `PATH` is a wrapper that refuses `kill-server`, set `PARVI_TMUX` to
the real binary. Write a test for behaviour that could break again, not for how
something looks. Chase a flake by reading the code and writing a deterministic
test, not by rerunning.
