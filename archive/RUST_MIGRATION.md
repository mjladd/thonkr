# Migration plan: thonk.py to Rust

Status: all eight phases complete. The port is finished.

Progress: 8 of 8 phases complete. One step is left for a human to run, and it
is marked below.

The program is `thonkr`, and the repository is
https://github.com/mjladd/thonkr. Phases 1 to 7 were built under the name
`thonk` and renamed in phase 8.

## Scope

The Rust program is a command line renderer. It keeps the engine, the file
reading, the streaming writer, the progress console, and the `--fix` header
repair. It drops `thonk_gui.py`, `build_app.sh`, `thOnk.app`, the icon, and the
three zip archives.

`thonk.py` and `thonk_gui.py` stay in the repository as the reference
implementation. The parity harness in phase 7 reads them.

## Decisions already made

A seed does not reproduce the Python output. Rust uses `rand_pcg::Pcg64` seeded
from the `--seed` value. A seed reproduces a Rust render exactly. Matching numpy
would mean cloning the `Generator.uniform` and `Generator.random` draw order
forever, and it constrains every future change to the loop structure.

Draw order still matters inside Rust. Fix it once in `engine.rs` and treat a
change to it as a change that breaks saved seeds.

## Crate layout

```
Cargo.toml
src/
  main.rs        command line parsing, signal handler, progress console
  lib.rs         re-exports
  audio/
    read.rs      AIFF, AIFF-C and WAV reader, 80-bit extended float decode
    write.rs     StreamWriter, 16-bit stereo AIFF and WAV
    fix.rs       fix_header
  curve.rs       Curve and Parameter
  score.rs       ScoreSpec, built-in scores, TOML loading, --set overrides
  engine.rs      Thonk
  session.rs     Session and Progress
tests/
  fixtures/      small AIFF and WAV files written by the Python version
  parity/        the A2B harness and the numbers it recorded
scores/          extra scores, tracked and shipped in the release archive
  example.toml   a documented starting point for the new feature
.github/
  workflows/
    ci.yml       fmt, clippy and tests on every push
    release.yml  archives for macOS and Linux, attached to a tag
```

Dependencies: `clap` with the derive feature, `serde` with derive, `toml`,
`rand`, `rand_pcg`, `thiserror`, `ctrlc`, and `dirs`. No audio crate is needed.
`hound` handles WAV only, and the Python code already writes both formats by
hand.

---

## Phase 1: skeleton

- [x] Write `Cargo.toml` with the dependency list above.
- [x] Create the module tree from the crate layout.
- [x] Define the error type in `lib.rs` with `thiserror`.
- [x] Parse every argument in `main.rs` and exit without rendering.
- [x] Make sure that `cargo build` and `cargo clippy` pass clean.

`clap` rejects `--duration -1`, because it reads a leading minus as a flag.
Python argparse accepts it. The command sets `allow_negative_numbers = true` to
keep the two the same.

The error type lives in `src/error.rs` rather than `lib.rs`, because the
`Invalid` variant carries the whole list of validation failures and is long.

## Phase 2: audio/read.rs

Phases 2 and 3 do not depend on phase 4, so they can run in either order.

- [x] Port `_ieee754_80_decode`.
- [x] Port `_pcm_to_float` for widths 1, 2, 3 and 4 bytes, both byte orders.
- [x] Handle 32-bit float samples.
- [x] Port `_read_aiff`, including the AIFF-C compression tag allow list.
- [x] Port `_read_wav`, including `WAVE_FORMAT_EXTENSIBLE`.
- [x] Mix multi-channel input down to mono.
- [x] Return `(Vec<f32>, u32)`.
- [x] Build the fixtures and compare against the Python reader. `tests/`
      `make_fixtures.py` writes 15 sound files plus `expected.json`, and the
      expected samples come out of `thonk.py` itself. `tests/read.rs` reads
      them back.
- [x] Read the input in `main.rs` and report its length, rate and peak.

An `Encoding` enum carries the sample layout instead of the four boolean
arguments `_pcm_to_float` takes. The three cases are signed integer, unsigned
8-bit, and 32-bit float.

Two quirks of the Python reader are kept on purpose, and the module doc comment
says so. The `COMM` frame count is ignored, so a truncated file still reads. The
AIFF-C tag `fl32` is read as little-endian and `FL32` as big-endian.

The 80-bit decoder needed one guard that Python does not. A mantissa word of
zero times an overflowed power of two is NaN, not zero, so each term is skipped
when its own mantissa is zero. Python raises `OverflowError` at the same point
and never reaches the multiply. Rust returns infinity, and `round_rate` turns
that into a readable error.

Rust ignores SIGPIPE at startup, so `thonk --list-scores | head` panicked out of
`println!`. `main` now restores the default disposition, and the program dies
quietly like every other command line tool.

Read the whole file into a `Vec<u8>` and walk the chunks with an index, rather
than seeking. Input files are short, so the memory cost is small and the code is
simpler.

One behavior changes here. `_pcm_to_float` treats 8-bit samples as signed for
both formats. WAV 8-bit is unsigned, so the Python decodes it with a half-scale
offset error. Pass the format into the converter and handle the two cases apart.

- [x] Fix the WAV 8-bit sign handling, and note the change in the README.
      Done in `thonk.py` first, so the parity harness compares two readers that
      agree. `_pcm_to_float` takes an `unsigned8` flag and the WAV caller passes
      it. Carry the same split into `read.rs`.

## Phase 3: audio/write.rs and audio/fix.rs

- [x] Port `_ieee754_80_encode`.
- [x] Port `StreamWriter` over a `BufWriter<File>` plus the raw `File`.
- [x] Write the header, record `data_start`, and append frames.
- [x] Implement `Drop` for `StreamWriter` to patch the header on unwind.
- [x] Give `StreamWriter` a `flush` that patches the header. Calling it every
      10 seconds is the render loop's job, in phase 5.
- [x] Port `fix_header` for both AIFF and WAV.

The overflow wrap needs care. Python computes `(scaled + 32768) % 65536 - 32768`,
and Python `%` returns a non-negative result for a positive modulus. Rust `%` is
a remainder and returns a negative result for a negative left operand.

- [x] Use `rem_euclid(65536)` for the wrap, and test both boundaries.
- [x] Compare whole written files against Python, byte for byte.
- [x] Wire `--fix` into `main.rs`.

`StreamWriter` holds a `BufWriter<File>` alone. `BufWriter` flushes before it
seeks, so the raw handle the plan called for is not needed.

`Drop` calls the same `finish` that `close` calls, and a `finished` flag stops
the work happening twice. `close` returns the error, and `Drop` discards it,
because a destructor has nowhere to report one.

The Python writer casts a NaN sample to an undefined integer. The Rust writer
writes silence instead. The engine never produces one, so nothing depends on it.

`fix_header` reads only the first 64 KiB of the file, not all of it. The chunk
tags are found by searching, because the sizes in a truncated file are exactly
what cannot be trusted. Python reads the whole file into memory, which is a
problem for an output that runs to gigabytes.

## Phase 4: curve.rs and the built-in scores

- [x] Port `Curve::new`, which places breakpoints at random times.
- [x] Port `Curve::at(t: f64) -> f64`, one time value per call.
- [x] Use `partition_point` in place of `np.searchsorted`.
- [x] Port `Parameter` with its base curve, noise curve and depth curve.
- [x] Transcribe the five built-in scores as Rust constants.
- [x] Compare every transcribed number against `thonk.SCORES`. The generator
      dumps the dictionary to `scores.json` and `tests/scores.rs` checks all 60
      numbers, plus the order, the descriptions and the stretch flags.
- [x] Wire `--list-scores` into `main.rs`, with the origin column.
- [x] Define `ParamSpec` and `ScoreSpec` with the `serde` derives already on
      them, so phase 6 only adds loading and overriding.

Every random draw goes through one `Rand` type. The draw order is part of what
a seed means, so keeping the calls in one place keeps saved seeds stable.

`Curve::new` clamps a segment length to 0.1 milliseconds and stops at 8 million
breakpoints. Neither guard is reachable from a validated score. They exist so
that a bad score cannot hang the program before phase 6 validation reports it.
A `debug_assert` was tried first and removed, because it fired on the very test
that proves the guard works.

Phase 6 must also bound the randomizer rate. A rate of 100 kHz over an hour
asks for 360 million breakpoints in one curve.

The score lookup in `main.rs` runs before the input file is read, so a mistyped
score name fails without touching the disk.

## Phase 5: engine.rs and session.rs

At the end of this phase the program renders.

- [x] Port the `Thonk` structure and its constructor.
- [x] Build the transposer voices and the voice weight table.
- [x] Port `onsets`: the 2 millisecond grid, the trapezoid integral, the
      inversion, and the fractional carry between blocks.
- [x] Write the inversion as a two-pointer walk, because both the targets and
      the integral are sorted.
- [x] Port `render_block`, returning a `Vec<[f32; 2]>`.
- [x] Cache envelopes in a `HashMap<(usize, usize), Arc<[f32]>>`, capped at 4096.
- [x] Port the overlap tail buffer.
- [x] Implement `Session::render` as an `Iterator<Item = Progress>`.
- [x] Stop on an `Arc<AtomicBool>` read once per block.
- [x] Install a `ctrlc` handler that sets the flag.
- [x] Port the autogain ramp as a per-sample multiply across the block.
- [x] Port the progress console and the closing summary.

One porting hazard sits in the grain mixing loop. Python writes `buf[s:s+n]` and
silently truncates a slice that runs past the end. Rust panics on the same index.
Custom scores from a file make this more than theoretical.

- [x] Clamp the grain write length to the remaining buffer before the loop.
- [x] Flush and patch the header every 10 seconds of output, from the render
      loop.
- [x] Wire the render into `main.rs`, with the opening report, the progress
      console and the closing summary.

`Render` yields `Result<Progress>` rather than `Progress`, so a failure to write
reaches the caller. Python lets the same error propagate out of its generator.

`Render` implements `Drop`, which flushes the writer. `StreamWriter` also
finalizes on its own drop, so a render that unwinds leaves a playable file
twice over.

Grain index arithmetic is `f64` where Python drops to `f32`. Above about 16
million samples a `f32` index cannot land on a single sample, and `f32` was an
accident of how numpy typed the expression rather than a choice.

The clip message in the Python version is unreachable. Clipping runs before the
writer, so the writer has nothing past full scale left to count, and
`--overflow clip` always reports zero. The Rust session counts the samples it
clips, which makes the existing message work.

Measured on a one second input at 22050 Hz, against Python with numpy:

| Render | Python | Rust | Faster by |
| --- | --- | --- | --- |
| `flowing`, 1 minute | 0.7 s | 0.06 s | 12x |
| `hectic`, 2 minutes | 10.4 s | 1.4 s | 7.6x |
| `hectic`, 20 minutes | not measured | 13.1 s | |

`hectic` at its full 20 minutes writes 3.8 million grains at 92 times realtime.
Ctrl-C part way through stopped at 3 minutes 13 seconds and left a file that
reads back at exactly that length.

Without an installed signal handler, SIGINT kills the process and the header
stays broken. The handler is required, not optional.

## Phase 6: scores from the command line

This is the one added feature. It has three parts.

### Score files

```
thonk in.aiff out.aiff --score-file my.toml --score glacial
```

`--score-file PATH` is repeatable. Each file holds a `[scores.NAME]` table per
score:

```toml
[scores.glacial]
description = "almost nothing, very slowly"
duration = 3600
spread = 0.6
voices = 3
stretch = false
density   = { range = [0.2, 4.0],   seg = [30, 180], rand = [0.1, 0.2] }
length    = { range = [0.06, 0.10], seg = [30, 180], rand = [0.1, 0.2] }
attack    = { range = [0.35, 0.50], seg = [30, 180], rand = [0.1, 0.1] }
position  = { range = [0.0, 1.0],   seg = [20, 120], rand = [0.1, 0.05] }
transpose = { range = [-24.0, 0.0], seg = [30, 180], rand = [0.1, 0.08] }
balance   = { range = [0.0, 1.0],   seg = [10, 60],  rand = [0.3, 0.3] }
```

Every field except the six parameter tables has a default, taken from `flowing`.
A file can therefore define a score in four lines.

- [x] Define `ScoreSpec` and `ParamSpec` with `serde` derives and defaults.
- [x] Load `--score-file PATH`, repeatable.
- [x] Load `~/.config/thonk/scores/*.toml` on every run.
- [x] Add `--no-user-scores` to turn the directory off.
- [x] Apply the load order: built-ins, then the user directory, then each
      `--score-file` in the order given. A later definition replaces an earlier
      one of the same name.

### Inline overrides

```
thonk in.aiff out.aiff --score flowing --set density.range=1,400 --set spread=0.6
```

`--set` is repeatable and applies to the active score after it loads. The grammar
is `<key>=<values>`, where `<key>` is either `<param>.<field>` or a top-level
name.

| Key | Values |
| --- | --- |
| `density.range`, `length.range`, `attack.range`, `position.range`, `transpose.range`, `balance.range` | low,high |
| `<param>.seg` | min,max in seconds |
| `<param>.rand` | rate in Hz, depth as a fraction |
| `spread`, `duration` | one number |
| `voices` | one integer |
| `stretch` | true or false |

- [x] Parse `--set`, repeatable.
- [x] Apply overrides after the score loads.
- [x] On a bad key, list the accepted keys.
- [x] On a bad value, name the key and the expected shape.

### Inspection

- [x] Add `--dump-score NAME`, which prints the resolved score as TOML and exits.
- [x] Add an origin column to `--list-scores`, reading `built-in`, the user
      directory, or the file path.

A user starts from a built-in, redirects `--dump-score` to a file, and edits it.
This makes the format discoverable without documentation.

### Validation

Validation runs once, after loading and after `--set`. It reports every problem
at once rather than the first one.

- [x] `low` is smaller than or equal to `high` in every range.
- [x] `seg` minimum and maximum are both greater than zero, and the minimum does
      not exceed the maximum.
- [x] `density.range` low is greater than zero.
- [x] `length.range` low is greater than zero, and the high bound does not exceed
      1.0 second. The high bound sizes the overlap buffer.
- [x] `attack.range` sits inside 0.0 to 0.5.
- [x] `position.range` sits inside 0.0 to 1.0.
- [x] `balance.range` sits inside 0.0 to 1.0.
- [x] `spread` sits inside 0.0 to 1.0.
- [x] `voices` sits between 1 and 16.
- [x] `duration` is greater than zero.
- [x] A `rand` rate is at most 1000 Hz. A higher rate asks one curve for
      millions of breakpoints.
- [x] A `rand` rate or depth of zero turns the randomizer off.
- [x] Collect all failures and print them together.
- [x] Reject a score with a number that is not finite.

Every field defaults, not only the whole-score ones. A file can name a single
parameter, or a single field of one, and everything else comes from the score it
replaces or from `flowing`. `density = { range = [0.5, 20.0] }` is a whole
score.

`deny_unknown_fields` is on, so a misspelled field is named rather than
silently ignored. A file is parsed in two steps, so one that forgot its
`[scores.NAME]` header is told the shape rather than which field was
unexpected.

`--dump-score` writes the inline-table form the documentation shows, not what
`toml::to_string` produces. A test round-trips all five built-ins through it.
`--set` applies to a dump as well as to a render, so a tweak that sounds right
can be saved straight to a file.

Validation runs on the score that will be rendered. `--list-scores` marks an
unusable score with the count of its problems instead of failing, so one broken
file in the user directory does not block every other score.

## Phase 7: testing

### Unit tests

- [x] 80-bit float round trip.
- [x] PCM converters at each width and byte order.
- [x] Wrap arithmetic at both boundaries.
- [x] Density integral against a constant density curve, where the answer is
      exact.
- [x] TOML parse of a full score and of a minimal score.
- [x] One test per validation rule.

The constant density test found a real off-by-one. A curve whose true integral
is exactly 10 accumulates to 9.999999999999998, and flooring that writes 9
grains where 10 are due. The count is now taken a billionth above the total, and
the remainder is held at or above zero. Python floors the same way and has the
same fault. The error cannot build up, because the remainder carries into the
next block.

### Fixture tests

- [x] Write small AIFF, AIFF-C and WAV fixtures with the Python version.
- [x] Compare the decoded samples against the Python decoder.
- [x] Compare the written header bytes against Python output for a known frame
      count.
- [x] Repair a truncated file and compare the result.

### A2B parity harness

`tests/parity/parity.sh` is a wrapper that finds a Python with numpy.
`tests/parity/parity.py` builds the Rust binary, writes one transient-rich input
file, renders every score through both programs across several seeds, and
compares aggregate statistics.

- [x] Peak and RMS level, in decibels.
- [x] Grain count per second.
- [x] Spectral centroid, from the power spectrum averaged over frames.
- [x] Silence fraction, skipped for a score that has almost none.
- [x] Record the measured numbers, so a later drift is visible.
- [x] Record the settings alongside them, and skip the drift check when a run
      does not match.

Per-seed comparison is meaningless because the generators differ. The test
asserts that the two implementations draw from the same distribution.

A fixed tolerance turned out to be the wrong tool. A score curve places only a
handful of breakpoints in one render, so a single seed says very little about
the distribution behind it. At 3 seeds of 15 seconds the two implementations
reported 105 and 63 grains a second, which looks like a serious disagreement and
is nothing but noise. The limit is now the larger of a fixed floor and 2.5
standard errors of the difference between the two means, which is what a
difference has to beat to mean anything. A run also says which measurements were
too noisy to conclude from, so a toothless run cannot pass quietly.

At the recorded settings of 6 seeds of 180 seconds, all 25 measurements agree:

| Score | Measurement | Python | Rust |
| --- | --- | --- | --- |
| `flowing` | grains a second | 125.6 | 129.0 |
| `flowing` | RMS | -20.2 dB | -19.3 dB |
| `hectic` | grains a second | 3072.3 | 2964.2 |
| `hectic` | spectral centroid | 4392 Hz | 4294 Hz |
| `sparse` | silent fraction | 0.275 | 0.301 |
| `stretch5` | grains a second | 686.4 | 672.3 |

`flowing` is the clearest result. Its density curve draws uniformly from 2 to
250, so the average must come out at 126, and both implementations land there.

One measurement needed chasing. `flowing.centroid_hz` passed by only 2.2
standard errors at 6 seeds, which is close enough to chance to be worth a
second look. Raising the seed count collapsed the gap from 482 Hz to 138 Hz to
31 Hz, which is the signature of sampling noise rather than a difference in the
engines.

Peak level saturates at 0 dB for the dense scores, because a render without
`--autogain` reaches full scale. It stays informative for `sparse`, and RMS
carries the loudness comparison everywhere else.

## Phase 8: rename, packaging, documentation and cleanup

This phase revises two decisions made earlier. The project takes a new name, and
the user configuration directory added in phase 6 is replaced by a directory
tracked in the repository.

### Rename the project to thonkr

The Rust program is `thonkr`. The Python reference keeps the name `thonk.py`,
because it is the thing being ported and the name is part of its history.

- [x] Set `name = "thonkr"` in `Cargo.toml`, for the package, the library and
      the binary.
- [x] Replace `thonk::` with `thonkr::` in `src/main.rs` and in every file under
      `tests/`. Code inside the library uses `crate::` and does not change.
- [ ] Rename the directory from `Thonk` to `thonkr`.
      Left for a human to run. Renaming the working directory out from under a
      running shell breaks it, and git does not care what the directory is
      called. The command is:

      ```sh
      mv ~/src/Thonk ~/src/thonkr
      ```
- [x] Point the git remote at `https://github.com/mjladd/thonkr`. The repository
      moves owner as well as name, from `Deep-Fried-Unicorn/Thonk`.
- [x] Update every mention of the command in `README.md` and in the doc
      comments.
- [x] Add a `LICENSE` file. `Cargo.toml` already claims MIT and no file backs
      that claim.

### Move the extra scores into the repository

Phase 6 loaded extra scores from `~/.config/thonkr/scores`. That directory is
invisible, it is not tracked, and a released binary arrives with nothing in it.
Scores now live in the repository instead, and ship inside the release archive.

- [x] Remove the user configuration directory from `Catalog::load` and drop the
      `dirs` dependency.
- [x] Remove the `--no-user-scores` flag, which has nothing left to turn off.
- [x] Track `scores/*.toml`, each one documented in comments.
- [x] Write `scores/example.toml`, which explains every field and every `--set`
      key in place.
- [x] Write two or three scores worth having, not only an example.
- [x] Update the `--list-scores` footer to name the `scores` directory and the
      `--score-file` flag.
- [x] Update the tests in `tests/score_files.rs` that cover the user directory.

`--score-file` stays exactly as it is. It is now the only way extra scores
arrive, which makes the loading order shorter: built-ins, then each
`--score-file` in the order given.

### Build and release with GitHub Actions

- [x] Add `.github/workflows/ci.yml`, running on every push and pull request:
      `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
      `cargo test`. The fixtures are committed, so this job needs no Python.
- [x] Add a second CI job that installs numpy and runs the parity harness from
      phase 7. It reports a failure without blocking the merge, because the
      tolerances are statistical.
- [x] Cache the cargo registry and the `target` directory between runs.
- [x] Add `.github/workflows/release.yml`, triggered by a tag matching `v*`.
- [x] Give the release job `permissions: contents: write`, which it needs to
      attach files.

Build one archive per target:

| Target | Runner | Archive |
| --- | --- | --- |
| `aarch64-apple-darwin` | `macos-latest` | `thonkr-<version>-aarch64-apple-darwin.tar.gz` |
| `x86_64-apple-darwin` | `macos-latest` | `thonkr-<version>-x86_64-apple-darwin.tar.gz` |
| `x86_64-unknown-linux-musl` | `ubuntu-latest` | `thonkr-<version>-x86_64-unknown-linux-musl.tar.gz` |

The Linux target is statically linked, so one binary runs on any distribution
whatever its glibc version. A glibc build refuses to start on any system older
than the runner, which is the most common reason a downloaded binary fails.

- [x] Build with `--release` and the `lto` profile already in `Cargo.toml`.
- [x] Put the binary, `README.md`, `LICENSE` and the whole `scores` directory in
      each archive.
- [x] Write a `SHA256SUMS` file and attach it alongside the archives.
- [x] Attach everything to a GitHub release created from the tag.
- [x] Do not publish to crates.io. The release job needs no secret beyond the
      token GitHub provides.

macOS refuses to open an unsigned binary that arrived from the internet.

- [x] Say in the README how to clear the quarantine flag:
      `xattr -d com.apple.quarantine thonkr`.

### Documentation

- [x] Rewrite `README.md` for the Rust command line program.
- [x] Document the score file format and every `--set` key.
- [x] Show the whole loop in the README: dump a built-in, edit it, render with
      it.
- [x] Note the WAV 8-bit fix, the seed change and the `--overflow clip` count
      under a compatibility heading.
- [x] Say that `thonk.py` is the reference implementation and that the parity
      harness compares the two.

### Remove what the command line version does not need

- [x] Delete `thonk_gui.py`, `build_app.sh`, `thOnk_icon.png`, `files.zip`,
      `thOnk.app.zip` and `thOnk-app.zip`.
- [x] Keep `thonk.py` and `tests/make_fixtures.py`.


### What phase 8 changed

The user configuration directory is gone, along with the `dirs` dependency and
the `--no-user-scores` flag. Extra scores are tracked in `scores/` and ship
inside the release archive, so a downloaded binary arrives with them.

Four score files ship. `example.toml` explains every field and every `--set`
key in place. Three are scores worth having, and they measure as different as
they sound, on the same input at 30 seconds:

| Score | Spectral centroid | RMS | Silence |
| --- | --- | --- | --- |
| `glacial` | 2213 Hz | 0.017 | 86.1% |
| `shimmer` | 4726 Hz | 0.110 | 0.2% |
| `rumble` | 1124 Hz | 0.189 | 0.1% |

A test loads every file in `scores/`, checks that each parses, validates and
carries a description, and that loading them together produces no name
collision. A broken score file is a broken release, so it fails the build.

`cargo fmt` ran across the tree for the first time, because CI checks the
formatting.

Every command the README claims was run before the README was committed.

---

## Risks

The parity tolerances are a judgment call. Set them from a first measurement run,
and record the measured numbers in the script so a later drift is visible.

The density integral inversion is the one place where a small numerical
difference changes the grain count. Test it against a constant density curve,
where the answer is exact.

Performance at 6000 grains per second is the stress case. Rust replaces a
per-grain Python loop, so it will be faster without any parallel work. If
`hectic` still runs slowly, `rayon` is an option, but the random draws must stay
in the sequential pass to keep seeds stable, and the mixing writes overlap. Defer
that until a measurement asks for it.
