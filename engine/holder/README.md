# parvi-holder

Parvi's Windows session holder: a detached per-user process that owns ConPTY
sessions, as tmux owns them on Linux. A Windows node attaches to it over a
named pipe, and sessions survive the node's restarts, upgrades and crashes.
They end when the holder does: at log off, at reboot, or on `kill-server`.
Design and measurements behind it: `docs/research/windows-sessions.md`.

## Interface

The crate is a program, `parvi-holder.exe`, and a client library:

- `Endpoint::for_user()` names this user's pipe, `\\.\pipe\parvi-holder-<SID>`.
  `Endpoint::scratch(name)` names a holder of its own for tests.
- `connect_or_start(endpoint, exe, log)` connects, first starting the holder
  hidden and detached if none is running. `connect` only connects.
- `Connection::command(words)` runs one command. `Connection::attach(target,
  flags)` attaches. After that the connection is a tmux control-mode client
  whose reply bodies are decoded before they reach the engine's control events.

`parvi-holder [--pipe <path>] [--log <path>]` serves until `kill-server`. It
logs to `%LOCALAPPDATA%\Parvi\holder.log` unless given `--log`.

## Protocol

Every connection opens with `parvi-holder <protocol>`. The holder answers
`parvi-holder <protocol> <version> <pid>`, or `%exit protocol <n> wanted,
holder speaks <m>` and hangs up. The protocol number changes only when the
wire format does, because a running holder outlives the node that started it.
Protocol 2 escapes a reply-body line beginning with `%` by prefixing another
`%`; both collected and streamed replies remove that prefix. Notifications and
reply markers are unchanged. A node refuses an older holder without stopping it:
its sessions remain alive, but need a node that speaks its protocol until they
end.

After the greeting the connection speaks tmux control mode: commands in,
replies between `%begin` and `%end`/`%error`, and `%output`,
`%layout-change`, `%sessions-changed`, `%pause`, `%continue` and `%exit`
out. The holder answers the subset that Parvi's engine, launcher and search
send: `attach-session`, `capture-pane` (`-e`, `-J`, `-S -`),
`display-message`, `has-session`, the guarded `if-shell -F` submit, `kill-server`, `kill-session`,
`list-clients`, `list-panes`, `list-sessions`, `new-session`, `paste-buffer`,
`refresh-client` (`-C`, `-f`, `-A`), `send-keys` (`-H`, `-l`, key names) and
`set-buffer`. Each session has one window with one pane. Pane ids are unique
for the holder's lifetime.

## Behaviour

- **One per user.** The pipe is created as the first instance, so a second
  holder on the same pipe exits. Only the owner and SYSTEM may open it, and
  remote clients are refused.
- **Hidden.** The program is Windows-subsystem, so it never has a console, and
  it is started detached with no inherited handles. ConPTY's console hosts are
  headless.
- **Flow control.** A pane's reader never waits for a client. Each client's
  queue is bounded at 1 MB. A client attached with `pause-after` has the pane
  paused (`%pause`) when it overfills or falls `pause-after` seconds behind,
  until `refresh-client -A '%<pane>:continue'`. Any other client is dropped
  with `%exit too far behind`. Replies and notifications are never paused;
  they get a second megabyte of room, and a client that lets them pile up past
  that is dropped too. A reply to a client that has read everything always
  goes, however big.
- **Bounded input.** Input goes through a writer thread per pane, so a program
  that stops reading blocks nobody else. Up to 1 MB may wait for it. Past
  that, `send-keys` and `paste-buffer` fail with `input refused`.
- **Bounded history.** Each pane keeps 2,000 lines of scrollback. Sizes are
  capped at 500x200, so history costs at most about 36 MB per pane and about
  8.6 MB at 120 columns, and an idle pane costs little.
- **Kill.** Each program runs in a job object. `kill-session` ends the job, so
  nothing it started is left behind, and the job ends with the holder.
- **Terminal queries.** In passthrough mode (Windows 11 22H2 and later) the
  console answers nothing, so the holder answers cursor position, device
  attributes and colour queries itself. It finds them in stream order, also
  when one is split across reads, and answers each with the cursor where that
  query left it.

## Checks

The holder runs on Windows. Its plain logic (handshake, flow control, screen,
words) builds and is tested by the workspace's `cargo test` on Linux. On a
Windows machine with WSL, `crates/parvi-holder/check-windows.sh` copies the
crate to a scratch directory under `%TEMP%` and runs fmt, clippy and every
test there with the Windows toolchain. Set `PARVI_HOLDER_SCRATCH` to a unique
directory name when other agents may run it at the same time. Each test starts
its own holder on a scratch pipe and ends it, so the operator's holder and
sessions are untouched.

| Test | Shows |
|---|---|
| `survival` | PowerShell 5 and 7 sessions survive killing the process attached to them: state, output and a new attach carry on |
| `flood` | three sessions flooding, with clients that stop reading, leave another session answering promptly. Memory stays bounded, and the stuck clients are paused or dropped |
| `no_windows` | starting the holder, PowerShell 5, 7 and cmd sessions, console programs in them, and kills show no window. A control window opened from a session on purpose is seen |
| `backpressure` | input to a program that isn't reading is refused at 1 MB, and a client pipelining whole-history captures it doesn't read is dropped. Holder memory stays bounded in both cases |
| `memory` | what an idle session costs |
| `sessions` | one holder per pipe, including racing starters, the protocol check, sizing, notifications, paste, history, `%exit`, and captured lines that look like `%end` |

## Measured

Measured on 2026-11-05 with release builds on a ThinkPad running Windows 11
Enterprise 10.0.26200, with other agents' builds running.

| What | Result |
|---|---|
| Holder, no sessions | 1.25 MB private |
| Holder, per idle session (10 cmd sessions) | **0.34 MB private**, so about 18 MB for 50 sessions |
| Console host per session (conhost.exe) | 1.74 MB private, 13.2 MB working set |
| Shell per session | cmd 6.8 / 12.1 MB, Windows PowerShell 5.1 58 / 81 MB, PowerShell 7 37 / 102 MB (private / working set) |
| Flood, three sessions with stuck clients | 20.8 MB in 5 s reached the reading clients. The slowest round on a quiet session was 111 ms over 49 rounds: a `display-message`, then a typed line, polling the screen every 100 ms until it echoed. Holder private memory went from 3.2 to 45.8 MB: three full histories plus bounded queues |
| ConPTY throughput | 0.2 to 0.9 MB/s per session, in passthrough and plain modes alike, read in pieces of about 260 bytes. The holder held each pane's lock for about 10% of that time, so ConPTY sets the pace |
| `parvi-holder.exe` | 864 KB release without the workspace's LTO and strip |
