#!/usr/bin/env python3
"""
thonk.py - a modern reimplementation of the thOnk_0+2 granular engine.

thOnk_0+2 (Arjen van der Schoot / Audio Ease, 1996-1999) was a classic Mac
application that turned a short mono AIFF file into hours of granular sound
over which the user had, by design, no control at all. The original is a
68k/PowerPC binary against InterfaceLib and cannot run on modern macOS.

This is not a port of that binary - no source for it survives here. It is a
reimplementation of the synthesis model as the original manual documents it:

    in-file time        where in the input file a grain is taken from
    granular frequency  grains written per second, up to 6000
    attack / decay      the faded portion at each end of a grain
    grain length        up to 1/10 second
    transposition       a statistical four-voice transposer of grains
    balance             left-to-right placement of each grain

    "For every parameter Thonk drops a certain amount of randomish values at
     randomish times in a score. During execution, there is a constant
     interpolation between these values."  - thOnk_0+2 manual

Each parameter therefore has two layers: a score curve of randomly placed,
interpolated breakpoints, and a separately controlled band-limited randomizer
riding on top of it. Output is written to disk as it is rendered, so the file
is usable at any moment; stopping with Ctrl-C finalizes the header rather than
leaving it broken (the original needed a separate "Fix 16bit AIFF" utility for
that, which is what --fix below reproduces).

Requires numpy. Reads 16/24/32-bit AIFF, AIFF-C (uncompressed) and WAV, mono
or stereo. Writes 16-bit stereo AIFF or WAV, chosen by output extension.

Usage
    python3 thonk.py in.aiff out.aiff
    python3 thonk.py in.aiff out.aiff --score hectic --duration 300
    python3 thonk.py --fix truncated.aiff
    python3 thonk.py --list-scores
"""

import argparse
import os
import signal
import struct
import sys
import time

import numpy as np

# --------------------------------------------------------------------------
# audio file reading
# --------------------------------------------------------------------------


def _ieee754_80_decode(b):
    """Decode an 80-bit IEEE 754 extended float (AIFF sample rate field)."""
    expon = struct.unpack(">H", b[0:2])[0]
    himant, lomant = struct.unpack(">LL", b[2:10])
    sign = -1 if expon & 0x8000 else 1
    expon &= 0x7FFF
    if expon == 0 and himant == 0 and lomant == 0:
        return 0.0
    f = himant * 2.0 ** (expon - 16383 - 31) + lomant * 2.0 ** (expon - 16383 - 63)
    return sign * f


def _ieee754_80_encode(value):
    """Encode a float as an 80-bit IEEE 754 extended number."""
    if value < 0:
        sign, value = 0x8000, -value
    else:
        sign = 0
    if value == 0:
        return struct.pack(">HLL", sign, 0, 0)
    mant, expon = np.frexp(value)
    expon = int(expon)
    if expon > 16384 or expon < -16382:
        raise ValueError("sample rate out of range")
    expon += 16382
    mant = float(mant) * 2.0  # normalize to [1, 2)
    expon -= 1
    himant = int(mant * 2 ** 31) & 0xFFFFFFFF
    frac = mant * 2 ** 31 - int(mant * 2 ** 31)
    lomant = int(frac * 2 ** 32) & 0xFFFFFFFF
    return struct.pack(">HLL", sign | (expon + 1), himant, lomant)


def _pcm_to_float(raw, sampwidth, big_endian, float_fmt=False, unsigned8=False):
    """Convert raw interleaved PCM bytes to a float32 array in [-1, 1).

    8-bit is signed in AIFF and unsigned in WAV, so the caller says which.
    """
    if float_fmt:
        dt = np.dtype(">f4" if big_endian else "<f4")
        return np.frombuffer(raw, dtype=dt).astype(np.float32)
    if sampwidth == 1:
        if unsigned8:
            a = np.frombuffer(raw, dtype=np.uint8).astype(np.float32) - 128.0
        else:
            a = np.frombuffer(raw, dtype=np.int8).astype(np.float32)
        return a / 128.0
    if sampwidth == 2:
        dt = np.dtype(">i2" if big_endian else "<i2")
        return np.frombuffer(raw, dtype=dt).astype(np.float32) / 32768.0
    if sampwidth == 3:
        b = np.frombuffer(raw, dtype=np.uint8)
        n = len(b) // 3
        b = b[: n * 3].reshape(n, 3).astype(np.int32)
        if big_endian:
            v = (b[:, 0] << 16) | (b[:, 1] << 8) | b[:, 2]
        else:
            v = (b[:, 2] << 16) | (b[:, 1] << 8) | b[:, 0]
        v = np.where(v & 0x800000, v - 0x1000000, v)
        return v.astype(np.float32) / 8388608.0
    if sampwidth == 4:
        dt = np.dtype(">i4" if big_endian else "<i4")
        return np.frombuffer(raw, dtype=dt).astype(np.float32) / 2147483648.0
    raise ValueError("unsupported sample width: %d bytes" % sampwidth)


def read_audio(path):
    """Read an AIFF/AIFF-C/WAV file. Returns (mono float32 array, sample rate)."""
    with open(path, "rb") as fh:
        head = fh.read(12)
        if len(head) < 12:
            raise ValueError("%s: too short to be an audio file" % path)
        if head[0:4] == b"FORM" and head[8:12] in (b"AIFF", b"AIFC"):
            data = _read_aiff(fh, head[8:12] == b"AIFC")
        elif head[0:4] == b"RIFF" and head[8:12] == b"WAVE":
            data = _read_wav(fh)
        else:
            raise ValueError("%s: not an AIFF or WAV file" % path)
    samples, rate, channels = data
    if channels > 1:
        n = len(samples) // channels
        samples = samples[: n * channels].reshape(n, channels).mean(axis=1)
    return np.ascontiguousarray(samples, dtype=np.float32), rate


def _read_aiff(fh, is_aifc):
    comm = ssnd = None
    while True:
        hdr = fh.read(8)
        if len(hdr) < 8:
            break
        cid, size = hdr[0:4], struct.unpack(">I", hdr[4:8])[0]
        if cid == b"COMM":
            body = fh.read(size)
            channels, frames, bits = struct.unpack(">hIh", body[0:8])
            rate = _ieee754_80_decode(body[8:18])
            compression = body[18:22] if is_aifc and size >= 22 else b"NONE"
            if compression not in (b"NONE", b"sowt", b"fl32", b"FL32", b"in24",
                                   b"in32", b"twos"):
                raise ValueError("compressed AIFF-C (%s) is not supported"
                                 % compression.decode("latin-1"))
            comm = (channels, frames, bits, rate, compression)
        elif cid == b"SSND":
            body = fh.read(size)
            offset, _block = struct.unpack(">II", body[0:8])
            ssnd = body[8 + offset:]
        else:
            fh.seek(size + (size & 1), os.SEEK_CUR)
            continue
        if size & 1:
            fh.seek(1, os.SEEK_CUR)
    if comm is None or ssnd is None:
        raise ValueError("AIFF file is missing a COMM or SSND chunk")
    channels, frames, bits, rate, compression = comm
    width = (bits + 7) // 8
    little = compression in (b"sowt", b"fl32")
    isfloat = compression in (b"fl32", b"FL32")
    usable = (len(ssnd) // (width * channels)) * width * channels
    samples = _pcm_to_float(ssnd[:usable], width, not little, isfloat)
    return samples, int(round(rate)), channels


def _read_wav(fh):
    fmt = data = None
    while True:
        hdr = fh.read(8)
        if len(hdr) < 8:
            break
        cid, size = hdr[0:4], struct.unpack("<I", hdr[4:8])[0]
        if cid == b"fmt ":
            body = fh.read(size)
            tag, channels, rate, _bps, _align, bits = struct.unpack("<HHIIHH", body[0:16])
            if tag == 0xFFFE and size >= 40:  # WAVE_FORMAT_EXTENSIBLE
                tag = struct.unpack("<H", body[24:26])[0]
            fmt = (tag, channels, rate, bits)
        elif cid == b"data":
            data = fh.read(size)
        else:
            fh.seek(size + (size & 1), os.SEEK_CUR)
            continue
        if size & 1:
            fh.seek(1, os.SEEK_CUR)
    if fmt is None or data is None:
        raise ValueError("WAV file is missing a fmt or data chunk")
    tag, channels, rate, bits = fmt
    if tag not in (1, 3):
        raise ValueError("unsupported WAV encoding (format tag %d)" % tag)
    width = (bits + 7) // 8
    usable = (len(data) // (width * channels)) * width * channels
    samples = _pcm_to_float(data[:usable], width, False, tag == 3, unsigned8=True)
    return samples, rate, channels


# --------------------------------------------------------------------------
# streaming 16-bit stereo output
# --------------------------------------------------------------------------


class StreamWriter:
    """Writes 16-bit stereo AIFF or WAV incrementally, header patched on close.

    The file on disk is a valid, playable sound file from the first flush
    onwards, so a render can be abandoned at any point without loss.
    """

    def __init__(self, path, rate):
        self.path = path
        self.rate = rate
        self.frames = 0
        self.aiff = os.path.splitext(path)[1].lower() in (".aiff", ".aif", ".aifc")
        self.fh = open(path, "wb")
        self._write_header()
        self.data_start = self.fh.tell()

    def _write_header(self):
        self.fh.seek(0)
        nbytes = self.frames * 4
        if self.aiff:
            comm = struct.pack(">hIh", 2, self.frames, 16) + _ieee754_80_encode(self.rate)
            self.fh.write(b"FORM" + struct.pack(">I", 4 + 8 + len(comm) + 8 + 8 + nbytes) + b"AIFF")
            self.fh.write(b"COMM" + struct.pack(">I", len(comm)) + comm)
            self.fh.write(b"SSND" + struct.pack(">I", 8 + nbytes) + struct.pack(">II", 0, 0))
        else:
            self.fh.write(b"RIFF" + struct.pack("<I", 36 + nbytes) + b"WAVE")
            self.fh.write(b"fmt " + struct.pack("<IHHIIHH", 16, 1, 2, self.rate,
                                                self.rate * 4, 4, 16))
            self.fh.write(b"data" + struct.pack("<I", nbytes))

    def write(self, stereo):
        """Append a float32 array of shape (frames, 2), wrapping on overflow.

        Returns the number of samples that exceeded full scale. The original
        wrapped rather than clipped ("compared to which clipping is a picnic"),
        which is reproduced here; --overflow clip is applied before this point.
        """
        scaled = np.rint(np.clip(stereo, -1e9, 1e9) * 32768.0)
        over = int(np.count_nonzero((scaled > 32767) | (scaled < -32768)))
        wrapped = ((scaled.astype(np.int64) + 32768) % 65536 - 32768).astype(np.int16)
        self.fh.write(wrapped.astype(">i2" if self.aiff else "<i2").tobytes())
        self.frames += len(stereo)
        return over

    def flush(self):
        pos = self.fh.tell()
        self._write_header()
        self.fh.seek(pos)
        self.fh.flush()

    def close(self):
        if not self.fh.closed:
            self.flush()
            self.fh.close()


def fix_header(path):
    """Rebuild the length fields of a truncated AIFF/WAV from its actual size.

    The modern equivalent of Matthew Xavier Mora's "Fix 16bit AIFF", which
    thOnk_0+2 users needed when a render was interrupted.
    """
    size = os.path.getsize(path)
    with open(path, "r+b") as fh:
        head = fh.read(12)
        if head[0:4] == b"FORM":
            fh.seek(0)
            body = fh.read()
            comm_at = body.find(b"COMM")
            ssnd_at = body.find(b"SSND")
            if comm_at < 0 or ssnd_at < 0:
                raise ValueError("%s: no COMM/SSND chunk to fix" % path)
            channels, _frames, bits = struct.unpack(">hIh", body[comm_at + 8:comm_at + 16])
            width = (bits + 7) // 8
            audio_at = ssnd_at + 8 + 8
            frames = (size - audio_at) // (width * channels)
            fh.seek(comm_at + 8 + 2)
            fh.write(struct.pack(">I", frames))
            fh.seek(ssnd_at + 4)
            fh.write(struct.pack(">I", 8 + frames * width * channels))
            fh.seek(4)
            fh.write(struct.pack(">I", size - 8))
        elif head[0:4] == b"RIFF":
            fh.seek(0)
            body = fh.read()
            data_at = body.find(b"data")
            if data_at < 0:
                raise ValueError("%s: no data chunk to fix" % path)
            fh.seek(data_at + 4)
            fh.write(struct.pack("<I", size - (data_at + 8)))
            fh.seek(4)
            fh.write(struct.pack("<I", size - 8))
            frames = None
        else:
            raise ValueError("%s: not an AIFF or WAV file" % path)
    return size, frames


# --------------------------------------------------------------------------
# scores: randomish values at randomish times
# --------------------------------------------------------------------------


class Curve:
    """Randomly placed breakpoints in [lo, hi], continuously interpolated.

    Segment durations are themselves random, within [min_seg, max_seg] - the
    "randomish values at randomish times" of the manual. Interpolation is
    raised-cosine so that parameter motion has no corners.
    """

    def __init__(self, rng, lo, hi, min_seg, max_seg, duration, curve=True):
        self.lo, self.hi, self.curve = lo, hi, curve
        times, values, t = [0.0], [rng.uniform(lo, hi)], 0.0
        while t < duration + max_seg:
            t += rng.uniform(min_seg, max_seg)
            times.append(t)
            values.append(rng.uniform(lo, hi))
        self.t = np.asarray(times, dtype=np.float64)
        self.v = np.asarray(values, dtype=np.float64)

    def at(self, t):
        t = np.asarray(t, dtype=np.float64)
        idx = np.clip(np.searchsorted(self.t, t, side="right") - 1, 0, len(self.t) - 2)
        t0, t1 = self.t[idx], self.t[idx + 1]
        v0, v1 = self.v[idx], self.v[idx + 1]
        frac = np.clip((t - t0) / np.maximum(t1 - t0, 1e-9), 0.0, 1.0)
        if self.curve:
            frac = 0.5 - 0.5 * np.cos(np.pi * frac)
        return v0 + (v1 - v0) * frac


class Parameter:
    """A score curve plus a separately controlled band-limited randomizer.

    The randomizer is smooth noise at a fixed rate; its depth is itself a slow
    curve, so a parameter can drift from steady to jittery and back.
    """

    def __init__(self, rng, lo, hi, min_seg, max_seg, duration,
                 rand_rate=0.0, rand_depth=0.0, curve=True):
        self.base = Curve(rng, lo, hi, min_seg, max_seg, duration, curve)
        self.lo, self.hi = lo, hi
        self.span = hi - lo
        if rand_rate > 0.0 and rand_depth > 0.0:
            seg = 1.0 / rand_rate
            self.noise = Curve(rng, -1.0, 1.0, seg, seg, duration, curve)
            self.depth = Curve(rng, 0.0, rand_depth, 3.0, 30.0, duration, curve)
        else:
            self.noise = self.depth = None

    def at(self, t):
        v = self.base.at(t)
        if self.noise is not None:
            v = v + self.noise.at(t) * self.depth.at(t) * self.span
        return np.clip(v, self.lo, self.hi)


# Score definitions. Ranges are (low, high); seg is the breakpoint spacing
# range in seconds; rand is (rate in Hz, depth as a fraction of the range).
SCORES = {
    "flowing": {
        "description": "long arcs, moderate density - the original's patient setting",
        "duration": 1200.0,
        "density": dict(range=(2.0, 250.0), seg=(8.0, 60.0), rand=(0.7, 0.10)),
        "length": dict(range=(0.020, 0.100), seg=(10.0, 60.0), rand=(0.5, 0.15)),
        "attack": dict(range=(0.15, 0.50), seg=(12.0, 60.0), rand=(0.3, 0.10)),
        "position": dict(range=(0.0, 1.0), seg=(6.0, 45.0), rand=(0.4, 0.03)),
        "transpose": dict(range=(-12.0, 12.0), seg=(10.0, 60.0), rand=(0.2, 0.05)),
        "balance": dict(range=(0.15, 0.85), seg=(4.0, 30.0), rand=(0.6, 0.20)),
        "spread": 0.35,
        "voices": 4,
    },
    "hectic": {
        "description": "dense, fast-shifting, up to 6000 grains a second",
        "duration": 1200.0,
        "density": dict(range=(40.0, 6000.0), seg=(0.5, 8.0), rand=(4.0, 0.25)),
        "length": dict(range=(0.003, 0.060), seg=(0.5, 8.0), rand=(3.0, 0.30)),
        "attack": dict(range=(0.05, 0.45), seg=(1.0, 10.0), rand=(2.0, 0.25)),
        "position": dict(range=(0.0, 1.0), seg=(0.4, 6.0), rand=(3.0, 0.12)),
        "transpose": dict(range=(-24.0, 24.0), seg=(0.8, 10.0), rand=(1.5, 0.20)),
        "balance": dict(range=(0.0, 1.0), seg=(0.3, 4.0), rand=(5.0, 0.35)),
        "spread": 0.8,
        "voices": 4,
    },
    "sparse": {
        "description": "isolated grains, wide silences, slow drift",
        "duration": 1200.0,
        "density": dict(range=(0.5, 25.0), seg=(15.0, 90.0), rand=(0.3, 0.20)),
        "length": dict(range=(0.040, 0.100), seg=(15.0, 90.0), rand=(0.2, 0.20)),
        "attack": dict(range=(0.25, 0.50), seg=(20.0, 90.0), rand=(0.2, 0.10)),
        "position": dict(range=(0.0, 1.0), seg=(10.0, 60.0), rand=(0.2, 0.05)),
        "transpose": dict(range=(-18.0, 7.0), seg=(15.0, 90.0), rand=(0.15, 0.08)),
        "balance": dict(range=(0.0, 1.0), seg=(8.0, 45.0), rand=(0.4, 0.30)),
        "spread": 0.5,
        "voices": 3,
    },
    "stretch1": {
        "description": "the input file dragged across one minute",
        "duration": 60.0,
        "stretch": True,
        "density": dict(range=(120.0, 900.0), seg=(3.0, 15.0), rand=(1.0, 0.10)),
        "length": dict(range=(0.030, 0.100), seg=(5.0, 20.0), rand=(0.5, 0.10)),
        "attack": dict(range=(0.30, 0.50), seg=(5.0, 20.0), rand=(0.3, 0.05)),
        "position": dict(range=(0.0, 0.02), seg=(2.0, 10.0), rand=(0.8, 0.40)),
        "transpose": dict(range=(-5.0, 5.0), seg=(6.0, 25.0), rand=(0.3, 0.10)),
        "balance": dict(range=(0.2, 0.8), seg=(3.0, 15.0), rand=(0.5, 0.25)),
        "spread": 0.4,
        "voices": 2,
    },
    "stretch5": {
        "description": "the input file dragged across five minutes",
        "duration": 300.0,
        "stretch": True,
        "density": dict(range=(150.0, 1200.0), seg=(6.0, 30.0), rand=(0.8, 0.10)),
        "length": dict(range=(0.040, 0.100), seg=(8.0, 40.0), rand=(0.4, 0.10)),
        "attack": dict(range=(0.30, 0.50), seg=(8.0, 40.0), rand=(0.2, 0.05)),
        "position": dict(range=(0.0, 0.01), seg=(4.0, 20.0), rand=(0.6, 0.40)),
        "transpose": dict(range=(-3.0, 3.0), seg=(10.0, 45.0), rand=(0.2, 0.10)),
        "balance": dict(range=(0.2, 0.8), seg=(6.0, 30.0), rand=(0.4, 0.25)),
        "spread": 0.4,
        "voices": 2,
    },
}


# --------------------------------------------------------------------------
# the engine
# --------------------------------------------------------------------------


class Thonk:
    def __init__(self, source, rate, score_name, duration, rng, spread=None):
        if score_name not in SCORES:
            raise ValueError("unknown score: %s" % score_name)
        self.score = SCORES[score_name]
        self.name = score_name
        self.x = source
        self.rate = rate
        self.duration = duration
        self.rng = rng
        self.spread = self.score["spread"] if spread is None else spread
        self.stretch = self.score.get("stretch", False)

        d = self.duration
        mk = lambda spec, curve=True: Parameter(
            rng, spec["range"][0], spec["range"][1], spec["seg"][0], spec["seg"][1],
            d, spec["rand"][0], spec["rand"][1], curve)
        self.p_density = mk(self.score["density"])
        self.p_length = mk(self.score["length"])
        self.p_attack = mk(self.score["attack"])
        self.p_position = mk(self.score["position"])
        self.p_balance = mk(self.score["balance"])

        # A statistical four-voice transposer: each voice has its own slowly
        # drifting interval, and each grain is assigned to one of them.
        nv = self.score["voices"]
        lo, hi = self.score["transpose"]["range"]
        seg = self.score["transpose"]["seg"]
        self.voices = [Curve(rng, lo, hi, seg[0], seg[1], d) for _ in range(nv)]
        w = rng.uniform(0.3, 1.0, size=nv)
        self.voice_cdf = np.cumsum(w / w.sum())

        self.max_len = self.score["length"]["range"][1]
        self.tail = int(self.max_len * rate * 2) + 64
        self.carry = 0.0        # fractional grain left over between blocks
        self.overlap = np.zeros((self.tail, 2), dtype=np.float32)
        self.grains = 0
        self._env_cache = {}

    def _envelope(self, n, atk):
        """Trapezoidal grain window: attack portion up, decay portion down."""
        a = max(1, int(n * atk))
        key = (n, a)
        env = self._env_cache.get(key)
        if env is None:
            env = np.ones(n, dtype=np.float32)
            if 2 * a >= n:
                a = max(1, n // 2)
            env[:a] = np.linspace(0.0, 1.0, a, endpoint=False, dtype=np.float32)
            env[n - a:] = np.linspace(1.0, 0.0, a, dtype=np.float32)
            if len(self._env_cache) < 4096:
                self._env_cache[key] = env
        return env

    def _onsets(self, t0, t1):
        """Grain onset times in [t0, t1), from the integral of the density curve."""
        grid = np.arange(t0, t1 + 1e-9, 0.002)
        if len(grid) < 2:
            return np.empty(0), 0.0
        dens = self.p_density.at(grid)
        integral = np.concatenate(([0.0], np.cumsum(0.5 * (dens[1:] + dens[:-1]) * np.diff(grid))))
        total = integral[-1] + self.carry
        count = int(total)
        if count <= 0:
            self.carry = total
            return np.empty(0), float(np.mean(dens))
        targets = np.arange(count) + 1.0 - self.carry
        onsets = np.interp(targets, integral, grid)
        self.carry = total - count
        return onsets, float(np.mean(dens))

    def render_block(self, t0, t1):
        """Render one block of output, returning a float32 (frames, 2) array."""
        n_out = int(round((t1 - t0) * self.rate))
        buf = np.zeros((n_out + self.tail, 2), dtype=np.float32)
        buf[: self.tail] += self.overlap

        onsets, density = self._onsets(t0, t1)
        if len(onsets):
            lengths = self.p_length.at(onsets)
            attacks = self.p_attack.at(onsets)
            positions = self.p_position.at(onsets)
            balances = np.clip(
                self.p_balance.at(onsets)
                + self.rng.uniform(-self.spread, self.spread, size=len(onsets)),
                0.0, 1.0)
            pick = np.searchsorted(self.voice_cdf, self.rng.random(len(onsets)))
            semis = np.empty(len(onsets))
            for vi, voice in enumerate(self.voices):
                sel = pick == vi
                if sel.any():
                    semis[sel] = voice.at(onsets[sel])
            ratios = 2.0 ** (semis / 12.0)

            if self.stretch:
                # in-file time walks the input from start to end across the
                # whole render, with the position curve as local jitter
                progress = (onsets / max(self.duration, 1e-9))
                positions = np.clip(progress + positions, 0.0, 1.0)

            starts = ((onsets - t0) * self.rate).astype(np.int64)
            nsamps = np.maximum((lengths * self.rate).astype(np.int64), 8)
            src_max = len(self.x) - 2
            # equal-power pan: sin/cos law rather than linear crossfade
            gl = np.cos(balances * (np.pi / 2)).astype(np.float32)
            gr = np.sin(balances * (np.pi / 2)).astype(np.float32)

            x = self.x
            for k in range(len(onsets)):
                n = int(nsamps[k])
                step = ratios[k]
                need = n * step
                origin = positions[k] * max(src_max - need, 0.0)
                idx = origin + step * np.arange(n, dtype=np.float32)
                np.clip(idx, 0.0, src_max, out=idx)
                i0 = idx.astype(np.int64)
                frac = idx - i0
                grain = x[i0] * (1.0 - frac) + x[i0 + 1] * frac
                grain *= self._envelope(n, float(attacks[k]))
                s = int(starts[k])
                buf[s:s + n, 0] += grain * gl[k]
                buf[s:s + n, 1] += grain * gr[k]
            self.grains += len(onsets)

        self.overlap = buf[n_out:n_out + self.tail].copy()
        if len(self.overlap) < self.tail:  # pragma: no cover - short final block
            self.overlap = np.pad(self.overlap, ((0, self.tail - len(self.overlap)), (0, 0)))
        return buf[:n_out], density


# --------------------------------------------------------------------------
# a render in progress
# --------------------------------------------------------------------------


def hms(seconds):
    seconds = int(seconds)
    return "%d:%02d:%02d" % (seconds // 3600, seconds // 60 % 60, seconds % 60)


class Progress:
    """One block's worth of news, handed back by Session.run()."""

    __slots__ = ("t", "duration", "density", "grains", "elapsed", "frames",
                 "overflows", "gain")

    def __init__(self, t, duration, density, grains, elapsed, frames, overflows, gain):
        self.t = t
        self.duration = duration
        self.density = density
        self.grains = grains
        self.elapsed = elapsed
        self.frames = frames
        self.overflows = overflows
        self.gain = gain

    @property
    def fraction(self):
        return min(self.t / self.duration, 1.0) if self.duration else 1.0

    @property
    def speed(self):
        return self.t / max(self.elapsed, 1e-9)


class Session:
    """One render: engine, output file, gain handling and stop flag.

    Driven by iterating run(), which yields a Progress per block. Set
    stop_requested at any time from another thread; the output file is
    finalized either way, so it stays playable.
    """

    def __init__(self, source, in_rate, output, score="flowing", duration=None,
                 rate=None, gain=1.0, overflow="wrap", autogain=False, seed=None,
                 spread=None, block=1.0):
        if len(source) < 64:
            raise ValueError("input file is too short to take grains from")
        self.source = source
        self.in_rate = in_rate
        self.rate = rate or in_rate
        self.score = score
        self.duration = float(duration) if duration else SCORES[score]["duration"]
        self.gain = gain
        self.overflow = overflow
        self.autogain = autogain
        self.block = block
        self.output = output
        self.seed = seed if seed is not None else int.from_bytes(os.urandom(4), "big")
        self.peak = float(np.max(np.abs(source))) or 1.0
        self.engine = Thonk(source, self.rate, score, self.duration,
                            np.random.default_rng(self.seed), spread)
        self.writer = StreamWriter(output, self.rate)
        self.overflows = 0
        self.stop_requested = False
        self.t = 0.0
        self.elapsed = 0.0

    @classmethod
    def from_file(cls, input_path, output, **kwargs):
        source, in_rate = read_audio(input_path)
        return cls(source, in_rate, output, **kwargs)

    @property
    def written_seconds(self):
        return self.writer.frames / self.rate

    def run(self):
        auto_gain = self.gain
        started = time.time()
        try:
            while self.t < self.duration and not self.stop_requested:
                t1 = min(self.t + self.block, self.duration)
                chunk, density = self.engine.render_block(self.t, t1)
                if self.autogain:
                    # The block is in hand before it is written, so the gain can
                    # come from its measured peak rather than from a model of how
                    # grains sum. Drops apply at once and are held flat for the
                    # block, so the ramp can never overshoot; recovery is gradual.
                    peak = float(np.max(np.abs(chunk))) or 1e-9
                    needed = min(self.gain, 0.9 / peak)
                    if needed < auto_gain:
                        chunk = chunk * needed
                        auto_gain = needed
                    else:
                        target = min(needed, auto_gain * 1.35)
                        chunk = chunk * np.linspace(auto_gain, target, len(chunk),
                                                    dtype=np.float32)[:, None]
                        auto_gain = target
                elif self.gain != 1.0:
                    chunk = chunk * self.gain
                if self.overflow == "clip":
                    np.clip(chunk, -1.0, 32767.0 / 32768.0, out=chunk)
                self.overflows += self.writer.write(chunk)
                self.t = t1
                self.elapsed = time.time() - started
                if int(self.t) % 10 == 0:
                    self.writer.flush()
                yield Progress(self.t, self.duration, density, self.engine.grains,
                               self.elapsed, self.writer.frames, self.overflows,
                               auto_gain)
        finally:
            self.writer.close()
            self.elapsed = time.time() - started


# --------------------------------------------------------------------------
# command line
# --------------------------------------------------------------------------


def main(argv=None):
    ap = argparse.ArgumentParser(
        description="thOnk_0+2's granular engine, reimplemented for modern systems.",
        epilog="Feed it a short, transient-rich mono file and leave it running.")
    ap.add_argument("input", nargs="?", help="input sound file (AIFF or WAV)")
    ap.add_argument("output", nargs="?", help="output file; .aiff or .wav")
    ap.add_argument("--score", default="flowing", help="score to use (default: flowing)")
    ap.add_argument("--duration", type=float, default=None,
                    help="output length in seconds (default: the score's own length)")
    ap.add_argument("--rate", type=int, default=None,
                    help="output sample rate (default: same as the input)")
    ap.add_argument("--gain", type=float, default=1.0, help="output gain (default: 1.0)")
    ap.add_argument("--overflow", choices=("wrap", "clip"), default="wrap",
                    help="what to do past full scale; the original wrapped")
    ap.add_argument("--autogain", action="store_true",
                    help="track grain density and back off the gain to keep headroom "
                         "(not original behaviour, but saves attenuating the input)")
    ap.add_argument("--seed", type=int, default=None,
                    help="fix the random seed; omit for a different result every run")
    ap.add_argument("--spread", type=float, default=None,
                    help="per-grain stereo scatter, 0 to 1 (default: per score)")
    ap.add_argument("--quiet", action="store_true", help="no progress console")
    ap.add_argument("--list-scores", action="store_true", help="list scores and exit")
    ap.add_argument("--fix", metavar="FILE",
                    help="repair the header of a truncated output file and exit")
    args = ap.parse_args(argv)

    if args.list_scores:
        for name, score in SCORES.items():
            print("%-10s %-58s default %s"
                  % (name, score["description"], hms(score["duration"])))
        return 0

    if args.fix:
        size, frames = fix_header(args.fix)
        print("fixed %s: %d bytes%s" % (args.fix, size,
                                        "" if frames is None else ", %d frames" % frames))
        return 0

    if not args.input or not args.output:
        ap.error("an input and an output file are required")

    try:
        session = Session.from_file(
            args.input, args.output, score=args.score, duration=args.duration,
            rate=args.rate, gain=args.gain, overflow=args.overflow,
            autogain=args.autogain, seed=args.seed, spread=args.spread)
    except (ValueError, OSError) as exc:
        ap.error(str(exc))

    if not args.quiet:
        print("thonk: %s -> %s" % (args.input, args.output))
        print("  input     %.2f s mono, %d Hz, peak %.2f"
              % (len(session.source) / session.in_rate, session.in_rate, session.peak))
        print("  score     %s - %s" % (args.score, SCORES[args.score]["description"]))
        print("  output    %s stereo, %d Hz, seed %d"
              % (hms(session.duration), session.rate, session.seed))
        print("  stop any time with Ctrl-C; the file stays playable")
        print()

    def on_signal(_sig, _frame):
        session.stop_requested = True

    signal.signal(signal.SIGINT, on_signal)
    try:
        signal.signal(signal.SIGTERM, on_signal)
    except (AttributeError, ValueError):  # pragma: no cover - platform dependent
        pass

    last_print = 0.0
    for progress in session.run():
        if not args.quiet and progress.elapsed - last_print > 0.5:
            last_print = progress.elapsed
            sys.stdout.write(
                "\r  %s / %s   %6.0f grains/s   %9d grains   %4.1fx realtime  "
                % (hms(progress.t), hms(progress.duration), progress.density,
                   progress.grains, progress.speed))
            sys.stdout.flush()

    if not args.quiet:
        print("\r" + " " * 78)
        print("  wrote %s of audio (%d grains) in %s, %.1fx realtime"
              % (hms(session.written_seconds), session.engine.grains,
                 hms(session.elapsed),
                 session.written_seconds / max(session.elapsed, 1e-9)))
        if session.overflows:
            if args.overflow == "clip":
                print("  %d samples hit full scale and were clipped; try a lower --gain"
                      % session.overflows)
            else:
                print("  %d samples went past full scale and wrapped, as the original did;"
                      % session.overflows)
                print("  attenuate the input, or use --gain 0.3, or --overflow clip")
        if session.stop_requested:
            print("  stopped early - %s is complete and playable" % args.output)
    return 0


if __name__ == "__main__":
    sys.exit(main())
