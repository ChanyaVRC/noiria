//! 自作コーデック群。外部非依存化ロードマップの実装置き場。
//! - inflate: zlib/DEFLATE 展開 (RFC1950/1951)
//! - deflate: zlib/DEFLATE 圧縮 (RFC1950/1951)。inflate の対
//! - png: PNG デコーダ/エンコーダ (8bit 非インターレース)
//! - wav: WAV デコーダ (PCM 8/16/24bit, float32)
//! - ogg: Ogg コンテナデコーダ (RFC 3533)
//! - vorbis: Vorbis I 音声デコーダ（クリーンルーム実装。Xiph の仕様書のみ参照）
//! - video: 動画再生の共通インターフェイスと AVI (MJPEG) / IVF (VP9, `vp9dec` 別リポジトリ) 実装
//! - mf_video: Windows のみ。Media Foundation (OS 内蔵デコーダ) 経由の
//!   H.264/MP4 等の再生・(テスト用) エンコード
//! - webp: WebP (VP8L ロスレス) デコーダ/エンコーダ。外部クレート `image-webp`
//!   （純 Rust、1 モジュール隔離、将来自作置換候補）を包む
//! - flac_subset: FLAC 風サブセットコーデックの**デコード側**（pak codec=2 "wav-flac"）。
//!   エンコード側は `noiria-tools/src/flac_enc.rs` に意図的に別クレートで独立実装している

pub mod deflate;
pub mod flac_subset;
pub mod inflate;
pub mod jpeg;
#[cfg(windows)]
pub mod mf_video;
pub mod ogg;
pub mod png;
pub mod video;
pub mod vorbis;
pub mod wav;
pub mod webp;
