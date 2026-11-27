use meridian_tdoa::{capture, geo, kiwi, model::*, pipeline};
use std::{fs, path::Path};
fn manifest() -> Manifest {
    serde_json::from_slice(&fs::read("data/recordings/manifest.json").unwrap()).unwrap()
}
#[test]
fn real_iq_retains_global_ambiguity_and_loss_of_sync_fails_closed() {
    let e = pipeline::process(Path::new("data/recordings/manifest.json"), None).unwrap();
    assert_eq!(e.status, "undetermined");
    assert!(e.estimate.is_none());
    assert_eq!(e.hypotheses.len(), 2);
    assert_eq!(
        e.receivers.iter().filter(|r| r.confirmed && r.used).count(),
        3
    );
    assert!(e.pairs.iter().all(|p| p.accepted && p.coherence > 0.9));
    let broken =
        pipeline::process(Path::new("data/recordings/manifest.json"), Some("pa0rdt")).unwrap();
    assert_eq!(broken.status, "undetermined");
    assert!(broken.estimate.is_none() && broken.hypotheses.is_empty());
    assert!(broken.receivers.iter().all(|r| !r.used));
    assert_eq!(
        broken.receivers.iter().filter(|r| r.synchronized).count(),
        2
    );
}
#[test]
fn refuses_duplicate_captures_devices_and_missing_receivers() {
    let mut m = manifest();
    m.receivers[1].sha256 = m.receivers[0].sha256.clone();
    assert!(pipeline::validate_manifest(&m).is_err());
    let mut m = manifest();
    m.receivers[1].independent_device = m.receivers[0].independent_device.clone();
    assert!(pipeline::validate_manifest(&m).is_err());
    let mut m = manifest();
    m.receivers.pop();
    assert!(pipeline::validate_manifest(&m).is_err());
}
#[test]
fn actual_wav_metadata_loss_and_time_jump_are_rejected() {
    let m = manifest();
    let original = fs::read(Path::new("data/recordings").join(&m.receivers[0].file)).unwrap();
    let mut aged = original.clone();
    let mut jumped = original.clone();
    let mut i = 12;
    let mut block = 0;
    while i + 8 < original.len() {
        let n = u32::from_le_bytes(original[i + 4..i + 8].try_into().unwrap()) as usize;
        if &original[i..i + 4] == b"kiwi" {
            block += 1;
            if block == 100 {
                aged[i + 8] = 255;
                let ns = u32::from_le_bytes(original[i + 14..i + 18].try_into().unwrap());
                jumped[i + 14..i + 18].copy_from_slice(&(ns + 1_000_000).to_le_bytes());
            }
        }
        i += 8 + n + (n % 2);
    }
    assert!(kiwi::parse(&aged).unwrap().sync_error.is_some());
    assert!(kiwi::parse(&jumped).unwrap().sync_error.is_some());
    assert!(kiwi::parse(&original[..original.len() - 2]).is_err());
}
#[test]
fn fourth_independent_receiver_can_remove_global_mirror_in_a_synthetic_geometry_test() {
    // Synthetic geometry unit test only; never displayed as a real recording.
    let target = Point {
        lat: 50.1,
        lon: 9.2,
    };
    let positions = [
        Point {
            lat: 46.5,
            lon: 8.8,
        },
        Point {
            lat: 51.46,
            lon: 11.97,
        },
        Point {
            lat: 51.5,
            lon: 3.6,
        },
        Point {
            lat: 54.2,
            lon: 17.4,
        },
    ];
    let rx: Vec<_> = positions
        .iter()
        .enumerate()
        .map(|(i, p)| RxResult {
            receiver: Receiver {
                id: i.to_string(),
                label: i.to_string(),
                independent_device: i.to_string(),
                position: *p,
                position_evidence: Evidence {
                    url: "https://example.invalid/synthetic".into(),
                    note: "Synthetic unit test only".into(),
                },
                position_uncertainty_m: 0.,
                file: String::new(),
                sha256: String::new(),
            },
            confirmed: true,
            used: true,
            synchronized: true,
            reason: None,
            gps_age_max_s: 0,
            sample_rate_hz: 12000.,
            signal_score_db: 20.,
            samples: 120000,
        })
        .collect();
    let mut pairs = vec![];
    for i in 0..4 {
        for j in i + 1..4 {
            pairs.push(Pair {
                a: i.to_string(),
                b: j.to_string(),
                delay_us: (geo::distance(target, positions[j])
                    - geo::distance(target, positions[i]))
                    / geo::C_KM_S
                    * 1e6,
                sigma_us: 20.,
                coherence: 1.,
                peak_ratio: 2.,
                windows: 10,
                accepted: true,
                correlation: vec![],
            });
        }
    }
    let solutions = geo::locate(&rx, &pairs).unwrap();
    assert_eq!(solutions.len(), 1);
    assert!(geo::distance(solutions[0].center, target) < 0.1);
}
#[test]
fn no_permissions_means_no_network_capture() {
    let c: Config = serde_json::from_slice(&fs::read("config/app.json").unwrap()).unwrap();
    let t = Target {
        verification: None,
        id: "test".into(),
        kind: "public_beacon".into(),
        frequency_hz: 77500.,
        eligibility_evidence: Evidence {
            url: "https://example.invalid".into(),
            note: "test".into(),
        },
        source_ids: vec![],
        duration_s: 30,
    };
    assert!(
        capture::authorize(&c, &t)
            .unwrap_err()
            .to_string()
            .contains("disabled")
    );
}

#[test]
fn too_short_real_iq_is_undetermined_without_claiming_no_emission() {
    let tmp = tempfile::tempdir().unwrap();
    let mut m = manifest();
    m.provenance
        .note
        .push_str(" Shortened copy for a local test; not a live capture.");
    for r in &mut m.receivers {
        let mut bytes = fs::read(Path::new("data/recordings").join(&r.file)).unwrap();
        let mut i = 12usize;
        let mut blocks = 0;
        while i + 8 <= bytes.len() {
            let len = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
            let data = &bytes[i..i + 4] == b"data";
            i += 8 + len + len % 2;
            if data {
                blocks += 1;
            }
            if blocks == 120 {
                break;
            }
        }
        bytes.truncate(i);
        let size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        let p = tmp.path().join(&r.file);
        fs::write(&p, bytes).unwrap();
        r.sha256 = pipeline::checksum(&p).unwrap();
    }
    let path = tmp.path().join("manifest.json");
    fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    let e = pipeline::process(&path, None).unwrap();
    assert_eq!(e.status, "undetermined");
    assert!(e.estimate.is_none() && e.hypotheses.is_empty());
    assert!(e.detected);
    assert!(e.reason.unwrap().contains("6 s"));
}

/// Replace every IQ sample of a real Kiwi WAV with deterministic noise, keeping its GNSS chunks.
fn noise_copy(original: &[u8]) -> Vec<u8> {
    let mut out = original.to_vec();
    let mut seed = 0x5eed_u64;
    let mut i = 12;
    while i + 8 <= original.len() {
        let n = u32::from_le_bytes(original[i + 4..i + 8].try_into().unwrap()) as usize;
        if &original[i..i + 4] == b"data" {
            for s in out[i + 8..i + 8 + n].chunks_exact_mut(2) {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                s.copy_from_slice(&(((seed >> 48) as i16) / 4).to_le_bytes());
            }
        }
        i += 8 + n + (n % 2);
    }
    out
}

#[test]
fn a_receiver_failing_the_phase_code_is_excluded_instead_of_discarding_the_capture() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = manifest();
    for r in &m.receivers {
        fs::copy(
            Path::new("data/recordings").join(&r.file),
            dir.path().join(&r.file),
        )
        .unwrap();
    }
    let noisy =
        noise_copy(&fs::read(Path::new("data/recordings").join(&m.receivers[0].file)).unwrap());
    fs::write(dir.path().join("noise.wav"), &noisy).unwrap();
    let mut extra = m.receivers[0].clone();
    extra.id = "noise-rx".into();
    extra.independent_device = "noise-rx".into();
    extra.file = "noise.wav".into();
    extra.position = Point {
        lat: 48.85,
        lon: 2.35,
    };
    extra.sha256 = pipeline::checksum(&dir.path().join("noise.wav")).unwrap();
    m.receivers.push(extra);
    m.emission_verified = false;
    m.identity = None;
    let path = dir.path().join("manifest.json");
    fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();

    let proof = meridian_tdoa::protocol::verify(&path).unwrap();
    assert_eq!(proof.iter().filter(|p| p.accepted).count(), 3);
    assert!(
        !proof
            .iter()
            .find(|p| p.receiver_id == "noise-rx")
            .unwrap()
            .accepted
    );
    let verified: Manifest = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(verified.emission_verified);
    assert_eq!(verified.receivers.len(), 3);
    assert_eq!(verified.rejected_receivers.len(), 1);
    assert_eq!(verified.rejected_receivers[0].receiver.id, "noise-rx");

    let e = pipeline::process(&path, None).unwrap();
    assert_eq!(e.excluded.len(), 1);
    assert!(e.receivers.iter().all(|r| r.receiver.id != "noise-rx"));
    assert_eq!(e.receivers.iter().filter(|r| r.used).count(), 3);
}
