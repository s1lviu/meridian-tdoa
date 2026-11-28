//! FT8 from our own IQ: decoding (vendored ft8_lib, MIT) and exact waveform synthesis.
//! The decoded payload is re-encoded to its 79 channel tones, so every receiver's arrival
//! time can be measured against the transmitted waveform itself (see `toa`).
use rustfft::num_complex::Complex;
use std::{
    ffi::{CStr, c_char, c_float, c_int},
    sync::Mutex,
};

pub const SYMBOLS: usize = 79;
pub const SYMBOL_PERIOD_S: f64 = 0.160;
pub const TONE_SPACING_HZ: f64 = 6.25;
pub const SLOT_S: i64 = 15;
/// GFSK bandwidth-time product for FT8 (ft8_lib demo/gen_ft8.c, WSJT-X).
const SYMBOL_BT: f64 = 2.0;
/// Measured relation between ft8_lib's reported `time_s` and the true first-symbol start
/// (synthetic round trip in the tests): reported − true lies within LAG ± TOLERANCE.
pub const DECODER_LAG_S: f64 = 0.156;
pub const DECODER_TOLERANCE_S: f64 = 0.06;
/// π·sqrt(2/ln 2), as defined in ft8_lib demo/gen_ft8.c.
const GFSK_CONST_K: f64 = 5.336446;

#[repr(C)]
struct Raw {
    freq_hz: c_float,
    time_s: c_float,
    score: c_int,
    ldpc_errors: c_int,
    payload: [u8; 10],
    tones: [u8; SYMBOLS],
    text: [c_char; 36],
    call_to: [c_char; 16],
    call_de: [c_char; 16],
    extra: [c_char; 16],
    field_types: [c_int; 3],
    rc: c_int,
}

unsafe extern "C" {
    fn mft8_decode(signal: *const c_float, n: c_int, fs: c_int, out: *mut Raw, max: c_int)
    -> c_int;
    fn mft8_tones(payload: *const u8, tones: *mut u8);
    fn mft8_sizeof() -> c_int;
    fn mft8_reset_hashes();
}

/// ft8_lib keeps a process-global callsign hash table.
static LOCK: Mutex<()> = Mutex::new(());

/// ft8_lib `ftx_field_t`.
const FIELD_CALL: i32 = 4;
const FIELD_GRID: i32 = 5;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Decode {
    /// Audio frequency of tone 0 in the USB-demodulated signal, Hz.
    pub freq_hz: f64,
    /// Start of the first symbol from the start of the analysed buffer, s.
    pub time_s: f64,
    pub score: i32,
    pub payload: [u8; 10],
    #[serde(with = "serde_tones")]
    pub tones: [u8; SYMBOLS],
    pub text: String,
    pub call_to: String,
    pub call_de: String,
    pub extra: String,
    pub field_types: [i32; 3],
}

mod serde_tones {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(t: &[u8; super::SYMBOLS], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&t.iter().map(|d| (b'0' + d) as char).collect::<String>())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; super::SYMBOLS], D::Error> {
        let s = String::deserialize(d)?;
        let v: Vec<u8> = s.bytes().map(|b| b.wrapping_sub(b'0')).collect();
        v.try_into()
            .map_err(|_| serde::de::Error::custom("79 tones expected"))
    }
}

fn text(raw: &[c_char]) -> String {
    unsafe { CStr::from_ptr(raw.as_ptr()) }
        .to_string_lossy()
        .trim()
        .to_owned()
}

fn plain_call(s: &str) -> Option<String> {
    // Hash-resolved calls (<...>) were not transmitted in this message; never used as identity.
    let ok = !s.is_empty()
        && !s.starts_with('<')
        && s.len() <= 11
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'/')
        && s.bytes().any(|b| b.is_ascii_digit());
    ok.then(|| s.to_owned())
}

impl Decode {
    /// The station that transmitted this message (second call of a standard message).
    pub fn transmitter(&self) -> Option<String> {
        (self.field_types[1] == FIELD_CALL)
            .then(|| plain_call(&self.call_de))
            .flatten()
    }

    /// Four-character Maidenhead locator sent by the transmitter, if the message carries one.
    pub fn grid(&self) -> Option<String> {
        if self.field_types[2] != FIELD_GRID {
            return None;
        }
        let g = self.extra.split_whitespace().last()?;
        let b = g.as_bytes();
        (b.len() == 4
            && g != "RR73"
            && (b'A'..=b'R').contains(&b[0])
            && (b'A'..=b'R').contains(&b[1])
            && b[2].is_ascii_digit()
            && b[3].is_ascii_digit())
        .then(|| g.to_owned())
    }
}

/// Decode every FT8 message in up to one 15 s slot of real audio (12 kHz recommended).
pub fn decode(audio: &[f32], sample_rate: u32) -> Vec<Decode> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        unsafe { mft8_sizeof() } as usize,
        std::mem::size_of::<Raw>()
    );
    let max = 300;
    let mut out: Vec<Raw> = Vec::with_capacity(max);
    let n = unsafe {
        mft8_decode(
            audio.as_ptr(),
            audio.len() as c_int,
            sample_rate as c_int,
            out.as_mut_ptr(),
            max as c_int,
        )
    };
    unsafe { out.set_len(n.max(0) as usize) };
    out.iter()
        .map(|r| Decode {
            freq_hz: r.freq_hz as f64,
            time_s: r.time_s as f64,
            score: r.score,
            payload: r.payload,
            tones: r.tones,
            text: text(&r.text),
            call_to: text(&r.call_to),
            call_de: text(&r.call_de),
            extra: text(&r.extra),
            field_types: r.field_types.map(|v| v as i32),
        })
        .collect()
}

/// Forget callsign hashes learned from earlier slots (each capture is decoded independently).
pub fn reset_hashes() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe { mft8_reset_hashes() };
}

/// Channel tones of a 77-bit payload (CRC-14 + LDPC(174,91) + Costas + Gray map).
pub fn tones(payload: &[u8; 10]) -> [u8; SYMBOLS] {
    let mut t = [0u8; SYMBOLS];
    unsafe { mft8_tones(payload.as_ptr(), t.as_mut_ptr()) };
    t
}

fn gfsk_pulse(n_spsym: usize) -> Vec<f64> {
    (0..3 * n_spsym)
        .map(|i| {
            let t = i as f64 / n_spsym as f64 - 1.5;
            let a = GFSK_CONST_K * SYMBOL_BT * (t + 0.5);
            let b = GFSK_CONST_K * SYMBOL_BT * (t - 0.5);
            (libm::erf(a) - libm::erf(b)) / 2.
        })
        .collect()
}

/// Instantaneous phase increment per sample of the GFSK waveform (port of ft8_lib `synth_gfsk`).
fn phase_steps(tones: &[u8; SYMBOLS], f0_hz: f64, fs: f64) -> (Vec<f64>, usize) {
    let n_spsym = (0.5 + fs * SYMBOL_PERIOD_S) as usize;
    let n_wave = SYMBOLS * n_spsym;
    let peak = std::f64::consts::TAU / n_spsym as f64; // h = 1
    let mut dphi = vec![std::f64::consts::TAU * f0_hz / fs; n_wave + 2 * n_spsym];
    let pulse = gfsk_pulse(n_spsym);
    for (i, &s) in tones.iter().enumerate() {
        for (j, p) in pulse.iter().enumerate() {
            dphi[j + i * n_spsym] += peak * s as f64 * p;
        }
    }
    for j in 0..2 * n_spsym {
        dphi[j] += peak * pulse[j + n_spsym] * tones[0] as f64;
        dphi[j + SYMBOLS * n_spsym] += peak * pulse[j] * tones[SYMBOLS - 1] as f64;
    }
    (dphi, n_spsym)
}

fn envelope(k: usize, n_wave: usize, n_spsym: usize) -> f64 {
    let ramp = n_spsym / 8;
    let i = k.min(n_wave - 1 - k);
    if i < ramp {
        (1. - (std::f64::consts::TAU * i as f64 / (2 * ramp) as f64).cos()) / 2.
    } else {
        1.
    }
}

/// Real audio waveform with tone 0 at `f0_hz` (identical to ft8_lib's generator).
pub fn synthesize(tones: &[u8; SYMBOLS], f0_hz: f64, fs: f64) -> Vec<f32> {
    let (dphi, n_spsym) = phase_steps(tones, f0_hz, fs);
    let n_wave = SYMBOLS * n_spsym;
    let mut phi = 0f64;
    (0..n_wave)
        .map(|k| {
            let v = phi.sin() * envelope(k, n_wave, n_spsym);
            phi = (phi + dphi[k + n_spsym]) % std::f64::consts::TAU;
            v as f32
        })
        .collect()
}

/// Complex baseband waveform with tone 0 at `f0_hz` (may be negative or zero).
pub fn synthesize_complex(tones: &[u8; SYMBOLS], f0_hz: f64, fs: f64) -> Vec<Complex<f64>> {
    let (dphi, n_spsym) = phase_steps(tones, f0_hz, fs);
    let n_wave = SYMBOLS * n_spsym;
    let mut phi = 0f64;
    (0..n_wave)
        .map(|k| {
            let v = Complex::from_polar(envelope(k, n_wave, n_spsym), phi);
            phi = (phi + dphi[k + n_spsym]).rem_euclid(std::f64::consts::TAU);
            v
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(bytes: &[u8]) -> Vec<f32> {
        // 16-bit PCM mono RIFF; data chunk located by tag.
        let mut i = 12;
        while i + 8 <= bytes.len() {
            let n = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
            if &bytes[i..i + 4] == b"data" {
                return bytes[i + 8..i + 8 + n]
                    .chunks_exact(2)
                    .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.)
                    .collect();
            }
            i += 8 + n + (n % 2);
        }
        panic!("no data chunk")
    }

    fn expected(txt: &str) -> Vec<String> {
        txt.lines()
            .filter_map(|l| l.split_once('~'))
            .map(|(_, m)| m.trim_start().split("  ").next().unwrap().trim().to_owned())
            .collect()
    }

    #[test]
    fn decodes_the_upstream_reference_recordings_like_wsjtx() {
        for (w, t) in [
            (
                &include_bytes!("../third_party/ft8_lib/test/191111_110130.wav")[..],
                include_str!("../third_party/ft8_lib/test/191111_110130.txt"),
            ),
            (
                &include_bytes!("../third_party/ft8_lib/test/20m_busy_test_01.wav")[..],
                include_str!("../third_party/ft8_lib/test/20m_busy_test_01.txt"),
            ),
        ] {
            reset_hashes();
            let got: Vec<String> = decode(&wav(w), 12000).into_iter().map(|d| d.text).collect();
            let want = expected(t);
            let found = want.iter().filter(|m| got.contains(m)).count();
            eprintln!(
                "decoded {} messages; {found}/{} of the WSJT-X decodes",
                got.len(),
                want.len()
            );
            // Every decode must be a real message (CRC-checked); most WSJT-X decodes must be found.
            // ft8_lib has no signal subtraction or a-priori decoding, so it finds fewer than WSJT-X
            // (measured: 4/5 and 16/24 here). The busy file also yields two plausible messages
            // absent from the WSJT-X list ("OE3MLC G3ZQQ 73", "JO1COV PA0CAH JO21"); they cannot be
            // verified here. Localization needs the same payload at several receivers, which a
            // single false decode cannot provide.
            assert!(found * 10 >= want.len() * 6, "{found}/{}", want.len());
            assert!(got.len() - found <= 2);
        }
    }

    #[test]
    fn synthesized_waveform_round_trips_through_the_decoder_and_fields_are_parsed() {
        reset_hashes();
        let audio = wav(include_bytes!(
            "../third_party/ft8_lib/test/191111_110130.wav"
        ));
        let reference = decode(&audio, 12000)
            .into_iter()
            .find(|d| d.text == "CQ TA6CQ KN70")
            .expect("reference message present");
        assert_eq!(reference.transmitter().as_deref(), Some("TA6CQ"));
        assert_eq!(reference.grid().as_deref(), Some("KN70"));
        assert_eq!(tones(&reference.payload), reference.tones);
        // Re-synthesize the decoded payload into silence at new frequencies and times.
        // ft8_lib reports time on a 0.08 s grid with a fixed lag; record both for the TOA search.
        let wave = synthesize(&reference.tones, 1500., 12000.);
        let mut lags = vec![];
        for start in (4000..=24000).step_by(1777) {
            let mut buffer = vec![0f32; 15 * 12000];
            for (k, v) in wave.iter().enumerate() {
                buffer[start + k] = 0.3 * v;
            }
            let back = decode(&buffer, 12000);
            let d = back
                .iter()
                .find(|d| d.payload == reference.payload)
                .expect("round trip");
            assert!((d.freq_hz - 1500.).abs() <= 3.2, "{}", d.freq_hz);
            lags.push(d.time_s - start as f64 / 12000.);
        }
        let (lo, hi) = lags
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        eprintln!(
            "decoder time minus true start: {lo:.3} … {hi:.3} s over {} starts",
            lags.len()
        );
        assert!(
            lo >= DECODER_LAG_S - DECODER_TOLERANCE_S && hi <= DECODER_LAG_S + DECODER_TOLERANCE_S
        );
        // Complex and real synthesis share one phase trajectory.
        let c = synthesize_complex(&reference.tones, 1500., 12000.);
        let wave = synthesize(&reference.tones, 1500., 12000.);
        assert!(
            c.iter()
                .zip(&wave)
                .all(|(z, r)| (z.im - *r as f64).abs() < 1e-5)
        );
    }
}
