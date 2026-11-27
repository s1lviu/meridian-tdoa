# Validation with real IQ

## Dataset and provenance

Source: [hcab14/TDoA](https://github.com/hcab14/TDoA/tree/2bb9dc2ecc2c6ebcc13ed11c7cbeadea0cd5dfcd), commit `2bb9dc2ecc2c6ebcc13ed11c7cbeadea0cd5dfcd`. The author explicitly associates these three files with DCF77 in `proc_tdoa_DCF77.m`. Original WAV bytes are not edited.

| Receiver | Published coordinates | SHA-256 |
|---|---|---|
| HB9ODP | 46.499351, 8.798828 | `87916f71b32a519d2abb55a85294112adfb4452397af0b0ccf12555ab3ac775d` |
| JO51xl | 51.466044, 11.977189 | `045c038e7bac3321abe9413d41a8c8d0fa3d60bc7a315a6cdb6a573bc484b702` |
| pa0rdt | 51.500500, 3.600690 | `90727bc9020b18088228709bbd3e77d0f876db0adcbf9503eb330e7fbc14fc8c` |

Recorded on **2020-08-13 at 77,500 Hz**. Common analysed interval: **06:52:21.801–06:52:50.168 UTC**. Effective sample rates derived from GNSS timestamps are approximately 12,001 Hz. GPS–UTC was 18 s at that date. A fixed rule excludes the first settling second, which contains invalid startup timing in the original files.

Receiver coordinates are published GNSS metadata, not independent antenna phase-centre surveys. The manifest's 1 km allowance is an operational assumption, not measured accuracy. Historical reception at 77.5 kHz does not establish today's bands, API permission or availability.

## Delay measurements

Normalized complex correlation; 14 accepted 2-second windows per pair. Sign convention: arrival B minus arrival A.

| Pair | Delay | Median correlation |
|---|---:|---:|
| HB9ODP → JO51xl | −427.875 µs | 0.97393 |
| HB9ODP → pa0rdt | +84.345 µs | 0.97903 |
| JO51xl → pa0rdt | +513.321 µs | 0.98320 |

The timing sigma floor is **83.333 µs**, one sample at the common 12 kHz rate. It is not a guaranteed experimental accuracy. Correlation coefficients are not probabilities.

## Location and measured error

Global search using spherical arc distance at c = 299,792.458 km/s produces two compatible minima:

| Hypothesis | Latitude | Longitude | Distance from PTB reference | Maximum contour radius |
|---|---:|---:|---:|---:|
| 1 | 50.012339° | 9.033148° | **1.7883 km** | 39.08 km |
| 2 | −49.156391° | −173.692373° | **19,798.1022 km** | 56.58 km |

The independent evaluation reference is **50°01′03″ N, 09°00′34″ E**, published by [PTB in maßstäbe 1, page 23](https://www.ptb.de/cms/fileadmin/internet/publikationen/masstaebe/Hefte_Komplett_PDF/mst01.pdf). It is used only by `scripts/evaluate.py`, after the solver finishes. Its rounding does not justify metre-level claims.

**Verdict: Position undetermined. No unique real-world localization has been validated.** The 1.79 km error belongs to one hypothesis. Selecting it because it is near DCF77 would contaminate the evaluation. Contours are conditional on the model and do not calibrate ionospheric propagation, dispersion, antenna offsets or cable delays.

Resolving ambiguity requires additional independent evidence, such as a fourth simultaneous receiver with useful geometry, or an operating domain/model justified independently of the known transmitter. No Europe-only domain was imposed to remove the distant hypothesis. A synthetic four-receiver test is explicitly a geometry test, not a real accuracy result.

## Synchronization loss

- `fault_receiver=pa0rdt` preserves the RF samples and invalidates that receiver's GNSS state in memory.
- RF reception remains correlated, but the receiver is excluded from localization.
- Only **2/3 receivers remain synchronized**: `estimate=null`, `hypotheses=[]`, `status=undetermined`.
- Rust tests separately corrupt a GNSS block in a temporary WAV copy (age 255), introduce a 1 ms timestamp jump and test truncation.
- This is software fault injection on real IQ, not an observed physical GNSS outage during live acquisition.

## Automated and browser evidence

The English interface and messages are checked using the existing real-data tests. Reports contain their actual execution timestamps:

- `evidence/rust-tests.txt`: 15 tests covering parsing, timing, access policy, provenance, geometry, real IQ, PRN verification and job recovery.
- `evidence/transport-tests.txt`: a local dummy server verifies that the KiwiClient adapter makes no second connection attempt; no RF is involved.
- `evidence/browser-tests.json`: Chromium checks for map intersection, receiver details outside the viewport, band/UTC filters, explicit ambiguity, GNSS loss, authentication, directory metadata, browser headers and a 390 px responsive layout.
- `evidence/desktop.png`, `evidence/mobile.png`, `evidence/sync-loss.png`: screenshots from the browser test.
- `evidence/validation.json`: separate post-hoc evaluation; `evidence/replay-result.json` and `evidence/sync-loss-result.json` are actual processing outputs.

Earlier container acceptance reports from **26 November 2026** are retained separately: `release-api-tests.json` records two authenticated release-mode real-IQ jobs; `deployment-checks.json` records persistence across container replacement and verifies UID 10001, a read-only root filesystem and readiness. It explicitly reports that requested cgroup limits were not enforced by that managed development environment. These timestamped reports are not a claim that a cloud provider has been deployed or tested.

Compose and Caddy configuration validation does not issue a TLS certificate. Render Blueprint schema validation does not create a Render instance. The PRN classifier passes the three real recordings and rejects synthetic carrier/noise cases; one historical dataset does not establish a universal false-positive rate.

## Open operational criteria

1. Successful simultaneous live IQ transport and RF verification from the deployment host; attempted development-environment captures failed.
2. Propagation-model scope and antenna/cable calibration sufficient for the intended operating conditions.
3. Actual hosting deployment, resource enforcement, backup restoration and restart acceptance.
4. A separate documented multireceiver amateur-radio beacon recording if validation specifically on an amateur beacon is required; the bundled reference is DCF77.
