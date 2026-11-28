# Live sources: public KiwiSDRs and how Meridian uses them

[Back to Meridian](../README.md)

Meridian's live input is IQ it records itself. Spot networks (PSK Reporter, WSPRnet, reverse beacon feeds) are not used for positions, activity or anything else.

## Directory

Receiver metadata comes from the documented public mirror `https://cdn.aethersdr.com/kiwi.json` (schema 1, cached ≥30 min). A receiver is a candidate only if its operator publishes positive `ext_api` and `tdoa_ch`, GNSS is healthy (`gps_good ≥ 4`, `fixes_min > 0`), a channel is free, the antenna is connected and documented, the position is valid and the endpoint is plain HTTP on a public host. Directory metadata is never evidence of reception: `/status` is checked again right before each connection and synchronization again in every recorded IQ block.

## Fair use

Volunteer receivers are shared with listeners. Meridian keeps one connection per receiver at a time, at least 60 s apart, at most 60 minutes per receiver per 24 h, backs off after failures and honours refusals: a receiver that reports its per-IP time limit (`ip_limit`), all channels busy, a password requirement or "down" is blocked for that period. Connection time counts from the opened session to its end, including sessions that fail, time out or are refused.

Receivers reached through a KiwiSDR relay (`<n>.proxy.kiwisdr.com`, `proxy2`, `proxy3`) share the relay's TCP endpoint. When the relay refuses the TCP connection to every receiver behind it in one capture, Meridian blocks the relay itself (`relay:<host>` in the ledger) with the same back-off as a receiver, and contacts no receiver behind it until that block ends. A receiver behind the relay that answers clears it. The native Rust SND client records refusal codes directly. A configured HTTP CONNECT proxy selects pinned KiwiClient before the connection. Set `MERIDIAN_KIWI_TRANSPORT=kiwiclient` to select it explicitly. For this transport, `scripts/kiwi_transport.py` echoes just those lines so Meridian can act on them, and refuses any second connection attempt or redirect to another endpoint.

Observed on 27 November 2026, from one residential IP, after the earlier monitor had run for six hours without a budget:

- the `*.proxy.kiwisdr.com` relay (and `kiwisdr.com` itself) refused every connection, removing 135 of the 268 then-eligible receivers;
- several European receivers answered `ip_limit: 120` (minutes per 24 h);
- of 85 receivers tried during the FT8 runs, 23 delivered IQ; the rest failed preflight (mostly GNSS reported unavailable) or delivered nothing.

Observed on 28 November 2026 from the production server, in its first 17 minutes ([evidence](../evidence/live/relay-refusals-2026-11-28.json)):

- `proxy.kiwisdr.com` refused the TCP connection to all 89 receivers tried through it, from 12:13:05 to 12:27:04 UTC. Blocking receiver by receiver, the monitor kept trying them one after another; the relay-wide block above replaces that;
- `proxy2.kiwisdr.com` delivered one capture at 12:11:42 UTC and refused both of its receivers at 12:22:26;
- 45 of 83 directly reachable receivers delivered IQ;
- the DCF77 target still used a fixed list of five receivers. Three were behind the relays, one answered with its per-IP limit and one had no free channel, so every DCF77 window failed before contacting anyone. DCF77 now picks its receivers from the directory, starting near its configured ones.

A real-time map of many transmitters cannot rest on borrowed channels alone. The receiver budget keeps Meridian a polite guest; [own nodes](NODES.md) are how coverage grows.

## Kiwi IQ

The Rust client uses one WebSocket session per authorized receiver and preserves its IQ blocks and sequence. The 12 kHz and 20/20.25 kHz Kiwi modes are supported. Wide captures are anti-alias filtered before resampling. Kiwi IQ WAV files carry a GNSS timestamp (`kiwi` chunk: GPS seconds of week, nanoseconds, solution age) before every block of samples. Meridian discards the first second, unwraps the GPS week, rejects stale solutions (age > 2 s), discontinuities or unstable rates, and resamples every receiver onto one common 12 kHz UTC grid with a Kaiser-windowed sinc interpolator. Receivers whose timestamps go backwards are excluded with the reason recorded.

## Provenance kept per capture

`source-checks.json` (each receiver's `/status` at capture time, contact e-mail removed), `capture-outcomes.json`, per-receiver recorder logs, `manifest.json` with SHA-256 of every IQ file, and the verifier's evidence (`verification.json`, `ft8-verification.json`, `channel-detection.json` or `ibp-verification.json`). These small files stay 30 days; raw IQ follows the [retention rules](PRODUCTION.md#storage-and-retention).
