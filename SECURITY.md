# Security Policy

## Supported versions

Only the latest release gets security fixes.

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting ("Report a vulnerability"
on the repository's Security tab). Do not open a public issue. You should get
a first response within a week.

## Verifying a release

Every release archive has a GitHub build provenance attestation. To check
that a download was built by this repository's release workflow:

```sh
gh attestation verify tachobar-x86_64-unknown-linux-musl.tar.xz --repo DrFritzi/tachobar
```

## What tachobar touches

- **Reads:** the JSON Claude Code sends on stdin, the session transcript and
  its subagent transcripts (`*.jsonl`, token counts only), and
  `~/.claude/settings.json` (`effortLevel`).
- **Writes:** its own cache, state and config directories. It writes
  `~/.claude/settings.json` only when you run `tachobar --init --write`, and
  keeps a `.bak` copy of the previous file.
- **Network:** at most once a day, HTTPS GET requests (made by running the
  system's `curl`, with fixed URLs and no shell) to
  `raw.githubusercontent.com` (litellm pricing JSON), `platform.claude.com`
  (Anthropic's pricing page, for fast mode and data residency prices) and
  `api.frankfurter.app` (exchange rates). No telemetry, and nothing from your sessions is sent
  anywhere.
