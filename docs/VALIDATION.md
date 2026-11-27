# Validation with real IQ

## Simultaneous bands and live digital reports

On 27 November 2026 four receivers recorded the 24.930 MHz channel (12 m) and six others recorded the 28.200 MHz channel (10 m). All four on 12 m and five on 10 m had continuous GNSS timing for both ten-second slots beginning at **13:31:40 and 13:31:50 UTC**. The capture manifests `live-97bf006c-f7c8-4813-80cc-bbd2b6011722` and `live-0704c266-e8f7-462f-a557-451cd277deda` contain ten distinct physical receiver IDs. The two slots on each band yielded zero verified callsigns and zero TDoA fixes.

At **13:38:44 UTC** the digital feed had supplied 224,273 FT8 reports and 53,692 WSPR reports from 5,463 distinct reporting stations within the live UTC window. Nineteen named amateur bands had activity. The two-letter field **KN** (20–40° E, 40–50° N), which includes Romania and neighbouring countries, accounted for 15,170 FT8 reports and 759 WSPR reports. The digital layer uses transmitter locators from the feed. Its counts measure reception reports, which can include multiple reports of one transmission. [Feed documentation and attribution](https://www.mqtt.pskreporter.info/)

## Live HF observations

On 27 November 2026 the 20 m capture `live-061e8100-8de7-46ec-ab4e-6270d7cbc51e` identified **CS3B at 13:41:20 UTC** on four independent receivers. Two of the six receiver pairs passed the differential-delay checks. A second live capture (`live-c36b379d-4f59-4ba1-a3d8-e33e8eee0ab6`, **14:05:20 UTC**) again confirmed CS3B on four receivers. Zero of its six pairs passed. Both observations appear as heard with undetermined position. A separate 20 m capture `live-e914a0fb-606c-42c5-9693-83ee39fc380c` verified **ZL6B at 14:00:40 UTC** on one receiver and **VK6RBP at 14:00:50 UTC** on two. Each ten-second emission was evaluated independently.

On 27 November 2026 the native monitor acquired fresh Kiwi IQ on every International Beacon Project channel. The table covers **12:12:00–12:33:00 UTC**. Each sampled HF slot had at least one receiver with continuous GNSS timing across the complete ten seconds. Counts include one additional operator-requested capture on 17 m.

| Band | Frequency | Sampled slots | Verified slots | Verified callsign | Unique fixes | Measured error |
|---|---:|---:|---:|---|---:|---|
| 20 m | 14.100 MHz | 6 | 1 | VK6RBP | 0 | — |
| 17 m | 18.110 MHz | 8 | 0 | — | 0 | — |
| 15 m | 21.150 MHz | 7 | 1 | OA4B | 0 | — |
| 12 m | 24.930 MHz | 6 | 0 | — | 0 | — |
| 10 m | 28.200 MHz | 5 | 0 | — | 0 | — |

**VK6RBP** was verified at 12:12:50 UTC by a KiwiSDR in Marahau, New Zealand. The full-call score was 0.850, the next callsign scored 0.542 and the measured key-to-gap contrast was 16.5 dB. The receiver IQ SHA-256 is `c7b5f72a2cfa9b136cc01421de8267c333fa183125d12a459742241111a800a3`. **OA4B** was verified at 12:30:00 UTC by W2NAF in Spring Brook Township, Pennsylvania. Its IQ SHA-256 is `6cf93ea0857eca14f244b0bb976928fa58b7b4baed22012a353b2ffe406906bd`. Each transmission had one verified receiver. The native archive retains every capture manifest, per-slot evidence and original receiver file.

In a separate 20 m follow-up at **12:36:50 UTC**, receivers in Masterton, the Chatham Islands and Hastings independently verified VK6RBP. Seven receivers delivered IQ. The three pairwise keying-edge scatters were **3.16, 4.67 and 3.55 ms**, all above the 1.5 ms gate. This reception has three confirmed receivers and an undetermined position. At **12:37:00 UTC** the following slot produced **JA2IGY** at a receiver in Rotorua. Both observations are in `live-6938c532-b5ed-40ad-83dc-02c6efb990a4`. Receiver selection for the follow-up used prior reception and receiver positions.

At **12:41:40 UTC** a receiver in Ihrhove, Germany, verified **4X6TU on 10 m** in capture `live-6a6c7079-3f96-4cb8-a77c-e5ab746a547f`. At **12:44:20 UTC** another receiver verified **CS3B on 20 m**. By 12:51 UTC the service had verified five distinct amateur beacons across three HF bands in 56 analysed slots. Each of the five HF bands had delivered synchronized samples. The monitor was still acquiring data.

From **12:02 to 13:02 UTC**, 72 sampled HF slots yielded seven verified transmissions from six distinct beacons. Five transmissions were heard at one receiver each. Two receivers verified **ZL6B** on 20 m at **12:51:40 UTC** and supplied one accepted delay. In the VK6RBP window described above, three receivers verified the callsign and all three delay pairs exceeded the scatter limit. These seven transmissions yielded zero unique HF positions. During the same hour, three separate DCF77 windows yielded unique LF positions.

The IQ centre is 700 Hz below the published channel. For each synchronized receiver, Meridian compares all 18 complete Morse patterns over a ±150 Hz carrier search and checks the first power dash. Acceptance requires the scheduled callsign, correlation ≥0.72, a lead of ≥0.10 over the runner-up and key-to-gap contrast ≥5 dB. An unverified slot establishes that the channel was sampled. It provides no evidence for a transmitter marker. [IBP schedule](https://www.ncdxf.org/beacon/) · [Documented keying](https://www.ncdxf.org/beacon/beaconcontroller.html)

Timing between two verified receivers uses matched keying edges. A pair needs at least eight common edges covering at least half the call's edges, robust scatter ≤1.5 ms, waveform agreement and a physically compatible delay. The HF pair uncertainty floor is 1 ms. The global solver checks synchronized geometry and cycle closure. Ionospheric propagation can bias even a well-bounded spherical-model result. Repeated fixes from one beacon can dominate a band's statistics, so the interface also reports distinct beacons and sample size.

Each HF reference is the centre of the published six-character Maidenhead cell. The per-observation evaluation includes distance to that centre and bounds allowing for the cell's farthest corner. The band aggregates count unique solutions only. Ambiguous observations retain all compatible regions in their individual reports. [Published IBP locators](https://www.ncdxf.org/beacon/beaconlocations.html)

## Live LF observations

On 27 November 2026 the native server recorded DCF77 at 77.5 kHz with four independent KiwiSDRs. The following three consecutive scheduled windows passed phase-code verification and produced a unique TDoA solution. The distance to the published PTB position was calculated separately after processing.

| Common UTC interval | Contributing receivers | Distance to reference | Maximum model contour radius |
|---|---:|---:|---:|
| 11:28:50.043–11:29:17.306 | 4 | 4.90 km | 35.77 km |
| 11:31:50.045–11:32:17.287 | 4 | 6.14 km | 35.95 km |
| 11:34:49.907–11:35:17.198 | 4 | 4.59 km | 35.74 km |

The native IQ archive retains the source checks, WAVs and hash-linked RF evidence for these measurements. This series evaluates one LF channel. Other bands require their own synchronized observations and independent evaluation.

## Replay dataset and provenance

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

The independent evaluation reference is **50°01′03″ N, 09°00′34″ E**. The replay evaluation script and the live evaluation dialog use it after the solver finishes. The published rounding limits the precision of the comparison. [PTB, maßstäbe 1, page 23](https://www.ptb.de/cms/fileadmin/internet/publikationen/masstaebe/Hefte_Komplett_PDF/mst01.pdf)

**The three-receiver replay has an undetermined position.** The 1.79 km error belongs to one hypothesis. Selecting it because it is near DCF77 would contaminate the evaluation. Contours are conditional on the model and do not calibrate ionospheric propagation, dispersion, antenna offsets or cable delays.

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

1. Repeat simultaneous live IQ acquisition and RF verification on each public hosting instance.
2. Propagation-model scope and antenna/cable calibration sufficient for the intended operating conditions.
3. Actual hosting deployment, resource enforcement, backup restoration and restart acceptance.
4. Measure and repeat unique HF beacon fixes with simultaneous, independently verified receivers and consistent differential delays on each band.
