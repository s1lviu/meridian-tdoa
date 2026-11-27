use axum::{
    Json, Router,
    extract::{Path as ApiPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use meridian_tdoa::{
    bands, capture,
    db::{self, Db},
    directory, ft8_tdoa, geo, ibp,
    model::*,
    pipeline, protocol, retention, store,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tower_http::services::ServeDir;
mod evaluation;
mod monitor;

#[derive(Clone)]
struct App {
    config: Arc<Config>,
    db: Arc<Db>,
    persistence_failures: Arc<Mutex<HashSet<String>>>,
    /// Concurrent jobs (capture windows, reprocessing); each holds one permit.
    workers: Arc<tokio::sync::Semaphore>,
    /// Receivers currently connected by an in-flight capture window.
    leases: Arc<Mutex<HashSet<String>>>,
    datasets: Arc<Vec<(Manifest, PathBuf)>>,
    token: Arc<Option<String>>,
    directory: Arc<RwLock<Option<directory::Directory>>>,
    directory_refreshing: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    monitor: Arc<RwLock<monitor::Status>>,
}
struct Error(StatusCode, String);
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
type Api<T> = Result<Json<T>, Error>;
async fn headers(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let is_api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let h = response.headers_mut();
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    h.insert("referrer-policy", "no-referrer".parse().unwrap());
    h.insert("x-frame-options", "DENY".parse().unwrap());
    h.insert("content-security-policy","default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'".parse().unwrap());
    if is_api {
        h.insert("cache-control", "no-store".parse().unwrap());
    }
    response
}
async fn ready(State(app): State<App>) -> impl IntoResponse {
    if app.stopping.load(Ordering::SeqCst) {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status":"stopping"})),
        )
    } else {
        (
            StatusCode::OK,
            Json(
                json!({"status":"ready","version":env!("CARGO_PKG_VERSION"),"busy":busy(&app),"free_workers":app.workers.available_permits()}),
            ),
        )
    }
}
fn bad(s: impl ToString) -> Error {
    Error(StatusCode::BAD_REQUEST, s.to_string())
}

#[derive(Default, Deserialize)]
struct Filters {
    bbox: Option<String>,
    band: Option<String>,
    start: Option<String>,
    end: Option<String>,
    mode: Option<String>,
    scenario: Option<String>,
    #[serde(default)]
    compact: bool,
}
struct Selection {
    bbox: Option<[f64; 4]>,
    low: f64,
    high: f64,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    mode: String,
    fault: bool,
}
fn selection(q: Filters) -> Result<Selection, Error> {
    let bbox = if let Some(raw) = q.bbox {
        let a = raw
            .split(',')
            .map(|s| s.parse::<f64>().map_err(bad))
            .collect::<Result<Vec<_>, _>>()?;
        if a.len() != 4
            || a.iter().any(|v| !v.is_finite())
            || a[0].abs() > 180.
            || a[2].abs() > 180.
            || a[1] < -90.
            || a[3] > 90.
            || a[1] >= a[3]
        {
            return Err(bad("bbox must be west,south,east,north"));
        }
        Some([a[0], a[1], a[2], a[3]])
    } else {
        None
    };
    let (low, high) = match q.band.as_deref().unwrap_or("all") {
        "all" => (0., 30e6),
        "lf" => (0., 300e3),
        "mf" => (300e3, 3e6),
        "hf" => (3e6, 30e6),
        "other" => (f64::INFINITY, f64::INFINITY),
        name => bands::BANDS
            .iter()
            .find(|(band, _, _)| *band == name)
            .map(|(_, lo, hi)| (lo * 1e6, hi * 1e6))
            .ok_or(bad("Unknown band"))?,
    };
    let parse = |s: &str| {
        DateTime::parse_from_rfc3339(s)
            .map(|t| t.with_timezone(&Utc))
            .map_err(bad)
    };
    let start = parse(q.start.as_deref().unwrap_or("2000-01-01T00:00:00Z"))?;
    let end = parse(q.end.as_deref().unwrap_or("2100-01-01T00:00:00Z"))?;
    if end <= start {
        return Err(bad("UTC interval must be increasing"));
    }
    let mode = q.mode.unwrap_or("replay".into());
    if !["live", "replay"].contains(&mode.as_str()) {
        return Err(bad("Unknown mode"));
    }
    let scenario = q.scenario.unwrap_or("normal".into());
    if !["normal", "sync-loss"].contains(&scenario.as_str()) {
        return Err(bad("Unknown scenario"));
    }
    Ok(Selection {
        bbox,
        low,
        high,
        start,
        end,
        mode,
        fault: scenario == "sync-loss",
    })
}
fn internal(e: impl std::fmt::Display) -> Error {
    Error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
/// Every observation matching the non-geographic filters, newest first.
fn select(app: &App, s: &Selection) -> Result<Vec<Emission>, Error> {
    app.db
        .emissions(&db::Query {
            mode: s.mode.clone(),
            fault: s.fault,
            low_hz: s.low,
            high_hz: s.high,
            start_ms: s.start.timestamp_millis(),
            end_ms: s.end.timestamp_millis(),
            limit: 50_000,
        })
        .map_err(internal)
}
// Observations arrive newest first. Earlier contours are reusable only for fixed reference beacons.
fn latest_signals<'a>(selected: &[&'a Emission]) -> Vec<(&'a Emission, Option<&'a Emission>)> {
    let mut groups = HashMap::new();
    let mut latest = Vec::new();
    for &e in selected {
        if !e.detected || !e.receivers.iter().any(|r| r.confirmed) {
            continue;
        }
        let profile = e.reference.as_ref().map(|r| r.profile.as_str());
        let call = e.identity.as_ref().map(|i| i.callsign.as_str());
        let key = (
            profile,
            call,
            e.frequency_hz.to_bits(),
            (profile.is_none() && call.is_none()).then_some(e.id.as_str()),
        );
        let index = *groups.entry(key).or_insert_with(|| {
            latest.push((e, None));
            latest.len() - 1
        });
        if profile.is_some()
            && latest[index].1.is_none()
            && (e.estimate.is_some() || !e.hypotheses.is_empty())
        {
            latest[index].1 = Some(e);
        }
    }
    latest
}

#[cfg(test)]
#[test]
fn reviewed_emissions_keep_their_own_measurement() {
    let mut recorded: Emission =
        serde_json::from_str(include_str!("../evidence/replay-result.json")).unwrap();
    recorded.identity = None;
    recorded.reference = None;
    let mut earlier = recorded.clone();
    earlier.identity = Some(Identity {
        callsign: "EXAMPLE".into(),
        evidence: recorded.provenance.clone(),
    });
    let mut later = earlier.clone();
    later.id = "later".into();
    later.hypotheses.clear();
    later.estimate = None;
    let mut another = recorded.clone();
    another.id = "independent".into();
    let mut silent = another.clone();
    silent.detected = false;
    let signals = latest_signals(&[&silent, &later, &earlier, &recorded, &another]);
    assert_eq!(
        signals
            .iter()
            .map(|(e, _)| e.id.as_str())
            .collect::<Vec<_>>(),
        ["later", &recorded.id, "independent"]
    );
    assert!(signals.iter().all(|(_, prior)| prior.is_none()));
}

async fn emissions(State(app): State<App>, Query(q): Query<Filters>) -> Api<Value> {
    let compact = q.compact;
    let s = selection(q)?;
    let results = select(&app, &s)?;
    let selected: Vec<&Emission> = results.iter().collect();
    let row = |e: &&Emission| {
        if !compact {
            return json!(e);
        }
        json!({"id":e.id,"identity":e.identity,"frequency_hz":e.frequency_hz,"start_utc":e.start_utc,
            "status":e.status,"estimate":e.estimate,"hypotheses":e.hypotheses,
            "confirmed_count":e.receivers.iter().filter(|r|r.confirmed).count(),
            "pair_count":e.pairs.len(),"accepted_pair_count":e.pairs.iter().filter(|p|p.accepted).count(),
            "used_ids":e.receivers.iter().filter(|r|r.used).map(|r|&r.receiver.id).collect::<Vec<_>>(),
            "profile":e.reference.as_ref().map(|r| &r.profile)})
    };
    let visible: Vec<_> = selected
        .iter()
        .filter(|e| {
            e.estimate
                .iter()
                .chain(e.hypotheses.iter())
                .any(|est| s.bbox.is_none_or(|b| geo::intersects(&est.polygon, b)))
        })
        .map(row)
        .collect();
    let unlocated: Vec<_> = selected
        .iter()
        .filter(|e| e.status == "undetermined" && e.estimate.is_none() && e.hypotheses.is_empty())
        .map(row)
        .collect();
    let history: Vec<_> = selected
        .iter()
        .filter(|e| compact && e.reference.is_some())
        .take(120)
        .map(row)
        .collect();
    let signals: Vec<_> = latest_signals(if compact { &selected } else { &[] }).into_iter()
        .map(|(e, position)| {
            let mut signal = row(&e);
            let receivers: Vec<_> = e.receivers.iter().filter(|r| r.confirmed)
                .map(|r| json!({"id":r.receiver.id,"position":r.receiver.position,"label":r.receiver.label})).collect();
            signal["confirmed_receivers"] = json!(receivers);
            if let Some(earlier) = position.filter(|p| p.id != e.id) {
                signal["last_position"] = row(&earlier);
            }
            signal
        })
        .collect();
    let band_stats = if compact && s.mode == "live" && !s.fault {
        let jobs = app
            .db
            .jobs_between(s.start.timestamp_millis(), s.end.timestamp_millis())
            .map_err(internal)?;
        evaluation::bands(&results, s.start, s.end, &jobs, &app.config.targets)
    } else {
        Vec::new()
    };
    let segments:Vec<_>=selected.iter().map(|e|json!({"start":e.start_utc,"end":e.end_utc,"frequency_hz":e.frequency_hz,"status":e.status,"receivers":e.receivers.len()})).collect();
    let mut intervals: Vec<_> = selected
        .iter()
        .filter_map(|e| {
            Some((
                DateTime::parse_from_rfc3339(&e.start_utc)
                    .ok()?
                    .with_timezone(&Utc)
                    .max(s.start),
                DateTime::parse_from_rfc3339(&e.end_utc)
                    .ok()?
                    .with_timezone(&Utc)
                    .min(s.end),
            ))
        })
        .collect();
    intervals.sort_by_key(|v| v.0);
    let mut cursor = s.start;
    let mut gaps = vec![];
    for (a, b) in intervals {
        if a > cursor {
            gaps.push(json!({"start":cursor,"end":a}));
        }
        cursor = cursor.max(b);
    }
    if cursor < s.end {
        gaps.push(json!({"start":cursor,"end":s.end}));
    }
    let monitoring = if selected.is_empty() {
        "unmonitored"
    } else if gaps.is_empty() {
        "captured_channels_only"
    } else {
        "partial"
    };
    Ok(Json(
        json!({"emissions":visible,"unlocated":unlocated,"history":history,"signals":signals,"band_stats":band_stats,"observatory":monitor::snapshot(&app),"monitoring":{"status":monitoring,"segments":segments,"gaps":gaps,"scope":"Only the sampled channels, receivers and times."},"total_matching":selected.iter().filter(|e|e.detected).count()}),
    ))
}
async fn detail(State(app): State<App>, ApiPath(id): ApiPath<String>) -> Api<Emission> {
    app.db
        .emission(&id)
        .map_err(internal)?
        .map(Json)
        .ok_or(Error(StatusCode::NOT_FOUND, "Emission not found".into()))
}
async fn evaluate_observation(State(app): State<App>, ApiPath(id): ApiPath<String>) -> Api<Value> {
    app.db
        .emission(&id)
        .map_err(internal)?
        .as_ref()
        .and_then(evaluation::report)
        .map(Json)
        .ok_or(Error(
            StatusCode::NOT_FOUND,
            "No published evaluation reference for this observation".into(),
        ))
}
async fn beacons(State(app): State<App>) -> Json<Value> {
    let guard = app.directory.read().unwrap();
    let directory = guard.as_ref();
    let mut receivers = Vec::new();
    let mut other_receivers = Vec::new();
    if let Some(d) = directory {
        for r in &d.receivers {
            let Some(position) = r.published_position() else {
                continue;
            };
            let bands: Vec<_> = ibp::CHANNELS
                .iter()
                .filter(|(_, h)| r.covers(*h as f64 - 700.))
                .map(|(b, _)| b)
                .collect();
            if bands.is_empty() {
                other_receivers.push(json!({
                    "id":r.id, "label":r.name, "position":position,
                    "reason":r.exclusion().unwrap_or("No monitored HF channel in the published tuning range"),
                    "gps_good":r.gps_good, "fixes_min":r.fixes_min,
                    "users":r.users, "users_max":r.users_max
                }));
            } else {
                receivers.push(json!({"id":r.id,"label":r.name,"position":position,"bands":bands}));
            }
        }
    }
    let channels: Vec<_> = ibp::CHANNELS
        .iter()
        .map(|(b, h)| json!({"band":b,"frequency_hz":h}))
        .collect();
    Json(
        json!({"channels":channels,"callsigns":ibp::CALLS,"schedule_source":ibp::SOURCE,"server_utc":Utc::now(),
        "directory_utc":directory.map(|d| &d.fetched_at),"directory_total":directory.map(|d|d.receivers.len()),
        "receiver_pool":receivers,"other_receivers":other_receivers}),
    )
}
fn pending_captures() -> Vec<Manifest> {
    std::fs::read_dir("data/captures")
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| std::fs::read(e.path().join("manifest.json")).ok())
        .filter_map(|v| serde_json::from_slice::<Manifest>(&v).ok())
        .filter(|m| !m.emission_verified && !slot_profile(m))
        .collect()
}

async fn info(State(app): State<App>) -> Json<Value> {
    Json(
        json!({"name":"Meridian","version":env!("CARGO_PKG_VERSION"),"live_enabled":app.config.live_enabled,"band_ranges_mhz":bands::BANDS,"admin_configured":app.token.is_some(),"sources":app.config.sources,"targets":app.config.targets,"pending_captures":pending_captures(),"busy":busy(&app),"latest_live":app.db.latest_live_start().ok().flatten().and_then(DateTime::from_timestamp_millis),"datasets":app.datasets.iter().map(|(m,_)|m).collect::<Vec<_>>(),"policy":"Public beacons, participating stations and the public DCF77 time reference for commissioning. Identity is not inferred from frequency or position."}),
    )
}
#[derive(Default, Deserialize)]
struct SourceQuery {
    offset: Option<usize>,
    limit: Option<usize>,
    eligible: Option<bool>,
    search: Option<String>,
}
async fn source_directory(State(app): State<App>, Query(q): Query<SourceQuery>) -> Api<Value> {
    let guard = app.directory.read().unwrap();
    let Some(d) = guard.as_ref() else {
        return Ok(Json(
            json!({"available":false,"receivers":[],"note":"Directory not available yet; this does not indicate an absence of RF emissions."}),
        ));
    };
    let search = q.search.unwrap_or_default().to_lowercase();
    let matching: Vec<_> = d
        .receivers
        .iter()
        .filter(|r| {
            (!q.eligible.unwrap_or(false) || r.exclusion().is_none())
                && (search.is_empty()
                    || format!("{} {} {}", r.id, r.name, r.antenna)
                        .to_lowercase()
                        .contains(&search))
        })
        .collect();
    let offset = q.offset.unwrap_or(0);
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let rows: Vec<_> = matching
        .iter()
        .skip(offset)
        .take(limit)
        .map(|r| json!({"receiver":r,"exclusion":r.exclusion(),"reception_confirmed":false}))
        .collect();
    let age = DateTime::parse_from_rfc3339(&d.fetched_at)
        .map(|t| Utc::now().signed_duration_since(t).num_seconds())
        .unwrap_or(i64::MAX);
    Ok(Json(
        json!({"available":true,"source":directory::URL,"fetched_at":d.fetched_at,"stale":age>21600,"total":d.receivers.len(),"api_allowed":d.receivers.iter().filter(|r|r.ext_api.unwrap_or(0)>0).count(),"candidates":d.receivers.iter().filter(|r|r.exclusion().is_none()).count(),"matching":matching.len(),"offset":offset,"next_offset":if offset+rows.len()<matching.len(){Some(offset+rows.len())}else{None},"receivers":rows,"note":"Directory metadata, not confirmed RF receptions. Access and GNSS are checked again before capture and in IQ."}),
    ))
}
async fn update_directory(State(app): State<App>, headers: HeaderMap) -> Api<Value> {
    admin(&app, &headers)?;
    if app.directory_refreshing.swap(true, Ordering::SeqCst) {
        return Err(Error(
            StatusCode::CONFLICT,
            "Refresh already running".into(),
        ));
    }
    let result = directory::refresh(Path::new("data/state/directory.json")).await;
    app.directory_refreshing.store(false, Ordering::SeqCst);
    let d = result.map_err(bad)?;
    let n = d.receivers.len();
    *app.directory.write().unwrap() = Some(d);
    Ok(Json(
        json!({"receivers":n,"cache_seconds":directory::CACHE_SECONDS}),
    ))
}
#[derive(Deserialize)]
struct ReplayRequest {
    dataset_id: String,
    fault_receiver: Option<String>,
    #[serde(default)]
    force: bool,
}
type WorkGuard = tokio::sync::OwnedSemaphorePermit;

fn busy(app: &App) -> bool {
    app.workers.available_permits() < app.config.max_parallel_jobs.max(1)
}

fn reserve_worker(app: &App) -> Result<WorkGuard, Error> {
    if app.stopping.load(Ordering::SeqCst) {
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service is stopping".into(),
        ));
    }
    let guard = app
        .workers
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error(StatusCode::CONFLICT, "Every worker is busy".into()))?;
    if app.stopping.load(Ordering::SeqCst) {
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service is stopping".into(),
        ));
    }
    Ok(guard)
}
fn new_job(app: &App, monitor_target: Option<&str>) -> Result<String, Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let mut value = json!({"id":id,"status":"running","started_at":Utc::now().to_rfc3339()});
    if let Some(target) = monitor_target {
        value["monitor_target"] = json!(target);
    }
    if let Err(e) = app.db.put_job(&id, &value) {
        return Err(Error(StatusCode::INSUFFICIENT_STORAGE, e.to_string()));
    }
    Ok(id)
}
fn lock_job(app: &App) -> Result<(String, WorkGuard), Error> {
    let guard = reserve_worker(app)?;
    Ok((new_job(app, None)?, guard))
}
fn record_job(app: &App, id: String, mut value: Value) {
    value["updated_at"] = json!(Utc::now().to_rfc3339());
    if let Ok(Some(job)) = app.db.job(&id) {
        for key in ["started_at", "monitor_target"] {
            if let Some(existing) = job.get(key) {
                value[key] = existing.clone();
            }
        }
    }
    if let Err(e) = app.db.put_job(&id, &value) {
        eprintln!("Job persistence failed: {e}");
        value["persistence_error"] = json!(e.to_string());
        if let Ok(mut failed) = app.persistence_failures.lock() {
            failed.insert(id);
        }
    }
}
/// Decide how long the capture's raw IQ is kept, then apply the time rules immediately.
fn retire_capture(app: &App, manifest: &Path, verified: bool) {
    let (Some(dir), Ok(captures)) = (manifest.parent(), Path::new("data/captures").canonicalize())
    else {
        return;
    };
    if !dir.canonicalize().is_ok_and(|d| d.starts_with(&captures)) {
        return; // replays and imported datasets are never retired
    }
    if let Err(e) = retention::mark(dir, verified) {
        eprintln!("Retention marker failed: {e}");
    }
    if let Err(e) = retention::sweep(&captures, &app.config.retention, Utc::now()) {
        eprintln!("IQ retention sweep failed: {e}");
    }
}
/// Captures analysed slot by slot (one observation per transmission) rather than as one emission.
fn slot_profile(m: &Manifest) -> bool {
    matches!(
        m.capture_profile.as_deref(),
        Some(ibp::PROFILE) | Some(ft8_tdoa::PROFILE)
    )
}
fn process_ibp(db: &Db, path: &Path, capture_id: &str) -> anyhow::Result<(Vec<Emission>, Value)> {
    let m: Manifest = serde_json::from_slice(&std::fs::read(path)?)?;
    let (emissions, summary) = if m.capture_profile.as_deref() == Some(ft8_tdoa::PROFILE) {
        let a = ft8_tdoa::analyze(path)?;
        let s = a.summary(capture_id);
        (a.emissions, s)
    } else {
        let a = ibp::analyze(path)?;
        let s = a.summary(capture_id);
        (a.emissions, s)
    };
    db.replace_dataset(capture_id, &emissions)?;
    Ok((emissions, summary))
}
/// Raw IQ worth keeping: a verified callsign (IBP) or a transmission timed at two receivers (FT8).
fn worth_keeping(rows: &[Emission]) -> bool {
    rows.iter().any(|e| {
        e.identity.is_some()
            && (e
                .reference
                .as_ref()
                .is_some_and(|r| r.profile == ibp::PROFILE)
                || e.receivers.iter().filter(|r| r.confirmed).count() >= 2)
    })
}
async fn finish(app: App, id: String, path: Result<PathBuf, anyhow::Error>, fault: Option<String>) {
    if let Ok(p) = &path {
        if let Ok(bytes) = std::fs::read(p) {
            if let Ok(m) = serde_json::from_slice::<Manifest>(&bytes) {
                if slot_profile(&m) {
                    let file = p.clone();
                    let capture = m.id.clone();
                    let db = app.db.clone();
                    let result =
                        tokio::task::spawn_blocking(move || process_ibp(&db, &file, &capture))
                            .await
                            .map_err(anyhow::Error::from)
                            .and_then(|v| v);
                    let mut value = match result {
                        Ok((rows, summary)) => {
                            retire_capture(&app, p, worth_keeping(&rows));
                            summary
                        }
                        Err(err) => {
                            let mut v = json!({"status":"failed","capture_id":m.id,"error":err.to_string()});
                            if err
                                .downcast_ref::<std::io::Error>()
                                .is_some_and(|e| e.kind() == std::io::ErrorKind::StorageFull)
                            {
                                v["persistence_error"] = json!(err.to_string());
                            }
                            v
                        }
                    };
                    value["id"] = json!(id);
                    record_job(&app, id, value);
                    return;
                }
                if m.mode == "live" && !m.emission_verified {
                    // Kept for the review window, then expires like verified IQ.
                    retire_capture(&app, p, true);
                    record_job(
                        &app,
                        id.clone(),
                        json!({"id":id,"status":"awaiting_review","capture_id":m.id,"message":"Capture saved. Confirm emission eligibility with evidence before localization."}),
                    );
                    return;
                }
            }
        }
    }
    let manifest = path.as_ref().ok().cloned();
    let result = match path {
        Ok(p) => tokio::task::spawn_blocking(move || pipeline::process(&p, fault.as_deref()))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r),
        Err(e) => Err(e),
    };
    if let (Some(m), Ok(e)) = (&manifest, &result) {
        if e.mode == "live" {
            retire_capture(&app, m, e.detected);
        }
    }
    let value = match result {
        Ok(e) => match app.db.put_emissions(std::slice::from_ref(&e)) {
            Ok(()) => json!({"id":id,"status":"completed","emission_id":e.id}),
            Err(err) => {
                json!({"id":id,"status":"failed","persistence_error":err.to_string(),"error":err.to_string()})
            }
        },
        Err(err) => {
            let mut job = json!({"id":id,"status":"failed","error":err.to_string(),"monitoring":"unmonitored"});
            if err
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::StorageFull)
            {
                job["persistence_error"] = json!(err.to_string());
            }
            job
        }
    };
    record_job(&app, id, value);
}
fn same_origin(headers: &HeaderMap) -> Result<(), Error> {
    if let Some(origin) = headers.get("origin") {
        let origin = origin.to_str().map_err(bad)?;
        let url = reqwest::Url::parse(origin).map_err(bad)?;
        let host = headers
            .get("host")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        if url.authority() != host {
            return Err(Error(StatusCode::FORBIDDEN, "Origin not allowed".into()));
        }
    }
    Ok(())
}
fn admin(app: &App, headers: &HeaderMap) -> Result<(), Error> {
    same_origin(headers)?;
    let expected = app.token.as_ref().as_ref().ok_or(Error(
        StatusCode::FORBIDDEN,
        "Administrator token not configured".into(),
    ))?;
    let received = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    use sha2::{Digest, Sha256};
    let a = Sha256::digest(expected.as_bytes());
    let b = Sha256::digest(received.as_bytes());
    if a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        != 0
    {
        return Err(Error(
            StatusCode::UNAUTHORIZED,
            "Administrator token required".into(),
        ));
    }
    Ok(())
}
async fn replay(
    State(app): State<App>,
    headers: HeaderMap,
    Json(r): Json<ReplayRequest>,
) -> Api<Value> {
    same_origin(&headers)?;
    let (m, path) = app
        .datasets
        .iter()
        .find(|(m, _)| m.id == r.dataset_id)
        .ok_or(bad("Unknown recording"))?;
    if let Some(f) = &r.fault_receiver {
        if !m.receivers.iter().any(|rx| &rx.id == f) {
            return Err(bad("Unknown receiver"));
        }
    }
    let path = path.clone();
    let eid = format!(
        "{}{}",
        m.id,
        if r.fault_receiver.is_some() {
            "-sync-loss"
        } else {
            ""
        }
    );
    let cached = app.db.emission(&eid).map_err(internal)?.is_some_and(|e| {
        r.fault_receiver.as_ref().is_none_or(|id| {
            e.receivers.iter().any(|rx| {
                &rx.receiver.id == id
                    && rx.reason.as_deref().is_some_and(|s| s.starts_with("TEST:"))
            })
        })
    });
    if !r.force && cached {
        return Ok(Json(
            json!({"job_id":format!("cached-{eid}"),"cached":true}),
        ));
    }
    admin(&app, &headers)?;
    let (id, guard) = lock_job(&app)?;
    let job_id = id.clone();
    tokio::spawn(async move {
        let _guard = guard;
        finish(app, job_id, Ok(path), r.fault_receiver).await;
    });
    Ok(Json(json!({"job_id":id})))
}
#[derive(Deserialize)]
struct CaptureRequest {
    target_id: String,
    source_ids: Option<Vec<String>>,
}
async fn start_capture(
    State(app): State<App>,
    headers: HeaderMap,
    Json(r): Json<CaptureRequest>,
) -> Api<Value> {
    admin(&app, &headers)?;
    let mut target = app
        .config
        .targets
        .iter()
        .find(|t| t.id == r.target_id)
        .cloned()
        .ok_or(bad("Target not configured"))?;
    let mut config = (*app.config).clone();
    if let Some(ids) = r.source_ids {
        if !(3..=8).contains(&ids.len()) {
            return Err(bad("Select 3–8 independent receivers"));
        }
        let guard = app.directory.read().unwrap();
        let d = guard.as_ref().ok_or(bad("Directory unavailable"))?;
        config.sources = ids
            .iter()
            .map(|id| {
                d.receivers
                    .iter()
                    .find(|s| &s.id == id)
                    .ok_or(bad("Receiver missing from directory"))?
                    .source(&d.fetched_at)
                    .map_err(bad)
            })
            .collect::<Result<Vec<_>, _>>()?;
        target.source_ids = ids;
    }
    capture::authorize(&config, &target).map_err(bad)?;
    let (id, guard) = lock_job(&app)?;
    let job_id = id.clone();
    tokio::spawn(async move {
        let _guard = guard;
        let result = capture::capture(&config, &target, Path::new("data/captures")).await;
        if let Ok(manifest) = &result {
            app.db.account(
                manifest.parent().unwrap_or(Path::new(".")),
                target.duration_s,
            );
        }
        finish(app, job_id, result, None).await;
    });
    Ok(Json(json!({"job_id":id})))
}

async fn job(State(app): State<App>, ApiPath(id): ApiPath<String>) -> Api<Value> {
    if let Some(eid) = id.strip_prefix("cached-") {
        if app.db.emission(eid).map_err(internal)?.is_some() {
            return Ok(Json(
                json!({"id":id,"status":"completed","emission_id":eid,"cached":true}),
            ));
        }
    }
    app.db
        .job(&id)
        .map_err(internal)?
        .map(Json)
        .ok_or(Error(StatusCode::NOT_FOUND, "Job not found".into()))
}

#[derive(Deserialize)]
struct ReviewRequest {
    capture_id: String,
    evidence: Evidence,
    identity: Option<Identity>,
}
async fn review(
    State(app): State<App>,
    headers: HeaderMap,
    Json(r): Json<ReviewRequest>,
) -> Api<Value> {
    admin(&app, &headers)?;
    if !r.capture_id.starts_with("live-")
        || r.capture_id.len() > 50
        || !r
            .capture_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || !pipeline::valid_evidence(&r.evidence)
    {
        return Err(bad("Invalid ID or evidence"));
    }
    let path = Path::new("data/captures")
        .join(&r.capture_id)
        .join("manifest.json");
    let mut m: Manifest =
        serde_json::from_slice(&std::fs::read(&path).map_err(bad)?).map_err(bad)?;
    if m.mode != "live" || m.id != r.capture_id {
        return Err(bad("Invalid capture"));
    }
    if slot_profile(&m) {
        return Err(bad(
            "Slot-profile captures (IBP, FT8) are verified per transmission",
        ));
    }
    m.provenance = r.evidence;
    m.identity = r.identity;
    m.emission_verified = true;
    pipeline::validate_manifest(&m).map_err(bad)?;
    let (id, guard) = lock_job(&app)?;
    if let Err(err) = store::atomic_json(&path, &m) {
        record_job(
            &app,
            id.clone(),
            json!({"id":id,"status":"failed","error":err.to_string()}),
        );
        return Err(bad(err));
    }
    let job_id = id.clone();
    tokio::spawn(async move {
        let _guard = guard;
        finish(app, job_id, Ok(path), None).await;
    });
    Ok(Json(json!({"job_id":id})))
}

/// Import the former JSON archive once, and refresh bundled replay results on every start.
fn migrate(db: &Db) -> anyhow::Result<(usize, usize)> {
    let mut imported = (0, 0);
    if db.meta("legacy_json_import")?.is_none() {
        imported = db.import_legacy(Path::new("data/results"), Path::new("data/jobs"))?;
        db.set_meta("legacy_json_import", &Utc::now().to_rfc3339())?;
    }
    let mut replays = vec![];
    for entry in std::fs::read_dir("data/results")
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        if path.extension().is_some_and(|s| s == "json")
            && !path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n.starts_with("live-"))
        {
            let e: Emission = serde_json::from_slice(&std::fs::read(&path)?)?;
            if e.mode == "replay" {
                replays.push(e);
            }
        }
    }
    db.put_emissions(&replays)?;
    Ok(imported)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    // An unknown word must never fall through to starting the server.
    if let Some(cmd) = args.get(1) {
        anyhow::ensure!(
            ["discover", "process", "capture", "calibrate"].contains(&cmd.as_str()),
            "Unknown command {cmd}. Use: discover | process MANIFEST [FAULT_RECEIVER] | capture TARGET_ID | calibrate"
        );
    }
    if args.get(1).is_some_and(|v| v == "discover") {
        let d = directory::refresh(Path::new("data/state/directory.json")).await?;
        println!(
            "{}",
            json!({"total":d.receivers.len(),"api_allowed":d.receivers.iter().filter(|r|r.ext_api.unwrap_or(0)>0).count(),"candidates":d.receivers.iter().filter(|r|r.exclusion().is_none()).count(),"fetched_at":d.fetched_at})
        );
        return Ok(());
    }
    if args.get(1).is_some_and(|v| v == "calibrate") {
        // Post-hoc only: residuals of stored live DCF77 delays at the published PTB position.
        let db = Db::open(Path::new("data/meridian.sqlite"))?;
        let rows = db.emissions(&db::Query {
            mode: "live".into(),
            fault: false,
            low_hz: 77_499.,
            high_hz: 77_501.,
            start_ms: 0,
            end_ms: i64::MAX,
            limit: 1_000_000,
        })?;
        let rows: Vec<_> = rows
            .into_iter()
            .filter(|e| {
                e.reference
                    .as_ref()
                    .is_some_and(|r| r.profile == protocol::PROFILE)
            })
            .collect();
        let truth: Point = serde_json::from_str(include_str!("../web/dcf77-reference.json"))?;
        let report = json!({
            "reference": truth,
            "wgs84": meridian_tdoa::calibration::residuals(&rows, truth, geo::distance),
            "sphere": meridian_tdoa::calibration::residuals(&rows, truth, geo::sphere_distance),
            "solver": meridian_tdoa::calibration::solver(&rows, truth),
            "formulations": meridian_tdoa::calibration::formulations(&rows, truth),
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if args.get(1).is_some_and(|v| v == "process") {
        let path = args
            .get(2)
            .ok_or_else(|| anyhow::anyhow!("process MANIFEST [FAULT_RECEIVER]"))?;
        let m: Manifest = serde_json::from_slice(&std::fs::read(path)?)?;
        if slot_profile(&m) {
            anyhow::ensure!(
                args.get(3).is_none(),
                "Fault injection is available for replays"
            );
            let db = Db::open(Path::new("data/meridian.sqlite"))?;
            let (_, summary) = process_ibp(&db, Path::new(path), &m.id)?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
            return Ok(());
        }
        let e = pipeline::process(Path::new(path), args.get(3).map(String::as_str))?;
        pipeline::save(&e, Path::new("data/results"))?;
        println!("{}", serde_json::to_string_pretty(&e)?);
        return Ok(());
    }
    let config: Config = serde_json::from_slice(&std::fs::read(
        std::env::var("MERIDIAN_CONFIG").unwrap_or("config/app.json".into()),
    )?)?;
    store::atomic_json(
        Path::new("data/state/storage-check.json"),
        &json!({"checked_at":Utc::now().to_rfc3339()}),
    )?;
    if args.get(1).is_some_and(|v| v == "capture") {
        // capture TARGET_ID [--directory N [--near LAT,LON]]
        let mut target = config
            .targets
            .iter()
            .find(|t| Some(&t.id) == args.get(2))
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "capture TARGET_ID [--sources ID,… | --directory N [--near LAT,LON]]"
                )
            })?;
        let mut cfg = config.clone();
        let flag = |name: &str| {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1))
        };
        if let Some(ids) = flag("--sources") {
            let d = directory::load(Path::new("data/state/directory.json"))?;
            cfg.sources = ids
                .split(',')
                .map(|id| {
                    d.receivers
                        .iter()
                        .find(|r| r.id == id)
                        .ok_or_else(|| anyhow::anyhow!("{id} is not in the directory"))?
                        .source(&d.fetched_at)
                })
                .collect::<anyhow::Result<_>>()?;
            target.source_ids = cfg.sources.iter().map(|s| s.id.clone()).collect();
        } else if let Some(n) = flag("--directory") {
            let d = directory::load(Path::new("data/state/directory.json"))?;
            let n: usize = n.parse()?;
            let center = capture::iq_center(&target);
            let excluded = Db::open(Path::new("data/meridian.sqlite"))?.unusable_receivers(
                Utc::now().timestamp_millis(),
                &cfg.receivers,
                target.duration_s as f64,
            )?;
            let round = match flag("--near") {
                Some(p) => {
                    let (lat, lon) = p
                        .split_once(',')
                        .ok_or_else(|| anyhow::anyhow!("--near LAT,LON"))?;
                    d.anchor_near(
                        center,
                        Point {
                            lat: lat.parse()?,
                            lon: lon.parse()?,
                        },
                        &excluded,
                    )?
                }
                None => 0,
            };
            cfg.sources = d.network_excluding(center, round, n, &excluded)?;
            target.source_ids = cfg.sources.iter().map(|s| s.id.clone()).collect();
        }
        let result = capture::capture(&cfg, &target, Path::new("data/captures")).await;
        let db = Db::open(Path::new("data/meridian.sqlite"))?;
        // Account even for a failed attempt: the newest capture directory holds the outcomes.
        if let Some(dir) = std::fs::read_dir("data/captures")?
            .flatten()
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .max()
            .map(|(_, p)| p)
        {
            db.account(&dir, target.duration_s);
        }
        let manifest = result?;
        println!("{}", manifest.display());
        return Ok(());
    }
    let db = Arc::new(Db::open(Path::new("data/meridian.sqlite"))?);
    let interrupted = db.interrupt_running()?;
    if interrupted > 0 {
        eprintln!("{interrupted} unfinished job(s) marked interrupted");
    }
    // Bundled replay results always reflect the shipped files; live JSON is imported once.
    let (imported, imported_jobs) = migrate(&db)?;
    if imported + imported_jobs > 0 {
        eprintln!(
            "Imported {imported} observation(s) and {imported_jobs} job(s) into data/meridian.sqlite"
        );
    }
    // Recover only a known IBP capture saved before its slot bundle was stored.
    if let Ok(captures) = std::fs::read_dir("data/captures") {
        for entry in captures.flatten() {
            let path = entry.path().join("manifest.json");
            let Some(m) = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<Manifest>(&b).ok())
            else {
                continue;
            };
            if !slot_profile(&m)
                || entry.file_name().to_str() != Some(m.id.as_str())
                || db.has_dataset(&m.id)?
            {
                continue;
            }
            if let Err(err) = process_ibp(&db, &path, &m.id) {
                eprintln!("Could not process IBP capture {}: {err}", m.id);
            }
        }
    }
    let mut datasets = vec![];
    let mut paths = vec![PathBuf::from("data/recordings/manifest.json")];
    if let Ok(entries) = std::fs::read_dir("data/datasets") {
        for entry in entries.flatten() {
            paths.push(entry.path().join("manifest.json"));
        }
    }
    for p in paths {
        if p.exists() {
            let m: Manifest = serde_json::from_slice(&std::fs::read(&p)?)?;
            pipeline::validate_manifest(&m)?;
            datasets.push((m, p));
        }
    }
    let listen = std::env::var("PORT")
        .map(|p| format!("0.0.0.0:{p}"))
        .unwrap_or_else(|_| config.listen.clone());
    let parallel = config.max_parallel_jobs.max(1);
    let app = App {
        config: Arc::new(config),
        db,
        persistence_failures: Arc::new(Mutex::new(HashSet::new())),
        workers: Arc::new(tokio::sync::Semaphore::new(parallel)),
        leases: Arc::new(Mutex::new(HashSet::new())),
        datasets: Arc::new(datasets),
        token: Arc::new(
            std::env::var("MERIDIAN_ADMIN_TOKEN_FILE")
                .ok()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .or_else(|| std::env::var("MERIDIAN_ADMIN_TOKEN").ok())
                .map(|s| s.trim().to_owned())
                .filter(|s| s.len() >= 24),
        ),
        directory: Arc::new(RwLock::new(
            directory::load(Path::new("data/state/directory.json")).ok(),
        )),
        directory_refreshing: Arc::new(AtomicBool::new(false)),
        stopping: Arc::new(AtomicBool::new(false)),
        monitor: Arc::new(RwLock::new(monitor::Status::default())),
    };
    let directory_app = app.clone();
    if std::env::var("MERIDIAN_DIRECTORY_OFFLINE").as_deref() != Ok("1") {
        tokio::spawn(async move {
            loop {
                if directory_app.stopping.load(Ordering::SeqCst) {
                    break;
                }
                if !directory_app
                    .directory_refreshing
                    .swap(true, Ordering::SeqCst)
                {
                    match directory::refresh(Path::new("data/state/directory.json")).await {
                        Ok(d) => *directory_app.directory.write().unwrap() = Some(d),
                        Err(e) => eprintln!("Directory refresh unavailable: {e}"),
                    }
                    directory_app
                        .directory_refreshing
                        .store(false, Ordering::SeqCst);
                }
                tokio::time::sleep(std::time::Duration::from_secs(directory::CACHE_SECONDS)).await;
            }
        });
    }
    let retention_app = app.clone();
    tokio::spawn(async move {
        loop {
            if retention_app.stopping.load(Ordering::SeqCst) {
                break;
            }
            let policy = retention_app.config.retention.clone();
            let db = retention_app.db.clone();
            let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                let iq = retention::sweep(Path::new("data/captures"), &policy, Utc::now())?;
                let rows = db.prune(&policy, Utc::now().timestamp_millis())?;
                Ok((iq, rows))
            })
            .await;
            match outcome {
                Ok(Ok((iq, (results, jobs))))
                    if iq.iq_files_deleted + iq.capture_dirs_deleted + results + jobs > 0 =>
                {
                    eprintln!(
                        "Retention: {} IQ file(s), {} byte(s), {} capture dir(s), {results} result(s), {jobs} job(s) removed",
                        iq.iq_files_deleted, iq.iq_bytes_deleted, iq.capture_dirs_deleted
                    )
                }
                Ok(Err(e)) => eprintln!("Retention failed: {e}"),
                _ => {}
            }
            tokio::time::sleep(std::time::Duration::from_secs(600)).await;
        }
    });
    let shutdown_app = app.clone();
    let monitor_target = monitor::configured_targets(&app.config)?;
    let router = Router::new()
        .route(
            "/api/health",
            get(|| async { Json(json!({"status":"ok"})) }),
        )
        .route("/api/info", get(info))
        .route("/api/beacons", get(beacons))
        .route("/api/ready", get(ready))
        .route("/api/sources", get(source_directory))
        .route("/api/directory/refresh", post(update_directory))
        .route("/api/emissions", get(emissions))
        .route("/api/emissions/{id}", get(detail))
        .route("/api/emissions/{id}/evaluation", get(evaluate_observation))
        .route("/api/replay", post(replay))
        .route("/api/capture", post(start_capture))
        .route("/api/review", post(review))
        .route("/api/jobs/{id}", get(job))
        .with_state(app)
        .fallback_service(ServeDir::new("web"))
        .layer(axum::middleware::from_fn(headers))
        .layer(axum::extract::DefaultBodyLimit::max(16384));
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    if let Some((settings, target)) = monitor_target {
        tokio::spawn(monitor::run(shutdown_app.clone(), settings, target));
    }
    println!("Meridian · http://{listen}");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            tokio::select! {_=tokio::signal::ctrl_c()=>(),_=terminate.recv()=>()}
            shutdown_app.stopping.store(true, Ordering::SeqCst);
            // Let the bounded capture/processing finish. After a forced stop the durable job is recovered as interrupted.
            for _ in 0..180 {
                if !busy(&shutdown_app) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        })
        .await?;
    Ok(())
}
