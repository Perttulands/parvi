# Design

What good looks like in the app. `VISION.md` says what Parvi is for; this says
how a page earns its place and how the pages become one app.

## The aim

Parvi is a terminal workspace with things to read beside it. The operator is
in a session most of the time. Everything else exists for one moment: they
need to see something (a file, a work item, a brain page, a host reading) so
they can prompt an agent or make a small edit. A page is good when that moment
is short: open, see the answer, hand it to a session, back to the terminal.

So every page is judged by three questions, in this order:

1. **The first screen answers the page's question** without a click, a scroll
   or a choice. If the answer is "all is well", the page says so in one line
   instead of listing everything that is fine.
2. **The next step is one action away**: open it beside the terminal, send it
   to a session, or change it by hand.
3. **Nothing else is on the screen.** No inventory for its own sake, no
   explanation of how the page works, no control that does nothing in the
   current state.

## The question each page answers

- **Terminal.** What are my agents doing, and let me talk to one. Sessions
  are shared with other places, so nothing here resizes or disturbs one.
- **Files.** Show me this file, and what just changed. A tree and a viewer:
  reach any file by typing part of its name, see recent changes first, read
  code, Markdown and pictures comfortably, edit and save safely. Not a
  launcher of project cards.
- **Work.** What is open, blocked and next in this project. Opens on one
  project's open work as a tree of epics and items, with the item beside it.
  The whole store across projects is a search away, not the default.
- **Brain.** Find what we already know. Search first; a result reads well and
  goes to a session in one action.
- **Host.** Is this machine all right now. Current values and anything wrong
  first; history is a small line beside each value, bigger on demand.
  Services, scheduled prompts and agent setup are parts of the same
  question (below).
- **Services.** What is running and what is broken. Running and failed first,
  one action per state (a stopped service offers Start, not Start and
  Restart), its address opens it.
- **Scheduled.** What will run when, and how the last run went.
- **Agent setup** (today "Agents"). Can each harness run here and are its
  skills current. One line when everything is in sync; otherwise only what is
  off, each with its fix.
- **Missions.** What long work is running, what it produced, and set off the
  next one. Runs and their outcome first; the graph is where you define and
  follow one.
- **Settings.** Every setting, said plainly, changed in place. The node's
  configuration is shown as settings, not as a file dump.

## One app

- **One skeleton.** A list on the left (what), the thing on the right. One
  header line per page, holding the page's own search or filter and at most
  two actions. Same row height, type sizes and spacing everywhere; a page
  does not invent its own frame, card or table style.
- **Quiet by default.** Colour means state (failed, needs you, running) or
  the one primary action. No coloured strips, frames or outlines as
  decoration. The device's colour appears on its plate only. Labels read as
  written, in JetBrains Mono.
- **Words only when they change what you do.** No sentences describing the
  page, no key legends, no badge for the normal state. Keys live in tooltips
  and the Keys overlay. Empty states are one short line and the action that
  fills them.
- **Send is the same everywhere.** Anything you can look at can be sent to a
  session, the same way, from the same place on the row or header.
- **Beside, not instead.** What you open from a terminal lies over the
  terminals and closes with Esc, leaving them as they were.
- **Fast.** A page shows what it has at once and fills in; nothing blocks on
  a read. Typing in a search answers as you type.
- **Sizes.** Right at 1456x939, at a wide desktop and narrow; names shorten
  in the middle, primary actions never clip.

## When in doubt

Remove it. A half-finished feature, a control nobody asked for, a number
nobody acts on: the page is better without it. Then say what was removed.
