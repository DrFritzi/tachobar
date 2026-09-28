---
name: setup
description: Install the tachobar binary and set it as the Claude Code status line. Use when the user runs /tachobar:setup or asks to set up or install tachobar.
argument-hint: "[currency code, e.g. EUR]"
disable-model-invocation: true
allowed-tools: Bash, Read
---

Set up tachobar as the user's Claude Code status line. Plugins cannot set
`statusLine` themselves, so this skill installs the binary and writes the
setting. Work through these steps and report what you did.

1. **Check for an existing install.** Run `tachobar --version`. If it prints a
   version, skip to step 3.

2. **Install the binary** from the latest GitHub release of
   `DrFritzi/tachobar`, using the installer for the user's OS. Show the user
   the command and ask before running it.
   - macOS / Linux:
     `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/DrFritzi/tachobar/releases/latest/download/tachobar-installer.sh | sh`
   - Windows: do **not** use the `irm … | iex` script installer. Microsoft
     Defender blocks that command pattern. Instead, in PowerShell, download
     `tachobar-<arch>-pc-windows-msvc.zip` from the latest release
     (`<arch>` is `aarch64` if `$env:PROCESSOR_ARCHITECTURE` is `ARM64`,
     otherwise `x86_64`). Check its SHA256 against the `.sha256` file next to
     it, then unpack it with `Expand-Archive` into
     `$env:LOCALAPPDATA\Programs\tachobar`. The README's Windows section has
     the exact commands.
   - Alternative with a Rust toolchain: `cargo install --git https://github.com/DrFritzi/tachobar --locked`

   The macOS/Linux installer and `cargo install` put `tachobar` in
   `~/.cargo/bin`; the Windows zip route puts it in
   `%LOCALAPPDATA%\Programs\tachobar`. If `tachobar --version` still fails,
   use the full path to the binary in the next step.

3. **Write the status line setting.** Run `tachobar --init` and show the user
   the snippet. It points at the binary's absolute path. If
   `~/.claude/settings.json` already has a `statusLine`, show the current
   value and ask before replacing it. Then run `tachobar --init --write`. It
   keeps every other setting and saves a backup as `settings.json.bak`.

4. **Currency.** If the user passed a currency code ($ARGUMENTS), or asks for
   a currency other than USD: run `tachobar config --write` if no config
   exists yet, then set `currency = "<CODE>"` in the config file whose path
   `tachobar doctor` prints.

5. **Verify.** Run `tachobar refresh` to fetch current prices and exchange
   rates, then `tachobar doctor`. Tell the user the status line appears on
   the next prompt, or after restarting Claude Code.
