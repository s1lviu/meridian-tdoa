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
        settings.directory_receivers == 0 || (3..=8).contains(&settings.directory_receivers),
        "A directory network needs 3–8 receivers per capture"
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
    let jobs = app.jobs.read().unwrap();
    let batch:Vec<_> = status.last_jobs.iter().filter_map(|id| jobs.get(id)).map(|j|json!({"id":j["id"],"status":j["status"],
        "started_at":j["started_at"],"updated_at":j["updated_at"],"emission_id":j["emission_id"],"target_id":j["monitor_target"],"verified_slots":j["verified_slots"],"sampled_slots":j["sampled_slots"],"captured_receivers":j["captured_receivers"]})).collect();
    json!({"target_id":settings.target_id,"interval_s":settings.interval_s,"next_at":status.next_at,
        "next_target_id":status.next_targets.first(),"next_target_ids":status.next_targets,"additional_target_ids":settings.additional_target_ids,"directory_receivers":settings.directory_receivers,
        "parallel_bands":settings.parallel_bands,"notice":status.notice,"last_job":batch.first(),"last_jobs":batch})
}

fn plan_bands(
    config: &Config,
    directory: Option<&Directory>,
    targets: &[Target],
    bands: &[usize],
    rounds: &mut [usize],
    limit: usize,
) -> Vec<Result<(Config, Target)>> {
    let dynamic = |t: &Target| limit > 0 && t.verification.as_deref() == Some(ibp::PROFILE);
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
    let mut assigned = HashSet::new();
    bands
        .iter()
        .map(|&i| {
            let mut t = targets[i].clone();
            let mut cfg = config.clone();
            if dynamic(&t) {
                let round = rounds[i];
                rounds[i] += 1;
                cfg.sources = directory
                    .ok_or_else(|| anyhow::anyhow!("Receiver directory unavailable"))?
                    .network_excluding(t.frequency_hz - 700., round, limit, &reserved)?;
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

fn next_slot(
    earliest: DateTime<Utc>,
    last_attempt: Option<DateTime<Utc>>,
    interval: TimeDelta,
) -> DateTime<Utc> {
    last_attempt.map_or(earliest, |at| earliest.max(at + interval))
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
    let mut earliest = Utc::now();
    let (mut index, last_id, mut rounds) = {
        let jobs = app.jobs.read().unwrap();
        let latest = jobs
            .values()
            .filter_map(|j| {
                let index = targets
                    .iter()
                    .position(|t| Some(t.id.as_str()) == j["monitor_target"].as_str())?;
                Some((j["started_at"].as_str()?, index, j["id"].as_str()?))
            })
            .max();
        let rounds: Vec<_> = targets
            .iter()
            .map(|t| {
                let count = jobs
                    .values()
                    .filter(|j| j["monitor_target"].as_str() == Some(t.id.as_str()))
                    .count();
                count + ibp::band(t.frequency_hz).unwrap_or(0)
            })
            .collect();
        let (index, id) = latest.map_or((0, None), |(_, i, id)| {
            ((i + 1) % targets.len(), Some(id.to_string()))
        });
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
    let paused = loop {
        if app.stopping.load(Ordering::SeqCst) {
            return;
        }
        let last = match capture::last_attempt(Path::new("data/captures")) {
            Ok(last) => last,
            Err(err) => break format!("Capture history unavailable: {err}"),
        };
        let due = next_slot(earliest, last, interval);
        let bands = band_indices(index);
        {
            let mut status = app.monitor.write().unwrap();
            status.next_at = Some(due);
            status.next_targets = target_names(&bands);
        }
        if let Ok(wait) = (due - Utc::now()).to_std() {
            tokio::time::sleep(wait).await;
            continue;
        }
        earliest = Utc::now() + interval;
        if let Err(err) = capture::validate_clock(
            &app.config,
            bands.iter().map(|&i| targets[i].duration_s).max().unwrap(),
        ) {
            break err.to_string();
        }
        let _guard = match reserve_worker(&app) {
            Ok(guard) => guard,
            Err(err) if err.0 == StatusCode::CONFLICT => {
                app.monitor.write().unwrap().notice =
                    Some("Window skipped while the receiver network was busy".into());
                continue;
            }
            Err(err) => break err.1,
        };
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
            next_at: Some(earliest),
            last_jobs: ids.clone(),
            notice: None,
        };
        let plans = plan_bands(
            &app.config,
            app.directory.read().unwrap().as_ref(),
            &targets,
            &bands,
            &mut rounds,
            settings.directory_receivers,
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
        if !ready.is_empty() {
            match capture::reserve_window(ready, Path::new("data/captures")) {
                Ok(captures) => {
                    let mut tasks = tokio::task::JoinSet::new();
                    for (id, capture) in work_ids.iter().cloned().zip(captures) {
                        let app = app.clone();
                        tasks.spawn(async move {
                            finish(app, id, capture::record(capture).await, None).await;
                        });
                    }
                    while let Some(result) = tasks.join_next().await {
                        if let Err(err) = result {
                            eprintln!("Band worker failed: {err}");
                        }
                    }
                    for id in &work_ids {
                        if app
                            .jobs
                            .read()
                            .unwrap()
                            .get(id)
                            .is_some_and(|j| j["status"] == "running")
                        {
                            failed(&app, id, "Band worker interrupted");
                        }
                    }
                }
                Err(err) => {
                    for id in &work_ids {
                        failed(&app, id, &err.to_string());
                    }
                    if err
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::StorageFull)
                    {
                        break "IQ archive is full. Archive data before restarting the monitor."
                            .into();
                    }
                }
            }
        }
        if ids.iter().any(|id| {
            app.jobs
                .read()
                .unwrap()
                .get(id)
                .is_some_and(|j| j.get("persistence_error").is_some())
        }) {
            break "Could not save the capture job. Check storage before restarting the monitor."
                .into();
        }
        if earliest <= Utc::now() {
            earliest = Utc::now() + interval;
        } // Never replay missed slots.
    };
    let mut status = app.monitor.write().unwrap();
    status.next_at = None;
    status.notice = Some(paused);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_and_manual_capture_observe_the_same_slot() {
        let now = Utc::now();
        let interval = TimeDelta::minutes(5);
        assert_eq!(next_slot(now, None, interval), now);
        assert_eq!(next_slot(now, Some(now - interval * 2), interval), now);
        assert_eq!(
            next_slot(now, Some(now - TimeDelta::seconds(10)), interval),
            now + interval - TimeDelta::seconds(10)
        );
    }

    #[test]
    fn simultaneous_beacon_bands_reserve_disjoint_devices_and_one_archive_window() {
        let mut cfg: Config =
            serde_json::from_str(include_str!("../config/production.json")).unwrap();
        cfg.gps_offset_valid_until_utc = Some((Utc::now() + TimeDelta::days(1)).to_rfc3339());
        let (settings, targets) = configured_targets(&cfg).unwrap().unwrap();
        let directory =
            meridian_tdoa::directory::load(Path::new("evidence/live/directory.json")).unwrap();
        let mut rounds = vec![0; targets.len()];
        let plans = plan_bands(
            &cfg,
            Some(&directory),
            &targets,
            &[1, 2],
            &mut rounds,
            settings.directory_receivers,
        )
        .into_iter()
        .collect::<Result<Vec<_>>>()
        .unwrap();
        let ids: HashSet<_> = plans.iter().flat_map(|(_, t)| &t.source_ids).collect();
        assert_eq!(ids.len(), 16);
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            capture::reserve_window(vec![plans[0].clone(), plans[0].clone()], tmp.path()).is_err()
        );
        assert!(capture::last_attempt(tmp.path()).unwrap().is_none());
        let window = capture::reserve_window(plans.clone(), tmp.path()).unwrap();
        assert_eq!(window.len(), 2);
        assert!(
            capture::reserve_window(plans.clone(), tmp.path())
                .err()
                .unwrap()
                .to_string()
                .contains("reserve the IQ archive")
        );
        drop(window);
        assert!(
            capture::reserve_window(plans, tmp.path())
                .err()
                .unwrap()
                .to_string()
                .contains("60 s")
        );
        let fixed = plan_bands(
            &cfg,
            Some(&directory),
            &targets,
            &[1, 0],
            &mut rounds,
            settings.directory_receivers,
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
