# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- README: an annotated screenshot numbering every part of the status line.

## [0.1.3]

### Changed

- A raw model id such as `claude-sonnet-5-5` shows family and version (`Sonnet 5.5`); ids that do not match `claude-<family>-<major>[-<minor>]` show as sent.

## [0.1.2]

### Changed

- A raw model id such as `claude-sonnet-5-5` shows as `Sonnet` (also Opus, Haiku).
  No segment is ever truncated or elided; long values wrap onto a new line.

## [0.1.1]

### Fixed

- `claude-sonnet-5-5` was unpriced, so every call of a subagent running it
  showed as `N subagent calls unpriced`. A bundled supplement
  (`data/pricing-supplement.json`) now carries current models litellm has not
  listed yet; litellm wins as soon as it has the model.

### Changed

- README: clearer install instructions (plugin, macOS/Linux, Windows zip,
  source) with update and uninstall steps, a guide to every number on the
  line, and screenshots of the color tiers, currencies and warnings.
- The PowerShell script installer is no longer published; Windows uses the
  release zip.

- Fewer dependencies: the `dirs`, `tempfile`, `ureq` and `toml` crates are
  replaced by a few hundred lines of own code (a minimal HTTPS client on
  rustls with proxy and `SSL_CERT_FILE` support, a parser for the config
  subset of TOML, an in-place `settings.json` editor). Runtime dependencies
  drop from 53 crates to about 24, and the binary from 3.2 MB to 2.6 MB.
- `tachobar --init --write` now edits `settings.json` in place, so its
  formatting, key order and line endings are kept.
- `tachobar doctor` shows whether a proxy is used.
- Third-party licence notices (`THIRD-PARTY-LICENSES.md`, generated with
  cargo-about and checked in CI) ship in every release archive.
- Bumped `actions/checkout` (v7.0.1), `Swatinem/rust-cache` and
  `EmbarkStudios/cargo-deny-action`, all still pinned to commit SHAs.
- Windows install docs: download and unpack the release zip instead of the
  `irm … | iex` script one-liner, which Microsoft Defender blocks as a
  suspicious command pattern.

## [0.1.0] - 2026-09-28

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
- All workflow actions are pinned to commit SHAs and kept current by
  Dependabot. Releases need no stored secrets.
