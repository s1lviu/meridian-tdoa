# Public source evidence

FT8 reports arrive through the PSK Reporter all-band FT8 subscription. WSPR reports come through the broker's consolidated WSPRnet.org, WSPRnet.eu, WSPR.live and PSK Reporter feed. Meridian aggregates the transmitter's published field and counts reporting stations with process-local hashes. The report time is `t_tx` when present, otherwise `t`. Reports can arrive late or in bursts. These counters describe reported radio activity. Position and accuracy calculations use independent synchronized IQ. [Feed schema, timing and terms](https://www.mqtt.pskreporter.info/) · [WSJT-X modes](https://wsjt.sourceforge.io/wsjtx-main_en.html)

The documented public mirror is `https://cdn.aethersdr.com/kiwi.json`, schema 1. Meridian does not poll every receiver in a loop or use the centralized KiwiSDR TDoA service's special exemption.

Primary contracts:

- [Directory schema](https://github.com/aethersdr/AetherSDR/blob/main/docs/kiwi-json-schema.md)
- [Public mirror and operator API policy](https://github.com/aethersdr/AetherSDR/blob/main/docs/kiwisdr-public-directory.md)

The stored snapshot `evidence/live/directory.json` is dated **2026-11-26 19:00:13 UTC** and contains **827** receivers, of which **677** publish `ext_api > 0`. Zero disables external API access; a missing field is not permission. The UI retains excluded entries and reasons without claiming RF reception.

The later directory update used in the container API test, dated **20:00:13 UTC**, has **828** entries, **677** positive API allowances and **256** metadata candidates. These counts are recorded in `evidence/release-api-tests.json`; they are not numbers of simultaneous receptions.

Six `/status` endpoints were checked directly (`evidence/live/status-*.json`). Five passed preflight; the Fenu-Radio receiver reported only two good GNSS sources and was excluded. Operator-published redirects to port 8073 and Kiwi proxy2 were checked as canonical endpoints. Redirects to another receiver are rejected. Operator contact email values have been redacted in the public status evidence; no measurement field was changed.

`evidence/live/transport-check.json` records the container connection attempts on 26 November 2026. That run acquired no IQ. Direct connections were refused, a CONNECT tunnel closed during the WebSocket handshake and a proxy Upgrade request returned HTTP 403. The failing layer was not conclusively identified. Directory metadata and preflight GPS are not substitutes for IQ.

On 27 November the native server acquired DCF77 IQ from four synchronized receivers in consecutive scheduled windows and verified its phase code. [Live LF measurements](VALIDATION.md#live-lf-observations)

The worldwide HF monitor samples the published NCDXF/IARU channels on 20, 17, 15, 12 and 10 metres. The directory supplies candidate receiver positions and access metadata. GNSS-stamped IQ supplies the callsign evidence and differential arrival times. Per-band receiver rotation uses receiver geometry alone. [Beacon schedule](https://www.ncdxf.org/beacon/) · [Live HF measurements](VALIDATION.md#live-hf-observations)

The functional-test recording remains the real **13 August 2020 DCF77** dataset from hcab14/TDoA. The RF verifier matches its IQ against the PRN sequence in [PTB-Mitteilungen 114 (2004), section 4.3, figures 7–8](https://www.ptb.de/cms/fileadmin/internet/fachabteilungen/abteilung_4/4.4_zeit_und_frequenz/4.42/dcf77.pdf). The first six tested windows per receiver score 0.862–0.895. Synthetic pure-carrier and noise cases fail the 0.65 threshold. These checks demonstrate archived-data behaviour, not current reception or a universal false-positive rate.

IERS and status evidence retain their observation dates. Policies and receiver state must be checked again for each acquisition, followed by GNSS and correlation checks in the actual recorded samples.
