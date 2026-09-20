//! Stream 16-bit stereo AIFF or WAV, with the header patched as it goes.
//!
//! The file on disk is a valid, playable sound file from the first flush
//! onward, so a render can be abandoned at any point without loss. The header
//! is rewritten on every flush and once more when the writer is dropped, which
//! is what makes Ctrl-C safe.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Bytes per output frame: two channels of 16-bit samples.
const FRAME_BYTES: u64 = 4;

pub struct StreamWriter {
    out: BufWriter<File>,
    path: PathBuf,
    rate: u32,
    frames: u64,
    aiff: bool,
    /// Set once the header is final, so `Drop` does not write it again.
    finished: bool,
}

impl StreamWriter {
    /// Create the output file and write its first header.
    ///
    /// The extension picks the format. Anything that is not `.aiff`, `.aif` or
    /// `.aifc` is written as WAV, as the Python version does.
    pub fn create(path: &Path, rate: u32) -> Result<Self> {
        let aiff = matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("aiff" | "aif" | "aifc")
        );
        let file = File::create(path).map_err(|e| Error::io(path, e))?;
        let mut writer = StreamWriter {
            out: BufWriter::new(file),
            path: path.to_path_buf(),
            rate,
            frames: 0,
            aiff,
            finished: false,
        };
        writer.write_header()?;
        Ok(writer)
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn is_aiff(&self) -> bool {
        self.aiff
    }

    /// Where the samples begin: 54 bytes for AIFF, 44 for WAV.
    pub fn data_start(&self) -> u64 {
        self.header_bytes().len() as u64
    }

    /// Append a block of frames, and report how many samples passed full scale.
    ///
    /// The original wrapped rather than clipped, because clip detection was too
    /// expensive at 600 layers. Wrapping is reproduced here. `--overflow clip`
    /// is applied by the caller, before this point.
    pub fn write(&mut self, block: &[[f32; 2]]) -> Result<u64> {
        let mut over = 0u64;
        let mut bytes = Vec::with_capacity(block.len() * FRAME_BYTES as usize);
        for frame in block {
            for sample in frame {
                let scaled = if sample.is_finite() {
                    (f64::from(*sample).clamp(-1e9, 1e9) * 32768.0).round()
                } else {
                    // Python casts a NaN to an undefined integer. Silence is a
                    // better answer, and the engine never produces one.
                    0.0
                };
                if scaled > 32767.0 || scaled < -32768.0 {
                    over += 1;
                }
                // rem_euclid, not %: Rust's remainder keeps the sign of the
                // left operand, where Python's modulo does not.
                let wrapped = ((scaled as i64 + 32768).rem_euclid(65536) - 32768) as i16;
                if self.aiff {
                    bytes.extend_from_slice(&wrapped.to_be_bytes());
                } else {
                    bytes.extend_from_slice(&wrapped.to_le_bytes());
                }
            }
        }
        self.out
            .write_all(&bytes)
            .map_err(|e| Error::io(&self.path, e))?;
        self.frames += block.len() as u64;
        Ok(over)
    }

    /// Rewrite the header for the frames written so far, then flush.
    pub fn flush(&mut self) -> Result<()> {
        let here = self.out.stream_position().map_err(|e| self.err(e))?;
        self.write_header()?;
        self.out
            .seek(SeekFrom::Start(here))
            .map_err(|e| self.err(e))?;
        self.out.flush().map_err(|e| self.err(e))?;
        Ok(())
    }

    /// Finalize the header and close the file, reporting any failure.
    pub fn close(mut self) -> Result<()> {
        self.finish()
    }

    fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        self.flush()
    }

    fn write_header(&mut self) -> Result<()> {
        let header = self.header_bytes();
        self.out.seek(SeekFrom::Start(0)).map_err(|e| self.err(e))?;
        self.out.write_all(&header).map_err(|e| self.err(e))?;
        Ok(())
    }

    fn header_bytes(&self) -> Vec<u8> {
        let data_bytes = self.frames * FRAME_BYTES;
        let mut h = Vec::with_capacity(64);
        if self.aiff {
            // COMM: 2 channels, the frame count, 16 bits, the rate.
            let mut comm = Vec::with_capacity(18);
            comm.extend_from_slice(&2i16.to_be_bytes());
            comm.extend_from_slice(&(self.frames as u32).to_be_bytes());
            comm.extend_from_slice(&16i16.to_be_bytes());
            comm.extend_from_slice(&ieee754_80_encode(f64::from(self.rate)));

            let form_size = 4 + 8 + comm.len() as u64 + 8 + 8 + data_bytes;
            h.extend_from_slice(b"FORM");
            h.extend_from_slice(&(form_size as u32).to_be_bytes());
            h.extend_from_slice(b"AIFF");
            h.extend_from_slice(b"COMM");
            h.extend_from_slice(&(comm.len() as u32).to_be_bytes());
            h.extend_from_slice(&comm);
            h.extend_from_slice(b"SSND");
            h.extend_from_slice(&((8 + data_bytes) as u32).to_be_bytes());
            h.extend_from_slice(&0u32.to_be_bytes()); // offset
            h.extend_from_slice(&0u32.to_be_bytes()); // block size
        } else {
            h.extend_from_slice(b"RIFF");
            h.extend_from_slice(&((36 + data_bytes) as u32).to_le_bytes());
            h.extend_from_slice(b"WAVE");
            h.extend_from_slice(b"fmt ");
            h.extend_from_slice(&16u32.to_le_bytes());
            h.extend_from_slice(&1u16.to_le_bytes()); // PCM
            h.extend_from_slice(&2u16.to_le_bytes()); // channels
            h.extend_from_slice(&self.rate.to_le_bytes());
            h.extend_from_slice(&(self.rate * 4).to_le_bytes()); // bytes per second
            h.extend_from_slice(&4u16.to_le_bytes()); // block align
            h.extend_from_slice(&16u16.to_le_bytes()); // bits
            h.extend_from_slice(b"data");
            h.extend_from_slice(&(data_bytes as u32).to_le_bytes());
        }
        h
    }

    fn err(&self, e: std::io::Error) -> Error {
        Error::io(&self.path, e)
    }
}

impl Drop for StreamWriter {
    /// Leave a playable file behind even when the render unwinds.
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

/// Encode a number as an 80-bit IEEE 754 extended float, for the AIFF rate.
pub fn ieee754_80_encode(value: f64) -> [u8; 10] {
    let mut out = [0u8; 10];
    let (sign, value) = if value < 0.0 {
        (0x8000u16, -value)
    } else {
        (0u16, value)
    };
    if value == 0.0 || !value.is_finite() {
        out[0..2].copy_from_slice(&sign.to_be_bytes());
        return out;
    }

    let (mant, exp) = frexp(value);
    // frexp gives a mantissa in [0.5, 1). The format wants [1, 2), and biases
    // the exponent by 16383.
    let field = (16382 + exp) as u16;
    let mant = mant * 2.0;
    let himant = (mant * 2f64.powi(31)) as u64;
    let frac = mant * 2f64.powi(31) - himant as f64;
    let lomant = (frac * 2f64.powi(32)) as u64;

    out[0..2].copy_from_slice(&(sign | field).to_be_bytes());
    out[2..6].copy_from_slice(&((himant & 0xFFFF_FFFF) as u32).to_be_bytes());
    out[6..10].copy_from_slice(&((lomant & 0xFFFF_FFFF) as u32).to_be_bytes());
    out
}

/// Split a number into a mantissa in [0.5, 1) and a power of two.
///
/// The standard library has no `frexp`, and the encoder needs one.
fn frexp(x: f64) -> (f64, i32) {
    if x == 0.0 || !x.is_finite() {
        return (x, 0);
    }
    let bits = x.to_bits();
    let raw = ((bits >> 52) & 0x7FF) as i32;
    if raw == 0 {
        // A subnormal has no leading one. Scale it up and correct afterwards.
        let (m, e) = frexp(x * 2f64.powi(64));
        return (m, e - 64);
    }
    // Replace the exponent field with the one that puts the value in [0.5, 1).
    let mantissa = f64::from_bits((bits & !(0x7FFu64 << 52)) | (1022u64 << 52));
    (mantissa, raw - 1022)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::read::decode;

    /// The same vectors the decoder test uses, from the Python encoder.
    #[test]
    fn extended_float_encodes_known_rates() {
        let cases: [(f64, &[u8; 10]); 4] = [
            (44100.0, b"\x40\x0e\xac\x44\x00\x00\x00\x00\x00\x00"),
            (22050.0, b"\x40\x0d\xac\x44\x00\x00\x00\x00\x00\x00"),
            (8000.0, b"\x40\x0b\xfa\x00\x00\x00\x00\x00\x00\x00"),
            (48000.0, b"\x40\x0e\xbb\x80\x00\x00\x00\x00\x00\x00"),
        ];
        for (value, want) in cases {
            assert_eq!(&ieee754_80_encode(value), want, "for {value}");
        }
        assert_eq!(ieee754_80_encode(0.0), [0u8; 10]);
    }

    #[test]
    fn frexp_splits_the_way_the_c_function_does() {
        for (x, m, e) in [
            (1.0, 0.5, 1),
            (0.5, 0.5, 0),
            (44100.0, 44100.0 / 65536.0, 16),
        ] {
            assert_eq!(frexp(x), (m, e), "for {x}");
        }
        let (m, e) = frexp(f64::MIN_POSITIVE / 4.0);
        assert_eq!(m, 0.5);
        assert_eq!(e, -1023);
    }

    #[test]
    fn a_written_header_reads_back_at_the_same_rate() {
        let dir = tempdir();
        for (name, rate) in [("a.aiff", 44100u32), ("b.wav", 22050), ("c.aifc", 96000)] {
            let path = dir.join(name);
            let mut w = StreamWriter::create(&path, rate).unwrap();
            w.write(&[[0.25, -0.25]; 10]).unwrap();
            let start = w.data_start();
            w.close().unwrap();

            assert_eq!(start, if name.ends_with(".wav") { 44 } else { 54 });
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(bytes.len() as u64, start + 10 * 4);
            let (samples, got_rate) = decode(&path, &bytes).unwrap();
            assert_eq!(got_rate, rate, "{name}");
            // Two channels of +0.25 and -0.25 average to silence.
            assert_eq!(samples.len(), 10);
            assert!(samples.iter().all(|v| v.abs() < 1e-6));
        }
    }

    #[test]
    fn full_scale_wraps_around_rather_than_clipping() {
        let dir = tempdir();
        let path = dir.join("wrap.aiff");
        let mut w = StreamWriter::create(&path, 44100).unwrap();
        // -1.0 is the last value that fits. Anything above +32767 wraps.
        let over = w
            .write(&[
                [-1.0, 1.0],               // -32768 fits, +32768 wraps to -32768
                [0.5, -0.5],               // inside
                [2.0, -2.0],               // +65536 wraps to 0, -65536 wraps to 0
                [32767.0 / 32768.0, -1.0], // both fit
            ])
            .unwrap();
        w.close().unwrap();

        assert_eq!(over, 3, "one sample per value past full scale");
        let bytes = std::fs::read(&path).unwrap();
        let pcm = &bytes[54..];
        let got: Vec<i16> = pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_be_bytes(*b))
            .collect();
        assert_eq!(
            got,
            vec![-32768, -32768, 16384, -16384, 0, 0, 32767, -32768]
        );
    }

    #[test]
    fn a_dropped_writer_still_leaves_a_playable_file() {
        let dir = tempdir();
        let path = dir.join("dropped.wav");
        {
            let mut w = StreamWriter::create(&path, 8000).unwrap();
            w.write(&[[0.1, 0.1]; 400]).unwrap();
            // No close. The header must still be right after the drop.
        }
        let (samples, rate) = crate::audio::read::read_audio(&path).unwrap();
        assert_eq!(rate, 8000);
        assert_eq!(samples.len(), 400);
    }

    #[test]
    fn a_file_is_readable_part_way_through_a_render() {
        let dir = tempdir();
        let path = dir.join("partial.aiff");
        let mut w = StreamWriter::create(&path, 44100).unwrap();
        w.write(&[[0.5, 0.5]; 100]).unwrap();
        w.flush().unwrap();
        let (samples, _) = crate::audio::read::read_audio(&path).unwrap();
        assert_eq!(
            samples.len(),
            100,
            "the flushed header must count 100 frames"
        );

        w.write(&[[0.5, 0.5]; 50]).unwrap();
        w.flush().unwrap();
        let (samples, _) = crate::audio::read::read_audio(&path).unwrap();
        assert_eq!(samples.len(), 150);
        w.close().unwrap();
    }

    #[test]
    fn a_non_finite_sample_is_written_as_silence() {
        let dir = tempdir();
        let path = dir.join("nan.wav");
        let mut w = StreamWriter::create(&path, 8000).unwrap();
        w.write(&[[f32::NAN, f32::INFINITY], [f32::NEG_INFINITY, 0.5]])
            .unwrap();
        w.close().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let got: Vec<i16> = bytes[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect();
        assert_eq!(got, vec![0, 0, 0, 16384]);
    }

    /// A scratch directory that the operating system cleans up.
    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "thonkr-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
