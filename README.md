# thonkr

A reimplementation of the granular engine from **thOnk_0+2** (Arjen van der
Schoot / Audio Ease, 1996-1999), in Rust, for the command line.

The original is a 68k/PowerPC application linked against `InterfaceLib`, and
its last possible home was Mac OS 9. This is a rebuild of the synthesis model
from the description in its manual, not a port of its code.

Feed it a short, transient-rich mono file. Get back hours of sound you had no
part in choosing.

## Installing

Download the archive for your machine from the
[releases page](https://github.com/mjladd/thonkr/releases), unpack it, and put
`thonkr` somewhere on your PATH. That is the whole installation: every score
is inside the binary, so there is nothing to keep beside it and nothing to
clone. The `scores` directory in the archive is the source of those scores,
kept there as a starting point for writing your own.

macOS refuses to open a binary that arrived from the internet and is not
signed. Clear the quarantine flag first:

```sh
xattr -d com.apple.quarantine thonkr
```

To build it yourself, with Rust 1.80 or later:

```sh
cargo build --release      # -> target/release/thonkr
```

There are no system dependencies. The AIFF and WAV handling is written here.

## Usage

```sh
thonkr input.aiff output.aiff                        # 20 minutes, flowing
thonkr input.aiff out.wav --score hectic --autogain
thonkr input.aiff out.aiff --duration 3600 --seed 5
thonkr --list-scores
thonkr --fix interrupted.aiff
```

It reads 16, 24 and 32-bit AIFF, uncompressed AIFF-C and WAV, mono or stereo.
Stereo is mixed down, because the original only accepted mono. It writes 16-bit
stereo AIFF or WAV, picked from the output extension.

Output is written as it renders, so the file is playable at every moment. Press
Ctrl-C to stop and the header is finalized on the way out. The original needed
a separate utility for that, which is what `--fix` reproduces for files another
program left truncated.

### Options

| Option | Effect |
| --- | --- |
| `--score NAME` | which score to run (default `flowing`) |
| `--score-file PATH` | load extra scores from a TOML file; repeatable |
| `--set KEY=VALUE` | change one field of the score for this run; repeatable |
| `--dump-score NAME` | print a score as TOML and exit |
| `--duration SEC` | output length; each score has its own default |
| `--rate HZ` | output sample rate (default: the input's) |
| `--gain G` | output gain (default 1.0) |
| `--overflow wrap\|clip` | behaviour past full scale; the original wrapped |
| `--autogain` | track the measured peak and hold 0.9 headroom |
| `--seed N` | reproducible run; omit for a different result every time |
| `--spread S` | per-grain stereo scatter, 0 to 1 |
| `--quiet` | no progress console |
| `--list-scores` | list the scores and exit |
| `--fix FILE` | repair the header of a truncated file and exit |

### Scores

Eight are built in, and all eight live inside the binary.

| Score | Character | Default length |
| --- | --- | --- |
| `flowing` | long arcs, moderate density | 20 min |
| `hectic` | dense, fast-shifting, up to 6000 grains/sec | 20 min |
| `sparse` | isolated grains, wide silences | 20 min |
| `stretch1` | the input dragged across one minute | 1 min |
| `stretch5` | the input dragged across five minutes | 5 min |
| `glacial` | almost nothing, very slowly, everything falling | 1 hour |
| `shimmer` | short bright grains, high and always moving | 15 min |
| `rumble` | thick and low, grains stacked into one moving mass | 20 min |

```sh
thonkr in.aiff out.aiff --score rumble --autogain
```

The first five are written in `src/score.rs`. The last three are `scores/`
`glacial.toml`, `shimmer.toml` and `rumble.toml`, read into the binary when it
is built, which is why they need no file at run time. Adding a `.toml` file to
`scores/` and rebuilding adds a score the same way.

## Scores of your own

This is the one thing the Rust version does that the original did not. A score
is the shape of a render, and you can write one.

Start from a score you already like:

```sh
thonkr --dump-score sparse > mine.toml
```

That writes the score out in full. Change the name on the first line, change
whatever numbers you want, and run it:

```sh
thonkr in.aiff out.aiff --score-file mine.toml --score mine
```

`scores/example.toml` explains every field in place. In short, each of the six
parameters takes three pairs:

```toml
[scores.mine]
description = "what this one sounds like"
duration = 600
spread = 0.45
voices = 4

density = { range = [4.0, 300.0], seg = [8.0, 50.0], rand = [0.7, 0.12] }
```

`range` is the low and high bound the parameter moves between. `seg` is the
shortest and longest gap between breakpoints, in seconds: small numbers move
fast, large numbers drift. `rand` is a randomizer riding on top of the curve,
as a rate in hertz and a depth as a fraction of the range.

Every field is optional. Anything you leave out is taken from the score this
one replaces, or from `flowing` if the name is new. A whole score can be two
lines:

```toml
[scores.slow]
density = { range = [0.5, 20.0] }
```

`--score-file` is repeatable, and a later file replaces an earlier score of the
same name. Naming a built-in replaces that one, which is how you keep a
favourite adjustment:

```sh
thonkr --dump-score rumble --set transpose.range=-24,0 > mine.toml
thonkr in.aiff out.aiff --score-file mine.toml --score rumble
```

`--dump-score rumble` writes the table as `[scores.rumble]`, so leaving the
name alone is what makes the file an adjustment to `rumble` rather than a new
score.

### Changing one thing without a file

```sh
thonkr in.aiff out.aiff --set density.range=1,400 --set spread=0.6
```

`--set` is repeatable and applies to whichever score is active. The keys are
`<parameter>.range`, `<parameter>.seg` and `<parameter>.rand` for each of
`position`, `density`, `length`, `attack`, `transpose` and `balance`, plus
`duration`, `spread`, `voices` and `stretch`.

It applies to `--dump-score` as well, so a change that sounds right can be
saved straight to a file:

```sh
thonkr --dump-score hectic --set length.range=0.002,0.02 > brighter.toml
```

A score that cannot work is refused with every reason at once, before anything
is read or written:

```
error: score "mine" is not usable:
  attack.range: [0.1, 0.9] is outside 0 to 0.5
  voices: 99 is outside 1 to 16
```

## The model

Each of the six parameters the manual lists — in-file time, granular frequency,
attack/decay portion, grain length, transposition, balance — is driven by two
layers, as described there: a score curve of randomly placed breakpoints with
random spacing, continuously interpolated, and a band-limited randomizer riding
on top whose depth is itself a slow curve.

> "For every parameter Thonk drops a certain amount of randomish values at
> randomish times in a score. During execution, there is a constant
> interpolation between these values."

Transposition goes through a statistical multi-voice transposer: each voice has
its own drifting interval and each grain is assigned to one of them by weight.
Grains are windowed with a trapezoid, panned by the sine/cosine law, and summed
with overlap.

Density reaches 6000 grains per second and grain length 1/10 second in the
`hectic` score, which is the 600-layer maximum the manual quotes.

Two deliberate departures from the original:

- **Overflow.** thOnk wrapped rather than clipped, since clip detection was too
  expensive at 600 layers, and told users to attenuate the input instead. Wrap
  is still the default here, but `--autogain` measures each block before
  writing it and keeps real headroom, and `--overflow clip` is available.
- **Speed.** The manual suggests launching it in the morning and collecting the
  results at the end of the day. `hectic` renders its full 20 minutes in about
  13 seconds here, which is 92 times faster than it plays.

It is not sample-identical to the original and cannot be: the exact scores and
random sequences live inside a binary whose source is gone. For a program whose
premise is that you have no control over the output, that may be beside the
point.

## The Python version

`archive/thonk.py` is the reference implementation this was ported from, and it
stays in the repository. It needs numpy. The two are compared by a harness that
renders every score through both and checks that they agree:

```sh
tests/parity/parity.sh
```

They use different random generators, so a seed does not produce the same audio
in both and comparing samples would be meaningless. What the harness checks is
that the two draw from the same distribution: across several seeds, the same
score comes out at the same loudness, the same grain rate, the same brightness
and the same sparseness. The numbers it measured are in
`tests/parity/baseline.json`.

### Differences from thonk.py

- **A seed gives different audio.** Matching numpy's generator exactly would
  have meant cloning its draw order forever. A seed still reproduces a render
  exactly within `thonkr`.
- **8-bit WAV reads correctly.** It is unsigned in WAV and signed in AIFF, and
  `thonk.py` read both as signed. Fixed in both versions.
- **`--overflow clip` reports what it clipped.** In `thonk.py` that message can
  never appear, because clipping runs before the count is taken.
- **No window.** `thonkr` is the command line only.

## Credit

thOnk_0+2 was written by Arjen van der Schoot, with early help from Peter
Bakker, and its interface was designed by =cw4t7abs (antiorp). The original was
freeware. This reimplementation carries no code from it.

`archive/thonk.py` was written by
[Deep-Fried-Unicorn](https://github.com/Deep-Fried-Unicorn), and `thonkr` is a
translation of that work into Rust. Reading the synthesis model out of a manual
for software nobody can run any more, and getting it to sound right, is the
part of this that took real listening. The scores, the two-layer parameter
model, the streaming writer and the header repair are all that program's
design. Thank you for writing it, and for leaving it where someone else could
pick it up.

MIT licensed. See `LICENSE`.
