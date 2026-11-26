// Ported from kaldi-native-fbank (https://github.com/csukuangfj/kaldi-native-fbank): feature-window.cc,
// mel-computations.cc and feature-fbank.cc, reduced to the one configuration below and rewritten in Rust. Modified.
//
// Copyright 2009-2012  Karel Vesely;  Petr Motlicek;  Phonexia s.r.o.;  Microsoft Corporation
// Copyright (c)  2022  Xiaomi Corporation (authors: Fangjun Kuang)
//
// Licensed under the Apache License, Version 2.0 (the "License"); you may not use this file except in compliance
// with the License. You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software distributed under the License is distributed
// on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the License for
// the specific language governing permissions and limitations under the License.

//! Kaldi fbank. Ports kaldi-native-fbank (`feature-window.cc`, `mel-computations.cc`, `feature-fbank.cc`) for only
//! the one configuration sherpa-onnx uses for 3D-Speaker models (docs/04 4절; cosine 1.00000 against sherpa-onnx
//! embeddings on 2026-10-30). Frames 25 ms / 10 ms, snip_edges off, no dither, DC removal, pre-emphasis 0.97,
//! povey window, FFT 512, power, 80 mel bins (20–7,600 Hz), no energy, log.

use std::sync::{Arc, OnceLock};

use rustfft::{Fft, FftPlanner, num_complex::Complex};

pub const SAMPLE_RATE: u32 = 16_000;
pub const MEL_BINS: usize = 80;

const FRAME_LEN: usize = 400;
const FRAME_SHIFT: usize = 160;
const FFT_LEN: usize = 512;
const PREEMPH: f32 = 0.97;
const LOW_FREQ: f32 = 20.0;
/// 0 이하면 나이퀴스트에서 뺀 값이에요(-400 → 7,600 Hz).
const HIGH_FREQ: f32 = -400.0;

struct Tables {
    window: Vec<f32>,
    /// 멜 필터마다 (첫 FFT 빈, 가중치들)이에요.
    mel: Vec<(usize, Vec<f32>)>,
    fft: Arc<dyn Fft<f32>>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let a = 2.0 * std::f64::consts::PI / (FRAME_LEN - 1) as f64;
        let window = (0..FRAME_LEN)
            .map(|i| (0.5 - 0.5 * (a * i as f64).cos()).powf(0.85) as f32)
            .collect();
        Tables {
            window,
            mel: mel_banks(),
            fft: FftPlanner::new().plan_fft_forward(FFT_LEN),
        }
    })
}

fn mel_scale(f: f32) -> f32 {
    1127.0 * (1.0 + f / 700.0).ln()
}

fn mel_banks() -> Vec<(usize, Vec<f32>)> {
    let nyquist = SAMPLE_RATE as f32 / 2.0;
    let high = if HIGH_FREQ <= 0.0 {
        nyquist + HIGH_FREQ
    } else {
        HIGH_FREQ
    };
    let bin_width = SAMPLE_RATE as f32 / FFT_LEN as f32;
    let (lo, hi) = (mel_scale(LOW_FREQ), mel_scale(high));
    let delta = (hi - lo) / (MEL_BINS + 1) as f32;
    (0..MEL_BINS)
        .map(|b| {
            let left = lo + b as f32 * delta;
            let center = left + delta;
            let right = center + delta;
            let mut first = None;
            let mut weights = Vec::new();
            // Kaldi는 나이퀴스트 빈(256)을 쓰지 않아요.
            for i in 0..FFT_LEN / 2 {
                let mel = mel_scale(bin_width * i as f32);
                if mel > left && mel < right {
                    first.get_or_insert(i);
                    weights.push(if mel <= center {
                        (mel - left) / (center - left)
                    } else {
                        (right - mel) / (right - center)
                    });
                }
            }
            (first.unwrap_or(0), weights)
        })
        .collect()
}

/// 로그 멜 에너지예요(프레임마다 80개, 평균 빼기 전). 프레임 수는 (n + 80) / 160이에요.
pub fn fbank(samples: &[f32]) -> Vec<[f32; MEL_BINS]> {
    let n = samples.len();
    let frames = (n + FRAME_SHIFT / 2) / FRAME_SHIFT;
    if frames == 0 {
        return Vec::new();
    }
    let t = tables();
    let mut out = Vec::with_capacity(frames);
    let mut w = vec![0f32; FRAME_LEN];
    let mut buf = vec![Complex::new(0f32, 0f32); FFT_LEN];
    for f in 0..frames {
        // snip_edges=false: 프레임 가운데가 f * 160 + 80이고, 범위 밖 샘플은 끝에서 반사해요.
        let start = (f * FRAME_SHIFT + FRAME_SHIFT / 2) as i64 - (FRAME_LEN / 2) as i64;
        for (s, x) in w.iter_mut().enumerate() {
            let mut k = start + s as i64;
            while k < 0 || k >= n as i64 {
                k = if k < 0 { -k - 1 } else { 2 * n as i64 - 1 - k };
            }
            *x = samples[k as usize];
        }
        let mean = w.iter().sum::<f32>() / FRAME_LEN as f32;
        w.iter_mut().for_each(|x| *x -= mean);
        for i in (1..FRAME_LEN).rev() {
            w[i] -= PREEMPH * w[i - 1];
        }
        w[0] -= PREEMPH * w[0];
        for (i, c) in buf.iter_mut().enumerate() {
            *c = Complex::new(
                if i < FRAME_LEN {
                    w[i] * t.window[i]
                } else {
                    0.0
                },
                0.0,
            );
        }
        t.fft.process(&mut buf);
        let mut row = [0f32; MEL_BINS];
        for (b, (first, weights)) in t.mel.iter().enumerate() {
            let e: f32 = weights
                .iter()
                .enumerate()
                .map(|(j, wt)| wt * buf[first + j].norm_sqr())
                .sum();
            row[b] = e.max(f32::EPSILON).ln();
        }
        out.push(row);
    }
    out
}

/// 발화 전체에서 멜마다 평균을 빼요(모델 메타데이터의 `feature_normalize_type: global-mean`).
pub fn subtract_mean(frames: &mut [[f32; MEL_BINS]]) {
    if frames.is_empty() {
        return;
    }
    let mut mean = [0f32; MEL_BINS];
    for f in frames.iter() {
        for (m, x) in mean.iter_mut().zip(f) {
            *m += x;
        }
    }
    let n = frames.len() as f32;
    mean.iter_mut().for_each(|m| *m /= n);
    for f in frames.iter_mut() {
        for (x, m) in f.iter_mut().zip(&mean) {
            *x -= m;
        }
    }
}
