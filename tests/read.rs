//! Compare the Rust reader against the Python reader on real files.
//!
//! `tests/make_fixtures.py` writes both the sound files and `expected.json`,
//! and the expected samples come out of `thonk.py` itself. Regenerate them with
//! `python3 tests/make_fixtures.py` after any change to either reader.

use std::path::{Path, PathBuf};

use thonkr::audio::read::{decode, read_audio};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[derive(serde::Deserialize)]
struct Case {
    file: String,
    rate: u32,
    samples: Vec<f64>,
}

fn cases() -> Vec<Case> {
    let path = fixture_dir().join("expected.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}\nrun: python3 tests/make_fixtures.py",
            path.display()
        )
    });
    serde_json::from_str(&text).expect("expected.json is not valid JSON")
}

#[test]
fn every_fixture_decodes_the_way_python_decodes_it() {
    let cases = cases();
    assert!(cases.len() >= 15, "expected.json looks incomplete");

    for case in cases {
        let path = fixture_dir().join(&case.file);
        let (samples, rate) = read_audio(&path).unwrap_or_else(|e| panic!("{}: {e}", case.file));

        assert_eq!(rate, case.rate, "sample rate of {}", case.file);
        assert_eq!(
            samples.len(),
            case.samples.len(),
            "sample count of {}",
            case.file
        );
        for (i, (got, want)) in samples.iter().zip(&case.samples).enumerate() {
            let diff = (f64::from(*got) - want).abs();
            assert!(
                diff < 1e-6,
                "{} sample {i}: got {got}, python got {want}",
                case.file
            );
        }
    }
}

#[test]
fn the_extremes_survive_every_width() {
    // Every fixture holds the same wave, so -1.0 and a near +1.0 must appear.
    for name in [
        "mono16.aiff",
        "mono24.aiff",
        "mono32.aiff",
        "mono16.wav",
        "mono8.wav",
    ] {
        let (samples, _) = read_audio(&fixture_dir().join(name)).unwrap();
        let low = samples.iter().cloned().fold(f32::INFINITY, f32::min);
        let high = samples.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (low + 1.0).abs() < 1e-6,
            "{name} lost its negative full scale"
        );
        assert!(high > 0.99, "{name} lost its positive peak, got {high}");
    }
}

#[test]
fn stereo_files_come_back_half_as_long() {
    let (mono, _) = read_audio(&fixture_dir().join("mono16.aiff")).unwrap();
    let (stereo, _) = read_audio(&fixture_dir().join("stereo16.aiff")).unwrap();
    assert_eq!(stereo.len(), mono.len());
    // The fixture puts x in the left channel and -x in the right, so the two
    // cancel. They cancel to within one step of 16-bit quantization rather than
    // exactly, because +1.0 clips to 32767 while -1.0 reaches -32768.
    let one_step = 1.0 / 32768.0;
    for v in &stereo {
        assert!(v.abs() <= one_step, "channels should cancel, got {v}");
    }
}

#[test]
fn a_truncated_file_reads_as_far_as_it_goes() {
    // The COMM frame count claims more than the file holds. The data wins.
    let full = std::fs::read(fixture_dir().join("mono16.aiff")).unwrap();
    let cut = &full[..full.len() - 6];
    let (samples, rate) = decode(Path::new("cut.aiff"), cut).unwrap();
    assert_eq!(rate, 44100);
    assert_eq!(samples.len(), 5, "3 whole frames were cut from 8");
}

#[test]
fn compressed_aiff_c_is_refused_by_name() {
    let mut bytes = std::fs::read(fixture_dir().join("sowt16.aifc")).unwrap();
    let at = bytes
        .windows(4)
        .position(|w| w == b"sowt")
        .expect("the fixture carries a sowt tag");
    bytes[at..at + 4].copy_from_slice(b"ima4");
    let err = decode(Path::new("bad.aifc"), &bytes)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("compressed AIFF-C (ima4) is not supported"),
        "{err}"
    );
}

#[test]
fn an_unknown_wav_encoding_names_its_tag() {
    let mut bytes = std::fs::read(fixture_dir().join("mono16.wav")).unwrap();
    let at = bytes.windows(4).position(|w| w == b"fmt ").unwrap() + 8;
    bytes[at..at + 2].copy_from_slice(&0x0011u16.to_le_bytes()); // IMA ADPCM
    let err = decode(Path::new("bad.wav"), &bytes)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("unsupported WAV encoding (format tag 17)"),
        "{err}"
    );
}

#[test]
fn a_missing_chunk_is_reported_rather_than_guessed() {
    let mut bytes = std::fs::read(fixture_dir().join("mono16.wav")).unwrap();
    let at = bytes.windows(4).position(|w| w == b"data").unwrap();
    bytes[at..at + 4].copy_from_slice(b"junk");
    let err = decode(Path::new("bad.wav"), &bytes)
        .unwrap_err()
        .to_string();
    assert!(err.contains("missing a fmt or data chunk"), "{err}");
}
