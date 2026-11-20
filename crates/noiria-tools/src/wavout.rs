//! 生成した波形を 16bit PCM WAV として書き出す（gen-assets / gen-demo-assets / gen_demo_audio 共通）。

use std::path::Path;

use anyhow::Result;

use crate::synth::{Stereo, SR};

pub fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// `samples` は多チャンネルならインターリーブ済み。
pub fn write_wav(path: &Path, channels: u16, samples: impl Iterator<Item = f32>) -> Result<()> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for v in samples {
        w.write_sample(f32_to_i16(v))?;
    }
    w.finalize()?;
    Ok(())
}

pub fn write_stereo(path: &Path, buf: &Stereo) -> Result<()> {
    write_wav(path, 2, (0..buf.len()).flat_map(|i| [buf.l[i], buf.r[i]]))
}

pub fn write_mono(path: &Path, samples: &[f32]) -> Result<()> {
    write_wav(path, 1, samples.iter().copied())
}
