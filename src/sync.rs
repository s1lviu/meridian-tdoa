//! Clock synchronization from reference emitters, for receivers without disciplined time
//! (RTL-SDR, Airspy, HackRF, …): the multilateration approach proven by ADS-B MLAT networks.
//!
//! Each node reports arrival times in its own free-running clock. A reference emitter at a
//! known position, received in the same stream, gives for node i relative to master m:
//!   z = (t_i − t_m) − (d_i − d_m)/c = b_i(t) − b_m(t) + noise,
//! so every node's clock offset relative to the master is tracked by a Kalman filter on
//! [offset, drift]. Target arrival times are corrected with it before multilateration.
//! Reference positions are calibration inputs; a reference is never evaluated against itself.
use crate::{geo, model::Point};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One node's arrival-time measurement of one emission, in the node's own clock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Detection {
    pub node: String,
    /// Identifies the emission across nodes (decoded content hash, slot, sequence number …).
    pub signal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transmitter: Option<String>,
    #[serde(default)]
    pub free_running: bool,
    /// Arrival time in the node's clock, seconds (any epoch, consistent per node).
    pub local_s: f64,
    pub sigma_s: f64,
}

/// Kalman state of one node's clock relative to the master: offset (s) and drift (s/s).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Clock {
    pub offset_s: f64,
    pub drift: f64,
    /// Covariance [[p00, p01], [p01, p11]].
    pub p: [f64; 3],
    pub at_s: f64,
    pub updates: usize,
}

/// Random-walk process noise: frequency wander of a TCXO/crystal (s²/s³ on drift).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ClockNoise {
    pub drift_rw: f64,
}

impl Default for ClockNoise {
    fn default() -> Self {
        // 1e-9 (1 ppb)/√s random-walk of drift: an ordinary crystal over seconds to minutes.
        ClockNoise { drift_rw: 1e-18 }
    }
}

impl Clock {
    fn predict(&mut self, t: f64, q: ClockNoise) {
        let dt = t - self.at_s;
        if dt <= 0. {
            return;
        }
        self.offset_s += self.drift * dt;
        let [p00, p01, p11] = self.p;
        // F = [[1, dt], [0, 1]], Q on drift only (integrated into offset).
        let qd = q.drift_rw * dt;
        self.p = [
            p00 + 2. * dt * p01 + dt * dt * p11 + qd * dt * dt / 3.,
            p01 + dt * p11 + qd * dt / 2.,
            p11 + qd,
        ];
        self.at_s = t;
    }

    fn update(&mut self, z: f64, r: f64) -> f64 {
        let [p00, p01, p11] = self.p;
        let innovation = z - self.offset_s;
        let s = p00 + r;
        let (k0, k1) = (p00 / s, p01 / s);
        self.offset_s += k0 * innovation;
        self.drift += k1 * innovation;
        self.p = [(1. - k0) * p00, (1. - k0) * p01, p11 - k1 * p01];
        self.updates += 1;
        innovation / s.sqrt()
    }

    /// Offset relative to the master at node-clock time `t`, with its standard deviation.
    pub fn at(&self, t: f64) -> (f64, f64) {
        let dt = t - self.at_s;
        let [p00, p01, p11] = self.p;
        (
            self.offset_s + self.drift * dt,
            (p00 + 2. * dt * p01 + dt * dt * p11).max(0.).sqrt(),
        )
    }
}

/// Network clock state: master node and every other node's clock relative to it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Network {
    pub master: Option<String>,
    pub clocks: BTreeMap<String, Clock>,
    pub noise: Option<ClockNoise>,
    /// Innovations beyond 5σ are rejected (multipath, wrong association).
    pub rejected: usize,
}

impl Network {
    /// Fold one reference emission (known position) seen by several nodes.
    pub fn reference(
        &mut self,
        reference: Point,
        nodes: &BTreeMap<String, Point>,
        seen: &[Detection],
    ) {
        let q = self.noise.unwrap_or_default();
        let master = self
            .master
            .get_or_insert_with(|| {
                seen.iter()
                    .map(|d| d.node.clone())
                    .min()
                    .unwrap_or_default()
            })
            .clone();
        let Some(m) = seen.iter().find(|d| d.node == master) else {
            return;
        };
        let Some(&pm) = nodes.get(&master) else {
            return;
        };
        let dm = geo::distance(reference, pm) / geo::C_KM_S;
        for d in seen.iter().filter(|d| d.node != master) {
            let Some(&pi) = nodes.get(&d.node) else {
                continue;
            };
            let di = geo::distance(reference, pi) / geo::C_KM_S;
            let z = (d.local_s - m.local_s) - (di - dm);
            let r = d.sigma_s.powi(2) + m.sigma_s.powi(2);
            let clock = self.clocks.entry(d.node.clone()).or_insert(Clock {
                offset_s: z,
                drift: 0.,
                // Unknown drift at start: ±100 ppm.
                p: [r, 0., 1e-8],
                at_s: d.local_s,
                updates: 0,
            });
            if clock.updates == 0 && clock.at_s == d.local_s {
                clock.updates = 1;
                continue;
            }
            if d.local_s <= clock.at_s {
                continue;
            }
            clock.predict(d.local_s, q);
            let (pred, sd) = clock.at(d.local_s);
            if (z - pred).abs() > 5. * (sd * sd + r).sqrt() && clock.updates >= 3 {
                self.rejected += 1;
                continue;
            }
            clock.update(z, r);
        }
    }

    /// Arrival times of a target emission on the master's time base, with their σ.
    /// Nodes without a synchronized clock are omitted.
    pub fn corrected(&self, seen: &[Detection]) -> Vec<(String, f64, f64)> {
        self.corrected_with(seen, &std::collections::HashSet::new())
    }

    /// As `corrected`, with GNSS-disciplined nodes taken on their own (UTC) time base.
    pub fn corrected_with(
        &self,
        seen: &[Detection],
        disciplined: &std::collections::HashSet<String>,
    ) -> Vec<(String, f64, f64)> {
        seen.iter()
            .filter_map(|d| {
                if disciplined.contains(&d.node) || self.master.as_deref() == Some(d.node.as_str())
                {
                    return Some((d.node.clone(), d.local_s, d.sigma_s));
                }
                self.master.as_ref()?;
                let c = self.clocks.get(&d.node).filter(|c| c.updates >= 3)?;
                let (offset, sd) = c.at(d.local_s);
                Some((
                    d.node.clone(),
                    d.local_s - offset,
                    (d.sigma_s.powi(2) + sd * sd).sqrt(),
                ))
            })
            .collect()
    }
}

/// Turn one associated target emission into a stored observation (profile `mlat_v1`).
pub fn emission(
    signal: &str,
    arrivals: &[(String, f64, f64)],
    nodes: &[crate::model::Node],
    frequency_hz: f64,
    utc: chrono::DateTime<chrono::Utc>,
) -> crate::model::Emission {
    use crate::model::*;
    let positions: BTreeMap<String, Point> =
        nodes.iter().map(|n| (n.id.clone(), n.position)).collect();
    let receivers: Vec<RxResult> = arrivals
        .iter()
        .filter_map(|(id, _, _)| {
            let n = nodes.iter().find(|n| &n.id == id)?;
            Some(RxResult {
                receiver: Receiver {
                    id: n.id.clone(),
                    label: n.label.clone(),
                    independent_device: n.id.clone(),
                    position: n.position,
                    position_evidence: n.position_evidence.clone(),
                    position_uncertainty_m: 0.,
                    file: String::new(),
                    sha256: String::new(),
                    timing_sigma_ns: None,
                },
                confirmed: true,
                used: false,
                synchronized: true,
                reason: None,
                gps_age_max_s: 0,
                sample_rate_hz: 0.,
                signal_score_db: 0.,
                samples: 0,
            })
        })
        .collect();
    let ts = utc.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut e = Emission {
        id: format!("mlat-{}-{}", utc.timestamp_millis(), signal),
        dataset_id: format!("mlat-{}", utc.timestamp()),
        mode: "live".into(),
        fault_injected: false,
        title: "Node detections".into(),
        frequency_hz,
        start_utc: ts.clone(),
        end_utc: ts,
        status: "undetermined".into(),
        reason: None,
        identity: None,
        provenance: Evidence {
            url: "https://github.com/s1lviu/meridian-tdoa/blob/main/docs/API.md#node-detections"
                .into(),
            note: format!(
                "Arrival times reported by {} own nodes, clock-corrected from reference emitters.",
                receivers.len()
            ),
        },
        receivers,
        pairs: pairs(arrivals, &positions),
        estimate: None,
        hypotheses: vec![],
        detected: true,
        spectrum: vec![],
        processed_at: chrono::Utc::now().to_rfc3339(),
        reference: Some(VerifiedReference {
            node_utc: None,
            profile: "mlat_v1".into(),
            three_receivers: None,
            cw: vec![],
            ft8: None,
        }),
        excluded: vec![],
    };
    crate::pipeline::locate_receptions(&mut e);
    if e.estimate.is_some() {
        e.reason = None;
    }
    e
}

/// Pair records for the solver from synchronized arrival times.
pub fn pairs(
    arrivals: &[(String, f64, f64)],
    nodes: &BTreeMap<String, Point>,
) -> Vec<crate::model::Pair> {
    let mut out = vec![];
    for i in 0..arrivals.len() {
        for j in i + 1..arrivals.len() {
            let (a, b) = (&arrivals[i], &arrivals[j]);
            let (Some(pa), Some(pb)) = (nodes.get(&a.0), nodes.get(&b.0)) else {
                continue;
            };
            let delay = (b.1 - a.1) * 1e6;
            let sigma = (a.2 * a.2 + b.2 * b.2).sqrt() * 1e6;
            let physical = geo::distance(*pa, *pb) / geo::C_KM_S * 1e6;
            out.push(crate::model::Pair {
                a: a.0.clone(),
                b: b.0.clone(),
                delay_us: delay,
                sigma_us: sigma,
                coherence: 1.,
                peak_ratio: 1.,
                windows: 1,
                accepted: delay.abs() <= physical + 3. * sigma,
                correlation: vec![],
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn rx(id: &str, p: Point) -> RxResult {
        RxResult {
            receiver: Receiver {
                id: id.into(),
                label: id.into(),
                independent_device: id.into(),
                position: p,
                position_evidence: Evidence {
                    url: "https://example.org".into(),
                    note: "synthetic".into(),
                },
                position_uncertainty_m: 0.,
                file: String::new(),
                sha256: String::new(),
                timing_sigma_ns: None,
            },
            confirmed: true,
            used: true,
            synchronized: true,
            reason: None,
            gps_age_max_s: 0,
            sample_rate_hz: 0.,
            signal_score_db: 0.,
            samples: 0,
        }
    }

    /// Regional VHF-like network of free-running receivers (offsets up to ±5 ms, drifts up to
    /// ±40 ppm, 100 ns arrival noise). A broadcast reference every second synchronizes them;
    /// an unknown target is then located. Without synchronization the same data is useless.
    #[test]
    fn reference_emitter_synchronizes_free_running_clocks_for_multilateration() {
        let nodes: BTreeMap<String, Point> = [
            ("n0", 44.40, 26.10),
            ("n1", 44.55, 25.80),
            ("n2", 44.20, 25.90),
            ("n3", 44.45, 26.45),
            ("n4", 44.25, 26.30),
        ]
        .iter()
        .map(|(id, lat, lon)| {
            (
                id.to_string(),
                Point {
                    lat: *lat,
                    lon: *lon,
                },
            )
        })
        .collect();
        let reference = Point {
            lat: 44.43,
            lon: 26.02,
        }; // e.g. a broadcast transmitter
        let target = Point {
            lat: 44.36,
            lon: 26.20,
        };
        let clock = |i: usize, t: f64| -> f64 {
            let offset = [0.0, 0.0031, -0.0047, 0.0012, -0.0020][i];
            let drift = [0.0, 31e-6, -12e-6, 40e-6, -25e-6][i];
            t + offset + drift * t
        };
        let mut seed = 9u64;
        let mut gauss = move || {
            let mut u = || {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
            };
            (-2. * u().ln()).sqrt() * (std::f64::consts::TAU * u()).cos()
        };
        let sigma = 100e-9;
        let observe = |emitter: Point,
                       t: f64,
                       signal: &str,
                       gauss: &mut dyn FnMut() -> f64|
         -> Vec<Detection> {
            nodes
                .iter()
                .enumerate()
                .map(|(i, (id, p))| Detection {
                    transmitter: None,
                    free_running: true,
                    node: id.clone(),
                    signal: signal.into(),
                    local_s: clock(i, t + geo::distance(emitter, *p) / geo::C_KM_S)
                        + sigma * gauss(),
                    sigma_s: sigma,
                })
                .collect()
        };
        let mut net = Network::default();
        for k in 0..30 {
            let seen = observe(reference, 1000. + k as f64, &format!("ref-{k}"), &mut gauss);
            net.reference(reference, &nodes, &seen);
        }
        let seen = observe(target, 1030.4, "target", &mut gauss);
        let arrivals = net.corrected(&seen);
        assert_eq!(arrivals.len(), 5);
        let rxs: Vec<RxResult> = nodes.iter().map(|(id, p)| rx(id, *p)).collect();
        let solved = geo::locate(&rxs, &pairs(&arrivals, &nodes)).unwrap();
        let best = &solved[0];
        let err = geo::distance(best.center, target) * 1000.;
        eprintln!(
            "synchronized network: target error {err:.0} m, 95% radius {:.2} km",
            best.radius_km
        );
        assert_eq!(solved.len(), 1);
        assert!(err < 200., "error {err} m");
        // The same raw arrivals without the clock models do not give the target.
        let raw: Vec<(String, f64, f64)> = seen
            .iter()
            .map(|d| (d.node.clone(), d.local_s, d.sigma_s))
            .collect();
        let unsynced = geo::locate(&rxs, &pairs(&raw, &nodes));
        assert!(
            unsynced.is_err()
                || unsynced
                    .unwrap()
                    .iter()
                    .all(|s| geo::distance(s.center, target) > 5.)
        );
    }
}
