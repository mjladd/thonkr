//! Compare the Rust writer and header repair against the Python ones, byte for
//! byte.
//!
//! `tests/make_fixtures.py` writes the same files with `thonk.StreamWriter` and
//! `thonk.fix_header`. Regenerate them with `python3 tests/make_fixtures.py`
//! after any change to either side.

use std::path::{Path, PathBuf};

use thonkr::audio::fix::fix_header;
use thonkr::audio::write::StreamWriter;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[derive(serde::Deserialize)]
struct Case {
    file: String,
    rate: u32,
    frames: usize,
    #[serde(default)]
    overflows: u64,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    fixed: Option<String>,
    #[serde(default)]
    fixed_frames: Option<u64>,
}

fn cases() -> Vec<Case> {
    let path = fixture_dir().join("writer.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}\nrun: python3 tests/make_fixtures.py",
            path.display()
        )
    });
    serde_json::from_str(&text).expect("writer.json is not valid JSON")
}

/// The block both writers render. Every value is a whole multiple of 1/128, so
/// neither side has to round, and the range drives the wrap on both sides.
fn test_block(n: usize) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let left = ((i % 513) as f32 - 256.0) / 128.0;
            [left, -left]
        })
        .collect()
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("thonkr-write-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn every_written_file_matches_python_byte_for_byte() {
    let mut checked = 0;
    for case in cases() {
        if case.fixed.is_some() {
            continue; // handled by the repair test
        }
        let want = std::fs::read(fixture_dir().join(&case.file)).unwrap();
        assert_eq!(want.len() as u64, case.size, "{} fixture size", case.file);

        let path = scratch().join(&case.file);
        let mut w = StreamWriter::create(&path, case.rate).unwrap();
        let block = test_block(case.frames);
        let mut over = 0;
        // Write in several pieces, as a render does.
        for piece in block.chunks(128) {
            over += w.write(piece).unwrap();
        }
        assert_eq!(w.frames(), case.frames as u64, "{}", case.file);
        w.close().unwrap();

        let got = std::fs::read(&path).unwrap();
        assert_eq!(over, case.overflows, "overflow count of {}", case.file);
        assert_eq!(got.len(), want.len(), "size of {}", case.file);
        if got != want {
            let at = got.iter().zip(&want).position(|(a, b)| a != b).unwrap();
            panic!(
                "{} differs at byte {at}: got {:?}, python {:?}",
                case.file, got[at], want[at]
            );
        }
        checked += 1;
    }
    assert!(checked >= 5, "only {checked} writer fixtures were checked");
}

#[test]
fn a_repair_lands_on_the_same_bytes_python_produces() {
    let mut checked = 0;
    for case in cases() {
        let Some(fixed_name) = case.fixed else {
            continue;
        };
        // Start from the crashed file and repair a copy of it.
        let crashed = std::fs::read(fixture_dir().join(&case.file)).unwrap();
        let path = scratch().join(format!("repair_{}", case.file));
        std::fs::write(&path, &crashed).unwrap();

        let repair = fix_header(&path).unwrap();
        assert_eq!(repair.size, case.size, "{} size", case.file);
        assert_eq!(repair.frames, case.fixed_frames, "{} frames", case.file);

        let want = std::fs::read(fixture_dir().join(&fixed_name)).unwrap();
        let got = std::fs::read(&path).unwrap();
        if got != want {
            let at = got.iter().zip(&want).position(|(a, b)| a != b).unwrap();
            panic!(
                "{fixed_name} differs at byte {at}: got {:?}, python {:?}",
                got[at], want[at]
            );
        }

        // The repaired file must hold every frame the crash left behind.
        let (samples, rate) = thonkr::audio::read::read_audio(&path).unwrap();
        assert_eq!(rate, case.rate);
        assert_eq!(samples.len(), case.frames);
        checked += 1;
    }
    assert_eq!(checked, 2, "both repair fixtures must be checked");
}

#[test]
fn a_render_that_never_closes_is_still_playable() {
    let path = scratch().join("abandoned.aiff");
    {
        let mut w = StreamWriter::create(&path, 44100).unwrap();
        for piece in test_block(1000).chunks(100) {
            w.write(piece).unwrap();
        }
        // The writer is dropped without close, as a panic would drop it.
    }
    let (samples, rate) = thonkr::audio::read::read_audio(&path).unwrap();
    assert_eq!(rate, 44100);
    assert_eq!(samples.len(), 1000);
}
