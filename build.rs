//! Embed the scores in `scores/` into the binary.
//!
//! A downloaded binary carries its scores with it, so nothing has to be
//! cloned, unpacked next to it, or kept track of. `scores/` stays the source
//! of those scores, and still ships in the release archive as a starting
//! point for editing.

use std::fmt::Write as _;
use std::path::PathBuf;

/// The documented worked example. It teaches the file format rather than being
/// a score worth shipping, so it is not embedded.
const EXAMPLE: &str = "example.toml";

fn main() {
    println!("cargo:rerun-if-changed=scores");

    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("scores"))
        .expect("the scores directory must exist")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .filter(|path| path.file_name().is_some_and(|n| n != EXAMPLE))
        .collect();
    files.sort();

    let mut out = String::from(
        "/// The score files embedded at build time, as (file name, contents).\n\
         pub static SHIPPED: &[(&str, &str)] = &[\n",
    );
    for path in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path
            .file_name()
            .expect("a file")
            .to_string_lossy()
            .to_string();
        writeln!(out, "    ({name:?}, include_str!({:?})),", path.display())
            .expect("writing to a String cannot fail");
    }
    out.push_str("];\n");

    let target = PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo"));
    std::fs::write(target.join("shipped_scores.rs"), out).expect("writing the generated file");
}
