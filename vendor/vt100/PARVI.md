# vt100 in Parvi

This is vt100 0.16.2, MIT licensed. In `src/row.rs`,
a uniform row stores one cell until a cell is changed. Clearing a row makes it
uniform again. This keeps the upstream parser and screen formatting behavior
while reducing the standing cost of empty terminal rows. Populated rows still
use upstream's 32-byte cells. See `bench/results/` for measurements.

`src/perform.rs` and `src/parser.rs` let the engine render U+FFFD, which tmux
3.6 and later use for malformed UTF-8 and which can also appear in output.
