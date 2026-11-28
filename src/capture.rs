//! Public KiwiSDR capture with operator preflight, quotas and GNSS-timed IQ.
use crate::{
    geo,
    model::*,
    pipeline::{checksum, valid_evidence, valid_position},
};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;

pub fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() < 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn recorder_python(configured: &str) -> Result<PathBuf> {
    if !configured.contains('/') {
        return Ok(PathBuf::from(configured));
    }
    // The recorder runs from its capture directory. Keep the venv symlink so
    // Python can find its pyvenv.cfg and installed packages.
    let path = std::path::absolute(configured)?;
    ensure!(
        path.is_file(),
        "Python interpreter not found: {}",
        path.display()
    );
    Ok(path)
}

/// IQ centre frequency for a target: IBP channels sit 700 Hz above the centre, the FT8
/// sub-band 0–3 kHz above the dial sits around a centre 1.5 kHz above it.
pub fn iq_center(target: &Target) -> f64 {
    match target.verification.as_deref() {
        Some(crate::ibp::PROFILE) => target.frequency_hz - 700.0,
        Some(crate::ft8_tdoa::PROFILE) => target.frequency_hz + crate::ft8_tdoa::IQ_OFFSET_HZ,
        _ => target.frequency_hz,
    }
}

pub fn validate_clock(config: &Config, duration_s: u64) -> Result<()> {
    ensure!((10..=120).contains(&duration_s), "Invalid capture duration");
    ensure!(
        valid_evidence(&config.gps_offset_evidence) && (0..=60).contains(&config.gps_utc_offset_s),
        "Verify the current GPS–UTC offset"
    );
    let valid_until = DateTime::parse_from_rfc3339(
        config
            .gps_offset_valid_until_utc
            .as_deref()
            .context("Missing GPS–UTC revalidation deadline")?,
    )?;
    ensure!(
        valid_until.with_timezone(&Utc)
            > Utc::now() + chrono::Duration::seconds(duration_s as i64 + 120),
        "Revalidate the IERS bulletin and GPS–UTC deadline before capture"
    );
    Ok(())
}

pub fn authorize(config: &Config, target: &Target) -> Result<Vec<LiveSource>> {
    if target.verification.as_deref() == Some(crate::ibp::PROFILE) {
        ensure!(
            target.kind == "public_beacon" && crate::ibp::band(target.frequency_hz).is_some(),
            "IBP capture requires a published beacon channel"
        );
    }
    if target.verification.as_deref() == Some(crate::ft8_tdoa::PROFILE) {
        ensure!(
            target.kind == "amateur_digital" && crate::bands::name(target.frequency_hz).is_some(),
            "FT8 capture requires a dial frequency inside an amateur band"
        );
    }
    ensure!(
        config.live_enabled,
        "Live capture is disabled in the configuration"
    );
    ensure!(
        [
            "public_beacon",
            "participating_amateur",
            "public_time_reference",
            "amateur_digital",
            "monitored_channel"
        ]
        .contains(&target.kind.as_str())
            && valid_evidence(&target.eligibility_evidence),
        "Target eligibility is not documented"
    );
    ensure!(safe_id(&target.id), "Invalid target id");
    ensure!(
        (10..=120).contains(&target.duration_s),
        "A capture lasts 10–120 s; {} s configured",
        target.duration_s
    );
    ensure!(
        (3..=crate::pipeline::MAX_RECEIVERS).contains(&target.source_ids.len()),
        "A capture needs 3–{} receivers; {} usable",
        crate::pipeline::MAX_RECEIVERS,
        target.source_ids.len()
    );
    ensure!(
        target.frequency_hz.is_finite() && (6000.0..=30e6).contains(&target.frequency_hz),
        "Invalid frequency"
    );
    validate_clock(config, target.duration_s)?;
    let mut sources = vec![];
    let mut devices = HashSet::new();
    for id in &target.source_ids {
        let s = config
            .sources
            .iter()
            .find(|s| &s.id == id)
            .context("Receiver is not configured")?;
        ensure!(
            safe_id(&s.id)
                && devices.insert(s.independent_device.clone())
                && !s.independent_device.is_empty(),
            "Receivers are not independent"
        );
        ensure!(
            s.permission.automatic_capture
                && valid_evidence(&s.permission.evidence)
                && (s.permission.basis == "published_api"
                    || DateTime::parse_from_rfc3339(&s.permission.expires_utc)?
                        .with_timezone(&Utc)
                        > Utc::now())
                && target.duration_s <= s.permission.max_capture_s,
            "Automatic access is missing, expired or exceeds the duration limit: {}",
            s.id
        );
        ensure!(
            s.antenna_position_verified
                && valid_position(s.position)
                && valid_evidence(&s.position_evidence)
                && s.position_uncertainty_m.is_finite()
                && (0.0..=1000.0).contains(&s.position_uncertainty_m),
            "Antenna position is unverified: {}",
            s.id
        );
        ensure!(
            valid_evidence(&s.bands_evidence)
                && s.bands_hz
                    .iter()
                    .any(|b| b[0] <= iq_center(target) - 5000.
                        && b[1] >= iq_center(target) + 5000.),
            "The IQ channel falls outside approved bands: {}",
            s.id
        );
        let url = reqwest::Url::parse(&s.base_url)?;
        ensure!(
            url.scheme() == "http"
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none(),
            "This adapter only supports a configured HTTP KiwiSDR endpoint; other protocols are rejected"
        );
        sources.push(s.clone());
    }
    ensure!(
        geo::geometry(&sources.iter().map(|s| s.position).collect::<Vec<_>>()) > 0.02,
        "Collinear receivers"
    );
    Ok(sources)
}

pub fn check_status(text: &str, source: &LiveSource, freq: f64) -> Result<Value> {
    let fields: HashMap<_, _> = text
        .lines()
        .filter_map(|s| s.trim().split_once('='))
        .filter(|(key, _)| *key != "op_email")
        .collect();
    ensure!(
        fields.get("status") == Some(&"active")
            && fields.get("offline") == Some(&"no")
            && !fields.contains_key("auth"),
        "Receiver unavailable or authentication required"
    );
    let bands = fields.get("bands").context("Missing bands in /status")?;
    let (lo, hi) = bands.split_once('-').context("Unknown bands format")?;
    ensure!(
        lo.parse::<f64>()? <= freq - 5000. && hi.parse::<f64>()? >= freq + 5000.,
        "Band unavailable"
    );
    ensure!(
        fields
            .get("freq_offset")
            .context("Unknown frequency offset")?
            .parse::<f64>()?
            == 0.,
        "Frequency-converted receivers require calibration and are not accepted by this adapter"
    );
    let gps = fields
        .get("gps")
        .context("Missing coordinates")?
        .trim_matches(|c| c == '(' || c == ')');
    let (lat, lon) = gps.split_once(',').context("Invalid GPS coordinates")?;
    let p = Point {
        lat: lat.trim().parse()?,
        lon: lon.trim().parse()?,
    };
    ensure!(
        valid_position(p)
            && geo::distance(p, source.position) * 1000. <= source.position_uncertainty_m.max(100.),
        "Reported position differs from the checked antenna metadata"
    );
    let integer = |key: &str| -> Result<u32> {
        Ok(fields
            .get(key)
            .with_context(|| format!("Missing {key}"))?
            .parse()?)
    };
    ensure!(
        integer("gps_good")? >= 4 && integer("fixes_min")? > 0,
        "GNSS unavailable during preflight"
    );
    ensure!(
        integer("users")? < integer("users_max")?,
        "No channels available"
    );
    ensure!(
        integer("ext_api")? > 0,
        "The operator does not permit external API access"
    );
    ensure!(
        integer("tdoa_ch")? > 0,
        "The operator does not provide GPS-timestamped TDoA channels"
    );
    ensure!(
        fields.get("ant_connected") == Some(&"1"),
        "Antenna disconnected"
    );
    ensure!(
        fields.get("antenna").is_some_and(|v| !v.trim().is_empty()),
        "Antenna undocumented"
    );
    Ok(
        json!({"checked_at":Utc::now().to_rfc3339(),"fields":fields,"note":"Preflight only; synchronization must also pass in each captured block."}),
    )
}

/// Follow only the receiver's own canonical origin or the documented Kiwi proxy cluster.
/// Every final endpoint is checked again; redirects to another receiver are refused.
async fn probe(
    client: &reqwest::Client,
    source: &LiveSource,
    freq: f64,
) -> Result<(LiveSource, Value)> {
    let original = reqwest::Url::parse(&source.base_url)?;
    let mut url = original.join("status")?;
    for _ in 0..4 {
        let mut response = client.get(url.clone()).send().await?.error_for_status()?;
        if response.status().is_redirection() {
            let next = url.join(
                response
                    .headers()
                    .get("location")
                    .context("Redirect has no Location")?
                    .to_str()?,
            )?;
            let a = original.host_str().unwrap_or("");
            let b = next.host_str().unwrap_or("");
            let relay = |h: &str| crate::directory::relay_of_host(h).is_some();
            ensure!(
                next.scheme() == "http"
                    && next.path() == "/status"
                    && next.query().is_none()
                    && next.username().is_empty()
                    && next.password().is_none()
                    && (a == b
                        || (relay(a) && relay(b) && a.split('.').next() == b.split('.').next())),
                "Redirect outside the checked receiver"
            );
            url = next;
            continue;
        }
        ensure!(response.status().is_success(), "Invalid status response");
        let mut bytes = vec![];
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 65536,
                "/status response too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        let observation = check_status(std::str::from_utf8(&bytes)?, source, freq)?;
        let mut canonical = source.clone();
        canonical.base_url = url.join("/")?.to_string();
        return Ok((canonical, observation));
    }
    anyhow::bail!("Too many /status redirects")
}

/// What one receiver did during a capture attempt, for fair-use accounting.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReceiverOutcome {
    pub id: String,
    pub ok: bool,
    pub reason: Option<String>,
    /// Channel time actually used on the receiver.
    pub connected_s: f64,
    /// Minimum block requested by the receiver's own refusal, seconds.
    pub block_s: f64,
    /// Endpoint contacted, when known: it tells whether a relay was in between.
    pub url: Option<String>,
    /// The host refused the TCP connection. Behind a relay, that is the relay refusing us.
    pub connection_refused: bool,
}

/// An error with its causes, e.g. "error sending request for url (…): client error (Connect):
/// tcp connect error: Connection refused (os error 111)". The outermost message alone names
/// only the URL, which hid why receivers failed.
fn describe(e: &anyhow::Error) -> String {
    format!("{e:#}")
}

/// The host did not accept the TCP connection: refused, reset or unreachable. Timeouts and
/// name-resolution errors are not counted, so a slow network never blocks a whole relay.
pub fn connection_refused(e: &anyhow::Error) -> bool {
    use std::io::ErrorKind::*;
    e.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                ConnectionRefused | ConnectionReset | HostUnreachable | NetworkUnreachable
            )
        })
    })
}

/// One ledger entry per KiwiSDR relay used by a capture. A relay answered if any receiver
/// behind it did; it refused us if every receiver behind it failed at the TCP connection.
/// Refusals back off like a receiver's, and while a relay is blocked no receiver behind it
/// is contacted.
pub fn relay_outcomes(outcomes: &[ReceiverOutcome]) -> Vec<ReceiverOutcome> {
    let mut relays = std::collections::BTreeMap::<&str, (usize, usize)>::new();
    for o in outcomes {
        if let Some(relay) = o.url.as_deref().and_then(crate::directory::relay_of) {
            let (answered, refused) = relays.entry(relay).or_default();
            if o.connection_refused {
                *refused += 1;
            } else {
                *answered += 1;
            }
        }
    }
    relays
        .into_iter()
        .map(|(relay, (answered, refused))| ReceiverOutcome {
            id: crate::directory::relay_key(relay),
            ok: answered > 0,
            reason: (answered == 0).then(|| {
                format!("Relay {relay} refused the TCP connection to {refused} receiver(s)")
            }),
            connected_s: 0.,
            block_s: 0.,
            url: None,
            connection_refused: answered == 0,
        })
        .collect()
}

/// Outcomes of a capture directory: preflight errors, IQ delivered, and receiver refusals
/// reported by KiwiClient (per-IP time limit, busy, down, bad password).
pub fn outcomes(dir: &Path, duration_s: u64) -> Vec<ReceiverOutcome> {
    let read = |name: &str| -> Vec<Value> {
        std::fs::read(dir.join(name))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    };
    let mut out = vec![];
    let checks = read("source-checks.json");
    let url = |id: &str| {
        checks
            .iter()
            .find(|c| c["source"]["id"] == id)
            .and_then(|c| c["source"]["base_url"].as_str())
            .map(str::to_owned)
    };
    for c in &checks {
        if let Some(err) = c["error"].as_str() {
            out.push(ReceiverOutcome {
                id: c["source"]["id"].as_str().unwrap_or_default().into(),
                ok: false,
                reason: Some(format!("Preflight: {err}")),
                // /status is plain HTTP; no receiver channel is opened.
                connected_s: 0.,
                block_s: 0.,
                url: c["source"]["base_url"].as_str().map(str::to_owned),
                connection_refused: c["connection_refused"] == true,
            });
        }
    }
    for c in read("capture-outcomes.json") {
        let id = c["receiver"].as_str().unwrap_or_default().to_string();
        // Measured from the opened session to its end. Records written before the
        // measurement existed count a capture as its nominal duration.
        let measured = c["connected_s"]
            .as_f64()
            .filter(|s| s.is_finite() && *s >= 0.);
        if c["captured"] == true {
            out.push(ReceiverOutcome {
                url: url(&id),
                id,
                ok: true,
                reason: None,
                connected_s: measured.unwrap_or(0.).max(duration_s as f64),
                block_s: 0.,
                connection_refused: false,
            });
            continue;
        }
        let log = std::fs::read_to_string(dir.join(format!("{id}.log"))).unwrap_or_default();
        let refusal = log
            .lines()
            .find_map(|l| l.strip_prefix("MERIDIAN-REFUSAL "))
            .map(|r| r.split_once(' ').map_or((r, ""), |(k, v)| (k, v)));
        let (reason, block_s) = match refusal {
            Some(("ip_limit", v)) => {
                // The minutes lead the value. What follows (an encoded separator and the IP the
                // receiver saw) differs between transports and older logs, and is not needed.
                let minutes: String = v.chars().take_while(char::is_ascii_digit).collect();
                (
                    format!(
                        "Receiver per-IP time limit reached ({} min per 24 h)",
                        if minutes.is_empty() { "?" } else { &minutes }
                    ),
                    86_400.,
                )
            }
            Some(("too_busy", _)) => ("All receiver channels busy".into(), 600.),
            Some(("down", _)) => ("Receiver reports it is down".into(), 3600.),
            Some(("badp", _)) => (
                "Receiver requires a password for these channels".into(),
                86_400.,
            ),
            Some((k, v)) => (format!("Receiver refused the connection ({k} {v})"), 600.),
            None => (c["error"].as_str().unwrap_or("No IQ received").into(), 0.),
        };
        out.push(ReceiverOutcome {
            url: url(&id),
            id,
            ok: false,
            reason: Some(reason),
            connected_s: measured.unwrap_or(0.),
            block_s,
            connection_refused: c["connection_refused"] == true,
        });
    }
    out
}

pub fn last_attempt(data_dir: &Path) -> Result<Option<DateTime<Utc>>> {
    #[derive(serde::Deserialize)]
    struct Attempt {
        utc: DateTime<Utc>,
    }
    match std::fs::read(data_dir.join("last-attempt.json")) {
        Ok(v) => Ok(Some(serde_json::from_slice::<Attempt>(&v)?.utc)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub struct Capture {
    config: Config,
    target: Target,
    sources: Vec<LiveSource>,
    id: String,
    directory: PathBuf,
    _reservation: std::fs::File,
    _receiver_locks: Vec<std::fs::File>,
}

impl Capture {
    pub fn receiver_ids(&self) -> Vec<String> {
        self.sources.iter().map(|s| s.id.clone()).collect()
    }
}

/// Reserve storage and the shared cooldown once before any receiver is contacted.
pub fn reserve_window(plans: Vec<(Config, Target)>, data_dir: &Path) -> Result<Vec<Capture>> {
    ensure!(!plans.is_empty(), "Empty capture window");
    let mut devices = HashSet::new();
    let mut authorized = Vec::new();
    for (config, target) in plans {
        let sources = authorize(&config, &target)?;
        for s in &sources {
            ensure!(
                devices.insert(s.independent_device.clone()),
                "Receiver already assigned to another band"
            );
        }
        authorized.push((config, target, sources));
    }
    std::fs::create_dir_all(data_dir)?;
    let mut windows = Vec::new();
    for (config, target, sources) in authorized {
        let mut locks = Vec::new();
        for source in &sources {
            use sha2::{Digest, Sha256};
            let key = Sha256::digest(source.independent_device.as_bytes());
            let lock = std::fs::File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(data_dir.join(format!(".rx-{key:x}.lock")))?;
            lock.try_lock()
                .with_context(|| format!("Receiver {} has an active capture", source.id))?;
            locks.push(lock);
        }
        let id = format!("live-{}", uuid::Uuid::new_v4());
        let (directory, reservation) =
            crate::retention::reserve(data_dir, &id, sources.len() as u64 * 64 * 1024 * 1024)?;
        windows.push(Capture {
            config,
            target,
            sources,
            id,
            directory,
            _reservation: reservation,
            _receiver_locks: locks,
        });
    }
    crate::store::atomic_json(
        &data_dir.join("last-attempt.json"),
        &json!({"utc":Utc::now().to_rfc3339()}),
    )?;
    Ok(windows)
}

pub async fn capture(
    config: &Config,
    target: &Target,
    data_dir: &Path,
    db: &crate::db::Db,
) -> Result<PathBuf> {
    let mut window = reserve_window(vec![(config.clone(), target.clone())], data_dir)?;
    record(window.pop().unwrap(), db).await
}

fn capture_minimum(target: &Target) -> usize {
    match target.verification.as_deref() {
        Some(crate::ibp::PROFILE | crate::ft8_tdoa::PROFILE) => 1,
        Some(crate::channel::PROFILE) => 2,
        _ => 3,
    }
}

pub async fn record(mut capture: Capture, db: &crate::db::Db) -> Result<PathBuf> {
    let blocked = db.unusable_receivers(
        Utc::now().timestamp_millis(),
        &capture.config.receivers,
        capture.target.duration_s as f64,
    )?;
    let unusable = |s: &LiveSource| {
        blocked.contains(&s.id) || crate::directory::behind_blocked_relay(&blocked, &s.base_url)
    };
    let skipped: Vec<_> = capture
        .sources
        .iter()
        .filter(|s| unusable(s))
        .map(|s| s.id.clone())
        .collect();
    if !skipped.is_empty() {
        crate::store::atomic_json(
            &capture.directory.join("capture-policy.json"),
            &json!({"skipped_receivers":skipped,"reason":"Receiver or relay cooldown, or daily budget"}),
        )?;
        capture.sources.retain(|s| !unusable(s));
    }
    ensure!(
        capture.sources.len() >= capture_minimum(&capture.target),
        "Insufficient eligible receivers after applying cooldown and daily budgets"
    );
    let outcome = record_iq(&capture).await;
    db.account(&capture.directory, capture.target.duration_s);
    if outcome.is_err() {
        crate::retention::mark(&capture.directory, false)?;
    }
    outcome
}

async fn record_iq(capture: &Capture) -> Result<PathBuf> {
    let config = &capture.config;
    let target = &capture.target;
    let id = capture.id.clone();
    let dir = &capture.directory;
    tokio::fs::create_dir_all(dir.join("gnss_pos")).await?;
    let dir = dir.canonicalize()?;
    let client = reqwest::Client::builder()
        .user_agent("Meridian-TDoA/0.2 source-verification")
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut checks = vec![];
    let mut available = vec![];
    let mut probes = tokio::task::JoinSet::new();
    let center = iq_center(target);
    for s in &capture.sources {
        let s = s.clone();
        let client = client.clone();
        probes.spawn(async move {
            let result = probe(&client, &s, center).await;
            (s, result)
        });
    }
    while let Some(probe) = probes.join_next().await {
        let (s, result) = probe?;
        match result {
            Ok((canonical, observation)) => {
                checks.push(json!({"source":canonical,"observation":observation}));
                available.push(canonical);
            }
            Err(e) => checks.push(
                json!({"source":s,"error":describe(&e),"connection_refused":connection_refused(&e)}),
            ),
        }
    }
    crate::store::atomic_json(&dir.join("source-checks.json"), &checks)?;
    let minimum = capture_minimum(target);
    ensure!(
        available.len() >= minimum,
        "{}: only {} receivers pass current checks; unmonitored interval. See source-checks.json",
        id,
        available.len()
    );
    ensure!(
        minimum < 3
            || geo::geometry(&available.iter().map(|s| s.position).collect::<Vec<_>>()) > 0.02,
        "Insufficient geometry after current checks"
    );
    // Select a transport before contacting the SND endpoint. Neither transport retries
    // a refused receiver. Existing HTTP CONNECT proxy deployments retain KiwiClient.
    let proxied = ["http_proxy", "HTTP_PROXY", "ALL_PROXY", "all_proxy"]
        .iter()
        .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()));
    let transport = match std::env::var("MERIDIAN_KIWI_TRANSPORT").ok().as_deref() {
        None if !proxied => None,
        Some("native") if !proxied => None,
        Some("native") => anyhow::bail!("Use the KiwiClient transport for a configured HTTP proxy"),
        None | Some("kiwiclient") => Some((
            recorder_python(&config.python)?,
            Path::new("scripts/kiwi_transport.py").canonicalize()?,
            Path::new(&config.kiwirecorder_path)
                .canonicalize()
                .context("Install KiwiClient with scripts/setup_kiwiclient.sh")?,
        )),
        Some(other) => anyhow::bail!("Unknown Kiwi transport {other}"),
    };
    let capture_time = Utc::now();
    let mut tasks = tokio::task::JoinSet::new();
    // Independent task per physical receiver: one busy/disconnected receiver cannot stop the other captures.
    for source in available {
        let transport = transport.clone();
        let dir = dir.clone();
        let target = target.clone();
        tasks.spawn(async move {
            // Seconds the receiver's channel was in use, successful or not: the ledger counts them.
            let mut connected_s = 0.;
            let result: Result<()> = async {
                let Some((python, transport, recorder)) = transport else {
                    let (result, seconds) = crate::kiwi_stream::record(
                        &source.base_url,
                        iq_center(&target),
                        target.duration_s,
                        &dir,
                        &source.id,
                    )
                    .await;
                    connected_s = seconds;
                    return result;
                };
                let url = reqwest::Url::parse(&source.base_url)?;
                let mut cmd = Command::new(python);
                cmd.arg(transport)
                    .arg(recorder)
                    .args([
                        "-s",
                        url.host_str().context("Missing host")?,
                        "-p",
                        &url.port_or_known_default()
                            .context("Missing port")?
                            .to_string(),
                        "--station",
                        &source.id,
                        "--filename",
                        "capture",
                        "-f",
                        &(iq_center(&target) / 1000.).to_string(),
                        "-m",
                        "iq",
                        "--kiwi-wav",
                        "-L",
                        "-5000",
                        "-H",
                        "5000",
                        "--tlimit",
                        &target.duration_s.to_string(),
                        "--connect-timeout",
                        "8",
                        "--connect-retries",
                        "1",
                        "--busy-retries",
                        "1",
                        "--socket-timeout",
                        "8",
                        "--user",
                        "Meridian-TDoA measurement",
                        "--quiet",
                    ])
                    .arg("--dir")
                    .arg(&dir)
                    .current_dir(&dir)
                    .env("OPENBLAS_NUM_THREADS", "1")
                    .kill_on_drop(true);
                let log = std::fs::File::create(dir.join(format!("{}.log", source.id)))?;
                cmd.stdout(log.try_clone()?).stderr(log);
                // KiwiClient holds the whole session: its run time bounds the channel time.
                let started = std::time::Instant::now();
                let status =
                    tokio::time::timeout(Duration::from_secs(target.duration_s + 20), cmd.status())
                        .await;
                connected_s = started.elapsed().as_secs_f64();
                let status = status.context("Timeout IQ")??;
                ensure!(status.success(), "KiwiClient exited with an error");
                Ok(())
            }
            .await;
            (source, result, connected_s)
        });
    }
    let mut receivers = vec![];
    let mut outcomes = vec![];
    while let Some(result) = tasks.join_next().await {
        let (s, status, connected_s) = result?;
        let file = dir.join(format!("capture_{}.wav", s.id));
        if status.is_err() || !file.is_file() || std::fs::metadata(&file)?.len() < 44 {
            let refused = status.as_ref().err().is_some_and(connection_refused);
            outcomes.push(json!({"receiver":s.id,"captured":false,
                "error":status.err().map(|e|describe(&e)).unwrap_or("No IQ received".into()),
                "connection_refused":refused,"connected_s":connected_s}));
            continue;
        }
        ensure!(
            std::fs::metadata(&file)?.len() <= 64 * 1024 * 1024,
            "IQ file too large"
        );
        outcomes.push(json!({"receiver":s.id,"captured":true,"connected_s":connected_s}));
        receivers.push(Receiver {
            id: s.id,
            label: s.label,
            independent_device: s.independent_device,
            position: s.position,
            position_evidence: s.position_evidence,
            position_uncertainty_m: s.position_uncertainty_m,
            file: file.file_name().unwrap().to_string_lossy().into(),
            sha256: checksum(&file)?,
            timing_sigma_ns: None,
        });
    }
    crate::store::atomic_json(&dir.join("capture-outcomes.json"), &outcomes)?;
    ensure!(
        receivers.len() >= minimum,
        "{}: only {} IQ streams saved; no valid TDoA measurement. See capture-outcomes.json and receiver logs",
        id,
        receivers.len()
    );
    receivers.sort_by(|a, b| a.id.cmp(&b.id));
    let manifest = Manifest {
        id,
        title: format!("Monitored emission · {}", target.id),
        kind: target.kind.clone(),
        mode: "live".into(),
        frequency_hz: target.frequency_hz,
        capture_profile: target.verification.clone(),
        iq_center_hz: Some(iq_center(target)),
        capture_utc: capture_time.to_rfc3339(),
        gps_utc_offset_s: config.gps_utc_offset_s,
        provenance: target.eligibility_evidence.clone(),
        emission_verified: false,
        identity: None,
        receivers,
        rejected_receivers: vec![],
    };
    // Eligibility of a monitored channel never establishes the callsign of a particular emission.
    let path = dir.join("manifest.json");
    crate::store::atomic_json(&path, &manifest)?;
    if target.verification.as_deref() == Some(crate::protocol::PROFILE) {
        // Failure leaves the durable capture pending review; it never promotes a channel by frequency alone.
        if let Err(e) = crate::protocol::verify(&path) {
            crate::store::atomic_json(
                &dir.join("verification-error.json"),
                &json!({"error":describe(&e)}),
            )?;
        }
    }
    Ok(path)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn recorder_preserves_virtualenv_interpreter_and_path_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let system = dir.path().join("system-python");
        let venv = dir.path().join("venv/bin/python");
        std::fs::write(&system, []).unwrap();
        std::fs::create_dir_all(venv.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&system, &venv).unwrap();

        assert_eq!(recorder_python(venv.to_str().unwrap()).unwrap(), venv);
        assert_eq!(recorder_python("python3").unwrap(), Path::new("python3"));
    }

    #[test]
    fn actual_status_is_accepted_but_api_optout_missing_policy_and_gnss_loss_are_refused() {
        let c: Config = serde_json::from_str(include_str!("../config/live-verified.json")).unwrap();
        let source = c.sources.iter().find(|s| s.id == "0cb2b7d6cd86").unwrap();
        let v: Value =
            serde_json::from_str(include_str!("../evidence/live/status-0cb2b7d6cd86.json"))
                .unwrap();
        let text = v["body"].as_str().unwrap();
        assert!(check_status(text, source, 77500.).is_ok());
        for (field, value) in [
            ("ext_api", "0"),
            ("tdoa_ch", "-1"),
            ("gps_good", "0"),
            ("ant_connected", "0"),
            ("offline", "yes"),
            ("gps", "(0, 0)"),
        ] {
            let changed = text
                .lines()
                .map(|line| {
                    if line.starts_with(&format!("{field}=")) {
                        format!("{field}={value}")
                    } else {
                        line.into()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                check_status(&changed, source, 77500.).is_err(),
                "{field} must reject"
            );
        }
        let missing = text
            .lines()
            .filter(|s| !s.starts_with("ext_api="))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(check_status(&missing, source, 77500.).is_err());
        assert!(
            !check_status(text, source, 77500.).unwrap()["fields"]
                .as_object()
                .unwrap()
                .contains_key("op_email")
        );
    }

    #[test]
    fn per_ip_limit_minutes_are_read_from_every_transport_log_format() {
        let dir = tempfile::tempdir().unwrap();
        let logs = [
            ("native", "MERIDIAN-REFUSAL ip_limit 120\n"),
            ("kiwiclient", "MERIDIAN-REFUSAL ip_limit 120%2c127.0.0.1\n"),
            // The native transport before this fix, as recorded on Render on 28 November 2026.
            ("legacy", "MERIDIAN-REFUSAL ip_limit 120%2127001\n"),
        ];
        let rows: Vec<Value> = logs
            .iter()
            .map(|(id, _)| json!({"receiver":id,"captured":false,"error":"Receiver refused the recording (ip_limit)"}))
            .collect();
        std::fs::write(
            dir.path().join("capture-outcomes.json"),
            serde_json::to_vec(&rows).unwrap(),
        )
        .unwrap();
        for (id, log) in logs {
            std::fs::write(dir.path().join(format!("{id}.log")), log).unwrap();
        }
        let found = outcomes(dir.path(), 30);
        assert_eq!(found.len(), 3);
        for o in found {
            assert!(!o.ok);
            assert_eq!(
                o.reason.as_deref(),
                Some("Receiver per-IP time limit reached (120 min per 24 h)"),
                "{}",
                o.id
            );
            assert_eq!(o.block_s, 86_400.);
        }
    }

    #[tokio::test]
    async fn a_failed_request_is_described_with_its_cause() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let err = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/status"))
            .send()
            .await
            .unwrap_err();
        let err = err.into();
        let text = describe(&err);
        assert!(text.contains("Connection refused"), "{text}");
        assert!(connection_refused(&err), "{text}");
        let answered = anyhow::anyhow!("GNSS unavailable during preflight");
        assert!(!connection_refused(&answered));
    }

    /// Writes the files a capture leaves behind for the ledger: preflight checks and outcomes.
    pub(crate) fn capture_records(dir: &Path) {
        let source = |id: &str, url: &str| json!({"id":id,"base_url":url});
        let refused = |id: &str, url: &str| {
            json!({"source":source(id,url),"connection_refused":true,
                "error":"error sending request for url: client error (Connect): tcp connect error: Connection refused (os error 111)"})
        };
        let checks = vec![
            refused("a", "http://21001.proxy2.kiwisdr.com:8073/"),
            refused("b", "http://21002.proxy2.kiwisdr.com:8073/"),
            refused("c", "http://22001.proxy.kiwisdr.com:8073/"),
            json!({"source":source("d","http://22002.proxy.kiwisdr.com:8073/"),
                "error":"GNSS unavailable during preflight","connection_refused":false}),
            json!({"source":source("e","http://198.51.100.7:8073/"),"observation":{}}),
            json!({"source":source("f","http://198.51.100.8:8073/"),"observation":{}}),
            json!({"source":source("g","http://198.51.100.9:8073/"),"observation":{}}),
        ];
        let rows = vec![
            json!({"receiver":"e","captured":true,"connected_s":31.5}),
            json!({"receiver":"f","captured":false,"error":"Kiwi IQ capture timed out",
                "connection_refused":false,"connected_s":50.0}),
            // Written before the session time was measured.
            json!({"receiver":"g","captured":true}),
        ];
        std::fs::write(
            dir.join("source-checks.json"),
            serde_json::to_vec(&checks).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("capture-outcomes.json"),
            serde_json::to_vec(&rows).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn channel_time_counts_failed_sessions_and_a_relay_refusing_everyone_is_one_entry() {
        let dir = tempfile::tempdir().unwrap();
        capture_records(dir.path());
        let found = outcomes(dir.path(), 30);
        let by = |id: &str| found.iter().find(|o| o.id == id).unwrap();
        assert_eq!(by("e").connected_s, 31.5);
        assert_eq!(
            by("f").connected_s,
            50.0,
            "a failed session still held a channel"
        );
        assert_eq!(by("g").connected_s, 30.0);
        assert_eq!(by("a").connected_s, 0.0, "/status is not a channel");
        assert!(by("a").connection_refused && !by("d").connection_refused);
        let relays = relay_outcomes(&found);
        assert_eq!(relays.len(), 2);
        let proxy2 = relays
            .iter()
            .find(|o| o.id == "relay:proxy2.kiwisdr.com")
            .unwrap();
        assert!(!proxy2.ok && proxy2.connection_refused);
        assert_eq!(
            proxy2.reason.as_deref(),
            Some("Relay proxy2.kiwisdr.com refused the TCP connection to 2 receiver(s)")
        );
        // A receiver behind the other relay answered /status, so that relay accepted us.
        let proxy = relays
            .iter()
            .find(|o| o.id == "relay:proxy.kiwisdr.com")
            .unwrap();
        assert!(proxy.ok && proxy.reason.is_none());
    }
}
