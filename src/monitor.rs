use super::{App, finish, new_job, record_job, reserve_worker};
use anyhow::{Result, ensure};
use axum::http::StatusCode;
use chrono::{DateTime, TimeDelta, Utc};
use meridian_tdoa::{
    capture,
    directory::Directory,
    ibp,
    model::{Config, MonitorConfig, Target},
    protocol,
};
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path, sync::atomic::Ordering};

#[derive(Clone, Default)]
pub(super) struct Status {
    next_at: Option<DateTime<Utc>>,
    last_jobs: Vec<String>,
    next_targets: Vec<String>,
    notice: Option<String>,
}

pub(super) fn configured_targets(config: &Config) -> Result<Option<(MonitorConfig, Vec<Target>)>> {
    let Some(settings) = &config.monitor else {
        return Ok(None);
    };
    ensure!(
        config.live_enabled && (60..=86400).contains(&settings.interval_s),
        "A monitor needs live capture and an interval of 60–86400 seconds"
    );
    ensure!(
        settings.directory_receivers == 0
            || (3..=meridian_tdoa::pipeline::MAX_RECEIVERS).contains(&settings.directory_receivers),
        "A directory network needs 3–12 receivers per capture"
    );
    let mut targets = vec![];
    for id in std::iter::once(&settings.target_id).chain(&settings.additional_target_ids) {
        ensure!(
            !targets.iter().any(|t: &Target| t.id == *id),
            "Duplicate monitor target"
        );
        let t = config
            .targets
            .iter()
            .find(|t| &t.id == id)
            .ok_or_else(|| anyhow::anyhow!("Monitor target {id} is missing"))?;
        let valid = match t.verification.as_deref() {
            Some(protocol::PROFILE) => {
                t.kind == "public_time_reference" && t.frequency_hz == 77500.
            }
            Some(ibp::PROFILE) => t.kind == "public_beacon" && ibp::band(t.frequency_hz).is_some(),
            Some(meridian_tdoa::ft8_tdoa::PROFILE) => {
                t.kind == "amateur_digital" && meridian_tdoa::bands::name(t.frequency_hz).is_some()
            }
            Some(meridian_tdoa::channel::PROFILE) => t.kind == "monitored_channel",
            _ => false,
        };
        ensure!(valid, "No automatic RF verifier for monitor target {id}");
        targets.push(t.clone());
    }
    ensure!(
        (1..=targets.len()).contains(&settings.parallel_bands),
        "Parallel bands must fit the configured target count"
    );
    Ok(Some((settings.clone(), targets)))
}

pub(super) fn snapshot(app: &App) -> Value {
    let Some(settings) = &app.config.monitor else {
        return Value::Null;
    };
    let status = app.monitor.read().unwrap().clone();
    let jobs: Vec<Value> = status
        .last_jobs
        .iter()
        .filter_map(|id| app.db.job(id).ok().flatten())
        .collect();
    let batch:Vec<_> = jobs.iter().map(|j|json!({"id":j["id"],"status":j["status"],
        "started_at":j["started_at"],"updated_at":j["updated_at"],"emission_id":j["emission_id"],"target_id":j["monitor_target"],"verified_slots":j["verified_slots"],"sampled_slots":j["sampled_slots"],"captured_receivers":j["captured_receivers"]})).collect();
    json!({"target_id":settings.target_id,"interval_s":settings.interval_s,"next_at":status.next_at,
        "next_target_id":status.next_targets.first(),"next_target_ids":status.next_targets,"additional_target_ids":settings.additional_target_ids,"directory_receivers":settings.directory_receivers,
        "parallel_bands":settings.parallel_bands,"notice":status.notice,"last_job":batch.first(),"last_jobs":batch})
}

/// Where reconnaissance starts for a target, if not a worldwide rotation. The DCF77 profile is
/// commissioned on receivers known to hear the time signal: the network grows from the mean
/// position of its configured receivers. Receiver geography only; no transmitter position is
/// used, and the solver never sees this point.
fn seed_point(config: &Config, t: &Target) -> Option<meridian_tdoa::model::Point> {
    if t.verification.as_deref() != Some(protocol::PROFILE) {
        return None;
    }
    let (mut x, mut y, mut z, mut n) = (0., 0., 0., 0);
    for s in config
        .sources
        .iter()
        .filter(|s| t.source_ids.contains(&s.id))
    {
        let (lat, lon) = (s.position.lat.to_radians(), s.position.lon.to_radians());
        x += lat.cos() * lon.cos();
        y += lat.cos() * lon.sin();
        z += lat.sin();
        n += 1;
    }
    (n > 0).then(|| meridian_tdoa::model::Point {
        lat: z.atan2(f64::hypot(x, y)).to_degrees(),
        lon: y.atan2(x).to_degrees(),
    })
}

fn plan_bands(
    config: &Config,
    directory: Option<&Directory>,
    targets: &[Target],
    bands: &[usize],
    rounds: &mut [usize],
    limit: usize,
    excluded: &HashSet<String>,
    per_target: &std::collections::HashMap<String, meridian_tdoa::db::Reception>,
) -> Vec<Result<(Config, Target)>> {
    // With a directory network every profile picks its receivers there. A fixed list
    // stopped DCF77 entirely once a few of its receivers were blocked or behind a relay.
    let dynamic = |t: &Target| {
        limit > 0
            && [
                Some(ibp::PROFILE),
                Some(meridian_tdoa::ft8_tdoa::PROFILE),
                Some(meridian_tdoa::channel::PROFILE),
                Some(protocol::PROFILE),
            ]
            .contains(&t.verification.as_deref())
    };
    // Fixed receivers (e.g. LF) get their frequency before the worldwide pool is allocated.
    let mut reserved: HashSet<_> = bands
        .iter()
        .map(|i| &targets[*i])
        .filter(|t| !dynamic(t))
        .flat_map(|t| {
            config
                .sources
                .iter()
                .filter(|s| t.source_ids.contains(&s.id))
                .map(|s| s.independent_device.clone())
        })
        .collect();
    reserved.extend(excluded.iter().cloned());
    let mut assigned = HashSet::new();
    bands
        .iter()
        .map(|&i| {
            let mut t = targets[i].clone();
            let mut cfg = config.clone();
            // Fixed receivers that are blocked, behind a blocked relay or out of budget are
            // skipped for this window.
            let configured = t.source_ids.len();
            t.source_ids.retain(|id| {
                !excluded.contains(id)
                    && !config.sources.iter().any(|s| {
                        &s.id == id
                            && meridian_tdoa::directory::behind_blocked_relay(excluded, &s.base_url)
                    })
            });
            if !dynamic(&t) {
                ensure!(
                    t.source_ids.len() >= 3,
                    "Only {} of {} configured receivers are usable now; the others are blocked, \
                     behind a blocked relay, cooling down or out of their daily budget",
                    t.source_ids.len(),
                    configured
                );
            }
            if dynamic(&t) {
                let round = rounds[i];
                rounds[i] += 1;
                let mut avoid = reserved.clone();
                let d =
                    directory.ok_or_else(|| anyhow::anyhow!("Receiver directory unavailable"))?;
                let center = capture::iq_center(&t);
                let spacing = t.min_spacing_km.unwrap_or(150.);
                let reception = per_target.get(&t.id);
                if let Some(r) = reception {
                    avoid.extend(r.quiet.iter().cloned());
                }
                // Alternate geographic discovery with measurement. Use existing scheduled
                // windows as probes, so selecting receivers costs no extra connections.
                cfg.sources =
                    match reception.filter(|r| round % 2 == 1 && !r.strength_db.is_empty()) {
                        Some(r) => {
                            d.network_heard(center, round, limit, &avoid, spacing, &r.strength_db)?
                        }
                        None => {
                            if t.verification.as_deref() == Some(meridian_tdoa::channel::PROFILE) {
                                t.duration_s = t.duration_s.min(15);
                            }
                            let anchor = match seed_point(config, &targets[i]) {
                                Some(p) => d.anchor_near(center, p, &avoid)?,
                                None => round / 2,
                            };
                            d.network_spaced(center, anchor, limit, &avoid, spacing)?
                        }
                    };
                t.source_ids = cfg.sources.iter().map(|s| s.id.clone()).collect();
            }
            let sources = capture::authorize(&cfg, &t)?;
            ensure!(
                sources
                    .iter()
                    .all(|s| !assigned.contains(&s.independent_device)),
                "Receiver already assigned to a simultaneous band"
            );
            for s in sources {
                reserved.insert(s.independent_device.clone());
                assigned.insert(s.independent_device);
            }
            Ok((cfg, t))
        })
        .collect()
}

/// FT8 transmissions start 0.5 s after each 15 s UTC boundary. Start the connections
/// `LEAD` seconds before a boundary so connection set-up and the 1 s settling end before it.
const FT8_LEAD_MS: i64 = 4000;
fn ft8_aligned(t: DateTime<Utc>) -> DateTime<Utc> {
    let s = t.timestamp_millis() + FT8_LEAD_MS;
    let boundary = (s + 14_999).div_euclid(15_000) * 15_000;
    DateTime::from_timestamp_millis(boundary - FT8_LEAD_MS).unwrap()
}

fn failed(app: &App, id: &str, reason: &str) {
    record_job(
        app,
        id.to_owned(),
        json!({"id":id,"status":"failed","error":reason,"monitoring":"unmonitored"}),
    );
}

pub(super) async fn run(app: App, settings: MonitorConfig, targets: Vec<Target>) {
    let interval = TimeDelta::seconds(settings.interval_s as i64);
    let (mut index, last_id, mut rounds) = {
        let (latest, counts) = app.db.monitor_history().unwrap_or_default();
        let rounds: Vec<_> = targets
            .iter()
            .map(|t| {
                let count = counts
                    .iter()
                    .find(|(id, _)| id == &t.id)
                    .map_or(0, |(_, n)| *n);
                count + ibp::band(t.frequency_hz).unwrap_or(0)
            })
            .collect();
        let resume = latest.and_then(|(target, id)| {
            let i = targets.iter().position(|t| t.id == target)?;
            Some(((i + 1) % targets.len(), id))
        });
        let (index, id) = resume.map_or((0, None), |(i, id)| (i, Some(id)));
        (index, id, rounds)
    };
    app.monitor.write().unwrap().last_jobs = last_id.into_iter().collect();
    let band_indices = |start: usize| {
        (0..settings.parallel_bands)
            .map(|n| (start + n) % targets.len())
            .collect::<Vec<_>>()
    };
    let target_names = |bands: &[usize]| {
        bands
            .iter()
            .map(|&i| targets[i].id.clone())
            .collect::<Vec<String>>()
    };
    let mut due = Utc::now();
    let paused = loop {
        if app.stopping.load(Ordering::SeqCst) {
            return;
        }
        let bands = band_indices(index);
        if bands
            .iter()
            .any(|&i| targets[i].verification.as_deref() == Some(meridian_tdoa::ft8_tdoa::PROFILE))
        {
            due = ft8_aligned(due);
        }
        {
            let mut status = app.monitor.write().unwrap();
            status.next_at = Some(due);
            status.next_targets = target_names(&bands);
        }
        if let Ok(wait) = (due - Utc::now()).to_std() {
            tokio::time::sleep(wait).await;
        }
        due = Utc::now() + interval; // the next window never replays missed slots
        if let Err(err) = capture::validate_clock(
            &app.config,
            bands.iter().map(|&i| targets[i].duration_s).max().unwrap(),
        ) {
            break err.to_string();
        }
        let guard = match reserve_worker(&app) {
            Ok(guard) => guard,
            Err(err) if err.0 == StatusCode::CONFLICT => {
                app.monitor.write().unwrap().notice =
                    Some("Window skipped: every worker is busy with earlier windows".into());
                continue;
            }
            Err(err) => break err.1,
        };
        // Receivers already in flight, blocked, inside their gap or out of daily budget.
        let planned = bands.iter().map(|&i| targets[i].duration_s).max().unwrap() as f64;
        let mut excluded = app
            .db
            .unusable_receivers(
                Utc::now().timestamp_millis(),
                &app.config.receivers,
                planned,
            )
            .unwrap_or_default();
        excluded.extend(app.leases.lock().unwrap().iter().cloned());
        let mut ids = Vec::new();
        let mut problem = None;
        for &i in &bands {
            match new_job(&app, Some(&targets[i].id)) {
                Ok(id) => ids.push(id),
                Err(err) => {
                    problem = Some(err.1);
                    break;
                }
            }
        }
        if let Some(error) = problem {
            for id in ids {
                failed(&app, &id, &error);
            }
            break error;
        }
        index = (index + bands.len()) % targets.len();
        *app.monitor.write().unwrap() = Status {
            next_targets: target_names(&band_indices(index)),
            next_at: Some(due),
            last_jobs: ids.clone(),
            notice: None,
        };
        // Listen-then-measure: receivers that heard nothing on a band recently are skipped there.
        let per_target: std::collections::HashMap<_, _> = bands
            .iter()
            .filter_map(|&i| {
                let key = super::channel_key(targets[i].frequency_hz);
                Some((
                    targets[i].id.clone(),
                    app.db.reception(&key, Utc::now().timestamp_millis()).ok()?,
                ))
            })
            .collect();
        let plans = plan_bands(
            &app.config,
            app.directory.read().unwrap().as_ref(),
            &targets,
            &bands,
            &mut rounds,
            settings.directory_receivers,
            &excluded,
            &per_target,
        );
        let mut ready = Vec::new();
        let mut work_ids = Vec::new();
        for (id, plan) in ids.iter().zip(plans) {
            match plan {
                Ok(plan) => {
                    ready.push(plan);
                    work_ids.push(id.clone());
                }
                Err(err) => failed(&app, id, &err.to_string()),
            }
        }
        if ready.is_empty() {
            continue;
        }
        let captures = match capture::reserve_window(ready, Path::new("data/captures")) {
            Ok(c) => c,
            Err(err) => {
                for id in &work_ids {
                    failed(&app, id, &err.to_string());
                }
                if err
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::StorageFull)
                {
                    app.monitor.write().unwrap().notice =
                        Some("Window skipped: insufficient space under the IQ quota".into());
                }
                continue;
            }
        };
        let leased: Vec<String> = captures.iter().flat_map(|c| c.receiver_ids()).collect();
        app.leases.lock().unwrap().extend(leased.iter().cloned());
        // The window runs in the background; the loop schedules the next one on time.
        let window_app = app.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let mut tasks = tokio::task::JoinSet::new();
            for (id, capture) in work_ids.iter().cloned().zip(captures) {
                let app = window_app.clone();
                tasks.spawn(async move {
                    let result = capture::record(capture, &app.db).await;
                    finish(app, id, result, None).await;
                });
            }
            while let Some(result) = tasks.join_next().await {
                if let Err(err) = result {
                    eprintln!("Band worker failed: {err}");
                }
            }
            for id in &work_ids {
                if window_app
                    .db
                    .job(id)
                    .ok()
                    .flatten()
                    .is_some_and(|j| j["status"] == "running")
                {
                    failed(&window_app, id, "Band worker interrupted");
                }
            }
            let mut leases = window_app.leases.lock().unwrap();
            for id in &leased {
                leases.remove(id);
            }
        });
        if ids
            .iter()
            .any(|id| app.persistence_failures.lock().unwrap().contains(id))
        {
            break "Could not save the capture job. Check storage before restarting the monitor."
                .into();
        }
    };
    let mut status = app.monitor.write().unwrap();
    status.next_at = None;
    status.notice = Some(paused);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ft8_windows_start_before_a_slot_boundary_and_never_in_the_past() {
        for ms in [0i64, 1, 10_999, 11_000, 11_001, 14_999, 29_000] {
            let t = DateTime::from_timestamp_millis(1_800_000_000_000 + ms).unwrap();
            let a = ft8_aligned(t);
            assert!(a >= t);
            assert_eq!((a.timestamp_millis() + FT8_LEAD_MS) % 15_000, 0);
            assert!(a - t < TimeDelta::seconds(15));
        }
    }

    #[test]
    fn simultaneous_bands_use_disjoint_receivers_and_skip_excluded_ones() {
        let mut cfg: Config =
            serde_json::from_str(include_str!("../config/production.json")).unwrap();
        cfg.gps_offset_valid_until_utc = Some((Utc::now() + TimeDelta::days(1)).to_rfc3339());
        let (settings, targets) = configured_targets(&cfg).unwrap().unwrap();
        let directory =
            meridian_tdoa::directory::load(Path::new("evidence/live/directory.json")).unwrap();
        let mut rounds = vec![0; targets.len()];
        let none = HashSet::new();
        let plans = plan_bands(
            &cfg,
            Some(&directory),
            &targets,
            &[1, 2],
            &mut rounds,
            settings.directory_receivers,
            &none,
            &Default::default(),
        )
        .into_iter()
        .collect::<Result<Vec<_>>>()
        .unwrap();
        let ids: HashSet<_> = plans
            .iter()
            .flat_map(|(_, t)| t.source_ids.clone())
            .collect();
        assert_eq!(
            ids.len(),
            2 * settings.directory_receivers,
            "two bands, eight receivers each, no receiver twice"
        );
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            capture::reserve_window(vec![plans[0].clone(), plans[0].clone()], tmp.path()).is_err()
        );
        // Windows on disjoint receivers may overlap in time: no global cooldown or archive lock.
        let first = capture::reserve_window(vec![plans[0].clone()], tmp.path()).unwrap();
        assert!(capture::reserve_window(vec![plans[0].clone()], tmp.path()).is_err());
        let second = capture::reserve_window(vec![plans[1].clone()], tmp.path()).unwrap();
        assert_eq!(first.len() + second.len(), 2);
        // Excluded receivers (blocked, out of budget or leased) are never selected.
        let mut rounds = vec![0; targets.len()];
        let excluded: HashSet<String> = plans[0].1.source_ids.iter().cloned().collect();
        let again = plan_bands(
            &cfg,
            Some(&directory),
            &targets,
            &[1],
            &mut rounds,
            settings.directory_receivers,
            &excluded,
            &Default::default(),
        )
        .into_iter()
        .collect::<Result<Vec<_>>>()
        .unwrap();
        assert!(
            again[0]
                .1
                .source_ids
                .iter()
                .all(|id| !excluded.contains(id))
        );
        let fixed = plan_bands(
            &cfg,
            Some(&directory),
            &targets,
            &[1, 0],
            &mut rounds,
            settings.directory_receivers,
            &none,
            &Default::default(),
        )
        .into_iter()
        .collect::<Result<Vec<_>>>()
        .unwrap();
        assert!(
            fixed[0]
                .1
                .source_ids
                .iter()
                .all(|id| !fixed[1].1.source_ids.contains(id))
        );
    }

    #[test]
    fn dcf77_is_planned_near_its_seed_receivers_when_they_are_blocked_or_behind_a_relay() {
        use meridian_tdoa::directory::{KIWI_RELAYS, relay_key, relay_of};
        let mut cfg: Config =
            serde_json::from_str(include_str!("../config/production.json")).unwrap();
        cfg.gps_offset_valid_until_utc = Some((Utc::now() + TimeDelta::days(1)).to_rfc3339());
        let (settings, targets) = configured_targets(&cfg).unwrap().unwrap();
        let directory =
            meridian_tdoa::directory::load(Path::new("evidence/live/directory.json")).unwrap();
        let dcf = targets
            .iter()
            .position(|t| t.id == "dcf77-validation")
            .unwrap();
        // Production on 28 November 2026: every configured receiver blocked, and the KiwiSDR
        // relays refusing the server's address. The fixed list then had no usable receiver.
        let mut excluded: HashSet<String> = targets[dcf].source_ids.iter().cloned().collect();
        excluded.extend(KIWI_RELAYS.iter().map(|r| relay_key(r)));
        let seed = seed_point(&cfg, &targets[dcf]).unwrap();
        assert!((45.0..56.0).contains(&seed.lat) && (0.0..12.0).contains(&seed.lon));
        let mut rounds = vec![0; targets.len()];
        for _ in 0..4 {
            let (plan_cfg, plan) = plan_bands(
                &cfg,
                Some(&directory),
                &targets,
                &[dcf],
                &mut rounds,
                settings.directory_receivers,
                &excluded,
                &Default::default(),
            )
            .pop()
            .unwrap()
            .unwrap();
            assert_eq!(plan.source_ids.len(), settings.directory_receivers);
            for s in &plan_cfg.sources {
                assert!(!excluded.contains(&s.id), "{}", s.id);
                assert!(relay_of(&s.base_url).is_none(), "{}", s.base_url);
                let km = meridian_tdoa::geo::distance(seed, s.position);
                assert!(km < 1500., "{} is {km:.0} km from the seed receivers", s.id);
            }
        }
        // Without a directory network the configured receivers are used, and the error says why
        // a window cannot start instead of calling the configuration invalid.
        let err = plan_bands(
            &cfg,
            Some(&directory),
            &targets,
            &[dcf],
            &mut rounds,
            0,
            &excluded,
            &Default::default(),
        )
        .pop()
        .unwrap()
        .unwrap_err()
        .to_string();
        assert!(
            err.starts_with("Only 0 of 5 configured receivers are usable now"),
            "{err}"
        );
    }

    #[test]
    fn scheduling_is_opt_in_and_requires_an_eligible_profile_and_interval() {
        let mut cfg: Config =
            serde_json::from_str(include_str!("../config/live-verified.json")).unwrap();
        assert!(configured_targets(&cfg).unwrap().is_none());
        cfg.monitor = Some(MonitorConfig {
            target_id: cfg.targets[0].id.clone(),
            interval_s: 60,
            additional_target_ids: vec![],
            directory_receivers: 0,
            parallel_bands: 1,
        });
        assert!(configured_targets(&cfg).unwrap().is_some());
        cfg.monitor.as_mut().unwrap().interval_s = 59;
        assert!(configured_targets(&cfg).is_err());
        cfg.monitor.as_mut().unwrap().interval_s = 300;
        cfg.targets[0].verification = None;
        assert!(configured_targets(&cfg).is_err());
    }
}
