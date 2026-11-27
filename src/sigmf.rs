//! SigMF recordings from any SDR (https://github.com/sigmf/SigMF, schema 1.x).
//! Timing comes from `captures[].core:datetime` (RFC 3339, UTC, fractional seconds) at each
//! segment's `core:sample_start`; several segments (e.g. one per PPS edge) give a piecewise
//! linear time base exactly like Kiwi GNSS blocks. Position from `core:geolocation` (GeoJSON).
//! Meridian extension keys (`meridian:`):
//! - `meridian:time_source`: `gnss_pps`, `gpsdo`, `kiwi_gnss`, `reference_sync`, … (required);
//! - `meridian:time_sigma_ns`: declared timing uncertainty of the time base (required);
//! - `meridian:receiver_id`: stable identifier of the physical receiver (required).
use crate::{kiwi, model::Point};
use anyhow::{Context, Result, bail, ensure};
use chrono::DateTime;
use rustfft::num_complex::Complex;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Time sources accepted for TDoA. NTP or a free-running clock cannot reach the µs level.
pub const TIME_SOURCES: [&str; 4] = ["gnss_pps", "gpsdo", "kiwi_gnss", "reference_sync"];

#[derive(Debug, Clone)]
pub struct Recording {
    pub receiver_id: String,
    pub position: Option<Point>,
    pub sample_rate: f64,
    pub frequency_hz: f64,
    pub time_source: String,
    pub time_sigma_ns: f64,
    /// Integer UTC second used as the time origin, so f64 offsets keep sub-ns resolution
    /// (absolute Unix seconds in an f64 resolve only ~0.24 µs).
    pub epoch_utc: i64,
    /// (sample index, seconds after `epoch_utc`) anchors, sorted by sample index.
    pub anchors: Vec<(u64, f64)>,
    pub samples: Vec<Complex<f64>>,
    pub hardware: Option<String>,
}

/// (whole UTC seconds, nanoseconds) of an RFC 3339 `Z` timestamp.
fn utc_parts(s: &str) -> Result<(i64, u32)> {
    ensure!(s.ends_with('Z'), "SigMF core:datetime must be UTC (Z)");
    let t =
        DateTime::parse_from_rfc3339(s).with_context(|| format!("Invalid core:datetime {s}"))?;
    Ok((t.timestamp(), t.timestamp_subsec_nanos()))
}

fn decode(datatype: &str, bytes: &[u8]) -> Result<Vec<Complex<f64>>> {
    let v: Vec<Complex<f64>> = match datatype {
        "ci16_le" | "ci16" => bytes
            .chunks_exact(4)
            .map(|b| {
                Complex::new(
                    i16::from_le_bytes([b[0], b[1]]) as f64 / 32768.,
                    i16::from_le_bytes([b[2], b[3]]) as f64 / 32768.,
                )
            })
            .collect(),
        "cf32_le" | "cf32" => bytes
            .chunks_exact(8)
            .map(|b| {
                Complex::new(
                    f32::from_le_bytes(b[..4].try_into().unwrap()) as f64,
                    f32::from_le_bytes(b[4..].try_into().unwrap()) as f64,
                )
            })
            .collect(),
        "cf64_le" | "cf64" => bytes
            .chunks_exact(16)
            .map(|b| {
                Complex::new(
                    f64::from_le_bytes(b[..8].try_into().unwrap()),
                    f64::from_le_bytes(b[8..].try_into().unwrap()),
                )
            })
            .collect(),
        "ci8" => bytes
            .chunks_exact(2)
            .map(|b| Complex::new(b[0] as i8 as f64 / 128., b[1] as i8 as f64 / 128.))
            .collect(),
        // RTL-SDR native: unsigned 8-bit, mid-scale 127.5.
        "cu8" => bytes
            .chunks_exact(2)
            .map(|b| Complex::new((b[0] as f64 - 127.5) / 127.5, (b[1] as f64 - 127.5) / 127.5))
            .collect(),
        other => {
            bail!("Unsupported SigMF datatype {other}; use ci16_le, cf32_le, cf64_le, ci8 or cu8")
        }
    };
    Ok(v)
}

/// Data file next to `meta`: `name.sigmf-meta` → `name.sigmf-data`.
pub fn data_path(meta: &Path) -> PathBuf {
    let s = meta.to_string_lossy();
    PathBuf::from(
        s.strip_suffix(".sigmf-meta")
            .map_or_else(|| format!("{s}.sigmf-data"), |b| format!("{b}.sigmf-data")),
    )
}

pub fn read(meta: &Path) -> Result<Recording> {
    let m: Value = serde_json::from_slice(
        &std::fs::read(meta).with_context(|| format!("Cannot read {}", meta.display()))?,
    )?;
    let g = &m["global"];
    let datatype = g["core:datatype"]
        .as_str()
        .context("Missing core:datatype")?;
    ensure!(g["core:version"].is_string(), "Missing core:version");
    ensure!(
        g["core:num_channels"].as_u64().unwrap_or(1) == 1,
        "Only single-channel SigMF recordings are supported"
    );
    let sample_rate = g["core:sample_rate"]
        .as_f64()
        .context("Missing core:sample_rate")?;
    ensure!(
        sample_rate.is_finite() && sample_rate > 0.,
        "Invalid core:sample_rate"
    );
    let time_source = g["meridian:time_source"]
        .as_str()
        .context("Missing meridian:time_source")?
        .to_owned();
    ensure!(
        TIME_SOURCES.contains(&time_source.as_str()),
        "meridian:time_source {time_source} cannot support TDoA timing"
    );
    let time_sigma_ns = g["meridian:time_sigma_ns"]
        .as_f64()
        .context("Missing meridian:time_sigma_ns")?;
    ensure!(
        time_sigma_ns.is_finite() && time_sigma_ns >= 0.,
        "Invalid meridian:time_sigma_ns"
    );
    let receiver_id = g["meridian:receiver_id"]
        .as_str()
        .context("Missing meridian:receiver_id")?
        .to_owned();
    let captures = m["captures"].as_array().context("Missing captures")?;
    ensure!(!captures.is_empty(), "No capture segments");
    let mut raw_anchors: Vec<(u64, (i64, u32))> = vec![];
    let mut frequency = None;
    let mut position = None;
    for c in captures {
        let start = c["core:sample_start"]
            .as_u64()
            .context("Missing core:sample_start")?;
        if let Some(dt) = c["core:datetime"].as_str() {
            raw_anchors.push((start, utc_parts(dt)?));
        }
        if let Some(f) = c["core:frequency"].as_f64() {
            ensure!(
                frequency.is_none_or(|v: f64| (v - f).abs() < 1e-6),
                "Frequency changes inside one recording are not supported"
            );
            frequency = Some(f);
        }
        if let Some(p) = c["core:geolocation"].as_object() {
            position = position.or(geojson(p));
        }
    }
    if position.is_none() {
        position = g["core:geolocation"].as_object().and_then(geojson);
    }
    ensure!(
        !raw_anchors.is_empty(),
        "No core:datetime anchor: the samples have no time base"
    );
    raw_anchors.sort_by_key(|a| a.0);
    let epoch_utc = raw_anchors[0].1.0;
    let anchors: Vec<(u64, f64)> = raw_anchors
        .iter()
        .map(|(i, (s, ns))| (*i, (s - epoch_utc) as f64 + *ns as f64 * 1e-9))
        .collect();
    ensure!(
        anchors
            .windows(2)
            .all(|w| w[1].0 > w[0].0 && w[1].1 > w[0].1),
        "SigMF time anchors are not monotonic"
    );
    let bytes = std::fs::read(data_path(meta)).context("Missing .sigmf-data")?;
    ensure!(
        bytes.len() <= 512 * 1024 * 1024,
        "SigMF dataset larger than 512 MiB"
    );
    let samples = decode(datatype, &bytes)?;
    ensure!(
        samples.len() as u64 > anchors.last().unwrap().0,
        "Anchor beyond the end of the dataset"
    );
    Ok(Recording {
        receiver_id,
        position,
        sample_rate,
        frequency_hz: frequency.context("Missing core:frequency")?,
        time_source,
        time_sigma_ns,
        epoch_utc,
        anchors,
        samples,
        hardware: g["core:hw"].as_str().map(str::to_owned),
    })
}

fn geojson(p: &serde_json::Map<String, Value>) -> Option<Point> {
    (p.get("type")?.as_str()? == "Point").then_some(())?;
    let c = p.get("coordinates")?.as_array()?;
    let (lon, lat) = (c.first()?.as_f64()?, c.get(1)?.as_f64()?);
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then_some(Point { lat, lon })
}

impl Recording {
    /// Seconds after `epoch_utc` of a (fractional) sample index from the anchors. A single anchor extrapolates
    /// with the nominal rate; several anchors interpolate and extrapolate piecewise linearly.
    pub fn time_of(&self, index: f64) -> f64 {
        if self.anchors.len() == 1 {
            let (i0, t0) = self.anchors[0];
            return t0 + (index - i0 as f64) / self.sample_rate;
        }
        let k = self
            .anchors
            .windows(2)
            .position(|w| index < w[1].0 as f64)
            .unwrap_or(self.anchors.len() - 2);
        let ((i0, t0), (i1, t1)) = (self.anchors[k], self.anchors[k + 1]);
        t0 + (index - i0 as f64) * (t1 - t0) / (i1 - i0) as f64
    }

    /// Channelize to `fs_out` around `offset_hz` (relative to the recording centre) with a
    /// Kaiser-windowed sinc low-pass (cutoff 0.45·fs_out, ≈80 dB), returning a Kiwi-style
    /// track whose block timestamps follow this recording's time base. `gps_minus_utc_s`
    /// converts UTC to GPS seconds of the week containing `week_start_utc`.
    pub fn to_track(
        &self,
        fs_out: f64,
        offset_hz: f64,
        week_start_utc: i64,
        gps_minus_utc_s: i64,
    ) -> Result<kiwi::Track> {
        ensure!(
            fs_out <= self.sample_rate * 1.0001,
            "Cannot upsample a SigMF recording"
        );
        let ratio = self.sample_rate / fs_out;
        let cutoff = 0.45 * fs_out / self.sample_rate; // cycles per input sample
        let half = filter_half(ratio);
        let beta = 7.86;
        let norm = bessel_i0(beta);
        let n_out = ((self.samples.len() as f64 - 2. * half as f64) / ratio)
            .floor()
            .max(0.) as usize;
        let step = Complex::from_polar(1., -std::f64::consts::TAU * offset_hz / self.sample_rate);
        // Mix once: x[n]·e^{−j2π f n/fs}
        let mut rot = Complex::new(1., 0.);
        let mixed: Vec<Complex<f64>> = self
            .samples
            .iter()
            .map(|z| {
                let v = z * rot;
                rot *= step;
                if rot.norm_sqr() < 0.999 || rot.norm_sqr() > 1.001 {
                    rot /= rot.norm();
                }
                v
            })
            .collect();
        let mut out = Vec::with_capacity(n_out);
        for k in 0..n_out {
            let center = half as f64 + k as f64 * ratio;
            let base = center.floor() as i64;
            let mut acc = Complex::new(0., 0.);
            for j in base - half + 1..=base + half {
                let u = j as f64 - center;
                let x = 2. * cutoff * u;
                let sinc = if x.abs() < 1e-12 {
                    1.
                } else {
                    (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                };
                let r = u / half as f64;
                let w = if r.abs() >= 1. {
                    0.
                } else {
                    bessel_i0(beta * (1. - r * r).sqrt()) / norm
                };
                acc += mixed[j as usize] * (2. * cutoff * sinc * w);
            }
            out.push(acc);
        }
        // Blocks of 512 output samples, each stamped from the recording's own time base.
        let blocks: Vec<kiwi::Block> = out
            .chunks(512)
            .enumerate()
            .map(|(b, iq)| {
                let input_index = half as f64 + (b * 512) as f64 * ratio;
                kiwi::Block {
                    // Integer parts first, so the f64 stays small and keeps its resolution.
                    t: (self.epoch_utc - week_start_utc + gps_minus_utc_s) as f64
                        + self.time_of(input_index),
                    age: 0,
                    iq: iq.to_vec(),
                }
            })
            .collect();
        ensure!(blocks.len() > 3, "Recording too short");
        let actual_fs = {
            let (a, b) = (&blocks[0], &blocks[blocks.len() - 2]);
            (blocks.len() - 2) as f64 * 512. / (b.t - a.t)
        };
        Ok(kiwi::Track {
            blocks,
            nominal_fs: fs_out,
            actual_fs,
            sync_error: None,
            max_age: 0,
        })
    }
}

/// Half-length (input samples) of the channelizing filter; output sample k is centred on
/// input index `filter_half + k·ratio`, which is the instant its timestamp refers to.
pub fn filter_half(ratio: f64) -> i64 {
    (8. * ratio).ceil().max(16.) as i64
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1., 1., 1.);
    while term > 1e-17 * sum {
        term *= (x / (2. * k)).powi(2);
        sum += term;
        k += 1.;
    }
    sum
}

/// Write a SigMF pair (used by tests and by `meridian-tdoa sigmf-from-kiwi`).
pub fn write(
    meta: &Path,
    receiver_id: &str,
    position: Point,
    sample_rate: f64,
    frequency_hz: f64,
    time_source: &str,
    time_sigma_ns: f64,
    anchors: &[(u64, String)],
    samples: &[Complex<f64>],
) -> Result<()> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for z in samples {
        for v in [z.re, z.im] {
            bytes.extend_from_slice(&((v * 32767.).clamp(-32767., 32767.) as i16).to_le_bytes());
        }
    }
    std::fs::write(data_path(meta), &bytes)?;
    let captures: Vec<Value> = anchors
        .iter()
        .map(|(i, t)| serde_json::json!({"core:sample_start":i,"core:datetime":t,"core:frequency":frequency_hz}))
        .collect();
    let meta_json = serde_json::json!({
        "global": {
            "core:datatype": "ci16_le",
            "core:version": "1.2.0",
            "core:sample_rate": sample_rate,
            "core:recorder": "meridian-tdoa",
            "core:geolocation": {"type":"Point","coordinates":[position.lon, position.lat]},
            "core:extensions": [{"name":"meridian","version":"1.0.0","optional":false}],
            "meridian:receiver_id": receiver_id,
            "meridian:time_source": time_source,
            "meridian:time_sigma_ns": time_sigma_ns
        },
        "captures": captures,
        "annotations": []
    });
    std::fs::write(meta, serde_json::to_vec_pretty(&meta_json)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_timestamps_channelizes_and_keeps_in_band_tones_only() {
        let dir = tempfile::tempdir().unwrap();
        let meta = dir.path().join("rx.sigmf-meta");
        let fs = 240_000.;
        let n = (fs * 2.) as usize;
        // In-band tone at +3 kHz and an out-of-band tone at +40 kHz (beyond 0.45·12 kHz).
        let x: Vec<Complex<f64>> = (0..n)
            .map(|k| {
                let t = k as f64 / fs;
                Complex::from_polar(0.4, std::f64::consts::TAU * 3000. * t)
                    + Complex::from_polar(0.4, std::f64::consts::TAU * 40_000. * t)
            })
            .collect();
        // Two PPS anchors: the clock runs 2 ppm fast relative to UTC.
        let anchors = vec![
            (0u64, "2026-11-27T12:00:00.000000000Z".to_string()),
            (240_000u64 + 0, "2026-11-27T12:00:00.999998000Z".to_string()),
        ];
        write(
            &meta,
            "node-1",
            Point {
                lat: 44.4,
                lon: 26.1,
            },
            fs,
            7.1e6,
            "gnss_pps",
            50.,
            &anchors,
            &x,
        )
        .unwrap();
        let r = read(&meta).unwrap();
        assert_eq!(r.receiver_id, "node-1");
        assert!((r.time_of(240_000.) - r.time_of(0.) - 0.999998).abs() < 1e-9);
        let week = 1_795_000_000i64;
        let t = r.to_track(12_000., 0., week, 18).unwrap();
        // Out of band rejected, in band preserved.
        let spec = crate::dsp::spectrum(
            &t.blocks
                .iter()
                .flat_map(|b| b.iq.clone())
                .collect::<Vec<_>>(),
            12_000.,
        )
        .0;
        let peak = spec
            .iter()
            .cloned()
            .fold([0., -1e9], |a, b| if b[1] > a[1] { b } else { a });
        assert!((peak[0] - 3000.).abs() < 100., "peak at {}", peak[0]);
        // Time base carried over: first block at the anchor + filter delay, GPS seconds of week.
        let first = t.blocks[0].t - (r.epoch_utc - week + 18) as f64;
        let origin = filter_half(fs / 12_000.) as f64;
        assert!(
            (first - r.time_of(origin)).abs() < 1e-9,
            "{}",
            first - r.time_of(origin)
        );
        assert!(read(&dir.path().join("missing.sigmf-meta")).is_err());
    }
}
