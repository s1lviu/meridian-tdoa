use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Point {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub url: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receiver {
    pub id: String,
    pub label: String,
    pub independent_device: String,
    pub position: Point,
    pub position_evidence: Evidence,
    pub position_uncertainty_m: f64,
    pub file: String,
    pub sha256: String,
}

/// A captured receiver withheld from localization, with the RF reason recorded for provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectedReceiver {
    #[serde(flatten)]
    pub receiver: Receiver,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub callsign: String,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub mode: String,
    pub frequency_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iq_center_hz: Option<f64>,
    pub capture_utc: String,
    pub gps_utc_offset_s: i64,
    pub provenance: Evidence,
    #[serde(default)]
    pub emission_verified: bool,
    pub identity: Option<Identity>,
    pub receivers: Vec<Receiver>,
    /// Receivers whose IQ failed the RF verifier; kept for provenance, never localized.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_receivers: Vec<RejectedReceiver>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RxResult {
    #[serde(flatten)]
    pub receiver: Receiver,
    pub confirmed: bool,
    pub used: bool,
    pub synchronized: bool,
    pub reason: Option<String>,
    pub gps_age_max_s: u8,
    pub sample_rate_hz: f64,
    pub signal_score_db: f64,
    pub samples: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pair {
    pub a: String,
    pub b: String,
    pub delay_us: f64,
    pub sigma_us: f64,
    pub coherence: f64,
    pub peak_ratio: f64,
    pub windows: usize,
    pub accepted: bool,
    pub correlation: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Estimate {
    pub center: Point,
    pub polygon: Vec<Point>,
    pub radius_km: f64,
    pub residual_us: f64,
    pub geometry_score: f64,
    pub uncertainty_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiverComparison {
    pub receiver_ids: Vec<String>,
    #[serde(default)]
    pub selection: String,
    pub solutions: Vec<Estimate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedReference {
    pub profile: String,
    pub three_receivers: Option<ReceiverComparison>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cw: Vec<crate::ibp::ReceiverEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Emission {
    pub id: String,
    pub dataset_id: String,
    pub mode: String,
    pub fault_injected: bool,
    pub title: String,
    pub frequency_hz: f64,
    pub start_utc: String,
    pub end_utc: String,
    pub status: String,
    pub reason: Option<String>,
    pub identity: Option<Identity>,
    pub provenance: Evidence,
    pub receivers: Vec<RxResult>,
    pub pairs: Vec<Pair>,
    pub estimate: Option<Estimate>,
    #[serde(default)]
    pub hypotheses: Vec<Estimate>,
    pub detected: bool,
    pub spectrum: Vec<[f64; 2]>,
    pub processed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<VerifiedReference>,
    /// Receivers captured but excluded by the RF verifier before correlation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<RejectedReceiver>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    /// `published_api`: recheck ext_api and tdoa_ch at each capture; expiry is not an operator promise.
    #[serde(default)]
    pub basis: String,
    pub automatic_capture: bool,
    pub evidence: Evidence,
    pub expires_utc: String,
    pub max_capture_s: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveSource {
    pub id: String,
    pub label: String,
    pub independent_device: String,
    pub base_url: String,
    pub position: Point,
    pub position_evidence: Evidence,
    pub position_uncertainty_m: f64,
    pub antenna_position_verified: bool,
    pub bands_hz: Vec<[f64; 2]>,
    pub bands_evidence: Evidence,
    pub permission: Permission,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    #[serde(default)]
    pub verification: Option<String>,
    pub id: String,
    pub kind: String,
    pub frequency_hz: f64,
    pub eligibility_evidence: Evidence,
    pub source_ids: Vec<String>,
    pub duration_s: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub listen: String,
    pub live_enabled: bool,
    pub kiwirecorder_path: String,
    pub python: String,
    pub gps_utc_offset_s: i64,
    pub gps_offset_evidence: Evidence,
    #[serde(default)]
    pub gps_offset_valid_until_utc: Option<String>,
    pub sources: Vec<LiveSource>,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub monitor: Option<MonitorConfig>,
    #[serde(default)]
    pub retention: crate::retention::Policy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorConfig {
    pub target_id: String,
    pub interval_s: u64,
    #[serde(default)]
    pub additional_target_ids: Vec<String>,
    #[serde(default)]
    pub directory_receivers: usize,
    #[serde(default = "one_band")]
    pub parallel_bands: usize,
}

fn one_band() -> usize {
    1
}
