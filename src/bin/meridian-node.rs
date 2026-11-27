//! Edge node agent: turns SigMF recordings from any SDR software into arrival-time reports.
//!
//!   meridian-node watch DIR --server URL --node ID --token-file FILE [--once]
//!
//! Every recording in DIR (`*.sigmf-meta` + `*.sigmf-data`, written by GNU Radio, UHD,
//! SDR++, rtl_sdr converters …) that covers an FT8 sub-band is decoded locally; each decoded
//! message is timed against its rebuilt waveform in the node's own clock and posted to
//! `/api/detections`. The optional SoapySDR input records timestamped IQ to the same format.
use anyhow::{Context, Result, ensure};
use meridian_tdoa::{ft8_tdoa, sigmf};
use std::{collections::HashSet, path::Path, time::Duration};

#[cfg(feature = "soapy")]
mod soapy;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "capture") {
        #[cfg(feature = "soapy")]
        {
            ensure!(
                args.len() == 4,
                "meridian-node capture CONFIG.json OUTPUT_DIR"
            );
            let meta = soapy::capture(Path::new(&args[2]), Path::new(&args[3]))?;
            println!("{}", meta.display());
            return Ok(());
        }
        #[cfg(not(feature = "soapy"))]
        anyhow::bail!("Build meridian-node with --features soapy for hardware capture");
    }
    ensure!(
        args.get(1).is_some_and(|a| a == "watch"),
        "meridian-node watch DIR --server URL --node ID --token-file FILE [--once], or capture CONFIG.json OUTPUT_DIR"
    );
    let dir = args.get(2).context("Missing DIR")?;
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let server = flag("--server").context("Missing --server URL")?;
    let node = flag("--node").context("Missing --node ID")?;
    let token = std::fs::read_to_string(flag("--token-file").context("Missing --token-file")?)?
        .trim()
        .to_owned();
    let once = args.iter().any(|a| a == "--once");
    let state = Path::new(dir).join(".meridian-node-done");
    let mut done: HashSet<String> = std::fs::read_to_string(&state)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    loop {
        let mut metas: Vec<_> = std::fs::read_dir(dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(".sigmf-meta"))
            .collect();
        metas.sort();
        let mut errors = 0;
        for meta in metas {
            let key = meta.file_name().unwrap().to_string_lossy().to_string();
            if done.contains(&key) || !sigmf::data_path(&meta).is_file() {
                continue;
            }
            match report(&client, &server, &node, &token, &meta) {
                Ok(n) => eprintln!("{key}: {n} detection(s) posted"),
                Err(e) => {
                    eprintln!("{key}: {e}");
                    errors += 1;
                    continue;
                }
            }
            done.insert(key.clone());
            let temporary = state.with_extension("tmp");
            std::fs::write(
                &temporary,
                done.iter().cloned().collect::<Vec<_>>().join("\n"),
            )?;
            std::fs::rename(temporary, &state)?;
        }
        if once {
            ensure!(errors == 0, "{errors} recording(s) pending retry");
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// Decode and time every fully covered FT8 slot of one recording; post the detections.
fn report(
    client: &reqwest::blocking::Client,
    server: &str,
    node: &str,
    token: &str,
    meta: &Path,
) -> Result<usize> {
    let (dial, detections) = ft8_tdoa::node_detections(meta, node)?;
    if !detections.is_empty() {
        client
            .post(format!("{}/api/detections", server.trim_end_matches('/')))
            .bearer_auth(token)
            .json(&serde_json::json!({"detections": detections, "frequency_hz": dial}))
            .send()?
            .error_for_status()?;
    }
    Ok(detections.len())
}
