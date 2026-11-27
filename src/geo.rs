use crate::model::{Estimate, Pair, Point, RxResult};
use anyhow::{Result, ensure};
pub const C_KM_S: f64 = 299792.458;
pub fn distance(a: Point, b: Point) -> f64 {
    let (p, q) = (a.lat.to_radians(), b.lat.to_radians());
    let h = ((q - p) / 2.).sin().powi(2)
        + p.cos() * q.cos() * ((b.lon - a.lon).to_radians() / 2.).sin().powi(2);
    6371.0088 * 2. * h.clamp(0., 1.).sqrt().asin()
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

pub fn locate(rx: &[RxResult], pairs: &[Pair]) -> Result<Vec<Estimate>> {
    let used: Vec<_> = rx.iter().filter(|r| r.used).collect();
    ensure!(
        used.len() >= 3,
        "At least three independent, synchronized and correlated receivers"
    );
    let points: Vec<_> = used.iter().map(|r| r.receiver.position).collect();
    let g = geometry(&points);
    ensure!(g > 0.02, "Nearly collinear geometry");
    let reference = &used[0].receiver.id;
    let obs: Vec<_> = used[1..]
        .iter()
        .map(|r| {
            pairs
                .iter()
                .find(|p| {
                    p.accepted
                        && ((&p.a == reference && p.b == r.receiver.id)
                            || (&p.b == reference && p.a == r.receiver.id))
                })
                .map(|p| {
                    (
                        r.receiver.position,
                        if &p.a == reference {
                            p.delay_us
                        } else {
                            -p.delay_us
                        },
                        p.sigma_us,
                    )
                })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| anyhow::anyhow!("Incomplete correlation"))?;
    // Shared-reference covariance: diag sigma², off-diag sigma_i sigma_j / 2.
    let loss = |x: Point| {
        let d0 = distance(x, points[0]);
        let r: Vec<_> = obs
            .iter()
            .map(|(p, delay, sigma)| ((distance(x, *p) - d0) / C_KM_S * 1e6 - delay) / sigma)
            .collect();
        2. * (r.iter().map(|v| v * v).sum::<f64>()
            - r.iter().sum::<f64>().powi(2) / (r.len() + 1) as f64)
    };
    // Entire globe, no transmitter coordinates or viewport supplied to the solver.
    let mut seeds = Vec::with_capacity(64800);
    for lat in -89..90 {
        for lon in -180..180 {
            let p = Point {
                lat: lat as f64,
                lon: lon as f64,
            };
            seeds.push((loss(p), p));
        }
    }
    seeds.sort_by(|a, b| a.0.total_cmp(&b.0));
    let refine = |mut p: Point| {
        let mut step = 1.;
        let mut score = loss(p);
        for _ in 0..220 {
            let mut next = p;
            let mut best = score;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let q = Point {
                        lat: (p.lat + dy as f64 * step).clamp(-89.99, 89.99),
                        lon: (p.lon + dx as f64 * step + 180.).rem_euclid(360.) - 180.,
                    };
                    let s = loss(q);
                    if s < best {
                        best = s;
                        next = q;
                    }
                }
            }
            if best >= score {
                step *= 0.5;
            } else {
                p = next;
                score = best;
            }
            if step < 0.000001 {
                break;
            }
        }
        (score, p)
    };
    let mut candidates: Vec<_> = seeds.iter().take(64).map(|(_, p)| refine(*p)).collect();
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (best, center) = candidates[0];

    ensure!(
        best < 9.,
        "Delays are incompatible with the propagation model"
    );
    let mut distinct = vec![(best, center)];
    for (s, p) in candidates {
        if s <= best + 5.991 && distinct.iter().all(|(_, q)| distance(p, *q) > 200.) {
            distinct.push((s, p));
        }
    }
    let mut solutions = vec![];
    for (best, center) in distinct {
        let mut polygon = vec![];
        let mut radius: f64 = 0.;
        for i in 0..72 {
            let theta = std::f64::consts::TAU * i as f64 / 72.;
            let at = |r: f64| Point {
                lat: (center.lat + r * theta.sin()).clamp(-89.99, 89.99),
                lon: (center.lon
                    + r * theta.cos() / center.lat.to_radians().cos().max(0.05)
                    + 180.)
                    .rem_euclid(360.)
                    - 180.,
            };
            let mut hi = 0.01;
            while loss(at(hi)) - best < 5.991 && hi < 30. {
                hi *= 1.5;
            }
            ensure!(hi < 30., "Unbounded uncertainty region");
            let mut lo = 0.;
            for _ in 0..35 {
                let mid = (lo + hi) / 2.;
                if loss(at(mid)) - best < 5.991 {
                    lo = mid
                } else {
                    hi = mid
                }
            }
            let p = at(hi);
            radius = radius.max(distance(center, p));
            polygon.push(p);
        }
        polygon.push(polygon[0]);
        let residual_us = (obs
            .iter()
            .map(|(p, dt, _)| {
                ((distance(center, *p) - distance(center, points[0])) / C_KM_S * 1e6 - dt).powi(2)
            })
            .sum::<f64>()
            / obs.len() as f64)
            .sqrt();
        solutions.push(Estimate{center,polygon,radius_km:radius,residual_us,geometry_score:g,uncertainty_note:"Δχ²=5.991 contour conditional on propagation at c over a sphere and timing errors. Minimum floor: one sample. Systematic ionospheric/antenna errors are not included; statistical coverage is not calibrated.".into()});
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
