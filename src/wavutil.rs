//! Small WAV helpers (concat for full-utterance replay).

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Concatenate mono PCM WAV chunks into one file (same format required).
///
/// Used so Replay plays the full multi-chunk utterance, not only the last chunk.
pub fn concat_wav_files(paths: &[PathBuf], out: &Path) -> Result<()> {
    if paths.is_empty() {
        bail!("no wav paths to concatenate");
    }
    if paths.len() == 1 {
        std::fs::copy(&paths[0], out)
            .with_context(|| format!("copy {} → {}", paths[0].display(), out.display()))?;
        return Ok(());
    }

    let first = std::fs::read(&paths[0]).with_context(|| format!("read {}", paths[0].display()))?;
    let (sample_rate, channels, bits) = parse_fmt(&first)?;
    let mut pcm: Vec<u8> = Vec::new();
    pcm.extend_from_slice(data_payload(&first)?);

    for p in paths.iter().skip(1) {
        let bytes = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
        let (sr, ch, b) = parse_fmt(&bytes)?;
        if sr != sample_rate || ch != channels || b != bits {
            bail!(
                "wav format mismatch: {} ({}Hz {}ch {}bit) vs first ({}Hz {}ch {}bit)",
                p.display(),
                sr,
                ch,
                b,
                sample_rate,
                channels,
                bits
            );
        }
        pcm.extend_from_slice(data_payload(&bytes)?);
    }

    write_pcm_wav(out, &pcm, sample_rate, channels, bits)
}

fn parse_fmt(bytes: &[u8]) -> Result<(u32, u16, u16)> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a WAV");
    }
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let data_start = offset + 8;
        if id == b"fmt " && size >= 16 {
            let channels =
                u16::from_le_bytes(bytes[data_start + 2..data_start + 4].try_into().unwrap());
            let sample_rate =
                u32::from_le_bytes(bytes[data_start + 4..data_start + 8].try_into().unwrap());
            let bits =
                u16::from_le_bytes(bytes[data_start + 14..data_start + 16].try_into().unwrap());
            return Ok((sample_rate, channels, bits));
        }
        offset = data_start.saturating_add(size);
        if size % 2 == 1 {
            offset += 1;
        }
        if size == 0 {
            break;
        }
    }
    bail!("missing fmt chunk");
}

fn data_payload(bytes: &[u8]) -> Result<&[u8]> {
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let data_start = offset + 8;
        let data_end = data_start.saturating_add(size).min(bytes.len());
        if id == b"data" {
            return Ok(&bytes[data_start..data_end]);
        }
        offset = data_end;
        if size % 2 == 1 {
            offset += 1;
        }
        if size == 0 {
            break;
        }
    }
    bail!("missing data chunk");
}

fn write_pcm_wav(path: &Path, pcm: &[u8], sample_rate: u32, channels: u16, bits: u16) -> Result<()> {
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits) / 8;
    let block_align = channels * bits / 8;
    let data_len = pcm.len() as u32;
    let riff_len = 36u32 + data_len;

    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, out).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn write_silence(path: &Path, samples: usize, sr: u32) {
        let pcm = vec![0u8; samples * 2];
        write_pcm_wav(path, &pcm, sr, 1, 16).unwrap();
    }

    #[test]
    fn concat_two_chunks_doubles_pcm() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("yapper-wav-concat-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.wav");
        let b = dir.join("b.wav");
        let out = dir.join("full.wav");
        write_silence(&a, 1000, 24000);
        write_silence(&b, 2000, 24000);
        concat_wav_files(&[a, b], &out).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let payload = data_payload(&bytes).unwrap();
        assert_eq!(payload.len(), 3000 * 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concat_single_is_copy() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("yapper-wav-one-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.wav");
        let out = dir.join("full.wav");
        write_silence(&a, 500, 16000);
        concat_wav_files(&[a.clone()], &out).unwrap();
        assert!(out.is_file());
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&out).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
