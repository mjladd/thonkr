//! Command line front end.
//!
//! Phase 1 parses and validates every argument, resolves what the run will do,
//! and reports it. The render path arrives in phase 5 and the score files in
//! phase 6. See `archive/RUST_MIGRATION.md`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering;

use clap::{CommandFactory, Parser};

use thonkr::session::{Overflow, Progress, Session, Settings};
use thonkr::{hms, score};

#[derive(Parser, Debug)]
#[command(
    name = "thonkr",
    version,
    about = "thOnk_0+2's granular engine, reimplemented for modern systems.",
    // argparse accepts a negative number as a value; clap must be told to.
    allow_negative_numbers = true,
    after_help = "Feed it a short, transient-rich mono file and leave it running.\n\
                  Stop any time with Ctrl-C. The output file stays playable."
)]
pub struct Cli {
    /// Input sound file (AIFF, AIFF-C or WAV).
    pub input: Option<PathBuf>,

    /// Output file. The extension picks the format: .aiff or .wav.
    pub output: Option<PathBuf>,

    /// Score to run.
    #[arg(long, value_name = "NAME", default_value = "flowing")]
    pub score: String,

    /// Output length in seconds. The default is the score's own length.
    #[arg(long, value_name = "SEC")]
    pub duration: Option<f64>,

    /// Output sample rate. The default is the input's rate.
    #[arg(long, value_name = "HZ")]
    pub rate: Option<u32>,

    /// Output gain.
    #[arg(long, value_name = "G", default_value_t = 1.0)]
    pub gain: f64,

    /// What to do past full scale. The original wrapped.
    #[arg(long, value_enum, default_value_t = Overflow::Wrap)]
    pub overflow: Overflow,

    /// Measure each block and back the gain off to hold 0.9 of full scale.
    #[arg(long)]
    pub autogain: bool,

    /// Fix the random seed. Omit it for a different result every run.
    #[arg(long, value_name = "N")]
    pub seed: Option<u64>,

    /// Per-grain stereo scatter, 0 to 1. The default comes from the score.
    #[arg(long, value_name = "S")]
    pub spread: Option<f64>,

    /// No progress console.
    #[arg(long)]
    pub quiet: bool,

    /// Load extra scores from a TOML file. Repeatable.
    #[arg(long = "score-file", value_name = "PATH")]
    pub score_file: Vec<PathBuf>,

    /// Override one field of the active score, as KEY=VALUE. Repeatable.
    /// The only way to reach `voices` and `stretch`, which have no flag of their own.
    #[arg(long = "set", value_name = "KEY=VALUE")]
    pub set: Vec<String>,

    /// List the scores and exit.
    #[arg(long = "list-scores")]
    pub list_scores: bool,

    /// Print one score as TOML and exit.
    #[arg(long = "dump-score", value_name = "NAME")]
    pub dump_score: Option<String>,

    /// Repair the header of a truncated output file and exit.
    #[arg(long, value_name = "FILE")]
    pub fix: Option<PathBuf>,
}

/// What this run will do, once the arguments agree on one thing.
#[derive(Debug)]
pub enum Mode {
    Fix(PathBuf),
    ListScores,
    DumpScore(String),
    Render { input: PathBuf, output: PathBuf },
}

impl Cli {
    /// Pick the mode and reject the argument combinations that contradict.
    ///
    /// `--fix`, `--list-scores` and `--dump-score` each stand alone, and they
    /// take the same precedence order the Python version used.
    pub fn mode(&self) -> Result<Mode, String> {
        let standalone = [
            self.fix.is_some(),
            self.list_scores,
            self.dump_score.is_some(),
        ]
        .iter()
        .filter(|on| **on)
        .count();
        if standalone > 1 {
            return Err("--fix, --list-scores and --dump-score cannot be combined".to_string());
        }
        if let Some(path) = &self.fix {
            return Ok(Mode::Fix(path.clone()));
        }
        if self.list_scores {
            return Ok(Mode::ListScores);
        }
        if let Some(name) = &self.dump_score {
            return Ok(Mode::DumpScore(name.clone()));
        }
        match (&self.input, &self.output) {
            (Some(input), Some(output)) => Ok(Mode::Render {
                input: input.clone(),
                output: output.clone(),
            }),
            _ => Err("an input and an output file are required".to_string()),
        }
    }

    /// Range checks that do not need a score loaded.
    pub fn validate(&self) -> Result<(), String> {
        if !(self.gain.is_finite() && self.gain > 0.0) {
            return Err(format!("--gain must be greater than 0 (got {})", self.gain));
        }
        if let Some(d) = self.duration {
            if !(d.is_finite() && d > 0.0) {
                return Err(format!("--duration must be greater than 0 (got {d})"));
            }
        }
        if let Some(r) = self.rate {
            if !(1000..=768_000).contains(&r) {
                return Err(format!("--rate must be between 1000 and 768000 (got {r})"));
            }
        }
        if let Some(s) = self.spread {
            if !(s.is_finite() && (0.0..=1.0).contains(&s)) {
                return Err(format!("--spread must be between 0 and 1 (got {s})"));
            }
        }
        Ok(())
    }
}

fn main() -> ExitCode {
    restore_default_sigpipe();
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            Cli::command()
                .error(clap::error::ErrorKind::ValueValidation, message)
                .exit();
        }
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    cli.validate()?;
    let mode = cli.mode()?;

    // Phase 1 stops here. Each arm reports what the finished program will do,
    // so the argument handling can be exercised before the engine exists.
    match mode {
        Mode::Fix(path) => {
            let repair = thonkr::audio::fix::fix_header(&path).map_err(|e| e.to_string())?;
            match repair.frames {
                Some(frames) => println!(
                    "fixed {}: {} bytes, {frames} frames",
                    path.display(),
                    repair.size
                ),
                None => println!("fixed {}: {} bytes", path.display(), repair.size),
            }
        }
        Mode::ListScores => {
            let catalog = catalog(cli)?;
            for (name, score, origin) in catalog.iter() {
                let problems = score.problems().len();
                let note = match problems {
                    0 => origin.to_string(),
                    1 => format!("{origin} - unusable, 1 problem"),
                    n => format!("{origin} - unusable, {n} problems"),
                };
                println!(
                    "{:<10} {:<58} default {:>8}  {}",
                    name,
                    score.description,
                    hms(score.duration),
                    note
                );
            }
            println!();
            println!("every score above is inside this program; nothing else is needed.");
            println!("to write your own, start from one of them:");
            println!("  thonkr --dump-score sparse > mine.toml");
            println!("  thonkr in.aiff out.aiff --score-file mine.toml --score mine");
        }
        Mode::DumpScore(name) => {
            let catalog = catalog(cli)?;
            let mut score = catalog.require(&name).map_err(|e| e.to_string())?;
            // --set applies to the dump too, so a tweak can be saved as a file.
            for arg in &cli.set {
                score::apply_set(&mut score, arg).map_err(|e| e.to_string())?;
            }
            print!("{}", score.to_toml(&name));
        }
        Mode::Render { input, output } => {
            // Resolve the score before touching the disk, so a mistake in a
            // name, a --set or a score file fails without a wasted read.
            let catalog = catalog(cli)?;
            let score = resolve(cli, &catalog)?;
            let (samples, in_rate) =
                thonkr::audio::read::read_audio(&input).map_err(|e| e.to_string())?;

            let settings = Settings {
                duration: cli.duration,
                rate: cli.rate,
                gain: cli.gain,
                overflow: cli.overflow,
                autogain: cli.autogain,
                seed: cli.seed,
                spread: cli.spread,
                ..Settings::default()
            };
            let mut session = Session::new(samples, in_rate, &output, &score, settings)
                .map_err(|e| e.to_string())?;

            if !cli.quiet {
                println!("thonkr: {} -> {}", input.display(), output.display());
                println!(
                    "  input     {:.2} s mono, {} Hz, peak {:.2}",
                    session.source_len as f64 / f64::from(session.in_rate.max(1)),
                    session.in_rate,
                    session.peak
                );
                match catalog.origin(&cli.score) {
                    Some(score::Origin::BuiltIn) | None => {
                        println!("  score     {} - {}", cli.score, score.description)
                    }
                    Some(origin) => println!(
                        "  score     {} - {} (from {origin})",
                        cli.score, score.description
                    ),
                }
                for arg in &cli.set {
                    println!("  set       {arg}");
                }
                println!(
                    "  output    {} stereo, {} Hz, seed {}",
                    hms(session.duration),
                    session.rate,
                    session.seed
                );
                println!("  stop any time with Ctrl-C; the file stays playable");
                println!();
            }

            // Ctrl-C sets the flag. The render checks it once per block, and
            // the output file is finalized on the way out either way.
            let flag = session.stop_flag();
            if let Err(e) = ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed)) {
                eprintln!("warning: Ctrl-C will not stop the render cleanly ({e})");
            }

            let mut last_print = 0.0;
            let mut failure = None;
            for step in session.render() {
                match step {
                    Ok(progress) => {
                        if !cli.quiet && progress.elapsed - last_print > 0.5 {
                            last_print = progress.elapsed;
                            print_progress(&progress);
                        }
                    }
                    Err(e) => {
                        failure = Some(e.to_string());
                        break;
                    }
                }
            }

            let stopped = session.stop_requested();
            let written = session.written_seconds();
            let grains = session.grains();
            let overflows = session.overflows();
            let elapsed = session.elapsed();
            session.finish().map_err(|e| e.to_string())?;

            if let Some(message) = failure {
                return Err(message);
            }
            if !cli.quiet {
                report(
                    &output,
                    written,
                    grains,
                    elapsed,
                    overflows,
                    cli.overflow,
                    stopped,
                );
            }
        }
    }
    Ok(())
}

/// Load every score this run can use.
fn catalog(cli: &Cli) -> Result<score::Catalog, String> {
    score::Catalog::load(&cli.score_file).map_err(|e| e.to_string())
}

/// The score the run will render, after every `--set` is applied.
fn resolve(cli: &Cli, catalog: &score::Catalog) -> Result<score::ScoreSpec, String> {
    let mut score = catalog.require(&cli.score).map_err(|e| e.to_string())?;
    for arg in &cli.set {
        score::apply_set(&mut score, arg).map_err(|e| e.to_string())?;
    }
    score.validate(&cli.score).map_err(|e| e.to_string())?;
    Ok(score)
}

/// One line of progress, rewritten in place.
fn print_progress(p: &Progress) {
    print!(
        "\r  {} / {}   {:6.0} grains/s   {:9} grains   {:4.1}x realtime  ",
        hms(p.t),
        hms(p.duration),
        p.density,
        p.grains,
        p.speed()
    );
    let _ = std::io::stdout().flush();
}

/// What the render produced, once it is over.
fn report(
    output: &Path,
    written: f64,
    grains: u64,
    elapsed: f64,
    overflows: u64,
    overflow: Overflow,
    stopped: bool,
) {
    println!("\r{:78}", " ");
    println!(
        "  wrote {} of audio ({grains} grains) in {}, {:.1}x realtime",
        hms(written),
        hms(elapsed),
        written / elapsed.max(1e-9)
    );
    if overflows > 0 {
        match overflow {
            Overflow::Clip => println!(
                "  {overflows} samples hit full scale and were clipped; try a lower --gain"
            ),
            Overflow::Wrap => {
                println!(
                    "  {overflows} samples went past full scale and wrapped, as the original did;"
                );
                println!("  attenuate the input, or use --gain 0.3, or --overflow clip");
            }
        }
    }
    if stopped {
        println!(
            "  stopped early - {} is complete and playable",
            output.display()
        );
    }
}

/// Die quietly when the reader of a pipe goes away, as every other tool does.
///
/// Rust ignores SIGPIPE at startup, so a closed pipe surfaces as a panic out of
/// `println!`. Running `thonkr --list-scores | head` must not look like a crash.
fn restore_default_sigpipe() {
    #[cfg(unix)]
    // SAFETY: setting a signal disposition to the default is always allowed,
    // and this runs before any thread is started.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Mode, Overflow};
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        Cli::parse_from(std::iter::once("thonkr").chain(args.iter().copied()))
    }

    #[test]
    fn defaults_match_the_python_version() {
        let cli = parse(&["in.aiff", "out.aiff"]);
        assert_eq!(cli.score, "flowing");
        assert_eq!(cli.gain, 1.0);
        assert_eq!(cli.overflow, Overflow::Wrap);
        assert!(!cli.autogain);
        assert!(cli.seed.is_none());
        assert!(cli.duration.is_none());
        assert!(cli.rate.is_none());
    }

    #[test]
    fn render_needs_both_files() {
        assert!(parse(&["in.aiff"]).mode().is_err());
        assert!(matches!(
            parse(&["in.aiff", "out.wav"]).mode().unwrap(),
            Mode::Render { .. }
        ));
    }

    #[test]
    fn standalone_modes_take_precedence_and_do_not_combine() {
        assert!(matches!(
            parse(&["--fix", "broken.aiff"]).mode().unwrap(),
            Mode::Fix(_)
        ));
        assert!(matches!(
            parse(&["--list-scores"]).mode().unwrap(),
            Mode::ListScores
        ));
        assert!(matches!(
            parse(&["--dump-score", "hectic"]).mode().unwrap(),
            Mode::DumpScore(_)
        ));
        assert!(parse(&["--list-scores", "--fix", "x.aiff"]).mode().is_err());
    }

    #[test]
    fn repeatable_flags_collect_every_value() {
        let cli = parse(&[
            "in.aiff",
            "out.aiff",
            "--score-file",
            "a.toml",
            "--score-file",
            "b.toml",
            "--set",
            "density.range=1,400",
            "--set",
            "spread=0.6",
        ]);
        assert_eq!(cli.score_file.len(), 2);
        assert_eq!(cli.set, vec!["density.range=1,400", "spread=0.6"]);
    }

    #[test]
    fn out_of_range_numbers_are_rejected() {
        assert!(parse(&["in.aiff", "out.aiff", "--gain", "0"])
            .validate()
            .is_err());
        assert!(parse(&["in.aiff", "out.aiff", "--spread", "1.5"])
            .validate()
            .is_err());
        assert!(parse(&["in.aiff", "out.aiff", "--duration", "-1"])
            .validate()
            .is_err());
        assert!(parse(&["in.aiff", "out.aiff", "--rate", "10"])
            .validate()
            .is_err());
        assert!(parse(&["in.aiff", "out.aiff", "--rate", "44100"])
            .validate()
            .is_ok());
    }

    #[test]
    fn the_command_definition_is_self_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
