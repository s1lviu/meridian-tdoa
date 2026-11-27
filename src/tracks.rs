//! Emitter registry: successive fixes of the same transmitter fused over time.
//! Fixes of one station share systematic errors (ionospheric path, antenna delays), so they are
//! not independent. Covariance intersection (Julier & Uhlmann) fuses them without assuming
//! independence and therefore never becomes more confident than the evidence allows.
use crate::{geo, model::*};
use serde::{Deserialize, Serialize};

/// χ² (2 dof) 99.9% gate: a fix farther than this from the track is kept out as an outlier.
pub const GATE: f64 = 13.816;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub key: String,
    pub callsign: Option<String>,
    pub profile: Option<String>,
    pub band: Option<String>,
    pub frequency_hz: f64,
    pub center: Point,
    /// East/north covariance at `center`, km²: [σee², σen, σnn²].
    pub covariance_km2: [f64; 3],
    pub fixes: usize,
    pub rejected: usize,
    pub first_utc: String,
    pub last_utc: String,
    /// Most recent contributing observation ids (newest last, at most 50).
    pub observations: Vec<String>,
}

impl Track {
    /// Semi-axes (km, 95%) and orientation (degrees clockwise from north) of the ellipse.
    pub fn ellipse_95(&self) -> (f64, f64, f64) {
        let [a, b, c] = self.covariance_km2;
        let tr = a + c;
        let det = a * c - b * b;
        let disc = ((tr * tr / 4.) - det).max(0.).sqrt();
        let (l1, l2) = (tr / 2. + disc, (tr / 2. - disc).max(0.));
        let k = geo::CONTOUR_95.sqrt();
        // Major-axis direction in the east/north frame.
        let angle = 0.5 * (2. * b).atan2(c - a);
        (k * l1.sqrt(), k * l2.sqrt(), angle.to_degrees())
    }
}

fn inv([a, b, c]: [f64; 3]) -> Option<[f64; 3]> {
    let det = a * c - b * b;
    (det > 0.).then(|| [c / det, -b / det, a / det])
}

/// East/north offset (km) of `p` from `origin`.
fn local(origin: Point, p: Point) -> (f64, f64) {
    let d = geo::distance(origin, p);
    let (p1, p2) = (origin.lat.to_radians(), p.lat.to_radians());
    let dl = (p.lon - origin.lon).to_radians();
    let az = (dl.sin() * p2.cos()).atan2(p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos());
    (d * az.sin(), d * az.cos())
}

fn mahalanobis2(dx: (f64, f64), cov: [f64; 3]) -> Option<f64> {
    let [a, b, c] = inv(cov)?;
    Some(a * dx.0 * dx.0 + 2. * b * dx.0 * dx.1 + c * dx.1 * dx.1)
}

/// Covariance intersection of (x1, P1) and (x2, P2), ω minimizing the fused trace.
fn intersect(x2: (f64, f64), p1: [f64; 3], p2: [f64; 3]) -> Option<((f64, f64), [f64; 3])> {
    let (i1, i2) = (inv(p1)?, inv(p2)?);
    let fuse = |w: f64| -> Option<((f64, f64), [f64; 3])> {
        let info = [
            w * i1[0] + (1. - w) * i2[0],
            w * i1[1] + (1. - w) * i2[1],
            w * i1[2] + (1. - w) * i2[2],
        ];
        let p = inv(info)?;
        // x = P (ω P1⁻¹ x1 + (1−ω) P2⁻¹ x2) with x1 = 0 (track origin).
        let v = (
            (1. - w) * (i2[0] * x2.0 + i2[1] * x2.1),
            (1. - w) * (i2[1] * x2.0 + i2[2] * x2.1),
        );
        Some(((p[0] * v.0 + p[1] * v.1, p[1] * v.0 + p[2] * v.1), p))
    };
    // Golden-section search on ω ∈ [0, 1] for the minimal trace.
    let (mut lo, mut hi) = (0f64, 1f64);
    let g = 0.618_033_988_75;
    for _ in 0..60 {
        let (m1, m2) = (hi - g * (hi - lo), lo + g * (hi - lo));
        let t = |w: f64| fuse(w).map_or(f64::MAX, |(_, p)| p[0] + p[2]);
        if t(m1) < t(m2) {
            hi = m2;
        } else {
            lo = m1;
        }
    }
    // Equal information makes the trace flat in ω; keep the symmetric weight then.
    let trace = |w: f64| fuse(w).map_or(f64::MAX, |(_, p)| p[0] + p[2]);
    let w = (lo + hi) / 2.;
    if trace(w) < trace(0.5) * (1. - 1e-9) {
        fuse(w)
    } else {
        fuse(0.5)
    }
}

/// Registry key: the decoded callsign, the reference profile (e.g. DCF77), or the channel.
pub fn key(e: &Emission) -> String {
    if let Some(i) = &e.identity {
        return format!("call:{}", i.callsign);
    }
    match e.reference.as_ref().map(|r| r.profile.as_str()) {
        Some(crate::protocol::PROFILE) => "ref:DCF77".into(),
        Some(p) => format!("{p}:{:.0}", e.frequency_hz),
        None => format!("channel:{:.0}", (e.frequency_hz / 100.).round() * 100.),
    }
}

/// The solution of `e` that can update `track`: its unique estimate, or — for ambiguous
/// observations — the one hypothesis inside the gate, if exactly one is.
fn candidate<'a>(e: &'a Emission, track: Option<&Track>) -> Option<&'a Estimate> {
    if let Some(est) = &e.estimate {
        return est.covariance_km2.is_some().then_some(est);
    }
    let t = track?;
    let inside: Vec<_> = e
        .hypotheses
        .iter()
        .filter(|h| {
            h.covariance_km2.is_some_and(|c| {
                let cov = [
                    c[0] + t.covariance_km2[0],
                    c[1] + t.covariance_km2[1],
                    c[2] + t.covariance_km2[2],
                ];
                mahalanobis2(local(t.center, h.center), cov).is_some_and(|d| d <= GATE)
            })
        })
        .collect();
    (inside.len() == 1).then(|| inside[0])
}

/// Fold one observation into the registry. Returns true when the track changed.
pub fn update(track: &mut Option<Track>, e: &Emission) -> bool {
    let Some(est) = candidate(e, track.as_ref()) else {
        return false;
    };
    let cov = est.covariance_km2.unwrap();
    match track {
        None => {
            *track = Some(Track {
                key: key(e),
                callsign: e.identity.as_ref().map(|i| i.callsign.clone()),
                profile: e.reference.as_ref().map(|r| r.profile.clone()),
                band: crate::bands::name(e.frequency_hz).map(str::to_owned),
                frequency_hz: e.frequency_hz,
                center: est.center,
                covariance_km2: cov,
                fixes: 1,
                rejected: 0,
                first_utc: e.start_utc.clone(),
                last_utc: e.start_utc.clone(),
                observations: vec![e.id.clone()],
            });
            true
        }
        Some(t) => {
            if t.observations.contains(&e.id) {
                return false;
            }
            let dx = local(t.center, est.center);
            let both = [
                cov[0] + t.covariance_km2[0],
                cov[1] + t.covariance_km2[1],
                cov[2] + t.covariance_km2[2],
            ];
            if mahalanobis2(dx, both).is_none_or(|d| d > GATE) {
                t.rejected += 1;
                return true;
            }
            let Some(((east, north), p)) = intersect(dx, t.covariance_km2, cov) else {
                return false;
            };
            t.center = geo::offset(t.center, east, north);
            t.covariance_km2 = p;
            t.fixes += 1;
            t.frequency_hz = e.frequency_hz;
            if e.start_utc > t.last_utc {
                t.last_utc = e.start_utc.clone();
            }
            t.observations.push(e.id.clone());
            if t.observations.len() > 50 {
                t.observations.remove(0);
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fix(id: &str, lat: f64, lon: f64, sigma_km: f64) -> Emission {
        let mut e: Emission =
            serde_json::from_str(include_str!("../data/results/dcf77-20200813.json")).unwrap();
        e.id = id.into();
        e.identity = Some(Identity {
            callsign: "YO3ABC".into(),
            evidence: e.provenance.clone(),
        });
        e.hypotheses.clear();
        let mut est = e.estimate.clone().unwrap_or_else(|| {
            serde_json::from_str::<Emission>(include_str!("../data/results/dcf77-20200813.json"))
                .unwrap()
                .hypotheses[0]
                .clone()
        });
        est.center = Point { lat, lon };
        est.covariance_km2 = Some([sigma_km * sigma_km, 0., sigma_km * sigma_km]);
        e.estimate = Some(est);
        e
    }

    #[test]
    fn fuses_consistent_fixes_rejects_outliers_and_never_shrinks_below_independent_evidence() {
        let mut t = None;
        assert!(update(&mut t, &fix("a", 44.4, 26.1, 100.)));
        assert!(update(&mut t, &fix("b", 44.6, 26.0, 100.)));
        let tr = t.as_ref().unwrap();
        assert_eq!(tr.fixes, 2);
        // Covariance intersection of two equal fixes keeps their covariance (no √2 gain claimed).
        assert!((tr.covariance_km2[0] - 100. * 100.).abs() < 1.);
        assert!(
            geo::distance(
                tr.center,
                Point {
                    lat: 44.5,
                    lon: 26.05
                }
            ) < 3.
        );
        // A precise fix dominates; a far fix is rejected by the gate.
        assert!(update(&mut t, &fix("c", 44.52, 26.04, 10.)));
        assert!(t.as_ref().unwrap().covariance_km2[0] <= 10.5 * 10.5);
        assert!(update(&mut t, &fix("d", 40.0, 20.0, 10.)));
        let (fixes, rejected) = (t.as_ref().unwrap().fixes, t.as_ref().unwrap().rejected);
        assert_eq!((fixes, rejected), (3, 1));
        assert!(
            !update(&mut t, &fix("c", 44.52, 26.04, 10.)),
            "the same observation is folded once"
        );
        let (a, b, _) = t.as_ref().unwrap().ellipse_95();
        assert!(a >= b && b > 0.);
    }
}
