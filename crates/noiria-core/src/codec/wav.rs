//! WAV (RIFF/WAVE) デコーダ（自作）。PCM 8bit/16bit/24bit と IEEE float 32bit に対応。
//! 出力は f32 インターリーブ（`mixer::BufferSource` にそのまま渡せる形）。

use anyhow::{bail, ensure, Context, Result};

pub struct DecodedWav {
    pub channels: u16,
    pub sample_rate: u32,
    /// インターリーブ済み [-1.0, 1.0]
    pub samples: Vec<f32>,
}

fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

pub fn decode(data: &[u8]) -> Result<DecodedWav> {
    ensure!(data.len() >= 12, "WAV ファイルが短すぎます");
    ensure!(&data[0..4] == b"RIFF", "RIFF ヘッダがありません");
    ensure!(&data[8..12] == b"WAVE", "WAVE 形式ではありません");

    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // (format, channels, rate, bits)
    let mut body: Option<&[u8]> = None;

    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let len = u32le(data, pos + 4) as usize;
        let start = pos + 8;
        ensure!(
            start + len <= data.len(),
            "チャンクがファイル末尾を超えています"
        );
        let chunk = &data[start..start + len];
        // チャンクは 2 バイト境界にパディングされる
        pos = start + len + (len & 1);

        match id {
            b"fmt " => {
                ensure!(len >= 16, "fmt チャンクが短すぎます");
                fmt = Some((
                    u16le(chunk, 0),
                    u16le(chunk, 2),
                    u32le(chunk, 4),
                    u16le(chunk, 14),
                ));
            }
            b"data" => {
                body = Some(chunk);
                // data 以降は読まなくてよい（fmt は data より先にあるのが通例）
                if fmt.is_some() {
                    break;
                }
            }
            _ => {}
        }
    }

    let (format, channels, sample_rate, bits) = fmt.context("fmt チャンクがありません")?;
    let body = body.context("data チャンクがありません")?;
    ensure!(
        (1..=8).contains(&channels),
        "チャンネル数が不正: {channels}"
    );
    ensure!(sample_rate > 0, "サンプルレートが不正");

    let samples: Vec<f32> = match (format, bits) {
        // PCM
        (1, 8) => body.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
        (1, 16) => body
            .as_chunks::<2>().0.iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect(),
        (1, 24) => body
            .as_chunks::<3>().0.iter()
            .map(|c| {
                let v = i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8;
                v as f32 / 8_388_608.0
            })
            .collect(),
        // IEEE float
        (3, 32) => body
            .as_chunks::<4>().0.iter()
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        (f, b) => bail!("非対応の WAV 形式: format={f} bits={b}"),
    };

    Ok(DecodedWav {
        channels,
        sample_rate,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小の WAV バイト列を手組みする。
    fn build_wav(channels: u16, rate: u32, bits: u16, body: &[u8]) -> Vec<u8> {
        let block_align = channels * bits / 8;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + body.len() as u32).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
        v.extend_from_slice(&block_align.to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn pcm16_known_samples() {
        // 0, +max, -max, half
        let mut body = Vec::new();
        for s in [0i16, 32767, -32768, 16384] {
            body.extend_from_slice(&s.to_le_bytes());
        }
        let wav = build_wav(1, 44100, 16, &body);
        let d = decode(&wav).unwrap();
        assert_eq!(d.channels, 1);
        assert_eq!(d.sample_rate, 44100);
        assert_eq!(d.samples.len(), 4);
        assert!((d.samples[0]).abs() < 1e-6);
        assert!((d.samples[1] - 0.99997).abs() < 1e-3);
        assert!((d.samples[2] + 1.0).abs() < 1e-6);
        assert!((d.samples[3] - 0.5).abs() < 1e-3);
    }

    #[test]
    fn pcm8_stereo() {
        let body = [128u8, 128, 255, 0]; // (0,0), (+max, -max)
        let wav = build_wav(2, 22050, 8, &body);
        let d = decode(&wav).unwrap();
        assert_eq!(d.channels, 2);
        assert_eq!(d.samples.len(), 4);
        assert!(d.samples[0].abs() < 1e-6);
        assert!(d.samples[2] > 0.98);
        assert!(d.samples[3] + 1.0 < 1e-6);
    }

    #[test]
    fn decodes_generated_game_assets() {
        // gen-assets 製の実 WAV (hound 16bit/44.1kHz) が読めること
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/sample/assets");
        if !root.exists() {
            return;
        }
        // bgm/main.wav は手続き生成シンセ製のステレオ BGM（32〜48秒級のシームレス
        // ループ）、se/click.wav・voice/alice_001.wav は従来通りモノラル。
        for (rel, channels, min_secs) in [
            ("bgm/main.wav", 2, 32.0),
            ("se/click.wav", 1, 0.05),
            ("voice/alice_001.wav", 1, 1.0),
        ] {
            let bytes = std::fs::read(root.join(rel)).unwrap();
            let d = decode(&bytes).unwrap();
            assert_eq!(d.sample_rate, 44100, "{rel}");
            assert_eq!(d.channels, channels, "{rel}");
            let secs = d.samples.len() as f64 / (44100.0 * channels as f64);
            assert!(secs >= min_secs, "{rel} が短すぎる: {secs}s");
            assert!(d.samples.iter().any(|&s| s.abs() > 0.01), "{rel} が無音");
            assert!(
                d.samples.iter().all(|&s| (-1.0..=1.0).contains(&s)),
                "{rel} がクリップ"
            );
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"not a wav").is_err());
        assert!(decode(b"RIFF\x00\x00\x00\x00MP3 ").is_err());
    }
    #[test]
    fn fuzz_corrupted_wav_never_panics_or_overallocates() {
        let body16: Vec<u8> = (0..200u32)
            .flat_map(|i| ((i * 331) as i16).to_le_bytes())
            .collect();
        let body24: Vec<u8> = (0..150u32)
            .flat_map(|i| [i as u8, (i * 3) as u8, (i * 5) as u8])
            .collect();
        let bases = [
            build_wav(2, 44100, 16, &body16),
            build_wav(1, 22050, 8, &body16[..100]),
            build_wav(2, 48000, 24, &body24),
        ];
        for (i, base) in bases.iter().enumerate() {
            assert!(decode(base).is_ok(), "基準入力 {i} がデコードできる");
            crate::test_support::fuzz("wav", 0x4444 + i as u64, 1500, base, |d| {
                let _ = decode(d);
            });
        }
    }
}
