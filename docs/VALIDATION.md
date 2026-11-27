# Method and validation

[Back to Meridian](../README.md)

Everything below was measured on real IQ recorded by public KiwiSDRs on **27 November 2026**, or on the bundled 2020 DCF77 recording, unless marked *synthetic*. Errors are post-hoc distances to references the solver never sees: the published DCF77 position (PTB), the published IBP beacon locators, or the locator an FT8 station sent in its own message. Synthetic tests check algorithms and geometry; they are not evidence of live accuracy.

## Summary

| Signal | Observations | Unique fixes | Error vs reference | 95% region contains it |
|---|---:|---:|---|---:|
| DCF77 77.5 kHz, four receivers | 32 windows | 32 | median **10.5 km**, P90 13.7 km, max 14.4 km | **32/32** |
| FT8 40 m, 5–12 receivers requested | 595 payload/slot observations from 16 captures, 78 timed at ≥4 receivers with a sent locator | **2** | **194 km**, **274 km** | 2/2 |
| IBP beacons, CW keying edges (retired method) | 769 slots | 0 | — | — |
| Anonymous monitored channels | 9 windows on LF, MF and HF | **2** on 648 kHz | no independent transmitter identity | — |

The scale follows physics. LF ground wave travels along the surface at a stable speed: kilometres. HF reaches distant receivers by one or more ionospheric hops whose extra path differs per receiver, and FT8 is only ~50 Hz wide: a timing error of about a millisecond, hundreds of kilometres per fix. A unique position also needs at least four receivers **around** the transmitter.

## DCF77 (LF ground wave)

Live captures from four receivers (57–261 km from Mainflingen), 11:28–17:31 UTC. Each receiver's IQ must carry the PTB phase code (normalized correlation ≥0.65 in ≥80% of 2 s windows). A capture used to be discarded when one receiver failed this; that happened in **46 of ~83** captures. The failing receiver is now withheld and the capture proceeds (regression test on the real 2020 IQ plus a noise receiver).

`meridian-tdoa calibrate` scores the stored delays at the PTB position:

| Quantity | Value |
|---|---:|
| Pair residual at the reference | rms 49 µs, robust σ 38 µs, median −19 µs |
| Reported pair σ (one-sample floor) | 83.3 µs (normalized rms 0.59: conservative) |
| Residual level by time of day | ≈60 µs 11:30–13:30, ≈27 µs 14:00–16:00, ≈55 µs after 17:00 |

The pairs between the three distant receivers carry biases of 40–100 µs and fail cycle closure by up to ~50 µs, while pairs involving the receiver nearest the transmitter are consistent. That is a delay-estimation effect (cross-correlating two differently distorted signals), not a weighting problem — and it decides the result:

| Pairs used | Median error | P90 |
|---|---:|---:|
| Only pairs with the nearest receiver (57 km) | 4.7 km | 8.5 km |
| Only pairs with one of the other receivers | 10.0–20.1 km | 12.2–24.0 km |
| **All pairs (current solver, no arbitrary choice)** | **10.5 km** | **13.7 km** |

The 4.9 km reported earlier came from the old solver using only the pairs of the alphabetically first receiver, which happened to be the nearest one. With all pairs, the 95% region contains the reference in 32/32 windows (median radius 50 km — conservative).

## FT8 (HF, decoded from our own IQ)

Sixteen stored captures on 40 m span 18:34–19:05 UTC. They contain 595 distinct payload/slot observations and 301 decoded transmitter identities. 467 observations were timed at two or more receivers. Up to 12 receivers were requested per capture. Every 15 s slot is decoded at every synchronized receiver (vendored ft8_lib), messages are associated by their 77-bit payload, and every receiver — including those too weak to decode, at ≥10σ — is timed against the waveform rebuilt from the payload.

- A first two-receiver test (Tremolat FR – Oberwölbling AT, 1150 km): 41 messages, 27 timed at both. Against the stations' sent locators the delay residuals have a **median of 945 µs**, while the jackknife σ has a median of **840 µs** — the uncertainty is honest. Wildly wrong delays (−138 ms, −12.9 ms) are rejected by the physical-limit test.
- 78 messages were timed at four or more receivers and carried a locator. **Two** gave a unique position under the 4000 km region cap: **KA9SOG** (EN52) **194 km** from its square's centre (95% region 854 km) and **NF3R** (FN20) **274 km** (1543 km), both located by mostly North-American receivers around them.
- Re-solving all 78 without the cap (`meridian-tdoa calibrate ft8`), 20 give a bounded region; their 95% regions contain the locator centre in **18/20**; their errors span 31 km to 14,000 km (median 2238 km), because most stations lay outside the small receiver groups — such geometry determines a direction, not a distance.

FT8 can be mapped where receivers surround the active stations; most of the time, with the public receivers available, it is "heard, position undetermined".

## Earlier HF method (retired)

From 12:12 to 17:32 UTC the previous monitor sampled 769 IBP beacon slots. It verified 45 (5.9%), mostly by one receiver; 37 delay pairs from CW keying edges, 3 within the 1.5 ms scatter gate (median scatter 2.9 ms ≈ 870 km); **zero** fixes. Keying edges of a ~50 Hz CW signal cannot time to better than milliseconds, and geography-only receiver clusters rarely heard the beacon. FT8's matched filter on the full waveform replaces it; IBP identification remains available.

## Anonymous emissions on LF, MF and HF

The correlated-channel profile examined every occupied sub-band. A correlated sub-band has at least one accepted inter-receiver delay. A unique fix requires a bounded single region supported by at least four receivers. “IQ” counts delivered recordings before timing and correlation checks. Capture identifiers, per-receiver IQ hashes, accepted delays and estimated centres are in the [field evidence](../evidence/live/rf-channel-validation.json).

| UTC start | Channel | IQ | Occupied | Correlated | Unique |
|---|---:|---:|---:|---:|---:|
| 19:40:58 | 77.5 kHz | 3 | 27 | 3 | 0 |
| 19:42:35 | 77.5 kHz | 5 | 7 | 3 | 0 |
| 19:44:14 | 60 kHz | 4 | 6 | 1 | 0 |
| 19:45:53 | 60 kHz | 3 | 10 | 0 | 0 |
| 19:47:31 | 60 kHz | 3 | 27 | 0 | 0 |
| 19:53:01 | 648 kHz | 5 | 22 | 5 | 1 |
| 19:53:43 | 5000 kHz | 4 | 9 | 0 | 0 |
| 20:19:34 | 648 kHz | 4 | 29 | 5 | 1 |
| 20:24:27 | 648 kHz | 4 | 32 | 4 | 0 |

At 19:53, four receivers constrained a 1.84 kHz wide sub-band near 648.006 kHz to a region centred at 51.90° N, 2.55° E with maximum 95% radius 438 km. A second four-receiver result at 20:19, acquired using the native Rust Kiwi client, was centred at 52.06° N, 1.08° E with maximum radius 970 km. The archived receiver coordinates, pair delays and timing uncertainties permit independent re-solving of both observations. Broadcast frequency alone does not establish which co-channel transmitter was heard or its location error. At 20:24 four recordings on the same channel produced four correlations and zero unique positions.

An isolated copy of the live database served both MF observations and both FT8 tracks to the map. The FT8 inspector displayed the locator calculated from the estimated position. Hardware-timestamped simulated reports also exercised delivery into an already-open browser and the session latency indicator. The open-tab 648 kHz trial at 20:24 yielded no position to time.

The 32 identified DCF77 fixes contained the PTB position in both the 68% ellipse and the 95% contour in 32/32 cases. The two FT8 fixes contained their respective **locator centres** in both regions. These samples give observed coverage on the recorded receiver geometries. The HF comparison has just two values, and the amateur locator is a grid square rather than an independently surveyed antenna.

A separate DCF77 commissioning attempt at 20:28 UTC delivered four IQ files. The automatic phase-code verification did not qualify this window, so it added no DCF77 position or field latency sample.

## Bundled recording (2020, three receivers)

hcab14/TDoA, 13 August 2020, DCF77 at HB9ODP, JO51xl and pa0rdt. Pair delays −427.89, +84.35 and +513.32 µs (sinc resampling; 2 s windows). Three receivers leave two mirror solutions: **50.017° N, 9.029° E (1.4 km from the PTB reference, 95% region 51 km)** and 49.13° S, 173.75° W. The result stays **undetermined**: choosing the hypothesis near DCF77 because it is near DCF77 would contaminate the evaluation. Invalidating one receiver's GNSS state in memory (`fault_receiver=pa0rdt`) leaves two synchronized receivers and no region.

## Synthetic verification (algorithms, not live accuracy)

| Check | Result |
|---|---|
| Solver Monte Carlo, five receivers, per-receiver and pair-specific noise | 95% region contains truth in 60/60 |
| Matched-filter timing, exact fractional delays in noise | rms error / jackknife σ = 1.04 (40 trials) |
| FT8 decoding of upstream WSJT-X recordings | 4/5 and 16/24 messages |
| FT8 TDoA, five Kiwi-format receivers with independent sample clocks | 0.64 km |
| SigMF, five 48 kSPS nodes, unknown 2 kHz emitter | 0.08 km |
| Edge nodes reporting FT8 arrivals, server association | 0.81 km |
| Five free-running clocks (±5 ms, ±40 ppm) synchronized from a reference emitter | 29 m; unsynchronized: no fix |
| Node detections through a real server process (`tests/detections.py`) | 33 m, track registered, push event received |

## Uncertainty model

Pair delays are reduced by least squares to n−1 independent differences whose covariance combines pair noise (scaled up when cycles do not close) with a per-receiver timing term; the position fit uses this generalized covariance on the WGS84 ellipsoid and is scaled up (never down) by its own χ²/(n−3). The 95% region is the Δχ² = 5.991 contour; a local east/north covariance feeds the emitter registry, which fuses fixes by covariance intersection so that errors shared between fixes of one station (ionosphere, antennas) are never assumed to average out.

## Not established

- Absolute accuracy for the anonymous 648 kHz emissions. Both have a measured region and no independently established source identity.
- Stable accuracy across propagation conditions, a unique broadband 5 MHz result, or FT8 accuracy on bands beyond 40 m.
- Physical GPSDO/PPS node performance and regional VHF/UHF accuracy. The SoapySDR library was tested with a software driver, and the regional server with simulated arrivals.
- Antenna and cable delays are not calibrated; receiver positions are operator-reported.
