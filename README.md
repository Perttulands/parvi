# Parvi

Where you run your agents: terminals, files, work, brain, host and missions in
one Rust binary, with a native app for you and a CLI for your agent.

`VISION.md` says what it is for. `ARCHITECTURE.md` says how it is built.

To run it on Linux or WSL, build `cargo build --release -p parvi -p parvi-app`,
then `target/release/parvi install --app target/release/parvi-app` makes the
node the systemd user unit `parvi`, and a later build goes in with
`target/release/parvi upgrade --app target/release/parvi-app`; terminal
sessions live on through both.
