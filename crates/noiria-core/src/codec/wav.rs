//! WAV (RIFF/WAVE) デコーダ（自作）。PCM 8bit/16bit/24bit と IEEE float 32bit に対応。
//! 出力は f32 インターリーブ。全量デコード (`decode`) とヘッダ解析＋逐次変換 (`parse_header` +
//! `WavSampleFormat::decode_into`) の両方を提供する。

use std::io::{Read, Seek, SeekFrom};

use anyhow::{bail, ensure, Context, Result};

pub struct DecodedWav {
    pub channels: u16,
    pub sample_rate: u32,
    /// インターリーブ済み [-1.0, 1.0]
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavSampleFormat {
    Pcm8,
    Pcm16,
    Pcm24,
    Float32,
}

impl WavSampleFormat {
    pub fn from_fmt(format: u16, bits: u16) -> Result<Self> {
        match (format, bits) {
            (1, 8) => Ok(Self::Pcm8),
            (1, 16) => Ok(Self::Pcm16),
            (1, 24) => Ok(Self::Pcm24),
            (3, 32) => Ok(Self::Float32),
            (f, b) => bail!("非対応の WAV 形式: format={f} bits={b}"),
        }
    }

    pub fn bytes_per_sample(self) -> usize {
        match self {
            Self::Pcm8 => 1,
            Self::Pcm16 => 2,
            Self::Pcm24 => 3,
            Self::Float32 => 4,
        }
    }

    /// `bytes` から最大 `out.len()` サンプルを f32 へ変換して `out` に書き、変換したサンプル数を返す。
    pub fn decode_into(self, bytes: &[u8], out: &mut [f32]) -> usize {
        match self {
            Self::Pcm8 => {
                let n = bytes.len().min(out.len());
                for (dst, &b) in out[..n].iter_mut().zip(&bytes[..n]) {
                    *dst = (b as f32 - 128.0) / 128.0;
                }
                n
            }
            Self::Pcm16 => {
                let chunks = bytes.as_chunks::<2>().0;
                let n = chunks.len().min(out.len());
                for (dst, c) in out[..n].iter_mut().zip(&chunks[..n]) {
                    *dst = i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0;
                }
                n
            }
            Self::Pcm24 => {
                let chunks = bytes.as_chunks::<3>().0;
                let n = chunks.len().min(out.len());
                for (dst, c) in out[..n].iter_mut().zip(&chunks[..n]) {
                    let v = i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8;
                    *dst = v as f32 / 8_388_608.0;
                }
                n
            }
            Self::Float32 => {
                let chunks = bytes.as_chunks::<4>().0;
                let n = chunks.len().min(out.len());
                for (dst, c) in out[..n].iter_mut().zip(&chunks[..n]) {
                    *dst = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                }
                n
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavHeader {
    pub format: WavSampleFormat,
    pub channels: u16,
    pub sample_rate: u32,
    pub data_offset: u64,
    pub data_len: u64,
}

fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// シーク可能なストリームから WAV ヘッダと `data` チャンクの位置を解析し、`data_offset` にシークする。
pub fn parse_header<R: Read + Seek>(reader: &mut R) -> Result<WavHeader> {
    let file_len = reader.seek(SeekFrom::End(0))?;
    ensure!(file_len >= 12, "WAV ファイルが短すぎます");
    reader.seek(SeekFrom::Start(0))?;
    let mut riff = [0u8; 12];
    reader.read_exact(&mut riff)?;
    ensure!(&riff[0..4] == b"RIFF", "RIFF ヘッダがありません");
    ensure!(&riff[8..12] == b"WAVE", "WAVE 形式ではありません");

    let mut pos: u64 = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data_range: Option<(u64, u64)> = None;

    while pos.checked_add(8).is_some_and(|end| end <= file_len) {
        reader.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        reader.read_exact(&mut hdr)?;
        let id = &hdr[0..4];
        let len = u32le(&hdr, 4) as u64;
        let start = pos + 8;
        let end = start
            .checked_add(len)
            .filter(|&e| e <= file_len)
            .context("チャンクがファイル末尾を超えています")?;

        match id {
            b"fmt " => {
                ensure!(len >= 16, "fmt チャンクが短すぎます");
                let mut fmt_buf = [0u8; 16];
                reader.read_exact(&mut fmt_buf)?;
                fmt = Some((
                    u16le(&fmt_buf, 0),
                    u16le(&fmt_buf, 2),
                    u32le(&fmt_buf, 4),
                    u16le(&fmt_buf, 14),
                ));
            }
            b"data" => {
                data_range = Some((start, len));
                if fmt.is_some() {
                    break;
                }
            }
            _ => {}
        }

        // チャンクは 2 バイト境界にパディングされる。
        let Some(next) = end.checked_add(len & 1) else {
            break;
        };
        pos = next;
    }

    let (format_tag, channels, sample_rate, bits) = fmt.context("fmt チャンクがありません")?;
    let (data_offset, data_len) = data_range.context("data チャンクがありません")?;
    ensure!(
        (1..=8).contains(&channels),
        "チャンネル数が不正: {channels}"
    );
    ensure!(sample_rate > 0, "サンプルレートが不正");
    let format = WavSampleFormat::from_fmt(format_tag, bits)?;
    reader.seek(SeekFrom::Start(data_offset))?;

    Ok(WavHeader {
        format,
        channels,
        sample_rate,
        data_offset,
        data_len,
    })
}

pub fn decode(data: &[u8]) -> Result<DecodedWav> {
    let mut cursor = std::io::Cursor::new(data);
    let hdr = parse_header(&mut cursor)?;
    let start = hdr.data_offset as usize;
    let end = start + hdr.data_len as usize;
    let body = &data[start..end];
    let sample_count = body.len() / hdr.format.bytes_per_sample();
    let mut samples = vec![0.0f32; sample_count];
    hdr.format.decode_into(body, &mut samples);

    Ok(DecodedWav {
        channels: hdr.channels,
        sample_rate: hdr.sample_rate,
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
