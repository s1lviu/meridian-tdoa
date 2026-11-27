//! Post-hoc timing calibration against a published reference transmitter.
//! Uses only stored measurements; the solver never reads these numbers implicitly.
use crate::{geo, model::Emission, model::Point};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Residuals {
    pub observations: usize,
    pub pairs: usize,
    /// Measured minus modelled delay at the reference position, µs.
    pub mean_us: f64,
    pub median_us: f64,
    pub rms_us: f64,
    pub robust_sigma_us: f64,
    pub p90_abs_us: f64,
    pub reported_sigma_median_us: f64,
    /// rms(residual / reported sigma); 1 means the reported sigmas are calibrated.
    pub normalized_rms: f64,
    /// Least-squares propagation speed through the origin, as a fraction of c.
    pub fitted_speed_over_c: Option<f64>,
}

fn quantile(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * q).round() as usize]
}

/// Residuals of every accepted pair between used receivers, evaluated at `truth`.
pub fn residuals(
    emissions: &[Emission],
    truth: Point,
    distance: fn(Point, Point) -> f64,
) -> Option<Residuals> {
    let mut r = vec![];
    let mut z = vec![];
    let mut reported = vec![];
    let (mut num, mut den) = (0., 0.);
    let mut used = 0;
    for e in emissions {
        let pos = |id: &str| {
            e.receivers
                .iter()
                .find(|x| x.receiver.id == id && x.used)
                .map(|x| x.receiver.position)
        };
        let mut any = false;
        for p in e.pairs.iter().filter(|p| p.accepted) {
            let (Some(a), Some(b)) = (pos(&p.a), pos(&p.b)) else {
                continue;
            };
            let dd = distance(b, truth) - distance(a, truth); // km
            let model = dd / geo::C_KM_S * 1e6;
            r.push(p.delay_us - model);
            z.push((p.delay_us - model) / p.sigma_us);
            reported.push(p.sigma_us);
            num += p.delay_us * 1e-6 * dd;
            den += dd * dd;
            any = true;
        }
        used += any as usize;
    }
    if r.is_empty() {
        return None;
    }
    let n = r.len() as f64;
    let mean = r.iter().sum::<f64>() / n;
    let median = quantile(&mut r.clone(), 0.5);
    let mut dev: Vec<f64> = r.iter().map(|v| (v - median).abs()).collect();
    let mut abs: Vec<f64> = r.iter().map(|v| v.abs()).collect();
    Some(Residuals {
        observations: used,
        pairs: r.len(),
        mean_us: mean,
        median_us: median,
        rms_us: (r.iter().map(|v| v * v).sum::<f64>() / n).sqrt(),
        robust_sigma_us: 1.4826 * quantile(&mut dev, 0.5),
        p90_abs_us: quantile(&mut abs, 0.9),
        reported_sigma_median_us: quantile(&mut reported, 0.5),
        normalized_rms: (z.iter().map(|v| v * v).sum::<f64>() / n).sqrt(),
        // delay = Δd / v  ⇒  1/v = Σ delay·Δd / Σ Δd²
        fitted_speed_over_c: (num > 0.).then(|| den / num / geo::C_KM_S),
    })
}

/// Ray-casting test in a local equirectangular frame around the polygon (regions are small).
pub fn contains(polygon: &[Point], p: Point) -> bool {
    let mut inside = false;
    for w in polygon.windows(2) {
        let (a, b) = (w[0], w[1]);
        let unwrap = |lon: f64| {
            let d = lon - p.lon;
            p.lon + (d + 180.).rem_euclid(360.) - 180.
        };
        let (alon, blon) = (unwrap(a.lon), unwrap(b.lon));
        if (a.lat > p.lat) != (b.lat > p.lat)
            && p.lon < (blon - alon) * (p.lat - a.lat) / (b.lat - a.lat) + alon
        {
            inside = !inside;
        }
    }
    inside
}

/// Squared Mahalanobis distance of `truth` from an estimate's covariance (east/north km²).
pub fn mahalanobis2(e: &crate::model::Estimate, truth: Point) -> Option<f64> {
    let [ee, en, nn] = e.covariance_km2?;
    let d = geo::distance(e.center, truth);
    let az = {
        // initial bearing from centre to truth on the sphere is adequate for tens of km
        let (p1, p2) = (e.center.lat.to_radians(), truth.lat.to_radians());
        let dl = (truth.lon - e.center.lon).to_radians();
        (dl.sin() * p2.cos()).atan2(p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos())
    };
    let (x, y) = (d * az.sin(), d * az.cos());
    let det = ee * nn - en * en;
    (det > 0.).then(|| (nn * x * x - 2. * en * x * y + ee * y * y) / det)
}

#[derive(Debug, Serialize)]
pub struct SolverReport {
    pub observations: usize,
    pub unique: usize,
    pub median_error_km: Option<f64>,
    pub p90_error_km: Option<f64>,
    pub max_error_km: Option<f64>,
    pub median_radius_km: Option<f64>,
    /// Fraction of unique fixes whose 95% region contains the reference.
    pub contour_coverage: Option<f64>,
    /// Fraction with Mahalanobis² ≤ 5.991 under the reported covariance.
    pub covariance_coverage: Option<f64>,
}

fn summarize(
    errors: &mut [f64],
    radii: &mut [f64],
    inside: usize,
    maha: &[f64],
    n: usize,
) -> SolverReport {
    let unique = errors.len();
    let q = |v: &mut [f64], f: f64| (!v.is_empty()).then(|| quantile(v, f));
    SolverReport {
        observations: n,
        unique,
        median_error_km: q(errors, 0.5),
        p90_error_km: q(errors, 0.9),
        max_error_km: q(errors, 1.0),
        median_radius_km: q(radii, 0.5),
        contour_coverage: (unique > 0).then(|| inside as f64 / unique as f64),
        covariance_coverage: (!maha.is_empty()).then(|| {
            maha.iter().filter(|m| **m <= geo::CONTOUR_95).count() as f64 / maha.len() as f64
        }),
    }
}

/// Re-solve stored observations with the current solver and score both against the reference.
pub fn solver(emissions: &[Emission], truth: Point) -> serde_json::Value {
    let (mut old_e, mut old_r, mut old_in) = (vec![], vec![], 0);
    let (mut new_e, mut new_r, mut new_in, mut maha) = (vec![], vec![], 0, vec![]);
    let mut n = 0;
    for e in emissions {
        if e.receivers.iter().filter(|r| r.used).count() < 3 {
            continue;
        }
        n += 1;
        if let Some(est) = &e.estimate {
            old_e.push(geo::distance(est.center, truth));
            old_r.push(est.radius_km);
            old_in += contains(&est.polygon, truth) as usize;
        }
        let used = e.receivers.iter().filter(|r| r.used).count();
        if let Ok(solutions) = geo::locate(&e.receivers, &e.pairs) {
            if solutions.len() == 1 && used >= 4 {
                let s = &solutions[0];
                new_e.push(geo::distance(s.center, truth));
                new_r.push(s.radius_km);
                new_in += contains(&s.polygon, truth) as usize;
                maha.extend(mahalanobis2(s, truth));
            }
        }
    }
    serde_json::json!({
        "stored_solver": summarize(&mut old_e, &mut old_r, old_in, &[], n),
        "current_solver": summarize(&mut new_e, &mut new_r, new_in, &maha, n),
    })
}

/// Sensitivity study: solve with only the pairs that include one reference receiver ("star")
/// for every possible reference, and with all pairs. Scored post-hoc against `truth`.
pub fn formulations(emissions: &[Emission], truth: Point) -> serde_json::Value {
    let mut rows = std::collections::BTreeMap::<String, Vec<f64>>::new();
    for e in emissions {
        let used: Vec<_> = e.receivers.iter().filter(|r| r.used).collect();
        if used.len() < 4 {
            continue;
        }
        let mut variants: Vec<(String, Vec<crate::model::Pair>)> =
            vec![("all_pairs".into(), e.pairs.clone())];
        for r in &used {
            let id = &r.receiver.id;
            variants.push((
                format!("star_{}", &id[..id.len().min(8)]),
                e.pairs
                    .iter()
                    .filter(|p| &p.a == id || &p.b == id)
                    .cloned()
                    .collect(),
            ));
        }
        for (name, pairs) in variants {
            if let Ok(s) = geo::locate(&e.receivers, &pairs) {
                if s.len() == 1 {
                    rows.entry(name)
                        .or_default()
                        .push(geo::distance(s[0].center, truth));
                }
            }
        }
    }
    rows.into_iter()
        .map(|(k, mut v)| {
            let n = v.len();
            (k, serde_json::json!({"unique": n, "median_km": quantile(&mut v, 0.5), "p90_km": quantile(&mut v, 0.9), "max_km": quantile(&mut v, 1.0)}))
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// Centre and half-diagonal (km) of a four-character Maidenhead square (2° × 1°).
pub fn grid4(g: &str) -> Option<(Point, f64)> {
    let b = g.as_bytes();
    if b.len() != 4
        || !(b'A'..=b'R').contains(&b[0])
        || !(b'A'..=b'R').contains(&b[1])
        || !b[2].is_ascii_digit()
        || !b[3].is_ascii_digit()
    {
        return None;
    }
    let lon = -180. + 20. * (b[0] - b'A') as f64 + 2. * (b[2] - b'0') as f64 + 1.;
    let lat = -90. + 10. * (b[1] - b'A') as f64 + (b[3] - b'0') as f64 + 0.5;
    let c = Point { lat, lon };
    let half = [(0.5, 1.), (-0.5, 1.)]
        .iter()
        .map(|(dl, dn)| {
            geo::distance(
                c,
                Point {
                    lat: lat + dl,
                    lon: lon + dn,
                },
            )
        })
        .fold(0., f64::max);
    Some((c, half))
}

/// Post-hoc FT8 evaluation: every observation timed at ≥4 receivers whose message carried a
/// locator, re-solved without the region cap so the actual uncertainty is visible.
pub fn ft8(emissions: &[Emission]) -> serde_json::Value {
    let mut rows = vec![];
    for e in emissions {
        let Some(ev) = e.reference.as_ref().and_then(|r| r.ft8.as_ref()) else {
            continue;
        };
        let Some((truth, half)) = ev.grid.as_deref().and_then(grid4) else {
            continue;
        };
        let mut rx = e.receivers.clone();
        for r in &mut rx {
            r.used = r.confirmed;
        }
        if rx.iter().filter(|r| r.used).count() < 4 {
            continue;
        }
        let row = match geo::locate_bounded(&rx, &e.pairs, geo::C_KM_S, 20_000.) {
            Ok(s) => {
                let best = s
                    .iter()
                    .min_by(|a, b| a.chi2.unwrap_or(0.).total_cmp(&b.chi2.unwrap_or(0.)))
                    .unwrap();
                serde_json::json!({"message":ev.message,"grid":ev.grid,"receivers":rx.iter().filter(|r|r.used).count(),
                    "solutions":s.len(),"error_km":geo::distance(best.center, truth),"grid_half_diagonal_km":half,
                    "radius_km":best.radius_km,"scale":best.scale,"pair_residual_us":best.residual_us,
                    "contains_grid_centre":contains(&best.polygon, truth),"stored_status":e.status})
            }
            Err(err) => {
                serde_json::json!({"message":ev.message,"grid":ev.grid,"failure":err.to_string()})
            }
        };
        rows.push(row);
    }
    serde_json::Value::Array(rows)
}
