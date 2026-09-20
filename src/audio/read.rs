//! Read AIFF, AIFF-C and WAV into mono `f32` samples.
//!
//! The whole file is read into memory and the chunks are walked with an index.
//! Input files are short, so the cost is small and the code stays simple.
//!
//! Two deliberate differences from a strict reading of the formats, both
//! carried over from `thonk.py`:
//!
//! * The frame count in the AIFF `COMM` chunk is ignored. The sample data
//!   decides the length, so a truncated file still reads.
//! * The AIFF-C compression tag `fl32` is treated as little-endian and `FL32`
//!   as big-endian.

use std::path::Path;

use crate::{Error, Result};

/// How one raw PCM block is laid out.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Encoding {
    /// Two's complement integer samples, 1 to 4 bytes wide.
    SignedPcm { width: usize, big_endian: bool },
    /// The WAV 8-bit form, where 128 is silence.
    Unsigned8,
    /// IEEE 754 single precision, already in the range -1 to 1.
    Float32 { big_endian: bool },
}

impl Encoding {
    fn width(self) -> usize {
        match self {
            Encoding::SignedPcm { width, .. } => width,
            Encoding::Unsigned8 => 1,
            Encoding::Float32 { .. } => 4,
        }
    }
}

/// Read a sound file and return its samples mixed to mono, with the rate.
///
/// Multi-channel input is averaged, because the original accepted mono only.
pub fn read_audio(path: &Path) -> Result<(Vec<f32>, u32)> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    decode(path, &bytes)
}

/// The body of [`read_audio`], split out so tests can work from bytes.
pub fn decode(path: &Path, bytes: &[u8]) -> Result<(Vec<f32>, u32)> {
    if bytes.len() < 12 {
        return Err(Error::format(path, "too short to be an audio file"));
    }
    let (samples, rate, channels) = match (&bytes[0..4], &bytes[8..12]) {
        (b"FORM", b"AIFF") => read_aiff(path, bytes, false)?,
        (b"FORM", b"AIFC") => read_aiff(path, bytes, true)?,
        (b"RIFF", b"WAVE") => read_wav(path, bytes)?,
        _ => return Err(Error::format(path, "not an AIFF or WAV file")),
    };
    Ok((mix_to_mono(samples, channels), rate))
}

/// Average the channels of an interleaved block down to one.
fn mix_to_mono(samples: Vec<f32>, channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples;
    }
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

// --------------------------------------------------------------------------
// chunk walking
// --------------------------------------------------------------------------

/// One chunk of a FORM or RIFF file.
struct Chunk<'a> {
    id: [u8; 4],
    /// The body, clamped to what the file actually holds.
    body: &'a [u8],
    /// Where the next chunk header starts, padded to an even offset.
    next: usize,
}

/// Read the chunk at `pos`, or `None` once no header fits.
fn chunk_at(bytes: &[u8], pos: usize, big_endian: bool) -> Option<Chunk<'_>> {
    if pos + 8 > bytes.len() {
        return None;
    }
    let raw: [u8; 4] = bytes[pos + 4..pos + 8].try_into().ok()?;
    let size = if big_endian {
        u32::from_be_bytes(raw)
    } else {
        u32::from_le_bytes(raw)
    } as usize;
    let start = pos + 8;
    Some(Chunk {
        id: bytes[pos..pos + 4].try_into().ok()?,
        body: &bytes[start..bytes.len().min(start + size)],
        next: start.saturating_add(size).saturating_add(size & 1),
    })
}

// --------------------------------------------------------------------------
// AIFF and AIFF-C
// --------------------------------------------------------------------------

/// Compression tags that hold uncompressed samples this reader understands.
const AIFC_UNCOMPRESSED: [&[u8; 4]; 7] = [
    b"NONE", b"sowt", b"fl32", b"FL32", b"in24", b"in32", b"twos",
];

fn read_aiff(path: &Path, bytes: &[u8], is_aifc: bool) -> Result<(Vec<f32>, u32, usize)> {
    let mut comm: Option<(usize, u16, f64, [u8; 4])> = None;
    let mut sound: Option<&[u8]> = None;

    let mut pos = 12;
    while let Some(chunk) = chunk_at(bytes, pos, true) {
        pos = chunk.next;
        match &chunk.id {
            b"COMM" => {
                let b = chunk.body;
                if b.len() < 18 {
                    return Err(Error::format(path, "COMM chunk is truncated"));
                }
                let channels = i16::from_be_bytes([b[0], b[1]]);
                // b[2..6] is the frame count. The sample data decides instead.
                let bits = i16::from_be_bytes([b[6], b[7]]);
                let rate = ieee754_80_decode(&b[8..18]);
                let compression: [u8; 4] = if is_aifc && b.len() >= 22 {
                    b[18..22].try_into().unwrap()
                } else {
                    *b"NONE"
                };
                if !AIFC_UNCOMPRESSED.contains(&&compression) {
                    return Err(Error::format(
                        path,
                        format!(
                            "compressed AIFF-C ({}) is not supported",
                            String::from_utf8_lossy(&compression)
                        ),
                    ));
                }
                if channels < 1 {
                    return Err(Error::format(path, format!("{channels} channels")));
                }
                if !(1..=32).contains(&bits) {
                    return Err(Error::format(path, format!("{bits}-bit samples")));
                }
                comm = Some((channels as usize, bits as u16, rate, compression));
            }
            b"SSND" => {
                let b = chunk.body;
                if b.len() < 8 {
                    return Err(Error::format(path, "SSND chunk is truncated"));
                }
                let offset = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
                let start = 8usize.saturating_add(offset);
                sound = Some(if start < b.len() { &b[start..] } else { &[] });
            }
            _ => {}
        }
    }

    let (channels, bits, rate, compression) =
        comm.ok_or_else(|| Error::format(path, "AIFF file is missing a COMM or SSND chunk"))?;
    let sound =
        sound.ok_or_else(|| Error::format(path, "AIFF file is missing a COMM or SSND chunk"))?;

    let width = usize::from(bits).div_ceil(8);
    let little = compression == *b"sowt" || compression == *b"fl32";
    let is_float = compression == *b"fl32" || compression == *b"FL32";
    let encoding = if is_float {
        if width != 4 {
            return Err(Error::format(
                path,
                format!("float AIFF-C with {bits}-bit samples"),
            ));
        }
        Encoding::Float32 {
            big_endian: !little,
        }
    } else {
        Encoding::SignedPcm {
            width,
            big_endian: !little,
        }
    };

    let samples = decode_pcm(path, whole_frames(sound, width, channels), encoding)?;
    Ok((samples, round_rate(path, rate)?, channels))
}

/// Decode an 80-bit IEEE 754 extended float, the AIFF sample rate field.
fn ieee754_80_decode(b: &[u8]) -> f64 {
    let expon = u16::from_be_bytes([b[0], b[1]]);
    let himant = u32::from_be_bytes([b[2], b[3], b[4], b[5]]);
    let lomant = u32::from_be_bytes([b[6], b[7], b[8], b[9]]);
    let sign = if expon & 0x8000 != 0 { -1.0 } else { 1.0 };
    let expon = i32::from(expon & 0x7FFF);
    if expon == 0 && himant == 0 && lomant == 0 {
        return 0.0;
    }
    // Each term is skipped when its mantissa is zero, because a zero times an
    // overflowed power of two is NaN rather than zero.
    let hi = if himant == 0 {
        0.0
    } else {
        f64::from(himant) * exp2(expon - 16383 - 31)
    };
    let lo = if lomant == 0 {
        0.0
    } else {
        f64::from(lomant) * exp2(expon - 16383 - 63)
    };
    sign * (hi + lo)
}

/// Two raised to a power, saturating at infinity rather than failing.
fn exp2(e: i32) -> f64 {
    if e > 1023 {
        f64::INFINITY
    } else if e < -1074 {
        0.0
    } else {
        (2.0f64).powi(e)
    }
}

fn round_rate(path: &Path, rate: f64) -> Result<u32> {
    if !rate.is_finite() || rate < 1.0 || rate > 4_000_000_000.0 {
        return Err(Error::format(
            path,
            format!("impossible sample rate: {rate}"),
        ));
    }
    Ok(rate.round() as u32)
}

// --------------------------------------------------------------------------
// WAV
// --------------------------------------------------------------------------

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

fn read_wav(path: &Path, bytes: &[u8]) -> Result<(Vec<f32>, u32, usize)> {
    let mut fmt: Option<(u16, usize, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;

    let mut pos = 12;
    while let Some(chunk) = chunk_at(bytes, pos, false) {
        pos = chunk.next;
        match &chunk.id {
            b"fmt " => {
                let b = chunk.body;
                if b.len() < 16 {
                    return Err(Error::format(path, "fmt chunk is truncated"));
                }
                let mut tag = u16::from_le_bytes([b[0], b[1]]);
                let channels = u16::from_le_bytes([b[2], b[3]]);
                let rate = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                let bits = u16::from_le_bytes([b[14], b[15]]);
                // An extensible header hides the real tag in its subformat.
                if tag == WAVE_FORMAT_EXTENSIBLE && b.len() >= 26 {
                    tag = u16::from_le_bytes([b[24], b[25]]);
                }
                if channels < 1 {
                    return Err(Error::format(path, "0 channels"));
                }
                if !(1..=32).contains(&bits) {
                    return Err(Error::format(path, format!("{bits}-bit samples")));
                }
                fmt = Some((tag, channels as usize, rate, bits));
            }
            b"data" => data = Some(chunk.body),
            _ => {}
        }
    }

    let (tag, channels, rate, bits) =
        fmt.ok_or_else(|| Error::format(path, "WAV file is missing a fmt or data chunk"))?;
    let data =
        data.ok_or_else(|| Error::format(path, "WAV file is missing a fmt or data chunk"))?;

    let width = usize::from(bits).div_ceil(8);
    let encoding = match tag {
        WAVE_FORMAT_FLOAT => {
            if width != 4 {
                return Err(Error::format(
                    path,
                    format!("float WAV with {bits}-bit samples"),
                ));
            }
            Encoding::Float32 { big_endian: false }
        }
        // 8-bit is unsigned in WAV and signed in AIFF.
        WAVE_FORMAT_PCM if width == 1 => Encoding::Unsigned8,
        WAVE_FORMAT_PCM => Encoding::SignedPcm {
            width,
            big_endian: false,
        },
        other => {
            return Err(Error::format(
                path,
                format!("unsupported WAV encoding (format tag {other})"),
            ))
        }
    };

    if rate < 1 {
        return Err(Error::format(path, "sample rate of 0"));
    }
    let samples = decode_pcm(path, whole_frames(data, width, channels), encoding)?;
    Ok((samples, rate, channels))
}

// --------------------------------------------------------------------------
// sample conversion
// --------------------------------------------------------------------------

/// Trim a raw block to a whole number of frames.
fn whole_frames(raw: &[u8], width: usize, channels: usize) -> &[u8] {
    let frame = width * channels;
    if frame == 0 {
        return &[];
    }
    &raw[..(raw.len() / frame) * frame]
}

/// Convert raw interleaved bytes to `f32` samples in the range -1 to 1.
fn decode_pcm(path: &Path, raw: &[u8], encoding: Encoding) -> Result<Vec<f32>> {
    let width = encoding.width();
    let count = raw.len() / width;
    let mut out = Vec::with_capacity(count);

    match encoding {
        Encoding::Unsigned8 => {
            out.extend(raw.iter().map(|b| (f32::from(*b) - 128.0) / 128.0));
        }
        Encoding::Float32 { big_endian } => {
            out.extend(raw.as_chunks::<4>().0.iter().map(|b| {
                if big_endian {
                    f32::from_be_bytes(*b)
                } else {
                    f32::from_le_bytes(*b)
                }
            }));
        }
        Encoding::SignedPcm { width: 1, .. } => {
            out.extend(raw.iter().map(|b| f32::from(*b as i8) / 128.0));
        }
        Encoding::SignedPcm {
            width: 2,
            big_endian,
        } => {
            out.extend(raw.as_chunks::<2>().0.iter().map(|b| {
                let v = if big_endian {
                    i16::from_be_bytes(*b)
                } else {
                    i16::from_le_bytes(*b)
                };
                f32::from(v) / 32768.0
            }));
        }
        Encoding::SignedPcm {
            width: 3,
            big_endian,
        } => {
            out.extend(raw.as_chunks::<3>().0.iter().map(|c| {
                let v = if big_endian {
                    i32::from_be_bytes([0, c[0], c[1], c[2]])
                } else {
                    i32::from_be_bytes([0, c[2], c[1], c[0]])
                };
                // Sign extend the 24-bit value into the top byte.
                ((v << 8) >> 8) as f32 / 8_388_608.0
            }));
        }
        Encoding::SignedPcm {
            width: 4,
            big_endian,
        } => {
            out.extend(raw.as_chunks::<4>().0.iter().map(|b| {
                let v = if big_endian {
                    i32::from_be_bytes(*b)
                } else {
                    i32::from_le_bytes(*b)
                };
                v as f32 / 2_147_483_648.0
            }));
        }
        Encoding::SignedPcm { width, .. } => {
            return Err(Error::format(
                path,
                format!("unsupported sample width: {width} bytes"),
            ))
        }
    }

    debug_assert_eq!(out.len(), count);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> &'static Path {
        Path::new("test")
    }

    /// Byte patterns taken from the Python encoder in `thonk.py`.
    #[test]
    fn extended_float_decodes_known_rates() {
        let cases: [(&[u8; 10], f64); 4] = [
            (b"\x40\x0e\xac\x44\x00\x00\x00\x00\x00\x00", 44100.0),
            (b"\x40\x0d\xac\x44\x00\x00\x00\x00\x00\x00", 22050.0),
            (b"\x40\x0b\xfa\x00\x00\x00\x00\x00\x00\x00", 8000.0),
            (b"\x40\x0e\xbb\x80\x00\x00\x00\x00\x00\x00", 48000.0),
        ];
        for (bytes, want) in cases {
            assert_eq!(ieee754_80_decode(bytes), want, "for {want}");
        }
        assert_eq!(ieee754_80_decode(&[0; 10]), 0.0);
    }

    #[test]
    fn a_huge_exponent_gives_infinity_rather_than_panicking() {
        let bytes = [0x7F, 0xFE, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0];
        assert!(ieee754_80_decode(&bytes).is_infinite());
        assert!(round_rate(p(), f64::INFINITY).is_err());
    }

    #[test]
    fn eight_bit_is_signed_in_aiff_and_unsigned_in_wav() {
        let signed = decode_pcm(
            p(),
            &[0x00, 0x7F, 0x80, 0x40],
            Encoding::SignedPcm {
                width: 1,
                big_endian: true,
            },
        )
        .unwrap();
        let unsigned = decode_pcm(p(), &[0x80, 0xFF, 0x00, 0xC0], Encoding::Unsigned8).unwrap();
        assert_eq!(signed, vec![0.0, 127.0 / 128.0, -1.0, 0.5]);
        assert_eq!(unsigned, vec![0.0, 127.0 / 128.0, -1.0, 0.5]);
    }

    #[test]
    fn sixteen_bit_reads_both_byte_orders() {
        let be = decode_pcm(
            p(),
            &[0x80, 0x00, 0x7F, 0xFF],
            Encoding::SignedPcm {
                width: 2,
                big_endian: true,
            },
        )
        .unwrap();
        let le = decode_pcm(
            p(),
            &[0x00, 0x80, 0xFF, 0x7F],
            Encoding::SignedPcm {
                width: 2,
                big_endian: false,
            },
        )
        .unwrap();
        assert_eq!(be, vec![-1.0, 32767.0 / 32768.0]);
        assert_eq!(be, le);
    }

    #[test]
    fn twenty_four_bit_sign_extends() {
        let be = decode_pcm(
            p(),
            &[0x80, 0x00, 0x00, 0x7F, 0xFF, 0xFF, 0x00, 0x00, 0x00],
            Encoding::SignedPcm {
                width: 3,
                big_endian: true,
            },
        )
        .unwrap();
        let le = decode_pcm(
            p(),
            &[0x00, 0x00, 0x80, 0xFF, 0xFF, 0x7F, 0x00, 0x00, 0x00],
            Encoding::SignedPcm {
                width: 3,
                big_endian: false,
            },
        )
        .unwrap();
        assert_eq!(be, vec![-1.0, 8_388_607.0 / 8_388_608.0, 0.0]);
        assert_eq!(be, le);
    }

    #[test]
    fn thirty_two_bit_integer_scales_to_full_range() {
        let v = decode_pcm(
            p(),
            &[0x80, 0, 0, 0, 0x40, 0, 0, 0],
            Encoding::SignedPcm {
                width: 4,
                big_endian: true,
            },
        )
        .unwrap();
        assert_eq!(v, vec![-1.0, 0.5]);
    }

    #[test]
    fn float_samples_pass_through_unscaled() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&0.25f32.to_be_bytes());
        raw.extend_from_slice(&(-0.75f32).to_be_bytes());
        let v = decode_pcm(p(), &raw, Encoding::Float32 { big_endian: true }).unwrap();
        assert_eq!(v, vec![0.25, -0.75]);
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        assert_eq!(mix_to_mono(vec![1.0, 0.0, -1.0, 1.0], 2), vec![0.5, 0.0]);
        assert_eq!(mix_to_mono(vec![0.3, 0.3], 1), vec![0.3, 0.3]);
    }

    #[test]
    fn a_partial_final_frame_is_dropped() {
        // Five bytes of 16-bit stereo hold one whole frame and a stray byte.
        assert_eq!(whole_frames(&[0; 5], 2, 2).len(), 4);
        assert_eq!(whole_frames(&[0; 3], 2, 1).len(), 2);
    }

    #[test]
    fn junk_is_rejected_with_a_readable_message() {
        assert!(decode(p(), b"short")
            .unwrap_err()
            .to_string()
            .contains("too short"));
        let junk = b"NOPE____NOPE________";
        assert!(decode(p(), junk)
            .unwrap_err()
            .to_string()
            .contains("not an AIFF or WAV file"));
    }
}
