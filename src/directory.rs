//! Public directory contract: https://github.com/aethersdr/AetherSDR/blob/main/docs/kiwi-json-schema.md
//! Discovery never opens a receiver connection or establishes reception/coverage.
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

pub const URL: &str = "https://cdn.aethersdr.com/kiwi.json";
pub const CACHE_SECONDS: u64 = 1800;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub antenna: String,
    #[serde(default)]
    pub gps: Option<[f64; 2]>,
    #[serde(default)]
    pub bands: Option<[f64; 2]>,
    #[serde(default)]
    pub ext_api: Option<i32>,
    #[serde(default)]
    pub tdoa_ch: Option<i32>,
    #[serde(default)]
    pub users: u32,
    #[serde(default)]
    pub users_max: u32,
    #[serde(default)]
    pub gps_good: u32,
    #[serde(default)]
    pub fixes_min: u32,
    #[serde(default)]
    pub offline: bool,
    #[serde(default)]
    pub flagged: bool,
    #[serde(default)]
    pub ant_connected: bool,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub freq_offset: f64,
}

impl Entry {
    pub fn source(&self, observed_at: &str) -> Result<crate::model::LiveSource> {
        use crate::model::*;
        ensure!(
            self.exclusion().is_none(),
            "{}: {}",
            self.id,
            self.exclusion().unwrap_or("invalid")
        );
        let url = reqwest::Url::parse(&self.url)?.to_string();
        let status = format!("{url}status");
        let p = self.gps.unwrap();
        Ok(LiveSource {
            id: self.id.clone(),
            label: self.name.clone(),
            independent_device: self.id.clone(),
            base_url: url,
            position: Point {
                lat: p[0],
                lon: p[1],
            },
            position_evidence: Evidence {
                url: status.clone(),
                note: format!(
                    "Operator-reported position for antenna «{}»; directory {}. Compared with /status before capture. Antenna offset is not independently measured. The 1 km allowance is a conservative assumption, not measured accuracy.",
                    self.antenna, observed_at
                ),
            },
            position_uncertainty_m: 1000.,
            antenna_position_verified: true,
            bands_hz: vec![self.bands.unwrap()],
            bands_evidence: Evidence {
                url: status.clone(),
                note: "Receiver-published range; /status is rechecked before capture.".into(),
            },
            permission: Permission {
                basis: "published_api".into(),
                automatic_capture: true,
                evidence: Evidence {
                    url: status,
                    note: format!(
                        "Directory-published policy: ext_api={}, tdoa_ch={}. Both must be positive at each capture, without preemption or bypassing refusals. One connection for at most 30 s.",
                        self.ext_api.unwrap(),
                        self.tdoa_ch.unwrap()
                    ),
                },
                expires_utc: "1970-01-01T00:00:00Z".into(),
                max_capture_s: 30,
            },
        })
    }
    pub fn exclusion(&self) -> Option<&'static str> {
        if self.flagged {
            return Some("flagged by directory");
        }
        if self.offline {
            return Some("offline");
        }
        match self.ext_api {
            None => return Some("API policy unpublished"),
            Some(n) if n <= 0 => return Some("External API disabled"),
            _ => (),
        }
        if self.tdoa_ch.unwrap_or(0) <= 0 {
            return Some("TDoA timestamps unavailable");
        }
        if self.gps_good < 4 || self.fixes_min == 0 {
            return Some("Insufficient directory GNSS status");
        }
        if self.users >= self.users_max {
            return Some("channels busy");
        }
        if !self.ant_connected || self.antenna.is_empty() {
            return Some("antenna undocumented/disconnected");
        }
        if self.gps.is_none_or(|p| {
            !p.iter().all(|v| v.is_finite())
                || p[0].abs() > 90.
                || p[1].abs() > 180.
                || p == [0., 0.]
        }) {
            return Some("invalid position");
        }
        if self
            .bands
            .is_none_or(|b| !b.iter().all(|v| v.is_finite()) || b[0] < 0. || b[0] >= b[1])
            || self.freq_offset != 0.
        {
            return Some("unsupported band/converter");
        }
        if self.mode == "rx3.wf3" {
            return Some("20 kHz IQ mode not yet supported");
        }
        if reqwest::Url::parse(&self.url).ok().is_none_or(|u| {
            u.scheme() != "http"
                || u.host_str().is_none()
                || u.path() != "/"
                || !u.username().is_empty()
                || u.password().is_some()
                || u.query().is_some()
        }) {
            return Some("unsupported endpoint");
        }
        None
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Directory {
    pub schema: u32,
    pub fetched_at: String,
    #[serde(default)]
    pub retrieved_at: Option<String>,
    pub receivers: Vec<Entry>,
}

pub fn parse(bytes: &[u8]) -> Result<Directory> {
    ensure!(bytes.len() <= 32 * 1024 * 1024, "Directory too large");
    let d: Directory = serde_json::from_slice(bytes).context("Public directory schema changed")?;
    ensure!(
        d.schema == 1 && d.receivers.len() <= 20000,
        "Unsupported directory version/size"
    );
    DateTime::parse_from_rfc3339(&d.fetched_at)?;
    ensure!(
        d.receivers.iter().all(|r| r.id.len() <= 128
            && r.url.len() <= 2048
            && r.name.len() <= 4096
            && r.antenna.len() <= 4096),
        "Directory field too large"
    );
    Ok(d)
}

pub fn load(path: &Path) -> Result<Directory> {
    parse(&std::fs::read(path)?)
}

pub async fn refresh(path: &Path) -> Result<Directory> {
    if let Ok(d) = load(path) {
        if let Some(t) = d
            .retrieved_at
            .as_ref()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        {
            if Utc::now().signed_duration_since(t).num_seconds() < CACHE_SECONDS as i64 {
                return Ok(d);
            }
        }
    }
    let client = reqwest::Client::builder()
        .user_agent("Meridian-TDoA/0.2 directory-discovery")
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut response = client.get(URL).send().await?.error_for_status()?;
    ensure!(
        response.status().is_success(),
        "Unexpected directory redirect"
    );
    let mut bytes = vec![];
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 32 * 1024 * 1024,
            "Directory too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    let mut d = parse(&bytes)?;
    d.retrieved_at = Some(Utc::now().to_rfc3339());
    crate::store::atomic_json(path, &d)?;
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn published_access_and_gnss_are_required_and_catalog_is_not_rf_evidence() {
        let bytes = std::fs::read("evidence/live/directory.json").unwrap();
        let d = parse(&bytes).unwrap();
        assert!(d.receivers.len() > 100);
        let mut r = d
            .receivers
            .iter()
            .find(|r| r.exclusion().is_none())
            .unwrap()
            .clone();
        assert!(r.ext_api.unwrap() > 0);
        r.ext_api = None;
        assert_eq!(r.exclusion(), Some("API policy unpublished"));
        r.ext_api = Some(0);
        assert_eq!(r.exclusion(), Some("External API disabled"));
        r.ext_api = Some(1);
        r.gps_good = 0;
        assert_eq!(r.exclusion(), Some("Insufficient directory GNSS status"));
        assert!(
            parse(br#"{"schema":2,"receivers":[],"fetched_at":"2026-11-26T00:00:00Z"}"#).is_err()
        );
    }
}
