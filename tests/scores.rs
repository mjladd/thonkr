//! Compare the Rust score constants against the Python `SCORES` dictionary.
//!
//! The five scores are hand-transcribed numbers, which is exactly the kind of
//! thing that goes wrong silently. `tests/make_fixtures.py` dumps
//! `thonk.SCORES` to `scores.json` and this compares every field against it.

use std::path::{Path, PathBuf};

use thonkr::score::{built_ins, ParamSpec, ScoreSpec};

#[derive(serde::Deserialize)]
struct Spec {
    range: [f64; 2],
    seg: [f64; 2],
    rand: [f64; 2],
}

#[derive(serde::Deserialize)]
struct Entry {
    name: String,
    description: String,
    duration: f64,
    stretch: bool,
    spread: f64,
    voices: usize,
    position: Spec,
    density: Spec,
    length: Spec,
    attack: Spec,
    transpose: Spec,
    balance: Spec,
}

fn python_scores() -> Vec<Entry> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scores.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}\nrun: python3 tests/make_fixtures.py",
            PathBuf::from(&path).display()
        )
    });
    serde_json::from_str(&text).expect("scores.json is not valid JSON")
}

fn same(name: &str, field: &str, got: ParamSpec, want: &Spec) {
    assert_eq!(got.range, want.range, "{name}.{field} range");
    assert_eq!(got.seg, want.seg, "{name}.{field} seg");
    assert_eq!(got.rand, want.rand, "{name}.{field} rand");
}

#[test]
fn every_built_in_score_matches_the_python_definition() {
    let python = python_scores();
    let rust = built_ins();
    assert_eq!(rust.len(), python.len(), "score count");

    for (want, (name, got)) in python.iter().zip(&rust) {
        assert_eq!(*name, want.name, "scores are in a different order");
        let got: &ScoreSpec = got;

        assert_eq!(got.description, want.description, "{name} description");
        assert_eq!(got.duration, want.duration, "{name} duration");
        assert_eq!(got.stretch, want.stretch, "{name} stretch");
        assert_eq!(got.spread, want.spread, "{name} spread");
        assert_eq!(got.voices, want.voices, "{name} voices");

        same(name, "position", got.position, &want.position);
        same(name, "density", got.density, &want.density);
        same(name, "length", got.length, &want.length);
        same(name, "attack", got.attack, &want.attack);
        same(name, "transpose", got.transpose, &want.transpose);
        same(name, "balance", got.balance, &want.balance);
    }
}
