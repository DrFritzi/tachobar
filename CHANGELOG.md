# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - Unreleased

### Added

- Status line segments: directory, model, effort, price per 1M tokens, context
  usage, session cost including subagents, and burn rate (tokens/s, cost/h).
- Per-bucket pricing from litellm: input, output, 5-minute and 1-hour cache
  writes, cache reads, the >200k long-context tier, and web search requests.
- Fast mode, US-only inference (`inference_geo`) and Batch pricing, read from
  each response's `usage`. Fast-mode prices and the data residency multiplier
  come from Anthropic's pricing page and are refreshed daily.
- Responses that can only be priced as a lower bound (cache writes without a
  TTL split, Priority Tier) are counted and flagged instead of silently
  treated as exact.
- Subagent transcripts priced line by line, each with its own model's rates,
  deduplicated by message and request id, and cached by file size and mtime.
- Any currency via ECB reference rates (frankfurter.app), with the
  conventional symbol placement for each currency and configurable formatting.
- Bundled pricing and FX snapshots so the first run works offline. Stale data
  and unknown models are flagged in the status line instead of guessed.
- Daily background refresh that never blocks rendering.
- TOML config: currency, segment order, color thresholds.
- `tachobar --init [--write]`, `config`, `refresh` and `doctor` commands.
- Claude Code plugin with a `/tachobar:setup` skill; the repository doubles as
  a plugin marketplace.
- Prebuilt binaries for Windows, macOS and Linux (x64 and arm64) via cargo-dist,
  with GitHub build provenance attestations.
- crates.io publishing through Trusted Publishing (OIDC), with no stored API
  token. All workflow actions are pinned to commit SHAs and kept current by
  Dependabot.
