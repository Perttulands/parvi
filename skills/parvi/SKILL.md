---
name: parvi
description: Drive Parvi, the operator's agent workspace, from the command line. Use when inspecting or acting on sessions, files, work, the brain, host services, scheduled prompts, roles, missions, runs, or the open app through Parvi.
---

# Parvi

Parvi runs a node in each machine environment. Use `parvi help` for the running local node's commands, `parvi <noun>` for its verbs and `parvi <noun> <verb> --help` for arguments. Commands depend on the areas enabled on that node. Start the node with `parvi serve` if needed. `parvi mcp` exposes the same tools over stdio.

Commands use `parvi <noun> <verb>`. Required arguments can be positional or named flags; object arguments are JSON. Add `--json` for structured output. Add `--node <id>` to act on a node through the local node, or `--node <url>` to contact a node directly. Without it, commands address the local node. Use the command's help for current arguments and defaults.

| Noun | Purpose and verbs |
|---|---|
| `session` | Terminal sessions: `list`, `read`, `history`, `search`, `send`, `keys`, `launch`, `copy`, `rename`, `kill`, `mouse`, `seen`. |
| `file` | Browse and change files: `list`, `read`, `write`, `mkdir`, `move`, `delete`, `find`, `search`, `copy`, `resume`, `transfer`. Use the base version on writes to detect stale edits. |
| `work` | Read the configured work board and item family: `list`, `show`. |
| `brain` | Find and read knowledge: `search`, `read`. |
| `host` | Read host information: `show`. |
| `service` | Inspect and control services: `list`, `show`, `start`, `stop`, `restart`. |
| `scheduled` | Manage prompts sent into sessions: `list`, `show`, `create`, `edit`, `copy`, `pause`, `resume`, `run`, `delete`. |
| `role` | Manage the role library: `list`, `inspect`, `usage`, `new`, `edit`, `retire`, `restore`, `delete`, `characters`. |
| `agents` | Inspect agent tools, skills and checkouts with `show`; `sync` a skills checkout. |
| `mission`, `formation`, `layout`, `gate`, `end`, `limit`, `run` | Author mission graphs and drive runs; read [missions](references/missions.md). |
| `view` | App view commands, when available; consult node help for verbs. |
| `config` | Node configuration commands, when available; consult node help for verbs. |

## Sessions and scheduled prompts

Read a session before sending into it; the operator may be working there. `session send` pastes text and normally presses Enter; `--no-enter` leaves it unsubmitted. A receipt says tmux took the input, not that the agent consumed it or finished. If delivery is uncertain, inspect the pane before retrying. Kill only sessions you launched.

The app's Send to Session composes a reference, note and attached file paths into a message; attachments are copied to the target node first. `session send` sends the supplied text only, so prepare any reference and paths yourself. The app leaves a message unsubmitted when it launches a new session.

For recurring work, inspect `scheduled list` and update a matching task instead of duplicating it. Target existing session names (or `user/name`), inspect the receiving session and its input, and write a durable prompt with the purpose, context and expected result. `scheduled create` takes a name, prompt, sessions and either `--every` minutes or a five-field `--cron`; specify an IANA `--timezone`. Use command help for exact syntax. Read back the task and its next run after changing it. `scheduled run` sends immediately; use it only when that is appropriate. The scheduler does not launch an agent. Check the receiving conversation and artifacts to verify consumption and completion.

For authoring a mission, staffing its steps, or driving and recovering a run, read [missions](references/missions.md).
