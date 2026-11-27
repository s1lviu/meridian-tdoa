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
    grid: Option<&'a str>,
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
        ibp::PROFILE if ibp::band(e.frequency_hz).is_some() => {
            let grid = IBP.locators.get(&e.identity.as_ref()?.callsign)?;
            let (point, radius) = locator(grid)?;
            Some(Reference {
                point,
                radius: Some(radius),
                grid: Some(grid),
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
