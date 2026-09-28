use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use tachobar::{config, currency, paths, pricing, refresh, setup, Env};

const HELP: &str = "\
tachobar - Claude Code status line with accurate costs in any currency

USAGE:
    tachobar                  Render the status line (reads Claude Code's JSON on stdin)
    tachobar --init           Print the statusLine snippet for ~/.claude/settings.json
    tachobar --init --write   Add it to ~/.claude/settings.json (backs up the old file)
    tachobar config           Print the default config and where it goes
    tachobar config --write   Write the default config file (if none exists)
    tachobar refresh          Download pricing and exchange rates now
    tachobar doctor           Show data sources, their age and file locations
    tachobar --version        Print the version

ENVIRONMENT:
    NO_COLOR                  Disable colors
    COLUMNS                   Terminal width used for wrapping (set by Claude Code)
    TACHOBAR_CONFIG           Config file path
    TACHOBAR_CACHE_DIR        Cache directory (pricing, FX, per-session caches)
    TACHOBAR_STATE_DIR        State directory (burn-rate samples)
    TACHOBAR_NO_REFRESH       Never start a background download
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let first = args.first().map(String::as_str);
    match first {
        None => render(),
        Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("-V" | "--version") => {
            println!("tachobar {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--init" | "init") => init(has("--write")),
        Some("config") => config_cmd(has("--write")),
        Some("refresh") => refresh_cmd(has("--quiet"), has("--no-fx")),
        Some("doctor") => doctor(),
        Some("snapshot") => match args.get(1) {
            Some(dir) => match refresh::write_snapshots(&PathBuf::from(dir)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => fail(&e),
            },
            None => fail("usage: tachobar snapshot <data-dir>"),
        },
        Some(other) => fail(&format!("unknown argument {other:?}, see --help")),
    }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("tachobar: {msg}");
    ExitCode::FAILURE
}

fn render() -> ExitCode {
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);
    let env = Env::from_system();
    let line = tachobar::render_statusline(&stdin, &env);
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
    tachobar::maybe_refresh(&env);
    ExitCode::SUCCESS
}

fn init(write: bool) -> ExitCode {
    let cmd = setup::command();
    if !write {
        println!(
            "Add this to {}:\n",
            setup::settings_path().map_or("~/.claude/settings.json".into(), |p| p
                .display()
                .to_string())
        );
        println!("{}", setup::snippet(&cmd));
        println!("\nOr run `tachobar --init --write` to add it for you.");
        return ExitCode::SUCCESS;
    }
    let Some(path) = setup::settings_path() else {
        return fail("cannot find the home directory");
    };
    match setup::write_settings(&path, &cmd) {
        Ok(prev) => {
            println!("statusLine set to `{cmd}` in {}", path.display());
            if let Some(prev) = prev {
                println!("previous statusLine (also in settings.json.bak): {prev}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

fn config_cmd(write: bool) -> ExitCode {
    let Some(path) = paths::config_file() else {
        return fail("cannot find the config directory");
    };
    if !write {
        println!("# {}\n", path.display());
        print!("{}", config::DEFAULT_TOML);
        return ExitCode::SUCCESS;
    }
    if path.exists() {
        return fail(&format!(
            "{} already exists, not overwriting",
            path.display()
        ));
    }
    match paths::write_atomic(&path, config::DEFAULT_TOML.as_bytes()) {
        Ok(()) => {
            println!("wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("{}: {e}", path.display())),
    }
}

fn refresh_cmd(quiet: bool, no_fx: bool) -> ExitCode {
    let mut ok = true;
    for r in refresh::run(!no_fx) {
        match r {
            Ok(msg) if !quiet => println!("{msg}"),
            Ok(_) => {}
            Err(e) => {
                ok = false;
                if !quiet {
                    eprintln!("tachobar: {e}");
                }
            }
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn age(secs: u64) -> String {
    match secs {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

fn doctor() -> ExitCode {
    let now = paths::now_secs();
    let (cfg, cfg_err) = config::Config::load();
    let prices = pricing::PriceTable::load();
    let fx = currency::FxTable::load();
    println!("tachobar {}", env!("CARGO_PKG_VERSION"));
    println!(
        "config:    {}",
        paths::config_file().map_or("-".into(), |p| p.display().to_string())
    );
    if let Some(e) = cfg_err {
        println!("           ERROR: {e} (using defaults)");
    }
    println!(
        "currency:  {} (example: {})",
        cfg.currency,
        cfg.money_style().format(1234.5)
    );
    println!("cache:     {}", paths::cache_dir().display());
    println!("state:     {}", paths::state_dir().display());
    let src = match prices.source {
        pricing::Source::Cache => "downloaded",
        pricing::Source::Bundled => "bundled snapshot",
    };
    println!(
        "pricing:   {} models, {src}, {} old",
        prices.models.len(),
        age(prices.age_secs(now))
    );
    let fx_src = if fx.bundled {
        "bundled snapshot"
    } else {
        "downloaded"
    };
    match fx.rate(&cfg.currency) {
        Some(r) => println!(
            "fx:        1 USD = {r} {} (ECB {}, {fx_src}, {} old)",
            cfg.currency,
            fx.date,
            age(fx.age_secs(now))
        ),
        None => println!(
            "fx:        no rate for {} (amounts shown in USD)",
            cfg.currency
        ),
    }
    match refresh::curl_version() {
        Some(v) => println!("curl:      {v} (used for the daily price and rate refresh)"),
        None => println!(
            "curl:      not found; prices and rates will not refresh (bundled data is used)"
        ),
    }
    if let Some(p) = setup::settings_path() {
        let configured = std::fs::read_to_string(&p)
            .ok()
            .is_some_and(|s| s.contains("tachobar"));
        println!(
            "claude:    {} ({})",
            p.display(),
            if configured {
                "statusLine mentions tachobar"
            } else {
                "tachobar not configured, run --init"
            }
        );
    }
    ExitCode::SUCCESS
}
