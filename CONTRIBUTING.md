# Contributing

Thanks for helping. Bug reports, pricing corrections and pull requests are
all welcome.

## Development

```sh
cargo test                      # unit + fixture + end-to-end tests
cargo clippy --all-targets -- -D warnings
cargo fmt
```

Try it by hand with a status line payload:

```sh
echo '{"model":{"id":"claude-opus-5-5","display_name":"Opus 5.5"},"cost":{"total_cost_usd":1.5}}' \
  | TACHOBAR_NO_REFRESH=1 cargo run -q
```

## Ground rules

- **Cost accuracy first.** Any change to pricing logic needs a test with a
  hand-computed expected value, like those in `tests/costs.rs`. Show the
  arithmetic in a comment.
- **Never guess silently.** If something cannot be priced, it must show up in
  the status line (`?`, `+`, or a `[note]`).
- **The render path stays fast and offline.** No network calls, no blocking
  I/O beyond the local cache files. Target: under 20 ms per render.
- **Cross-platform.** CI runs on Linux, macOS and Windows. Use `std::path`,
  never hard-coded separators or home paths.
- Keep dependencies few. Every new crate must pass `cargo deny check`.

## Updating the bundled data

Before a release, refresh the offline snapshots:

```sh
cargo run -q -- snapshot data/
```

This rewrites `data/pricing-snapshot.json` (Claude entries from litellm, plus
fast-mode prices and the data residency multiplier from Anthropic's pricing
page) and `data/fx-snapshot.json` (ECB rates with USD as the base). It fails
instead of writing partial data if any source cannot be read or parsed.

## Fixtures

Test transcripts live in `tests/fixtures/`. They are synthetic. Never commit
real transcripts, which contain your prompts and file contents.

## Releases

1. Update `version` in `Cargo.toml` and `.claude-plugin/plugin.json`, and
   `CHANGELOG.md`.
2. Refresh the bundled data (above).
3. Tag `vX.Y.Z` and push the tag. cargo-dist builds the binaries and
   installers, attests their build provenance, creates the GitHub release,
   and then publishes to crates.io.

### One-time release setup (maintainer)

No long-lived secrets are stored in the repository:

- **crates.io** uses [Trusted Publishing](https://crates.io/docs/trusted-publishing).
  crates.io only allows it for a crate that already exists, so:
  1. Publish the first version by hand: `cargo publish` with a short-lived,
     `publish-new`-scoped API token. Revoke the token afterwards.
  2. On crates.io, open the crate's *Settings -> Trusted Publishing* and add
     repository `DrFritzi/tachobar`, workflow `release.yml`, environment
     `crates-io`.
  3. In the GitHub repository settings, create the `crates-io` environment
     and limit it to tags matching `v*`. Adding yourself as a required
     reviewer is optional.
- **GitHub releases** use the workflow's own `GITHUB_TOKEN`. Build provenance
  uses GitHub's OIDC-backed attestations.
- Actions are pinned to full commit SHAs. Dependabot opens PRs to update them.
  To re-pin the release workflow, edit `[dist.github-action-commits]` in
  `dist-workspace.toml` and run `dist generate`. Do not edit `release.yml` by hand.
