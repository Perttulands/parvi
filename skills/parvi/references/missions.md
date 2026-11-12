# Missions

A mission is a reusable graph. An Input card starts it, formations are steps with staffed slots and ports, gates route verdicts, and End nodes end paths as `done` or `rejected`. A run snapshots the graph and staffing at admission; later edits affect later runs. Seats are ordinary agent sessions with the harness's access. The runtime creates and cleans them up.

## Connect

Use the node that owns the mission, as described in [the Parvi skill](../SKILL.md). `parvi help`, `parvi <noun>` and `parvi <noun> <verb> --help` are the current command reference; use `--json` for IDs and `--node` on each command when targeting another node. The examples below use the local node. An area can be disabled or a node can run an older build: check its help before relying on a command.

## Author

Read `parvi mission list`, `parvi mission inspect <mission> --json`, `parvi mission notes <mission> --json`, and `parvi role list` before changing an existing graph. Preserve the draft and notes. Append an explanation of each new or changed step with `parvi mission note <mission> --target <step-id> --text '<explanation>'`; notes express operator intent, while briefs instruct seats. Take IDs and port IDs from the returned JSON, not guesses. Selectors can also be unique titles; endpoints are `node:port`.

Create an empty mission with `parvi mission new <slug> --title '<title>'`, then its Input card with `parvi mission create <slug> --goal '<goal>'`. Add a step with `parvi formation create <slug> --type solo --title '<title>' --json`; its returned `formation` has `id`, `inputs`, `outputs`, and `slots`. Staff it explicitly with `parvi formation assign <slug> <step-id> --harness claude --model haiku --effort low` (name the seat with `--slot <slot-id>` when the step has several); choose harness, model and effort for the work. A model the harness does not offer on this node is refused, and the refusal lists the ones it does. Edits answer with what changed, under `changed`, `removed` and the created item. `parvi role characters` lists saved staffing, and `--role` adds reusable role text. Put task-specific instructions in `parvi formation set-brief <slug> <step-id> --goal '<instructions>'`, not in a role.

Connect the Input card with `parvi mission wire <slug> --to '<step-id>'`. Connect later ports with `parvi formation wire <slug> '<from-id>' '<to-id>'`; a bare id takes a step's only port, an Input's `out` and an End's or gate's `in`, and a gate leaving needs `:pass` or `:fail`. Name the port as `id:port` when there are several. Output ports can fan out; a step waits for all inputs. `--join` adds an input to an occupied step input. Route every step output and both gate verdicts to a step, gate or End. Add Ends with `parvi end create <slug> --outcome done` or `--outcome rejected` and wire to `<end-id>:in`. `parvi mission validate <slug> --json` must have no errors before admission; resolve warnings about unintended unreachable nodes too. Edit nodes in place to preserve edges. `parvi mission edit` accepts one JSON authoring operation when needed; prefer named commands and their help.

`parvi layout show <slug>` reads positions; `parvi layout edit <slug> '{"arrange":true}'` replaces them with arranged positions. Arrange deliberately after substantial rewiring, and read the layout back; it changes the operator's canvas.

## Inputs and outputs

`parvi mission input <slug> --name <name> --kind text --required --description '<meaning>'` declares an input (`text`, `file`, or `folder`); `--optional` and `--delete` change or remove it. With no declarations the run takes a required `brief`. Reference inputs in step briefs as `{name}`; `{{name}}` is literal. File and folder inputs are existing absolute paths on the node. `parvi formation add-output` or `formation set-port` sets a step port's `text`, `file`, or `folder` kind. File and folder outputs are copied into the attempt's evidence and the copy's path is routed. Declare a promised mission output using `parvi mission output <slug> --name <name> --kind file --from '<step-id>:<port-id>' --required`; a missing required output on a done path fails the run. Check exact flags with command help.

## Gates

`parvi gate create <slug> --title '<title>' --criterion '<criterion>' --kinds formation,human --json` creates a judge followed by human sign-off. Create a separate solo judge formation and attach it with `parvi gate judge <slug> <gate-id> --chain <judge-id>`. Wire work output to `<gate-id>:in`, and `<gate-id>:pass` and `<gate-id>:fail` to their next destinations. A fail may loop back to work; cap loops with a Limit card. A human-only gate is the default. A code gate checks `output_contains` or `output_absent` version 1; it does not run shell commands. Kinds execute in code, formation, human order, stopping at failure.

The runtime supplies each seat its output contract; a judge also returns a fenced verdict block containing `verdict` (`pass` or `fail`), `reason`, and `evidence`. Do not direct a seat by typing into its session. Inspect run evidence and the seat if a result fails to route.

## Limits

Runs have no automatic limits. `parvi limit create <slug> --target <step-id> --rounds 3` caps step attempts; `--target input` covers the whole mission. `--time 30m`, `--warn 5m` and `--tokens <count>` are optional. The default on exhaustion is to block; `--when-spent fail` ends the run, while `--when-spent best` selects among judged attempts at a suitable send-back gate. Use `limit update` and `limit delete` to change cards. A human pick or a grant needs the operator's authority.

## Run and watch

Start only with the authority and inputs appropriate to the work. `parvi mission run <slug> --inputs '{"brief":"<work>"}' --actor 'agent:<session>' --json` returns `runId`; use named values in the JSON object for declared inputs. `--cwd` selects an existing workspace; `--work` identifies the owning work item and repeated `--context-paths` supply references. `parvi formation run` starts at one step. If the receipt is lost, inspect `parvi run list --mission <slug>` before starting another.

Drive with `parvi run wait <run-id> --json`. The default `needs-you` returns when a gate needs a decision, the run blocks, or it ends. The result has `outcome`, `status`, `cursor`, `asks`, and exact answer commands in `next`; exit 0 means the wait answered, not that the run succeeded. After an answer, wait again using `--since <cursor>`. `--until final` waits for the end, `--until any-change` reports the next change, and `--timeout <seconds>` bounds a wait. On timeout, wait again with the returned cursor. Inspect `parvi run status`, `run seats`, `run logs`, `run attempts`, and `run evidence` for state, attempts, problems, briefs, produced file paths and (`written`) the files agents wrote in the run's folder. Use `--step` with evidence or attempts to narrow the result. Read final status and outputs before reporting success; a picked result carries failed work selected for continuation, so inspect its findings before using it.

## Decide

For a human gate, use `parvi run gates <run-id> --json` and `parvi gate request <run-id> <gate-id> --json` to inspect its criterion, inputs, consequences and current request. Decide only with the operator's authority. Use the exact `parvi gate approve` or `parvi gate reject` command from `run wait`, including the current `--requested-seq`; `--response` travels onward on approval or becomes rejection feedback. For a best-attempt request, inspect candidates and `run attempts`, then use the offered `gate pick` with its attempt and current sequence. On a stale request, read it again rather than reusing the sequence. Wait again after the decision.

## Recover

Read `run status` and `run evidence` for the block and whether resume is allowed. A seat that went quiet without saying it was done is reminded twice, then its step blocks with `seat_idle_without_sentinel` (or `seat_stuck` when it never took its brief, with what its screen said); even `run wait --until final` returns then. Its terminal is kept: resuming asks that seat once to go on, and a closed terminal blocks again with `seat_gone`, which needs `--mode redispatch`. Resolve the named cause, then `parvi run resume <run-id> --reason '<reason>'`; use `--mode redispatch` when a lost seat must run again. A spent limit may require an authorized `--grant` or `--pick-best`; otherwise `parvi run abort <run-id> --reason '<reason>'` stops that run. Preserve its briefs and artifacts while investigating. A final failed or canceled run needs a new run after fixing its cause. Follow a resumed run with `run wait` again.

`parvi run delete <run-id>` removes a finished run with its record, artifacts and the folder Parvi made for it; a repository run must be landed first. `parvi mission delete <slug>` removes a mission. Delete only with the operator's say-so.
