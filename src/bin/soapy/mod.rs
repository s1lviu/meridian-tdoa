//! Receive-only SoapySDR input. Publish the SigMF metadata last so an edge watcher
//! only sees a recording after all samples and their hardware time base are complete.
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, SecondsFormat, Utc};
use meridian_tdoa::{model::Point, pipeline, store};
use serde::Deserialize;
use serde_json::json;
use std::{
    ffi::{CStr, CString, c_char},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    device: String,
    receiver_id: String,
    position: Point,
    time_sigma_ns: f64,
    frequency_hz: f64,
    sample_rate_hz: f64,
    duration_s: f64,
    #[serde(default)]
    channel: usize,
    gain_db: Option<f64>,
    start_utc: Option<DateTime<Utc>>,
}

#[repr(C)]
#[derive(Default)]
struct Measurement {
    utc_ns: i64,
    sample_rate: f64,
    frequency: f64,
    samples: u64,
}

unsafe extern "C" {
    fn meridian_receive(
        device: *const c_char,
        channel: usize,
        rate: f64,
        frequency: f64,
        gain: f64,
        duration: f64,
        start_ns: i64,
        path: *const c_char,
        result: *mut Measurement,
        error: *mut c_char,
        error_size: usize,
    ) -> i32;
}

pub(super) fn capture(config: &Path, output: &Path) -> Result<PathBuf> {
    let s: Settings = serde_json::from_slice(&std::fs::read(config)?)?;
    ensure!(
        !s.device.trim().is_empty() && meridian_tdoa::capture::safe_id(&s.receiver_id),
        "Specify a device and a valid receiver_id"
    );
    ensure!(
        pipeline::valid_position(s.position),
        "Invalid receiver position"
    );
    ensure!(
        s.time_sigma_ns > 0.
            && s.duration_s > 0.
            && s.sample_rate_hz >= 12000.
            && (6000.0..=6e9).contains(&s.frequency_hz),
        "Invalid timing, duration, sample rate or frequency"
    );
    ensure!(
        cfg!(target_endian = "little"),
        "The CF32 recorder requires a little-endian host"
    );
    let start = s
        .start_utc
        .map(|t| {
            t.timestamp_nanos_opt()
                .filter(|&n| n > 0)
                .context("Requested UTC start is outside the hardware time range")
        })
        .transpose()?;
    std::fs::create_dir_all(output)?;
    let temporary = tempfile::NamedTempFile::new_in(output)?;
    let device = CString::new(s.device.as_str())?;
    let path = CString::new(
        temporary
            .path()
            .to_str()
            .context("Output path must be UTF-8")?,
    )?;
    let mut result = Measurement::default();
    let mut error = [0 as c_char; 512];
    // The native call owns the stream until it returns. Its only writable pointers refer
    // to these live objects, and it always NUL-terminates a nonempty error buffer.
    let status = unsafe {
        meridian_receive(
            device.as_ptr(),
            s.channel,
            s.sample_rate_hz,
            s.frequency_hz,
            s.gain_db.unwrap_or(f64::NAN),
            s.duration_s,
            start.unwrap_or(0),
            path.as_ptr(),
            &mut result,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    ensure!(
        status == 0,
        "SoapySDR: {}",
        unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy()
    );
    ensure!(
        temporary.as_file().metadata()?.len() == result.samples * 8,
        "Incomplete hardware capture"
    );
    temporary.as_file().sync_all()?;
    let stem = format!("{}-{}", s.receiver_id, uuid::Uuid::new_v4());
    let data = output.join(format!("{stem}.sigmf-data"));
    let meta = output.join(format!("{stem}.sigmf-meta"));
    let metadata = json!({"global":{
        "core:datatype":"cf32_le","core:version":"1.2.0","core:sample_rate":result.sample_rate,
        "core:recorder":"meridian-node / SoapySDR","core:geolocation":{
            "type":"Point","coordinates":[s.position.lon,s.position.lat]},
        "meridian:receiver_id":s.receiver_id,"meridian:time_source":"gpsdo",
        "meridian:time_sigma_ns":s.time_sigma_ns},
        "captures":[{"core:sample_start":0,"core:frequency":result.frequency,
            "core:datetime":DateTime::from_timestamp_nanos(result.utc_ns).to_rfc3339_opts(SecondsFormat::Nanos,true)}],
        "annotations":[]});
    temporary.persist_noclobber(&data)?;
    if let Err(e) = store::atomic_json(&meta, &metadata) {
        std::fs::remove_file(data)?;
        return Err(e);
    }
    Ok(meta)
}
