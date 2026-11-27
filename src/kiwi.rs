//! Kiwi WAV format: jks-prv/kiwiclient/src/read_kiwi_iq_wav.cc.
//! Facts about the format only; independent Rust implementation.
use anyhow::{Context, Result, bail, ensure};
use rustfft::num_complex::Complex;
use std::{fs, path::Path};

#[derive(Clone)]
pub struct Block {
    pub t: f64,
    pub age: u8,
    pub iq: Vec<Complex<f64>>,
}
pub struct Track {
    pub blocks: Vec<Block>,
    pub nominal_fs: f64,
    pub actual_fs: f64,
    pub sync_error: Option<String>,
    pub max_age: u8,
}
fn u32le(x: &[u8]) -> u32 {
    u32::from_le_bytes(x[..4].try_into().unwrap())
}
fn u16le(x: &[u8]) -> u16 {
    u16::from_le_bytes(x[..2].try_into().unwrap())
}

pub fn read(path: &Path) -> Result<Track> {
    let bytes = fs::read(path).with_context(|| format!("Cannot read {}", path.display()))?;
    parse(&bytes)
}

pub fn parse(bytes: &[u8]) -> Result<Track> {
    ensure!(
        bytes.len() >= 44 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
        "File is not RIFF/WAVE"
    );
    let mut pos = 12;
    let mut fs = 0.;
    let mut stamp = None;
    let mut blocks = vec![];
    while pos < bytes.len() {
        ensure!(pos + 8 <= bytes.len(), "Truncated WAV header");
        let tag = &bytes[pos..pos + 4];
        let size = u32le(&bytes[pos + 4..]) as usize;
        pos += 8;
        ensure!(size <= bytes.len() - pos, "Truncated WAV block");
        let b = &bytes[pos..pos + size];
        pos += size + (size % 2);
        match tag {
            b"fmt " => {
                ensure!(
                    size >= 16 && u16le(b) == 1 && u16le(&b[2..]) == 2 && u16le(&b[14..]) == 16,
                    "Stereo PCM16 IQ is required"
                );
                fs = u32le(&b[4..]) as f64;
                ensure!(
                    (11990.0..=12010.0).contains(&fs),
                    "This adapter accepts only approximately 12 kHz Kiwi IQ; other rates require dedicated antialias filtering"
                );
            }
            b"kiwi" => {
                ensure!(
                    size == 10 && stamp.is_none(),
                    "Invalid or duplicate GNSS timestamp"
                );
                let sec = u32le(&b[2..]);
                let ns = u32le(&b[6..]);
                ensure!(ns < 1_000_000_000, "Invalid GNSS nanoseconds");
                stamp = Some((sec as f64 + ns as f64 * 1e-9, b[0]));
            }
            b"data" => {
                let (t, age) = stamp
                    .take()
                    .context("Missing GNSS timestamps; ordinary audio is not accepted")?;
                ensure!(
                    size > 0 && size % 4 == 0 && size <= 65536,
                    "Invalid IQ length"
                );
                let iq = b
                    .chunks_exact(4)
                    .map(|x| {
                        Complex::new(
                            i16::from_le_bytes([x[0], x[1]]) as f64 / 32768.,
                            i16::from_le_bytes([x[2], x[3]]) as f64 / 32768.,
                        )
                    })
                    .collect();
                blocks.push(Block { t, age, iq });
            }
            _ => {}
        }
    }
    ensure!(stamp.is_none() && fs > 0., "Incomplete capture");
    // Fixed startup exclusion, independent of the signal or transmitter location.
    let mut warmup = 0;
    let mut skip = 0;
    while skip < blocks.len() && warmup < (fs as usize) {
        warmup += blocks[skip].iq.len();
        skip += 1;
    }
    blocks.drain(..skip);
    ensure!(
        blocks.len() > 50,
        "Capture is too short after 1 s of settling time"
    );
    // Unwrap GPS week, never a timestamp discontinuity within a week.
    let mut week = 0.;
    let mut prev = blocks[0].t;
    for b in &mut blocks {
        if b.t + week < prev - 600000. {
            week += 604800.;
        }
        b.t += week;
        prev = b.t;
    }
    let rates: Vec<f64> = blocks
        .windows(2)
        .map(|w| w[0].iq.len() as f64 / (w[1].t - w[0].t))
        .collect();
    let mut sorted = rates.clone();
    sorted.sort_by(f64::total_cmp);
    let actual_fs = sorted[sorted.len() / 2];
    let max_age = blocks.iter().map(|b| b.age).max().unwrap();
    let mut sync_error = None;
    if max_age > 2 {
        sync_error = Some("GNSS synchronization is missing or stale (>2 s) in the capture".into());
    }
    if !actual_fs.is_finite()
        || (actual_fs / fs - 1.).abs() > 0.001
        || rates
            .iter()
            .any(|r| !r.is_finite() || (*r / actual_fs - 1.).abs() > 0.001)
    {
        sync_error = Some("GNSS discontinuity, missing block or unstable clock".into());
    }
    if actual_fs <= 0. || blocks.windows(2).any(|w| w[1].t <= w[0].t) {
        bail!("Marcaje GNSS nemonotone");
    }
    Ok(Track {
        blocks,
        nominal_fs: fs,
        actual_fs,
        sync_error,
        max_age,
    })
}

impl Track {
    pub fn start(&self) -> f64 {
        self.blocks[0].t
    }
    pub fn end(&self) -> f64 {
        self.blocks.last().unwrap().t
    }
    pub fn resample(&self, start: f64, fs: f64, n: usize) -> Vec<Complex<f64>> {
        let mut k = 0;
        (0..n)
            .map(|i| {
                let t = start + i as f64 / fs;
                while k + 1 < self.blocks.len() - 1 && self.blocks[k + 1].t <= t {
                    k += 1;
                }
                let b = &self.blocks[k];
                let next = &self.blocks[k + 1];
                let x = ((t - b.t) / (next.t - b.t) * b.iq.len() as f64)
                    .clamp(0., b.iq.len() as f64 - 1e-8);
                let j = x.floor() as usize;
                let f = x - j as f64;
                let right = if j + 1 < b.iq.len() {
                    b.iq[j + 1]
                } else {
                    next.iq[0]
                };
                b.iq[j] * (1. - f) + right * f
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_audio_and_truncation() {
        assert!(parse(b"not a wav").is_err());
    }
}
