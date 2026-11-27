//! RF-only eligibility check for the public DCF77 time-reference commissioning channel.
//! PTB-Mitteilungen 114 (2004), pp. 353–354, Fig. 8: 9-stage LFSR, taps 5/9,
//! zero initial state with a start pulse, 512 chips, chip rate 77500/120 Hz.
//! Matching this code never supplies a delay, position or search region to TDoA.
use crate::{dsp, kiwi, model::*, pipeline, store};
use anyhow::{Result, ensure};
use rustfft::{FftPlanner, num_complex::Complex};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SOURCE: &str = "https://www.ptb.de/cms/fileadmin/internet/fachabteilungen/abteilung_4/4.4_zeit_und_frequenz/4.42/dcf77.pdf";
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
        "Profilul DCF77 este limitat la etalonul public de 77,5 kHz"
    );
    let root = manifest_path.parent().unwrap().canonicalize()?;
    let mut proof = vec![];
    for r in &m.receivers {
        let file = root.join(&r.file).canonicalize()?;
        ensure!(
            file.starts_with(&root) && std::fs::metadata(&file)?.len() <= 64 * 1024 * 1024,
            "Invalid file"
        );
        ensure!(pipeline::checksum(&file)? == r.sha256, "Checksum invalid");
        let t = kiwi::read(&file)?;
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
        &serde_json::json!({"profile":"dcf77_prn_v1","reference":SOURCE,"threshold":0.65,"minimum_windows":4,"required_fraction":0.8,"receivers":proof,"uses_transmitter_coordinates":false}),
    )?;
    if proof.len() >= 3 && proof.iter().all(|p| p.accepted) {
        m.emission_verified = true;
        m.provenance = Evidence {
            url: SOURCE.into(),
            note: format!(
                "DCF77 PRN modulation matched the PTB code in at least four 2 s windows and ≥80% of windows on {} independent receivers (normalized threshold 0.65). Per-SHA256 evidence is in verification.json. This public time reference is used for commissioning. Transmitter coordinates are not used.",
                proof.iter().filter(|p| p.accepted).count()
            ),
        };
        // The modulation establishes the public time-signal profile. Callsign assignment remains conservative.
        m.identity = None;
        store::atomic_json(manifest_path, &m)?;
    }
    Ok(proof)
}

#[cfg(test)]
mod tests {
    use super::*;
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
