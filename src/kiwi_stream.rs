//! Receive the public KiwiSDR SND stream after the operator-policy preflight.
//! One WebSocket handshake to the verified endpoint, with Kiwi GNSS chunks on disk.
use anyhow::{Context, Result, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use std::{path::Path, time::Duration};
use tokio::net::TcpStream;
use tokio::time::{Instant, timeout};
use tokio_tungstenite::{
    client_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

fn header(rate: u32) -> Vec<u8> {
    let mut out = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * 4).to_le_bytes());
    out.extend(4u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out
}

/// Return the number of complex samples. GPS fields use little endian, IQ uses
/// network byte order by default. Store interleaved PCM16 in RIFF byte order.
fn append_iq(out: &mut Vec<u8>, frame: &[u8], previous: &mut Option<u32>) -> Result<usize> {
    ensure!(
        frame.len() >= 24 && (frame.len() - 20).is_multiple_of(4),
        "Malformed Kiwi IQ block"
    );
    ensure!(frame[3] & 0x10 == 0, "Compressed audio is not IQ");
    let seq = u32::from_le_bytes(frame[4..8].try_into()?);
    ensure!(
        previous.is_none_or(|n| n.wrapping_add(1) == seq),
        "Sequence gap in Kiwi IQ"
    );
    *previous = Some(seq);
    let stamp = &frame[10..20];
    ensure!(
        u32::from_le_bytes(stamp[2..6].try_into()?) < 604800
            && u32::from_le_bytes(stamp[6..10].try_into()?) < 1_000_000_000,
        "Invalid Kiwi GPS stamp"
    );
    let iq = &frame[20..];
    ensure!(
        iq.len() <= 65536 && out.len() + 26 + iq.len() <= 64 * 1024 * 1024,
        "Kiwi IQ exceeds capture limit"
    );
    out.extend(b"kiwi");
    out.extend(10u32.to_le_bytes());
    out.extend(stamp);
    out.extend(b"data");
    out.extend((iq.len() as u32).to_le_bytes());
    if frame[3] & 0x80 != 0 {
        out.extend(iq);
    } else {
        for x in iq.chunks_exact(2) {
            out.extend([x[1], x[0]]);
        }
    }
    Ok(iq.len() / 4)
}

pub async fn record(
    endpoint: &str,
    center: f64,
    duration_s: u64,
    dir: &Path,
    id: &str,
) -> Result<()> {
    timeout(
        Duration::from_secs(duration_s + 20),
        receive(endpoint, center, duration_s, dir, id),
    )
    .await
    .context("Kiwi IQ capture timed out")?
}

async fn receive(endpoint: &str, center: f64, duration_s: u64, dir: &Path, id: &str) -> Result<()> {
    let mut url = reqwest::Url::parse(endpoint)?;
    let host = url.host_str().context("Missing receiver host")?;
    let port = url
        .port_or_known_default()
        .context("Missing receiver port")?;
    let tcp = timeout(Duration::from_secs(8), TcpStream::connect((host, port))).await??;
    url.set_scheme("ws")
        .map_err(|_| anyhow::anyhow!("Invalid receiver scheme"))?;
    url.set_path(&format!(
        "/{}/SND",
        chrono::Utc::now().timestamp_millis() as u32
    ));
    let cfg = WebSocketConfig::default()
        .max_message_size(Some(2 * 1024 * 1024))
        .max_frame_size(Some(2 * 1024 * 1024));
    let (mut ws, _) = timeout(
        Duration::from_secs(8),
        client_async_with_config(url.as_str(), tcp, Some(cfg)),
    )
    .await??;
    ws.send(Message::text("SET auth t=kiwi p=")).await?;
    let (mut rate, mut samples) = (0u32, 0usize);
    let mut out = Vec::new();
    let mut previous = None;
    let mut last_keepalive = Instant::now();
    loop {
        let packet = timeout(Duration::from_secs(8), ws.next())
            .await?
            .context("Receiver closed before the requested duration")??;
        if packet.is_close() {
            bail!("Receiver closed the stream");
        }
        if !packet.is_binary() && !packet.is_text() {
            continue;
        }
        let frame = packet.into_data();
        if let Some(msg) = frame.strip_prefix(b"MSG ") {
            for field in std::str::from_utf8(msg)?.split_ascii_whitespace() {
                let (key, value) = field.split_once('=').unwrap_or((field, ""));
                if matches!(
                    key,
                    "too_busy"
                        | "ip_limit"
                        | "down"
                        | "redirect"
                        | "inactivity_timeout"
                        | "monitor"
                ) || (key == "badp" && value != "0")
                {
                    // Log only refusal metadata. Redirect URLs and server configuration are unnecessary.
                    let detail: String = value
                        .chars()
                        .take(32)
                        .filter(|c| c.is_ascii_digit() || *c == '%')
                        .collect();
                    tokio::fs::write(
                        dir.join(format!("{id}.log")),
                        format!("MERIDIAN-REFUSAL {key} {detail}\n"),
                    )
                    .await?;
                    bail!("Receiver refused the recording ({key})");
                }
                match key {
                    "freq_offset" => ensure!(
                        value.parse::<f64>()? == 0.,
                        "Receiver frequency offset changed"
                    ),
                    "audio_rate" => {
                        let input: u32 = value.parse()?;
                        ws.send(Message::text(format!("SET AR OK in={input} out=44100")))
                            .await?;
                    }
                    "sample_rate" => {
                        let resolved = value.parse::<f64>()?.round() as u32;
                        ensure!(
                            (11990..=12010).contains(&resolved)
                                || (19990..=20270).contains(&resolved),
                            "Unsupported Kiwi IQ sample rate {resolved}"
                        );
                        if rate != 0 {
                            ensure!(rate == resolved, "Kiwi sample rate changed");
                            continue;
                        }
                        rate = resolved;
                        out = header(rate);
                        let tune = format!(
                            "SET mod=iq low_cut=-5000 high_cut=5000 freq={:.3}",
                            center / 1000.
                        );
                        for cmd in [
                            "SET squelch=0 max=0",
                            "SET ident_user=Meridian-TDoA%20measurement",
                            &tune,
                            "SET agc=1 hang=0 thresh=-100 slope=6 decay=1000 manGain=50",
                            "SET keepalive",
                        ] {
                            ws.send(Message::text(cmd.to_owned())).await?;
                        }
                    }
                    _ => {}
                }
            }
        } else if frame.starts_with(b"SND") && rate > 0 && frame.get(3).is_some_and(|f| f & 8 != 0)
        {
            samples += append_iq(&mut out, &frame, &mut previous)?;
            if samples as u64 >= u64::from(rate) * duration_s {
                break;
            }
        }
        if last_keepalive.elapsed() >= Duration::from_secs(1) {
            ws.send(Message::text("SET keepalive")).await?;
            last_keepalive = Instant::now();
        }
    }
    let size = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&size.to_le_bytes());
    tokio::fs::write(dir.join(format!("capture_{id}.wav")), out).await?;
    let _ = ws.close(None).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_public_handshake_preserves_gps_blocks_and_operator_refusals() {
        for refuse in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                assert_eq!(
                    ws.next().await.unwrap().unwrap().to_text().unwrap(),
                    "SET auth t=kiwi p="
                );
                if refuse {
                    ws.send(Message::binary(b"MSG badp=0 ip_limit=60".to_vec()))
                        .await
                        .unwrap();
                    return;
                }
                ws.send(Message::binary(
                    b"MSG badp=0 audio_rate=12000 sample_rate=12000".to_vec(),
                ))
                .await
                .unwrap();
                let mut commands = vec![];
                loop {
                    let msg = ws.next().await.unwrap().unwrap();
                    let cmd = msg.into_text().unwrap().to_string();
                    let done = cmd == "SET keepalive";
                    commands.push(cmd);
                    if done {
                        break;
                    }
                }
                assert!(
                    commands
                        .iter()
                        .any(|s| s == "SET mod=iq low_cut=-5000 high_cut=5000 freq=648.000")
                );
                for seq in 0..94u32 {
                    let mut block = b"SND".to_vec();
                    block.push(8);
                    block.extend(seq.to_le_bytes());
                    block.extend([0, 0, 0, 0]);
                    let t = u64::from(seq) * 512 * 1_000_000_000 / 12000;
                    block.extend((100 + (t / 1_000_000_000) as u32).to_le_bytes());
                    block.extend(((t % 1_000_000_000) as u32).to_le_bytes());
                    block.extend([0x40, 0, 0xc0, 0].repeat(512));
                    ws.send(Message::binary(block)).await.unwrap();
                }
            });
            let tmp = tempfile::tempdir().unwrap();
            let result = record(&format!("http://{addr}"), 648000., 4, tmp.path(), "rx").await;
            server.await.unwrap();
            let wav = tmp.path().join("capture_rx.wav");
            if refuse {
                assert!(result.is_err() && !wav.exists());
                assert!(
                    std::fs::read_to_string(tmp.path().join("rx.log"))
                        .unwrap()
                        .contains("ip_limit")
                );
            } else {
                result.unwrap();
                let iq = crate::kiwi::read(&wav).unwrap();
                assert!(iq.sync_error.is_none());
                assert_eq!(
                    iq.blocks[0].iq[0],
                    rustfft::num_complex::Complex::new(0.5, -0.5)
                );
            }
        }
    }
}
