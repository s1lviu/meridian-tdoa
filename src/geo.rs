use crate::model::{Estimate, Pair, Point, RxResult};
use anyhow::{Result, ensure};
use geographiclib_rs::{DirectGeodesic, Geodesic, InverseGeodesic};
use std::sync::LazyLock;
pub const C_KM_S: f64 = 299792.458;
static WGS84: LazyLock<Geodesic> = LazyLock::new(Geodesic::wgs84);

/// Geodesic distance on the WGS84 ellipsoid (Karney), in km.
pub fn distance(a: Point, b: Point) -> f64 {
    let metres: f64 = WGS84.inverse(a.lat, a.lon, b.lat, b.lon);
    metres / 1000.
}

/// Great-circle distance on the mean-radius sphere, in km. Only for coarse seeding.
pub fn sphere_distance(a: Point, b: Point) -> f64 {
    let (p, q) = (a.lat.to_radians(), b.lat.to_radians());
    let h = ((q - p) / 2.).sin().powi(2)
        + p.cos() * q.cos() * ((b.lon - a.lon).to_radians() / 2.).sin().powi(2);
    6371.0088 * 2. * h.clamp(0., 1.).sqrt().asin()
}

/// Point reached from `p` along the WGS84 geodesic with the given azimuth (degrees) and length (km).
pub fn destination(p: Point, azimuth_deg: f64, km: f64) -> Point {
    let (lat, lon): (f64, f64) = WGS84.direct(p.lat, p.lon, azimuth_deg, km * 1000.);
    Point {
        lat,
        lon: (lon + 180.).rem_euclid(360.) - 180.,
    }
}
pub fn geometry(p: &[Point]) -> f64 {
    let scale = p
        .iter()
        .flat_map(|a| p.iter().map(move |b| distance(*a, *b)))
        .fold(0., f64::max);
    if scale < 1. {
        return 0.;
    }
    let mut best: f64 = 0.;
    for i in 0..p.len() {
        for j in i + 1..p.len() {
            for k in j + 1..p.len() {
                let a = distance(p[i], p[j]);
                let b = distance(p[j], p[k]);
                let c = distance(p[k], p[i]);
                let s = (a + b + c) / 2.;
                best =
                    best.max(4. * (s * (s - a) * (s - b) * (s - c)).max(0.).sqrt() / scale.powi(2));
            }
        }
    }
    best
}

/// One differential-delay observation between two used receivers (B minus A, µs).
#[derive(Clone, Copy, Debug)]
pub struct Tdoa {
    pub a: Point,
    pub b: Point,
    pub delay_us: f64,
    pub sigma_us: f64,
}

/// χ² of all pair observations at `x` for propagation speed `speed_km_s`.
pub fn chi2(x: Point, obs: &[Tdoa], speed_km_s: f64, dist: fn(Point, Point) -> f64) -> f64 {
    obs.iter()
        .map(|o| {
            let model = (dist(x, o.b) - dist(x, o.a)) / speed_km_s * 1e6;
            ((model - o.delay_us) / o.sigma_us).powi(2)
        })
        .sum()
}

/// Small displacement in a local east/north frame (km), along WGS84 geodesics.
pub fn offset(p: Point, east_km: f64, north_km: f64) -> Point {
    let r = east_km.hypot(north_km);
    if r == 0. {
        return p;
    }
    destination(p, east_km.atan2(north_km).to_degrees(), r)
}

/// Pattern search in the local frame; `step_km` halves until `tol_km`.
fn polish(mut p: Point, f: &dyn Fn(Point) -> f64, mut step_km: f64, tol_km: f64) -> (f64, Point) {
    let mut score = f(p);
    while step_km > tol_km {
        let mut best = (score, p);
        for (dx, dy) in [
            (1., 0.),
            (-1., 0.),
            (0., 1.),
            (0., -1.),
            (0.7, 0.7),
            (-0.7, 0.7),
            (0.7, -0.7),
            (-0.7, -0.7),
        ] {
            let q = offset(p, dx * step_km, dy * step_km);
            let s = f(q);
            if s < best.0 {
                best = (s, q);
            }
        }
        if best.0 < score {
            (score, p) = best;
        } else {
            step_km *= 0.5;
        }
    }
    (score, p)
}

/// Covariance (km², east/north) from the numerical Hessian of χ²: C = 2·H⁻¹.
pub fn covariance(center: Point, f: &dyn Fn(Point) -> f64, h_km: f64) -> Option<[f64; 3]> {
    let l = |e: f64, n: f64| f(offset(center, e, n));
    let l0 = l(0., 0.);
    let hee = (l(h_km, 0.) - 2. * l0 + l(-h_km, 0.)) / (h_km * h_km);
    let hnn = (l(0., h_km) - 2. * l0 + l(0., -h_km)) / (h_km * h_km);
    let hen =
        (l(h_km, h_km) - l(h_km, -h_km) - l(-h_km, h_km) + l(-h_km, -h_km)) / (4. * h_km * h_km);
    let det = hee * hnn - hen * hen;
    (hee > 0. && hnn > 0. && det > 0.).then(|| [2. * hnn / det, -2. * hen / det, 2. * hee / det])
}

/// Δχ² for a two-parameter 95% region.
pub const CONTOUR_95: f64 = 5.991;

fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<Vec<f64>>) -> Option<Vec<Vec<f64>>> {
    // Gauss–Jordan with partial pivoting: returns A⁻¹B.
    let n = a.len();
    for c in 0..n {
        let piv = (c..n).max_by(|&x, &y| a[x][c].abs().total_cmp(&a[y][c].abs()))?;
        a.swap(c, piv);
        b.swap(c, piv);
        let d = a[c][c];
        if d.abs() < 1e-300 {
            return None;
        }
        for r in 0..n {
            if r != c {
                let f = a[r][c] / d;
                if f != 0. {
                    for k in 0..n {
                        a[r][k] -= f * a[c][k];
                    }
                    for k in 0..b[r].len() {
                        b[r][k] -= f * b[c][k];
                    }
                }
            }
        }
    }
    Some(
        (0..n)
            .map(|i| b[i].iter().map(|v| v / a[i][i]).collect())
            .collect(),
    )
}

/// Pair delays reduced to n−1 independent differences with their generalized covariance.
/// Each pair is τ_ab = u_b − u_a + pair noise; each receiver also has its own timing noise.
/// Result: v_i = u_i − u_0 (µs), the inverse covariance of v, the closure χ² and its dof.
pub struct Reduced {
    pub sites: Vec<Point>,
    pub v: Vec<f64>,
    pub weight: Vec<Vec<f64>>,
    pub closure_chi2: f64,
    pub closure_dof: usize,
    pub pair_scale: f64,
}

pub fn reduce(sites: &[Point], obs: &[Tdoa]) -> Option<Reduced> {
    let n = sites.len();
    let idx = |p: Point| sites.iter().position(|q| q.lat == p.lat && q.lon == p.lon);
    let mut l = vec![vec![0.0f64; n]; n];
    let mut b = vec![0.0f64; n];
    let mut sig2 = vec![];
    for o in obs {
        let (i, j) = (idx(o.a)?, idx(o.b)?);
        let w = 1. / (o.sigma_us * o.sigma_us);
        l[i][i] += w;
        l[j][j] += w;
        l[i][j] -= w;
        l[j][i] -= w;
        b[j] += w * o.delay_us;
        b[i] -= w * o.delay_us;
        sig2.push(o.sigma_us * o.sigma_us);
    }
    // L⁺ = (L + J/n)⁻¹ − J/n for a connected graph.
    let lj: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|k| l[i][k] + 1. / n as f64).collect())
        .collect();
    let eye: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|k| (i == k) as u8 as f64).collect())
        .collect();
    let inv = solve(lj, eye)?;
    let lplus: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|k| inv[i][k] - 1. / n as f64).collect())
        .collect();
    let u: Vec<f64> = (0..n)
        .map(|i| (0..n).map(|k| lplus[i][k] * b[k]).sum())
        .collect();
    let closure_chi2: f64 = obs
        .iter()
        .filter_map(|o| Some(((o.delay_us - (u[idx(o.b)?] - u[idx(o.a)?])) / o.sigma_us).powi(2)))
        .sum();
    let closure_dof = obs.len().saturating_sub(n - 1);
    let pair_scale = if closure_dof > 0 {
        (closure_chi2 / closure_dof as f64).max(1.)
    } else {
        1.
    };
    sig2.sort_by(f64::total_cmp);
    let receiver_var = sig2[sig2.len() / 2] / 2.;
    // Cov(u) = s·L⁺ + σr²(I − J/n); v = A u with A_i = e_{i+1} − e_0.
    let cov_u = |i: usize, k: usize| {
        pair_scale * lplus[i][k] + receiver_var * ((i == k) as u8 as f64 - 1. / n as f64)
    };
    let m = n - 1;
    let cov_v: Vec<Vec<f64>> = (0..m)
        .map(|i| {
            (0..m)
                .map(|k| cov_u(i + 1, k + 1) - cov_u(i + 1, 0) - cov_u(0, k + 1) + cov_u(0, 0))
                .collect()
        })
        .collect();
    let eye_m: Vec<Vec<f64>> = (0..m)
        .map(|i| (0..m).map(|k| (i == k) as u8 as f64).collect())
        .collect();
    let weight = solve(cov_v, eye_m)?;
    Some(Reduced {
        sites: sites.to_vec(),
        v: (1..n).map(|i| u[i] - u[0]).collect(),
        weight,
        closure_chi2,
        closure_dof,
        pair_scale,
    })
}

impl Reduced {
    /// Position part of the generalized χ² at `x` (n−3 degrees of freedom at the optimum).
    pub fn chi2(&self, x: Point, speed_km_s: f64, dist: fn(Point, Point) -> f64) -> f64 {
        let d0 = dist(x, self.sites[0]);
        let r: Vec<f64> = (1..self.sites.len())
            .map(|i| self.v[i - 1] - (dist(x, self.sites[i]) - d0) / speed_km_s * 1e6)
            .collect();
        let mut s = 0.;
        for (i, ri) in r.iter().enumerate() {
            for (k, rk) in r.iter().enumerate() {
                s += ri * self.weight[i][k] * rk;
            }
        }
        s
    }
}

/// Locate an emitter from every accepted pair among the used receivers.
/// The transmitter position, a search domain or a viewport are never inputs.
/// Fewer than four receivers cannot exclude the mirror solution and never yield `located`.
pub fn locate(rx: &[RxResult], pairs: &[Pair]) -> Result<Vec<Estimate>> {
    locate_with_speed(rx, pairs, C_KM_S)
}

pub fn locate_with_speed(
    rx: &[RxResult],
    pairs: &[Pair],
    speed_km_s: f64,
) -> Result<Vec<Estimate>> {
    locate_bounded(rx, pairs, speed_km_s, MAX_REGION_KM)
}

/// Largest 95% region radius reported as a position; beyond it the result is undetermined.
pub const MAX_REGION_KM: f64 = 4000.;

pub fn locate_bounded(
    rx: &[RxResult],
    pairs: &[Pair],
    speed_km_s: f64,
    max_region_km: f64,
) -> Result<Vec<Estimate>> {
    let used: Vec<_> = rx.iter().filter(|r| r.used).collect();
    ensure!(
        used.len() >= 3,
        "At least three independent, synchronized and correlated receivers"
    );
    let points: Vec<_> = used.iter().map(|r| r.receiver.position).collect();
    let g = geometry(&points);
    ensure!(g > 0.02, "Nearly collinear geometry");
    let pos = |id: &str| {
        used.iter()
            .find(|r| r.receiver.id == id)
            .map(|r| r.receiver.position)
    };
    let obs: Vec<Tdoa> = pairs
        .iter()
        .filter(|p| p.accepted)
        .filter_map(|p| {
            Some(Tdoa {
                a: pos(&p.a)?,
                b: pos(&p.b)?,
                delay_us: p.delay_us,
                sigma_us: p.sigma_us.max(1e-3),
            })
        })
        .collect();
    // Every used receiver must be connected by accepted pairs.
    let mut reached = vec![used[0].receiver.position];
    loop {
        let before = reached.len();
        for o in &obs {
            let ha = reached.iter().any(|p| p.lat == o.a.lat && p.lon == o.a.lon);
            let hb = reached.iter().any(|p| p.lat == o.b.lat && p.lon == o.b.lon);
            if ha && !hb {
                reached.push(o.b);
            } else if hb && !ha {
                reached.push(o.a);
            }
        }
        if reached.len() == before {
            break;
        }
    }
    ensure!(reached.len() == points.len(), "Incomplete correlation");
    let reduced = reduce(&points, &obs).ok_or_else(|| anyhow::anyhow!("Singular delay network"))?;
    let coarse = |x: Point| reduced.chi2(x, speed_km_s, sphere_distance);
    let fine = |x: Point| reduced.chi2(x, speed_km_s, distance);
    // Global 1° grid on the sphere: no transmitter coordinates, viewport or region are supplied.
    let mut seeds = Vec::with_capacity(64800);
    for lat in -89..90 {
        for lon in -180..180 {
            let p = Point {
                lat: lat as f64,
                lon: lon as f64,
            };
            seeds.push((coarse(p), p));
        }
    }
    seeds.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut starts: Vec<Point> = vec![];
    for (_, p) in &seeds {
        if starts.len() >= 12 {
            break;
        }
        if starts.iter().all(|q| sphere_distance(*p, *q) > 400.) {
            starts.push(*p);
        }
    }
    let mut minima: Vec<(f64, Point)> = starts
        .iter()
        .map(|p| {
            let (_, q) = polish(*p, &coarse, 120., 0.5);
            polish(q, &fine, 2., 1e-4)
        })
        .collect();
    minima.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (best, _) = minima[0];
    let dof = points.len().saturating_sub(3); // n−1 independent delays, two coordinates
    // A-posteriori factor on the position fit; never shrinks the region.
    let scale = if dof > 0 {
        (best / dof as f64).max(1.)
    } else {
        1.
    };
    ensure!(
        scale < 100. && reduced.pair_scale < 100.,
        "Delays are incompatible with the propagation model"
    );
    let threshold = CONTOUR_95 * scale;
    let mut distinct: Vec<(f64, Point)> = vec![];
    for (s, p) in minima {
        if s <= best + threshold && distinct.iter().all(|(_, q)| distance(p, *q) > 200.) {
            distinct.push((s, p));
        }
    }
    let mut solutions = vec![];
    for (score, center) in distinct {
        let mut polygon = vec![];
        let mut radius: f64 = 0.;
        for i in 0..72 {
            let az = 5. * i as f64;
            let inside = |r: f64| fine(destination(center, az, r)) - score < threshold;
            let mut hi = 0.5;
            while inside(hi) && hi < max_region_km {
                hi *= 1.6;
            }
            ensure!(hi < max_region_km, "Unbounded uncertainty region");
            let mut lo = 0.;
            for _ in 0..30 {
                let mid = (lo + hi) / 2.;
                if inside(mid) { lo = mid } else { hi = mid }
            }
            let p = destination(center, az, hi);
            radius = radius.max(hi);
            polygon.push(p);
        }
        polygon.push(polygon[0]);
        let residual_us = (obs
            .iter()
            .map(|o| {
                ((distance(center, o.b) - distance(center, o.a)) / speed_km_s * 1e6 - o.delay_us)
                    .powi(2)
            })
            .sum::<f64>()
            / obs.len() as f64)
            .sqrt();
        let h = (radius / 20.).clamp(0.05, 50.);
        let covariance_km2 = covariance(center, &fine, h).map(|c| c.map(|v| v * scale));
        solutions.push(Estimate {
            center,
            polygon,
            radius_km: radius,
            residual_us,
            geometry_score: g,
            uncertainty_note: format!(
                "95% region (Δχ²={:.3}) from {} pair delays reduced to {} independent differences on the WGS84 ellipsoid at {:.0} km/s. Pair disagreement scales the pair noise by {:.2}; the position fit by {:.2}. Ionospheric paths and antenna delays can shift the estimate.",
                threshold, obs.len(), points.len() - 1, speed_km_s, reduced.pair_scale, scale
            ),
            chi2: Some(score),
            dof: Some(dof),
            scale: Some(scale),
            covariance_km2,
        });
    }
    Ok(solutions)
}

/// Polygon/rectangle intersection, including a viewport entirely inside the region.
pub fn intersects(poly: &[Point], bbox: [f64; 4]) -> bool {
    if bbox[0] > bbox[2] {
        return intersects(poly, [bbox[0], bbox[1], 180., bbox[3]])
            || intersects(poly, [-180., bbox[1], bbox[2], bbox[3]]);
    }
    if poly.is_empty() {
        return false;
    }
    let mut unwrapped = vec![poly[0]];
    for p in &poly[1..] {
        let last = unwrapped.last().unwrap().lon;
        let mut lon = p.lon;
        while lon - last > 180. {
            lon -= 360.;
        }
        while lon - last < -180. {
            lon += 360.;
        }
        unwrapped.push(Point { lat: p.lat, lon });
    }
    let mean = unwrapped.iter().map(|p| p.lon).sum::<f64>() / unwrapped.len() as f64;
    let shift = (((bbox[0] + bbox[2]) / 2. - mean) / 360.).round() * 360.;
    [-360., 0., 360.].iter().any(|extra| {
        let adjusted: Vec<_> = unwrapped
            .iter()
            .map(|p| Point {
                lat: p.lat,
                lon: p.lon + shift + extra,
            })
            .collect();
        intersects_flat(&adjusted, bbox)
    })
}
fn intersects_flat(poly: &[Point], bbox: [f64; 4]) -> bool {
    let [w, s, e, n] = bbox;
    let inside = |p: &Point| p.lon >= w && p.lon <= e && p.lat >= s && p.lat <= n;
    if poly.iter().any(inside) {
        return true;
    }
    for corner in [
        Point { lat: s, lon: w },
        Point { lat: n, lon: w },
        Point { lat: s, lon: e },
        Point { lat: n, lon: e },
    ] {
        let mut isin = false;
        for edge in poly.windows(2) {
            let (a, b) = (edge[0], edge[1]);
            if (a.lat > corner.lat) != (b.lat > corner.lat)
                && corner.lon < (b.lon - a.lon) * (corner.lat - a.lat) / (b.lat - a.lat) + a.lon
            {
                isin = !isin;
            }
        }
        if isin {
            return true;
        }
    }
    poly.windows(2).any(|v| {
        let (a, b) = (v[0], v[1]);
        let (dx, dy) = (b.lon - a.lon, b.lat - a.lat);
        let (mut low, mut high) = (0f64, 1f64);
        for (p, q) in [
            (-dx, a.lon - w),
            (dx, e - a.lon),
            (-dy, a.lat - s),
            (dy, n - a.lat),
        ] {
            if p == 0. {
                if q < 0. {
                    return false;
                }
            } else {
                let t = q / p;
                if p < 0. {
                    low = low.max(t)
                } else {
                    high = high.min(t)
                }
            }
        }
        low <= high
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(
        truth: Point,
        sites: &[Point],
        sigma_us: f64,
        seed: &mut u64,
    ) -> (Vec<RxResult>, Vec<Pair>) {
        synthetic_noise(truth, sites, sigma_us, 0., seed)
    }

    /// `sigma_us` per-receiver arrival noise plus independent `pair_us` noise on every pair.
    fn synthetic_noise(
        truth: Point,
        sites: &[Point],
        sigma_us: f64,
        pair_us: f64,
        seed: &mut u64,
    ) -> (Vec<RxResult>, Vec<Pair>) {
        let mut gauss = || {
            // Box–Muller on a 64-bit LCG; deterministic.
            let mut u = || {
                *seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((*seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
            };
            (-2. * u().ln()).sqrt() * (std::f64::consts::TAU * u()).cos()
        };
        let rx: Vec<RxResult> = sites
            .iter()
            .enumerate()
            .map(|(i, p)| RxResult {
                receiver: crate::model::Receiver {
                    id: format!("r{i}"),
                    label: String::new(),
                    independent_device: format!("r{i}"),
                    position: *p,
                    position_evidence: crate::model::Evidence {
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
                sample_rate_hz: 12000.,
                signal_score_db: 30.,
                samples: 0,
            })
            .collect();
        // Per-receiver arrival noise, so pair errors are correlated exactly as in a real TOA network.
        let toa: Vec<f64> = sites
            .iter()
            .map(|p| distance(truth, *p) / C_KM_S * 1e6 + sigma_us * gauss())
            .collect();
        let mut pairs = vec![];
        for i in 0..sites.len() {
            for j in i + 1..sites.len() {
                pairs.push(Pair {
                    a: format!("r{i}"),
                    b: format!("r{j}"),
                    delay_us: toa[j] - toa[i] + pair_us * gauss(),
                    sigma_us: (2. * sigma_us * sigma_us + pair_us * pair_us).sqrt(),
                    coherence: 1.,
                    peak_ratio: 2.,
                    windows: 10,
                    accepted: true,
                    correlation: vec![],
                });
            }
        }
        (rx, pairs)
    }

    #[test]
    fn solver_recovers_a_synthetic_emitter_and_its_region_is_not_overconfident() {
        let truth = Point {
            lat: 45.5,
            lon: 25.0,
        };
        let sites = [
            Point {
                lat: 47.2,
                lon: 22.1,
            },
            Point {
                lat: 44.0,
                lon: 21.5,
            },
            Point {
                lat: 43.6,
                lon: 27.9,
            },
            Point {
                lat: 47.6,
                lon: 27.6,
            },
            Point {
                lat: 46.0,
                lon: 24.6,
            },
        ];
        let mut seed = 42;
        // Both noise structures seen in practice: per-receiver timing and pair-specific bias.
        let (mut covered, mut maha_ok, trials) = (0, 0, 60);
        for k in 0..trials {
            let (rx, pairs) = if k % 2 == 0 {
                synthetic(truth, &sites, 30., &mut seed)
            } else {
                synthetic_noise(truth, &sites, 0., 40., &mut seed)
            };
            let s = locate(&rx, &pairs).unwrap();
            assert_eq!(
                s.len(),
                1,
                "five receivers around the emitter give one solution"
            );
            let est = &s[0];
            assert!(distance(est.center, truth) < 25.);
            covered += crate::calibration::contains(&est.polygon, truth) as usize;
            maha_ok +=
                (crate::calibration::mahalanobis2(est, truth).unwrap() <= CONTOUR_95) as usize;
        }
        // The pairs over-count independent information (10 pairs, 4 independent delays),
        // which the a-posteriori factor partly offsets; require at least nominal coverage.
        eprintln!("coverage: contour {covered}/{trials}, covariance {maha_ok}/{trials}");
        assert!(
            covered as f64 / trials as f64 >= 0.9,
            "contour coverage {covered}/{trials}"
        );
        assert!(
            maha_ok as f64 / trials as f64 >= 0.9,
            "covariance coverage {maha_ok}/{trials}"
        );
    }

    #[test]
    fn three_receivers_keep_both_mirror_solutions() {
        let truth = Point {
            lat: 50.0,
            lon: 9.0,
        };
        let sites = [
            Point {
                lat: 46.5,
                lon: 8.8,
            },
            Point {
                lat: 51.5,
                lon: 12.0,
            },
            Point {
                lat: 51.5,
                lon: 3.6,
            },
        ];
        let (rx, pairs) = synthetic(truth, &sites, 1., &mut 7);
        let s = locate(&rx, &pairs).unwrap();
        assert!(s.len() >= 2, "a 2-TDoA system has a mirror intersection");
        assert!(s.iter().any(|e| distance(e.center, truth) < 5.));
    }
    #[test]
    fn viewport_inside_polygon() {
        let p = vec![
            Point { lat: 0., lon: 0. },
            Point { lat: 0., lon: 10. },
            Point { lat: 10., lon: 10. },
            Point { lat: 10., lon: 0. },
            Point { lat: 0., lon: 0. },
        ];
        assert!(intersects(&p, [2., 2., 3., 3.]));
        assert!(!intersects(&p, [20., 20., 30., 30.]));
        assert!(intersects(&p, [9., 4., 12., 5.]));
    }
    #[test]
    fn collinearity_rejected() {
        assert!(
            geometry(&[
                Point { lat: 0., lon: 1. },
                Point { lat: 0., lon: 2. },
                Point { lat: 0., lon: 3. }
            ]) < 0.02
        );
    }
    #[test]
    fn dateline_intersection_is_not_a_worldwide_polygon() {
        let p = vec![
            Point { lat: 0., lon: 179. },
            Point {
                lat: 0.,
                lon: -179.,
            },
            Point {
                lat: 2.,
                lon: -179.,
            },
            Point { lat: 2., lon: 179. },
            Point { lat: 0., lon: 179. },
        ];
        assert!(intersects(&p, [179.5, 0.5, -179.5, 1.5]));
        assert!(!intersects(&p, [-10., 0., 10., 2.]));
    }
}
