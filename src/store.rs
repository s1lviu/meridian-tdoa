//! Durable single-writer storage. Files and their directory entries are fsynced.
use anyhow::{Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Missing parent"))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn job(id: &str, value: &Value) -> Result<()> {
    ensure!(uuid::Uuid::parse_str(id).is_ok(), "Invalid job ID");
    atomic_json(&Path::new("data/jobs").join(format!("{id}.json")), value)
}

pub fn recover_jobs(path: &Path) -> Result<HashMap<String, Value>> {
    fs::create_dir_all(path)?;
    let mut jobs = HashMap::new();
    for entry in fs::read_dir(path)? {
        let p = entry?.path();
        if p.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let mut v: Value = serde_json::from_slice(&fs::read(&p)?)?;
        let id = v["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Job without ID"))?
            .to_owned();
        if v["status"] == "running" {
            v["status"] = json!("interrupted");
            v["error"] = json!(
                "The process stopped before capture/processing finished. No automatic retry."
            );
            v["monitoring"] = json!("unmonitored");
            atomic_json(&p, &v)?;
        }
        jobs.insert(id, v);
    }
    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_marks_inflight_job_interrupted_but_keeps_finished_results() {
        let d = tempfile::tempdir().unwrap();
        for (id, status) in [("a", "running"), ("b", "completed")] {
            atomic_json(
                &d.path().join(format!("{id}.json")),
                &json!({"id":id,"status":status}),
            )
            .unwrap();
        }
        let r = recover_jobs(d.path()).unwrap();
        assert_eq!(r["a"]["status"], "interrupted");
        assert_eq!(r["a"]["monitoring"], "unmonitored");
        assert_eq!(r["b"]["status"], "completed");
    }
}
