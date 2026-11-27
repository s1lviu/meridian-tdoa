use crate::{dsp, geo, kiwi, model::*};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Datelike, Duration, Utc};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, fs, path::Path};

pub fn checksum(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}
pub fn valid_position(p: Point) -> bool {
    p.lat.is_finite() && p.lon.is_finite() && p.lat.abs() <= 90. && p.lon.abs() <= 180.
}
pub fn valid_evidence(e: &Evidence) -> bool {
    (e.url.starts_with("https://") || e.url.starts_with("http://")) && !e.note.trim().is_empty()
}
pub fn validate_manifest(m: &Manifest) -> Result<()> {
    ensure!(
        !m.id.is_empty()
            && m.id.len() <= 80
            && m.id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            && m.frequency_hz > 0.
            && m.frequency_hz <= 30e6,
        "Invalid manifest/frequency"
    );
    ensure!(
        ["replay", "live"].contains(&m.mode.as_str()),
        "Unknown mode"
    );
    ensure!(
        [
            "public_beacon",
            "participating_amateur",
            "public_time_reference"
        ]
        .contains(&m.kind.as_str()),
        "Target is not eligible"
    );
    let minimum_receivers = if m.capture_profile.as_deref() == Some(crate::ibp::PROFILE)
        && m.kind == "public_beacon"
        && m.mode == "live"
    {
        1
    } else {
        3
    };
    ensure!(
        (minimum_receivers..=8).contains(&m.receivers.len()),
        "Invalid receiver count for this capture profile"
    );
    ensure!(valid_evidence(&m.provenance), "Missing provenance");
    DateTime::parse_from_rfc3339(&m.capture_utc).context("Invalid UTC timestamp")?;
    ensure!(
        (0..=60).contains(&m.gps_utc_offset_s),
        "Invalid GPS/UTC offset"
    );
    let mut ids = HashSet::new();
    let mut devices = HashSet::new();
    let mut hashes = HashSet::new();
    for r in &m.receivers {
        ensure!(
            ids.insert(&r.id)
                && devices.insert(&r.independent_device)
                && !r.independent_device.is_empty(),
            "Non-independent or duplicate receivers"
        );
        ensure!(
            hashes.insert(&r.sha256) && r.sha256.len() == 64,
            "Duplicate captures or missing SHA256"
        );
        ensure!(
            valid_position(r.position)
                && valid_evidence(&r.position_evidence)
                && r.position_uncertainty_m.is_finite()
                && r.position_uncertainty_m >= 0.
                && r.position_uncertainty_m <= 5000.,
            "Antenna position is insufficiently documented"
        );
    }
    if let Some(i) = &m.identity {
        ensure!(
            !i.callsign.is_empty() && valid_evidence(&i.evidence),
            "Identity has no evidence"
        );
    }
    Ok(())
}

/// The solver has no parameter for known transmitter coordinates. Evaluation is a separate command.
pub fn process(manifest_path: &Path, fault_receiver: Option<&str>) -> Result<Emission> {
    let m: Manifest = serde_json::from_slice(&fs::read(manifest_path)?)?;
    ensure!(
        m.capture_profile.as_deref() != Some(crate::ibp::PROFILE),
        "Process IBP captures as individual UTC slots"
    );
    validate_manifest(&m)?;
    ensure!(
        m.emission_verified,
        "The captured emission needs eligibility evidence before localization; target frequency alone is insufficient"
    );
    if let Some(id) = fault_receiver {
        ensure!(
            m.mode == "replay" && m.receivers.iter().any(|r| r.id == id),
            "Fault injection requires a receiver from the recording"
        );
    }
    let root = manifest_path.parent().unwrap().canonicalize()?;
    let mut total_bytes = 0u64;
    for r in &m.receivers {
        let p = root.join(&r.file).canonicalize()?;
        ensure!(p.starts_with(&root), "Path outside the recording directory");
        total_bytes = total_bytes.saturating_add(fs::metadata(p)?.len());
    }
    ensure!(
        total_bytes <= 64 * 1024 * 1024,
        "IQ dataset exceeds the total 64 MiB limit"
    );
    let mut tracks = vec![];
    for r in &m.receivers {
        let p = root.join(&r.file).canonicalize()?;
        ensure!(p.starts_with(&root), "Path outside the recording directory");
        ensure!(
            fs::metadata(&p)?.len() <= 64 * 1024 * 1024,
            "IQ file too large"
        );
        ensure!(checksum(&p)? == r.sha256, "SHA-256 mismatch: {}", r.id);
        let mut t = kiwi::read(&p)?;
        if Some(r.id.as_str()) == fault_receiver {
            t.max_age = 255;
            t.sync_error = Some("TEST: injected GNSS loss; RF samples are unchanged".into());
        }
        tracks.push(t);
    }
    let start = tracks
        .iter()
        .map(|t| t.start())
        .fold(f64::NEG_INFINITY, f64::max)
        + 0.05;
    let end =
        (tracks.iter().map(|t| t.end()).fold(f64::INFINITY, f64::min) - 0.05).min(start + 120.);
    let date = DateTime::parse_from_rfc3339(&m.capture_utc)?.with_timezone(&Utc);
    let gps_date = date + Duration::seconds(m.gps_utc_offset_s);
    let week = gps_date
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        - Duration::days(gps_date.weekday().num_days_from_sunday() as i64);
    let utc = |s: f64| {
        (week + Duration::microseconds((s * 1e6) as i64) - Duration::seconds(m.gps_utc_offset_s))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    };
    ensure!(
        (week.timestamp() as f64 + start - m.gps_utc_offset_s as f64 - date.timestamp() as f64)
            .abs()
            < 180.,
        "GPS week is inconsistent with the UTC capture anchor"
    );
    let overlap = end - start;
    let mut e = Emission {
        id: format!(
            "{}{}",
            m.id,
            if fault_receiver.is_some() {
                "-sync-loss"
            } else {
                ""
            }
        ),
        dataset_id: m.id.clone(),
        mode: m.mode.clone(),
        fault_injected: fault_receiver.is_some(),
        title: m.title,
        frequency_hz: m.frequency_hz,
        start_utc: utc(start),
        end_utc: utc(end.max(start)),
        status: "undetermined".into(),
        reason: None,
        identity: m.identity,
        provenance: m.provenance,
        receivers: vec![],
        pairs: vec![],
        estimate: None,
        hypotheses: vec![],
        detected: false,
        spectrum: vec![],
        processed_at: Utc::now().to_rfc3339(),
        reference: None,
        excluded: m.rejected_receivers.clone(),
    };
    let fs = 12000.;
    let n = (overlap.max(0.).min(120.) * fs) as usize;
    let mut streams = vec![];
    for (i, t) in tracks.iter().enumerate() {
        let (spectrum, score, stream) = if n >= 24000 {
            let raw = t.resample(start, fs, n);
            let (spec, score) = dsp::spectrum(&raw, fs);
            (spec, score, dsp::highpass(&raw))
        } else {
            (vec![], 0., vec![])
        };
        if i == 0 {
            e.spectrum = spectrum;
        }
        e.receivers.push(RxResult {
            receiver: m.receivers[i].clone(),
            confirmed: false,
            used: false,
            synchronized: t.sync_error.is_none(),
            reason: t.sync_error.clone(),
            gps_age_max_s: t.max_age,
            sample_rate_hz: t.actual_fs,
            signal_score_db: score,
            samples: n,
        });
        streams.push(stream);
    }
    if overlap < 6. {
        e.detected = e.receivers.iter().any(|r| r.signal_score_db >= 15.);
        e.reason = Some("Captures lack at least 6 s of overlap on the same GNSS timebase".into());
        e.reference = crate::protocol::reference_view(&e, &root);
        return Ok(e);
    }
    for i in 0..tracks.len() {
        for j in i + 1..tracks.len() {
            let p = dsp::correlate(
                &m.receivers[i].id,
                &m.receivers[j].id,
                &streams[i],
                &streams[j],
                fs,
                geo::distance(m.receivers[i].position, m.receivers[j].position) / geo::C_KM_S,
            );
            if p.accepted {
                e.receivers[i].confirmed = true;
                e.receivers[j].confirmed = true;
            }
            e.pairs.push(p);
        }
    }
    e.detected = e
        .receivers
        .iter()
        .any(|r| r.signal_score_db >= 15. || r.confirmed);
    locate_receptions(&mut e);
    if !e.detected {
        e.status = "no_detection".into();
        e.reason=Some("No emission detected by the spectral/correlation detector in this recorded channel and interval; this does not describe the entire band or map".into());
    }
    e.reference = crate::protocol::reference_view(&e, &root);
    Ok(e)
}

pub(crate) fn locate_receptions(e: &mut Emission) {
    // Largest fully correlated synchronized subset; other confirmed receptions remain visible.
    let mut best = vec![];
    for mask in 0usize..(1 << e.receivers.len()) {
        let ix: Vec<_> = (0..e.receivers.len())
            .filter(|i| mask & (1 << i) != 0)
            .collect();
        if ix.len() <= best.len()
            || ix
                .iter()
                .any(|&i| !e.receivers[i].synchronized || !e.receivers[i].confirmed)
        {
            continue;
        }
        if ix.iter().enumerate().all(|(k, &i)| {
            ix[k + 1..].iter().all(|&j| {
                e.pairs.iter().any(|p| {
                    p.a == e.receivers[i].receiver.id
                        && p.b == e.receivers[j].receiver.id
                        && p.accepted
                })
            })
        }) {
            best = ix;
        }
    }
    if best.len() >= 3 {
        for &i in &best {
            e.receivers[i].used = true;
        }
        // Pairwise cycle closure detects grossly incompatible delays (5σ, at least 250 µs).
        let pair = |a: usize, b: usize| {
            e.pairs
                .iter()
                .find(|p| p.a == e.receivers[a].receiver.id && p.b == e.receivers[b].receiver.id)
                .unwrap()
        };
        let mut consistent = true;
        for a in 0..best.len() {
            for b in a + 1..best.len() {
                for c in b + 1..best.len() {
                    let (ab, bc, ac) = (
                        pair(best[a], best[b]),
                        pair(best[b], best[c]),
                        pair(best[a], best[c]),
                    );
                    let limit = (5.
                        * (ab.sigma_us.powi(2) + bc.sigma_us.powi(2) + ac.sigma_us.powi(2)).sqrt())
                    .max(250.);
                    if (ab.delay_us + bc.delay_us - ac.delay_us).abs() > limit {
                        consistent = false;
                    }
                }
            }
        }
        if consistent {
            match geo::locate(&e.receivers, &e.pairs) {
                Ok(mut solutions) => {
                    if solutions.len() == 1 && best.len() >= 4 {
                        e.estimate = solutions.pop();
                        e.status = "located".into();
                    } else {
                        e.hypotheses = solutions;
                        e.reason = Some(if best.len() < 4 {
                            "Three receivers cannot exclude the mirror solution on the globe. Position undetermined; a fourth synchronized receiver is required.".into()
                        } else {
                            "Global search found two or more compatible regions. Position undetermined; an additional receiver or an independently justified search domain is required.".into()
                        });
                    }
                }
                Err(err) => e.reason = Some(err.to_string()),
            }
        } else {
            e.reason = Some("Delays fail cycle closure".into());
        }
    } else {
        e.reason = Some(
            "Fewer than three independent receivers with sufficient synchronization and correlation"
                .into(),
        );
    }
    if e.estimate.is_none() && e.hypotheses.is_empty() {
        for r in &mut e.receivers {
            r.used = false;
        }
    }
}

pub fn save(result: &Emission, dir: &Path) -> Result<()> {
    let path = dir.join(format!("{}.json", result.id));
    crate::store::atomic_json(&path, result)
}
