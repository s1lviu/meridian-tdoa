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
    /// Declared timing uncertainty of the receiver's time base (SigMF nodes), ns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing_sigma_ns: Option<f64>,
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
    /// χ² at the centre, its degrees of freedom and the a-posteriori variance factor applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chi2: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dof: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// Gaussian approximation in a local east/north frame: [σee², σen, σnn²] in km².
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covariance_km2: Option<[f64; 3]>,
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
    /// Decoded FT8 content and per-receiver arrival evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ft8: Option<crate::ft8_tdoa::Evidence>,
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
    /// Minimum distance between receivers picked from the directory. Baselines must be long
    /// compared with c·σ(timing): ~150 km suffices for LF ground wave, HF needs several hundred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_spacing_km: Option<f64>,
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
    #[serde(default)]
    pub receivers: ReceiverPolicy,
    /// Own receiver nodes that report arrival times (`POST /api/detections`).
    #[serde(default)]
    pub nodes: Vec<Node>,
    /// Emitters with published positions used only to synchronize node clocks.
    #[serde(default)]
    pub reference_emitters: Vec<ReferenceEmitter>,
    /// Capture windows processed concurrently (each on disjoint receivers).
    #[serde(default = "two")]
    pub max_parallel_jobs: usize,
}

fn two() -> usize {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub position: Point,
    pub position_evidence: Evidence,
    /// SHA-256 (hex) of the node's bearer token; the token itself is never stored.
    pub token_sha256: String,
    /// The node's clock is GNSS-disciplined to UTC (PPS/GPSDO): its reports need no
    /// reference-emitter synchronization.
    #[serde(default)]
    pub disciplined: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceEmitter {
    /// Detections whose `signal` starts with this prefix come from this emitter.
    pub signal_prefix: String,
    pub position: Point,
    pub evidence: Evidence,
}

/// Fair use of volunteer receivers: a connection-time budget per receiver and UTC day,
/// a minimum gap between connections, and back-off after failures or explicit refusals.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReceiverPolicy {
    pub daily_minutes: f64,
    pub min_gap_s: u64,
}

impl Default for ReceiverPolicy {
    fn default() -> Self {
        ReceiverPolicy {
            daily_minutes: 60.,
            min_gap_s: 60,
        }
    }
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
