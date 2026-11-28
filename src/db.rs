//! Durable single-writer store for observations and jobs (SQLite, WAL, synchronous=FULL).
//! Replaces the per-file JSON archive that was loaded entirely into memory at startup.
use crate::model::Emission;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Mutex,
};

pub struct Db(Mutex<Connection>);

#[derive(Default)]
pub struct Reception {
    pub quiet: std::collections::HashSet<String>,
    pub strength_db: std::collections::HashMap<String, f64>,
}

/// Observation query. Time is a half-open overlap test on [start_ms, end_ms).
#[derive(Clone, Debug)]
pub struct Query {
    pub mode: String,
    pub fault: bool,
    pub low_hz: f64,
    pub high_hz: f64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub limit: usize,
}

pub fn millis(rfc3339: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|t| t.timestamp_millis())
}

fn profile(e: &Emission) -> Option<&str> {
    e.reference.as_ref().map(|r| r.profile.as_str())
}

/// Upgrade a historical reported position from its recorded receiver pairs. The
/// archive lacks covariance. Never invent it from a radius or an evaluation reference.
fn resolve_archived_fix(e: &mut Emission) -> bool {
    if e.mode != "live" || e.fault_injected || !e.detected {
        return false;
    }
    if !e
        .estimate
        .iter()
        .chain(&e.hypotheses)
        .any(|h| h.covariance_km2.is_none())
    {
        return false;
    }
    let used = e
        .receivers
        .iter()
        .filter(|r| r.used && r.synchronized && r.confirmed)
        .count();
    e.estimate = None;
    e.hypotheses.clear();
    e.status = "undetermined".into();
    let result = crate::geo::locate(&e.receivers, &e.pairs);
    match result {
        Ok(mut hypotheses) if hypotheses.len() == 1 && used >= 4 => {
            e.estimate = hypotheses.pop();
            e.status = "located".into();
            e.reason = None;
        }
        Ok(hypotheses) if used >= 3 => {
            e.hypotheses = hypotheses;
            e.reason = Some(
                "Archived pair delays yield no unique position with the current solver.".into(),
            );
        }
        Err(err) => e.reason = Some(err.to_string()),
        _ => e.reason = Some("Insufficient synchronized receiver evidence.".into()),
    }
    true
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS emissions (
                id TEXT PRIMARY KEY,
                dataset_id TEXT NOT NULL,
                mode TEXT NOT NULL,
                fault INTEGER NOT NULL,
                start_ms INTEGER NOT NULL,
                end_ms INTEGER NOT NULL,
                frequency_hz REAL NOT NULL,
                status TEXT NOT NULL,
                detected INTEGER NOT NULL,
                profile TEXT,
                callsign TEXT,
                body TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS emissions_time ON emissions(mode, fault, start_ms);
             CREATE INDEX IF NOT EXISTS emissions_dataset ON emissions(dataset_id);
             CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                started_ms INTEGER,
                updated_ms INTEGER,
                status TEXT,
                monitor_target TEXT,
                body TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS jobs_started ON jobs(started_ms);
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS receiver_health (
                id TEXT PRIMARY KEY,
                ok INTEGER NOT NULL DEFAULT 0,
                failed INTEGER NOT NULL DEFAULT 0,
                fails_in_row INTEGER NOT NULL DEFAULT 0,
                last_ok_ms INTEGER,
                last_fail_ms INTEGER,
                last_reason TEXT,
                blocked_until_ms INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS receiver_usage (
                id TEXT NOT NULL,
                start_ms INTEGER NOT NULL,
                seconds REAL NOT NULL);
             CREATE INDEX IF NOT EXISTS receiver_usage_id ON receiver_usage(id, start_ms);
             CREATE TABLE IF NOT EXISTS receiver_decodes (
                id TEXT NOT NULL,
                band TEXT NOT NULL,
                at_ms INTEGER NOT NULL,
                decodes INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS receiver_decodes_id ON receiver_decodes(band, at_ms);
             CREATE TABLE IF NOT EXISTS receiver_listens (
                dataset TEXT NOT NULL, id TEXT NOT NULL, band TEXT NOT NULL, at_ms INTEGER NOT NULL,
                heard INTEGER NOT NULL, snr_db REAL, PRIMARY KEY(dataset,id));
             CREATE INDEX IF NOT EXISTS receiver_listens_band ON receiver_listens(band,at_ms);
             CREATE TABLE IF NOT EXISTS tracks (
                key TEXT PRIMARY KEY,
                last_ms INTEGER NOT NULL,
                band TEXT,
                body TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS tracks_last ON tracks(last_ms);
             CREATE TABLE IF NOT EXISTS track_inputs (
                id TEXT PRIMARY KEY REFERENCES emissions(id) ON DELETE CASCADE, key TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS track_inputs_key ON track_inputs(key);
             CREATE TABLE IF NOT EXISTS fix_publications (id TEXT PRIMARY KEY, at_ms INTEGER NOT NULL);",
        )?;
        let db = Db(Mutex::new(conn));
        db.migrate_tracks()?;
        Ok(db)
    }

    fn insert(tx: &rusqlite::Transaction, e: &Emission) -> Result<()> {
        let start = millis(&e.start_utc).context("Emission start is not RFC 3339")?;
        let end = millis(&e.end_utc).context("Emission end is not RFC 3339")?;
        tx.execute(
            "INSERT OR REPLACE INTO emissions
             (id,dataset_id,mode,fault,start_ms,end_ms,frequency_hz,status,detected,profile,callsign,body)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                e.id,
                e.dataset_id,
                e.mode,
                e.fault_injected,
                start,
                end,
                e.frequency_hz,
                e.status,
                e.detected,
                profile(e),
                e.identity.as_ref().map(|i| i.callsign.as_str()),
                serde_json::to_string(e)?
            ],
        )?;
        Ok(())
    }

    pub fn put_emissions(&self, es: &[Emission]) -> Result<()> {
        self.write_observations(None, es, true).map(|_| ())
    }

    /// Atomically replace every observation of one capture (a multi-slot bundle is never half-published).
    pub fn replace_dataset(&self, dataset_id: &str, es: &[Emission]) -> Result<()> {
        self.write_observations(Some(dataset_id), es, true)
            .map(|_| ())
    }

    pub fn import_observations(&self, mut es: Vec<Emission>) -> Result<usize> {
        for e in &mut es {
            resolve_archived_fix(e);
        }
        self.write_observations(None, &es, false)
    }

    fn write_observations(
        &self,
        dataset: Option<&str>,
        es: &[Emission],
        publish: bool,
    ) -> Result<usize> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let mut written = 0;
        let mut rebuild = BTreeSet::new();
        if let Some(id) = dataset {
            let mut old = tx.prepare_cached("SELECT DISTINCT key FROM track_inputs JOIN emissions USING(id) WHERE dataset_id=?1")?;
            for key in old.query_map([id], |row| row.get::<_, String>(0))? {
                rebuild.insert(key?);
            }
            tx.execute("DELETE FROM emissions WHERE dataset_id=?1", [id])?;
        }
        let mut append = BTreeMap::<String, Vec<&Emission>>::new();
        let now = Utc::now().timestamp_millis();
        for e in es {
            if !publish
                && tx
                    .prepare_cached("SELECT 1 FROM emissions WHERE id=?1")?
                    .exists([&e.id])?
            {
                continue;
            }
            written += 1;
            // An observation may be replaced independently of its original capture.
            if let Some(key) = tx
                .query_row("SELECT key FROM track_inputs WHERE id=?1", [&e.id], |r| {
                    r.get(0)
                })
                .optional()?
            {
                rebuild.insert(key);
            }
            Self::insert(&tx, e)?;
            if let Some(key) = Self::index_track(&tx, e)? {
                append.entry(key).or_default().push(e);
            }
            if publish && e.mode == "live" && !e.fault_injected && e.estimate.is_some() {
                tx.execute(
                    "INSERT OR IGNORE INTO fix_publications VALUES (?1,?2)",
                    params![e.id, now],
                )?;
            }
        }
        for (key, mut batch) in append {
            if rebuild.contains(&key) {
                continue;
            }
            batch.sort_by(|a, b| a.start_utc.cmp(&b.start_utc).then(a.id.cmp(&b.id)));
            let body: Option<String> = tx
                .query_row("SELECT body FROM tracks WHERE key=?1", [&key], |row| {
                    row.get(0)
                })
                .optional()?;
            let mut track: Option<crate::tracks::Track> =
                body.map(|b| serde_json::from_str(&b)).transpose()?;
            if track
                .as_ref()
                .is_some_and(|t| batch[0].start_utc <= t.last_utc)
            {
                rebuild.insert(key);
                continue;
            }
            for e in batch {
                crate::tracks::update(&mut track, e);
            }
            Self::save_track(&tx, &key, track)?;
        }
        for key in rebuild {
            Self::rebuild_track(&tx, &key)?;
        }
        if dataset.is_some() {
            tx.execute("DELETE FROM fix_publications WHERE NOT EXISTS (SELECT 1 FROM emissions e WHERE e.id=fix_publications.id)", [])?;
        }
        if dataset.is_some()
            || (written > 0 && es.iter().any(|e| e.mode == "live" && !e.fault_injected))
        {
            tx.execute("INSERT INTO meta VALUES ('live_revision',1) ON CONFLICT(key) DO UPDATE SET value=value+1", [])?;
        }
        tx.commit()?;
        Ok(written)
    }

    pub fn emission(&self, id: &str) -> Result<Option<Emission>> {
        let conn = self.0.lock().unwrap();
        let body: Option<String> = conn
            .query_row("SELECT body FROM emissions WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        body.map(|b| serde_json::from_str(&b).map_err(Into::into))
            .transpose()
    }

    /// Newest first.
    pub fn emissions(&self, q: &Query) -> Result<Vec<Emission>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT body FROM emissions
             WHERE mode=?1 AND fault=?2 AND frequency_hz>=?3 AND frequency_hz<?4
               AND start_ms<?6 AND end_ms>?5
             ORDER BY start_ms DESC, id ASC LIMIT ?7",
        )?;
        let rows = stmt.query_map(
            params![
                q.mode,
                q.fault,
                q.low_hz,
                q.high_hz,
                q.start_ms,
                q.end_ms,
                q.limit as i64
            ],
            |r| r.get::<_, String>(0),
        )?;
        rows.map(|b| Ok(serde_json::from_str(&b?)?)).collect()
    }

    pub fn track(&self, key: &str) -> Result<Option<crate::tracks::Track>> {
        let conn = self.0.lock().unwrap();
        let body: Option<String> = conn
            .query_row("SELECT body FROM tracks WHERE key=?1", [key], |r| r.get(0))
            .optional()?;
        body.map(|b| serde_json::from_str(&b).map_err(Into::into))
            .transpose()
    }

    /// Tracks updated since `since_ms`, most recent first.
    pub fn tracks_in_band(
        &self,
        since_ms: i64,
        low: f64,
        high: f64,
    ) -> Result<Vec<crate::tracks::Track>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT body FROM tracks WHERE last_ms>=?1 AND json_extract(body,'$.frequency_hz')>=?2
            AND json_extract(body,'$.frequency_hz')<?3 ORDER BY last_ms DESC,key",
        )?;
        let rows = stmt.query_map(params![since_ms, low, high], |r| r.get::<_, String>(0))?;
        rows.map(|b| Ok(serde_json::from_str(&b?)?)).collect()
    }

    fn index_track(tx: &rusqlite::Transaction, e: &Emission) -> Result<Option<String>> {
        if e.mode != "live"
            || e.fault_injected
            || !e.detected
            || !e
                .estimate
                .iter()
                .chain(&e.hypotheses)
                .any(|h| h.covariance_km2.is_some())
        {
            return Ok(None);
        }
        let key = crate::tracks::key(e);
        tx.execute(
            "INSERT INTO track_inputs(id,key) VALUES (?1,?2)",
            params![e.id, key],
        )?;
        Ok(Some(key))
    }

    fn save_track(
        tx: &rusqlite::Transaction,
        key: &str,
        track: Option<crate::tracks::Track>,
    ) -> Result<()> {
        if let Some(t) = track {
            tx.execute(
                "INSERT OR REPLACE INTO tracks(key,last_ms,band,body) VALUES (?1,?2,?3,?4)",
                params![key, millis(&t.last_utc), t.band, serde_json::to_string(&t)?],
            )?;
        } else {
            tx.execute("DELETE FROM tracks WHERE key=?1", [key])?;
        }
        Ok(())
    }

    fn rebuild_track(tx: &rusqlite::Transaction, key: &str) -> Result<()> {
        let mut stmt = tx.prepare_cached(
            "SELECT body FROM track_inputs JOIN emissions USING(id)
            WHERE key=?1 ORDER BY start_ms,id",
        )?;
        let mut track = None;
        for body in stmt.query_map([key], |r| r.get::<_, String>(0))? {
            crate::tracks::update(&mut track, &serde_json::from_str(&body?)?);
        }
        Self::save_track(tx, key, track)
    }

    fn migrate_tracks(&self) -> Result<()> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if tx
            .prepare("SELECT 1 FROM meta WHERE key='track_inputs_v2'")?
            .exists([])?
        {
            return Ok(());
        }
        let ids = tx.prepare("SELECT id FROM emissions WHERE mode='live' AND fault=0 AND detected=1
                AND (json_type(body,'$.estimate')='object' OR json_array_length(body,'$.hypotheses')>0)")?
            .query_map([],|r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        tx.execute("DELETE FROM track_inputs", [])?;
        tx.execute("DELETE FROM tracks", [])?;
        let mut keys = BTreeSet::new();
        for id in ids {
            let body: String =
                tx.query_row("SELECT body FROM emissions WHERE id=?1", [id], |r| r.get(0))?;
            let mut e: Emission = serde_json::from_str(&body)?;
            if resolve_archived_fix(&mut e) {
                Self::insert(&tx, &e)?;
            }
            if let Some(key) = Self::index_track(&tx, &e)? {
                keys.insert(key);
            }
        }
        for key in keys {
            Self::rebuild_track(&tx, &key)?;
        }
        tx.execute("INSERT INTO meta VALUES ('track_inputs_v2','1')", [])?;
        Ok(tx.commit()?)
    }

    /// Stream a full UTC interval to the statistics calculation without loading its IQ evidence
    /// into one large vector or silently dropping older observations at a result-count limit.
    pub fn visit_live(
        &self,
        start_ms: i64,
        end_ms: i64,
        mut visit: impl FnMut(Emission, Option<i64>),
    ) -> Result<()> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT body, at_ms FROM emissions LEFT JOIN fix_publications USING(id)
            WHERE mode='live' AND fault=0 AND start_ms>=?1 AND start_ms<?2",
        )?;
        let mut rows = stmt.query([start_ms, end_ms])?;
        while let Some(row) = rows.next()? {
            visit(
                serde_json::from_str(&row.get::<_, String>(0)?)?,
                row.get(1)?,
            );
        }
        Ok(())
    }

    pub fn has_dataset(&self, dataset_id: &str) -> Result<bool> {
        let conn = self.0.lock().unwrap();
        Ok(conn
            .prepare_cached("SELECT 1 FROM emissions WHERE dataset_id=?1 LIMIT 1")?
            .exists([dataset_id])?)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        let conn = self.0.lock().unwrap();
        Ok(conn
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.0.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO meta (key,value) VALUES (?1,?2)",
            [key, value],
        )?;
        Ok(())
    }

    /// Change marker for push notifications: (newest live start, live rows, newest track update).
    pub fn change_marker(&self) -> Result<(Option<i64>, i64, Option<i64>, i64)> {
        let conn = self.0.lock().unwrap();
        let (a, n): (Option<i64>, i64) = conn.query_row(
            "SELECT MAX(start_ms), COUNT(*) FROM emissions WHERE mode='live' AND fault=0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let t: Option<i64> = conn.query_row("SELECT MAX(last_ms) FROM tracks", [], |r| r.get(0))?;
        let revision = conn.query_row("SELECT COALESCE((SELECT CAST(value AS INTEGER) FROM meta WHERE key='live_revision'),0)", [], |r| r.get(0))?;
        Ok((a, n, t, revision))
    }

    pub fn latest_live_start(&self) -> Result<Option<i64>> {
        let conn = self.0.lock().unwrap();
        Ok(conn.query_row(
            "SELECT MAX(start_ms) FROM emissions WHERE mode='live' AND fault=0",
            [],
            |r| r.get(0),
        )?)
    }

    /// Record one receiver's capture outcome. Failures back off exponentially from 10 min
    /// (capped at 24 h); an explicit refusal can impose a longer block (e.g. a per-IP time limit).
    pub fn record_receiver(&self, o: &crate::capture::ReceiverOutcome, now_ms: i64) -> Result<()> {
        let conn = self.0.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO receiver_health (id) VALUES (?1)",
            [&o.id],
        )?;
        if o.ok {
            conn.execute(
                "UPDATE receiver_health SET ok=ok+1, fails_in_row=0, last_ok_ms=?2 WHERE id=?1",
                params![o.id, now_ms],
            )?;
        } else {
            let n: i64 = conn.query_row(
                "SELECT fails_in_row FROM receiver_health WHERE id=?1",
                [&o.id],
                |r| r.get(0),
            )?;
            let backoff_ms = (600_000i64 << n.min(8)).min(86_400_000);
            let block = backoff_ms.max((o.block_s * 1000.) as i64);
            conn.execute(
                "UPDATE receiver_health SET failed=failed+1, fails_in_row=fails_in_row+1, last_fail_ms=?2,
                 last_reason=?3, blocked_until_ms=MAX(blocked_until_ms, ?4) WHERE id=?1",
                params![o.id, now_ms, o.reason, now_ms + block],
            )?;
        }
        if o.connected_s > 0. {
            conn.execute(
                "INSERT INTO receiver_usage (id,start_ms,seconds) VALUES (?1,?2,?3)",
                params![o.id, now_ms, o.connected_s],
            )?;
        }
        Ok(())
    }

    /// Receivers that must not be contacted now: blocked, within the minimum gap since their
    /// last connection, or out of their daily connection-time budget for `planned_s` more.
    pub fn unusable_receivers(
        &self,
        now_ms: i64,
        policy: &crate::model::ReceiverPolicy,
        planned_s: f64,
    ) -> Result<std::collections::HashSet<String>> {
        let conn = self.0.lock().unwrap();
        let mut out = std::collections::HashSet::new();
        let mut stmt = conn.prepare("SELECT id FROM receiver_health WHERE blocked_until_ms>?1")?;
        for id in stmt.query_map([now_ms], |r| r.get::<_, String>(0))? {
            out.insert(id?);
        }
        let mut stmt = conn.prepare(
            "SELECT id, SUM(seconds), MAX(start_ms) FROM receiver_usage WHERE start_ms>?1 GROUP BY id",
        )?;
        let rows = stmt.query_map([now_ms - 86_400_000], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (id, used, last) = row?;
            if used + planned_s > policy.daily_minutes * 60.
                || now_ms - last < policy.min_gap_s as i64 * 1000
            {
                out.insert(id);
            }
        }
        Ok(out)
    }

    /// Record every receiver contacted by the capture in `dir` (fair-use accounting), and
    /// every KiwiSDR relay in between: a relay that refused us blocks all receivers behind it.
    pub fn account(&self, dir: &Path, duration_s: u64) {
        let now = chrono::Utc::now().timestamp_millis();
        let receivers = crate::capture::outcomes(dir, duration_s);
        let relays = crate::capture::relay_outcomes(&receivers);
        for o in receivers.iter().chain(&relays) {
            if let Err(e) = self.record_receiver(o, now) {
                eprintln!("Receiver accounting failed for {}: {e}", o.id);
            }
        }
    }

    /// One independent listen per capture, dated by its IQ even when reprocessed later.
    pub fn record_listen(
        &self,
        m: &crate::model::Manifest,
        counts: &[(String, usize)],
        strength: &std::collections::HashMap<&str, f64>,
    ) -> Result<()> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM receiver_listens WHERE dataset=?1", [&m.id])?;
        for (id, n) in counts {
            tx.execute(
                "INSERT INTO receiver_listens VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    m.id,
                    id,
                    format!("{:.0}", m.frequency_hz),
                    millis(&m.capture_utc).context("Invalid capture UTC")?,
                    *n as i64,
                    strength.get(id.as_str())
                ],
            )?;
        }
        Ok(tx.commit()?)
    }

    /// Receivers that heard nothing on channel `band` in at least two captures of the last six hours.
    pub fn reception(&self, band: &str, now_ms: i64) -> Result<Reception> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "WITH recent AS (SELECT id, heard, snr_db FROM receiver_listens WHERE band=?1 AND at_ms BETWEEN ?2 AND ?3),
             all_listens AS (SELECT * FROM recent UNION ALL SELECT id,decodes,NULL FROM receiver_decodes
             WHERE band=?1 AND at_ms BETWEEN ?2 AND ?3 AND id NOT IN (SELECT id FROM recent))
             SELECT id, COUNT(*)>=2 AND SUM(heard)=0, AVG(snr_db) FROM all_listens GROUP BY id",
        )?;
        let mut rows = stmt.query(params![band, now_ms - 6 * 3_600_000, now_ms])?;
        let mut result = Reception::default();
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            if r.get::<_, bool>(1)? {
                result.quiet.insert(id);
            } else if let Some(db) = r.get(2)? {
                result.strength_db.insert(id, db);
            }
        }
        Ok(result)
    }

    pub fn receiver_health(&self) -> Result<Vec<Value>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT h.id, h.ok, h.failed, h.fails_in_row, h.last_ok_ms, h.last_fail_ms, h.last_reason, h.blocked_until_ms,
                    (SELECT COALESCE(SUM(seconds),0) FROM receiver_usage u WHERE u.id=h.id AND u.start_ms>?1)
             FROM receiver_health h ORDER BY h.id",
        )?;
        let since = chrono::Utc::now().timestamp_millis() - 86_400_000;
        let rows = stmt.query_map([since], |r| {
            Ok(serde_json::json!({"id":r.get::<_,String>(0)?,"ok":r.get::<_,i64>(1)?,"failed":r.get::<_,i64>(2)?,
                "fails_in_row":r.get::<_,i64>(3)?,"last_ok_ms":r.get::<_,Option<i64>>(4)?,"last_fail_ms":r.get::<_,Option<i64>>(5)?,
                "last_reason":r.get::<_,Option<String>>(6)?,"blocked_until_ms":r.get::<_,i64>(7)?,"seconds_24h":r.get::<_,f64>(8)?}))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn put_job(&self, id: &str, job: &Value) -> Result<()> {
        let conn = self.0.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO jobs (id,started_ms,updated_ms,status,monitor_target,body)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                id,
                job["started_at"].as_str().and_then(millis),
                job["updated_at"].as_str().and_then(millis),
                job["status"].as_str(),
                job["monitor_target"].as_str(),
                serde_json::to_string(job)?
            ],
        )?;
        Ok(())
    }

    pub fn job(&self, id: &str) -> Result<Option<Value>> {
        let conn = self.0.lock().unwrap();
        let body: Option<String> = conn
            .query_row("SELECT body FROM jobs WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        body.map(|b| serde_json::from_str(&b).map_err(Into::into))
            .transpose()
    }

    /// Jobs started in [start_ms, end_ms), newest first.
    pub fn jobs_between(&self, start_ms: i64, end_ms: i64) -> Result<Vec<Value>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT body FROM jobs WHERE started_ms>=?1 AND started_ms<?2 ORDER BY started_ms DESC",
        )?;
        let rows = stmt.query_map([start_ms, end_ms], |r| r.get::<_, String>(0))?;
        rows.map(|b| Ok(serde_json::from_str(&b?)?)).collect()
    }

    /// Latest scheduled job and the number of scheduled jobs per monitor target.
    pub fn monitor_history(&self) -> Result<(Option<(String, String)>, Vec<(String, usize)>)> {
        let conn = self.0.lock().unwrap();
        let latest = conn
            .query_row(
                "SELECT monitor_target, id FROM jobs WHERE monitor_target IS NOT NULL
                 ORDER BY started_ms DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let mut stmt = conn.prepare(
            "SELECT monitor_target, COUNT(*) FROM jobs WHERE monitor_target IS NOT NULL GROUP BY monitor_target",
        )?;
        let counts = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((latest, counts))
    }

    /// Unfinished jobs from a previous process become `interrupted`; there is no automatic retry.
    pub fn interrupt_running(&self) -> Result<usize> {
        let ids: Vec<(String, String)> = {
            let conn = self.0.lock().unwrap();
            let mut stmt = conn.prepare("SELECT id, body FROM jobs WHERE status='running'")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (id, body) in &ids {
            let mut v: Value = serde_json::from_str(body)?;
            v["status"] = "interrupted".into();
            v["error"] =
                "The process stopped before capture/processing finished. No automatic retry."
                    .into();
            v["monitoring"] = "unmonitored".into();
            v["updated_at"] = Utc::now().to_rfc3339().into();
            self.put_job(id, &v)?;
        }
        Ok(ids.len())
    }

    /// Delete expired rows. Negative results (nothing detected in the sampled channel) expire sooner.
    pub fn prune(&self, policy: &crate::retention::Policy, now_ms: i64) -> Result<(usize, usize)> {
        let day = 86_400_000f64;
        let mut guard = self.0.lock().unwrap();
        let conn = guard.transaction()?;
        let mut removed = 0;
        if policy.result_days > 0. {
            let cutoff = now_ms - (policy.result_days * day) as i64;
            let keys: Vec<String> = conn.prepare("SELECT DISTINCT key FROM track_inputs JOIN emissions USING(id) WHERE mode='live' AND end_ms<?1")?
                .query_map([cutoff], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            removed += conn.execute(
                "DELETE FROM emissions WHERE mode='live' AND end_ms<?1",
                [cutoff],
            )?;
            for key in keys {
                Self::rebuild_track(&conn, &key)?;
            }
        }
        if policy.negative_result_days > 0. {
            removed += conn.execute(
                "DELETE FROM emissions WHERE mode='live' AND detected=0 AND end_ms<?1",
                [now_ms - (policy.negative_result_days * day) as i64],
            )?;
        }
        conn.execute(
            "DELETE FROM receiver_usage WHERE start_ms<?1",
            [now_ms - 2 * 86_400_000],
        )?;
        for table in ["receiver_decodes", "receiver_listens"] {
            conn.execute(
                &format!("DELETE FROM {table} WHERE at_ms<?1"),
                [now_ms - 6 * 3_600_000],
            )?;
        }
        if removed > 0 {
            conn.execute(
                "DELETE FROM fix_publications WHERE id NOT IN (SELECT id FROM emissions)",
                [],
            )?;
        }
        let jobs = if policy.job_days > 0. {
            conn.execute(
                "DELETE FROM jobs WHERE status!='running' AND COALESCE(updated_ms, started_ms, 0)<?1",
                [now_ms - (policy.job_days * day) as i64],
            )?
        } else {
            0
        };
        conn.commit()?;
        Ok((removed, jobs))
    }

    /// One-time import of the former JSON archive (`data/results`, `data/jobs`). Idempotent.
    pub fn import_legacy(&self, results: &Path, jobs: &Path) -> Result<(usize, usize)> {
        let mut emissions = vec![];
        if let Ok(dir) = std::fs::read_dir(results) {
            for entry in dir.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|s| s != "json") {
                    continue;
                }
                let bytes = std::fs::read(&path)?;
                if path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|n| n.ends_with("-slots.json"))
                {
                    emissions.extend(serde_json::from_slice::<Vec<Emission>>(&bytes)?);
                } else {
                    emissions.push(serde_json::from_slice::<Emission>(&bytes)?);
                }
            }
        }
        let imported = self.import_observations(emissions)?;
        let mut imported_jobs = 0;
        if let Ok(dir) = std::fs::read_dir(jobs) {
            for entry in dir.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|s| s != "json") {
                    continue;
                }
                let v: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
                let Some(id) = v["id"].as_str() else { continue };
                if self.job(id)?.is_none() {
                    self.put_job(id, &v)?;
                    imported_jobs += 1;
                }
            }
        }
        Ok((imported, imported_jobs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracks_survive_replacement_and_never_merge_anonymous_transmissions_by_frequency() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("data.sqlite")).unwrap();
        let mut a: Emission =
            serde_json::from_str(include_str!("../data/results/dcf77-20200813.json")).unwrap();
        a.id = "a".into();
        a.dataset_id = "window".into();
        a.mode = "live".into();
        a.detected = true;
        a.identity = None;
        a.reference = None;
        let mut fix = a.hypotheses.remove(0);
        fix.covariance_km2 = Some([5., 0., 5.]);
        a.estimate = Some(fix);
        a.hypotheses.clear();
        let mut b = a.clone();
        b.id = "b".into();
        db.replace_dataset("window", &[a.clone(), b.clone()])
            .unwrap();
        assert_eq!(db.tracks_in_band(0, 0., 1e6).unwrap().len(), 2);
        db.replace_dataset("window", std::slice::from_ref(&a))
            .unwrap();
        assert!(db.track("obs:b").unwrap().is_none());
        assert_eq!(db.track("obs:a").unwrap().unwrap().fixes, 1);
        assert_eq!(db.import_observations(vec![a.clone()]).unwrap(), 0);
        db.replace_dataset("window", &[]).unwrap();
        assert!(db.tracks_in_band(0, 0., 1e6).unwrap().is_empty());
        assert_eq!(db.import_observations(vec![a]).unwrap(), 1);
        db.visit_live(0, i64::MAX, |_, published| assert!(published.is_none()))
            .unwrap();
    }

    #[test]
    fn a_refusing_relay_blocks_every_receiver_behind_it_and_failed_sessions_use_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("m.sqlite")).unwrap();
        crate::capture::tests::capture_records(dir.path());
        db.account(dir.path(), 30);
        let now = chrono::Utc::now().timestamp_millis();
        let blocked = db
            .unusable_receivers(now, &crate::model::ReceiverPolicy::default(), 30.)
            .unwrap();
        assert!(blocked.contains("relay:proxy2.kiwisdr.com"));
        assert!(!blocked.contains("relay:proxy.kiwisdr.com"));
        for (url, behind) in [
            ("http://29999.proxy2.kiwisdr.com:8073/", true),
            ("http://29999.proxy.kiwisdr.com:8073/", false),
            ("http://198.51.100.20:8073/", false),
        ] {
            assert_eq!(
                crate::directory::behind_blocked_relay(&blocked, url),
                behind,
                "{url}"
            );
        }
        let used = |id: &str| {
            db.receiver_health()
                .unwrap()
                .into_iter()
                .find(|r| r["id"] == id)
                .unwrap()["seconds_24h"]
                .as_f64()
                .unwrap()
        };
        assert_eq!(used("f"), 50.0, "the timed-out session counts");
        assert_eq!(used("e"), 31.5);
        assert_eq!(used("a"), 0.0);
        // A later capture that reaches the relay again resets its back-off.
        let again = tempfile::tempdir().unwrap();
        std::fs::write(
            again.path().join("source-checks.json"),
            br#"[{"source":{"id":"a","base_url":"http://21001.proxy2.kiwisdr.com:8073/"},"observation":{}}]"#,
        )
        .unwrap();
        std::fs::write(
            again.path().join("capture-outcomes.json"),
            br#"[{"receiver":"a","captured":true,"connected_s":30.2}]"#,
        )
        .unwrap();
        db.account(again.path(), 30);
        let relay = db
            .receiver_health()
            .unwrap()
            .into_iter()
            .find(|r| r["id"] == "relay:proxy2.kiwisdr.com")
            .unwrap();
        assert_eq!(relay["fails_in_row"], 0);
        assert_eq!(relay["ok"], 1);
    }

    #[test]
    fn listening_twice_to_one_dataset_counts_as_one_receiver_observation() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("data.sqlite")).unwrap();
        let mut m: crate::model::Manifest =
            serde_json::from_str(include_str!("../data/recordings/manifest.json")).unwrap();
        let now = millis(&m.capture_utc).unwrap();
        let channel = format!("{:.0}", m.frequency_hz);
        let counts = vec![("weak".into(), 0), ("strong".into(), 3)];
        let snr = std::collections::HashMap::from([("strong", 12.5)]);
        for _ in 0..2 {
            db.record_listen(&m, &counts, &snr).unwrap();
        }
        let r = db.reception(&channel, now).unwrap();
        assert!(r.quiet.is_empty());
        assert_eq!(r.strength_db["strong"], 12.5);
        m.id.push_str("-next");
        db.record_listen(&m, &counts, &snr).unwrap();
        assert!(db.reception(&channel, now).unwrap().quiet.contains("weak"));
        assert!(
            db.reception(&channel, now + 7 * 3600000)
                .unwrap()
                .strength_db
                .is_empty()
        );
    }
    #[test]
    fn stores_queries_replaces_bundles_and_prunes_negative_results_first() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("m.sqlite")).unwrap();
        let mut e: Emission =
            serde_json::from_str(include_str!("../data/results/dcf77-20200813.json")).unwrap();
        db.put_emissions(std::slice::from_ref(&e)).unwrap();
        let q = Query {
            mode: "replay".into(),
            fault: false,
            low_hz: 0.,
            high_hz: 30e6,
            start_ms: millis("2020-08-13T06:52:00Z").unwrap(),
            end_ms: millis("2020-08-13T06:53:00Z").unwrap(),
            limit: 10,
        };
        assert_eq!(db.emissions(&q).unwrap().len(), 1);
        assert!(db.emission(&e.id).unwrap().is_some());
        assert!(
            db.emissions(&Query {
                mode: "live".into(),
                ..q.clone()
            })
            .unwrap()
            .is_empty()
        );

        // A live bundle is replaced atomically.
        e.mode = "live".into();
        let mut a = e.clone();
        a.id = "cap-1".into();
        a.dataset_id = "cap".into();
        let mut b = a.clone();
        b.id = "cap-2".into();
        b.detected = false;
        db.replace_dataset("cap", &[a.clone(), b.clone()]).unwrap();
        db.replace_dataset("cap", std::slice::from_ref(&a)).unwrap();
        assert!(db.emission("cap-2").unwrap().is_none());

        db.put_emissions(std::slice::from_ref(&b)).unwrap();
        let policy = crate::retention::Policy {
            result_days: 0.,
            negative_result_days: 1.,
            ..Default::default()
        };
        let later = millis(&a.end_utc).unwrap() + 2 * 86_400_000;
        let (removed, _) = db.prune(&policy, later).unwrap();
        assert_eq!(removed, 1);
        assert!(db.emission("cap-1").unwrap().is_some());
        assert!(db.emission("cap-2").unwrap().is_none());

        db.put_job(
            "j",
            &serde_json::json!({"id":"j","status":"running","started_at":"2020-01-01T00:00:00Z"}),
        )
        .unwrap();
        assert_eq!(db.interrupt_running().unwrap(), 1);
        assert_eq!(db.job("j").unwrap().unwrap()["status"], "interrupted");
    }
}
