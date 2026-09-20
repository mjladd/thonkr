#!/usr/bin/env python3
"""Write the sound files that tests/read.rs decodes, and what they decode to.

Run it from the repository root with a Python that has numpy:

    python3 tests/make_fixtures.py

It writes tests/fixtures/*.aiff, *.aifc, *.wav and expected.json. The expected
samples come from thonk.py itself, so the two readers are compared against each
other rather than against a second guess.
"""

import json
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import numpy as np  # noqa: E402

import thonk  # noqa: E402

HERE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures")

# A short ramp with both extremes in it, so sign errors cannot hide.
WAVE = np.array([0.0, 0.5, -0.5, 0.999969, -1.0, 0.25, -0.25, 0.75],
                dtype=np.float64)


def aiff(path, samples, channels, bits, rate, compression=None):
    width = (bits + 7) // 8
    little = compression in (b"sowt", b"fl32")
    if compression in (b"fl32", b"FL32"):
        raw = samples.astype("<f4" if little else ">f4").tobytes()
    else:
        full = float(1 << (bits - 1))
        q = np.clip(np.rint(samples * full), -full, full - 1).astype(np.int64)
        if width == 1:
            raw = q.astype(np.int8).tobytes()
        elif width == 2:
            raw = q.astype("<i2" if little else ">i2").tobytes()
        elif width == 3:
            b = (q.astype(np.int64) & 0xFFFFFF)
            trip = np.stack([(b >> 16) & 255, (b >> 8) & 255, b & 255], axis=1)
            if little:
                trip = trip[:, ::-1]
            raw = trip.astype(np.uint8).tobytes()
        else:
            raw = q.astype("<i4" if little else ">i4").tobytes()

    frames = len(samples) // channels
    comm = struct.pack(">hIh", channels, frames, bits) + thonk._ieee754_80_encode(rate)
    if compression is not None:
        comm += compression + bytes([len(compression)]) + compression + b"\x00"
    form = b"AIFC" if compression is not None else b"AIFF"
    ssnd = struct.pack(">II", 0, 0) + raw
    body = (b"COMM" + struct.pack(">I", len(comm)) + comm
            + b"SSND" + struct.pack(">I", len(ssnd)) + ssnd)
    with open(path, "wb") as fh:
        fh.write(b"FORM" + struct.pack(">I", 4 + len(body)) + form + body)


def wav(path, samples, channels, bits, rate, tag=1, extensible=False):
    width = (bits + 7) // 8
    if tag == 3:
        raw = samples.astype("<f4").tobytes()
    elif width == 1:
        q = np.clip(np.rint(samples * 128.0), -128, 127).astype(np.int64)
        raw = (q + 128).astype(np.uint8).tobytes()
    else:
        full = float(1 << (bits - 1))
        q = np.clip(np.rint(samples * full), -full, full - 1).astype(np.int64)
        if width == 3:  # numpy has no 24-bit type, so lay the bytes out by hand
            b = q & 0xFFFFFF
            trip = np.stack([b & 255, (b >> 8) & 255, (b >> 16) & 255], axis=1)
            raw = trip.astype(np.uint8).tobytes()
        else:
            raw = q.astype("<i%d" % width).tobytes()

    if extensible:
        fmt = struct.pack("<HHIIHH", 0xFFFE, channels, rate, rate * width * channels,
                          width * channels, bits)
        fmt += struct.pack("<HHI", 22, bits, 3)
        fmt += struct.pack("<H", tag) + b"\x00\x00" + bytes.fromhex(
            "000000001000800000aa00389b71")
    else:
        fmt = struct.pack("<HHIIHH", tag, channels, rate, rate * width * channels,
                          width * channels, bits)
    body = (b"fmt " + struct.pack("<I", len(fmt)) + fmt
            + b"LIST" + struct.pack("<I", 4) + b"INFO"     # a chunk to skip over
            + b"data" + struct.pack("<I", len(raw)) + raw)
    with open(path, "wb") as fh:
        fh.write(b"RIFF" + struct.pack("<I", 4 + len(body)) + b"WAVE" + body)


def main():
    os.makedirs(HERE, exist_ok=True)
    mono = WAVE
    stereo = np.repeat(WAVE, 2) * np.tile([1.0, -1.0], len(WAVE))  # L = x, R = -x

    cases = []

    def add(name, writer, samples, channels, rate):
        path = os.path.join(HERE, name)
        writer(path)
        got, got_rate = thonk.read_audio(path)
        cases.append({
            "file": name,
            "rate": int(got_rate),
            "channels": channels,
            "samples": [float(v) for v in got],
        })
        assert got_rate == rate, name

    add("mono16.aiff", lambda p: aiff(p, mono, 1, 16, 44100), mono, 1, 44100)
    add("stereo16.aiff", lambda p: aiff(p, stereo, 2, 16, 22050), stereo, 2, 22050)
    add("mono24.aiff", lambda p: aiff(p, mono, 1, 24, 48000), mono, 1, 48000)
    add("mono8.aiff", lambda p: aiff(p, mono, 1, 8, 8000), mono, 1, 8000)
    add("mono32.aiff", lambda p: aiff(p, mono, 1, 32, 96000), mono, 1, 96000)
    add("sowt16.aifc", lambda p: aiff(p, mono, 1, 16, 44100, b"sowt"), mono, 1, 44100)
    add("float32be.aifc", lambda p: aiff(p, mono, 1, 32, 44100, b"FL32"), mono, 1, 44100)
    add("float32le.aifc", lambda p: aiff(p, mono, 1, 32, 44100, b"fl32"), mono, 1, 44100)

    add("mono16.wav", lambda p: wav(p, mono, 1, 16, 44100), mono, 1, 44100)
    add("stereo16.wav", lambda p: wav(p, stereo, 2, 16, 44100), stereo, 2, 44100)
    add("mono24.wav", lambda p: wav(p, mono, 1, 24, 48000), mono, 1, 48000)
    add("mono8.wav", lambda p: wav(p, mono, 1, 8, 22050), mono, 1, 22050)
    add("mono32.wav", lambda p: wav(p, mono, 1, 32, 44100), mono, 1, 44100)
    add("float32.wav", lambda p: wav(p, mono, 1, 32, 44100, tag=3), mono, 1, 44100)
    add("ext16.wav", lambda p: wav(p, mono, 1, 16, 44100, extensible=True), mono, 1, 44100)

    with open(os.path.join(HERE, "expected.json"), "w") as fh:
        json.dump(cases, fh, indent=1)
    print("wrote %d read fixtures to %s" % (len(cases), HERE))

    writer_fixtures()
    score_fixture()


def score_fixture():
    """Dump thonk.SCORES so the Rust transcription can be compared to it."""
    out = []
    for name, score in thonk.SCORES.items():
        entry = {
            "name": name,
            "description": score["description"],
            "duration": score["duration"],
            "stretch": bool(score.get("stretch", False)),
            "spread": score["spread"],
            "voices": score["voices"],
        }
        for field in ("position", "density", "length", "attack", "transpose",
                      "balance"):
            spec = score[field]
            entry[field] = {"range": list(spec["range"]),
                            "seg": list(spec["seg"]),
                            "rand": list(spec["rand"])}
        out.append(entry)
    with open(os.path.join(HERE, "scores.json"), "w") as fh:
        json.dump(out, fh, indent=1)
    print("wrote %d score definitions" % len(out))


def test_block(n):
    """The block both writers render, chosen so neither has to round.

    Every value is a whole multiple of 1/128, so multiplying by 32768 lands on
    an integer. The range runs to plus and minus two, which drives the wrap.
    """
    i = np.arange(n, dtype=np.float64)
    left = ((i % 513) - 256) / 128.0
    return np.stack([left, -left], axis=1).astype(np.float32)


def writer_fixtures():
    """Write output files with thonk.StreamWriter for Rust to reproduce."""
    cases = []
    for name, rate, frames in [("w44100_600.aiff", 44100, 600),
                               ("w44100_600.wav", 44100, 600),
                               ("w22050_7.aiff", 22050, 7),
                               ("w48000_0.aiff", 48000, 0),
                               ("w8000_1000.wav", 8000, 1000)]:
        path = os.path.join(HERE, name)
        writer = thonk.StreamWriter(path, rate)
        over = 0
        block = test_block(frames)
        for start in range(0, frames, 128):      # several writes, as a render does
            over += writer.write(block[start:start + 128])
        writer.close()
        cases.append({"file": name, "rate": rate, "frames": frames,
                      "overflows": int(over), "size": os.path.getsize(path)})

    # A file a crash left with its length fields never updated, and the same
    # file after thonk.fix_header has put them right.
    for name, rate, frames in [("crashed.aiff", 44100, 500), ("crashed.wav", 22050, 321)]:
        src = os.path.join(HERE, name)
        writer = thonk.StreamWriter(src, rate)
        writer.write(test_block(frames))
        writer.close()
        raw = bytearray(open(src, "rb").read())
        if name.endswith(".wav"):
            raw[4:8] = struct.pack("<I", 36)
            raw[40:44] = struct.pack("<I", 0)
        else:
            raw[4:8] = struct.pack(">I", 46)
            raw[22:26] = struct.pack(">I", 0)
            raw[42:46] = struct.pack(">I", 8)
        open(src, "wb").write(bytes(raw))

        fixed = os.path.join(HERE, "fixed_" + name)
        open(fixed, "wb").write(bytes(raw))
        size, got_frames = thonk.fix_header(fixed)
        cases.append({"file": name, "rate": rate, "frames": frames,
                      "fixed": "fixed_" + name, "size": size,
                      "fixed_frames": None if got_frames is None else int(got_frames)})

    with open(os.path.join(HERE, "writer.json"), "w") as fh:
        json.dump(cases, fh, indent=1)
    print("wrote %d writer fixtures" % len(cases))


if __name__ == "__main__":
    main()
