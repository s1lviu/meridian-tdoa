use axum::{
    Json, Router,
    extract::{Path as ApiPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use meridian_tdoa::{capture, directory, geo, model::*, pipeline, store};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tower_http::services::ServeDir;

#[derive(Clone)]
struct App {
    config: Arc<Config>,
    results: Arc<RwLock<Vec<Emission>>>,
    jobs: Arc<RwLock<HashMap<String, Value>>>,
    busy: Arc<AtomicBool>,
    datasets: Arc<Vec<(Manifest, PathBuf)>>,
    token: Arc<Option<String>>,
    directory: Arc<RwLock<Option<directory::Directory>>>,
    directory_refreshing: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
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
                json!({"status":"ready","version":env!("CARGO_PKG_VERSION"),"busy":app.busy.load(Ordering::SeqCst)}),
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
        "80m" => (3.5e6, 4e6),
        "40m" => (7e6, 7.3e6),
        "20m" => (14e6, 14.35e6),
        _ => return Err(bad("Unknown band")),
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
fn matches(e: &Emission, s: &Selection) -> bool {
    let (Ok(a), Ok(b)) = (
        DateTime::parse_from_rfc3339(&e.start_utc),
        DateTime::parse_from_rfc3339(&e.end_utc),
    ) else {
        return false;
    };
    e.mode == s.mode
        && e.fault_injected == s.fault
        && e.frequency_hz >= s.low
        && e.frequency_hz < s.high
        && a < s.end
        && b > s.start
}
async fn emissions(State(app): State<App>, Query(q): Query<Filters>) -> Api<Value> {
    let s = selection(q)?;
    let results = app.results.read().unwrap();
    let selected: Vec<_> = results.iter().filter(|e| matches(e, &s)).collect();
    let visible: Vec<_> = selected
        .iter()
        .filter(|e| {
            e.estimate
                .iter()
                .chain(e.hypotheses.iter())
                .any(|est| s.bbox.is_none_or(|b| geo::intersects(&est.polygon, b)))
        })
        .map(|e| *e)
        .collect();
    let unlocated: Vec<_> = selected
        .iter()
        .filter(|e| e.status == "undetermined" && e.estimate.is_none() && e.hypotheses.is_empty())
        .map(|e| *e)
        .collect();
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
        json!({"emissions":visible,"unlocated":unlocated,"monitoring":{"status":monitoring,"segments":segments,"gaps":gaps,"scope":"Only recorded channels and receivers. This does not represent RF coverage of the viewport or monitoring of the entire band."},"total_matching":selected.iter().filter(|e|e.detected).count()}),
    ))
}
async fn detail(State(app): State<App>, ApiPath(id): ApiPath<String>) -> Api<Emission> {
    app.results
        .read()
        .unwrap()
        .iter()
        .find(|e| e.id == id)
        .cloned()
        .map(Json)
        .ok_or(Error(StatusCode::NOT_FOUND, "Emission not found".into()))
}
fn pending_captures() -> Vec<Manifest> {
    std::fs::read_dir("data/captures")
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| std::fs::read(e.path().join("manifest.json")).ok())
        .filter_map(|v| serde_json::from_slice::<Manifest>(&v).ok())
        .filter(|m| !m.emission_verified)
        .collect()
}
async fn info(State(app): State<App>) -> Json<Value> {
    Json(
        json!({"name":"Meridian","version":env!("CARGO_PKG_VERSION"),"live_enabled":app.config.live_enabled,"admin_configured":app.token.is_some(),"sources":app.config.sources,"targets":app.config.targets,"pending_captures":pending_captures(),"busy":app.busy.load(Ordering::SeqCst),"latest_live":app.results.read().unwrap().iter().filter(|e|e.mode=="live").map(|e| &e.start_utc).max(),"datasets":app.datasets.iter().map(|(m,_)|m).collect::<Vec<_>>(),"policy":"Public beacons, participating stations and the public DCF77 time reference for commissioning. Identity is not inferred from frequency or position."}),
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
fn lock_job(app: &App) -> Result<String, Error> {
    if app.stopping.load(Ordering::SeqCst) {
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service is stopping".into(),
        ));
    }
    if app.jobs.read().unwrap().len() >= 10000 {
        return Err(Error(
            StatusCode::INSUFFICIENT_STORAGE,
            "Export/archive the job history before continuing".into(),
        ));
    }
    if app
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(Error(
            StatusCode::CONFLICT,
            "A job is already running".into(),
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let value = json!({"id":id,"status":"running","started_at":Utc::now().to_rfc3339()});
    if let Err(e) = store::job(&id, &value) {
        app.busy.store(false, Ordering::SeqCst);
        return Err(Error(StatusCode::INSUFFICIENT_STORAGE, e.to_string()));
    }
    app.jobs.write().unwrap().insert(id.clone(), value);
    Ok(id)
}
fn record_job(app: &App, id: String, mut value: Value) {
    value["updated_at"] = json!(Utc::now().to_rfc3339());
    if let Some(start) = app
        .jobs
        .read()
        .unwrap()
        .get(&id)
        .and_then(|v| v.get("started_at"))
    {
        value["started_at"] = start.clone();
    }
    if let Err(e) = store::job(&id, &value) {
        eprintln!("Job persistence failed: {e}");
        value["persistence_error"] = json!(e.to_string());
    }
    app.jobs.write().unwrap().insert(id, value);
}
async fn finish(app: App, id: String, path: Result<PathBuf, anyhow::Error>, fault: Option<String>) {
    if let Ok(p) = &path {
        if let Ok(bytes) = std::fs::read(p) {
            if let Ok(m) = serde_json::from_slice::<Manifest>(&bytes) {
                if m.mode == "live" && !m.emission_verified {
                    record_job(
                        &app,
                        id.clone(),
                        json!({"id":id,"status":"awaiting_review","capture_id":m.id,"message":"Capture saved. Confirm emission eligibility with evidence before localization."}),
                    );
                    app.busy.store(false, Ordering::SeqCst);
                    return;
                }
            }
        }
    }
    let result = match path {
        Ok(p) => tokio::task::spawn_blocking(move || pipeline::process(&p, fault.as_deref()))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r),
        Err(e) => Err(e),
    };
    let value = match result {
        Ok(e) => match pipeline::save(&e, Path::new("data/results")) {
            Ok(()) => {
                let eid = e.id.clone();
                let mut rs = app.results.write().unwrap();
                rs.retain(|v| v.id != eid);
                rs.push(e);
                json!({"id":id,"status":"completed","emission_id":eid})
            }
            Err(err) => json!({"id":id,"status":"failed","error":err.to_string()}),
        },
        Err(err) => {
            json!({"id":id,"status":"failed","error":err.to_string(),"monitoring":"unmonitored"})
        }
    };
    record_job(&app, id, value);
    app.busy.store(false, Ordering::SeqCst);
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
    if !r.force
        && app.results.read().unwrap().iter().any(|e| {
            e.id == eid
                && r.fault_receiver.as_ref().is_none_or(|id| {
                    e.receivers.iter().any(|rx| {
                        &rx.receiver.id == id
                            && rx.reason.as_deref().is_some_and(|s| s.starts_with("TEST:"))
                    })
                })
        })
    {
        return Ok(Json(
            json!({"job_id":format!("cached-{eid}"),"cached":true}),
        ));
    }
    admin(&app, &headers)?;
    let id = lock_job(&app)?;
    tokio::spawn(finish(app, id.clone(), Ok(path), r.fault_receiver));
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
    let id = lock_job(&app)?;
    let job_id = id.clone();
    tokio::spawn(async move {
        let result = capture::capture(&config, &target, Path::new("data/captures")).await;
        finish(app, job_id, result, None).await;
    });
    Ok(Json(json!({"job_id":id})))
}
async fn job(State(app): State<App>, ApiPath(id): ApiPath<String>) -> Api<Value> {
    if let Some(eid) = id.strip_prefix("cached-") {
        if app.results.read().unwrap().iter().any(|e| e.id == eid) {
            return Ok(Json(
                json!({"id":id,"status":"completed","emission_id":eid,"cached":true}),
            ));
        }
    }
    app.jobs
        .read()
        .unwrap()
        .get(&id)
        .cloned()
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
    m.provenance = r.evidence;
    m.identity = r.identity;
    m.emission_verified = true;
    pipeline::validate_manifest(&m).map_err(bad)?;
    let id = lock_job(&app)?;
    if let Err(err) = store::atomic_json(&path, &m) {
        record_job(
            &app,
            id.clone(),
            json!({"id":id,"status":"failed","error":err.to_string()}),
        );
        app.busy.store(false, Ordering::SeqCst);
        return Err(bad(err));
    }
    tokio::spawn(finish(app, id.clone(), Ok(path), None));
    Ok(Json(json!({"job_id":id})))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|v| v == "discover") {
        let d = directory::refresh(Path::new("data/state/directory.json")).await?;
        println!(
            "{}",
            json!({"total":d.receivers.len(),"api_allowed":d.receivers.iter().filter(|r|r.ext_api.unwrap_or(0)>0).count(),"candidates":d.receivers.iter().filter(|r|r.exclusion().is_none()).count(),"fetched_at":d.fetched_at})
        );
        return Ok(());
    }
    if args.get(1).is_some_and(|v| v == "process") {
        let path = args
            .get(2)
            .ok_or_else(|| anyhow::anyhow!("process MANIFEST [FAULT_RECEIVER]"))?;
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
        let target = config
            .targets
            .iter()
            .find(|t| Some(&t.id) == args.get(2))
            .ok_or_else(|| anyhow::anyhow!("capture TARGET_ID (from MERIDIAN_CONFIG)"))?;
        let manifest = capture::capture(&config, target, Path::new("data/captures")).await?;
        println!("{}", manifest.display());
        return Ok(());
    }
    let mut results = vec![];
    std::fs::create_dir_all("data/results")?;
    for entry in std::fs::read_dir("data/results")? {
        let path = entry?.path();
        if path.extension().is_some_and(|s| s == "json") {
            results.push(serde_json::from_slice(&std::fs::read(path)?)?);
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
    let app = App {
        config: Arc::new(config),
        results: Arc::new(RwLock::new(results)),
        jobs: Arc::new(RwLock::new(store::recover_jobs(Path::new("data/jobs"))?)),
        busy: Arc::new(AtomicBool::new(false)),
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
    let shutdown_app = app.clone();
    let router = Router::new()
        .route(
            "/api/health",
            get(|| async { Json(json!({"status":"ok"})) }),
        )
        .route("/api/info", get(info))
        .route("/api/ready", get(ready))
        .route("/api/sources", get(source_directory))
        .route("/api/directory/refresh", post(update_directory))
        .route("/api/emissions", get(emissions))
        .route("/api/emissions/{id}", get(detail))
        .route("/api/replay", post(replay))
        .route("/api/capture", post(start_capture))
        .route("/api/review", post(review))
        .route("/api/jobs/{id}", get(job))
        .with_state(app)
        .fallback_service(ServeDir::new("web"))
        .layer(axum::middleware::from_fn(headers))
        .layer(axum::extract::DefaultBodyLimit::max(16384));
    let listener = tokio::net::TcpListener::bind(&listen).await?;
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
                if !shutdown_app.busy.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        })
        .await?;
    Ok(())
}
