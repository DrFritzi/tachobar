# Security Policy

## Supported versions

Only the latest release gets security fixes.

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting ("Report a vulnerability"
on the repository's Security tab). Do not open a public issue. You should get
a first response within a week.

## What tachobar touches

- **Reads:** the JSON Claude Code sends on stdin, the session transcript and
  its subagent transcripts (`*.jsonl`, token counts only), and
  `~/.claude/settings.json` (`effortLevel`).
- **Writes:** its own cache, state and config directories. It writes
  `~/.claude/settings.json` only when you run `tachobar --init --write`, and
  keeps a `.bak` copy of the previous file.
- **Network:** at most once a day, HTTPS GET requests to
  `raw.githubusercontent.com` (litellm pricing JSON) and `api.frankfurter.app`
  (exchange rates). No telemetry, and nothing from your sessions is sent
  anywhere.
