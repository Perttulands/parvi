# Waiting-prompt fixtures (issue #18)

Plain text screens, one row per line, for `src/waiting.rs`. Where each one
comes from:

- **Captured** from a real pane with `tmux capture-pane -p` in WSL at 120x40:
  - `claude-trust.txt`: Claude Code 2.1.174's folder-trust prompt.
  - `top.txt`, `vim.txt`, `shell.txt` and `man-page.txt`: programs that never wait.
- **Codex's own snapshot tests**, codex-rs `tui/src/bottom_pane/approval_overlay`. Codex 0.134 doesn't start in WSL here, so these weren't captured:
  - `codex-network.txt` is the `network_exec_prompt` snapshot.
  - `codex-narrow.txt` is the 40x7 clipped exec approval, where the question is cut off.
  - `codex-exec.txt` is an exec approval put together from the strings in `approval_overlay.rs`.
- **Reconstructed:** `claude-permission.txt` is a Claude Code permission prompt, laid out like the captured trust prompt. Replace it with a capture at the milestone's end-to-end check.