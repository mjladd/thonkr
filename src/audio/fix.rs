//! Rebuild the length fields of a truncated output file.
//!
//! This is the modern equivalent of Matthew Xavier Mora's "Fix 16bit AIFF",
//! which thOnk_0+2 users needed when a render was interrupted. The Rust writer
//! finalizes its own header, so this is for files another program left broken.
//!
//! The chunk sizes in a truncated file are exactly what cannot be trusted, so
//! the chunks are found by searching for their tags rather than by walking the
//! sizes. Only the start of the file is searched, because a header sits there
//! and an output file can run to gigabytes.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::{Error, Result};

/// How much of the file to search for the chunk tags.
const SEARCH_BYTES: usize = 64 * 1024;

/// What a repair did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repair {
    /// The size the length fields were rebuilt from.
    pub size: u64,
    /// The frame count written into an AIFF `COMM` chunk. WAV has no such field.
    pub frames: Option<u64>,
}

/// Rebuild the length fields of an AIFF or WAV file from its size on disk.
pub fn fix_header(path: &Path) -> Result<Repair> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| Error::io(path, e))?;
    let size = file.metadata().map_err(|e| Error::io(path, e))?.len();

    let mut head = vec![0u8; SEARCH_BYTES.min(size as usize)];
    file.read_exact(&mut head).map_err(|e| Error::io(path, e))?;
    if head.len() < 12 {
        return Err(Error::format(path, "too short to be an audio file"));
    }

    match &head[0..4] {
        b"FORM" => fix_aiff(path, &mut file, &head, size),
        b"RIFF" => fix_wav(path, &mut file, &head, size),
        _ => Err(Error::format(path, "not an AIFF or WAV file")),
    }
}

fn fix_aiff(path: &Path, file: &mut std::fs::File, head: &[u8], size: u64) -> Result<Repair> {
    let comm_at =
        find(head, b"COMM").ok_or_else(|| Error::format(path, "no COMM/SSND chunk to fix"))?;
    let ssnd_at =
        find(head, b"SSND").ok_or_else(|| Error::format(path, "no COMM/SSND chunk to fix"))?;
    if comm_at + 16 > head.len() {
        return Err(Error::format(path, "COMM chunk is truncated"));
    }

    let channels = i16::from_be_bytes([head[comm_at + 8], head[comm_at + 9]]);
    let bits = i16::from_be_bytes([head[comm_at + 14], head[comm_at + 15]]);
    if channels < 1 || !(1..=32).contains(&bits) {
        return Err(Error::format(
            path,
            format!("COMM says {channels} channels of {bits} bits"),
        ));
    }
    let width = u64::from(bits as u16).div_ceil(8);
    // The samples start after the SSND tag, its size, and its two long fields.
    let audio_at = (ssnd_at + 8 + 8) as u64;
    if size < audio_at {
        return Err(Error::format(
            path,
            "the file ends before its samples begin",
        ));
    }
    let frame = width * channels as u64;
    let frames = (size - audio_at) / frame;

    write_at(
        path,
        file,
        (comm_at + 8 + 2) as u64,
        &(frames as u32).to_be_bytes(),
    )?;
    write_at(
        path,
        file,
        (ssnd_at + 4) as u64,
        &((8 + frames * frame) as u32).to_be_bytes(),
    )?;
    write_at(path, file, 4, &((size - 8) as u32).to_be_bytes())?;
    file.flush().map_err(|e| Error::io(path, e))?;

    Ok(Repair {
        size,
        frames: Some(frames),
    })
}

fn fix_wav(path: &Path, file: &mut std::fs::File, head: &[u8], size: u64) -> Result<Repair> {
    let data_at = find(head, b"data").ok_or_else(|| Error::format(path, "no data chunk to fix"))?;
    let audio_at = (data_at + 8) as u64;
    if size < audio_at {
        return Err(Error::format(
            path,
            "the file ends before its samples begin",
        ));
    }

    write_at(
        path,
        file,
        (data_at + 4) as u64,
        &((size - audio_at) as u32).to_le_bytes(),
    )?;
    write_at(path, file, 4, &((size - 8) as u32).to_le_bytes())?;
    file.flush().map_err(|e| Error::io(path, e))?;

    Ok(Repair { size, frames: None })
}

/// The offset of the first occurrence of `tag`.
fn find(haystack: &[u8], tag: &[u8; 4]) -> Option<usize> {
    haystack.windows(4).position(|w| w == tag)
}

fn write_at(path: &Path, file: &mut std::fs::File, at: u64, bytes: &[u8]) -> Result<()> {
    file.seek(SeekFrom::Start(at))
        .map_err(|e| Error::io(path, e))?;
    file.write_all(bytes).map_err(|e| Error::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::read::read_audio;
    use crate::audio::write::StreamWriter;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("thonkr-fix-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// Write a file, then break its header the way a crash would.
    fn broken(name: &str, rate: u32, frames: usize) -> std::path::PathBuf {
        let path = scratch(name);
        let mut w = StreamWriter::create(&path, rate).unwrap();
        w.write(&vec![[0.5, -0.5]; frames]).unwrap();
        w.close().unwrap();

        // A crash leaves the length fields as the very first header wrote
        // them, when no frames had been written yet.
        let mut bytes = std::fs::read(&path).unwrap();
        if name.ends_with(".wav") {
            bytes[4..8].copy_from_slice(&36u32.to_le_bytes()); // RIFF size
            bytes[40..44].copy_from_slice(&0u32.to_le_bytes()); // data size
        } else {
            bytes[4..8].copy_from_slice(&46u32.to_be_bytes()); // FORM size
            bytes[22..26].copy_from_slice(&0u32.to_be_bytes()); // COMM frames
            bytes[42..46].copy_from_slice(&8u32.to_be_bytes()); // SSND size
        }
        std::fs::write(&path, &bytes).unwrap();
        path
    }

    #[test]
    fn a_truncated_aiff_gets_its_length_back() {
        let path = broken("broken.aiff", 44100, 500);
        let (before, _) = read_audio(&path).unwrap();
        assert_eq!(before.len(), 0, "the broken header must claim nothing");

        let repair = fix_header(&path).unwrap();
        assert_eq!(repair.frames, Some(500));
        assert_eq!(repair.size, 54 + 500 * 4);

        let (after, rate) = read_audio(&path).unwrap();
        assert_eq!(rate, 44100);
        assert_eq!(after.len(), 500);
        assert!(
            after.iter().all(|v| v.abs() < 1e-6),
            "left and right cancel"
        );
    }

    #[test]
    fn a_truncated_wav_gets_its_length_back() {
        let path = broken("broken.wav", 22050, 321);
        let repair = fix_header(&path).unwrap();
        assert_eq!(repair.frames, None, "WAV carries no frame count");
        assert_eq!(repair.size, 44 + 321 * 4);

        let (after, rate) = read_audio(&path).unwrap();
        assert_eq!(rate, 22050);
        assert_eq!(after.len(), 321);
    }

    #[test]
    fn a_partial_final_frame_is_not_counted() {
        let path = broken("odd.aiff", 8000, 100);
        // Cut three bytes, so the last frame is incomplete.
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 3]).unwrap();

        let repair = fix_header(&path).unwrap();
        assert_eq!(repair.frames, Some(99));
        let (after, _) = read_audio(&path).unwrap();
        assert_eq!(after.len(), 99);
    }

    #[test]
    fn repairing_twice_changes_nothing_the_second_time() {
        let path = broken("twice.aiff", 44100, 64);
        fix_header(&path).unwrap();
        let once = std::fs::read(&path).unwrap();
        fix_header(&path).unwrap();
        assert_eq!(once, std::fs::read(&path).unwrap());
    }

    #[test]
    fn a_file_that_is_not_audio_is_refused() {
        let path = scratch("notaudio.aiff");
        std::fs::write(&path, b"this is not a sound file at all").unwrap();
        assert!(fix_header(&path)
            .unwrap_err()
            .to_string()
            .contains("not an AIFF or WAV file"));
    }

    #[test]
    fn a_header_with_no_chunk_tags_is_refused() {
        let path = scratch("nochunks.aiff");
        let mut bytes = b"FORM\x00\x00\x00\x04AIFF".to_vec();
        bytes.extend_from_slice(&[0u8; 40]);
        std::fs::write(&path, &bytes).unwrap();
        assert!(fix_header(&path)
            .unwrap_err()
            .to_string()
            .contains("no COMM/SSND chunk to fix"));
    }
}
