# tachobar

Rust CLI (Claude Code status line). Ground rules, fixtures, data snapshots and releases: see CONTRIBUTING.md; the rules below are the ones that bite.

- Cost accuracy is the core value. Pricing changes need a hand-computed test (pattern: `tests/costs.rs`). Unpriceable input must surface as `?`, `+` or `[note]`, never a silent estimate.
- Render path is offline and under 20 ms. Network happens only in the daily refresh (see SECURITY.md "What tachobar touches"); keep new I/O out of render.
- Must build on Linux, macOS, Windows. CI sets `RUSTFLAGS=-D warnings`, checks MSRV 1.85, and runs `cargo deny`, `cargo audit`.
- Changing dependencies: regenerate `THIRD-PARTY-LICENSES.md` (command in CONTRIBUTING.md) or CI fails.
- Fixtures in `tests/fixtures/` are synthetic. Never commit real transcripts.
- Release: bump `Cargo.toml`, `.claude-plugin/plugin.json`, `CHANGELOG.md`; refresh `data/` snapshots; run the Release workflow on `main`. Never push tags or hand-edit `release.yml` (regenerate via `dist generate`).
