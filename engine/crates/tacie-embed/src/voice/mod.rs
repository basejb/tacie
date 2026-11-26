//! Makes speaker embeddings from utterance WAVs (docs/04 4절 "목소리로 화자 묶기").

mod eres2net;
mod fbank;
mod wav;

pub use eres2net::{VOICE_DIM, VoiceEmbed, VoiceEmbedder, Voiceprint};
pub use fbank::{MEL_BINS, SAMPLE_RATE, fbank, subtract_mean};
pub use wav::pcm16_mono;
