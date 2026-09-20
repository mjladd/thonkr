//! One render in progress: engine, output file, gain and stop flag.
//!
//! A render is driven by iterating [`Session::render`], which yields one
//! [`Progress`] per block. The stop flag can be set from a signal handler at
//! any time. The output file is finalized either way, so it stays playable.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::audio::write::StreamWriter;
use crate::curve::Rand;
use crate::engine::Thonk;
use crate::score::ScoreSpec;
use crate::{Error, Result};

/// What to do with a sample past full scale.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Overflow {
    /// Fold back around full scale, as the original did.
    #[default]
    Wrap,
    /// Hold at full scale.
    Clip,
}

/// The headroom `--autogain` holds below full scale.
const HEADROOM: f64 = 0.9;

/// The most the automatic gain will climb back per block.
const RECOVERY: f64 = 1.35;

/// The largest sample a 16-bit file holds.
const CEILING: f32 = 32767.0 / 32768.0;

/// How often the header is rewritten, in seconds of output.
const FLUSH_SECONDS: u64 = 10;

/// The shortest input the engine can take grains from.
const MIN_SOURCE: usize = 64;

/// Everything the command line chooses about a render.
#[derive(Clone, Debug)]
pub struct Settings {
    pub duration: Option<f64>,
    pub rate: Option<u32>,
    pub gain: f64,
    pub overflow: Overflow,
    pub autogain: bool,
    pub seed: Option<u64>,
    pub spread: Option<f64>,
    /// The block length in seconds.
    pub block: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            duration: None,
            rate: None,
            gain: 1.0,
            overflow: Overflow::Wrap,
            autogain: false,
            seed: None,
            spread: None,
            block: 1.0,
        }
    }
}

/// One block's worth of news.
#[derive(Copy, Clone, Debug)]
pub struct Progress {
    /// How far into the render this block ends, in seconds.
    pub t: f64,
    pub duration: f64,
    /// The mean grain density across the block, in grains per second.
    pub density: f64,
    pub grains: u64,
    /// Wall clock seconds since the render started.
    pub elapsed: f64,
    pub frames: u64,
    pub overflows: u64,
    /// The gain applied to this block.
    pub gain: f64,
}

impl Progress {
    pub fn fraction(&self) -> f64 {
        if self.duration > 0.0 {
            (self.t / self.duration).min(1.0)
        } else {
            1.0
        }
    }

    /// How many seconds of output are produced per second of waiting.
    pub fn speed(&self) -> f64 {
        self.t / self.elapsed.max(1e-9)
    }
}

pub struct Session {
    engine: Thonk,
    rng: Rand,
    writer: StreamWriter,
    settings: Settings,

    /// The rate of the input file, for the opening report.
    pub in_rate: u32,
    /// The output rate.
    pub rate: u32,
    pub duration: f64,
    pub seed: u64,
    /// The peak of the input file.
    pub peak: f32,
    /// How many input samples there are.
    pub source_len: usize,

    overflows: u64,
    t: f64,
    elapsed: f64,
    stop: Arc<AtomicBool>,
}

impl Session {
    /// Open the output file and prepare the engine.
    pub fn new(
        source: Vec<f32>,
        in_rate: u32,
        output: &Path,
        score: &ScoreSpec,
        settings: Settings,
    ) -> Result<Self> {
        if source.len() < MIN_SOURCE {
            return Err(Error::other("input file is too short to take grains from"));
        }
        let rate = settings.rate.unwrap_or(in_rate);
        let duration = settings.duration.unwrap_or(score.duration);
        let seed = settings.seed.unwrap_or_else(random_seed);
        let peak = source.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let source_len = source.len();

        let mut rng = Rand::from_seed(seed);
        let engine = Thonk::new(source, rate, score, duration, &mut rng, settings.spread);
        let writer = StreamWriter::create(output, rate)?;

        Ok(Session {
            engine,
            rng,
            writer,
            settings,
            in_rate,
            rate,
            duration,
            seed,
            peak: if peak > 0.0 { peak } else { 1.0 },
            source_len,
            overflows: 0,
            t: 0.0,
            elapsed: 0.0,
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The flag a signal handler sets to stop the render.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    pub fn grains(&self) -> u64 {
        self.engine.grains()
    }

    pub fn overflows(&self) -> u64 {
        self.overflows
    }

    pub fn elapsed(&self) -> f64 {
        self.elapsed
    }

    /// How much audio reached the file.
    pub fn written_seconds(&self) -> f64 {
        self.writer.frames() as f64 / f64::from(self.rate.max(1))
    }

    /// Start the render. Iterate the result to drive it.
    pub fn render(&mut self) -> Render<'_> {
        let auto_gain = self.settings.gain;
        Render {
            started: Instant::now(),
            auto_gain,
            next_flush: FLUSH_SECONDS,
            session: self,
        }
    }

    /// Finalize the header and close the file, reporting any failure.
    pub fn finish(self) -> Result<()> {
        self.writer.close()
    }
}

/// A render in progress. Each step yields one block's [`Progress`].
pub struct Render<'a> {
    session: &'a mut Session,
    started: Instant,
    auto_gain: f64,
    next_flush: u64,
}

impl Iterator for Render<'_> {
    type Item = Result<Progress>;

    fn next(&mut self) -> Option<Self::Item> {
        let s = &mut *self.session;
        if s.t >= s.duration || s.stop.load(Ordering::Relaxed) {
            return None;
        }

        let t1 = (s.t + s.settings.block).min(s.duration);
        let (mut chunk, density) = s.engine.render_block(s.t, t1, &mut s.rng);

        if s.settings.autogain {
            // The block is in hand before it is written, so the gain can come
            // from its measured peak rather than from a model of how grains
            // sum. A drop applies at once and is held flat for the block, so
            // the ramp can never overshoot. Recovery is gradual.
            let peak = chunk
                .iter()
                .flatten()
                .fold(0.0f32, |m, v| m.max(v.abs()))
                .max(1e-9);
            let needed = s.settings.gain.min(HEADROOM / f64::from(peak));
            if needed < self.auto_gain {
                scale_flat(&mut chunk, needed);
                self.auto_gain = needed;
            } else {
                let target = needed.min(self.auto_gain * RECOVERY);
                ramp(&mut chunk, self.auto_gain, target);
                self.auto_gain = target;
            }
        } else if s.settings.gain != 1.0 {
            scale_flat(&mut chunk, s.settings.gain);
        }

        if s.settings.overflow == Overflow::Clip {
            // Counted here rather than in the writer. Clipping happens first,
            // so by the time the writer sees the block there is nothing left
            // past full scale for it to count.
            for frame in chunk.iter_mut() {
                for v in frame.iter_mut() {
                    if *v < -1.0 || *v > CEILING {
                        s.overflows += 1;
                    }
                    *v = v.clamp(-1.0, CEILING);
                }
            }
        }

        match s.writer.write(&chunk) {
            Ok(over) => s.overflows += over,
            Err(e) => return Some(Err(e)),
        }
        s.t = t1;
        s.elapsed = self.started.elapsed().as_secs_f64();

        if s.t as u64 >= self.next_flush {
            self.next_flush = (s.t as u64 / FLUSH_SECONDS + 1) * FLUSH_SECONDS;
            if let Err(e) = s.writer.flush() {
                return Some(Err(e));
            }
        }

        Some(Ok(Progress {
            t: s.t,
            duration: s.duration,
            density,
            grains: s.engine.grains(),
            elapsed: s.elapsed,
            frames: s.writer.frames(),
            overflows: s.overflows,
            gain: self.auto_gain,
        }))
    }
}

impl Drop for Render<'_> {
    /// Leave a playable file behind however the render ended.
    fn drop(&mut self) {
        self.session.elapsed = self.started.elapsed().as_secs_f64();
        let _ = self.session.writer.flush();
    }
}

fn scale_flat(chunk: &mut [[f32; 2]], gain: f64) {
    let g = gain as f32;
    for frame in chunk.iter_mut() {
        frame[0] *= g;
        frame[1] *= g;
    }
}

/// Ride the gain from `from` to `to` across the block, endpoints included.
fn ramp(chunk: &mut [[f32; 2]], from: f64, to: f64) {
    let n = chunk.len();
    if n == 0 {
        return;
    }
    let step = if n > 1 {
        (to - from) / (n - 1) as f64
    } else {
        0.0
    };
    for (i, frame) in chunk.iter_mut().enumerate() {
        let g = (from + step * i as f64) as f32;
        frame[0] *= g;
        frame[1] *= g;
    }
}

/// A seed for a run that did not ask for one.
fn random_seed() -> u64 {
    use rand::RngExt;
    rand::rng().random()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::read::read_audio;
    use crate::score::built_in;

    fn source() -> Vec<f32> {
        (0..4410)
            .map(|i| {
                let t = i as f32 / 4410.0;
                (1.0 - t) * (i as f32 * 0.3).sin() * 0.5
            })
            .collect()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("thonkr-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn settings(duration: f64, seed: u64) -> Settings {
        Settings {
            duration: Some(duration),
            seed: Some(seed),
            ..Settings::default()
        }
    }

    #[test]
    fn a_render_writes_the_length_it_was_asked_for() {
        let path = scratch("length.aiff");
        let score = built_in("flowing").unwrap();
        let mut s = Session::new(source(), 22050, &path, &score, settings(3.0, 1)).unwrap();
        let blocks = s.render().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(blocks.len(), 3, "three one-second blocks");
        assert!((s.written_seconds() - 3.0).abs() < 1e-9);
        s.finish().unwrap();

        let (samples, rate) = read_audio(&path).unwrap();
        assert_eq!(rate, 22050);
        assert_eq!(samples.len(), 3 * 22050);
        assert!(
            samples.iter().any(|v| v.abs() > 0.001),
            "the file is silent"
        );
    }

    #[test]
    fn progress_counts_up_to_the_end() {
        let path = scratch("progress.wav");
        let score = built_in("flowing").unwrap();
        let mut s = Session::new(source(), 22050, &path, &score, settings(4.0, 2)).unwrap();
        let mut last = 0.0;
        let mut grains = 0;
        for step in s.render() {
            let p = step.unwrap();
            assert!(p.t > last, "time must advance");
            assert!(p.grains >= grains, "grains must not go backwards");
            assert!(p.fraction() > 0.0 && p.fraction() <= 1.0);
            assert!(p.speed() > 0.0);
            last = p.t;
            grains = p.grains;
        }
        assert_eq!(last, 4.0);
        assert!(grains > 0);
    }

    #[test]
    fn a_stop_request_ends_the_render_and_leaves_a_playable_file() {
        let path = scratch("stopped.aiff");
        let score = built_in("flowing").unwrap();
        let mut s = Session::new(source(), 22050, &path, &score, settings(600.0, 3)).unwrap();
        let flag = s.stop_flag();

        let mut blocks = 0;
        for step in s.render() {
            step.unwrap();
            blocks += 1;
            if blocks == 2 {
                flag.store(true, Ordering::Relaxed);
            }
        }
        assert_eq!(blocks, 2, "the render must stop when asked");
        assert!(s.stop_requested());
        assert!((s.written_seconds() - 2.0).abs() < 1e-9);
        s.finish().unwrap();

        let (samples, _) = read_audio(&path).unwrap();
        assert_eq!(
            samples.len(),
            2 * 22050,
            "the header counts what was written"
        );
    }

    #[test]
    fn autogain_holds_the_output_below_full_scale() {
        let path = scratch("autogain.aiff");
        let score = built_in("hectic").unwrap();
        let mut s = Session::new(
            source(),
            22050,
            &path,
            &score,
            Settings {
                autogain: true,
                ..settings(8.0, 4)
            },
        )
        .unwrap();
        for step in s.render() {
            let p = step.unwrap();
            assert!(p.gain > 0.0 && p.gain <= 1.0, "gain was {}", p.gain);
        }
        let overflows = s.overflows();
        s.finish().unwrap();

        assert_eq!(overflows, 0, "autogain must leave nothing past full scale");
        let (samples, _) = read_audio(&path).unwrap();
        let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(
            peak > 0.05,
            "autogain must not silence the output, peak {peak}"
        );
    }

    #[test]
    fn clipping_holds_at_full_scale_where_wrapping_folds_over() {
        let score = built_in("hectic").unwrap();
        let loud = Settings {
            gain: 40.0,
            ..settings(3.0, 5)
        };

        let wrapped_path = scratch("wrap.aiff");
        let mut wrapped =
            Session::new(source(), 22050, &wrapped_path, &score, loud.clone()).unwrap();
        wrapped.render().collect::<Result<Vec<_>>>().unwrap();
        let wrap_overflows = wrapped.overflows();
        wrapped.finish().unwrap();

        let clipped_path = scratch("clip.aiff");
        let mut clipped = Session::new(
            source(),
            22050,
            &clipped_path,
            &score,
            Settings {
                overflow: Overflow::Clip,
                ..loud
            },
        )
        .unwrap();
        clipped.render().collect::<Result<Vec<_>>>().unwrap();
        let clip_overflows = clipped.overflows();
        clipped.finish().unwrap();

        assert!(wrap_overflows > 0, "a gain of 40 must overflow");
        assert!(clip_overflows > 0, "clipping must report what it clipped");

        let (wrap, _) = read_audio(&wrapped_path).unwrap();
        let (clip, _) = read_audio(&clipped_path).unwrap();
        assert_eq!(wrap.len(), clip.len());
        assert_ne!(wrap, clip, "the two must not produce the same file");

        // Clipping pins a loud sample to the rail. Wrapping folds it back to
        // somewhere arbitrary, so far fewer samples end up at the rail.
        let at_rail = |v: &[f32]| v.iter().filter(|s| s.abs() > 0.999).count();
        assert!(
            at_rail(&clip) > at_rail(&wrap) * 4,
            "clip held {} at the rail, wrap held {}",
            at_rail(&clip),
            at_rail(&wrap)
        );
    }

    #[test]
    fn an_input_that_is_too_short_is_refused() {
        let path = scratch("short.aiff");
        let score = built_in("flowing").unwrap();
        let err = match Session::new(vec![0.0; 10], 22050, &path, &score, settings(1.0, 1)) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("a 10 sample input must be refused"),
        };
        assert!(err.contains("too short to take grains from"), "{err}");
    }

    #[test]
    fn a_seed_reproduces_the_whole_file() {
        let render = |name: &str, seed: u64| {
            let path = scratch(name);
            let score = built_in("hectic").unwrap();
            let mut s = Session::new(source(), 22050, &path, &score, settings(2.0, seed)).unwrap();
            s.render().collect::<Result<Vec<_>>>().unwrap();
            s.finish().unwrap();
            std::fs::read(&path).unwrap()
        };
        assert_eq!(render("seed_a.aiff", 99), render("seed_b.aiff", 99));
        assert_ne!(render("seed_a.aiff", 99), render("seed_c.aiff", 100));
    }

    #[test]
    fn a_render_left_unfinished_is_still_playable() {
        let path = scratch("abandoned.aiff");
        let score = built_in("flowing").unwrap();
        let mut s = Session::new(source(), 22050, &path, &score, settings(600.0, 6)).unwrap();
        {
            let mut render = s.render();
            render.next().unwrap().unwrap();
            render.next().unwrap().unwrap();
            // The iterator is dropped part way through, as a panic would drop it.
        }
        drop(s);
        let (samples, _) = read_audio(&path).unwrap();
        assert_eq!(samples.len(), 2 * 22050);
    }

    #[test]
    fn a_seed_is_picked_when_none_is_given() {
        let score = built_in("flowing").unwrap();
        let a = Session::new(
            source(),
            22050,
            &scratch("rand_a.aiff"),
            &score,
            Settings {
                duration: Some(1.0),
                ..Settings::default()
            },
        )
        .unwrap();
        let b = Session::new(
            source(),
            22050,
            &scratch("rand_b.aiff"),
            &score,
            Settings {
                duration: Some(1.0),
                ..Settings::default()
            },
        )
        .unwrap();
        assert_ne!(a.seed, b.seed, "two runs must not pick the same seed");
    }

    #[test]
    fn the_output_rate_can_differ_from_the_input_rate() {
        let path = scratch("resampled.wav");
        let score = built_in("flowing").unwrap();
        let mut s = Session::new(
            source(),
            22050,
            &path,
            &score,
            Settings {
                rate: Some(44100),
                ..settings(2.0, 7)
            },
        )
        .unwrap();
        s.render().collect::<Result<Vec<_>>>().unwrap();
        s.finish().unwrap();

        let (samples, rate) = read_audio(&path).unwrap();
        assert_eq!(rate, 44100);
        assert_eq!(samples.len(), 2 * 44100);
    }
}
