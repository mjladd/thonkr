#!/usr/bin/env python3
"""Compare the Rust renderer against thonk.py, statistically.

The two use different random generators, so a seed does not produce the same
audio in both. Comparing samples is therefore meaningless. What must hold is
that the two draw from the same distribution: across several seeds, the same
score must come out at the same loudness, the same grain rate, the same
brightness and the same sparseness.

Run it from the repository root:

    python3 tests/parity/parity.py
    python3 tests/parity/parity.py --seeds 8 --duration 60
    python3 tests/parity/parity.py --score hectic --verbose

It needs numpy, and it builds the Rust binary itself. It exits non-zero when a
measurement falls outside its tolerance.
"""

import argparse
import json
import os
import re
import struct
import subprocess
import sys
import tempfile

import numpy as np

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(ROOT, "archive"))

BASELINE_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "baseline.json")

SCORES = ["flowing", "hectic", "sparse", "stretch1", "stretch5"]

# The smallest difference that counts as a real disagreement, whatever the
# sampling noise says. A difference below this is uninteresting even if the
# seeds happen to agree closely.
FLOOR = {
    "peak_db": 1.0,          # decibels
    "rms_db": 1.0,           # decibels
    "grains_per_sec": 0.05,  # fraction of the Python mean
    "centroid_hz": 0.05,     # fraction of the Python mean
    "silent_fraction": 0.10,  # absolute
}

# How many standard errors of the difference count as agreement. At 2.5, two
# implementations drawing from the same distribution fail about one comparison
# in eighty by chance.
SIGMA = 2.5

# Anything below this counts as silence, about -80 decibels.
SILENCE = 1e-4


# --------------------------------------------------------------------------
# rendering
# --------------------------------------------------------------------------


def build_rust():
    """Build the release binary and return its path."""
    subprocess.run(["cargo", "build", "--release", "--quiet"], cwd=ROOT, check=True)
    for name in ("thonkr", "thonk"):
        path = os.path.join(ROOT, "target", "release", name)
        if os.path.exists(path):
            return path
    raise SystemExit("no release binary was built")


def make_input(path, rate=22050):
    """A short, transient-rich mono file, the kind the original asked for."""
    rng = np.random.default_rng(0)
    n = rate
    x = rng.normal(0, 0.25, n) * np.linspace(1.0, 0.0, n) ** 2
    # A few clicks, so there is something for the grains to bite on.
    for at in (0, n // 5, n // 2):
        x[at:at + 40] += np.linspace(1.0, 0.0, 40) * 0.8
    x = np.clip(x, -1.0, 1.0)

    q = np.rint(x * 32767).astype(">i2").tobytes()
    comm = struct.pack(">hIh", 1, n, 16) + thonk._ieee754_80_encode(rate)
    ssnd = struct.pack(">II", 0, 0) + q
    body = (b"COMM" + struct.pack(">I", len(comm)) + comm
            + b"SSND" + struct.pack(">I", len(ssnd)) + ssnd)
    with open(path, "wb") as fh:
        fh.write(b"FORM" + struct.pack(">I", 4 + len(body)) + b"AIFF" + body)
    return rate


GRAINS = re.compile(r"\((\d+) grains\)")


def render_rust(binary, src, out, score, duration, seed):
    result = subprocess.run(
        [binary, src, out, "--score", score, "--duration", str(duration),
         "--seed", str(seed)],
        cwd=ROOT, check=True, capture_output=True, text=True)
    return grains_from(result.stdout, "rust")


def render_python(src, out, score, duration, seed):
    result = subprocess.run(
        [sys.executable, os.path.join(ROOT, "archive", "thonk.py"), src, out,
         "--score", score, "--duration", str(duration), "--seed", str(seed)],
        cwd=ROOT, check=True, capture_output=True, text=True)
    return grains_from(result.stdout, "python")


def grains_from(text, which):
    match = GRAINS.search(text)
    if not match:
        raise SystemExit("could not read the grain count from %s:\n%s" % (which, text))
    return int(match.group(1))


# --------------------------------------------------------------------------
# measuring
# --------------------------------------------------------------------------


def read_stereo(path):
    """Read a 16-bit stereo AIFF or WAV as a float array of shape (n, 2)."""
    with open(path, "rb") as fh:
        raw = fh.read()
    if raw[0:4] == b"FORM":
        start, dtype = 54, ">i2"
    elif raw[0:4] == b"RIFF":
        start, dtype = 44, "<i2"
    else:
        raise SystemExit("%s: not an AIFF or WAV file" % path)
    pcm = np.frombuffer(raw[start:], dtype=dtype).astype(np.float64) / 32768.0
    return pcm[: (len(pcm) // 2) * 2].reshape(-1, 2)


def db(value):
    return 20.0 * np.log10(max(float(value), 1e-12))


def spectral_centroid(mono, rate, frame=2048, hop=1024):
    """The centre of mass of the average power spectrum, in hertz.

    Averaging the spectrum over frames first, rather than taking one transform
    of the whole file, keeps a single loud moment from deciding the answer.
    """
    if len(mono) < frame:
        return 0.0
    window = np.hanning(frame)
    starts = range(0, len(mono) - frame, hop)
    power = np.zeros(frame // 2 + 1)
    count = 0
    for start in starts:
        spectrum = np.fft.rfft(mono[start:start + frame] * window)
        power += np.abs(spectrum) ** 2
        count += 1
    if not count:
        return 0.0
    power /= count
    freqs = np.fft.rfftfreq(frame, 1.0 / rate)
    total = power.sum()
    return float((freqs * power).sum() / total) if total > 0 else 0.0


def measure(path, grains, rate):
    stereo = read_stereo(path)
    mono = stereo.mean(axis=1)
    seconds = len(stereo) / rate
    return {
        "peak_db": db(np.max(np.abs(stereo))),
        "rms_db": db(np.sqrt(np.mean(stereo ** 2))),
        "grains_per_sec": grains / max(seconds, 1e-9),
        "centroid_hz": spectral_centroid(mono, rate),
        "silent_fraction": float(np.mean(np.abs(mono) < SILENCE)),
    }


# --------------------------------------------------------------------------
# comparing
# --------------------------------------------------------------------------


def compare(name, python, rust):
    """One row per measurement, with whether the two agree.

    A score curve places only a handful of breakpoints in a short render, so
    one seed says very little about the distribution behind it. The limit is
    therefore the larger of a fixed floor and the sampling noise itself, which
    is what a difference of means has to beat to mean anything.
    """
    rows = []
    n = len(python)
    for key, floor in FLOOR.items():
        p_values = np.array([m[key] for m in python], dtype=float)
        r_values = np.array([m[key] for m in rust], dtype=float)
        p, r = float(p_values.mean()), float(r_values.mean())

        # The standard error of the difference between the two means.
        noise = float(np.sqrt(p_values.var(ddof=1) / n + r_values.var(ddof=1) / n))
        absolute_floor = floor if key.endswith("_db") or key == "silent_fraction" \
            else floor * abs(p)
        limit = max(absolute_floor, SIGMA * noise)
        difference = abs(r - p)

        # Silence only says anything about a score that has some.
        skip = key == "silent_fraction" and max(p, r) < 0.01
        rows.append({
            "score": name, "metric": key, "python": p, "rust": r,
            "noise": noise, "difference": difference, "limit": limit,
            "bound_by": "noise" if SIGMA * noise > absolute_floor else "floor",
            "ok": skip or difference <= limit, "skipped": skip,
        })
    return rows


def show(rows, verbose):
    print("%-9s %-16s %12s %12s %11s %11s %4s" %
          ("score", "measurement", "python", "rust", "difference", "allowed", ""))
    for row in rows:
        if row["skipped"]:
            mark = "   -"
        else:
            mark = "  ok" if row["ok"] else "  NO"
        print("%-9s %-16s %12.4f %12.4f %11.4f %11.4f %4s" %
              (row["score"], row["metric"], row["python"], row["rust"],
               row["difference"], row["limit"], mark))
        if verbose and not row["skipped"]:
            print("%-26s bound by the %s; sampling noise %.4f"
                  % ("", row["bound_by"], row["noise"]))


def check_drift(rows, args):
    """Compare against the numbers measured the first time this ran.

    Only a run at the same settings is comparable. A different seed count or
    duration moves every measurement on its own, which says nothing about
    either implementation.
    """
    if not os.path.exists(BASELINE_PATH) or args.record:
        save_baseline(rows, args)
        print("\nwrote the measurements to %s" % BASELINE_PATH)
        return []
    with open(BASELINE_PATH) as fh:
        baseline = json.load(fh)

    settings = baseline.get("settings", {})
    if settings.get("seeds") != args.seeds or settings.get("duration") != args.duration:
        print("\nno drift check: the baseline was recorded at %s seeds of %s seconds"
              % (settings.get("seeds"), settings.get("duration")))
        return []

    drifted = []
    for row in rows:
        key = "%s.%s" % (row["score"], row["metric"])
        was = baseline.get("measurements", {}).get(key)
        if was is None:
            continue
        for side in ("python", "rust"):
            before, now = was[side], row[side]
            change = abs(now - before) / max(abs(before), 1e-9)
            if change > 0.10:
                drifted.append("%s %s: %.4f -> %.4f" % (key, side, before, now))
    return drifted


def save_baseline(rows, args):
    data = {
        "note": "measured by tests/parity/parity.py; a later change here at the "
                "same settings means one of the two implementations moved",
        "settings": {"seeds": args.seeds, "duration": args.duration},
        "measurements": {
            "%s.%s" % (r["score"], r["metric"]): {"python": r["python"], "rust": r["rust"]}
            for r in rows
        },
    }
    with open(BASELINE_PATH, "w") as fh:
        json.dump(data, fh, indent=1, sort_keys=True)


# --------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--seeds", type=int, default=6, help="how many seeds per score")
    ap.add_argument("--duration", type=float, default=180.0,
                    help="seconds per render; shorter renders are too noisy to say much")
    ap.add_argument("--score", action="append", help="only this score; repeatable")
    ap.add_argument("--record", action="store_true",
                    help="rewrite the baseline from this run")
    ap.add_argument("--verbose", action="store_true", help="show tolerances and spread")
    args = ap.parse_args()

    scores = args.score or SCORES
    binary = build_rust()
    work = tempfile.mkdtemp(prefix="thonk-parity-")
    src = os.path.join(work, "input.aiff")
    rate = make_input(src)

    all_rows = []
    for name in scores:
        python, rust = [], []
        for seed in range(1, args.seeds + 1):
            p_out = os.path.join(work, "p_%s_%d.aiff" % (name, seed))
            r_out = os.path.join(work, "r_%s_%d.aiff" % (name, seed))
            p_grains = render_python(src, p_out, name, args.duration, seed)
            r_grains = render_rust(binary, src, r_out, name, args.duration, seed)
            python.append(measure(p_out, p_grains, rate))
            rust.append(measure(r_out, r_grains, rate))
            print("  %s seed %d" % (name, seed), end="\r", flush=True)
        all_rows.extend(compare(name, python, rust))

    print(" " * 40, end="\r")
    show(all_rows, args.verbose)

    drifted = check_drift(all_rows, args)
    if drifted:
        print("\ndrift from the recorded baseline:")
        for line in drifted:
            print("  %s" % line)

    failed = [r for r in all_rows if not r["ok"]]
    print()
    if failed:
        print("%d of %d measurements are outside tolerance" % (len(failed), len(all_rows)))
        for row in failed:
            print("  %s %s: python %.4f, rust %.4f, allowed %.4f (%s)"
                  % (row["score"], row["metric"], row["python"], row["rust"],
                     row["limit"], row["bound_by"]))
        return 1
    print("all %d measurements agree across %d seeds of %.0f seconds"
          % (len(all_rows), args.seeds, args.duration))
    loose = [r for r in all_rows
             if not r["skipped"] and r["bound_by"] == "noise" and r["limit"] > abs(r["python"])]
    if loose:
        print("%d of them were too noisy to say much; raise --seeds or --duration:"
              % len(loose))
        for row in loose:
            print("  %s %s" % (row["score"], row["metric"]))
    return 0


import thonk  # noqa: E402  (after ROOT is on the path)

if __name__ == "__main__":
    sys.exit(main())
