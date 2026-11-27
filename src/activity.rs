//! Live FT8/WSPR report counts. Only two-letter Maidenhead fields leave the collector.
use chrono::{DateTime, Utc};
use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS, Transport};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet, hash_map::RandomState},
    hash::BuildHasher,
    sync::{Arc, Mutex},
    time::Duration,
};

const MINUTES: i64 = 15;
const BROKER: &str = "mqtt.pskreporter.info";
const FT8_TOPIC: &str = "pskr/filter/v2raw/+/FT8/#";
const WSPR_TOPIC: &str = "wspr/filter/#";
// ADIF band enumeration, in MHz. These are display filters.
pub(super) const BANDS: &[(&str, f64, f64)] = &[
    ("2190m", 0.1357, 0.1378),
    ("630m", 0.472, 0.479),
    ("560m", 0.501, 0.504),
    ("160m", 1.8, 2.),
    ("80m", 3.5, 4.),
    ("60m", 5.06, 5.45),
    ("40m", 7., 7.3),
    ("30m", 10.1, 10.15),
    ("20m", 14., 14.35),
    ("17m", 18.068, 18.168),
    ("15m", 21., 21.45),
    ("12m", 24.89, 24.99),
    ("10m", 28., 29.7),
    ("8m", 40., 45.),
    ("6m", 50., 54.),
    ("5m", 54.000001, 69.9),
    ("4m", 70., 71.),
    ("2m", 144., 148.),
    ("1.25m", 222., 225.),
    ("70cm", 420., 450.),
    ("33cm", 902., 928.),
    ("23cm", 1240., 1300.),
    ("13cm", 2300., 2450.),
    ("9cm", 3300., 3500.),
    ("6cm", 5650., 5925.),
    ("3cm", 10000., 10500.),
    ("1.25cm", 24000., 24250.),
    ("6mm", 47000., 47200.),
    ("4mm", 75500., 81000.),
    ("2.5mm", 119980., 123000.),
    ("2mm", 134000., 149000.),
    ("1mm", 241000., 250000.),
    ("submm", 300000., 7500000.),
];

#[derive(Deserialize)]
struct Spot<'a> {
    md: &'a str,
    #[serde(default)]
    b: &'a str,
    t: i64,
    t_tx: Option<i64>,
    rc: Option<&'a str>,
    sl: Option<&'a str>,
    #[serde(default)]
    f: u64,
}

fn field(locator: &str) -> Option<String> {
    let b = locator.as_bytes();
    if !matches!(b.len(), 2 | 4 | 6 | 8 | 10)
        || !b[..2]
            .iter()
            .all(|c| (b'A'..=b'R').contains(&c.to_ascii_uppercase()))
        || (b.len() >= 4 && !b[2..4].iter().all(u8::is_ascii_digit))
        || (b.len() >= 6
            && !b[4..6]
                .iter()
                .all(|c| (b'A'..=b'X').contains(&c.to_ascii_uppercase())))
        || (b.len() >= 8 && !b[6..8].iter().all(u8::is_ascii_digit))
        || (b.len() == 10
            && !b[8..10]
                .iter()
                .all(|c| (b'A'..=b'X').contains(&c.to_ascii_uppercase())))
    {
        return None;
    }
    Some(locator[..2].to_ascii_uppercase())
}

#[derive(Default)]
struct Counts {
    reports: u64,
    fields: BTreeMap<String, u64>,
    reporters: HashSet<u64>,
    latest: i64,
}

type Band = (String, String);

#[derive(Default)]
pub(super) struct Activity {
    minutes: BTreeMap<i64, BTreeMap<Band, Counts>>,
    hash: RandomState,
    connected: bool,
    since: Option<i64>,
}

fn window_start(now: i64) -> i64 {
    (now.div_euclid(60) + 1 - MINUTES) * 60
}

impl Activity {
    fn add(&mut self, spot: Spot<'_>, now: i64) {
        let start = window_start(now);
        self.minutes.retain(|minute, _| *minute >= start);
        let timestamp = spot.t_tx.unwrap_or(spot.t);
        if !(start..=now).contains(&timestamp) || !matches!(spot.md, "FT8" | "WSPR") {
            return;
        }
        let band = BANDS
            .iter()
            .find(|(b, _, _)| b.eq_ignore_ascii_case(spot.b))
            .or_else(|| {
                BANDS
                    .iter()
                    .find(|(_, low, high)| (*low * 1e6..=*high * 1e6).contains(&(spot.f as f64)))
            })
            .map_or("other", |(name, _, _)| name);
        let counts = self
            .minutes
            .entry(timestamp.div_euclid(60) * 60)
            .or_default()
            .entry((spot.md.into(), band.into()))
            .or_default();
        counts.reports += 1;
        counts.latest = counts.latest.max(timestamp);
        if let Some(cell) = spot.sl.and_then(field) {
            *counts.fields.entry(cell).or_default() += 1;
        }
        if let Some(rx) = spot.rc.filter(|s| !s.is_empty() && s.len() <= 30) {
            counts
                .reporters
                .insert(self.hash.hash_one(rx.to_ascii_uppercase()));
        }
    }

    pub(super) fn snapshot(&self, enabled: bool, now: i64) -> Value {
        let mut bands = BTreeMap::<&Band, Counts>::new();
        let mut reporters = HashSet::new();
        for minute in self.minutes.range(window_start(now)..=now).map(|(_, m)| m) {
            for (band, bucket) in minute {
                let totals = bands.entry(band).or_default();
                totals.reports += bucket.reports;
                totals.latest = totals.latest.max(bucket.latest);
                totals.reporters.extend(&bucket.reporters);
                reporters.extend(bucket.reporters.iter().copied());
                for (cell, n) in &bucket.fields {
                    *totals.fields.entry(cell.clone()).or_default() += n;
                }
            }
        }
        let rows: Vec<_> = bands.into_iter().map(|((mode, band), counts)| json!({
            "mode":mode, "band":band, "reports":counts.reports, "reporters":counts.reporters.len(),
            "latest_utc":DateTime::from_timestamp(counts.latest, 0), "fields":counts.fields
        })).collect();
        json!({"enabled":enabled, "connected":self.connected, "window_minutes":MINUTES,
            "start_utc":DateTime::from_timestamp(window_start(now),0), "end_utc":DateTime::from_timestamp(now,0),
            "connected_since_utc":self.since.and_then(|s|DateTime::from_timestamp(s,0)),
            "reporters":reporters.len(), "bands":rows})
    }
}

pub(super) async fn run(activity: Arc<Mutex<Activity>>) {
    let mut options = MqttOptions::new(
        format!("meridian-tdoa-{}", uuid::Uuid::new_v4().simple()),
        BROKER,
        1884,
    );
    options.set_keep_alive(Duration::from_secs(60));
    options.set_transport(Transport::tls_with_default_config());
    let mut retry = 5;
    loop {
        let (client, mut events) = AsyncClient::new(options.clone(), 8);
        loop {
            match events.poll().await {
                Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                    let subscriptions = [FT8_TOPIC, WSPR_TOPIC]
                        .map(|topic| rumqttc::SubscribeFilter::new(topic.into(), QoS::AtMostOnce));
                    if client.subscribe_many(subscriptions).await.is_err() {
                        break;
                    }
                }
                Ok(Event::Incoming(Incoming::SubAck(ack))) => {
                    if ack.return_codes.len() != 2
                        || ack
                            .return_codes
                            .iter()
                            .any(|q| matches!(q, rumqttc::SubscribeReasonCode::Failure))
                    {
                        break;
                    }
                    let mut state = activity.lock().unwrap();
                    state.connected = true;
                    state.since = Some(Utc::now().timestamp());
                }
                Ok(Event::Incoming(Incoming::Publish(message))) => {
                    if message.retain {
                        continue;
                    }
                    if let Ok(spot) = serde_json::from_slice::<Spot>(&message.payload) {
                        if (spot.md == "FT8" && message.topic.starts_with("pskr/filter/v2raw/"))
                            || (spot.md == "WSPR" && message.topic.starts_with("wspr/filter/"))
                        {
                            activity.lock().unwrap().add(spot, Utc::now().timestamp());
                            retry = 5;
                        }
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    eprintln!("Digital activity feed disconnected: {err}");
                    break;
                }
            }
        }
        {
            let mut state = activity.lock().unwrap();
            state.connected = false;
            state.since = None;
        }
        tokio::time::sleep(Duration::from_secs(retry)).await;
        retry = (retry * 2).min(120);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn groups_modes_and_bands_expires_old_reports_and_keeps_only_fields() {
        let mut state = Activity::default();
        let spot = |md, b, sl, t| Spot {
            md,
            b,
            sl: Some(sl),
            rc: Some("TEST-RX"),
            t,
            t_tx: None,
            f: 0,
        };
        state.add(spot("FT8", "20m", "KN35ab", 1200), 1201);
        state.add(spot("WSPR", "40m", "kn35CD", 1200), 1201);
        state.add(spot("FT8", "20m", "UNKNOWN", 1199), 1201);
        state.add(spot("FT8", "20m", "KN35ab", 100), 1201);
        state.add(spot("FT8", "20m", "KN35ab", 1299), 1201);
        let s = state.snapshot(true, 1201);
        assert_eq!(s["reporters"], 1);
        assert_eq!(s["bands"][0]["reports"], 2);
        assert_eq!(s["bands"][0]["fields"], json!({"KN":1}));
        assert_eq!(s["bands"][1]["fields"], json!({"KN":1}));
        assert!(!s.to_string().contains("TEST-RX") && !s.to_string().contains("KN35"));
        assert!(
            state.snapshot(true, 2100)["bands"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
