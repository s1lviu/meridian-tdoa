# Own SDR nodes: SigMF recordings and arrival-time reports

[Back to Meridian](../README.md)

Public KiwiSDRs supply individual GNSS-timestamped channels. To map more transmitters — wider bands, VHF/UHF, your own region — add nodes you control. Meridian accepts two forms of input, depending on whether the node's clock is disciplined.

## The clock decides

| Node | Time of each sample | Use |
|---|---|---|
| GPSDO-equipped USRP, a recorder with independently verified sample-to-PPS alignment, or a KiwiSDR | Absolute sample time with a measured or declared uncertainty | SigMF recordings (`data/inbox/`) or arrival-time reports with `disciplined: true` |
| RTL-SDR, Airspy, SDRplay, HackRF without PPS | Free-running crystal (ppm drift, unknown offset) | Arrival-time reports; clocks synchronized from reference emitters |
| WebSDR, OpenWebRX, audio streams | None (network jitter of milliseconds) | Not usable for TDoA |

A frequency reference alone fixes oscillator drift. Absolute or relative sample time also requires a measured epoch.

## Native SoapySDR capture

Build the receive-only agent with the SoapySDR development library and the hardware driver installed.

```sh
cargo build --locked --release --features soapy --bin meridian-node
meridian-node capture station.json /data/sigmf
```

The following settings illustrate the file format. Supply the actual device selection, surveyed antenna position and measured timing uncertainty for each site.

```json
{"device":"driver=uhd,serial=YOUR_SERIAL","channel":0,"receiver_id":"n3",
 "position":{"lat":44.43,"lon":26.10},"time_sigma_ns":100,
 "frequency_hz":7075500,"sample_rate_hz":48000,"duration_s":30}
```

An optional `start_utc` requests a common future hardware start and `gain_db` sets manual gain. The native path requires the driver to expose a GPSDO clock and time source, last-PPS hardware time, the hardware GPS UTC second, GPS lock and reference lock. It brackets each recording with PPS checks and validates timestamp continuity for every read. A successful capture publishes the SigMF pair with metadata written last. Drivers with different timing interfaces can supply their own timestamped SigMF recordings. Meridian has exercised this native interface against a software device through SoapySDR 0.8.1. Verification on a physical GPSDO-equipped receiver remains open. [UHD GPSDO example](https://github.com/EttusResearch/uhd/blob/master/host/examples/sync_to_gps.cpp) · [SoapyUHD driver](https://github.com/pothosware/SoapyUHD/blob/master/SoapyUHDDevice.cpp)

## 1. SigMF recordings (GNSS-disciplined nodes)

One recording per node per event, each a [SigMF](https://github.com/sigmf/SigMF) pair (`.sigmf-meta` + `.sigmf-data`):

- `global`: `core:datatype` (`ci16_le`, `cf32_le`, `cf64_le`, `ci8` or `cu8`), `core:sample_rate`, `core:version`, `core:geolocation` (GeoJSON point of the antenna);
- `global` Meridian keys: `meridian:receiver_id` (stable id of the physical receiver), `meridian:time_source` (`gnss_pps`, `gpsdo`, `kiwi_gnss` or `reference_sync`) and `meridian:time_sigma_ns` (declared timing uncertainty);
- `captures`: `core:sample_start`, `core:datetime` (RFC 3339 UTC with fractional seconds) and `core:frequency`. Several capture segments, for example one per PPS edge, give a piecewise-linear time base; one segment extrapolates at the nominal rate.

Put the files of one event in `data/inbox/<event>/` and create an empty `READY` file (or write `ft8_v1` into it for an FT8 sub-band). The server ingests it within 15 s: the recordings are channelized to 12 kHz (Kaiser-windowed sinc, ≈80 dB), hashed, analysed by the unknown-emitter (`correlated_v1`) or FT8 profile and stored. Failed events move to `data/inbox-failed/<event>/ERROR.txt` and are never retried automatically. The same from the command line:

```sh
meridian-tdoa ingest path/to/event --profile correlated_v1   # or ft8_v1, optionally --center HZ
```

Raw IQ then follows the [retention policy](PRODUCTION.md#storage-and-retention).

## 2. Arrival-time reports (any node)

Nodes that detect and time signals locally send only arrival times:

```http
POST /api/detections
Authorization: Bearer <node token>
Content-Type: application/json

{"frequency_hz": 14074000,
 "detections": [{"node": "n3", "signal": "ft8-00000024af806f085a08-1795806255", "transmitter": "KA9SOG", "local_s": 1795850415.6512, "sigma_s": 0.0004}]}
```

- `signal` must be identical at every node that heard the same emission (a decoded content hash, a slot, a sequence number).
- `local_s` is the arrival time in the node's own clock; `sigma_s` its standard deviation.
- Each node is declared in the config with the SHA-256 of its token (`printf %s TOKEN | sha256sum`). The token itself is never stored.

Reports of one signal on the same nominal frequency settle after 2 s of inactivity. Late reports can revise the same observation for 120 s after the last change. Configure a matching `signal_prefix` for reference events. For an FT8 reference in the same passband, also configure its `callsign` and nominal `frequency_hz`. The identity must agree across decoded reports. Only use a reference with an independently surveyed position. Three synchronized sites can yield hypotheses. A unique fix requires at least four.

```json
"nodes": [{"id": "n3", "label": "Roof, Bucharest", "position": {"lat": 44.43, "lon": 26.10},
           "position_evidence": {"url": "https://example.org/survey", "note": "GNSS survey of the antenna"},
           "token_sha256": "…", "disciplined": false}],
"reference_emitters": [{"signal_prefix": "ref-dcf77-", "position": {"lat": 50.0175, "lon": 9.0094},
                        "evidence": {"url": "https://www.ptb.de/…", "note": "Published transmitter position"}}]
```

### Clock synchronization from reference emitters

For each free-running node, a Kalman filter tracks its clock offset and drift relative to a master node from every reference emission both received:

```
z = (t_node − t_master) − (d_node − d_master)/c   =   offset(t) + noise
```

Target arrival times are then mapped onto the master's time base with the predicted offset and its uncertainty. In simulation (five nodes, offsets up to ±5 ms, drifts up to ±40 ppm, 100 ns arrival noise, one reference emission per second) the target lands 29 m from truth after 30 references; the same raw times without synchronization do not locate it. `GET /api/clocks` shows the live state. A reference emitter is a calibration input and is never evaluated against itself.

## 3. `meridian-node`: FT8 on the edge

```sh
meridian-node watch /data/sigmf --server https://meridian.example --node n3 --token-file /etc/meridian/token
```

The agent reads every SigMF recording that appears in the directory (written by GNU Radio, UHD tools, SDR++ or an rtl_sdr converter, centred 1.5 kHz above an FT8 dial frequency), decodes FT8 locally, times each message against its rebuilt waveform in the node's own clock and posts the detections. The agent sends compact reports. It can read recordings written by its own SoapySDR capture command or by an external recorder. A local recording can declare `meridian:time_source: "free_running"` to measure on the node clock. The time anchors must still identify the correct 15 s FT8 slot for association. Configure that node as `disciplined: false` and use common reference emissions for clock alignment. Central raw-IQ ingestion requires a calibrated time base.

In a synthetic check, five disciplined nodes at 48 kSPS each report exactly one arrival for a message and the server locates the station 0.81 km from truth.

## VHF/UHF

Nothing in the solver is HF-specific: the frequency limit is 6 GHz, distances are WGS84 geodesics and the global search refines down to metres. The VHF/UHF path accepts calibrated IQ or matched arrival reports from regional receivers. Simulations exercise both the disciplined and free-running paths. A field accuracy claim needs simultaneous recordings at surveyed sites and an independent reference transmitter.
