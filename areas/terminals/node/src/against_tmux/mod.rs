//! Tests against a real tmux on scratch sockets that reach inside the area.
//! Linux and macOS only; without tmux they skip, unless
//! `PARVI_REQUIRE_TMUX=1`, where a missing tmux fails them.

mod launch;
mod send;
