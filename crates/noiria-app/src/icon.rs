//! ウィンドウ/タスクバー用アイコン（実行時）。ゲーム側 `icon.png`（`GameDir` 経由、
//! `main.rs` が `read_bytes("icon.png")` して渡す）があれば優先し、無ければここに
//! 埋め込んだエンジン既定アイコンを使う。Windows (`win32.rs`) の `WM_SETICON` と
//! Linux (`x11/window.rs`) の `_NET_WM_ICON` の両方から共通で参照する。
//!
//! exe ファイル自体のアイコン（Windows のリソース）は別経路（`build.rs`、
//! `assets/icon.ico`）で扱う。こちらはあくまで実行時のウィンドウ/タスクバー
//! アイコンで、ゲームごとに差し替え可能。

/// エンジン既定アイコン（`crates/noiria-app/assets/icon.png`、256x256）。
/// `crates/noiria-tools` の `gen-icon` バイナリで生成する
/// （`cargo run -p noiria-tools --bin gen-icon -- engine`）。
const DEFAULT_ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// 1 サイズぶんの RGBA（straight alpha, `w*h*4` バイト）アイコン画像。
pub(crate) struct IconImage {
    pub(crate) w: u32,
    pub(crate) h: u32,
    pub(crate) rgba: Vec<u8>,
}

/// ウィンドウ/タスクバーアイコンの元画像（デコード済み・リサイズ前）。
pub(crate) struct AppIcon {
    src_w: u32,
    src_h: u32,
    src_rgba: Vec<u8>,
}

impl AppIcon {
    /// `icon_png`: ゲーム側 `icon.png` の生バイト列（`GameDir::read_bytes("icon.png").ok()`）。
    /// `None`、またはデコードに失敗した場合はエンジン既定アイコンへフォールバックする
    /// （アイコンの都合で起動そのものを失敗させない）。
    ///
    /// PNG マジックバイトだけでなく WebP（RIFF/WEBP）も見る: `noiria-tools::pack::pack_dir`
    /// は `.png` 拡張子のエントリを（元より小さければ）中身だけ WebP へ差し替えるため
    /// （W8）、pak/export 経由で読んだ `icon.png` の中身が WebP になっている場合がある
    /// （`noiria_core::assets::decode_image_bytes` と同じマジックバイト判定方針）。
    pub(crate) fn from_optional_png(icon_png: Option<&[u8]>) -> Self {
        if let Some(bytes) = icon_png {
            match decode_icon_bytes(bytes) {
                Ok((w, h, rgba)) => {
                    return Self {
                        src_w: w,
                        src_h: h,
                        src_rgba: rgba,
                    }
                }
                Err(e) => {
                    log::warn!("icon.png のデコードに失敗しました。既定アイコンを使います: {e}")
                }
            }
        }
        let (w, h, rgba) = decode_icon_bytes(DEFAULT_ICON_PNG).expect(
            "埋め込みエンジン既定アイコンのデコードに失敗しました（gen-icon で再生成してください）",
        );
        Self {
            src_w: w,
            src_h: h,
            src_rgba: rgba,
        }
    }

    /// 指定サイズへ最近傍リサイズした RGBA を返す（`Pixmap` にリサイズが無いため自前実装。
    /// アイコン程度の小さいサイズ・起動時 1 回だけの用途なので最近傍で十分）。
    pub(crate) fn image(&self, size: u32) -> IconImage {
        let rgba = nearest_resize(self.src_w, self.src_h, &self.src_rgba, size, size);
        IconImage {
            w: size,
            h: size,
            rgba,
        }
    }
}

/// PNG または WebP（マジックバイト判定）をデコードする。
fn decode_icon_bytes(bytes: &[u8]) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return noiria_core::codec::webp::decode_rgba(bytes);
    }
    let png = noiria_core::codec::png::decode(bytes)?;
    Ok((png.width, png.height, png.rgba))
}

fn nearest_resize(src_w: u32, src_h: u32, src: &[u8], dst_w: u32, dst_h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dst_w * dst_h * 4) as usize];
    for y in 0..dst_h {
        let sy = (y * src_h) / dst_h.max(1);
        for x in 0..dst_w {
            let sx = (x * src_w) / dst_w.max(1);
            let si = ((sy * src_w + sx) * 4) as usize;
            let di = ((y * dst_w + x) * 4) as usize;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    out
}

/// Win32 `CreateIconFromResourceEx` にそのまま渡せる ICONIMAGE 形式
/// （BITMAPINFOHEADER 40B + XOR 32bpp BGRA bottom-up + AND マスク 1bpp bottom-up、
/// AND マスクは全 0 = アルファチャンネルのみで透過を表現。`.ico`/`.res` 内の
/// アイコン実体と同一フォーマット）。`crates/noiria-tools/src/bin/gen_icon.rs` にも
/// 同じロジックの複製がある（gen-icon はオフライン生成用の別クレートで、
/// クレート境界をまたいで共有する価値より複製の単純さを優先した。
/// `docs/implementation-notes.md` 2026-07-13 参照）。
#[cfg(windows)]
pub(crate) fn build_dib_icon_image(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(40 + (w * h * 4) as usize);
    out.extend_from_slice(&40u32.to_le_bytes()); // biSize
    out.extend_from_slice(&(w as i32).to_le_bytes()); // biWidth
    out.extend_from_slice(&((h * 2) as i32).to_le_bytes()); // biHeight = XOR+AND合計
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    out.extend_from_slice(&0u32.to_le_bytes()); // biSizeImage
    out.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    out.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant
    for y in (0..h).rev() {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let (r, g, b, a) = (rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]);
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    let row_bytes = (w as usize).div_ceil(32) * 4;
    let and_row = vec![0u8; row_bytes];
    for _ in 0..h {
        out.extend_from_slice(&and_row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_optional_png_falls_back_to_embedded_default_on_none() {
        let icon = AppIcon::from_optional_png(None);
        assert!(icon.src_w > 0 && icon.src_h > 0);
        let img = icon.image(32);
        assert_eq!(img.w, 32);
        assert_eq!(img.h, 32);
        assert_eq!(img.rgba.len(), 32 * 32 * 4);
    }

    /// `pack_dir` が `icon.png` の中身を WebP へ差し替えた場合でも（拡張子は `.png` の
    /// ままでも）マジックバイトで判別してデコードできることの回帰テスト。
    #[test]
    fn from_optional_png_sniffs_webp_by_magic() {
        let rgba = vec![9u8, 8, 7, 255, 1, 2, 3, 128, 4, 5, 6, 0, 200, 201, 202, 255];
        let webp = noiria_core::codec::webp::encode_lossless(2, 2, &rgba).unwrap();
        let icon = AppIcon::from_optional_png(Some(&webp));
        assert_eq!((icon.src_w, icon.src_h), (2, 2));
        assert_eq!(icon.src_rgba, rgba);
    }

    #[test]
    fn from_optional_png_falls_back_on_garbage_bytes() {
        let icon = AppIcon::from_optional_png(Some(b"not a png"));
        let img = icon.image(16);
        assert_eq!(img.rgba.len(), 16 * 16 * 4);
    }

    #[test]
    fn nearest_resize_preserves_corner_pixels() {
        // 2x2 の四隅色付き画像を 4x4 へ拡大し、四隅が保たれることを確認する。
        let src = [
            255, 0, 0, 255, /* */ 0, 255, 0, 255, //
            0, 0, 255, 255, /* */ 255, 255, 0, 255,
        ];
        let out = nearest_resize(2, 2, &src, 4, 4);
        assert_eq!(&out[0..4], &[255, 0, 0, 255]); // 左上
        assert_eq!(
            &out[(3 * 4) as usize..(3 * 4 + 4) as usize],
            &[0, 255, 0, 255]
        ); // 右上
    }

    #[cfg(windows)]
    #[test]
    fn build_dib_icon_image_has_expected_header_and_length() {
        let rgba = vec![
            10u8, 20, 30, 255, 40, 50, 60, 128, 70, 80, 90, 0, 100, 110, 120, 255,
        ];
        let dib = build_dib_icon_image(2, 2, &rgba);
        // BITMAPINFOHEADER(40) + XOR(2*2*4=16) + ANDマスク(高さ2行 * 4バイト境界パディング済み1行分)
        let and_row_bytes = 4usize; // (2+31)/32*4 = 4
        assert_eq!(dib.len(), 40 + 16 + and_row_bytes * 2);
        assert_eq!(u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]), 40); // biSize
        assert_eq!(i32::from_le_bytes([dib[4], dib[5], dib[6], dib[7]]), 2); // biWidth
        assert_eq!(i32::from_le_bytes([dib[8], dib[9], dib[10], dib[11]]), 4); // biHeight = 2*2
                                                                               // XOR 先頭ピクセルは bottom-up の最終行 (70,80,90,0) が BGRA で入る。
        assert_eq!(&dib[40..44], &[90, 80, 70, 0]);
    }
}
