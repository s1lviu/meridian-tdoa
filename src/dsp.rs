use crate::model::Pair;
use rustfft::{FftPlanner, num_complex::Complex};

const SINC_HALF: usize = 16;
const SINC_PHASES: usize = 1024;
const KAISER_BETA: f64 = 7.86; // Kaiser: β = 0.1102 (A − 8.7) for A = 80 dB

fn bessel_i0(x: f64) -> f64 {
    // Power series; converges quickly for the window's argument range (≤ β).
    let (mut sum, mut term, mut k) = (1., 1., 1.);
    while term > 1e-17 * sum {
        term *= (x / (2. * k)).powi(2);
        sum += term;
        k += 1.;
    }
    sum
}

static SINC_TABLE: std::sync::LazyLock<Vec<f64>> = std::sync::LazyLock::new(|| {
    // tap(j, phase) for j in -H+1..=H and fractional phase in [0,1)
    let mut table = vec![0.; (SINC_PHASES + 1) * 2 * SINC_HALF];
    let norm = bessel_i0(KAISER_BETA);
    for p in 0..=SINC_PHASES {
        let frac = p as f64 / SINC_PHASES as f64;
        for (idx, j) in (-(SINC_HALF as i64) + 1..=SINC_HALF as i64).enumerate() {
            let u = j as f64 - frac;
            let sinc = if u.abs() < 1e-12 {
                1.
            } else {
                (std::f64::consts::PI * u).sin() / (std::f64::consts::PI * u)
            };
            let r = u / SINC_HALF as f64;
            let w = if r.abs() >= 1. {
                0.
            } else {
                bessel_i0(KAISER_BETA * (1. - r * r).sqrt()) / norm
            };
            table[p * 2 * SINC_HALF + idx] = sinc * w;
        }
    }
    table
});

/// Band-limited value of a uniformly sampled stream at fractional index `x` (zero outside).
pub fn sinc_at(x: &[Complex<f64>], pos: f64) -> Complex<f64> {
    let base = pos.floor();
    let frac = pos - base;
    let phase = frac * SINC_PHASES as f64;
    let p0 = phase.floor() as usize;
    let w1 = phase - p0 as f64;
    let p1 = (p0 + 1).min(SINC_PHASES);
    let (t0, t1) = (
        &SINC_TABLE[p0 * 2 * SINC_HALF..],
        &SINC_TABLE[p1 * 2 * SINC_HALF..],
    );
    let mut acc = Complex::new(0., 0.);
    for idx in 0..2 * SINC_HALF {
        let j = base as i64 + idx as i64 - SINC_HALF as i64 + 1;
        if j >= 0 && (j as usize) < x.len() {
            acc += x[j as usize] * (t0[idx] * (1. - w1) + t1[idx] * w1);
        }
    }
    acc
}

pub fn median(x: &[f64]) -> f64 {
    let mut v = x.to_vec();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Identical centered moving-average high-pass on each stream; no relative phase shift.
pub fn highpass(x: &[Complex<f64>]) -> Vec<Complex<f64>> {
    let mut prefix = Vec::with_capacity(x.len() + 1);
    prefix.push(Complex::new(0., 0.));
    for z in x {
        prefix.push(*prefix.last().unwrap() + z);
    }
    (0..x.len())
        .map(|i| {
            let lo = i.saturating_sub(40);
            let hi = (i + 41).min(x.len());
            x[i] - (prefix[hi] - prefix[lo]) / (hi - lo) as f64
        })
        .collect()
}

pub fn spectrum(x: &[Complex<f64>], fs: f64) -> (Vec<[f64; 2]>, f64) {
    let n = 4096.min(x.len());
    if n < 256 {
        return (vec![], 0.);
    }
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(n);
    let mut power = vec![0.; n];
    let mut count: f64 = 0.;
    for chunk in x.chunks_exact(n).take(40) {
        let mut v: Vec<_> = chunk
            .iter()
            .enumerate()
            .map(|(i, z)| {
                z * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (n - 1) as f64).cos())
            })
            .collect();
        fft.process(&mut v);
        for (p, z) in power.iter_mut().zip(v) {
            *p += z.norm_sqr() / (n * n) as f64;
        }
        count += 1.;
    }
    for p in &mut power {
        *p = (*p / count.max(1.0)).max(1e-20);
    }
    let noise = median(&power);
    let peak = power.iter().copied().fold(0., f64::max);
    let score = 10. * (peak / noise).log10();
    let bins = (0..128)
        .map(|k| {
            let p = (0..n / 128)
                .map(|j| power[(k * n / 128 + j + n / 2) % n])
                .sum::<f64>()
                / (n / 128) as f64;
            [(k as f64 / 128. - 0.5) * fs, 10. * p.log10()]
        })
        .collect();
    (bins, score)
}

pub fn correlate(
    aid: &str,
    bid: &str,
    a: &[Complex<f64>],
    b: &[Complex<f64>],
    fs: f64,
    max_delay_s: f64,
) -> Pair {
    correlate_with(aid, bid, a, b, fs, max_delay_s, 2.0)
}

/// Pairwise delay from 2 s windows. Each window is split into coherent sub-windows of
/// `coherent_s` whose normalized cross-correlation magnitudes are combined non-coherently,
/// so a carrier offset of up to ~1/(4·coherent_s) Hz between receivers (ionospheric Doppler)
/// costs little. `coherent_s = 2` reproduces fully coherent 2 s windows (ground wave).
pub fn correlate_with(
    aid: &str,
    bid: &str,
    a: &[Complex<f64>],
    b: &[Complex<f64>],
    fs: f64,
    max_delay_s: f64,
    coherent_s: f64,
) -> Pair {
    let window = (fs * 2.) as usize;
    let sub = ((fs * coherent_s.clamp(0.05, 2.)) as usize)
        .min(window)
        .max(64);
    let parts = (window / sub).max(1);
    let len = (sub * 2).next_power_of_two();
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(len);
    let ifft = planner.plan_fft_inverse(len);
    let lagmax = ((max_delay_s + 0.002) * fs).ceil() as i32;
    let mut delays = vec![];
    let mut co = vec![];
    let mut ratios = vec![];
    let mut aggregate = vec![0.; (2 * lagmax + 1) as usize];
    let mut total = 0;
    for (aa, bb) in a.chunks_exact(window).zip(b.chunks_exact(window)).take(60) {
        let mut vals = vec![0.; (2 * lagmax + 1) as usize];
        let mut used = 0;
        for k in 0..parts {
            let (sa, sb) = (&aa[k * sub..(k + 1) * sub], &bb[k * sub..(k + 1) * sub]);
            let energy = (sa.iter().map(|z| z.norm_sqr()).sum::<f64>()
                * sb.iter().map(|z| z.norm_sqr()).sum::<f64>())
            .sqrt();
            if energy < 1e-15 {
                continue;
            }
            let mut x = vec![Complex::default(); len];
            let mut y = x.clone();
            x[..sub].copy_from_slice(sa);
            y[..sub].copy_from_slice(sb);
            fft.process(&mut x);
            fft.process(&mut y);
            for i in 0..len {
                y[i] *= x[i].conj();
            }
            ifft.process(&mut y);
            for (v, lag) in vals.iter_mut().zip(-lagmax..=lagmax) {
                *v += (y[lag.rem_euclid(len as i32) as usize].norm() / len as f64 / energy).powi(2);
            }
            used += 1;
        }
        if used == 0 {
            continue;
        }
        for v in &mut vals {
            *v = (*v / used as f64).sqrt();
        }
        let m = (1..vals.len() - 1)
            .max_by(|&i, &j| vals[i].total_cmp(&vals[j]))
            .unwrap();
        let exclusion = (fs * 0.001) as usize;
        let second = vals
            .iter()
            .enumerate()
            .filter(|(i, _)| i.abs_diff(m) > exclusion)
            .map(|(_, v)| *v)
            .fold(1e-12, f64::max);
        let ratio = vals[m] / second;
        let den = vals[m - 1] - 2. * vals[m] + vals[m + 1];
        let delta = if den.abs() > 1e-12 {
            0.5 * (vals[m - 1] - vals[m + 1]) / den
        } else {
            0.
        };
        let delay = (m as f64 - lagmax as f64 + delta.clamp(-0.5, 0.5)) / fs;
        for (v, c) in aggregate.iter_mut().zip(&vals) {
            *v += c;
        }
        total += 1;
        if vals[m] >= 0.35 && ratio >= 1.05 && delay.abs() <= max_delay_s + 1. / fs {
            delays.push(delay);
            co.push(vals[m]);
            ratios.push(ratio);
        }
    }
    let delay = if delays.is_empty() {
        0.
    } else {
        median(&delays)
    };
    let spread = if delays.len() > 1 {
        median(&delays.iter().map(|v| (v - delay).abs()).collect::<Vec<_>>()) * 1.4826
    } else {
        1.
    };
    let sigma = spread.max(1. / fs); // conservative sample-period floor, not a calibrated accuracy claim
    let accepted = delays.len() >= 3 && delays.len() * 2 >= total && spread < 0.00025;
    Pair {
        a: aid.into(),
        b: bid.into(),
        delay_us: delay * 1e6,
        sigma_us: sigma * 1e6,
        coherence: if co.is_empty() { 0. } else { median(&co) },
        peak_ratio: if ratios.is_empty() {
            0.
        } else {
            median(&ratios)
        },
        windows: delays.len(),
        accepted,
        correlation: aggregate
            .iter()
            .enumerate()
            .map(|(i, v)| {
                [
                    (i as f64 - lagmax as f64) / fs * 1e6,
                    v / total.max(1) as f64,
                ]
            })
            .collect(),
    }
}

/// Occupied sub-band of a channel.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Segment {
    pub low_hz: f64,
    pub high_hz: f64,
    /// Peak of the combined normalized PSD over the noise median, dB.
    pub peak_db: f64,
    /// Mean in-band signal power above this receiver's spectral noise floor.
    #[serde(default)]
    pub receiver_snr_db: Vec<Option<f64>>,
}

/// Upper quantile of χ²_ν by Wilson–Hilferty (accurate for ν ≳ 10).
fn chi2_quantile(nu: f64, z: f64) -> f64 {
    let a = 2. / (9. * nu);
    nu * (1. - a + z * a.sqrt()).powi(3)
}

/// Φ⁻¹(1 − 10⁻⁶): per-bin false-alarm probability of the occupancy detector.
const Z_PFA_1E6: f64 = 4.753424;

/// CFAR occupancy: Welch PSD (1024-point Hann, 50% overlap) per stream, normalized by its own
/// median, averaged over streams; bins above the χ² threshold for P_fa = 10⁻⁶ form segments.
pub fn occupied(streams: &[&[Complex<f64>]], fs: f64, passband_hz: f64) -> Vec<Segment> {
    let n = 1024usize;
    let hop = n / 2;
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(n);
    let window: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
        .collect();
    let mut combined = vec![0.; n];
    let mut spectra = Vec::with_capacity(streams.len());
    let mut k_segments = usize::MAX;
    let mut used = 0;
    for x in streams {
        if x.len() < n {
            spectra.push(Vec::new());
            continue;
        }
        let mut psd = vec![0.; n];
        let mut k = 0;
        let mut start = 0;
        while start + n <= x.len() {
            let mut v: Vec<Complex<f64>> = x[start..start + n]
                .iter()
                .zip(&window)
                .map(|(z, w)| z * w)
                .collect();
            fft.process(&mut v);
            for (p, z) in psd.iter_mut().zip(&v) {
                *p += z.norm_sqr();
            }
            k += 1;
            start += hop;
        }
        let med = median(&psd).max(1e-300);
        for (c, p) in combined.iter_mut().zip(&mut psd) {
            *p /= med;
            *c += *p;
        }
        spectra.push(psd);
        k_segments = k_segments.min(k);
        used += 1;
    }
    if used == 0 {
        return vec![];
    }
    for c in &mut combined {
        *c /= used as f64;
    }
    // Welch dof for Hann at 50% overlap (overlap correlation 0.167), summed over streams.
    let nu = (2. * k_segments as f64 / (1. + 2. * 0.167f64.powi(2)) * used as f64).max(10.);
    let median_ratio = chi2_quantile(nu, 0.) / nu;
    let threshold = chi2_quantile(nu, Z_PFA_1E6) / nu / median_ratio;
    let freq = |i: usize| -> f64 {
        let k = if i < n / 2 {
            i as f64
        } else {
            i as f64 - n as f64
        };
        k * fs / n as f64
    };
    // Bins in increasing frequency order.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| freq(a).total_cmp(&freq(b)));
    let mut segments: Vec<Segment> = vec![];
    let mut current: Option<(f64, f64, f64, usize)> = None; // (lo, hi, peak, gap)
    let bin = fs / n as f64;
    for &i in &order {
        let f = freq(i);
        if f.abs() > passband_hz {
            continue;
        }
        let above = combined[i] > threshold;
        current = match (current, above) {
            (None, true) => Some((f, f, combined[i], 0)),
            (Some((lo, _, pk, _)), true) => Some((lo, f, pk.max(combined[i]), 0)),
            (Some((lo, hi, pk, gap)), false) if gap < 2 => Some((lo, hi, pk, gap + 1)),
            (Some((lo, hi, pk, _)), false) => {
                segments.push(Segment {
                    low_hz: lo - bin,
                    high_hz: hi + bin,
                    peak_db: 10. * pk.log10(),
                    receiver_snr_db: vec![],
                });
                None
            }
            (None, false) => None,
        };
    }
    if let Some((lo, hi, pk, _)) = current {
        segments.push(Segment {
            low_hz: lo - bin,
            high_hz: hi + bin,
            peak_db: 10. * pk.log10(),
            receiver_snr_db: vec![],
        });
    }
    segments.retain(|s| s.high_hz - s.low_hz >= 3. * bin);
    for seg in &mut segments {
        let bins: Vec<_> = order
            .iter()
            .copied()
            .filter(|&i| (seg.low_hz..=seg.high_hz).contains(&freq(i)))
            .collect();
        seg.receiver_snr_db = spectra
            .iter()
            .map(|p| {
                if p.is_empty() {
                    return None;
                }
                let excess = bins.iter().map(|&i| p[i]).sum::<f64>() / bins.len() as f64 - 1.;
                (excess > 0.).then(|| 10. * excess.log10())
            })
            .collect();
    }
    segments
}

/// Zero-phase band-pass by FFT mask with raised-cosine edges of `edge_hz`.
pub fn bandpass(
    x: &[Complex<f64>],
    fs: f64,
    low_hz: f64,
    high_hz: f64,
    edge_hz: f64,
) -> Vec<Complex<f64>> {
    let n = x.len();
    let mut planner = FftPlanner::new();
    let mut v = x.to_vec();
    planner.plan_fft_forward(n).process(&mut v);
    for (i, z) in v.iter_mut().enumerate() {
        let k = if i < n / 2 {
            i as f64
        } else {
            i as f64 - n as f64
        };
        let f = k * fs / n as f64;
        let w = if f < low_hz - edge_hz || f > high_hz + edge_hz {
            0.
        } else if f < low_hz {
            0.5 - 0.5 * (std::f64::consts::PI * (f - (low_hz - edge_hz)) / edge_hz).cos()
        } else if f > high_hz {
            0.5 + 0.5 * (std::f64::consts::PI * (f - high_hz) / edge_hz).cos()
        } else {
            1.
        };
        *z *= w / n as f64;
    }
    planner.plan_fft_inverse(n).process(&mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    fn noise(n: usize, seed: &mut u64, amp: f64) -> Vec<Complex<f64>> {
        (0..n)
            .map(|_| {
                let mut u = || {
                    *seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ((*seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
                };
                let (a, b) = (u(), u());
                Complex::from_polar((-2. * a.ln()).sqrt() * amp, std::f64::consts::TAU * b)
            })
            .collect()
    }

    #[test]
    fn occupancy_finds_two_emitters_and_no_false_alarm_in_noise() {
        let fs = 12000.;
        let mut seed = 3;
        let n = 12000 * 10;
        let mk = |seed: &mut u64| {
            let mut x = noise(n, seed, 1.);
            // A ~300 Hz wide noise-like signal at +1.5 kHz (8 dB above the floor in its band).
            let wide = bandpass(&noise(n, seed, 5.), fs, 1350., 1650., 20.);
            for (i, z) in x.iter_mut().enumerate() {
                let t = i as f64 / fs;
                // Narrow carrier at −2 kHz.
                *z += Complex::from_polar(0.3, std::f64::consts::TAU * -2000. * t) + wide[i];
            }
            x
        };
        let (a, b) = (mk(&mut seed), mk(&mut seed));
        let segs = occupied(&[&a, &b], fs, 4800.);
        assert!(
            segs.iter()
                .any(|s| s.low_hz <= -2000. && s.high_hz >= -2000.),
            "{segs:?}"
        );
        assert!(
            segs.iter().any(|s| s.low_hz <= 1500. && s.high_hz >= 1500.),
            "{segs:?}"
        );
        let quiet = (noise(n, &mut seed, 1.), noise(n, &mut seed, 1.));
        assert!(occupied(&[&quiet.0, &quiet.1], fs, 4800.).is_empty());
    }

    #[test]
    fn subwindows_survive_a_doppler_offset_that_defeats_coherent_windows() {
        let fs = 12000.;
        let mut seed = 11u64;
        let n = 12000 * 20;
        let base = noise(n + 100, &mut seed, 1.);
        let a: Vec<_> = base[..n].to_vec();
        // B: same broadband signal 9 samples later with a 0.5 Hz carrier offset, plus noise.
        let extra = noise(n, &mut seed, 0.5);
        let b: Vec<_> = (0..n)
            .map(|i| {
                let s = if i >= 9 {
                    base[i - 9]
                } else {
                    Complex::default()
                };
                s * Complex::from_polar(1., std::f64::consts::TAU * 0.5 * i as f64 / fs) + extra[i]
            })
            .collect();
        let coherent = correlate_with("a", "b", &a, &b, fs, 0.002, 2.0);
        let split = correlate_with("a", "b", &a, &b, fs, 0.002, 0.25);
        assert!(!coherent.accepted || coherent.coherence < split.coherence);
        assert!(
            split.accepted && (split.delay_us - 750.).abs() < 20.,
            "{} µs",
            split.delay_us
        );
    }
    #[test]
    fn sinc_interpolation_is_band_limited_where_linear_is_not() {
        // 4.5 kHz complex tone sampled at 12003.7 Hz, read back at arbitrary fractional times.
        let (fs, f) = (12003.7, 4500.);
        let x: Vec<_> = (0..4000)
            .map(|n| Complex::from_polar(1., std::f64::consts::TAU * f * n as f64 / fs))
            .collect();
        let (mut err, mut lin) = (0f64, 0f64);
        for i in 0..1000 {
            let pos = 1000. + i as f64 * 1.37301;
            let exact = Complex::from_polar(1., std::f64::consts::TAU * f * pos / fs);
            err = err.max((sinc_at(&x, pos) - exact).norm());
            let j = pos.floor() as usize;
            let a = pos - j as f64;
            lin = lin.max((x[j] * (1. - a) + x[j + 1] * a - exact).norm());
        }
        assert!(err < 1e-3, "sinc error {err}");
        assert!(lin > 0.1, "linear error {lin}");
    }
    #[test]
    fn recovers_delay_and_rejects_a_tone() {
        let mut seed = 123u64;
        let a: Vec<_> = (0..96000)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                Complex::new((seed >> 32) as u32 as f64 / u32::MAX as f64 - 0.5, 0.)
            })
            .collect();
        let mut b = vec![Complex::default(); a.len()];
        b[9..].copy_from_slice(&a[..a.len() - 9]);
        let p = correlate("a", "b", &a, &b, 12000., 0.002);
        assert!(p.accepted);
        assert!((p.delay_us - 750.).abs() < 1.);
        assert!(spectrum(&a, 12000.).1 < 15.); // White noise is not reported as an emission.
        let tone: Vec<_> = (0..96000)
            .map(|i| Complex::from_polar(1., i as f64 * 0.3))
            .collect();
        assert!(!correlate("a", "b", &tone, &tone, 12000., 0.002).accepted);
    }
}
