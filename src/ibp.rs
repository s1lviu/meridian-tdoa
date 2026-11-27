//! Public NCDXF/IARU beacon schedule. Transmitter coordinates are not inputs.
mod cw;
use crate::{dsp, geo, kiwi, model::*, pipeline, protocol, store};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Datelike, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
pub const PROFILE: &str = "ncdxf_cw_v1";
pub const SOURCE: &str = "https://www.ncdxf.org/beacon/";
pub const CHANNELS: [(&str, u32); 5] = [
    ("20m", 14_100_000),
    ("17m", 18_110_000),
    ("15m", 21_150_000),
    ("12m", 24_930_000),
    ("10m", 28_200_000),
];
pub const CALLS: [&str; 18] = [
    "4U1UN", "VE8AT", "W6WX", "KH6RS", "ZL6B", "VK6RBP", "JA2IGY", "RR9O", "VR2B", "4S7B", "ZS6DN",
    "5Z4B", "4X6TU", "OH2B", "CS3B", "LU4AA", "OA4B", "YV5B",
];
pub fn band(frequency: f64) -> Option<usize> {
    CHANNELS.iter().position(|(_, hz)| *hz as f64 == frequency)
}
pub fn scheduled(utc: i64, band: usize) -> &'static str {
    CALLS[(utc.div_euclid(10) - band as i64).rem_euclid(18) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_band_repeats_all_eighteen_slots_at_utc_boundaries() {
        for (i, (_, hz)) in CHANNELS.iter().enumerate() {
            assert_eq!(band(*hz as f64), Some(i));
            for (j, call) in CALLS.iter().enumerate() {
                let start = 86400 + 10 * (i + j) as i64;
                assert_eq!(scheduled(start, i), *call);
                assert_eq!(scheduled(start + 9, i), *call);
                assert_eq!(scheduled(start + 180, i), *call);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiverEvidence {
    pub receiver_id: String,
    pub sha256: String,
    #[serde(flatten)]
    pub cw: cw::Match,
}
struct Stream {
    receiver: Receiver,
    iq: kiwi::Track,
    week_utc: i64,
}

pub struct Analysis {
    pub emissions: Vec<Emission>,
    pub captured_receivers: usize,
}
impl Analysis {
    pub fn summary(&self, capture_id: &str) -> serde_json::Value {
        let verified: Vec<_> = self
            .emissions
            .iter()
            .filter(|e| e.identity.is_some())
            .map(|e| &e.id)
            .collect();
        serde_json::json!({"status":"completed","capture_id":capture_id,"sampled_slots":self.emissions.len(),
            "verified_slots":verified.len(),"captured_receivers":self.captured_receivers,
            "emission_id":verified.first(),"emission_ids":verified})
    }
    pub fn save(&self, dir: &Path, capture_id: &str) -> Result<()> {
        // One atomic bundle prevents a partially published multi-transmitter capture.
        store::atomic_json(
            &dir.join(format!("{capture_id}-slots.json")),
            &self.emissions,
        )
    }
}

pub fn analyze(path: &Path) -> Result<Analysis> {
    let m: Manifest = serde_json::from_slice(&std::fs::read(path)?)?;
    pipeline::validate_manifest(&m)?;
    ensure!(
        m.mode == "live"
            && m.kind == "public_beacon"
            && m.capture_profile.as_deref() == Some(PROFILE),
        "IBP analysis requires an IBP live capture"
    );
    let channel = band(m.frequency_hz).context("Unknown IBP channel")?;
    let center = m.iq_center_hz.context("Missing IQ centre")?;
    ensure!(
        (m.frequency_hz - center - 700.).abs() < 0.1,
        "Unexpected IBP IQ centre"
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
    let mut total_bytes = 0u64;
    for r in &m.receivers {
        let file = root.join(&r.file).canonicalize()?;
        total_bytes = total_bytes.saturating_add(file.metadata()?.len());
        ensure!(
            file.starts_with(&root)
                && total_bytes <= 64 * 1024 * 1024
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
        // A receiver can connect on either side of the GPS week boundary.
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
    let patterns = cw::templates();
    let mut emissions = vec![];
    if start.is_finite() && end - start <= 180. {
        let mut slot = (start / 10.).floor() as i64 * 10;
        while slot as f64 + 10. <= end {
            let call = scheduled(slot, channel);
            let mut receivers = vec![];
            let mut decoded = vec![];
            let mut proof = vec![];
            let mut spectrum = vec![];
            for s in &streams {
                let local = (slot - s.week_utc) as f64 - cw::LEAD_MS as f64 / 1000.;
                if s.iq.start() > local || s.iq.end() < (slot + 10 - s.week_utc) as f64 {
                    continue;
                }
                let samples = (10000 + cw::LEAD_MS) * 12;
                let raw = s.iq.resample(local, 12000., samples);
                let rx = cw::identify(&raw, call, m.frequency_hz - center, &patterns);
                let (mut spec, score) = dsp::spectrum(&raw[cw::LEAD_MS * 12..], 12000.);
                for bin in &mut spec {
                    bin[0] += center - m.frequency_hz;
                }
                if spectrum.is_empty()
                    || (rx.evidence.accepted
                        && !decoded.iter().any(|v: &cw::Reception| v.evidence.accepted))
                {
                    spectrum = spec;
                }
                proof.push(ReceiverEvidence {
                    receiver_id: s.receiver.id.clone(),
                    sha256: s.receiver.sha256.clone(),
                    cw: rx.evidence.clone(),
                });
                receivers.push(RxResult {
                    receiver: s.receiver.clone(),
                    confirmed: rx.evidence.accepted,
                    used: false,
                    synchronized: true,
                    reason: (!rx.evidence.accepted)
                        .then(|| "Scheduled CW callsign unverified in this IQ".into()),
                    gps_age_max_s: s.iq.max_age,
                    sample_rate_hz: s.iq.actual_fs,
                    signal_score_db: score,
                    samples: 120000,
                });
                decoded.push(rx);
            }
            if !receivers.is_empty() {
                let mut pairs = vec![];
                for (i, a) in decoded
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.evidence.accepted)
                {
                    for (j, b) in decoded
                        .iter()
                        .enumerate()
                        .skip(i + 1)
                        .filter(|(_, r)| r.evidence.accepted)
                    {
                        let ra = &receivers[i].receiver;
                        let rb = &receivers[j].receiver;
                        pairs.push(cw::delay(
                            &ra.id,
                            &rb.id,
                            a,
                            b,
                            geo::distance(ra.position, rb.position) / geo::C_KM_S,
                        ));
                    }
                }
                let heard = receivers.iter().filter(|r| r.confirmed).count();
                let evidence = Evidence {
                    url: SOURCE.into(),
                    note: format!(
                        "{} m-band IBP slot at {} UTC. {} receiver(s) matched the scheduled callsign against all 18 full Morse patterns and the first power dash. IQ hashes and per-receiver scores are in ibp-verification.json.",
                        CHANNELS[channel].0.trim_end_matches('m'),
                        slot,
                        heard
                    ),
                };
                let utc = |s| {
                    DateTime::from_timestamp(s, 0)
                        .unwrap()
                        .to_rfc3339_opts(SecondsFormat::Millis, true)
                };
                let mut e = Emission {
                    id: format!("{}-{slot}", m.id),
                    dataset_id: m.id.clone(),
                    mode: "live".into(),
                    fault_injected: false,
                    title: format!("IBP · {}", CHANNELS[channel].0),
                    frequency_hz: m.frequency_hz,
                    start_utc: utc(slot),
                    end_utc: utc(slot + 10),
                    status: if heard > 0 {
                        "undetermined"
                    } else {
                        "no_verified_beacon"
                    }
                    .into(),
                    reason: Some("No verified callsign in this sampled IBP slot".into()),
                    identity: (heard > 0).then(|| Identity {
                        callsign: call.into(),
                        evidence: evidence.clone(),
                    }),
                    provenance: evidence,
                    receivers,
                    pairs,
                    estimate: None,
                    hypotheses: vec![],
                    detected: heard > 0,
                    spectrum,
                    processed_at: Utc::now().to_rfc3339(),
                    reference: None,
                    excluded: vec![],
                };
                if heard > 0 {
                    pipeline::locate_receptions(&mut e);
                    if e.estimate.is_some() {
                        e.reason = None;
                    }
                    e.reference = Some(VerifiedReference {
                        profile: PROFILE.into(),
                        three_receivers: protocol::comparison(&e),
                        cw: proof.clone(),
                        ft8: None,
                    });
                }
                emissions.push((e,serde_json::json!({"slot_utc":utc(slot),"scheduled_callsign":call,"receivers":proof})));
            }
            slot += 10;
        }
    }
    let (emissions, slots): (Vec<_>, Vec<_>) = emissions.into_iter().unzip();
    store::atomic_json(
        &root.join("ibp-verification.json"),
        &serde_json::json!({"profile":PROFILE,"frequency_hz":m.frequency_hz,
        "source":SOURCE,"iq_center_hz":center,"excluded":excluded,"slots":slots,"uses_transmitter_coordinates":false}),
    )?;
    Ok(Analysis {
        emissions,
        captured_receivers: m.receivers.len(),
    })
}
