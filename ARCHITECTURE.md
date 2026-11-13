# Architecture

Two binaries. `parvi` is the node and the CLI. `parvi-app` is the native app; it
talks to a node only over the node's WebSocket and HTTP interface.

```
areas/<area>/
    contract/   the area's wire types: requests, events, errors. No I/O.
    node/       the node side: state, routes, commands
    app/        the app side: store and views
kit/
    core/       ids (node + local id), the error shape, the frame envelope
    node-kit/   what a node area plugs into: events, config, registration
    app-kit/    what an app area plugs into: tokens, widgets (fields, a code editor),
                node link, Host and Page
    app-shell/  the app's shell: window chrome, tab bar, keys, view control
    testkit/    starts a node holding only the areas a test names
bin/
    parvi/      the node and CLI: a list of areas, install and upgrade
    parvi-app/  the native app: the shell in a window, and its list of pages
engine/         terminal engine, session holder, work references, mission model and engine
```

Each directory under an area, `kit/`, `bin/` and `engine/` is one crate. The
areas are terminals, files, work, brain, host, scheduled, agents and missions;
fleet becomes one after the first release. Launching sessions and agent events
live in terminals. Settings is not an area: the shell's Settings page lists
what pages declare.

## Rules the compiler holds

1. **One contract per area.** Both sides of an area use the same `contract`
   crate, so a changed field fails the build on the other side. There is no
   second declaration and no document keeping two in step.
2. **Areas register themselves.** A node area hands `node-kit` its routes, its
   client actions and its commands; one command definition yields the CLI verb
   and the MCP tool. An app area hands `app-kit` its page, which declares its
   frame kinds, channels, the asks it answers and its chords.
   The binaries hold only the list of areas. Adding to an area touches that
   area's directory and nothing else; adding an area adds one line to each list
   and one dependency line to that binary's `Cargo.toml`; the workspace finds
   crates by where they sit, and CI by their names (`parvi-<area>-app`).
3. **Dependencies point one way.** `contract` depends on `core` and on plain
   data in `engine/` that has no dependencies (`work-ref`, a work item's
   reference, shared by the work area and the mission engine). The one other
   is `mission-model`, the missions' wire data, which the mission engine uses
   and which depends only on `core` and serde's data formats. `node` depends on
   its `contract` and `node-kit`; `app` on its `contract` and `app-kit`. An
   area that needs another uses that area's `contract` or an interface it
   provides, never its `node` or `app` crate (tests may). `kit/` and `engine/`
   use no area. Node crates never depend on the GUI toolkit, and app crates
   never depend on the web server. `bin/parvi/tests/layout.rs` holds this.

## Node areas

A node area is a `Listing` in `bin/parvi`'s `AREAS`: its name, frame kinds,
command names and build function, known without building it. Built from the
node's `Setup` (its identity, folder, address, config and event log), it
answers with an `Area`: HTTP routes, commands, a channel for each `/ws` client,
and a stop. Before building, `node-kit` refuses listings that share a name, a
frame kind or a command, naming both, or that take a kind `core` reserves
(the hello, the fleet's peer list, view control); a built area whose commands
are not the ones listed is refused too. `bin/parvi/tests/areas.rs` checks the
real list, and `parvi kinds` prints who owns which kinds.

- **Frames.** The first byte of a `/ws` frame is its kind. A client's frame
  goes to the channel of the area owning its kind; each channel pushes its own.
- **Commands.** A command is a `noun_verb` name (the verb may be several words: `formation_set_type` is `parvi formation set-type`), a description, an input
  schema and a call answering JSON or the error shape. `node-kit` serves it as
  an MCP tool on `/mcp` and as the CLI verb `parvi noun verb`. A `node`
  argument names where it runs.
- **One node at a time.** A request names its node with `?node=` on any route
  and on `/ws`, a command call with its `node` argument. Relaying to other
  nodes is not built: one naming another node is refused as `unknown_node`
  (HTTP 421), never run here.
- **Config.** `config.toml` in the node's folder has a `[node]` section and one
  section per area, read by that area. `enabled = false` leaves an area unbuilt
  and listed as off in `/api/health`; a section no area reads refuses to start.
  `parvi config show [--section]` and `GET /api/config` answer the file's
  path and each section as the node loaded it, and the Settings page shows
  the connected node's read-only; an agent edits the file and a restart
  applies it.
- **The node's own commands.** Every node has a few commands whatever areas
  it serves (`node-kit`'s `own::COMMANDS`: `config_show`, `view_show`,
  `view_open`); a listing that names one is refused.
- **View control.** The node keeps the view-control kinds itself: a client
  that reports the page it shows (0xb0) is an app until it goes. `parvi view
  show` lists what each app shows; `parvi view open --page <id> [--path|item|
  run|mission|document|session <id>] [--step]` sends the apps a view command
  (0xb8) and answers the first answer (0xb1): `no_app` with none connected,
  `timeout` after 5 s. The app shows the page and hands it the thing as an
  `Ask::Reveal`, on this node.
- **Provided interfaces.** What one area offers others is a narrow trait that
  lives with its provider's contract or engine crate, never in `node-kit`. The
  provider fills the slot keyed by that trait (`setup.provided.provide`) while
  it is built; a reader answers `Built::then` and reads the slot once every
  area is built, so the order of `AREAS` never matters. A slot filled twice is
  refused, and an empty one reads as `not_provided`. `Sessions` (in
  `engine/term`: list, follow, deliver to, start, and run a mission's seats)
  is the first; terminals provides it. `Resolver` (in `areas/work/contract`:
  the canonical reference for a written one, a bare Bead id placed by the
  node's Work configuration) is provided by work.
- **Hooks.** `parvi hook <url> [--then command...]` is how an outside
  program's own hook reports to the node: it posts its payload to the URL an
  area gave it and runs `--then` regardless. The URL carries what the area
  needs, such as the session and event.

## App pages

A page is a build function in `bin/parvi-app`'s `PAGES`, held by the shell in
`kit/app-shell`. Built from a `Host`, it answers with one entity implementing
`app-kit`'s `Page`: an id, a title, the node area it needs (its tab hides while
every node has that area off), its frame kinds, the channels it turns on while
shown, the asks it answers and its chords. The shell owns the window, the tab
bar, the dismissal stack, the leader, the status line and the node connection,
and refuses pages that share an id or a frame kind, take a kind `core`
reserves, or whose `everywhere` chords share a key, naming both;
`bin/parvi-app/tests/pages.rs` mounts the real list.

- **Host and Hub.** A `Host` is a cheap handle on the `Hub`, the state the
  shell and every page share: the fleet link, settings, status line, toast and
  the stack of menus and layers. Through it a page lists live nodes and where
  each device is, makes HTTP requests, sends frames, turns channels on,
  reports, opens menus and layers, posts desktop notifications (the shell
  owns the one click handler and runs the page's click), redials, keeps its
  section of `app.json`, and asks.
- **Taps.** A page whose frames are too many for the UI thread (terminal
  screens) registers a tap: the network thread hands it every frame of the
  page's own kinds first, and the page wakes itself at most once per paint.
- **Events.** The hub never calls a page. A frame of a page's kinds, a link
  to a node opening or going, an ask it answers or a closed layer's dismiss
  reaches the shell as a hub event after the update that caused it, so a page
  may use its Host inside its own update.
- **Tabs and chords.** A page may draw its own part of the tab bar (the
  terminals draw their workspace tabs there). A chord marked `everywhere`
  works from every page, as CHROTE's terminal chords do, and brings its page
  forward when it acts.
- **Asks.** Pages never depend on each other. An `Ask` (send a reference, open
  a path, reveal a thing, launch a session in a folder on a node) is plain data that goes to the page declaring it
  answers it; when none does, the asker gets an `unanswered` error and the
  status line says so. View control's `open` reaches pages the same way. A
  beside ask (open a path beside, or a reveal marked `beside`, as a work item
  named in a terminal opens its card) leaves the asker shown: the answering page
  hands it a view (`Host::show_beside`), which the asker draws where its
  layout has room (`Host::beside`), as the terminals draw a file a link
  named.
- **Docks.** A page may keep a view of its own beside every page, at the
  window's side (`Host::show_dock`, `close_dock`, `docked`), toggled by an
  `everywhere` chord it declares. While its dock is open the page's channels
  stay on, as while it shows.
- **Unsaved work.** The shell owns the window's one close handler. A page
  guards itself, or a view it shows beside another page or in a dock, with
  `Host::guard_close`, which names what that view holds unsaved; while any
  guarded view names something, closing the window brings its page forward
  and asks over the window whether to leave it or keep editing. A view that
  is gone guards nothing.
- **Settings.** A page declares its settings beside it (`Page::SETTINGS`: key,
  label, control, default). The shell's Settings page lists the shell's own
  (node, keys) and each page's under its title, keeps a change under its key in
  the page's section of `app.json`, and tells the page
  (`Page::settings_changed`). A key declared twice refuses to start. The app
  watches `app.json` (`Hub::watch`): an edit made outside it, as an agent
  makes, applies at once and each page whose section changed hears
  `settings_changed`; the app's own writes are not edits, and a file that
  does not read is reported while the last good settings stay.
- **Read models.** What one page knows that others show (the terminals' session
  list) is a plain type in its area's `contract`, published by that page
  (`Host::publish`) and read and observed by any page (`Host::model`), built in
  any order. A second publisher is refused.
- **Tests.** An app area's test drives `parvi-app-shell` with its own page, or
  pages it defines, against a testkit or fake node, building no other area.

## Why this shape

Work on Parvi is done by many agents at once. The shape keeps one piece of work
inside one directory, keeps a build to the area being changed, and turns the
boundaries from a convention into something the build refuses to break.

## Errors

One shape everywhere: a stable `code` and a human `message`, defined in `core`.
Each area owns its codes in its `contract`. Routes, commands and the app all
carry that shape; nothing returns a bare string.

## Sessions survive

Terminal sessions belong to the holder process, not to the node. Installing,
upgrading or restarting Parvi never stops a session.
tmux records a session as active when a node attaches a control client; Parvi preserves the pane's earlier output time until it writes again.

## Install and upgrade

`parvi install` and `parvi upgrade` are the binary's own verbs, not an area's
commands: they put the binary itself in place and restart the node, so no
node can own them. They have no MCP tool, because a node upgrading itself
would be stopped by its own restart before it could answer. On Linux and WSL
the node is a systemd user unit (`parvi` unless `--unit` names another) that
runs `<root>/bin/parvi serve --listen <addr>`, with `KillMode=process` so a
restart signals only the node and a tmux server it started lives on.

- **Install** copies the build (and `--app`) to `<root>/bin` (`~/.local` by
  default), writes the unit, enables and starts it, and waits for
  `/api/health` to answer as the build's version. Run again with nothing
  changed, it changes nothing. A running unit with another build is
  refused: that is an upgrade.
- **Upgrade** reads the binary and address from the unit's `ExecStart`,
  sets the installed build aside, puts the new one in its place and
  restarts the unit. If the new node is not healthy in time, it puts the
  build it set aside back, restarts it and fails with `upgrade_failed`;
  once the new node is healthy the old build is deleted.
  A unit without `KillMode=process` is refused before anything changes.
- **Version.** A build's version is the package version and its commit
  (`0.0.1+8cdb58b`), in `parvi --version`, `parvi version` and the health.

`bin/parvi/tests/install.rs` runs both against a stand-in `systemctl` on
`PATH`, with a node, a tmux server and a session of the test's own.

## Tests

An area's tests live in its crates and run against `testkit`, which starts a
node with only the named areas on a port and tmux server of its own. `bin/parvi`
holds the few tests that need the whole binary.
