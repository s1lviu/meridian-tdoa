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
    let window = (fs * 2.) as usize;
    let len = (window * 2).next_power_of_two();
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
        let mut x = vec![Complex::default(); len];
        let mut y = x.clone();
        x[..window].copy_from_slice(aa);
        y[..window].copy_from_slice(bb);
        let energy = (aa.iter().map(|z| z.norm_sqr()).sum::<f64>()
            * bb.iter().map(|z| z.norm_sqr()).sum::<f64>())
        .sqrt();
        if energy < 1e-15 {
            continue;
        }
        fft.process(&mut x);
        fft.process(&mut y);
        for k in 0..len {
            y[k] *= x[k].conj();
        }
        ifft.process(&mut y);
        let vals: Vec<f64> = (-lagmax..=lagmax)
            .map(|lag| y[lag.rem_euclid(len as i32) as usize].norm() / len as f64 / energy)
            .collect();
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

#[cfg(test)]
mod tests {
    use super::*;
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
