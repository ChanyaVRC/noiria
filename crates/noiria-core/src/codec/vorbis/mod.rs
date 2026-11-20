//! Vorbis I 音声デコーダ（クリーンルーム実装）。
//! 参照したのは Xiph の Vorbis I 仕様書
//! (<https://xiph.org/vorbis/doc/Vorbis_I_spec.html>) と Ogg 仕様 (RFC 3533) のみ。
//! libvorbis/lewton 等の既存デコーダ実装は一切参照していない。
//!
//! モジュール構成:
//! - `bitreader`: Vorbis のビットパッキング規約 (LSB ファースト) 用ビットリーダ
//! - `codebook`: コードブック (VQ) デコード。カノニカルハフマン + lookup type 0/1/2
//! - `floor`: Floor type 1 (setup + パケットデコード + 曲線合成)
//! - `residue`: Residue type 0/1/2 デコード
//! - `mdct`: 逆 MDCT + Vorbis 窓関数 + オーバーラップ加算

mod bitreader;
mod codebook;
mod floor;
mod mdct;
mod residue;

mod decoder;

pub use decoder::{decode, decode_packets, DecodedVorbis, VorbisStreamDecoder};
