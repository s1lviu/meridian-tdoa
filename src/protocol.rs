//! RF-only eligibility check for the public DCF77 time-reference commissioning channel.
//! PTB-Mitteilungen 114 (2004), pp. 353–354, Fig. 8: 9-stage LFSR, taps 5/9,
//! zero initial state with a start pulse, 512 chips, chip rate 77500/120 Hz.
//! Matching this code never supplies a delay, position or search region to TDoA.
#[cfg(test)]
use crate::kiwi;
use crate::{dsp, model::*, pipeline, store};
use anyhow::{Result, ensure};
use rustfft::{FftPlanner, num_complex::Complex};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SOURCE: &str = "https://www.ptb.de/cms/fileadmin/internet/fachabteilungen/abteilung_4/4.4_zeit_und_frequenz/4.42/dcf77.pdf";
pub const PROFILE: &str = "dcf77_prn_v1";
const FS: usize = 12000;

fn template() -> Vec<Complex<f64>> {
    let mut state = 0u16;
    let mut chips = vec![];
    for k in 0..512 {
        chips.push(if state & 256 == 0 { 1. } else { -1. });
        let feedback = ((state >> 4) ^ (state >> 8)) & 1;
        state = ((state << 1) | if k == 0 { 1 } else { feedback }) & 511;
    }
    let n = (512. * 120. / 77500. * FS as f64) as usize;
    dsp::highpass(
        &(0..n)
            .map(|i| Complex::new(chips[(i as f64 / FS as f64 * 77500. / 120.) as usize], 0.))
            .collect::<Vec<_>>(),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Match {
    pub receiver_id: String,
    pub sha256: String,
    pub tested_windows: usize,
    pub accepted_windows: usize,
    pub correlations: Vec<f64>,
    pub accepted: bool,
}
pub fn match_samples(raw: &[Complex<f64>]) -> Vec<f64> {
    let template = template();
    let m = template.len();
    let n = (FS * 2 + m).next_power_of_two();
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(n);
    let inverse = planner.plan_fft_inverse(n);
    let energy = template.iter().map(|z| z.norm_sqr()).sum::<f64>();
    let mut reference = vec![Complex::default(); n];
    reference[..m].copy_from_slice(&template);
    fft.process(&mut reference);
    let mut scores = vec![];
    for part in raw.chunks_exact(FS * 2).take(30) {
        let filtered = dsp::highpass(part);
        let mut prefix = vec![0.];
        for z in &filtered {
            prefix.push(prefix.last().unwrap() + z.norm_sqr());
        }
        let mut spectrum = vec![Complex::default(); n];
        spectrum[..part.len()].copy_from_slice(&filtered);
        fft.process(&mut spectrum);
        for (s, r) in spectrum.iter_mut().zip(&reference) {
            *s *= r.conj();
        }
        inverse.process(&mut spectrum);
        let best = (0..=part.len() - m)
            .map(|i| {
                spectrum[i].norm()
                    / n as f64
                    / (energy * (prefix[i + m] - prefix[i])).sqrt().max(1e-20)
            })
            .fold(0., f64::max);
        scores.push(best.min(1.));
    }
    scores
}

pub fn verify(manifest_path: &Path) -> Result<Vec<Match>> {
    let mut m: Manifest = serde_json::from_slice(&std::fs::read(manifest_path)?)?;
    pipeline::validate_manifest(&m)?;
    ensure!(
        m.kind == "public_time_reference" && (m.frequency_hz - 77500.).abs() < 0.1,
        "The DCF77 profile is limited to the public 77.5 kHz time reference"
    );
    let root = manifest_path.parent().unwrap().canonicalize()?;
    let mut proof = vec![];
    for r in &m.receivers {
        let file = root.join(&r.file).canonicalize()?;
        ensure!(
            file.starts_with(&root) && std::fs::metadata(&file)?.len() <= 64 * 1024 * 1024,
            "Invalid file"
        );
        ensure!(
            pipeline::checksum(&crate::iq::hashed_file(&file))? == r.sha256,
            "Checksum invalid"
        );
        let t = crate::iq::read(&file, &m)?;
        let n = (((t.end() - t.start()).max(0.).min(60.)) * FS as f64) as usize;
        let scores = match_samples(&t.resample(t.start(), FS as f64, n));
        let accepted = scores.iter().filter(|v| **v >= 0.65).count();
        proof.push(Match {
            receiver_id: r.id.clone(),
            sha256: r.sha256.clone(),
            tested_windows: scores.len(),
            accepted_windows: accepted,
            accepted: accepted >= 4 && accepted * 10 >= scores.len() * 8,
            correlations: scores,
        });
    }
    store::atomic_json(
        &root.join("verification.json"),
        &serde_json::json!({"profile":PROFILE,"reference":SOURCE,"threshold":0.65,"minimum_windows":4,"required_fraction":0.8,"receivers":proof,"uses_transmitter_coordinates":false}),
    )?;
    let passed = |r: &Receiver| {
        proof
            .iter()
            .any(|p| p.accepted && p.receiver_id == r.id && p.sha256 == r.sha256)
    };
    if proof.iter().filter(|p| p.accepted).count() >= 3 {
        // A receiver whose IQ does not carry the phase code is withheld; the others still measure.
        let (kept, failed): (Vec<_>, Vec<_>) = m.receivers.drain(..).partition(|r| passed(r));
        m.receivers = kept;
        for r in failed {
            let p = proof.iter().find(|p| p.receiver_id == r.id);
            m.rejected_receivers.push(RejectedReceiver {
                reason: format!(
                    "DCF77 phase code not verified in this IQ ({} of {} windows ≥0.65)",
                    p.map_or(0, |p| p.accepted_windows),
                    p.map_or(0, |p| p.tested_windows)
                ),
                receiver: r,
            });
        }
        m.emission_verified = true;
        m.provenance = Evidence {
            url: SOURCE.into(),
            note: format!(
                "DCF77 PRN modulation matched the PTB code in at least four 2 s windows and ≥80% of windows on {} independent receivers (normalized threshold 0.65); {} receiver(s) failed and are excluded. Per-SHA256 evidence is in verification.json. Transmitter coordinates are not used.",
                m.receivers.len(),
                m.rejected_receivers.len()
            ),
        };
        // The modulation establishes the public time-signal profile. Callsign assignment remains conservative.
        m.identity = None;
        store::atomic_json(manifest_path, &m)?;
    }
    Ok(proof)
}

/// Link observations across time only when every IQ hash has matching phase-code evidence.
pub fn reference_view(e: &Emission, capture_dir: &Path) -> Option<VerifiedReference> {
    if e.mode != "live"
        || e.fault_injected
        || e.frequency_hz != 77_500.0
        || e.provenance.url != SOURCE
    {
        return None;
    }
    #[derive(Deserialize)]
    struct Proof {
        profile: String,
        receivers: Vec<Match>,
    }
    let proof: Proof =
        serde_json::from_slice(&std::fs::read(capture_dir.join("verification.json")).ok()?).ok()?;
    if proof.profile != PROFILE
        || e.receivers.len() < 3
        || !e.receivers.iter().all(|rx| {
            proof.receivers.iter().any(|p| {
                p.accepted && p.receiver_id == rx.receiver.id && p.sha256 == rx.receiver.sha256
            })
        })
    {
        return None;
    }
    Some(VerifiedReference {
        profile: proof.profile,
        three_receivers: comparison(e),
        cw: vec![],
        ft8: None,
    })
}

pub fn comparison(e: &Emission) -> Option<ReceiverComparison> {
    let mut used: Vec<_> = e.receivers.iter().filter(|r| r.used).collect();
    used.sort_by_key(|r| &r.receiver.id);
    (used.len() > 3).then(|| {
        // Choose by receiver geometry alone, before looking at any delays or solutions.
        let mut best = [used[0], used[1], used[2]];
        let mut best_score = -1.0;
        for (i, &a) in used.iter().enumerate() {
            for (j, &b) in used.iter().enumerate().skip(i + 1) {
                for &c in &used[j + 1..] {
                    let triangle = [a, b, c];
                    let score = crate::geo::geometry(&triangle.map(|r| r.receiver.position));
                    if score > best_score {
                        best_score = score;
                        best = triangle;
                    }
                }
            }
        }
        let three = best.map(Clone::clone);
        let (solutions, reason) = match crate::geo::locate(&three, &e.pairs) {
            Ok(solutions) => (solutions, None),
            Err(err) => (vec![], Some(err.to_string())),
        };
        ReceiverComparison {
            receiver_ids: three.iter().map(|r| r.receiver.id.clone()).collect(),
            selection: "max_geometry".into(),
            solutions,
            reason,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grouping_requires_live_phase_evidence_for_every_iq_file() {
        let mut e: Emission =
            serde_json::from_str(include_str!("../data/results/dcf77-20200813.json")).unwrap();
        let d = tempfile::tempdir().unwrap();
        let receivers: Vec<_> = e
            .receivers
            .iter()
            .map(|r| {
                serde_json::json!({
                    "receiver_id":r.receiver.id, "sha256":r.receiver.sha256, "tested_windows":5,
                    "accepted_windows":5, "correlations":[0.9,0.9,0.9,0.9,0.9], "accepted":true
                })
            })
            .collect();
        let mut proof = serde_json::json!({"profile":PROFILE, "receivers":receivers});
        let path = d.path().join("verification.json");
        store::atomic_json(&path, &proof).unwrap();
        e.provenance.url = SOURCE.into();
        assert!(reference_view(&e, d.path()).is_none());
        e.mode = "live".into();
        let view = reference_view(&e, d.path()).unwrap();
        assert_eq!(view.profile, PROFILE);
        assert!(view.three_receivers.is_none());
        e.fault_injected = true;
        assert!(reference_view(&e, d.path()).is_none());
        e.fault_injected = false;
        proof["receivers"][0]["sha256"] = serde_json::json!("different file");
        store::atomic_json(&path, &proof).unwrap();
        assert!(reference_view(&e, d.path()).is_none());
    }

    #[test]
    fn real_recording_matches_published_code_while_carrier_and_noise_do_not() {
        let m: Manifest =
            serde_json::from_slice(&std::fs::read("data/recordings/manifest.json").unwrap())
                .unwrap();
        for r in &m.receivers {
            let t = kiwi::read(&Path::new("data/recordings").join(&r.file)).unwrap();
            let scores = match_samples(&t.resample(t.start(), FS as f64, FS * 12));
            eprintln!("PRN {}: {:?}", r.id, scores);
            assert!(scores.iter().filter(|v| **v >= 0.65).count() >= 5);
        }
        let tone = vec![Complex::new(1., 0.); FS * 12];
        assert!(match_samples(&tone).iter().all(|v| *v < 0.65));
        let mut seed = 7193u64;
        let noise: Vec<_> = (0..FS * 12)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                Complex::new(((seed >> 32) as i32) as f64 / i32::MAX as f64, 0.)
            })
            .collect();
        assert!(match_samples(&noise).iter().all(|v| *v < 0.65));
    }
}
