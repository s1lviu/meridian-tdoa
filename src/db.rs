//! Durable single-writer store for observations and jobs (SQLite, WAL, synchronous=FULL).
//! Replaces the per-file JSON archive that was loaded entirely into memory at startup.
use crate::model::Emission;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::{path::Path, sync::Mutex};

pub struct Db(Mutex<Connection>);

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
             CREATE INDEX IF NOT EXISTS receiver_usage_id ON receiver_usage(id, start_ms);",
        )?;
        Ok(Db(Mutex::new(conn)))
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
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        for e in es {
            Self::insert(&tx, e)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Atomically replace every observation of one capture (a multi-slot bundle is never half-published).
    pub fn replace_dataset(&self, dataset_id: &str, es: &[Emission]) -> Result<()> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM emissions WHERE dataset_id=?1", [dataset_id])?;
        for e in es {
            Self::insert(&tx, e)?;
        }
        tx.commit()?;
        Ok(())
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

    /// Record every receiver contacted by the capture in `dir` (fair-use accounting).
    pub fn account(&self, dir: &Path, duration_s: u64) {
        let now = chrono::Utc::now().timestamp_millis();
        for o in crate::capture::outcomes(dir, duration_s) {
            if let Err(e) = self.record_receiver(&o, now) {
                eprintln!("Receiver accounting failed for {}: {e}", o.id);
            }
        }
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
        let conn = self.0.lock().unwrap();
        let mut removed = 0;
        if policy.result_days > 0. {
            removed += conn.execute(
                "DELETE FROM emissions WHERE mode='live' AND end_ms<?1",
                [now_ms - (policy.result_days * day) as i64],
            )?;
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
        let jobs = if policy.job_days > 0. {
            conn.execute(
                "DELETE FROM jobs WHERE status!='running' AND COALESCE(updated_ms, started_ms, 0)<?1",
                [now_ms - (policy.job_days * day) as i64],
            )?
        } else {
            0
        };
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
        let fresh: Vec<_> = {
            let conn = self.0.lock().unwrap();
            let mut stmt = conn.prepare_cached("SELECT 1 FROM emissions WHERE id=?1")?;
            emissions
                .into_iter()
                .filter(|e| !stmt.exists([&e.id]).unwrap_or(true))
                .collect()
        };
        self.put_emissions(&fresh)?;
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
        Ok((fresh.len(), imported_jobs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
