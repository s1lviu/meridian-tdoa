//! Post-hoc evaluation for published reference beacons. No imports into the solver or receiver selection.
use chrono::{DateTime, Utc};
use meridian_tdoa::{
    geo, ibp,
    model::{Emission, Point},
    protocol,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

#[derive(Deserialize)]
struct Catalog {
    source: String,
    locators: HashMap<String, String>,
}
#[derive(Deserialize)]
struct Dcf {
    lat: f64,
    lon: f64,
    source: String,
}
static IBP: LazyLock<Catalog> =
    LazyLock::new(|| serde_json::from_str(include_str!("../web/ibp-evaluation.json")).unwrap());
static DCF: LazyLock<Dcf> =
    LazyLock::new(|| serde_json::from_str(include_str!("../web/dcf77-reference.json")).unwrap());

fn locator(cell: &str) -> Option<(Point, f64)> {
    let b = cell.to_ascii_uppercase().into_bytes();
    if b.len() != 6
        || b[..2].iter().any(|c| !(b'A'..=b'R').contains(c))
        || !b[2..4].iter().all(u8::is_ascii_digit)
        || b[4..].iter().any(|c| !(b'A'..=b'X').contains(c))
    {
        return None;
    }
    let w = 2. / 24.;
    let h = 1. / 24.;
    let lon = -180.
        + 20. * (b[0] - b'A') as f64
        + 2. * (b[2] - b'0') as f64
        + w * ((b[4] - b'A') as f64 + 0.5);
    let lat =
        -90. + 10. * (b[1] - b'A') as f64 + (b[3] - b'0') as f64 + h * ((b[5] - b'A') as f64 + 0.5);
    let point = Point { lat, lon };
    let radius = [-h / 2., h / 2.]
        .iter()
        .map(|d| {
            geo::distance(
                point,
                Point {
                    lat: lat + d,
                    lon: lon + w / 2.,
                },
            )
        })
        .fold(0., f64::max);
    Some((point, radius))
}
struct Reference<'a> {
    point: Point,
    radius: Option<f64>,
    grid: Option<String>,
    source: &'a str,
}
fn reference(e: &Emission) -> Option<Reference<'static>> {
    if e.mode != "live" || e.fault_injected {
        return None;
    }
    match e.reference.as_ref()?.profile.as_str() {
        protocol::PROFILE if e.frequency_hz == 77_500. => Some(Reference {
            point: Point {
                lat: DCF.lat,
                lon: DCF.lon,
            },
            radius: None,
            grid: None,
            source: &DCF.source,
        }),
        meridian_tdoa::ft8_tdoa::PROFILE => {
            // The locator the station itself sent (hold-out; never a solver input).
            let grid = e.reference.as_ref()?.ft8.as_ref()?.grid.clone()?;
            let (point, radius) = meridian_tdoa::calibration::grid4(&grid)?;
            Some(Reference {
                point,
                radius: Some(radius),
                grid: Some(grid),
                source: "Locator sent in the decoded FT8 message",
            })
        }
        ibp::PROFILE if ibp::band(e.frequency_hz).is_some() => {
            let grid = IBP.locators.get(&e.identity.as_ref()?.callsign)?;
            let (point, radius) = locator(grid)?;
            Some(Reference {
                point,
                radius: Some(radius),
                grid: Some(grid.clone()),
                source: &IBP.source,
            })
        }
        _ => None,
    }
}
pub fn report(e: &Emission) -> Option<Value> {
    let r = reference(e)?;
    let solutions: Vec<_> = e
        .estimate
        .iter()
        .chain(&e.hypotheses)
        .map(|s| {
            let error = geo::distance(s.center, r.point);
            json!({"center":s.center,"error_km":error,"model_radius_km":s.radius_km,
            "error_range_km":r.radius.map(|v|[(error-v).max(0.),error+v])})
        })
        .collect();
    Some(
        json!({"id":e.id,"start_utc":e.start_utc,"frequency_hz":e.frequency_hz,"unique":e.estimate.is_some(),
        "reference":r.point,"reference_source":r.source,"grid":r.grid,"grid_radius_km":r.radius,"solutions":solutions}),
    )
}

pub fn bands(
    results: &[Emission],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    jobs: &[Value],
    targets: &[meridian_tdoa::model::Target],
) -> Vec<Value> {
    let mut rows = vec![];
    for (band, hz) in std::iter::once(("LF", 77_500)).chain(ibp::CHANNELS) {
        let mut errors = vec![];
        let mut calls = HashSet::new();
        let (mut sampled, mut heard, mut ambiguous, mut unique) = (0, 0, 0, 0);
        let mut latest = None;
        for e in results {
            if e.frequency_hz != hz as f64 || e.mode != "live" || e.fault_injected {
                continue;
            }
            let reference = reference(e);
            if reference.is_none()
                && (e.status != "no_verified_beacon" || e.provenance.url != ibp::SOURCE)
            {
                continue;
            }
            let (Ok(a), Ok(b)) = (
                DateTime::parse_from_rfc3339(&e.start_utc),
                DateTime::parse_from_rfc3339(&e.end_utc),
            ) else {
                continue;
            };
            if a >= end || b <= start {
                continue;
            }
            sampled += 1;
            latest = latest.max(Some(e.start_utc.as_str()));
            let Some(reference) = reference else { continue };
            heard += 1;
            calls.insert(e.identity.as_ref().map_or("DCF77", |i| i.callsign.as_str()));
            if !e.hypotheses.is_empty() {
                ambiguous += 1;
            }
            if let Some(est) = &e.estimate {
                unique += 1;
                errors.push(geo::distance(est.center, reference.point));
            }
        }
        let ids: HashSet<_> = targets
            .iter()
            .filter(|t| t.frequency_hz == hz as f64)
            .map(|t| t.id.as_str())
            .collect();
        let attempts = jobs
            .iter()
            .filter(|j| {
                j["monitor_target"]
                    .as_str()
                    .is_some_and(|id| ids.contains(id))
                    && j["started_at"]
                        .as_str()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .is_some_and(|t| t >= start && t < end)
            })
            .count();
        errors.sort_by(f64::total_cmp);
        let n = errors.len();
        let median = (n > 0).then(|| (errors[(n - 1) / 2] + errors[n / 2]) / 2.);
        let p90 = (n > 0).then(|| errors[(n * 9).div_ceil(10) - 1]);
        rows.push(json!({"band":band,"frequency_hz":hz,"scheduled_attempts":attempts,"sampled_slots":sampled,
            "verified_slots":heard,"distinct_beacons":calls.len(),"unique_fixes":unique,"ambiguous":ambiguous,
            "evaluated":n,"median_error_km":median,"p90_error_km":p90,"latest_utc":latest}));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use meridian_tdoa::model::{Identity, VerifiedReference};
    #[test]
    fn evaluation_uses_grid_only_for_the_correct_band_and_unique_fixes() {
        assert!(
            ibp::CALLS
                .iter()
                .all(|c| locator(&IBP.locators[*c]).is_some())
        );
        assert!(locator("XX00ab").is_none());
        let (point, radius) = locator("AA00aa").unwrap();
        assert!(
            (point.lon + 179.958333).abs() < 0.00001
                && (point.lat + 89.9791667).abs() < 0.00001
                && radius > 2.
        );
        let mut e: Emission =
            serde_json::from_str(include_str!("../evidence/replay-result.json")).unwrap();
        e.mode = "live".into();
        e.frequency_hz = ibp::CHANNELS[0].1 as f64;
        e.reference = Some(VerifiedReference {
            node_utc: None,
            profile: ibp::PROFILE.into(),
            cw: vec![],
            three_receivers: None,
            ft8: None,
        });
        e.identity = Some(Identity {
            callsign: "OH2B".into(),
            evidence: e.provenance.clone(),
        });
        assert!(!e.hypotheses.is_empty());
        let mut resolved = e.clone();
        resolved.estimate = resolved.hypotheses.pop();
        resolved.hypotheses.clear();
        let report = report(&resolved).unwrap();
        assert_eq!(report["grid"], "KP20eh");
        let row = &report["solutions"][0];
        assert!(row["error_range_km"][0].as_f64() < row["error_km"].as_f64());
        let date = DateTime::parse_from_rfc3339(&e.start_utc)
            .unwrap()
            .with_timezone(&Utc);
        let stats = bands(
            &[e, resolved],
            date,
            date + chrono::Duration::hours(1),
            &[],
            &[],
        );
        assert_eq!(stats[1]["ambiguous"], 1);
        assert_eq!(stats[1]["evaluated"], 1);
        assert_eq!(stats[1]["distinct_beacons"], 1);
        assert_eq!(stats[1]["median_error_km"], row["error_km"]);
        assert_eq!(stats[2]["median_error_km"], Value::Null);
    }
}

/// Worldwide summary for a UTC interval: one row per profile and band (or channel), with the
/// post-hoc accuracy against the hold-out reference where one exists (DCF77, IBP, FT8 grid).
pub fn summary(
    db: &meridian_tdoa::db::Db,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> anyhow::Result<Vec<Value>> {
    use meridian_tdoa::calibration::{contains, mahalanobis2};
    use std::collections::BTreeMap;
    #[derive(Default)]
    struct Totals {
        observations: usize,
        heard: usize,
        timed: usize,
        fixes: usize,
        calls: HashSet<String>,
        errors: Vec<f64>,
        latency: Vec<f64>,
        inside95: usize,
        inside68: usize,
        with_covariance: usize,
        latest: String,
    }
    let mut groups = BTreeMap::<(String, String), Totals>::new();
    db.visit_live(
        start.timestamp_millis(),
        end.timestamp_millis(),
        |e, published| {
            let profile = e.reference.as_ref().map(|r| r.profile.as_str()).unwrap_or(
                if e.status == "no_verified_beacon" {
                    ibp::PROFILE
                } else {
                    "unverified"
                },
            );
            let band = meridian_tdoa::bands::name(e.frequency_hz).unwrap_or(match e.frequency_hz {
                f if f < 300e3 => "LF",
                f if f < 3e6 => "MF",
                f if f < 30e6 => "HF",
                f if f < 300e6 => "VHF",
                f if f < 3e9 => "UHF",
                _ => "SHF",
            });
            let g = groups
                .entry((profile.to_owned(), band.to_owned()))
                .or_default();
            g.observations += 1;
            g.heard += e.detected as usize;
            g.timed += (e.receivers.iter().filter(|r| r.confirmed).count() >= 2) as usize;
            if let Some(id) = &e.identity {
                g.calls.insert(id.callsign.clone());
            }
            if g.latest < e.start_utc {
                g.latest.clone_from(&e.start_utc);
            }
            let Some(est) = &e.estimate else {
                return;
            };
            g.fixes += 1;
            // For legacy node reports the association time is the only UTC time available.
            // A network latency measured from that time would omit transport and processing.
            if e.has_rf_utc() {
                if let (Some(seen), Some(rf)) = (published, meridian_tdoa::db::millis(&e.start_utc))
                {
                    if seen >= rf {
                        g.latency.push((seen - rf) as f64 / 1000.);
                    }
                }
            }
            if let Some(r) = reference(&e) {
                g.errors.push(geo::distance(est.center, r.point));
                g.inside95 += contains(&est.polygon, r.point) as usize;
                if let Some(d2) = mahalanobis2(est, r.point) {
                    g.with_covariance += 1;
                    // Exactly 68% probability mass for a 2-dimensional Gaussian ellipse.
                    g.inside68 += (d2 <= -2. * 0.32_f64.ln()) as usize;
                }
            }
        },
    )?;
    let hours = (end - start).num_milliseconds() as f64 / 3_600_000.;
    let mut rows = Vec::new();
    for ((profile, band), mut g) in groups {
        let quantiles = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            let n = v.len();
            (
                (n > 0).then(|| (v[(n - 1) / 2] + v[n / 2]) / 2.),
                (n > 0).then(|| v[(9 * n).div_ceil(10) - 1]),
            )
        };
        let (median, p90) = quantiles(&mut g.errors);
        let (lat50, lat90) = quantiles(&mut g.latency);
        let coverage = |a: usize, n: usize| (n > 0).then(|| a as f64 / n as f64);
        rows.push(json!({"profile":profile,"band":band,"observations":g.observations,"heard":g.heard,
            "timed_two_plus":g.timed,"located":g.fixes,"fixes_per_hour":g.fixes as f64/hours,
            "distinct_identities":g.calls.len(),"evaluated":g.errors.len(),"median_error_km":median,"p90_error_km":p90,
            "contour_coverage":coverage(g.inside95,g.errors.len()),"ellipse_68_evaluated":g.with_covariance,
            "ellipse_68_coverage":coverage(g.inside68,g.with_covariance),
            "publication_latency_count":g.latency.len(),"median_publication_latency_s":lat50,
            "p90_publication_latency_s":lat90,"latest_utc":g.latest}));
    }
    Ok(rows)
}
