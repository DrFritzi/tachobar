//! Burn rate: a rolling one-hour window of (time, cost, tokens) samples per
//! session, stored in the platform state dir.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::paths;

const WINDOW_SECS: f64 = 3600.0;
const MIN_SPAN_SECS: f64 = 60.0;
const MAX_SAMPLES: usize = 360;
/// Sample files of sessions idle for this long are deleted.
const EXPIRE_SECS: u64 = 7 * 24 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub t: f64,
    pub usd: f64,
    pub tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub tokens_per_sec: f64,
    pub usd_per_hour: f64,
}

pub fn dir() -> PathBuf {
    paths::state_dir().join("burn")
}

/// Add a sample and drop the ones outside the window. Keeps at most
/// `MAX_SAMPLES`, thinning the oldest half when full, so the file stays small
/// even with a 1-second refresh interval.
pub fn push(samples: &mut Vec<Sample>, s: Sample) {
    // Cost can reset (e.g. `/clear` starts over); restart the window then.
    if samples
        .last()
        .is_some_and(|l| s.usd < l.usd || s.tokens < l.tokens)
    {
        samples.clear();
    }
    samples.retain(|x| s.t - x.t <= WINDOW_SECS && x.t <= s.t);
    samples.push(s);
    if samples.len() > MAX_SAMPLES {
        let half = samples.len() / 2;
        let mut i = 0;
        samples.retain(|_| {
            i += 1;
            i > half || i % 2 == 1
        });
    }
}

pub fn rate(samples: &[Sample]) -> Option<Rate> {
    let (first, last) = (samples.first()?, samples.last()?);
    let span = last.t - first.t;
    if span < MIN_SPAN_SECS {
        return None;
    }
    let dusd = last.usd - first.usd;
    let dtok = last.tokens.saturating_sub(first.tokens) as f64;
    if dusd <= 0.0 && dtok <= 0.0 {
        return None;
    }
    Some(Rate {
        tokens_per_sec: dtok / span,
        usd_per_hour: dusd.max(0.0) / span * 3600.0,
    })
}

fn load(path: &Path) -> Vec<Sample> {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Record a sample for the session and return the current rate.
pub fn record(session_id: &str, now: f64, usd: f64, tokens: u64) -> Option<Rate> {
    let id = paths::sanitize(session_id);
    if id.is_empty() {
        return None;
    }
    let dir = dir();
    let path = dir.join(format!("{id}.json"));
    let is_new = !path.exists();
    let mut samples = load(&path);
    push(
        &mut samples,
        Sample {
            t: now,
            usd,
            tokens,
        },
    );
    if let Ok(bytes) = serde_json::to_vec(&samples) {
        let _ = paths::write_atomic(&path, &bytes);
    }
    if is_new {
        prune(&dir, now as u64);
    }
    rate(&samples)
}

/// Delete sample files of long-idle sessions. Runs when a new session starts.
fn prune(dir: &Path, now: u64) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for ent in rd.flatten() {
        let old = ent
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|d| now.saturating_sub(d.as_secs()) > EXPIRE_SECS);
        if old {
            let _ = fs::remove_file(ent.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(t: f64, usd: f64, tokens: u64) -> Sample {
        Sample { t, usd, tokens }
    }

    #[test]
    fn needs_a_minute_of_data() {
        let mut v = Vec::new();
        push(&mut v, s(0.0, 0.0, 0));
        push(&mut v, s(30.0, 1.0, 1000));
        assert_eq!(rate(&v), None);
        push(&mut v, s(120.0, 2.0, 12000));
        let r = rate(&v).unwrap();
        assert!((r.usd_per_hour - 60.0).abs() < 1e-9);
        assert!((r.tokens_per_sec - 100.0).abs() < 1e-9);
    }

    #[test]
    fn drops_samples_outside_window() {
        let mut v = Vec::new();
        push(&mut v, s(0.0, 0.0, 0));
        push(&mut v, s(3000.0, 1.0, 10));
        push(&mut v, s(3700.0, 2.0, 20));
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].t, 3000.0);
    }

    #[test]
    fn resets_when_cost_goes_down() {
        let mut v = Vec::new();
        push(&mut v, s(0.0, 5.0, 100));
        push(&mut v, s(100.0, 0.1, 5));
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn stays_bounded() {
        let mut v = Vec::new();
        for i in 0..5000 {
            push(&mut v, s(i as f64 * 0.5, i as f64, i));
        }
        assert!(v.len() <= MAX_SAMPLES);
        assert_eq!(v.last().unwrap().tokens, 4999);
    }
}
