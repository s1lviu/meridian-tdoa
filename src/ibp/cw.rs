use super::CALLS;
use crate::{dsp::median, model::Pair};
use rustfft::num_complex::Complex;
use serde::{Deserialize, Serialize};

// NCDXF specifies a 54 ms dot. All envelope indices below are milliseconds.
const DOT: usize = 54;
pub(super) const LEAD_MS: usize = 150;
const FS: usize = 12_000;

fn morse(c: u8) -> &'static str {
    const LETTERS: [&str; 26] = [
        ".-", "-...", "-.-.", "-..", ".", "..-.", "--.", "....", "..", ".---", "-.-", ".-..", "--",
        "-.", "---", ".--.", "--.-", ".-.", "...", "-", "..-", "...-", ".--", "-..-", "-.--",
        "--..",
    ];
    const DIGITS: [&str; 10] = [
        "-----", ".----", "..---", "...--", "....-", ".....", "-....", "--...", "---..", "----.",
    ];
    if c.is_ascii_digit() {
        DIGITS[(c - b'0') as usize]
    } else {
        LETTERS[(c - b'A') as usize]
    }
}

pub(super) struct Template {
    call: &'static str,
    marks: Vec<(usize, usize)>,
    length: usize,
    on: usize,
}
pub(super) fn templates() -> Vec<Template> {
    CALLS
        .iter()
        .map(|&call| {
            let mut marks = Vec::new();
            let mut t = 0;
            for c in call.bytes() {
                for element in morse(c).bytes() {
                    let end = t + DOT * if element == b'.' { 1 } else { 3 };
                    marks.push((t, end));
                    t = end + DOT;
                }
                t += 2 * DOT;
            }
            let on = marks.iter().map(|(a, b)| b - a).sum();
            Template {
                call,
                length: t - 3 * DOT,
                marks,
                on,
            }
        })
        .collect()
}

fn envelope(iq: &[Complex<f64>], hz: f64) -> Vec<f64> {
    let step = Complex::from_polar(1., -std::f64::consts::TAU * hz / FS as f64);
    let mut phase = Complex::new(1., 0.);
    let mut sums = vec![Complex::<f64>::default()];
    for block in iq.chunks_exact(FS / 1000) {
        let mut sum = *sums.last().unwrap();
        for &z in block {
            sum += z * phase;
            phase *= step;
        }
        sums.push(sum);
    }
    (0..sums.len() - 1)
        .map(|i| {
            let lo = i.saturating_sub(10);
            let hi = (i + 11).min(sums.len() - 1);
            (sums[hi] - sums[lo]).norm() / ((hi - lo) * (FS / 1000)) as f64
        })
        .collect()
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Match {
    pub accepted: bool,
    pub best_call: String,
    pub score: f64,
    pub runner_up: f64,
    pub key_to_gap_db: f64,
    pub tone_hz: f64,
    pub onset_ms: f64,
}
pub(super) struct Reception {
    pub evidence: Match,
    // Independently observed half-amplitude crossings. Missing/faded edges remain missing.
    edges: Vec<Option<f64>>,
    envelope: Vec<f64>,
    onset: usize,
    length: usize,
}

#[derive(Default, Clone, Copy)]
struct Fit {
    score: f64,
    hz: f64,
    start: usize,
    on: f64,
    off: f64,
}

pub(super) fn identify(
    iq: &[Complex<f64>],
    expected: &str,
    center_offset: f64,
    patterns: &[Template],
) -> Reception {
    let mut fits = vec![Fit::default(); patterns.len()];
    for delta in (-150..=150).step_by(25) {
        let hz = center_offset + delta as f64;
        let env = envelope(iq, hz);
        let mut sum = vec![0.];
        let mut squares = vec![0.];
        for &v in &env {
            sum.push(sum.last().unwrap() + v);
            squares.push(squares.last().unwrap() + v * v);
        }
        for (t, best) in patterns.iter().zip(&mut fits) {
            // Include silence after the final character when comparing different-length calls.
            let n = t.length + 2 * DOT;
            for start in (50..=510).step_by(3) {
                let end = start + n;
                if end >= env.len() {
                    break;
                }
                let total = sum[end] - sum[start];
                let on: f64 = t
                    .marks
                    .iter()
                    .map(|(a, b)| sum[start + b] - sum[start + a])
                    .sum();
                let keyed = t.on as f64;
                let var = squares[end] - squares[start] - total * total / n as f64;
                let cov = on - total * keyed / n as f64;
                let score = cov
                    / (var.max(0.) * keyed * (1. - keyed / n as f64))
                        .sqrt()
                        .max(1e-20);
                if score > best.score {
                    *best = Fit {
                        score,
                        hz,
                        start,
                        on: on / keyed,
                        off: (total - on) / (n as f64 - keyed),
                    };
                }
            }
        }
    }
    let mut ranks: Vec<_> = (0..fits.len()).collect();
    ranks.sort_by(|&i, &j| fits[j].score.total_cmp(&fits[i].score));
    let winner = ranks[0];
    let fit = fits[winner];
    let t = &patterns[winner];
    let env = envelope(iq, fit.hz);
    let dash = fit.start + t.length + 7 * DOT;
    let first_dash = env.get(dash + 100..dash + 850).map(median).unwrap_or(0.);
    let contrast = 20. * (fit.on.max(1e-15) / fit.off.max(1e-15)).log10();
    let runner_up = fits[ranks[1]].score;
    let accepted = t.call == expected
        && fit.score >= 0.72
        && fit.score - runner_up >= 0.10
        && contrast >= 5.
        && first_dash > fit.off + 0.30 * (fit.on - fit.off);
    let mut edges = Vec::new();
    if accepted {
        for &(a, b) in &t.marks {
            for (edge, rising) in [(a, true), (b, false)] {
                let x = fit.start + edge;
                let left = median(&env[x - 23..x - 13]);
                let right = median(&env[x + 13..x + 23]);
                let level = (left + right) / 2.;
                let sign = if rising { 1. } else { -1. };
                let crossing = if sign * (right - left) > 0.35 * (fit.on - fit.off) {
                    (x - 13..x + 13)
                        .filter(|&i| {
                            sign * (env[i] - level) <= 0. && sign * (env[i + 1] - level) > 0.
                        })
                        .min_by_key(|&i| i.abs_diff(x))
                        .map(|i| i as f64 + (level - env[i]) / (env[i + 1] - env[i]))
                } else {
                    None
                };
                edges.push(crossing);
            }
        }
    }
    Reception {
        evidence: Match {
            accepted,
            best_call: t.call.into(),
            score: fit.score,
            runner_up,
            key_to_gap_db: contrast,
            tone_hz: fit.hz,
            onset_ms: fit.start as f64 - LEAD_MS as f64,
        },
        edges,
        envelope: env,
        onset: fit.start,
        length: t.length,
    }
}

// Compare two measured envelopes and the matched keying edges in the same GNSS slot.
// The schedule is never used as an absolute transmit timestamp in the position solver.
pub(super) fn delay(aid: &str, bid: &str, a: &Reception, b: &Reception, physical_s: f64) -> Pair {
    let times: Vec<_> = a
        .edges
        .iter()
        .zip(&b.edges)
        .filter_map(|(x, y)| Some((*y)? - (*x)?))
        .collect();
    let dt = if times.is_empty() { 0. } else { median(&times) };
    let mad = if times.is_empty() {
        10.
    } else {
        1.4826 * median(&times.iter().map(|v| (v - dt).abs()).collect::<Vec<_>>())
    };
    let range = ((physical_s * 1000.).ceil() as i32 + 2).max(2 * DOT as i32);
    let first = a.onset.max(range as usize);
    let end = (a.onset + a.length).min(a.envelope.len().saturating_sub(range as usize));
    let correlation: Vec<[f64; 2]> = (-range..=range)
        .map(|lag| {
            let (mut sx, mut sy, mut xx, mut yy, mut xy, mut n) = (0., 0., 0., 0., 0., 0.);
            for i in (first..end).step_by(2) {
                let x = a.envelope[i];
                let y = b.envelope[(i as i32 + lag) as usize];
                sx += x;
                sy += y;
                xx += x * x;
                yy += y * y;
                xy += x * y;
                n += 1.;
            }
            let corr = if n > 0. {
                (xy - sx * sy / n)
                    / ((xx - sx * sx / n).max(0.) * (yy - sy * sy / n).max(0.))
                        .sqrt()
                        .max(1e-20)
            } else {
                0.
            };
            [lag as f64 * 1000., corr.clamp(-1., 1.)]
        })
        .collect();
    let index = (dt.round() as i32 + range).clamp(0, 2 * range) as usize;
    let coherence = correlation[index][1];
    let secondary = correlation
        .iter()
        .filter(|v| (v[0] / 1000. - dt).abs() >= 20.)
        .map(|v| v[1])
        .fold(0.01, f64::max);
    let ratio = coherence / secondary;
    let accepted = times.len() >= 8
        && times.len() * 2 >= a.edges.len()
        && mad <= 1.5
        && dt.abs() <= physical_s * 1000. + 1.
        && coherence >= 0.65
        && ratio >= 1.15;
    Pair {
        a: aid.into(),
        b: bid.into(),
        delay_us: dt * 1000.,
        sigma_us: mad.max(1.) * 1000.,
        coherence,
        peak_ratio: ratio,
        windows: times.len(),
        accepted,
        correlation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyed(t: &Template, lag: usize) -> Vec<Complex<f64>> {
        let offset = LEAD_MS + 23 + lag;
        let dash = t.length + 7 * DOT;
        (0..(10000 + LEAD_MS) * 12)
            .map(|i| {
                let keyed = (i / 12).checked_sub(offset).is_some_and(|ms| {
                    t.marks.iter().any(|&(a, b)| (a..b).contains(&ms))
                        || (dash..dash + 1000).contains(&ms)
                });
                Complex::from_polar(
                    if keyed { 0.7 } else { 0.01 },
                    std::f64::consts::TAU * 725. * i as f64 / FS as f64,
                )
            })
            .collect()
    }

    #[test]
    fn complete_call_and_observed_delay_are_independent_checks() {
        let patterns = templates();
        let call = "ZL6B";
        let t = patterns.iter().find(|t| t.call == call).unwrap();
        let raw = keyed(t, 0);
        let first = identify(&raw, call, 700., &patterns);
        assert!(first.evidence.accepted);
        let later = identify(&keyed(t, 4), call, 700., &patterns);
        assert!(later.evidence.accepted);
        let pair = delay("first", "later", &first, &later, 0.010);
        assert!(pair.accepted && (pair.delay_us - 4000.).abs() < 100.);
        assert!(!delay("first", "later", &first, &later, 0.001).accepted);
        assert!(!identify(&raw, "OH2B", 700., &patterns).evidence.accepted);
        let mut no_dash = raw.clone();
        no_dash[(LEAD_MS + 23 + t.length) * 12..].fill(Complex::default());
        assert!(!identify(&no_dash, call, 700., &patterns).evidence.accepted);
        let carrier: Vec<_> = (0..raw.len())
            .map(|i| Complex::from_polar(1., std::f64::consts::TAU * 700. * i as f64 / FS as f64))
            .collect();
        assert!(!identify(&carrier, call, 700., &patterns).evidence.accepted);
    }
}
