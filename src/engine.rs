//! The grain engine.
//!
//! Each of the six parameters the manual lists is driven by a score curve and
//! a randomizer, built in [`crate::curve`]. This module turns those curves into
//! grains: when each one starts, how long it is, where in the input it comes
//! from, what it is transposed by, and where it sits in the stereo field.
//!
//! Transposition goes through a statistical multi-voice transposer. Each voice
//! has its own drifting interval, and each grain is assigned to one of them by
//! weight.

use std::collections::HashMap;
use std::sync::Arc;

use crate::curve::{Curve, Parameter, Rand};
use crate::score::ScoreSpec;

/// The spacing of the grid the density curve is integrated on, in seconds.
const DENSITY_STEP: f64 = 0.002;

/// The shortest grain, in samples.
const MIN_GRAIN: usize = 8;

/// How close to a whole grain counts as a whole grain.
///
/// The integral is accumulated one segment at a time, so a density curve whose
/// true integral is exactly 10 can come back as 9.999999999999998. Flooring
/// that gives 9 grains where 10 are due. A billionth of a grain is not audible
/// and the error cannot accumulate, because the remainder carries over.
const GRAIN_EPSILON: f64 = 1e-9;

/// How many grain envelopes to keep.
const ENVELOPE_CACHE: usize = 4096;

pub struct Thonk {
    /// The input file, mono.
    x: Vec<f32>,
    rate: u32,
    duration: f64,
    spread: f64,
    stretch: bool,

    density: Parameter,
    length: Parameter,
    attack: Parameter,
    position: Parameter,
    balance: Parameter,

    /// One drifting interval per voice, and the weights that pick between them.
    voices: Vec<Curve>,
    voice_cdf: Vec<f64>,

    /// Samples of overlap carried into the next block, so a grain that starts
    /// near the end of one block finishes in the next.
    tail: usize,
    overlap: Vec<[f32; 2]>,

    /// The fraction of a grain left over between blocks.
    carry: f64,
    grains: u64,
    envelopes: HashMap<(usize, usize), Arc<[f32]>>,
}

impl Thonk {
    /// Build an engine. `spread` overrides the score's own value when given.
    pub fn new(
        source: Vec<f32>,
        rate: u32,
        score: &ScoreSpec,
        duration: f64,
        rng: &mut Rand,
        spread: Option<f64>,
    ) -> Self {
        let d = duration;
        let mut make = |spec: &crate::score::ParamSpec| {
            Parameter::new(
                rng,
                spec.range[0],
                spec.range[1],
                spec.seg[0],
                spec.seg[1],
                d,
                spec.rand[0],
                spec.rand[1],
                true,
            )
        };
        // The draw order below is part of what a seed means. Do not reorder it
        // without accepting that every saved seed renders differently.
        let density = make(&score.density);
        let length = make(&score.length);
        let attack = make(&score.attack);
        let position = make(&score.position);
        let balance = make(&score.balance);

        let t = &score.transpose;
        let voices: Vec<Curve> = (0..score.voices)
            .map(|_| Curve::new(rng, t.range[0], t.range[1], t.seg[0], t.seg[1], d, true))
            .collect();
        let weights: Vec<f64> = (0..score.voices).map(|_| rng.uniform(0.3, 1.0)).collect();
        let total: f64 = weights.iter().sum();
        let mut running = 0.0;
        let voice_cdf: Vec<f64> = weights
            .iter()
            .map(|w| {
                running += w / total;
                running
            })
            .collect();

        // Room for the longest grain that can start at the last sample of a
        // block, at any transposition.
        let tail = (score.length.range[1] * f64::from(rate) * 2.0) as usize + 64;

        Thonk {
            x: source,
            rate,
            duration,
            spread: spread.unwrap_or(score.spread),
            stretch: score.stretch,
            density,
            length,
            attack,
            position,
            balance,
            voices,
            voice_cdf,
            tail,
            overlap: vec![[0.0, 0.0]; tail],
            carry: 0.0,
            grains: 0,
            envelopes: HashMap::new(),
        }
    }

    /// How many grains have been written so far.
    pub fn grains(&self) -> u64 {
        self.grains
    }

    /// Render one block of output, and report the mean density across it.
    pub fn render_block(&mut self, t0: f64, t1: f64, rng: &mut Rand) -> (Vec<[f32; 2]>, f64) {
        let n_out = ((t1 - t0) * f64::from(self.rate)).round() as usize;
        let mut buf = vec![[0.0f32, 0.0]; n_out + self.tail];
        for (slot, carried) in buf.iter_mut().zip(&self.overlap) {
            slot[0] += carried[0];
            slot[1] += carried[1];
        }

        let (onsets, density) = self.onsets(t0, t1);
        if !onsets.is_empty() {
            self.mix_grains(&onsets, t0, &mut buf, rng);
            self.grains += onsets.len() as u64;
        }

        self.overlap.clear();
        self.overlap
            .extend_from_slice(&buf[n_out..n_out + self.tail]);
        buf.truncate(n_out);
        (buf, density)
    }

    /// Grain onset times in `[t0, t1)`, from the integral of the density curve.
    ///
    /// The density curve says grains per second. Its integral says how many
    /// grains have happened, so inverting the integral at 1, 2, 3 and so on
    /// gives the time of each grain. The fractional remainder carries into the
    /// next block, so density is continuous across the join.
    fn onsets(&mut self, t0: f64, t1: f64) -> (Vec<f64>, f64) {
        // The grid runs to t1 inclusive, matching numpy's arange with the same
        // small tolerance on the end.
        let steps = ((t1 + 1e-9 - t0) / DENSITY_STEP).ceil() as usize;
        if steps < 2 {
            return (Vec::new(), 0.0);
        }
        let grid: Vec<f64> = (0..steps).map(|i| t0 + i as f64 * DENSITY_STEP).collect();
        let dens: Vec<f64> = grid.iter().map(|t| self.density.at(*t)).collect();
        let mean = dens.iter().sum::<f64>() / dens.len() as f64;

        // The trapezoid rule, accumulated.
        let mut integral = Vec::with_capacity(steps);
        integral.push(0.0);
        let mut running = 0.0;
        for i in 1..steps {
            running += 0.5 * (dens[i] + dens[i - 1]) * (grid[i] - grid[i - 1]);
            integral.push(running);
        }

        let total = running + self.carry;
        if total + GRAIN_EPSILON < 1.0 {
            self.carry = total;
            return (Vec::new(), mean);
        }
        let count = (total + GRAIN_EPSILON) as usize;

        // Both the targets and the integral are sorted, so one walk serves all
        // of them. This is the inversion np.interp does.
        let mut onsets = Vec::with_capacity(count);
        let mut j = 0usize;
        for k in 0..count {
            let target = k as f64 + 1.0 - self.carry;
            while j + 2 < integral.len() && integral[j + 1] <= target {
                j += 1;
            }
            let (x0, x1) = (integral[j], integral[j + 1]);
            let (y0, y1) = (grid[j], grid[j + 1]);
            let span = x1 - x0;
            // A flat integral means no grains in that segment, so stay put.
            let frac = if span > 0.0 {
                ((target - x0) / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            onsets.push(y0 + (y1 - y0) * frac);
        }
        self.carry = (total - count as f64).max(0.0);
        (onsets, mean)
    }

    /// Cut each grain out of the input, window it, pan it, and sum it in.
    fn mix_grains(&mut self, onsets: &[f64], t0: f64, buf: &mut [[f32; 2]], rng: &mut Rand) {
        let rate = f64::from(self.rate);
        // i0 + 1 must stay inside the input, so the last usable origin is two
        // samples from the end.
        let src_max = (self.x.len() as f64 - 2.0).max(0.0);

        for &onset in onsets {
            let length = self.length.at(onset);
            let attack = self.attack.at(onset);
            let mut position = self.position.at(onset);
            // One draw for the stereo scatter, then one to pick a voice. This
            // order is part of what a seed means.
            let balance =
                (self.balance.at(onset) + rng.uniform(-self.spread, self.spread)).clamp(0.0, 1.0);
            let pick = self.voice_cdf.partition_point(|c| *c < rng.unit());
            let pick = pick.min(self.voices.len() - 1);
            let ratio = (self.voices[pick].at(onset) / 12.0).exp2();

            if self.stretch {
                // In-file time walks the input from start to end across the
                // whole render, with the position curve as local jitter.
                let progress = onset / self.duration.max(1e-9);
                position = (progress + position).clamp(0.0, 1.0);
            }

            let n = ((length * rate) as usize).max(MIN_GRAIN);
            let start = ((onset - t0) * rate) as usize;
            // Python truncates a slice that runs past the end. Rust panics, so
            // the length is clamped here instead.
            let n = n.min(buf.len().saturating_sub(start));
            if n == 0 {
                continue;
            }

            let origin = position * (src_max - n as f64 * ratio).max(0.0);
            let env = self.envelope(n, attack);
            // The sine and cosine pan law, so a grain keeps its power as it
            // moves across the field.
            let gl = (balance * std::f64::consts::FRAC_PI_2).cos() as f32;
            let gr = (balance * std::f64::consts::FRAC_PI_2).sin() as f32;

            for i in 0..n {
                let idx = (origin + ratio * i as f64).clamp(0.0, src_max);
                let i0 = idx as usize;
                let frac = (idx - i0 as f64) as f32;
                let sample = self.x[i0] * (1.0 - frac) + self.x[i0 + 1] * frac;
                let value = sample * env[i];
                buf[start + i][0] += value * gl;
                buf[start + i][1] += value * gr;
            }
        }
    }

    /// The trapezoidal grain window: the attack portion up, the decay down.
    fn envelope(&mut self, n: usize, attack: f64) -> Arc<[f32]> {
        let a = ((n as f64 * attack) as usize).max(1);
        let key = (n, a);
        if let Some(env) = self.envelopes.get(&key) {
            return Arc::clone(env);
        }
        // A grain shorter than two ramps is all ramp.
        let a = if 2 * a >= n { (n / 2).max(1) } else { a };
        let mut env = vec![1.0f32; n];
        for (i, slot) in env.iter_mut().take(a).enumerate() {
            *slot = i as f32 / a as f32;
        }
        for i in 0..a {
            // A one-sample decay holds at full, as numpy's linspace does.
            env[n - a + i] = if a > 1 {
                1.0 - i as f32 / (a - 1) as f32
            } else {
                1.0
            };
        }
        let env: Arc<[f32]> = Arc::from(env);
        if self.envelopes.len() < ENVELOPE_CACHE {
            self.envelopes.insert(key, Arc::clone(&env));
        }
        env
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::built_in;

    /// A short click, the kind of input the original asked for.
    fn source(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / n as f32;
                (1.0 - t) * (i as f32 * 0.3).sin()
            })
            .collect()
    }

    fn engine(name: &str, duration: f64, seed: u64) -> (Thonk, Rand) {
        let mut rng = Rand::from_seed(seed);
        let score = built_in(name).unwrap();
        let engine = Thonk::new(source(4410), 22050, &score, duration, &mut rng, None);
        (engine, rng)
    }

    #[test]
    fn a_block_comes_back_the_length_it_was_asked_for() {
        let (mut e, mut rng) = engine("flowing", 20.0, 1);
        let (block, _) = e.render_block(0.0, 1.0, &mut rng);
        assert_eq!(block.len(), 22050);
        let (half, _) = e.render_block(1.0, 1.5, &mut rng);
        assert_eq!(half.len(), 11025);
    }

    #[test]
    fn a_block_holds_sound_rather_than_silence() {
        let (mut e, mut rng) = engine("flowing", 20.0, 1);
        let (block, density) = e.render_block(0.0, 2.0, &mut rng);
        let peak = block.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.01, "peak was {peak}");
        assert!(density > 0.0, "density was {density}");
        assert!(e.grains() > 0, "no grains were written");
        assert!(block.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn grain_count_follows_the_density_curve() {
        // Over a long enough stretch, the grains written must match the
        // integral of the density curve to within one grain per block.
        let (mut e, mut rng) = engine("flowing", 60.0, 5);
        let mut expected = 0.0;
        for k in 0..30 {
            let (t0, t1) = (k as f64, k as f64 + 1.0);
            let (_, density) = e.render_block(t0, t1, &mut rng);
            expected += density;
        }
        let got = e.grains() as f64;
        let error = (got - expected).abs() / expected;
        assert!(
            error < 0.02,
            "wrote {got} grains, the curve says {expected}"
        );
    }

    /// A constant density curve has an exact integral, so the onsets can be
    /// checked against arithmetic rather than against a tolerance.
    #[test]
    fn a_constant_density_puts_grains_at_an_exact_spacing() {
        let mut score = built_in("flowing").unwrap();
        // lo equals hi, so the curve is flat. No randomizer on top of it.
        score.density = crate::score::ParamSpec::new([10.0, 10.0], [5.0, 5.0], [0.0, 0.0]);
        let mut rng = Rand::from_seed(1);
        let mut e = Thonk::new(source(4410), 22050, &score, 10.0, &mut rng, None);

        let (onsets, density) = e.onsets(0.0, 1.0);
        assert!((density - 10.0).abs() < 1e-12, "the curve is flat at 10");
        assert_eq!(onsets.len(), 10, "ten grains a second for one second");
        for (k, onset) in onsets.iter().enumerate() {
            let want = (k + 1) as f64 / 10.0;
            assert!(
                (onset - want).abs() < 1e-9,
                "grain {k} at {onset}, wanted {want}"
            );
        }
        assert!(
            e.carry.abs() < 1e-9,
            "a whole number of grains leaves no carry"
        );
    }

    /// The same, where the count does not divide evenly into the block.
    #[test]
    fn a_constant_density_carries_the_exact_remainder() {
        let mut score = built_in("flowing").unwrap();
        score.density = crate::score::ParamSpec::new([2.5, 2.5], [5.0, 5.0], [0.0, 0.0]);
        let mut rng = Rand::from_seed(1);
        let mut e = Thonk::new(source(4410), 22050, &score, 10.0, &mut rng, None);

        let (first, _) = e.onsets(0.0, 1.0);
        assert_eq!(first.len(), 2, "2.5 grains a second gives two whole ones");
        assert!((first[0] - 0.4).abs() < 1e-9, "the first at 1/2.5 seconds");
        assert!((first[1] - 0.8).abs() < 1e-9);
        assert!((e.carry - 0.5).abs() < 1e-9, "half a grain carries over");

        // The carry shifts the next block, so the spacing stays 0.4 across the
        // join rather than restarting.
        let (second, _) = e.onsets(1.0, 2.0);
        assert_eq!(second.len(), 3);
        assert!(
            (second[0] - 1.2).abs() < 1e-9,
            "0.4 after the last one, not 1.4"
        );
        assert!((second[1] - 1.6).abs() < 1e-9);
        assert!((second[2] - 2.0).abs() < 1e-9);
    }

    #[test]
    fn density_is_continuous_across_a_block_join() {
        // The same span rendered as one block and as ten must hold the same
        // number of grains, because the fractional remainder carries over.
        let (mut whole, mut rng_a) = engine("flowing", 30.0, 9);
        whole.render_block(0.0, 10.0, &mut rng_a);

        let (mut split, mut rng_b) = engine("flowing", 30.0, 9);
        for k in 0..10 {
            split.render_block(k as f64, k as f64 + 1.0, &mut rng_b);
        }
        let (a, b) = (whole.grains() as i64, split.grains() as i64);
        assert!((a - b).abs() <= 1, "one block wrote {a}, ten wrote {b}");
    }

    #[test]
    fn the_tail_of_one_block_arrives_in_the_next() {
        let (mut e, mut rng) = engine("hectic", 20.0, 3);
        let (_, _) = e.render_block(0.0, 1.0, &mut rng);
        let carried = e.overlap.iter().flatten().any(|v| v.abs() > 0.0);
        assert!(
            carried,
            "a dense score must leave grains hanging over the join"
        );

        // Those samples must appear at the start of the next block.
        let want = e.overlap[0];
        let (block, _) = e.render_block(1.0, 2.0, &mut rng);
        assert!(block[0][0].abs() >= want[0].abs() - 1e-6 || block[0][0] != 0.0);
    }

    #[test]
    fn a_seed_reproduces_a_render_exactly() {
        let render = |seed: u64| {
            let (mut e, mut rng) = engine("hectic", 10.0, seed);
            let (block, _) = e.render_block(0.0, 2.0, &mut rng);
            block
        };
        assert_eq!(render(42), render(42));
        assert_ne!(render(42), render(43));
    }

    #[test]
    fn every_score_renders_without_panicking() {
        for name in crate::score::BUILT_IN_NAMES {
            let (mut e, mut rng) = engine(name, 30.0, 11);
            for k in 0..3 {
                let (block, _) = e.render_block(k as f64, k as f64 + 1.0, &mut rng);
                assert!(block.iter().flatten().all(|v| v.is_finite()), "{name}");
            }
            assert!(e.grains() > 0, "{name} wrote no grains");
        }
    }

    #[test]
    fn the_stretch_scores_walk_the_input_from_start_to_end() {
        // Mark the input so the output says which part it came from: silence
        // for the first half, a tone for the second.
        let mut marked = vec![0.0f32; 4410];
        for (i, v) in marked.iter_mut().enumerate().skip(2205) {
            *v = (i as f32 * 0.4).sin();
        }
        let score = built_in("stretch1").unwrap();
        let mut rng = Rand::from_seed(4);
        let mut e = Thonk::new(marked, 22050, &score, 60.0, &mut rng, None);

        let early = e.render_block(0.0, 2.0, &mut rng).0;
        let early_peak = early.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
        // Skip to the far end of the render.
        for k in 1..29 {
            e.render_block(k as f64 * 2.0, k as f64 * 2.0 + 2.0, &mut rng);
        }
        let late = e.render_block(58.0, 60.0, &mut rng).0;
        let late_peak = late.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));

        assert!(
            early_peak < 0.01,
            "the start should be silent, got {early_peak}"
        );
        assert!(late_peak > 0.05, "the end should sound, got {late_peak}");
    }

    #[test]
    fn the_envelope_opens_and_closes() {
        let (mut e, _) = engine("flowing", 10.0, 1);
        let env = e.envelope(100, 0.25);
        assert_eq!(env.len(), 100);
        assert_eq!(env[0], 0.0, "starts silent");
        assert_eq!(env[99], 0.0, "ends silent");
        assert_eq!(env[50], 1.0, "full in the middle");
        assert!(env[10] > 0.0 && env[10] < 1.0, "ramps up");
    }

    #[test]
    fn a_grain_too_short_for_two_ramps_is_all_ramp() {
        let (mut e, _) = engine("flowing", 10.0, 1);
        let env = e.envelope(8, 0.5);
        assert_eq!(env.len(), 8);
        assert_eq!(env[0], 0.0);
        assert_eq!(env[7], 0.0);
        assert!(env.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn the_envelope_cache_is_capped() {
        let (mut e, _) = engine("flowing", 10.0, 1);
        for n in 1..(ENVELOPE_CACHE + 500) {
            e.envelope(n, 0.3);
        }
        assert_eq!(e.envelopes.len(), ENVELOPE_CACHE);
    }

    #[test]
    fn a_silent_stretch_writes_no_grains_and_carries_the_remainder() {
        // Density below one grain per block leaves nothing to write.
        let mut score = built_in("sparse").unwrap();
        score.density = crate::score::ParamSpec::new([0.1, 0.1], [100.0, 100.0], [0.0, 0.0]);
        let mut rng = Rand::from_seed(2);
        let mut e = Thonk::new(source(4410), 22050, &score, 100.0, &mut rng, None);

        let (block, density) = e.render_block(0.0, 1.0, &mut rng);
        assert_eq!(e.grains(), 0, "0.1 grains a second cannot fill one block");
        assert!((density - 0.1).abs() < 1e-9);
        assert!(block.iter().flatten().all(|v| *v == 0.0));
        assert!((e.carry - 0.1).abs() < 1e-9, "the fraction must carry");

        // Nothing is lost across the joins: what has been written plus what is
        // still carried always equals the integral of the density curve.
        for k in 1..30 {
            e.render_block(k as f64, k as f64 + 1.0, &mut rng);
            let accounted = e.grains() as f64 + e.carry;
            let due = 0.1 * (k + 1) as f64;
            assert!(
                (accounted - due).abs() < 1e-9,
                "after {} seconds: {accounted} accounted for, {due} due",
                k + 1
            );
        }
        assert_eq!(e.grains(), 3, "three grains in thirty seconds");
    }
}
