//! Time of arrival of a known waveform (matched filter). Used when the transmitted signal can
//! be reconstructed exactly from what was decoded (FT8 payload → tones → GFSK). Each receiver
//! is measured against the template independently, so TDoAs between receivers are consistent
//! by construction (no pair-specific cross-correlation bias).
use rustfft::num_complex::Complex;

/// Factor applied to the jackknife error before it is used as a timing σ (see the Monte Carlo
/// calibration test below, which bounds the measured rms-error / jackknife ratio).
pub const JACKKNIFE_INFLATION: f64 = 2.0;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Arrival {
    /// Lag of the template start in the searched signal, in samples (sub-sample).
    pub lag: f64,
    /// Residual carrier offset found by the search, Hz.
    pub freq_hz: f64,
    /// Normalized non-coherent matched-filter statistic at the peak, 0–1.
    pub peak: f64,
    /// Same statistic far from the peak: mean and standard deviation (noise reference).
    pub noise_mean: f64,
    pub noise_sd: f64,
    /// Leave-one-segment-out (jackknife) standard error of `lag`, samples.
    pub lag_sigma: f64,
}

impl Arrival {
    /// Peak height above the noise reference in noise standard deviations.
    pub fn significance(&self) -> f64 {
        (self.peak - self.noise_mean) / self.noise_sd.max(1e-12)
    }
}

/// Statistic for one lag and frequency, per segment: |Σ y·conj(t)|² / (Et·Ey).
fn segment_stats(
    y: &[Complex<f64>],
    template: &[Complex<f64>],
    bounds: &[(usize, usize)],
    lag: usize,
    rot: &[Complex<f64>],
) -> Vec<f64> {
    bounds
        .iter()
        .map(|&(a, b)| {
            let mut acc = Complex::new(0., 0.);
            let (mut ey, mut et) = (0., 0.);
            for n in a..b {
                let v = y[lag + n] * rot[n];
                acc += v * template[n].conj();
                ey += v.norm_sqr();
                et += template[n].norm_sqr();
            }
            acc.norm_sqr() / (ey * et).max(1e-300)
        })
        .collect()
}

fn combine(stats: &[f64], skip: Option<usize>) -> f64 {
    let (s, n) = stats
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != skip)
        .fold((0., 0.), |(s, n), (_, v)| (s + v, n + 1.));
    s / n
}

fn parabolic(f: &[f64], i: usize) -> f64 {
    if i == 0 || i + 1 >= f.len() {
        return i as f64;
    }
    let den = f[i - 1] - 2. * f[i] + f[i + 1];
    if den.abs() < 1e-300 {
        return i as f64;
    }
    i as f64 + (0.5 * (f[i - 1] - f[i + 1]) / den).clamp(-0.5, 0.5)
}

/// Search `template` in `y` for lags `lo..=hi` (samples) and carrier offsets `freqs` (Hz).
/// The template is split into `segments` coherent parts combined non-coherently, so slow
/// phase wander (Doppler, oscillator drift) does not destroy the correlation.
/// `noise_lags` are lags far from any expected arrival, used as the empirical noise reference.
pub fn matched(
    y: &[Complex<f64>],
    template: &[Complex<f64>],
    fs: f64,
    lo: usize,
    hi: usize,
    freqs: &[f64],
    segments: usize,
    noise_lags: &[usize],
) -> Option<Arrival> {
    let m = template.len();
    if hi + m > y.len() || lo > hi || segments < 2 {
        return None;
    }
    let bounds: Vec<(usize, usize)> = (0..segments)
        .map(|k| (k * m / segments, (k + 1) * m / segments))
        .collect();
    let rotation = |f: f64| -> Vec<Complex<f64>> {
        (0..m)
            .map(|n| Complex::from_polar(1., -std::f64::consts::TAU * f * n as f64 / fs))
            .collect()
    };
    // Coarse grid over frequency, full lag range.
    let mut best: Option<(f64, usize, f64)> = None;
    for &f in freqs {
        let rot = rotation(f);
        for lag in lo..=hi {
            let s = combine(&segment_stats(y, template, &bounds, lag, &rot), None);
            if best.is_none_or(|(b, _, _)| s > b) {
                best = Some((s, lag, f));
            }
        }
    }
    let (_, lag0, f0) = best?;
    // Refine frequency around the coarse optimum.
    let step = if freqs.len() > 1 {
        (freqs[1] - freqs[0]).abs()
    } else {
        0.1
    };
    let mut fbest = (f64::MIN, f0);
    for k in -5..=5 {
        let f = f0 + k as f64 * step / 5.;
        let rot = rotation(f);
        let s = combine(&segment_stats(y, template, &bounds, lag0, &rot), None);
        if s > fbest.0 {
            fbest = (s, f);
        }
    }
    let rot = rotation(fbest.1);
    let span: Vec<usize> = (lag0.saturating_sub(3).max(lo)..=(lag0 + 3).min(hi)).collect();
    let per_lag: Vec<Vec<f64>> = span
        .iter()
        .map(|&l| segment_stats(y, template, &bounds, l, &rot))
        .collect();
    let at = |skip: Option<usize>| -> f64 {
        let curve: Vec<f64> = per_lag.iter().map(|s| combine(s, skip)).collect();
        let i = curve
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap();
        span[0] as f64 + parabolic(&curve, i)
    };
    let lag = at(None);
    let jack: Vec<f64> = (0..segments).map(|k| at(Some(k))).collect();
    let mean = jack.iter().sum::<f64>() / segments as f64;
    let lag_sigma = ((segments - 1) as f64 / segments as f64
        * jack.iter().map(|v| (v - mean).powi(2)).sum::<f64>())
    .sqrt();
    let peak = combine(
        &segment_stats(y, template, &bounds, lag.round() as usize, &rot),
        None,
    );
    let noise: Vec<f64> = noise_lags
        .iter()
        .filter(|&&l| l + m <= y.len())
        .map(|&l| combine(&segment_stats(y, template, &bounds, l, &rot), None))
        .collect();
    let nm = noise.iter().sum::<f64>() / noise.len().max(1) as f64;
    let nsd =
        (noise.iter().map(|v| (v - nm).powi(2)).sum::<f64>() / noise.len().max(1) as f64).sqrt();
    Some(Arrival {
        lag,
        freq_hz: fbest.1,
        peak,
        noise_mean: nm,
        noise_sd: nsd,
        lag_sigma,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovers_a_fractional_delay_of_a_reconstructed_ft8_waveform_in_noise() {
        let tones: [u8; crate::ft8::SYMBOLS] = std::array::from_fn(|i| ((i * 7 + 3) % 8) as u8);
        let fs = 400.;
        let template = crate::ft8::synthesize_complex(&tones, 0., fs);
        let true_lag = 123.37; // samples
        let doppler = 0.4; // Hz, uncompensated by the caller
        let mut seed = 99u64;
        let mut gauss = || {
            let mut u = || {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
            };
            (-2. * u().ln()).sqrt() * (std::f64::consts::TAU * u()).cos()
        };
        // Fractional delay via the exact synthesizer phase: shift the time base.
        let shifted = crate::ft8::synthesize_complex(&tones, 0., fs);
        let n = template.len() + 600;
        let y: Vec<Complex<f64>> = (0..n)
            .map(|i| {
                let t = i as f64 - true_lag;
                let s = if t >= 0. && (t as usize) + 1 < shifted.len() {
                    let j = t.floor() as usize;
                    let a = t - j as f64;
                    shifted[j] * (1. - a) + shifted[j + 1] * a
                } else {
                    Complex::new(0., 0.)
                };
                s * Complex::from_polar(1., std::f64::consts::TAU * doppler * i as f64 / fs)
                    + Complex::new(gauss(), gauss()) * 0.7
            })
            .collect();
        let freqs: Vec<f64> = (-12..=12).map(|k| k as f64 * 0.25).collect();
        let noise: Vec<usize> = (420..560).collect();
        let a = matched(&y, &template, fs, 80, 170, &freqs, 6, &noise).unwrap();
        eprintln!(
            "lag error {:.4} samples ({:.3} ms), jackknife sigma {:.4} samples, freq {:.3} Hz, significance {:.1}",
            a.lag - true_lag,
            (a.lag - true_lag) / fs * 1e3,
            a.lag_sigma,
            a.freq_hz,
            a.significance()
        );
        assert!(
            (a.lag - true_lag).abs() < 0.5,
            "lag {} vs {true_lag}",
            a.lag
        );
        assert!((a.freq_hz - doppler).abs() < 0.15, "freq {}", a.freq_hz);
        assert!(a.significance() > 8., "significance {}", a.significance());
        assert!(
            a.lag_sigma > 0. && a.lag_sigma < 2.,
            "jackknife {}",
            a.lag_sigma
        );
        // Pure noise is not significant.
        let quiet: Vec<Complex<f64>> = (0..n).map(|_| Complex::new(gauss(), gauss())).collect();
        let q = matched(&quiet, &template, fs, 80, 170, &freqs, 6, &noise).unwrap();
        assert!(
            q.significance() < 6.,
            "noise significance {}",
            q.significance()
        );
    }
}

#[cfg(test)]
mod calibration {
    use super::*;
    /// Monte Carlo: exact band-limited fractional delays (sinc), Doppler, complex white noise.
    /// Measures how the jackknife standard error compares with the actual error.
    #[test]
    fn jackknife_sigma_tracks_the_actual_timing_error() {
        let tones: [u8; crate::ft8::SYMBOLS] = std::array::from_fn(|i| ((i * 5 + 1) % 8) as u8);
        let fs = 400.;
        let template = crate::ft8::synthesize_complex(&tones, 0., fs);
        let mut seed = 7u64;
        let mut u = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let (mut e2, mut s2, trials) = (0., 0., 40);
        for k in 0..trials {
            let true_lag = 100. + 0.37 * k as f64;
            let n = template.len() + 600;
            let mut y: Vec<Complex<f64>> = (0..n)
                .map(|i| {
                    crate::dsp::sinc_at(&template, i as f64 - true_lag)
                        * Complex::from_polar(1., std::f64::consts::TAU * 0.3 * i as f64 / fs)
                })
                .collect();
            for v in &mut y {
                let (a, b) = (u(), u());
                let r = (-2. * a.ln()).sqrt() * 1.2;
                *v += Complex::from_polar(r, std::f64::consts::TAU * b);
            }
            let freqs: Vec<f64> = (-8..=8).map(|k| k as f64 * 0.25).collect();
            let noise: Vec<usize> = (430..560).collect();
            let a = matched(&y, &template, fs, 60, 160, &freqs, 6, &noise).unwrap();
            e2 += (a.lag - true_lag).powi(2);
            s2 += a.lag_sigma.powi(2);
        }
        let (rms, sig) = ((e2 / trials as f64).sqrt(), (s2 / trials as f64).sqrt());
        eprintln!(
            "rms error {rms:.4} samples, rms jackknife sigma {sig:.4} samples, ratio {:.2}",
            rms / sig
        );
        assert!(rms / sig < JACKKNIFE_INFLATION, "ratio {}", rms / sig);
    }
}
