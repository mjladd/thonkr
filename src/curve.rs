//! Interpolated breakpoint curves and the parameters built on them.
//!
//! > "For every parameter Thonk drops a certain amount of randomish values at
//! > randomish times in a score. During execution, there is a constant
//! > interpolation between these values."
//! >
//! > -- the thOnk_0+2 manual
//!
//! Each parameter is two layers: a score curve of randomly placed breakpoints,
//! and a band-limited randomizer riding on top whose depth is itself a slow
//! curve. A parameter can therefore drift from steady to jittery and back.

use rand::{RngExt, SeedableRng};
use rand_pcg::Pcg64;

/// Every random draw in the engine goes through this one type.
///
/// A seed reproduces a render exactly, but only within this implementation.
/// The draw order is part of what a seed means, so changing the order of the
/// calls below changes what every saved seed produces.
pub struct Rand(Pcg64);

impl Rand {
    pub fn from_seed(seed: u64) -> Self {
        Rand(Pcg64::seed_from_u64(seed))
    }

    /// A number in `[lo, hi)`, or `lo` when the two are equal.
    pub fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        if hi <= lo {
            return lo;
        }
        self.0.random_range(lo..hi)
    }

    /// A number in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        self.0.random_range(0.0..1.0)
    }
}

/// The shortest segment a curve will place, as a guard against a score that
/// asks for an unbounded number of breakpoints. A validated score never
/// reaches it.
const MIN_SEGMENT: f64 = 1e-4;

/// The most breakpoints one curve will hold, for the same reason.
const MAX_BREAKPOINTS: usize = 8_000_000;

/// Randomly placed breakpoints in `[lo, hi]`, continuously interpolated.
///
/// Segment durations are themselves random, within `[min_seg, max_seg]`. The
/// interpolation is raised-cosine, so parameter motion has no corners.
pub struct Curve {
    times: Vec<f64>,
    values: Vec<f64>,
    smooth: bool,
}

impl Curve {
    pub fn new(
        rng: &mut Rand,
        lo: f64,
        hi: f64,
        min_seg: f64,
        max_seg: f64,
        duration: f64,
        smooth: bool,
    ) -> Self {
        // Clamped rather than rejected here: score validation gives the user a
        // real message, and this only has to stay finite if it is reached.
        let min_seg = min_seg.max(MIN_SEGMENT);
        let max_seg = max_seg.max(min_seg);

        // The curve runs one segment past the end, so `at` always has a pair of
        // breakpoints to interpolate between.
        let mut times = vec![0.0];
        let mut values = vec![rng.uniform(lo, hi)];
        let mut t = 0.0;
        while t < duration + max_seg && times.len() < MAX_BREAKPOINTS {
            t += rng.uniform(min_seg, max_seg);
            times.push(t);
            values.push(rng.uniform(lo, hi));
        }
        // A single breakpoint has nothing to interpolate towards.
        if times.len() < 2 {
            times.push(times[0] + max_seg);
            values.push(values[0]);
        }
        Curve {
            times,
            values,
            smooth,
        }
    }

    /// The value of the curve at time `t`.
    pub fn at(&self, t: f64) -> f64 {
        // partition_point is searchsorted with side="right".
        let last = self.times.len() - 2;
        let idx = self
            .times
            .partition_point(|x| *x <= t)
            .saturating_sub(1)
            .min(last);
        let (t0, t1) = (self.times[idx], self.times[idx + 1]);
        let (v0, v1) = (self.values[idx], self.values[idx + 1]);
        let mut frac = ((t - t0) / (t1 - t0).max(1e-9)).clamp(0.0, 1.0);
        if self.smooth {
            frac = 0.5 - 0.5 * (std::f64::consts::PI * frac).cos();
        }
        v0 + (v1 - v0) * frac
    }

    /// How many breakpoints the curve holds.
    pub fn len(&self) -> usize {
        self.times.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

/// A score curve plus a separately controlled band-limited randomizer.
pub struct Parameter {
    base: Curve,
    /// Smooth noise at a fixed rate, and the slow curve that scales it.
    modulation: Option<(Curve, Curve)>,
    lo: f64,
    hi: f64,
    span: f64,
}

impl Parameter {
    /// Build a parameter. `rand_rate` is in hertz and `rand_depth` is a
    /// fraction of the range. Either at zero turns the randomizer off.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rng: &mut Rand,
        lo: f64,
        hi: f64,
        min_seg: f64,
        max_seg: f64,
        duration: f64,
        rand_rate: f64,
        rand_depth: f64,
        smooth: bool,
    ) -> Self {
        let base = Curve::new(rng, lo, hi, min_seg, max_seg, duration, smooth);
        let modulation = if rand_rate > 0.0 && rand_depth > 0.0 {
            let seg = 1.0 / rand_rate;
            let noise = Curve::new(rng, -1.0, 1.0, seg, seg, duration, smooth);
            let depth = Curve::new(rng, 0.0, rand_depth, 3.0, 30.0, duration, smooth);
            Some((noise, depth))
        } else {
            None
        };
        Parameter {
            base,
            modulation,
            lo,
            hi,
            span: hi - lo,
        }
    }

    pub fn at(&self, t: f64) -> f64 {
        let mut v = self.base.at(t);
        if let Some((noise, depth)) = &self.modulation {
            v += noise.at(t) * depth.at(t) * self.span;
        }
        v.clamp(self.lo, self.hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng() -> Rand {
        Rand::from_seed(1)
    }

    #[test]
    fn a_seed_reproduces_the_same_draws() {
        let a: Vec<f64> = (0..8).map(|_| Rand::from_seed(7).unit()).collect();
        let mut r = Rand::from_seed(7);
        let b: Vec<f64> = (0..8).map(|_| r.unit()).collect();
        assert!(
            a.iter().all(|v| *v == b[0]),
            "a fresh seed repeats its first draw"
        );
        assert_ne!(b[0], b[1], "successive draws differ");

        let mut c = Rand::from_seed(7);
        let d: Vec<f64> = (0..8).map(|_| c.unit()).collect();
        assert_eq!(b, d, "the same seed gives the same sequence");
    }

    #[test]
    fn different_seeds_give_different_sequences() {
        let mut a = Rand::from_seed(1);
        let mut b = Rand::from_seed(2);
        assert_ne!(a.unit(), b.unit());
    }

    #[test]
    fn uniform_stays_inside_its_bounds() {
        let mut r = rng();
        for _ in 0..1000 {
            let v = r.uniform(-12.0, 12.0);
            assert!((-12.0..12.0).contains(&v), "got {v}");
        }
        assert_eq!(
            r.uniform(3.0, 3.0),
            3.0,
            "an empty range gives its one value"
        );
        assert_eq!(
            r.uniform(5.0, 1.0),
            5.0,
            "a backwards range gives the low bound"
        );
    }

    #[test]
    fn a_curve_covers_its_whole_duration() {
        let c = Curve::new(&mut rng(), 0.0, 1.0, 1.0, 5.0, 100.0, true);
        assert!(c.len() >= 20, "100 seconds of 1 to 5 second segments");
        for t in [0.0, 0.5, 50.0, 99.9, 100.0] {
            let v = c.at(t);
            assert!((0.0..=1.0).contains(&v), "at {t} got {v}");
        }
    }

    #[test]
    fn a_curve_reads_its_breakpoints_back_exactly() {
        let c = Curve::new(&mut rng(), -5.0, 5.0, 2.0, 2.0, 20.0, true);
        // Segments are all 2 seconds, so the breakpoints sit on even times.
        for k in 0..10 {
            let t = 2.0 * k as f64;
            assert!((c.at(t) - c.values[k]).abs() < 1e-12, "breakpoint {k}");
        }
    }

    #[test]
    fn interpolation_is_raised_cosine_when_asked_and_linear_otherwise() {
        let smooth = Curve::new(&mut rng(), 0.0, 1.0, 4.0, 4.0, 8.0, true);
        let straight = Curve::new(&mut rng(), 0.0, 1.0, 4.0, 4.0, 8.0, false);
        // Both land on the midpoint half way between two breakpoints.
        for c in [&smooth, &straight] {
            let want = 0.5 * (c.values[0] + c.values[1]);
            assert!((c.at(2.0) - want).abs() < 1e-12);
        }
        // A quarter of the way along, the raised cosine lags the straight line.
        let quarter_smooth =
            (smooth.at(1.0) - smooth.values[0]) / (smooth.values[1] - smooth.values[0]);
        assert!(
            (quarter_smooth - 0.1464466).abs() < 1e-6,
            "got {quarter_smooth}"
        );
        let quarter_straight =
            (straight.at(1.0) - straight.values[0]) / (straight.values[1] - straight.values[0]);
        assert!(
            (quarter_straight - 0.25).abs() < 1e-12,
            "got {quarter_straight}"
        );
    }

    #[test]
    fn a_curve_holds_flat_outside_its_range() {
        let c = Curve::new(&mut rng(), 0.0, 1.0, 1.0, 1.0, 5.0, true);
        assert_eq!(c.at(-100.0), c.values[0], "before the start");
        let last = *c.values.last().unwrap();
        assert_eq!(c.at(1e6), last, "after the end");
    }

    #[test]
    fn a_parameter_without_a_randomizer_is_just_its_curve() {
        let mut a = rng();
        let p = Parameter::new(&mut a, 2.0, 8.0, 3.0, 9.0, 60.0, 0.0, 0.0, true);
        let mut b = rng();
        let c = Curve::new(&mut b, 2.0, 8.0, 3.0, 9.0, 60.0, true);
        for t in [0.0, 7.5, 30.0, 59.0] {
            assert_eq!(p.at(t), c.at(t), "at {t}");
        }
    }

    #[test]
    fn a_randomizer_moves_the_value_but_never_past_the_bounds() {
        let plain = Parameter::new(&mut rng(), 0.0, 1.0, 5.0, 20.0, 120.0, 0.0, 0.0, true);
        let jittery = Parameter::new(&mut rng(), 0.0, 1.0, 5.0, 20.0, 120.0, 4.0, 0.3, true);
        let mut moved = 0;
        for k in 0..1200 {
            let t = k as f64 * 0.1;
            let v = jittery.at(t);
            assert!((0.0..=1.0).contains(&v), "at {t} got {v}");
            if (v - plain.at(t)).abs() > 1e-9 {
                moved += 1;
            }
        }
        assert!(
            moved > 600,
            "the randomizer should move most samples, moved {moved}"
        );
    }

    #[test]
    fn a_zero_length_segment_cannot_hang_the_build() {
        // Validation rejects this, but the guard must hold regardless.
        let c = Curve::new(&mut rng(), 0.0, 1.0, 0.0, 0.0, 1.0, true);
        assert!(c.len() >= 2);
        assert!(c.len() <= MAX_BREAKPOINTS);
    }
}
