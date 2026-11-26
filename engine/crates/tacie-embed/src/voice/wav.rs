//! Reads the utterance WAV the app sends (16 kHz mono 16-bit PCM) into samples in [-1, 1].

use anyhow::{Context, bail};

use super::fbank::SAMPLE_RATE;

/// RIFF 청크를 돌며 `fmt `와 `data`를 찾아요. 16 kHz mono 16-bit PCM이 아니면 오류예요. 값은 ÷32768이에요.
pub fn pcm16_mono(wav: &[u8]) -> anyhow::Result<Vec<f32>> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        bail!("wav: not a RIFF/WAVE file");
    }
    let mut at = 12;
    let mut format_ok = false;
    while at + 8 <= wav.len() {
        let id = &wav[at..at + 4];
        let size = u32::from_le_bytes(wav[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &wav[at + 8..(at + 8).saturating_add(size).min(wav.len())];
        match id {
            b"fmt " => {
                let field = |o: usize| -> anyhow::Result<u32> {
                    let b = body.get(o..o + 2).context("wav: short fmt chunk")?;
                    Ok(u32::from(u16::from_le_bytes([b[0], b[1]])))
                };
                let rate = body
                    .get(4..8)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .context("wav: short fmt chunk")?;
                let (format, channels, bits) = (field(0)?, field(2)?, field(14)?);
                if format != 1 || channels != 1 || bits != 16 || rate != SAMPLE_RATE {
                    bail!(
                        "wav: need 16 kHz mono 16-bit PCM, got format {format}, {channels} ch, {bits} bit, {rate} Hz"
                    );
                }
                format_ok = true;
            }
            b"data" => {
                if !format_ok {
                    bail!("wav: data before fmt");
                }
                return Ok(body
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
                    .collect());
            }
            _ => {}
        }
        // 청크는 짝수 바이트로 채워져요.
        at += 8 + size + (size & 1);
    }
    bail!("wav: no data chunk")
}
