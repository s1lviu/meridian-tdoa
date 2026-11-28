//! One entry point for timestamped IQ, whatever the SDR: Kiwi GNSS WAV or SigMF.
use crate::{kiwi, model::Manifest, sigmf};
use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Utc};
use std::path::Path;

/// Analysis sample rate of every pipeline (Kiwi IQ is natively ≈12 kHz).
pub const FS: f64 = 12_000.;

/// Unix timestamp (GPS scale) of the start of the GPS week containing the manifest's anchor.
pub fn gps_week_start(m: &Manifest) -> Result<i64> {
    let anchor = DateTime::parse_from_rfc3339(&m.capture_utc)?.with_timezone(&Utc);
    let gps = anchor + Duration::seconds(m.gps_utc_offset_s);
    Ok((gps.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc()
        - Duration::days(gps.weekday().num_days_from_sunday() as i64))
    .timestamp())
}

/// Read a receiver file as a track in GPS seconds of week. SigMF recordings are channelized
/// to 12 kHz around the manifest's IQ centre; their declared clock σ travels with the track.
pub fn read(path: &Path, m: &Manifest) -> Result<kiwi::Track> {
    if path.to_string_lossy().ends_with(".sigmf-meta") {
        let r = sigmf::read(path)?;
        let center = m.iq_center_hz.unwrap_or(m.frequency_hz);
        r.to_track(
            FS,
            center - r.frequency_hz,
            gps_week_start(m)?,
            m.gps_utc_offset_s,
        )
    } else {
        kiwi::read(path)
    }
}

/// Integrity file for a receiver entry: the SigMF dataset (samples), else the file itself.
pub fn hashed_file(path: &Path) -> std::path::PathBuf {
    if path.to_string_lossy().ends_with(".sigmf-meta") {
        sigmf::data_path(path)
    } else {
        path.to_path_buf()
    }
}
