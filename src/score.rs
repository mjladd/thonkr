//! Score definitions, and the built-in five.
//!
//! A score is the shape of a render: for each of the six parameters the manual
//! lists, the range it moves in, how far apart its breakpoints fall, and how
//! hard the randomizer rides on top. Phase 6 adds loading these from TOML and
//! overriding them from the command line.

use serde::{Deserialize, Serialize};

/// One parameter of a score.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct ParamSpec {
    /// The low and high bound the parameter moves between.
    pub range: [f64; 2],
    /// The shortest and longest gap between breakpoints, in seconds.
    pub seg: [f64; 2],
    /// The randomizer: its rate in hertz, and its depth as a fraction of the
    /// range. Either at zero turns it off.
    pub rand: [f64; 2],
}

impl ParamSpec {
    pub const fn new(range: [f64; 2], seg: [f64; 2], rand: [f64; 2]) -> Self {
        ParamSpec { range, seg, rand }
    }

    pub fn lo(&self) -> f64 {
        self.range[0]
    }

    pub fn hi(&self) -> f64 {
        self.range[1]
    }
}

/// A whole score.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ScoreSpec {
    /// One line about the character of the score, shown by `--list-scores`.
    pub description: String,
    /// The output length in seconds when `--duration` is absent.
    pub duration: f64,
    /// Walk the input from start to end across the render, rather than taking
    /// grains from wherever the position curve points.
    pub stretch: bool,
    /// Per-grain stereo scatter, 0 to 1.
    pub spread: f64,
    /// How many voices the statistical transposer runs.
    pub voices: usize,

    /// Where in the input file a grain is taken from, as a fraction.
    pub position: ParamSpec,
    /// Grains written per second.
    pub density: ParamSpec,
    /// Grain length in seconds.
    pub length: ParamSpec,
    /// The faded portion at each end of a grain, as a fraction of its length.
    pub attack: ParamSpec,
    /// Transposition in semitones.
    pub transpose: ParamSpec,
    /// Left-to-right placement of each grain, 0 to 1.
    pub balance: ParamSpec,
}

/// Where a score came from, for `--list-scores`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    BuiltIn,
    File(std::path::PathBuf),
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Origin::BuiltIn => write!(f, "built-in"),
            Origin::File(path) => write!(f, "{}", path.display()),
        }
    }
}

/// The built-in scores, in the order `--list-scores` prints them.
pub const BUILT_IN_NAMES: [&str; 5] = ["flowing", "hectic", "sparse", "stretch1", "stretch5"];

/// Every built-in score, in order.
pub fn built_ins() -> Vec<(&'static str, ScoreSpec)> {
    BUILT_IN_NAMES
        .iter()
        .map(|name| (*name, built_in(name).expect("every listed name is defined")))
        .collect()
}

/// One built-in score by name.
pub fn built_in(name: &str) -> Option<ScoreSpec> {
    let spec = match name {
        "flowing" => ScoreSpec {
            description: "long arcs, moderate density - the original's patient setting".into(),
            duration: 1200.0,
            stretch: false,
            spread: 0.35,
            voices: 4,
            density: ParamSpec::new([2.0, 250.0], [8.0, 60.0], [0.7, 0.10]),
            length: ParamSpec::new([0.020, 0.100], [10.0, 60.0], [0.5, 0.15]),
            attack: ParamSpec::new([0.15, 0.50], [12.0, 60.0], [0.3, 0.10]),
            position: ParamSpec::new([0.0, 1.0], [6.0, 45.0], [0.4, 0.03]),
            transpose: ParamSpec::new([-12.0, 12.0], [10.0, 60.0], [0.2, 0.05]),
            balance: ParamSpec::new([0.15, 0.85], [4.0, 30.0], [0.6, 0.20]),
        },
        "hectic" => ScoreSpec {
            description: "dense, fast-shifting, up to 6000 grains a second".into(),
            duration: 1200.0,
            stretch: false,
            spread: 0.8,
            voices: 4,
            density: ParamSpec::new([40.0, 6000.0], [0.5, 8.0], [4.0, 0.25]),
            length: ParamSpec::new([0.003, 0.060], [0.5, 8.0], [3.0, 0.30]),
            attack: ParamSpec::new([0.05, 0.45], [1.0, 10.0], [2.0, 0.25]),
            position: ParamSpec::new([0.0, 1.0], [0.4, 6.0], [3.0, 0.12]),
            transpose: ParamSpec::new([-24.0, 24.0], [0.8, 10.0], [1.5, 0.20]),
            balance: ParamSpec::new([0.0, 1.0], [0.3, 4.0], [5.0, 0.35]),
        },
        "sparse" => ScoreSpec {
            description: "isolated grains, wide silences, slow drift".into(),
            duration: 1200.0,
            stretch: false,
            spread: 0.5,
            voices: 3,
            density: ParamSpec::new([0.5, 25.0], [15.0, 90.0], [0.3, 0.20]),
            length: ParamSpec::new([0.040, 0.100], [15.0, 90.0], [0.2, 0.20]),
            attack: ParamSpec::new([0.25, 0.50], [20.0, 90.0], [0.2, 0.10]),
            position: ParamSpec::new([0.0, 1.0], [10.0, 60.0], [0.2, 0.05]),
            transpose: ParamSpec::new([-18.0, 7.0], [15.0, 90.0], [0.15, 0.08]),
            balance: ParamSpec::new([0.0, 1.0], [8.0, 45.0], [0.4, 0.30]),
        },
        "stretch1" => ScoreSpec {
            description: "the input file dragged across one minute".into(),
            duration: 60.0,
            stretch: true,
            spread: 0.4,
            voices: 2,
            density: ParamSpec::new([120.0, 900.0], [3.0, 15.0], [1.0, 0.10]),
            length: ParamSpec::new([0.030, 0.100], [5.0, 20.0], [0.5, 0.10]),
            attack: ParamSpec::new([0.30, 0.50], [5.0, 20.0], [0.3, 0.05]),
            position: ParamSpec::new([0.0, 0.02], [2.0, 10.0], [0.8, 0.40]),
            transpose: ParamSpec::new([-5.0, 5.0], [6.0, 25.0], [0.3, 0.10]),
            balance: ParamSpec::new([0.2, 0.8], [3.0, 15.0], [0.5, 0.25]),
        },
        "stretch5" => ScoreSpec {
            description: "the input file dragged across five minutes".into(),
            duration: 300.0,
            stretch: true,
            spread: 0.4,
            voices: 2,
            density: ParamSpec::new([150.0, 1200.0], [6.0, 30.0], [0.8, 0.10]),
            length: ParamSpec::new([0.040, 0.100], [8.0, 40.0], [0.4, 0.10]),
            attack: ParamSpec::new([0.30, 0.50], [8.0, 40.0], [0.2, 0.05]),
            position: ParamSpec::new([0.0, 0.01], [4.0, 20.0], [0.6, 0.40]),
            transpose: ParamSpec::new([-3.0, 3.0], [10.0, 45.0], [0.2, 0.10]),
            balance: ParamSpec::new([0.2, 0.8], [6.0, 30.0], [0.4, 0.25]),
        },
        _ => return None,
    };
    Some(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_five_built_ins_are_defined_and_in_order() {
        let scores = built_ins();
        assert_eq!(scores.len(), 5);
        let names: Vec<&str> = scores.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            vec!["flowing", "hectic", "sparse", "stretch1", "stretch5"]
        );
        assert!(built_in("glacial").is_none());
    }

    #[test]
    fn every_built_in_holds_together() {
        for (name, score) in built_ins() {
            assert!(!score.description.is_empty(), "{name} has no description");
            assert!(score.duration > 0.0, "{name} duration");
            assert!((1..=16).contains(&score.voices), "{name} voices");
            assert!((0.0..=1.0).contains(&score.spread), "{name} spread");

            for (field, p) in [
                ("position", score.position),
                ("density", score.density),
                ("length", score.length),
                ("attack", score.attack),
                ("transpose", score.transpose),
                ("balance", score.balance),
            ] {
                assert!(p.lo() <= p.hi(), "{name}.{field} range is backwards");
                assert!(p.seg[0] > 0.0, "{name}.{field} seg starts at zero");
                assert!(p.seg[0] <= p.seg[1], "{name}.{field} seg is backwards");
                assert!(p.rand[0] >= 0.0 && p.rand[1] >= 0.0, "{name}.{field} rand");
            }
        }
    }

    /// The bounds the manual quotes, and the ones the engine relies on.
    #[test]
    fn the_scores_stay_inside_the_limits_the_manual_sets() {
        for (name, score) in built_ins() {
            assert!(
                score.density.hi() <= 6000.0,
                "{name} exceeds 6000 grains a second"
            );
            assert!(score.density.lo() > 0.0, "{name} density reaches zero");
            assert!(
                score.length.hi() <= 0.100,
                "{name} exceeds a tenth of a second"
            );
            assert!(score.length.lo() > 0.0, "{name} length reaches zero");
            assert!(
                (0.0..=0.5).contains(&score.attack.lo()),
                "{name} attack low"
            );
            assert!(
                (0.0..=0.5).contains(&score.attack.hi()),
                "{name} attack high"
            );
            assert!(
                (0.0..=1.0).contains(&score.position.lo()),
                "{name} position low"
            );
            assert!(
                (0.0..=1.0).contains(&score.position.hi()),
                "{name} position high"
            );
            assert!(
                (0.0..=1.0).contains(&score.balance.lo()),
                "{name} balance low"
            );
            assert!(
                (0.0..=1.0).contains(&score.balance.hi()),
                "{name} balance high"
            );
        }
    }

    #[test]
    fn hectic_is_the_dense_one_and_sparse_is_the_thin_one() {
        let hectic = built_in("hectic").unwrap();
        let sparse = built_in("sparse").unwrap();
        assert_eq!(
            hectic.density.hi(),
            6000.0,
            "the manual's 600-layer maximum"
        );
        assert!(sparse.density.hi() < hectic.density.lo());
    }

    #[test]
    fn only_the_stretch_scores_walk_the_input() {
        for (name, score) in built_ins() {
            assert_eq!(score.stretch, name.starts_with("stretch"), "{name}");
        }
    }

    #[test]
    fn a_score_survives_a_round_trip_through_toml() {
        for (name, score) in built_ins() {
            let text = toml::to_string(&score).expect("serializes");
            let back: ScoreSpec = toml::from_str(&text).expect("parses");
            assert_eq!(back, score, "{name} changed on the way through TOML");
        }
    }
}

// --------------------------------------------------------------------------
// scores from files and from the command line
// --------------------------------------------------------------------------

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

// The scores in `scores/`, embedded at build time by `build.rs`. They are
// listed and used exactly like the five written in this file, so a downloaded
// binary needs no files beside it. This defines SHIPPED.
include!(concat!(env!("OUT_DIR"), "/shipped_scores.rs"));

/// The name of the score every default is taken from.
const DEFAULT_BASE: &str = "flowing";

/// The most breakpoints a randomizer may be asked for, as a rate in hertz.
const MAX_RAND_RATE: f64 = 1000.0;

/// The longest grain a score may ask for, in seconds.
const MAX_GRAIN: f64 = 1.0;

/// The most voices the transposer will run.
const MAX_VOICES: usize = 16;

/// One parameter as a file writes it. Every field is optional and falls back
/// to the same parameter of `flowing`.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawParam {
    range: Option<[f64; 2]>,
    seg: Option<[f64; 2]>,
    rand: Option<[f64; 2]>,
}

impl RawParam {
    fn onto(self, base: ParamSpec) -> ParamSpec {
        ParamSpec {
            range: self.range.unwrap_or(base.range),
            seg: self.seg.unwrap_or(base.seg),
            rand: self.rand.unwrap_or(base.rand),
        }
    }
}

/// One score as a file writes it. Every field is optional.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScore {
    description: Option<String>,
    duration: Option<f64>,
    stretch: Option<bool>,
    spread: Option<f64>,
    voices: Option<usize>,
    position: Option<RawParam>,
    density: Option<RawParam>,
    length: Option<RawParam>,
    attack: Option<RawParam>,
    transpose: Option<RawParam>,
    balance: Option<RawParam>,
}

impl RawScore {
    fn onto(self, base: ScoreSpec) -> ScoreSpec {
        ScoreSpec {
            description: self.description.unwrap_or(base.description),
            duration: self.duration.unwrap_or(base.duration),
            stretch: self.stretch.unwrap_or(base.stretch),
            spread: self.spread.unwrap_or(base.spread),
            voices: self.voices.unwrap_or(base.voices),
            position: opt(self.position, base.position),
            density: opt(self.density, base.density),
            length: opt(self.length, base.length),
            attack: opt(self.attack, base.attack),
            transpose: opt(self.transpose, base.transpose),
            balance: opt(self.balance, base.balance),
        }
    }
}

fn opt(raw: Option<RawParam>, base: ParamSpec) -> ParamSpec {
    raw.map(|r| r.onto(base)).unwrap_or(base)
}

/// A whole score file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScoreFile {
    #[serde(default)]
    scores: BTreeMap<String, RawScore>,
}

/// Every score this run can use, with where each one came from.
#[derive(Debug, Clone)]
pub struct Catalog {
    entries: Vec<(String, ScoreSpec, Origin)>,
}

impl Catalog {
    /// Load the scores written in this file, then the ones embedded from
    /// `scores/`, then each file named on the command line.
    ///
    /// A later definition replaces an earlier one of the same name, in place,
    /// so the listing order does not shift under an override.
    pub fn load(files: &[PathBuf]) -> Result<Self> {
        let mut catalog = Catalog {
            entries: built_ins()
                .into_iter()
                .map(|(name, spec)| (name.to_string(), spec, Origin::BuiltIn))
                .collect(),
        };
        for (name, text) in SHIPPED {
            // A score that ships inside the binary and does not parse is a
            // fault in the build, and the test suite fails on it.
            catalog.add_toml(text, Origin::BuiltIn, Path::new(name))?;
        }
        for path in files {
            catalog.add_file(path)?;
        }
        Ok(catalog)
    }

    /// The built-in scores alone.
    pub fn built_in_only() -> Self {
        Catalog::load(&[]).expect("the built-in scores always load")
    }

    fn add_file(&mut self, path: &Path) -> Result<()> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        self.add_toml(&text, Origin::File(path.to_path_buf()), path)
    }

    /// Add every score in one TOML document.
    ///
    /// `source` names the document in any error, and is the file path for a
    /// score read from disk or just the file name for an embedded one.
    fn add_toml(&mut self, text: &str, origin: Origin, source: &Path) -> Result<()> {
        let path = source;
        // Parsed in two steps, so a file that forgot its [scores.NAME] header
        // is told what the shape is rather than which field is unexpected.
        let table: toml::Table =
            toml::from_str(text).map_err(|e| Error::format(path, e.message().to_string()))?;
        if !table.contains_key("scores") {
            return Err(Error::format(
                path,
                "no scores here. Each one needs a [scores.NAME] table.",
            ));
        }
        let file: ScoreFile = toml::Value::Table(table)
            .try_into()
            .map_err(|e: toml::de::Error| Error::format(path, e.message().to_string()))?;
        if file.scores.is_empty() {
            return Err(Error::format(
                path,
                "no scores here. Each one needs a [scores.NAME] table.",
            ));
        }

        for (name, raw) in file.scores {
            // A score builds on the one it replaces, or on `flowing`.
            let base = self
                .get(&name)
                .cloned()
                .unwrap_or_else(|| built_in(DEFAULT_BASE).expect("flowing is defined"));
            let spec = raw.onto(base);
            let origin = origin.clone();
            match self.entries.iter_mut().find(|(n, _, _)| *n == name) {
                Some(entry) => {
                    entry.1 = spec;
                    entry.2 = origin;
                }
                None => self.entries.push((name, spec, origin)),
            }
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ScoreSpec> {
        self.entries
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|e| &e.1)
    }

    pub fn origin(&self, name: &str) -> Option<&Origin> {
        self.entries
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|e| &e.2)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ScoreSpec, &Origin)> {
        self.entries.iter().map(|(n, s, o)| (n.as_str(), s, o))
    }

    pub fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|(n, _, _)| n.as_str()).collect()
    }

    /// One score by name, or an error listing what is available.
    pub fn require(&self, name: &str) -> Result<ScoreSpec> {
        self.get(name).cloned().ok_or_else(|| Error::UnknownScore {
            name: name.to_string(),
            available: self.names().join(", "),
        })
    }
}

// --------------------------------------------------------------------------
// --set
// --------------------------------------------------------------------------

/// The parameter names `--set` accepts.
pub const PARAM_KEYS: [&str; 6] = [
    "position",
    "density",
    "length",
    "attack",
    "transpose",
    "balance",
];

/// The fields of a parameter that `--set` accepts.
const PARAM_FIELDS: [&str; 3] = ["range", "seg", "rand"];

/// The whole-score keys `--set` accepts.
pub const SCORE_KEYS: [&str; 4] = ["duration", "spread", "voices", "stretch"];

/// Apply one `KEY=VALUE` override to a score.
///
/// The key is either `<parameter>.<field>` or one of the whole-score names.
pub fn apply_set(score: &mut ScoreSpec, arg: &str) -> Result<()> {
    let Some((key, value)) = arg.split_once('=') else {
        return Err(Error::BadSet {
            arg: arg.to_string(),
            reason: "expected KEY=VALUE, for example density.range=1,400".to_string(),
        });
    };
    let key = key.trim();
    let value = value.trim();

    if let Some((param, field)) = key.split_once('.') {
        let pair = two_numbers(arg, key, value)?;
        let spec = param_mut(score, param).ok_or_else(|| bad_key(arg, key))?;
        match field {
            "range" => spec.range = pair,
            "seg" => spec.seg = pair,
            "rand" => spec.rand = pair,
            _ => return Err(bad_key(arg, key)),
        }
        return Ok(());
    }

    match key {
        "duration" => score.duration = one_number(arg, key, value)?,
        "spread" => score.spread = one_number(arg, key, value)?,
        "voices" => {
            let n = one_number(arg, key, value)?;
            if n.fract() != 0.0 || n < 0.0 {
                return Err(Error::BadSet {
                    arg: arg.to_string(),
                    reason: "voices takes a whole number, for example voices=3".to_string(),
                });
            }
            score.voices = n as usize;
        }
        "stretch" => {
            score.stretch = match value {
                "true" => true,
                "false" => false,
                _ => {
                    return Err(Error::BadSet {
                        arg: arg.to_string(),
                        reason: "stretch takes true or false".to_string(),
                    })
                }
            }
        }
        _ => return Err(bad_key(arg, key)),
    }
    Ok(())
}

fn param_mut<'a>(score: &'a mut ScoreSpec, name: &str) -> Option<&'a mut ParamSpec> {
    Some(match name {
        "position" => &mut score.position,
        "density" => &mut score.density,
        "length" => &mut score.length,
        "attack" => &mut score.attack,
        "transpose" => &mut score.transpose,
        "balance" => &mut score.balance,
        _ => return None,
    })
}

fn bad_key(arg: &str, key: &str) -> Error {
    let mut keys: Vec<String> = PARAM_KEYS
        .iter()
        .flat_map(|p| PARAM_FIELDS.iter().map(move |f| format!("{p}.{f}")))
        .collect();
    keys.extend(SCORE_KEYS.iter().map(|k| k.to_string()));
    Error::BadSet {
        arg: arg.to_string(),
        reason: format!("no such key: {key}\n  accepted keys: {}", keys.join(", ")),
    }
}

fn one_number(arg: &str, key: &str, value: &str) -> Result<f64> {
    value.trim().parse::<f64>().map_err(|_| Error::BadSet {
        arg: arg.to_string(),
        reason: format!("{key} takes one number, for example {key}=0.5"),
    })
}

fn two_numbers(arg: &str, key: &str, value: &str) -> Result<[f64; 2]> {
    let parts: Vec<&str> = value.split(',').collect();
    let shape = || Error::BadSet {
        arg: arg.to_string(),
        reason: format!("{key} takes two numbers separated by a comma, for example {key}=1,400"),
    };
    if parts.len() != 2 {
        return Err(shape());
    }
    let lo = parts[0].trim().parse::<f64>().map_err(|_| shape())?;
    let hi = parts[1].trim().parse::<f64>().map_err(|_| shape())?;
    Ok([lo, hi])
}

// --------------------------------------------------------------------------
// validation
// --------------------------------------------------------------------------

impl ScoreSpec {
    /// Every reason this score cannot be rendered, or an empty list.
    ///
    /// All the problems are collected rather than only the first, so one run
    /// tells the user everything that needs fixing.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();

        for (name, p) in [
            ("position", self.position),
            ("density", self.density),
            ("length", self.length),
            ("attack", self.attack),
            ("transpose", self.transpose),
            ("balance", self.balance),
        ] {
            if !p.range.iter().all(|v| v.is_finite())
                || !p.seg.iter().all(|v| v.is_finite())
                || !p.rand.iter().all(|v| v.is_finite())
            {
                out.push(format!("{name}: every number must be finite"));
                continue;
            }
            if p.lo() > p.hi() {
                out.push(format!(
                    "{name}.range: the low bound {} is above the high bound {}",
                    p.lo(),
                    p.hi()
                ));
            }
            if p.seg[0] <= 0.0 || p.seg[1] <= 0.0 {
                out.push(format!("{name}.seg: both times must be greater than 0"));
            } else if p.seg[0] > p.seg[1] {
                out.push(format!(
                    "{name}.seg: the shortest gap {} is longer than the longest gap {}",
                    p.seg[0], p.seg[1]
                ));
            }
            if p.rand[0] < 0.0 || p.rand[1] < 0.0 {
                out.push(format!(
                    "{name}.rand: the rate and depth cannot be negative"
                ));
            }
            if p.rand[0] > MAX_RAND_RATE {
                out.push(format!(
                    "{name}.rand: a rate of {} Hz is above the limit of {MAX_RAND_RATE} Hz",
                    p.rand[0]
                ));
            }
        }

        if self.density.lo() <= 0.0 {
            out.push("density.range: the low bound must be greater than 0".to_string());
        }
        if self.length.lo() <= 0.0 {
            out.push("length.range: the low bound must be greater than 0".to_string());
        }
        if self.length.hi() > MAX_GRAIN {
            out.push(format!(
                "length.range: a grain of {} s is above the limit of {MAX_GRAIN} s",
                self.length.hi()
            ));
        }
        inside("attack.range", self.attack, 0.0, 0.5, &mut out);
        inside("position.range", self.position, 0.0, 1.0, &mut out);
        inside("balance.range", self.balance, 0.0, 1.0, &mut out);

        if !self.spread.is_finite() || !(0.0..=1.0).contains(&self.spread) {
            out.push(format!("spread: {} is outside 0 to 1", self.spread));
        }
        if !(1..=MAX_VOICES).contains(&self.voices) {
            out.push(format!(
                "voices: {} is outside 1 to {MAX_VOICES}",
                self.voices
            ));
        }
        if !self.duration.is_finite() || self.duration <= 0.0 {
            out.push(format!(
                "duration: {} must be greater than 0",
                self.duration
            ));
        }
        out
    }

    /// Fail with every problem at once, or pass.
    pub fn validate(&self, name: &str) -> Result<()> {
        let problems = self.problems();
        if problems.is_empty() {
            Ok(())
        } else {
            Err(Error::Invalid {
                name: name.to_string(),
                problems,
            })
        }
    }

    /// The score as TOML, in the form a score file takes.
    pub fn to_toml(&self, name: &str) -> String {
        let param = |field: &str, p: ParamSpec| {
            format!(
                "{field:<9} = {{ range = [{}, {}], seg = [{}, {}], rand = [{}, {}] }}\n",
                p.range[0], p.range[1], p.seg[0], p.seg[1], p.rand[0], p.rand[1]
            )
        };
        let mut out = format!("[scores.{name}]\n");
        out.push_str(&format!(
            "description = {}\n",
            toml_string(&self.description)
        ));
        out.push_str(&format!("duration    = {}\n", self.duration));
        out.push_str(&format!("stretch     = {}\n", self.stretch));
        out.push_str(&format!("spread      = {}\n", self.spread));
        out.push_str(&format!("voices      = {}\n", self.voices));
        out.push_str(&param("position", self.position));
        out.push_str(&param("density", self.density));
        out.push_str(&param("length", self.length));
        out.push_str(&param("attack", self.attack));
        out.push_str(&param("transpose", self.transpose));
        out.push_str(&param("balance", self.balance));
        out
    }
}

fn inside(name: &str, p: ParamSpec, lo: f64, hi: f64, out: &mut Vec<String>) {
    if p.lo() < lo || p.hi() > hi {
        out.push(format!(
            "{name}: [{}, {}] is outside {lo} to {hi}",
            p.lo(),
            p.hi()
        ));
    }
}

/// A string as TOML writes it.
fn toml_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}
