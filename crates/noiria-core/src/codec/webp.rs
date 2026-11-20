//! WebP (VP8L ロスレス) の 1 モジュール隔離ラッパー。
//!
//! 外部クレート `image-webp`（image-rs 公式・純 Rust・`#![forbid(unsafe_code)]`、
//! C 依存なし）をここでのみ使う。公開 API は [`decode_rgba`]/[`encode_lossless`] の
//! 2 関数だけで、`image_webp` の型は一切外へ漏らさない（将来自作 VP8L 実装へ
//! 差し替える際にこのファイルだけを書き換えれば済むようにするための安定 ABI。
//! `docs/implementation-notes.md` 2026-07-12 配布対応 W8 参照）。
//!
//! - デコード: ロスレス (VP8L) は完全対応。ロッシー (VP8) も `image-webp` の
//!   対応範囲内でデコードできる（アニメーションは先頭フレームのみ、ICC/EXIF 等の
//!   メタデータは読み飛ばす）。pack/export は PNG→WebP のロスレス変換にしか
//!   使わないため、ロッシー入力を作ることはこのエンジン自身は行わない。
//! - エンコード: `image-webp` の `WebPEncoder` はロスレス (VP8L) のみ対応
//!   （ロッシー VP8 エンコードには非対応）。

use std::io::Cursor;

use anyhow::{ensure, Context, Result};
use image_webp::{ColorType, WebPDecoder, WebPEncoder};

/// デコードを受け付ける最大画素数（8192x8192）。ヘッダの申告する寸法だけで巨大な確保をさせない。
const MAX_PIXELS: u64 = 1 << 26;

/// WebP バイト列を straight RGBA8 にデコードする。`(width, height, rgba)` を返す。
pub fn decode_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let mut decoder =
        WebPDecoder::new(Cursor::new(bytes)).context("WebP ヘッダの解析に失敗しました")?;
    let (width, height) = decoder.dimensions();
    ensure!(
        width as u64 * height as u64 <= MAX_PIXELS,
        "WebP 画像が大きすぎます: {width}x{height}"
    );
    let has_alpha = decoder.has_alpha();
    let buf_len = decoder
        .output_buffer_size()
        .context("WebP 画像サイズが不正です")?;
    let mut buf = vec![0u8; buf_len];
    decoder
        .read_image(&mut buf)
        .context("WebP のデコードに失敗しました")?;

    let rgba = if has_alpha {
        buf
    } else {
        // has_alpha() が false のとき read_image は RGB8 (3 バイト/px) を返すため、
        // 不透明 (alpha=255) の RGBA8 へ詰め直す。
        let mut out = Vec::with_capacity(width as usize * height as usize * 4);
        for px in buf.as_chunks::<3>().0 {
            out.extend_from_slice(px);
            out.push(255);
        }
        out
    };
    Ok((width, height, rgba))
}

/// straight RGBA8 を WebP ロスレス (VP8L) にエンコードする。
pub fn encode_lossless(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        rgba.len() == width as usize * height as usize * 4,
        "rgba のサイズが width*height*4 と一致しません（{}x{} に対し {} バイト）",
        width,
        height,
        rgba.len()
    );
    let mut out = Vec::new();
    WebPEncoder::new(&mut out)
        .encode(rgba, width, height, ColorType::Rgba8)
        .context("WebP エンコードに失敗しました")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkerboard(w: u32, h: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
        for y in 0..h {
            for x in 0..w {
                let on = (x / 4 + y / 4) % 2 == 0;
                if on {
                    rgba.extend_from_slice(&[255, 0, 0, 255]);
                } else {
                    rgba.extend_from_slice(&[0, 255, 0, 128]);
                }
            }
        }
        rgba
    }

    #[test]
    fn round_trip_checkerboard_with_alpha() {
        let (w, h) = (37, 29); // 4 の倍数でないサイズも確認
        let rgba = checkerboard(w, h);
        let webp = encode_lossless(w, h, &rgba).unwrap();
        assert!(webp.starts_with(b"RIFF"));
        assert_eq!(&webp[8..12], b"WEBP");
        let (dw, dh, decoded) = decode_rgba(&webp).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert_eq!(
            decoded, rgba,
            "ロスレス round-trip はピクセル完全一致のはず"
        );
    }

    #[test]
    fn round_trip_opaque_1x1() {
        let rgba = vec![10u8, 20, 30, 255];
        let webp = encode_lossless(1, 1, &rgba).unwrap();
        let (w, h, decoded) = decode_rgba(&webp).unwrap();
        assert_eq!((w, h), (1, 1));
        assert_eq!(decoded, rgba);
    }

    #[test]
    fn round_trip_gradient() {
        let (w, h) = (64, 48);
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                rgba.push((x * 255 / w) as u8);
                rgba.push((y * 255 / h) as u8);
                rgba.push(((x + y) * 255 / (w + h)) as u8);
                rgba.push(255);
            }
        }
        let webp = encode_lossless(w as u32, h as u32, &rgba).unwrap();
        let (dw, dh, decoded) = decode_rgba(&webp).unwrap();
        assert_eq!((dw, dh), (w as u32, h as u32));
        assert_eq!(decoded, rgba);
    }

    #[test]
    fn encode_rejects_mismatched_buffer_len() {
        assert!(encode_lossless(2, 2, &[0u8; 4]).is_err());
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode_rgba(b"not a webp file").is_err());
    }

    #[test]
    fn decode_lossy_vp8_fixture_to_rgba() {
        let webp = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/lossy_4quad.webp"
        ));
        assert_eq!(&webp[12..16], b"VP8 ");

        let (w, h, decoded) = decode_rgba(webp).unwrap();
        assert_eq!((w, h), (16, 16));
        assert_eq!(decoded.len(), 16 * 16 * 4);

        let centers = [
            (4usize, 4usize, [255u8, 0, 0]),
            (11, 4, [0, 255, 0]),
            (4, 11, [0, 0, 255]),
            (11, 11, [255, 255, 255]),
        ];
        for (x, y, expected) in centers {
            let offset = (y * 16 + x) * 4;
            let actual = &decoded[offset..offset + 4];
            for channel in 0..3 {
                assert!(
                    actual[channel].abs_diff(expected[channel]) <= 8,
                    "({x}, {y}) channel {channel}: actual={} expected={}",
                    actual[channel],
                    expected[channel]
                );
            }
            assert_eq!(actual[3], 255);
        }
    }

    /// W8 クロスチェック: `image` クレート（dev-dep、`webp` feature 有効。内部で同じ
    /// `image_webp` を使ってはいるが、デコーダ設定/呼び出しコードが別経路のため
    /// 統合バグの検出には有効）と相互デコードが一致することを確認する。
    #[test]
    fn cross_check_our_encode_decodes_with_image_crate() {
        let (w, h) = (33, 21);
        let rgba = checkerboard(w, h);
        let webp = encode_lossless(w, h, &rgba).unwrap();

        let img = image::load_from_memory(&webp).expect("image クレートでのデコードに失敗");
        let decoded = img.into_rgba8();
        assert_eq!(decoded.width(), w);
        assert_eq!(decoded.height(), h);
        assert_eq!(
            decoded.into_raw(),
            rgba,
            "自作エンコード結果を image クレートでデコードしても一致するはず"
        );
    }

    /// W8: sample の全 PNG (10 ファイル) を自作 PNG デコーダで読み、
    /// `encode_lossless` → `decode_rgba` の round-trip がピクセル完全一致すること。
    #[test]
    fn round_trip_all_sample_pngs() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/sample/assets");
        if !root.exists() {
            return; // アセット未生成の環境ではスキップ
        }
        let rels = [
            "bg/room.png",
            "bg/street.png",
            "fg/alice_normal.png",
            "fg/alice_smile.png",
            "fg/btn_menu.png",
            "fg/icon_blink_a.png",
            "fg/icon_blink_b.png",
            "fg/icon_spark.png",
            "rule/wipe.png",
            "rule/circle.png",
        ];
        for rel in rels {
            let path = root.join(rel);
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{rel} が読めません: {e}"));
            let png = crate::codec::png::decode(&bytes)
                .unwrap_or_else(|e| panic!("{rel} のPNGデコードに失敗: {e}"));
            let webp = encode_lossless(png.width, png.height, &png.rgba)
                .unwrap_or_else(|e| panic!("{rel} のWebPエンコードに失敗: {e}"));
            let (w, h, decoded) =
                decode_rgba(&webp).unwrap_or_else(|e| panic!("{rel} のWebPデコードに失敗: {e}"));
            assert_eq!((w, h), (png.width, png.height), "{rel} のサイズ不一致");
            assert_eq!(decoded, png.rgba, "{rel} の round-trip でピクセル不一致");
        }
    }

    #[test]
    fn cross_check_image_crate_encode_decodes_with_us() {
        let (w, h) = (33, 21);
        let rgba = checkerboard(w, h);

        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .encode(&rgba, w, h, image::ExtendedColorType::Rgba8)
            .expect("image クレートでのエンコードに失敗");

        let (dw, dh, decoded) = decode_rgba(&webp).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert_eq!(
            decoded, rgba,
            "image クレートのエンコード結果を自作ラッパーでデコードしても一致するはず"
        );
    }
}
