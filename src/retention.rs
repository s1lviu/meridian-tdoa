//! Bounded IQ archive. Raw IQ is kept only while it is useful:
//! verified captures for `verified_iq_hours`, everything else for `unverified_iq_hours`
//! (default 0: deleted as soon as processing finishes). Small JSON evidence (manifest,
//! source checks, verifier scores, hashes) is kept for `capture_metadata_days`.
//! When a capture window would exceed the quota, the oldest IQ is evicted first.
use anyhow::{Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub verified_iq_hours: f64,
    pub unverified_iq_hours: f64,
    pub capture_metadata_days: f64,
    pub job_days: f64,
    /// 0 keeps live results indefinitely.
    pub result_days: f64,
    /// Results without a detection (coverage records) expire sooner.
    pub negative_result_days: f64,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            verified_iq_hours: 24.,
            unverified_iq_hours: 0.,
            capture_metadata_days: 30.,
            job_days: 30.,
            result_days: 90.,
            negative_result_days: 7.,
        }
    }
}

const MARKER: &str = "retention.json";

#[derive(Serialize, Deserialize)]
struct Marker {
    verified: bool,
    processed_utc: DateTime<Utc>,
}

/// Record the processing outcome that decides how long this capture's IQ is kept.
pub fn mark(capture_dir: &Path, verified: bool) -> Result<()> {
    crate::store::atomic_json(
        &capture_dir.join(MARKER),
        &Marker {
            verified,
            processed_utc: Utc::now(),
        },
    )
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Report {
    pub iq_files_deleted: usize,
    pub iq_bytes_deleted: u64,
    pub capture_dirs_deleted: usize,
}

fn wavs(dir: &Path) -> Vec<(PathBuf, u64)> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let len = e.metadata().ok()?.len();
            (p.extension()
                .is_some_and(|x| x == "wav" || x == "sigmf-data"))
            .then_some((p, len))
        })
        .collect()
}

fn delete_wavs(dir: &Path, report: &mut Report) -> Result<()> {
    for (p, len) in wavs(dir) {
        std::fs::remove_file(&p)?;
        report.iq_files_deleted += 1;
        report.iq_bytes_deleted += len;
    }
    Ok(())
}

/// Capture time from its manifest; otherwise the oldest file in the directory.
/// Directory timestamps are not used: writing the marker or deleting IQ changes them.
fn created(dir: &Path) -> Option<DateTime<Utc>> {
    #[derive(Deserialize)]
    struct Anchor {
        capture_utc: DateTime<Utc>,
    }
    if let Some(a) = std::fs::read(dir.join("manifest.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Anchor>(&b).ok())
    {
        return Some(a.capture_utc);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .min()
        .map(Into::into)
}

/// Apply the time-based rules to every capture directory.
pub fn sweep(captures: &Path, policy: &Policy, now: DateTime<Utc>) -> Result<Report> {
    let mut report = Report::default();
    let Ok(entries) = std::fs::read_dir(captures) else {
        return Ok(report);
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(born) = created(&dir) else { continue };
        let age_h = (now - born).num_seconds() as f64 / 3600.;
        if age_h >= policy.capture_metadata_days * 24. {
            std::fs::remove_dir_all(&dir)?;
            report.capture_dirs_deleted += 1;
            continue;
        }
        let marker = std::fs::read(dir.join(MARKER))
            .ok()
            .and_then(|b| serde_json::from_slice::<Marker>(&b).ok());
        let expired = match marker {
            Some(m) => {
                let since = (now - m.processed_utc).num_seconds() as f64 / 3600.;
                since
                    >= if m.verified {
                        policy.verified_iq_hours
                    } else {
                        policy.unverified_iq_hours
                    }
            }
            // No outcome after an hour: the capture failed or the process stopped mid-way.
            None => age_h >= 1. && age_h >= policy.unverified_iq_hours,
        };
        if expired {
            delete_wavs(&dir, &mut report)?;
        }
    }
    Ok(report)
}

fn bytes_in(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => bytes_in(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Make room for `reserve` bytes under `quota` by evicting the oldest IQ first.
pub fn enforce_quota(captures: &Path, quota: u64, reserve: u64) -> Result<Report> {
    ensure!(
        reserve <= quota,
        "The capture window needs more space than the whole IQ quota"
    );
    let mut report = Report::default();
    let mut used = bytes_in(captures);
    if used + reserve <= quota {
        return Ok(report);
    }
    let mut dirs: Vec<_> = std::fs::read_dir(captures)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|p| Some((created(&p)?, p)))
        .collect();
    dirs.sort();
    for (_, dir) in dirs {
        if used + reserve <= quota {
            break;
        }
        let before = report.iq_bytes_deleted;
        delete_wavs(&dir, &mut report)?;
        used = used.saturating_sub(report.iq_bytes_deleted - before);
    }
    ensure!(
        used + reserve <= quota,
        "IQ quota exhausted by files that are not captures; free space under data/captures"
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unverified_iq_goes_immediately_verified_iq_after_its_ttl_and_quota_evicts_oldest() {
        let root = tempfile::tempdir().unwrap();
        let make = |name: &str, verified: Option<bool>, bytes: usize| {
            let d = root.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("capture_a.wav"), vec![0u8; bytes]).unwrap();
            std::fs::write(d.join("manifest.json"), b"{}").unwrap();
            if let Some(v) = verified {
                mark(&d, v).unwrap();
            }
            d
        };
        let quiet = make("quiet", Some(false), 1000);
        let heard = make("heard", Some(true), 1000);
        let running = make("running", None, 1000);
        let policy = Policy::default();
        let r = sweep(root.path(), &policy, Utc::now()).unwrap();
        assert_eq!(r.iq_files_deleted, 1);
        assert!(wavs(&quiet).is_empty() && quiet.join("manifest.json").exists());
        assert_eq!(wavs(&heard).len(), 1);
        assert_eq!(
            wavs(&running).len(),
            1,
            "an in-progress capture keeps its IQ"
        );
        let r = sweep(
            root.path(),
            &policy,
            Utc::now() + chrono::Duration::hours(25),
        )
        .unwrap();
        assert_eq!(r.iq_files_deleted, 2);
        let r = sweep(
            root.path(),
            &policy,
            Utc::now() + chrono::Duration::days(31),
        )
        .unwrap();
        assert_eq!(r.capture_dirs_deleted, 3);

        let a = make("a-old", Some(true), 4000);
        let b = make("b-new", Some(true), 4000);
        let r = enforce_quota(root.path(), 10_000, 3000).unwrap();
        assert_eq!(r.iq_files_deleted, 1);
        assert!(wavs(&a).is_empty() || wavs(&b).is_empty());
        assert!(enforce_quota(root.path(), 100, 1000).is_err());
    }
}
