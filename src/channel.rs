//! Monitored channel, unknown emitters (`correlated_v1`). Nothing is assumed about who
//! transmits. The RF evidence of an emission is the same signal correlated between
//! synchronized receivers; each occupied sub-band (CFAR) is localized as its own emission.
use crate::{dsp, geo, iq, model::*, pipeline, protocol};
use anyhow::{Context, Result, ensure};
use chrono::{Duration, SecondsFormat, Utc};
use std::path::Path;

pub const PROFILE: &str = "correlated_v1";
/// Ground wave below this frequency: fully coherent 2 s windows. Above it (ionospheric
/// paths, Doppler of a fraction of a hertz) 0.25 s coherent sub-windows are combined.
const SKYWAVE_HZ: f64 = 3e6;
const MAX_SEGMENTS: usize = 8;

pub struct Analysis {
    pub emissions: Vec<Emission>,
    pub segments: Vec<dsp::Segment>,
}

pub fn analyze(path: &Path) -> Result<Analysis> {
    let m: Manifest = serde_json::from_slice(&std::fs::read(path)?)?;
    pipeline::validate_manifest(&m)?;
    ensure!(
        m.kind == "monitored_channel" && m.capture_profile.as_deref() == Some(PROFILE),
        "Channel analysis requires a monitored-channel capture"
    );
    let root = path
        .parent()
        .context("Missing capture directory")?
        .canonicalize()?;
    let mut tracks = vec![];
    let mut receivers = vec![];
    let mut excluded = vec![];
    for r in &m.receivers {
        let file = root.join(&r.file).canonicalize()?;
        ensure!(
            file.starts_with(&root),
            "Path outside the capture directory"
        );
        ensure!(
            pipeline::checksum(&iq::hashed_file(&file))? == r.sha256,
            "SHA-256 mismatch: {}",
            r.id
        );
        match iq::read(&file, &m) {
            Ok(t) if t.sync_error.is_none() => {
                tracks.push(t);
                receivers.push(r.clone());
            }
            Ok(t) => excluded.push(RejectedReceiver {
                receiver: r.clone(),
                reason: t.sync_error.unwrap(),
            }),
            Err(e) => excluded.push(RejectedReceiver {
                receiver: r.clone(),
                reason: e.to_string(),
            }),
        }
    }
    let week = iq::gps_week_start(&m)?;
    let start = tracks
        .iter()
        .map(|t| t.start())
        .fold(f64::NEG_INFINITY, f64::max)
        + 0.05;
    let end =
        (tracks.iter().map(|t| t.end()).fold(f64::INFINITY, f64::min) - 0.05).min(start + 120.);
    let center = m.iq_center_hz.unwrap_or(m.frequency_hz);
    let utc = |s: f64| {
        (chrono::DateTime::from_timestamp(week, 0).unwrap()
            + Duration::microseconds((s * 1e6) as i64)
            - Duration::seconds(m.gps_utc_offset_s))
        .to_rfc3339_opts(SecondsFormat::Millis, true)
    };
    let overlap = end - start;
    let fs = iq::FS;
    let n = (overlap.max(0.) * fs) as usize;
    let mut emissions = vec![];
    let mut segments = vec![];
    if tracks.len() >= 2 && overlap >= 6. {
        let streams: Vec<Vec<_>> = tracks.iter().map(|t| t.resample(start, fs, n)).collect();
        let refs: Vec<&[_]> = streams.iter().map(|s| s.as_slice()).collect();
        segments = dsp::occupied(&refs, fs, 4800.);
        segments.sort_by(|a, b| b.peak_db.total_cmp(&a.peak_db));
        segments.truncate(MAX_SEGMENTS);
        let coherent_s = if center < SKYWAVE_HZ { 2.0 } else { 0.25 };
        for seg in &segments {
            let edge = ((seg.high_hz - seg.low_hz) * 0.1).max(20.);
            let filtered: Vec<Vec<_>> = streams
                .iter()
                .map(|s| dsp::highpass(&dsp::bandpass(s, fs, seg.low_hz, seg.high_hz, edge)))
                .collect();
            let mut rx: Vec<RxResult> = receivers
                .iter()
                .zip(&tracks)
                .map(|(r, t)| RxResult {
                    receiver: r.clone(),
                    confirmed: false,
                    used: false,
                    synchronized: true,
                    reason: None,
                    gps_age_max_s: t.max_age,
                    sample_rate_hz: t.actual_fs,
                    signal_score_db: seg.peak_db,
                    samples: n,
                })
                .collect();
            let mut pairs = vec![];
            for i in 0..rx.len() {
                for j in i + 1..rx.len() {
                    let p = dsp::correlate_with(
                        &rx[i].receiver.id,
                        &rx[j].receiver.id,
                        &filtered[i],
                        &filtered[j],
                        fs,
                        geo::distance(rx[i].receiver.position, rx[j].receiver.position)
                            / geo::C_KM_S,
                        coherent_s,
                    );
                    if p.accepted {
                        rx[i].confirmed = true;
                        rx[j].confirmed = true;
                    }
                    pairs.push(p);
                }
            }
            for r in rx.iter_mut().filter(|r| !r.confirmed) {
                r.reason = Some("Not correlated with another receiver in this sub-band".into());
            }
            let detected = rx.iter().any(|r| r.confirmed);
            let mid = (seg.low_hz + seg.high_hz) / 2.;
            let mut e = Emission {
                id: format!("{}-{:+.0}", m.id, mid),
                dataset_id: m.id.clone(),
                mode: m.mode.clone(),
                fault_injected: false,
                title: format!("Unknown emitter · {:.0} Hz wide", seg.high_hz - seg.low_hz),
                frequency_hz: center + mid,
                start_utc: utc(start),
                end_utc: utc(end),
                status: if detected {
                    "undetermined"
                } else {
                    "no_detection"
                }
                .into(),
                reason: (!detected)
                    .then(|| "Occupied sub-band, but no two receivers correlate on it".into()),
                identity: None,
                provenance: Evidence {
                    url: "https://github.com/s1lviu/meridian-tdoa/blob/main/docs/VALIDATION.md"
                        .into(),
                    note: format!(
                        "Sub-band {:.0}…{:.0} Hz detected by CFAR (P_fa 10⁻⁶ per bin) in this capture; RF evidence is pairwise correlation between synchronized receivers. Identity unknown.",
                        center + seg.low_hz,
                        center + seg.high_hz
                    ),
                },
                receivers: rx,
                pairs,
                estimate: None,
                hypotheses: vec![],
                detected,
                spectrum: vec![],
                processed_at: Utc::now().to_rfc3339(),
                reference: Some(VerifiedReference {
                    profile: PROFILE.into(),
                    three_receivers: None,
                    cw: vec![],
                    ft8: None,
                }),
                excluded: excluded.clone(),
            };
            if detected {
                pipeline::locate_receptions(&mut e);
                if e.estimate.is_some() {
                    e.reason = None;
                }
                let three = protocol::comparison(&e);
                if let Some(r) = e.reference.as_mut() {
                    r.three_receivers = three;
                }
            }
            emissions.push(e);
        }
    }
    crate::store::atomic_json(
        &root.join("channel-detection.json"),
        &serde_json::json!({"profile":PROFILE,"iq_center_hz":center,"segments":segments,
            "excluded":excluded.iter().map(|x| serde_json::json!({"id":x.receiver.id,"reason":x.reason})).collect::<Vec<_>>(),
            "uses_transmitter_coordinates":false}),
    )?;
    Ok(Analysis {
        emissions,
        segments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::num_complex::Complex;

    /// Five SigMF receivers with GNSS-PPS time bases record one unknown wideband emitter.
    #[test]
    fn unknown_wideband_emitter_in_sigmf_recordings_is_detected_and_located() {
        let dir = tempfile::tempdir().unwrap();
        let truth = Point {
            lat: 45.9,
            lon: 24.9,
        };
        let sites = [
            Point {
                lat: 47.6,
                lon: 22.0,
            },
            Point {
                lat: 44.0,
                lon: 21.9,
            },
            Point {
                lat: 43.8,
                lon: 27.6,
            },
            Point {
                lat: 47.9,
                lon: 27.4,
            },
            Point {
                lat: 46.2,
                lon: 25.6,
            },
        ];
        let fs = 48_000.; // SDR rate, channelized to 12 kHz by the reader
        let seconds = 14.;
        let n = (fs * seconds) as usize;
        let mut seed = 21u64;
        let mut u = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        // Emitter: band-limited noise 2 kHz wide around +1 kHz (a voice/digital-like signal).
        let src: Vec<Complex<f64>> = {
            let raw: Vec<_> = (0..n + 4000)
                .map(|_| {
                    let (a, b) = (u(), u());
                    Complex::from_polar((-2. * a.ln()).sqrt(), std::f64::consts::TAU * b)
                })
                .collect();
            dsp::bandpass(&raw, fs, 0., 2000., 100.)
        };
        let t0 = "2026-11-27T12:00:00";
        let mut receivers = vec![];
        for (i, p) in sites.iter().enumerate() {
            let delay = geo::distance(truth, *p) / geo::C_KM_S; // s
            let x: Vec<Complex<f64>> = (0..n)
                .map(|k| {
                    let s = dsp::sinc_at(&src, 2000. + k as f64 - delay * fs) * 0.2;
                    let (a, b) = (u(), u());
                    s + Complex::from_polar((-2. * a.ln()).sqrt() * 0.02, std::f64::consts::TAU * b)
                })
                .collect();
            // One PPS anchor per second.
            let anchors: Vec<(u64, String)> = (0..seconds as u64)
                .map(|s| {
                    (
                        s * fs as u64,
                        format!("{t0}.{:09}Z", 0).replace(":00.", &format!(":{:02}.", s)),
                    )
                })
                .collect();
            let meta = dir.path().join(format!("node{i}.sigmf-meta"));
            crate::sigmf::write(
                &meta,
                &format!("node{i}"),
                *p,
                fs,
                7.1e6,
                "gnss_pps",
                50.,
                &anchors,
                &x,
            )
            .unwrap();
            receivers.push(Receiver {
                id: format!("node{i}"),
                label: format!("synthetic node {i}"),
                independent_device: format!("node{i}"),
                position: *p,
                position_evidence: Evidence {
                    url: "https://example.org/synthetic".into(),
                    note: "synthetic".into(),
                },
                position_uncertainty_m: 0.,
                file: format!("node{i}.sigmf-meta"),
                sha256: pipeline::checksum(&crate::sigmf::data_path(&meta)).unwrap(),
            });
        }
        let m = Manifest {
            id: "ingest-synthetic".into(),
            title: "synthetic".into(),
            kind: "monitored_channel".into(),
            mode: "live".into(),
            frequency_hz: 7.1e6,
            capture_profile: Some(PROFILE.into()),
            iq_center_hz: Some(7.1e6),
            capture_utc: format!("{t0}Z"),
            gps_utc_offset_s: 18,
            provenance: Evidence {
                url: "https://example.org/synthetic".into(),
                note: "synthetic".into(),
            },
            emission_verified: false,
            identity: None,
            receivers,
            rejected_receivers: vec![],
        };
        let path = dir.path().join("manifest.json");
        std::fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
        let a = analyze(&path).unwrap();
        let e = a
            .emissions
            .iter()
            .find(|e| (e.frequency_hz - 7.101e6).abs() < 600.)
            .unwrap_or_else(|| panic!("segments {:?}", a.segments));
        assert_eq!(e.receivers.iter().filter(|r| r.confirmed).count(), 5);
        let est = e.estimate.as_ref().expect("unique fix");
        let err = geo::distance(est.center, truth);
        eprintln!(
            "SigMF unknown emitter: error {err:.2} km, 95% radius {:.1} km",
            est.radius_km
        );
        assert!(err < 10.);
    }
}
