The `seats` and `tokens` fixtures are copied from Archon at db76358; they
contain synthetic data. The `real` fixtures were recorded with Claude Code
2.1.318, Codex 0.166.0 and Pi 1.5.0 using the same one-line prompt. They retain
only records and fields used by the readers, the prompt, answer, model, stop
reason and token counts. Paths and identifiers were replaced, and all account,
system prompt and other personal data were removed. The native session records
were compared with Archon's readers using the same sanitized files.
