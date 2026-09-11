# thonk.py

A reimplementation of the granular engine from **thOnk_0+2** (Arjen van der
Schoot / Audio Ease, 1996–1999) for machines that no longer run classic Mac
software. The original is a 68k/PowerPC application linked against
`InterfaceLib`; its last possible home was Mac OS 9. This is a rebuild of the
synthesis model from the description in its manual, not a port of its code.

Feed it a short, transient-rich mono file. Get back hours of sound you had no
part in choosing.

## Requirements

Python 3.9+ and numpy for the command line; tkinter as well for the window.
Nothing else — the AIFF and WAV handling is built in. `thOnk.app` sorts this
out for you, and `build_app.sh` removes the requirement entirely.

```sh
pip install numpy
```

## Usage

```sh
python3 thonk.py input.aiff output.aiff                      # 20 minutes, flowing
python3 thonk.py input.aiff out.wav --score hectic --autogain
python3 thonk.py input.aiff out.aiff --duration 3600 --seed 5
python3 thonk.py --list-scores
python3 thonk.py --fix interrupted.aiff
```

Reads 16/24/32-bit AIFF, uncompressed AIFF-C and WAV, mono or stereo (stereo is
mixed down, as the original only accepted mono). Writes 16-bit stereo AIFF or
WAV, picked from the output extension.

Output is written as it renders, so the file is playable at every moment. Press
Ctrl-C to stop; the header is finalized on the way out. The original needed a
separate utility for that, which is what `--fix` reproduces for files left
truncated by a crash.

### Options

| Option | Effect |
| --- | --- |
| `--score NAME` | which score to run (default `flowing`) |
| `--duration SEC` | output length; each score has its own default |
| `--rate HZ` | output sample rate (default: the input's) |
| `--gain G` | output gain (default 1.0) |
| `--overflow wrap\|clip` | behaviour past full scale; the original wrapped |
| `--autogain` | track the measured peak and hold 0.9 headroom |
| `--seed N` | reproducible run; omit for a different result every time |
| `--spread S` | per-grain stereo scatter, 0–1 |
| `--quiet` | no progress console |

### Scores

| Score | Character | Default length |
| --- | --- | --- |
| `flowing` | long arcs, moderate density | 20 min |
| `hectic` | dense, fast-shifting, up to 6000 grains/sec | 20 min |
| `sparse` | isolated grains, wide silences | 20 min |
| `stretch1` | the input dragged across one minute | 1 min |
| `stretch5` | the input dragged across five minutes | 5 min |

## The window

```sh
python3 thonk_gui.py            # or double-click thOnk.app
```

Same order of operations as the original: choose a score, read what it says
about itself, hit **Thonk**, pick an input file, pick where the output goes,
then watch the console. The Thonk button becomes **Stop**, and quitting during
a render asks first — either way the file is closed properly and stays
playable. **Show file** reveals the result in the Finder, **Play** opens it in
whatever handles AIFF on your machine, and File ▸ Repair a truncated file… is
the `--fix` path for anything a crash left half-written.

Two boxes are ticked by default that the original did not have: *Keep headroom
automatically* (`--autogain`) and *Wrap past full scale* (untick for clipping).
Leave the Seed field empty for a different result every run, or put a number in
it to get the same one back.

The window needs tkinter as well as numpy. The python.org installers include
it, Homebrew has it as `python-tk`, and Apple's `/usr/bin/python3` has it too.

## The app

`thOnk.app` in `thOnk-app.zip` is double-clickable and needs no build step. It
carries the engine, the window and the original application's own icon —
decoded out of the `icl4` resource in the resource fork of the 1999 binary — and
on launch it looks for a Python with numpy and tkinter, checking Homebrew,
python.org and Apple's in that order.

If it finds an interpreter with tkinter but no numpy, it offers to build a
private virtual environment at `~/Library/Application Support/thOnk` and install
numpy into that. It deliberately does not try to install into a system Python:
Homebrew's refuses `--user` installs, and PEP 668 interpreters refuse unmanaged
installs altogether. Nothing outside that one folder is touched, and deleting it
undoes the setup. If it still fails, the dialog shows what pip actually said and
the full log is at `~/Library/Application Support/thOnk/setup.log`.

To see which interpreters it found and what each one has:

```sh
/Applications/thOnk.app/Contents/MacOS/thOnk --doctor
```

macOS will refuse to open the app the first time, because it is unsigned and
arrived from the internet. Either right-click it and choose **Open**, or:

```sh
xattr -dr com.apple.quarantine /Applications/thOnk.app
```

### A bundle that needs nothing installed

`build_app.sh` builds the other kind of app: PyInstaller puts Python, numpy and
Tk inside the bundle, so it runs on a Mac with no Python at all — the version to
hand to someone else. Run it on your Mac, from the unzipped folder:

```sh
./build_app.sh                              # -> dist/thOnk.app
PYTHON=/opt/homebrew/bin/python3 ./build_app.sh   # if the default python3 lacks tkinter
```

It smoke-tests the engine before bundling and ad-hoc signs the result so
Gatekeeper allows it locally. The zip keeps `thonk.py`, `thonk_gui.py` and the
icon at the top level for that script to collect, alongside the same files in
`thOnk.app/Contents/Resources/`.


## The model

Each of the six parameters the manual lists — in-file time, granular frequency,
attack/decay portion, grain length, transposition, balance — is driven by two
layers, as described there: a score curve of randomly placed breakpoints with
random spacing, continuously interpolated, and a band-limited randomizer riding
on top whose depth is itself a slow curve. Transposition goes through a
statistical multi-voice transposer: each voice has its own drifting interval and
each grain is assigned to one of them by weight. Grains are windowed with a
trapezoid, panned by the sine/cosine law, and summed with overlap.

Density reaches 6000 grains per second and grain length 1/10 second in the
`hectic` score, which is the 600-layer maximum the manual quotes.

Two deliberate departures from the original:

- **Overflow.** thOnk wrapped rather than clipped, since clip detection was too
  expensive at 600 layers, and told users to attenuate the input instead. Wrap
  is still the default here, but `--autogain` measures each block before writing
  it and keeps real headroom, and `--overflow clip` is available.
- **Speed.** The manual suggests launching it in the morning and collecting the
  results at the end of the day. A 20-minute render takes about 10 seconds to a
  few minutes here, depending on density.

It is not sample-identical to the original and cannot be: the exact scores and
random sequences live inside a binary whose source is gone. For a program whose
premise is that you have no control over the output, that may be beside the
point.

## Credit

thOnk_0+2 was written by Arjen van der Schoot, with early help from Peter
Bakker, and its interface was designed by =cw4t7abs (antiorp). The original was
freeware. This reimplementation carries no code from it.
