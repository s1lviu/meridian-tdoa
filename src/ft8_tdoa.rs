//! FT8 TDoA from our own IQ. For every 15 s slot covered by the capture:
//! 1. decode FT8 at every synchronized receiver (USB audio derived from its IQ);
//! 2. associate receivers by identical 77-bit payload;
//! 3. rebuild the transmitted waveform from the payload and measure each receiver's arrival
//!    time with a matched filter — also at receivers that heard it too weakly to decode;
//! 4. localize from the arrival differences.
//! The locator inside a message is kept as post-hoc evaluation evidence and is never a solver
//! or receiver-selection input.
use crate::{ft8, geo, kiwi, model::*, pipeline, protocol, toa};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Datelike, Duration, SecondsFormat, Utc};
use rustfft::{FftPlanner, num_complex::Complex};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

pub const PROFILE: &str = "ft8_v1";
/// IQ centre above the dial frequency: the 0–3 kHz FT8 sub-band then sits at −1.5…+1.5 kHz.
pub const IQ_OFFSET_HZ: f64 = 1500.;
const FS: f64 = 12000.;
const SLOT_SAMPLES: usize = 180_000; // 15 s at 12 kHz
const FS_TOA: f64 = 400.;
const TOA_SAMPLES: usize = 6000; // 15 s at 400 Hz
/// Matched-filter significance (noise σ above the noise mean) required at a receiver that
/// did not decode the message itself. Noise-only maxima stay below 6 in the TOA tests.
pub const DETECTION_SIGMA: f64 = 10.;
/// Required at a receiver that decoded the message, before its arrival time is used.
pub const TIMING_SIGMA: f64 = 5.;
/// Coherent segments of the 12.64 s waveform (≈2.1 s each), combined non-coherently.
const SEGMENTS: usize = 6;
const TIMING_FLOOR_US: f64 = 10.;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiverArrival {
    pub receiver_id: String,
    pub sha256: String,
    pub decoded: bool,
    pub decoder_score: Option<i32>,
    pub arrival: Option<toa::Arrival>,
    /// First-symbol arrival, seconds after the slot start (UTC), when timed.
    pub arrival_s: Option<f64>,
    pub sigma_us: Option<f64>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub slot_utc: String,
    pub message: String,
    pub payload_hex: String,
    pub transmitter: Option<String>,
    /// Locator sent in the message: post-hoc evaluation only, never a solver input.
    pub grid: Option<String>,
    pub audio_hz: f64,
    pub receivers: Vec<ReceiverArrival>,
}

struct Stream {
    receiver: Receiver,
    iq: kiwi::Track,
    week_utc: i64,
}

pub struct Analysis {
    pub emissions: Vec<Emission>,
    pub captured_receivers: usize,
    pub sampled_slots: usize,
    pub decodes: usize,
    /// Messages decoded by each synchronized receiver over all covered slots.
    pub receiver_decodes: Vec<(String, usize)>,
}

impl Analysis {
    pub fn summary(&self, capture_id: &str) -> serde_json::Value {
        let located = self
            .emissions
            .iter()
            .filter(|e| e.estimate.is_some())
            .count();
        let timed = self
            .emissions
            .iter()
            .filter(|e| e.receivers.iter().filter(|r| r.confirmed).count() >= 2)
            .count();
        let ids: Vec<_> = self.emissions.iter().map(|e| &e.id).collect();
        serde_json::json!({"status":"completed","capture_id":capture_id,"profile":PROFILE,
            "sampled_slots":self.sampled_slots,"decodes":self.decodes,"messages":self.emissions.len(),
            "multi_receiver":timed,"located":located,"captured_receivers":self.captured_receivers,
            "emission_id":ids.first(),"emission_ids":ids})
    }
}

fn spectrum(planner: &mut FftPlanner<f64>, x: &[Complex<f64>]) -> Vec<Complex<f64>> {
    let mut s = x.to_vec();
    planner.plan_fft_forward(s.len()).process(&mut s);
    s
}

fn taper(f: f64, lo: f64, hi: f64, edge: f64) -> f64 {
    if f < lo - edge || f > hi + edge {
        0.
    } else if f < lo {
        0.5 - 0.5 * (std::f64::consts::PI * (f - (lo - edge)) / edge).cos()
    } else if f > hi {
        0.5 + 0.5 * (std::f64::consts::PI * (f - hi) / edge).cos()
    } else {
        1.
    }
}

/// Upper-sideband audio (dial at 0 Hz) from the slot spectrum of IQ centred `IQ_OFFSET_HZ` above.
fn usb_audio(planner: &mut FftPlanner<f64>, spec: &[Complex<f64>]) -> Vec<f32> {
    let n = spec.len();
    let per_hz = n as f64 / FS; // bins per Hz
    let shift = (IQ_OFFSET_HZ * per_hz).round() as usize;
    let mut a = vec![Complex::new(0., 0.); n];
    for (k, slot) in a.iter_mut().enumerate().take(n / 2) {
        let f = k as f64 / per_hz;
        let w = taper(f, 150., 3300., 50.);
        if w > 0. {
            *slot = spec[(k + n - shift) % n] * w;
        }
    }
    planner.plan_fft_inverse(n).process(&mut a);
    a.iter().map(|z| (z.re / n as f64) as f32).collect()
}

/// 400 Hz complex baseband with FT8 tone 0 at 0 Hz, from the 12 kHz slot spectrum.
fn baseband(
    planner: &mut FftPlanner<f64>,
    spec: &[Complex<f64>],
    tone0_bb_hz: f64,
) -> Vec<Complex<f64>> {
    let n = spec.len();
    let per_hz = n as f64 / FS;
    let k0 = (tone0_bb_hz * per_hz).round() as i64;
    let mut y = vec![Complex::new(0., 0.); TOA_SAMPLES];
    for (j, slot) in y.iter_mut().enumerate() {
        let jj = if j < TOA_SAMPLES / 2 {
            j as i64
        } else {
            j as i64 - TOA_SAMPLES as i64
        };
        let delta = jj as f64 * FS_TOA / TOA_SAMPLES as f64;
        let w = taper(delta, -20., 64., 10.);
        if w > 0. {
            let k = (k0 + jj).rem_euclid(n as i64) as usize;
            *slot = spec[k] * w;
        }
    }
    planner.plan_fft_inverse(TOA_SAMPLES).process(&mut y);
    y
}

fn hex(p: &[u8]) -> String {
    p.iter().map(|b| format!("{b:02x}")).collect()
}

fn utc(s: i64) -> String {
    DateTime::from_timestamp(s, 0)
        .unwrap()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// One receiver alone: decode a 15 s slot of 12 kHz IQ (centred `IQ_OFFSET_HZ` above the dial)
/// and time every decoded message against its rebuilt waveform. Arrival seconds are from the
/// slot buffer start, in that receiver's own clock. Used by edge nodes (`meridian-node`).
pub fn time_slot(iq: &[Complex<f64>]) -> Vec<(ft8::Decode, toa::Arrival)> {
    let mut planner = FftPlanner::new();
    let spec = spectrum(&mut planner, iq);
    let found = ft8::decode(&usb_audio(&mut planner, &spec), FS as u32);
    let freqs: Vec<f64> = (-14..=14).map(|k| k as f64 * 0.25).collect();
    found
        .into_iter()
        .filter_map(|d| {
            let template = ft8::synthesize_complex(&d.tones, 0., FS_TOA);
            let max_lag = TOA_SAMPLES.checked_sub(template.len())?;
            let expect = (d.time_s - ft8::DECODER_LAG_S) * FS_TOA;
            let half = (ft8::DECODER_TOLERANCE_S + 0.05) * FS_TOA;
            let lo = (expect - half).floor().max(0.) as usize;
            let hi = ((expect + half).ceil() as usize).min(max_lag);
            let noise: Vec<usize> = (0..=max_lag)
                .filter(|l| (*l as f64 - expect).abs() > 160.)
                .step_by(3)
                .collect();
            let y = baseband(&mut planner, &spec, d.freq_hz - IQ_OFFSET_HZ);
            let a = toa::matched(&y, &template, FS_TOA, lo, hi, &freqs, SEGMENTS, &noise)?;
            (a.significance() >= TIMING_SIGMA).then_some((d, a))
        })
        .collect()
}

pub const SLOT_SAMPLES_12K: usize = SLOT_SAMPLES;
pub const TOA_RATE_HZ: f64 = FS_TOA;

/// Edge-node processing of one SigMF recording centred `IQ_OFFSET_HZ` above an FT8 dial:
/// arrival-time reports of every decoded message, in the node's own clock (UTC seconds as the
/// node's time base states them). A slot and its 77-bit payload identify the emission.
pub fn node_detections(meta: &Path, node: &str) -> Result<(f64, Vec<crate::sync::Detection>)> {
    let r = crate::sigmf::read_local(meta)?;
    ensure!(
        r.receiver_id == node,
        "Recording belongs to {}, not {node}",
        r.receiver_id
    );
    let dial = r.frequency_hz - IQ_OFFSET_HZ;
    ensure!(
        crate::bands::name(dial).is_some(),
        "Recording is not centred on an amateur FT8 sub-band"
    );
    let track = r.to_track(FS, 0., r.epoch_utc, 0)?;
    let (start, end) = (track.start(), track.end());
    let mut out = vec![];
    let mut slot = ((r.epoch_utc as f64 + start) / 15.).ceil() as i64 * 15;
    ft8::reset_hashes();
    while slot as f64 + 15. <= r.epoch_utc as f64 + end {
        let iq = track.resample((slot - r.epoch_utc) as f64, FS, SLOT_SAMPLES);
        for (d, a) in time_slot(&iq) {
            out.push(crate::sync::Detection {
                node: node.into(),
                signal: format!("ft8-{}-{slot}", hex(&d.payload)),
                transmitter: d.transmitter(),
                free_running: r.time_source == "free_running",
                local_s: slot as f64 + a.lag / FS_TOA,
                sigma_s: (a.lag_sigma / FS_TOA)
                    .max(TIMING_FLOOR_US * 1e-6)
                    .hypot(r.time_sigma_ns * 1e-9),
            });
        }
        slot += 15;
    }
    Ok((dial, out))
}

pub fn analyze(path: &Path) -> Result<Analysis> {
    let m: Manifest = serde_json::from_slice(&std::fs::read(path)?)?;
    pipeline::validate_manifest(&m)?;
    ensure!(
        m.mode == "live"
            && m.kind == "amateur_digital"
            && m.capture_profile.as_deref() == Some(PROFILE),
        "FT8 analysis requires an FT8 live capture"
    );
    let center = m.iq_center_hz.context("Missing IQ centre")?;
    ensure!(
        (center - m.frequency_hz - IQ_OFFSET_HZ).abs() < 0.1,
        "Unexpected FT8 IQ centre"
    );
    let root = path
        .parent()
        .context("Missing capture directory")?
        .canonicalize()?;
    let anchor = DateTime::parse_from_rfc3339(&m.capture_utc)?.with_timezone(&Utc);
    let gps = anchor + Duration::seconds(m.gps_utc_offset_s);
    let week = (gps.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc()
        - Duration::days(gps.weekday().num_days_from_sunday() as i64))
    .timestamp()
        - m.gps_utc_offset_s;
    let mut streams = vec![];
    let mut excluded = vec![];
    let mut total = 0u64;
    for r in &m.receivers {
        let file = root.join(&r.file).canonicalize()?;
        total = total.saturating_add(file.metadata()?.len());
        ensure!(
            file.starts_with(&root)
                && total <= 64 * 1024 * 1024
                && pipeline::checksum(&crate::iq::hashed_file(&file))? == r.sha256,
            "Invalid IQ path, size or hash"
        );
        let iq = match crate::iq::read(&file, &m) {
            Ok(iq) => iq,
            Err(err) => {
                excluded.push(serde_json::json!({"id":r.id,"reason":err.to_string()}));
                continue;
            }
        };
        let week_utc = week
            + (((anchor.timestamp() - week) as f64 - iq.start()) / 604800.).round() as i64 * 604800;
        if iq.sync_error.is_some()
            || ((week_utc - anchor.timestamp()) as f64 + iq.start()).abs() > 180.
        {
            excluded.push(serde_json::json!({"id":r.id,"reason":iq.sync_error.as_deref().unwrap_or("GPS week does not match capture UTC")}));
            continue;
        }
        streams.push(Stream {
            receiver: r.clone(),
            iq,
            week_utc,
        });
    }
    let start = streams
        .iter()
        .map(|s| s.week_utc as f64 + s.iq.start())
        .fold(f64::INFINITY, f64::min);
    let end = streams
        .iter()
        .map(|s| s.week_utc as f64 + s.iq.end())
        .fold(f64::NEG_INFINITY, f64::max);
    let mut emissions = vec![];
    let mut evidence_log = vec![];
    let mut per_receiver: BTreeMap<String, usize> = BTreeMap::new();
    let (mut sampled, mut decodes) = (0, 0);
    let mut planner = FftPlanner::new();
    if start.is_finite() && end - start <= 180. {
        ft8::reset_hashes();
        let mut slot = (start / 15.).ceil() as i64 * 15;
        while slot as f64 + 15. <= end {
            // Per receiver: slot IQ spectrum and its own decodes.
            let mut covered = vec![];
            for s in &streams {
                let local = (slot - s.week_utc) as f64;
                if s.iq.start() > local || s.iq.end() < local + 15. {
                    continue;
                }
                let iq = s.iq.resample(local, FS, SLOT_SAMPLES);
                let spec = spectrum(&mut planner, &iq);
                let found = ft8::decode(&usb_audio(&mut planner, &spec), FS as u32);
                decodes += found.len();
                *per_receiver.entry(s.receiver.id.clone()).or_default() += found.len();
                covered.push((s, spec, found));
            }
            if covered.is_empty() {
                slot += 15;
                continue;
            }
            sampled += 1;
            evidence_log.push(serde_json::json!({"slot_utc":utc(slot),"receivers":covered.iter().map(|(s,_,d)| serde_json::json!({
                "id":s.receiver.id,"sha256":s.receiver.sha256,"decodes":d.iter().map(|x| serde_json::json!({"text":x.text,"audio_hz":x.freq_hz,"time_s":x.time_s,"score":x.score})).collect::<Vec<_>>()})).collect::<Vec<_>>()}));
            let mut messages: BTreeMap<[u8; 10], Vec<(usize, &ft8::Decode)>> = BTreeMap::new();
            for (i, (_, _, found)) in covered.iter().enumerate() {
                for d in found {
                    messages.entry(d.payload).or_default().push((i, d));
                }
            }
            for (payload, heard) in messages {
                let first = heard[0].1;
                let mut audio: Vec<f64> = heard.iter().map(|(_, d)| d.freq_hz).collect();
                audio.sort_by(f64::total_cmp);
                let audio_hz = audio[audio.len() / 2];
                let template = ft8::synthesize_complex(&first.tones, 0., FS_TOA);
                let mut lags: Vec<f64> = heard
                    .iter()
                    .map(|(_, d)| (d.time_s - ft8::DECODER_LAG_S) * FS_TOA)
                    .collect();
                lags.sort_by(f64::total_cmp);
                let center_lag = lags[lags.len() / 2];
                let freqs: Vec<f64> = (-14..=14).map(|k| k as f64 * 0.25).collect();
                let max_lag = TOA_SAMPLES - template.len();
                let mut arrivals = vec![];
                for (i, (s, spec, _)) in covered.iter().enumerate() {
                    let own = heard.iter().find(|(j, _)| *j == i).map(|(_, d)| *d);
                    let (expect, half) = match own {
                        Some(d) => (
                            (d.time_s - ft8::DECODER_LAG_S) * FS_TOA,
                            (ft8::DECODER_TOLERANCE_S + 0.05) * FS_TOA,
                        ),
                        // Unknown at this receiver: allow the largest terrestrial propagation difference too.
                        None => (
                            center_lag,
                            (ft8::DECODER_TOLERANCE_S + 0.05 + 0.07) * FS_TOA,
                        ),
                    };
                    let lo = (expect - half).floor().max(0.) as usize;
                    let hi = ((expect + half).ceil() as usize).min(max_lag);
                    let noise: Vec<usize> = (0..=max_lag)
                        .filter(|l| (*l as f64 - expect).abs() > 160.)
                        .step_by(3)
                        .collect();
                    let y = baseband(&mut planner, spec, audio_hz - IQ_OFFSET_HZ);
                    let found = (lo <= hi)
                        .then(|| {
                            toa::matched(&y, &template, FS_TOA, lo, hi, &freqs, SEGMENTS, &noise)
                        })
                        .flatten();
                    let need = if own.is_some() {
                        TIMING_SIGMA
                    } else {
                        DETECTION_SIGMA
                    };
                    let timed = found.filter(|a| a.significance() >= need && a.noise_sd > 0.);
                    arrivals.push(ReceiverArrival {
                        receiver_id: s.receiver.id.clone(),
                        sha256: s.receiver.sha256.clone(),
                        decoded: own.is_some(),
                        decoder_score: own.map(|d| d.score),
                        arrival: found,
                        arrival_s: timed.map(|a| a.lag / FS_TOA),
                        sigma_us: timed.map(|a| {
                            (a.lag_sigma / FS_TOA * 1e6)
                                .max(TIMING_FLOOR_US)
                                .hypot(s.receiver.timing_sigma_ns.unwrap_or(0.) / 1000.)
                        }),
                        reason: timed.is_none().then(|| {
                            if own.is_some() {
                                "Decoded, but the matched-filter peak is too weak to time".into()
                            } else {
                                "Waveform not detected above the noise reference".into()
                            }
                        }),
                    });
                }
                let receivers: Vec<RxResult> = covered
                    .iter()
                    .zip(&arrivals)
                    .map(|((s, _, _), a)| RxResult {
                        receiver: s.receiver.clone(),
                        confirmed: a.arrival_s.is_some(),
                        used: false,
                        synchronized: true,
                        reason: a.reason.clone(),
                        gps_age_max_s: s.iq.max_age,
                        sample_rate_hz: s.iq.actual_fs,
                        // Excess matched-filter power over the off-peak noise reference.
                        signal_score_db: a
                            .arrival
                            .filter(|x| x.noise_mean > 0. && x.peak > x.noise_mean)
                            .map_or(0., |x| 10. * (x.peak / x.noise_mean - 1.).log10()),
                        samples: SLOT_SAMPLES,
                    })
                    .collect();
                let mut pairs = vec![];
                for i in 0..arrivals.len() {
                    for j in i + 1..arrivals.len() {
                        let (Some(ta), Some(tb)) = (arrivals[i].arrival_s, arrivals[j].arrival_s)
                        else {
                            continue;
                        };
                        let (sa, sb) =
                            (arrivals[i].sigma_us.unwrap(), arrivals[j].sigma_us.unwrap());
                        let physical = geo::distance(
                            receivers[i].receiver.position,
                            receivers[j].receiver.position,
                        ) / geo::C_KM_S
                            * 1e6;
                        let delay = (tb - ta) * 1e6;
                        let (pa, pb) = (arrivals[i].arrival.unwrap(), arrivals[j].arrival.unwrap());
                        pairs.push(Pair {
                            a: receivers[i].receiver.id.clone(),
                            b: receivers[j].receiver.id.clone(),
                            delay_us: delay,
                            sigma_us: (sa * sa + sb * sb).sqrt(),
                            coherence: pa.peak.min(pb.peak),
                            peak_ratio: pa.significance().min(pb.significance()),
                            windows: SEGMENTS,
                            accepted: delay.abs() <= physical + 3. * (sa * sa + sb * sb).sqrt(),
                            correlation: vec![],
                        });
                    }
                }
                let transmitter = first.transmitter();
                let evidence = Evidence {
                    slot_utc: utc(slot),
                    message: first.text.clone(),
                    payload_hex: hex(&payload),
                    transmitter: transmitter.clone(),
                    grid: first.grid(),
                    audio_hz,
                    receivers: arrivals.clone(),
                };
                let heard_by = heard.len();
                let provenance = crate::model::Evidence {
                    url: "https://wsjt.sourceforge.io/FT4_FT8_QEX.pdf".into(),
                    note: format!(
                        "FT8 message decoded from this capture's own IQ at {heard_by} receiver(s) (CRC-14 and LDPC checked, ft8_lib); arrival times measured against the waveform rebuilt from the decoded payload. Per-receiver evidence is in ft8-verification.json."
                    ),
                };
                let mut e = Emission {
                    id: format!("{}-{slot}-{}", m.id, hex(&payload)),
                    dataset_id: m.id.clone(),
                    mode: "live".into(),
                    fault_injected: false,
                    title: format!(
                        "FT8 · {}",
                        crate::bands::name(m.frequency_hz).unwrap_or("HF")
                    ),
                    frequency_hz: m.frequency_hz + audio_hz + 3.5 * ft8::TONE_SPACING_HZ,
                    start_utc: utc(slot),
                    end_utc: utc(slot + 15),
                    status: "undetermined".into(),
                    reason: None,
                    identity: transmitter.map(|callsign| Identity {
                        callsign,
                        evidence: provenance.clone(),
                    }),
                    provenance,
                    receivers,
                    pairs,
                    estimate: None,
                    hypotheses: vec![],
                    detected: true,
                    spectrum: vec![],
                    processed_at: Utc::now().to_rfc3339(),
                    reference: None,
                    excluded: vec![],
                };
                pipeline::locate_receptions(&mut e);
                if e.estimate.is_some() {
                    e.reason = None;
                }
                e.reference = Some(VerifiedReference {
                    node_utc: None,
                    profile: PROFILE.into(),
                    three_receivers: protocol::comparison(&e),
                    cw: vec![],
                    ft8: Some(evidence),
                });
                emissions.push(e);
            }
            slot += 15;
        }
    }
    crate::store::atomic_json(
        &root.join("ft8-verification.json"),
        &serde_json::json!({"profile":PROFILE,"dial_hz":m.frequency_hz,"iq_center_hz":center,
            "excluded":excluded,"slots":evidence_log,"uses_transmitter_coordinates":false}),
    )?;
    Ok(Analysis {
        emissions,
        captured_receivers: m.receivers.len(),
        sampled_slots: sampled,
        decodes,
        receiver_decodes: per_receiver.into_iter().collect(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Write a Kiwi IQ WAV (stereo PCM16, GNSS chunk before every 512-sample block).
    pub(crate) fn write_kiwi_wav(path: &Path, gps_sow: f64, fs: f64, iq: &[Complex<f64>]) {
        let mut body = vec![];
        body.extend_from_slice(b"WAVE");
        let mut fmt = vec![];
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&12000u32.to_le_bytes());
        fmt.extend_from_slice(&(12000u32 * 4).to_le_bytes());
        fmt.extend_from_slice(&4u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        body.extend_from_slice(&fmt);
        for (k, block) in iq.chunks(512).enumerate() {
            let t = gps_sow + (k * 512) as f64 / fs;
            let mut g = vec![0u8; 10];
            g[2..6].copy_from_slice(&(t.floor() as u32).to_le_bytes());
            g[6..10].copy_from_slice(&(((t - t.floor()) * 1e9) as u32).to_le_bytes());
            body.extend_from_slice(b"kiwi");
            body.extend_from_slice(&10u32.to_le_bytes());
            body.extend_from_slice(&g);
            body.extend_from_slice(b"data");
            body.extend_from_slice(&((block.len() * 4) as u32).to_le_bytes());
            for z in block {
                for v in [z.re, z.im] {
                    body.extend_from_slice(
                        &((v * 32767.).clamp(-32767., 32767.) as i16).to_le_bytes(),
                    );
                }
            }
        }
        let mut file = b"RIFF".to_vec();
        file.extend_from_slice(&(body.len() as u32).to_le_bytes());
        file.extend_from_slice(&body);
        std::fs::write(path, file).unwrap();
    }

    #[test]
    fn synthetic_network_decodes_associates_times_and_locates_an_ft8_station() {
        let dir = tempfile::tempdir().unwrap();
        let truth = Point {
            lat: 47.0,
            lon: 10.0,
        };
        let sites = [
            Point {
                lat: 49.5,
                lon: 6.0,
            },
            Point {
                lat: 44.8,
                lon: 7.5,
            },
            Point {
                lat: 45.5,
                lon: 14.2,
            },
            Point {
                lat: 49.8,
                lon: 13.5,
            },
            Point {
                lat: 47.6,
                lon: 8.6,
            },
        ];
        // A real payload decoded from the upstream reference recording.
        let audio = {
            let bytes = include_bytes!("../third_party/ft8_lib/test/191111_110130.wav");
            let i = bytes.windows(4).position(|w| w == b"data").unwrap();
            bytes[i + 8..]
                .chunks_exact(2)
                .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.)
                .collect::<Vec<_>>()
        };
        ft8::reset_hashes();
        let reference = ft8::decode(&audio, 12000)
            .into_iter()
            .find(|d| d.text == "CQ TA6CQ KN70")
            .unwrap();
        let capture = DateTime::parse_from_rfc3339("2026-11-27T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let gps = capture + Duration::seconds(18);
        let week_gps = gps.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc()
            - Duration::days(gps.weekday().num_days_from_sunday() as i64);
        let slot = capture.timestamp() + 15; // first full slot inside the capture
        let tone0_bb = 1200. - IQ_OFFSET_HZ;
        let wave = ft8::synthesize_complex(&reference.tones, tone0_bb, FS);
        let mut seed = 5u64;
        let mut u = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let mut receivers = vec![];
        for (i, p) in sites.iter().enumerate() {
            let fs = 12000. + 1.3 * i as f64; // independent sample clocks, as on real Kiwis
            let start_utc = capture.timestamp() as f64 + 2.; // recording begins 2 s after the anchor
            let n = (32. * fs) as usize;
            let delay = geo::distance(truth, *p) / geo::C_KM_S;
            let emit = slot as f64 + 0.5;
            let iq: Vec<Complex<f64>> = (0..n)
                .map(|k| {
                    let t = start_utc + k as f64 / fs;
                    let x = (t - emit - delay) * FS; // index into the 12 kHz waveform
                    let s = if x > -20. && x < wave.len() as f64 + 20. {
                        crate::dsp::sinc_at(&wave, x) * 0.05
                    } else {
                        Complex::new(0., 0.)
                    };
                    let (a, b) = (u(), u());
                    s + Complex::from_polar((-2. * a.ln()).sqrt() * 0.01, std::f64::consts::TAU * b)
                })
                .collect();
            let file = format!("rx{i}.wav");
            let sow = (start_utc + 18.) - week_gps.timestamp() as f64;
            write_kiwi_wav(&dir.path().join(&file), sow, fs, &iq);
            receivers.push(Receiver {
                id: format!("rx{i}"),
                label: format!("synthetic {i}"),
                independent_device: format!("rx{i}"),
                position: *p,
                position_evidence: crate::model::Evidence {
                    url: "https://example.org/synthetic".into(),
                    note: "synthetic test".into(),
                },
                position_uncertainty_m: 0.,
                sha256: pipeline::checksum(&dir.path().join(&file)).unwrap(),
                timing_sigma_ns: None,
                file,
            });
        }
        let m = Manifest {
            id: "live-synthetic".into(),
            title: "synthetic".into(),
            kind: "amateur_digital".into(),
            mode: "live".into(),
            frequency_hz: 14_074_000.,
            capture_profile: Some(PROFILE.into()),
            iq_center_hz: Some(14_074_000. + IQ_OFFSET_HZ),
            capture_utc: capture.to_rfc3339(),
            gps_utc_offset_s: 18,
            provenance: crate::model::Evidence {
                url: "https://example.org/synthetic".into(),
                note: "synthetic test".into(),
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
            .find(|e| e.identity.as_ref().is_some_and(|i| i.callsign == "TA6CQ"))
            .expect("decoded");
        let ev = e.reference.as_ref().unwrap().ft8.as_ref().unwrap();
        assert_eq!(ev.grid.as_deref(), Some("KN70"));
        assert_eq!(e.receivers.iter().filter(|r| r.confirmed).count(), 5);
        for p in &e.pairs {
            let (a, b) = (
                sites[p.a[2..].parse::<usize>().unwrap()],
                sites[p.b[2..].parse::<usize>().unwrap()],
            );
            let model = (geo::distance(truth, b) - geo::distance(truth, a)) / geo::C_KM_S * 1e6;
            assert!(
                (p.delay_us - model).abs() < 50.,
                "pair {}-{} error {} µs",
                p.a,
                p.b,
                p.delay_us - model
            );
        }
        let est = e.estimate.as_ref().expect("unique fix from five receivers");
        let error = geo::distance(est.center, truth);
        eprintln!(
            "synthetic FT8 fix error {error:.2} km, 95% radius {:.1} km, pairs {}",
            est.radius_km,
            e.pairs.len()
        );
        assert!(error < 15., "error {error} km");
        assert!(crate::calibration::contains(&est.polygon, truth));
    }
}

#[cfg(test)]
mod node_tests {
    use super::*;
    /// GNSS-disciplined edge nodes record FT8 as SigMF at 48 kSPS, decode and time it locally,
    /// and only arrival times reach the server, which associates and locates the station.
    #[test]
    fn edge_nodes_report_ft8_arrivals_that_the_server_locates() {
        let dir = tempfile::tempdir().unwrap();
        let truth = Point {
            lat: 47.0,
            lon: 10.0,
        };
        let sites = [
            Point {
                lat: 49.5,
                lon: 6.0,
            },
            Point {
                lat: 44.8,
                lon: 7.5,
            },
            Point {
                lat: 45.5,
                lon: 14.2,
            },
            Point {
                lat: 49.8,
                lon: 13.5,
            },
            Point {
                lat: 47.6,
                lon: 8.6,
            },
        ];
        let audio = {
            let bytes = include_bytes!("../third_party/ft8_lib/test/191111_110130.wav");
            let i = bytes.windows(4).position(|w| w == b"data").unwrap();
            bytes[i + 8..]
                .chunks_exact(2)
                .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.)
                .collect::<Vec<_>>()
        };
        ft8::reset_hashes();
        let reference = ft8::decode(&audio, 12000)
            .into_iter()
            .find(|d| d.text == "CQ TA6CQ KN70")
            .unwrap();
        let fs = 48_000.;
        let wave = ft8::synthesize_complex(&reference.tones, 900. - IQ_OFFSET_HZ, fs);
        let slot = 1_795_000_005i64 - 1_795_000_005 % 15 + 15; // a 15 s boundary
        let start = slot - 3; // recordings begin 3 s before it
        let mut seed = 17u64;
        let mut u = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let mut nodes = vec![];
        let mut seen = vec![];
        for (i, p) in sites.iter().enumerate() {
            let delay = geo::distance(truth, *p) / geo::C_KM_S;
            let n = (fs * 20.) as usize;
            let x: Vec<Complex<f64>> = (0..n)
                .map(|k| {
                    let t = start as f64 + k as f64 / fs;
                    let pos = (t - slot as f64 - 0.5 - delay) * fs;
                    let s = if pos > -40. && pos < wave.len() as f64 + 40. {
                        crate::dsp::sinc_at(&wave, pos) * 0.05
                    } else {
                        Complex::default()
                    };
                    let (a, b) = (u(), u());
                    s + Complex::from_polar(
                        (-2. * a.ln()).sqrt() * 0.005,
                        std::f64::consts::TAU * b,
                    )
                })
                .collect();
            let anchors: Vec<(u64, String)> = (0..20u64)
                .map(|s| {
                    (
                        s * fs as u64,
                        chrono::DateTime::from_timestamp(start + s as i64, 0)
                            .unwrap()
                            .to_rfc3339_opts(SecondsFormat::Nanos, true),
                    )
                })
                .collect();
            let meta = dir.path().join(format!("node{i}.sigmf-meta"));
            crate::sigmf::write(
                &meta,
                &format!("node{i}"),
                *p,
                fs,
                14_074_000. + IQ_OFFSET_HZ,
                "gnss_pps",
                50.,
                &anchors,
                &x,
            )
            .unwrap();
            let (dial, detections) = node_detections(&meta, &format!("node{i}")).unwrap();
            assert_eq!(dial, 14_074_000.);
            assert_eq!(detections.len(), 1, "node {i}: {detections:?}");
            seen.extend(detections);
            nodes.push(crate::model::Node {
                id: format!("node{i}"),
                label: format!("node {i}"),
                position: *p,
                position_evidence: crate::model::Evidence {
                    url: "https://example.org".into(),
                    note: "synthetic".into(),
                },
                token_sha256: String::new(),
                disciplined: true,
            });
        }
        assert!(seen.iter().all(|d| d.signal == seen[0].signal
            && d.signal.ends_with(&format!("-{slot}"))
            && d.transmitter.as_deref() == Some("TA6CQ")));
        let disciplined = nodes.iter().map(|n| n.id.clone()).collect();
        let arrivals = crate::sync::Network::default().corrected_with(&seen, &disciplined);
        let e = crate::sync::emission(&seen[0].signal, &arrivals, &nodes, 14_074_000., Utc::now());
        let est = e.estimate.expect("located from five edge reports");
        let err = geo::distance(est.center, truth);
        eprintln!(
            "edge FT8 reports: error {err:.2} km, 95% radius {:.1} km",
            est.radius_km
        );
        assert!(err < 15.);
    }
}
