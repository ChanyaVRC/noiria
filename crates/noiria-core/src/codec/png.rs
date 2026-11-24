//! PNG デコーダ（自作）。8bit / 非インターレースの
//! グレースケール・RGB・パレット・グレースケール+α・RGBA に対応。
//! 出力は straight RGBA8。

use anyhow::{bail, ensure, Context, Result};

use super::deflate::zlib_compress;
use super::inflate::zlib_decompress_limited_with_capacity;

/// デコードを受け付ける最大画素数（8192x8192。RGBA で 256MB）。IHDR の 32bit 幅・高さが申告する
/// だけで巨大な確保をさせない。
const MAX_PIXELS: usize = 1 << 26;

/// IDAT の展開後サイズが IHDR から求めた期待値を超えてよい余白（末尾の余分なバイトを許す）。
const IDAT_SLACK: usize = 64 * 1024;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

pub struct DecodedPng {
    pub width: u32,
    pub height: u32,
    /// straight RGBA8
    pub rgba: Vec<u8>,
}

struct Ihdr {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace: u8,
}

pub fn decode(data: &[u8]) -> Result<DecodedPng> {
    ensure!(
        data.len() > 8 && data[..8] == SIGNATURE,
        "PNG シグネチャがありません"
    );

    let mut pos = 8usize;
    let mut ihdr: Option<Ihdr> = None;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut first_idat: Option<&[u8]> = None;
    let mut idat_buf: Vec<u8> = Vec::new();

    while pos + 8 <= data.len() {
        let len =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let ctype = &data[pos + 4..pos + 8];
        let body_start = pos + 8;
        ensure!(
            body_start + len + 4 <= data.len(),
            "チャンクがファイル末尾を超えています"
        );
        let body = &data[body_start..body_start + len];
        // CRC は検証しない（4 バイト読み飛ばし）
        pos = body_start + len + 4;

        match ctype {
            b"IHDR" => {
                ensure!(len == 13, "IHDR 長が不正");
                let h = Ihdr {
                    width: u32::from_be_bytes([body[0], body[1], body[2], body[3]]),
                    height: u32::from_be_bytes([body[4], body[5], body[6], body[7]]),
                    bit_depth: body[8],
                    color_type: body[9],
                    interlace: body[12],
                };
                ensure!(body[10] == 0, "非対応の圧縮方式");
                ensure!(body[11] == 0, "非対応のフィルタ方式");
                ihdr = Some(h);
            }
            b"PLTE" => {
                ensure!(len.is_multiple_of(3), "PLTE 長が不正");
                palette = body
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|c| [c[0], c[1], c[2]])
                    .collect();
            }
            b"tRNS" => trns = body.to_vec(),
            b"IDAT" => {
                if !idat_buf.is_empty() {
                    idat_buf.extend_from_slice(body);
                } else if let Some(prev_chunk) = first_idat {
                    idat_buf.reserve(prev_chunk.len() + body.len());
                    idat_buf.extend_from_slice(prev_chunk);
                    idat_buf.extend_from_slice(body);
                } else {
                    first_idat = Some(body);
                }
            }
            b"IEND" => break,
            _ => {} // 補助チャンクは無視
        }
    }

    let idat: &[u8] = if !idat_buf.is_empty() {
        &idat_buf
    } else {
        first_idat.unwrap_or(&[])
    };

    let ihdr = ihdr.context("IHDR がありません")?;
    ensure!(ihdr.interlace == 0, "インターレース PNG は非対応です");
    ensure!(
        ihdr.bit_depth == 8,
        "ビット深度 {} は非対応です (8 のみ)",
        ihdr.bit_depth
    );
    ensure!(ihdr.width > 0 && ihdr.height > 0, "画像サイズが不正");

    // チャンネル数
    let channels: usize = match ihdr.color_type {
        0 => 1, // grayscale
        2 => 3, // RGB
        3 => 1, // palette index
        4 => 2, // gray + alpha
        6 => 4, // RGBA
        t => bail!("非対応のカラータイプ: {t}"),
    };

    let w = ihdr.width as usize;
    let h = ihdr.height as usize;
    ensure!(
        w.checked_mul(h).is_some_and(|px| px <= MAX_PIXELS),
        "画像が大きすぎます: {w}x{h}"
    );
    let stride = w * channels;
    let expected = h * (stride + 1);
    let mut raw =
        zlib_decompress_limited_with_capacity(idat, expected.saturating_add(IDAT_SLACK), expected)
            .context("IDAT の展開に失敗")?;
    drop(idat_buf);
    ensure!(
        raw.len() >= expected,
        "画像データが不足しています (期待 {} 実際 {})",
        expected,
        raw.len()
    );

    if ihdr.color_type == 6 {
        // RGBA8: row y の出力先 [y*stride..] は入力 [y*(stride+1)+1..] より常に前方なので in-place でフィルタ解除できる。
        let zero_row = vec![0u8; stride];
        for y in 0..h {
            let src_start = y * (stride + 1);
            let filter = raw[src_start];
            let dst_start = y * stride;
            let shift = (src_start + 1) - dst_start;
            if y == 0 {
                unfilter_row_in_place(filter, &mut raw, shift, &zero_row, channels, stride)?;
            } else {
                let (before, rest) = raw.split_at_mut(dst_start);
                let prev = &before[(y - 1) * stride..dst_start];
                unfilter_row_in_place(filter, rest, shift, prev, channels, stride)?;
            }
        }
        raw.truncate(h * stride);
        raw.shrink_to_fit();
        return Ok(DecodedPng {
            width: ihdr.width,
            height: ihdr.height,
            rgba: raw,
        });
    }

    if ihdr.color_type == 3 {
        ensure!(!palette.is_empty(), "パレットがありません");
    }

    // 非 RGBA8: 2 行分の走査線バッファだけでフィルタ解除しつつ直接 RGBA8 へ展開する。
    let mut out = vec![0u8; w * h * 4];
    let mut prev_row = vec![0u8; stride];
    let mut cur_row = vec![0u8; stride];
    for y in 0..h {
        let src = &raw[y * (stride + 1)..(y + 1) * (stride + 1)];
        unfilter_row(src[0], &src[1..], &prev_row, channels, &mut cur_row)?;
        let dst_pixels = &mut out.as_chunks_mut::<4>().0[y * w..(y + 1) * w];
        match ihdr.color_type {
            0 => {
                for (dst, &g) in dst_pixels.iter_mut().zip(&cur_row) {
                    *dst = [g, g, g, 255];
                }
            }
            2 => {
                for (dst, px) in dst_pixels.iter_mut().zip(cur_row.as_chunks::<3>().0) {
                    *dst = [px[0], px[1], px[2], 255];
                }
            }
            3 => {
                for (dst, &idx) in dst_pixels.iter_mut().zip(&cur_row) {
                    let c = palette.get(idx as usize).context("パレット添字が範囲外")?;
                    let a = trns.get(idx as usize).copied().unwrap_or(255);
                    *dst = [c[0], c[1], c[2], a];
                }
            }
            4 => {
                for (dst, px) in dst_pixels.iter_mut().zip(cur_row.as_chunks::<2>().0) {
                    *dst = [px[0], px[0], px[0], px[1]];
                }
            }
            _ => unreachable!(),
        }
        std::mem::swap(&mut prev_row, &mut cur_row);
    }

    Ok(DecodedPng {
        width: ihdr.width,
        height: ihdr.height,
        rgba: out,
    })
}

#[inline(always)]
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let a = a as i16;
    let b = b as i16;
    let c = c as i16;
    let pa = (b - c).abs();
    let pb = (a - c).abs();
    let pc = (a + b - 2 * c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

fn unfilter_row_in_place(
    filter: u8,
    buf: &mut [u8],
    shift: usize,
    prev: &[u8],
    channels: usize,
    stride: usize,
) -> Result<()> {
    if shift >= stride {
        let (dst, tail) = buf.split_at_mut(shift);
        return unfilter_row(filter, &tail[..stride], prev, channels, &mut dst[..stride]);
    }
    let buf = &mut buf[..shift + stride];
    let prev = &prev[..stride];
    match filter {
        0 => buf.copy_within(shift..shift + stride, 0),
        1 => {
            buf.copy_within(shift..shift + channels, 0);
            for i in channels..stride {
                buf[i] = buf[shift + i].wrapping_add(buf[i - channels]);
            }
        }
        2 => {
            for i in 0..stride {
                buf[i] = buf[shift + i].wrapping_add(prev[i]);
            }
        }
        3 => {
            for i in 0..channels {
                buf[i] = buf[shift + i].wrapping_add(prev[i] >> 1);
            }
            for i in channels..stride {
                let avg = ((buf[i - channels] as u16 + prev[i] as u16) >> 1) as u8;
                buf[i] = buf[shift + i].wrapping_add(avg);
            }
        }
        4 => {
            for i in 0..channels {
                buf[i] = buf[shift + i].wrapping_add(prev[i]);
            }
            for i in channels..stride {
                buf[i] = buf[shift + i].wrapping_add(paeth(
                    buf[i - channels],
                    prev[i],
                    prev[i - channels],
                ));
            }
        }
        f => bail!("非対応のフィルタタイプ: {f}"),
    }
    Ok(())
}

fn unfilter_row(
    filter: u8,
    row: &[u8],
    prev: &[u8],
    channels: usize,
    dst: &mut [u8],
) -> Result<()> {
    let stride = row.len();
    match filter {
        0 => dst.copy_from_slice(row),
        1 => {
            dst[..channels].copy_from_slice(&row[..channels]);
            for i in channels..stride {
                dst[i] = row[i].wrapping_add(dst[i - channels]);
            }
        }
        2 => {
            for ((d, &r), &p) in dst.iter_mut().zip(row).zip(prev) {
                *d = r.wrapping_add(p);
            }
        }
        3 => {
            for i in 0..channels {
                dst[i] = row[i].wrapping_add(prev[i] >> 1);
            }
            for i in channels..stride {
                let avg = ((dst[i - channels] as u16 + prev[i] as u16) >> 1) as u8;
                dst[i] = row[i].wrapping_add(avg);
            }
        }
        4 => {
            for i in 0..channels {
                dst[i] = row[i].wrapping_add(prev[i]);
            }
            for i in channels..stride {
                dst[i] = row[i].wrapping_add(paeth(dst[i - channels], prev[i], prev[i - channels]));
            }
        }
        f => bail!("非対応のフィルタタイプ: {f}"),
    }
    Ok(())
}

/// 1 行に 5 種類のフィルタ (None/Sub/Up/Average/Paeth) を適用しつつ MSAD コストも同時に計算する。
fn apply_filter_and_msad(
    filter: u8,
    row: &[u8],
    prev: &[u8],
    channels: usize,
    out: &mut [u8],
) -> u64 {
    #[inline(always)]
    fn cost_byte(b: u8) -> u64 {
        (b as i8).unsigned_abs() as u64
    }
    let stride = row.len();
    let mut sum = 0u64;
    match filter {
        0 => {
            out.copy_from_slice(row);
            for &b in row {
                sum += cost_byte(b);
            }
        }
        1 => {
            for i in 0..channels {
                let v = row[i];
                out[i] = v;
                sum += cost_byte(v);
            }
            for i in channels..stride {
                let v = row[i].wrapping_sub(row[i - channels]);
                out[i] = v;
                sum += cost_byte(v);
            }
        }
        2 => {
            for ((o, &r), &p) in out.iter_mut().zip(row).zip(prev) {
                let v = r.wrapping_sub(p);
                *o = v;
                sum += cost_byte(v);
            }
        }
        3 => {
            for i in 0..channels {
                let v = row[i].wrapping_sub(prev[i] >> 1);
                out[i] = v;
                sum += cost_byte(v);
            }
            for i in channels..stride {
                let avg = ((row[i - channels] as u16 + prev[i] as u16) >> 1) as u8;
                let v = row[i].wrapping_sub(avg);
                out[i] = v;
                sum += cost_byte(v);
            }
        }
        4 => {
            for i in 0..channels {
                let v = row[i].wrapping_sub(prev[i]);
                out[i] = v;
                sum += cost_byte(v);
            }
            for i in channels..stride {
                let v = row[i].wrapping_sub(paeth(row[i - channels], prev[i], prev[i - channels]));
                out[i] = v;
                sum += cost_byte(v);
            }
        }
        _ => unreachable!(),
    }
    sum
}

/// straight RGBA8 から PNG の IDAT 生データ（フィルタタイプバイト + フィルタ後
/// ピクセル列、圧縮前）を作る。行ごとに 5 種類のフィルタを試し MSAD 最小を選ぶ。
fn build_filtered_raw(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    const CHANNELS: usize = 4;
    let stride = width as usize * CHANNELS;
    let mut raw = Vec::with_capacity(height as usize * (stride + 1));
    let zero_row = vec![0u8; stride];
    let mut prev: &[u8] = &zero_row;
    let mut best_buf = vec![0u8; stride];
    let mut trial_buf = vec![0u8; stride];
    for row in rgba.chunks_exact(stride) {
        let mut best = 0u8;
        let mut best_cost = u64::MAX;
        for f in 0u8..5 {
            let cost = apply_filter_and_msad(f, row, prev, CHANNELS, &mut trial_buf);
            if cost < best_cost {
                best_cost = cost;
                best = f;
                std::mem::swap(&mut best_buf, &mut trial_buf);
            }
        }
        raw.push(best);
        raw.extend_from_slice(&best_buf);
        prev = row;
    }
    raw
}

/// straight RGBA8 を PNG にエンコードする（自作）。行フィルタ選択 (MSAD) +
/// 自作 DEFLATE (`deflate::zlib_compress`) による実圧縮。
pub fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(
        rgba.len(),
        width as usize * height as usize * 4,
        "rgba のサイズが width*height*4 と一致しません"
    );

    let compressed = {
        let raw = build_filtered_raw(width, height, rgba);
        zlib_compress(&raw)
    };

    let mut out = Vec::with_capacity(8 + 25 + 12 + compressed.len() + 12);
    out.extend_from_slice(&SIGNATURE);

    let wb = width.to_be_bytes();
    let hb = height.to_be_bytes();
    let ihdr = [
        wb[0], wb[1], wb[2], wb[3], hb[0], hb[1], hb[2], hb[3], 8, 6, 0, 0, 0,
    ];
    write_chunk(&mut out, b"IHDR", &ihdr);
    write_chunk(&mut out, b"IDAT", &compressed);
    write_chunk(&mut out, b"IEND", &[]);
    out
}

fn write_chunk(out: &mut Vec<u8>, ctype: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(ctype);
    out.extend_from_slice(data);
    let crc = crc32_update(crc32_update(0xFFFF_FFFF, ctype), data) ^ 0xFFFF_FFFF;
    out.extend_from_slice(&crc.to_be_bytes());
}

/// zlib (RFC1950) ラッパで包んだ無圧縮 (stored ブロック) DEFLATE ストリームを作る。
/// W4 で実圧縮 (`deflate::zlib_compress`) に切り替えたため通常経路では未使用。
/// テストでの圧縮率比較・stored 経路の参照実装として残す。
#[cfg(test)]
fn zlib_compress_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01]; // CMF=8(深さ32K)/CINFO=7, FLG (ヘッダチェック OK かつ最速)
    const MAX_BLOCK: usize = 65535; // stored ブロックの LEN は u16
    let mut pos = 0;
    loop {
        let end = (pos + MAX_BLOCK).min(data.len());
        let chunk = &data[pos..end];
        let is_final = end == data.len();
        out.push(if is_final { 1 } else { 0 }); // BFINAL(1bit) + BTYPE=00(2bit) + パディング
        out.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
        out.extend_from_slice(chunk);
        pos = end;
        if is_final {
            break;
        }
    }
    out.extend_from_slice(&super::inflate::adler32(data).to_be_bytes());
    out
}

/// slice-by-4 用 CRC32 テーブル (ITU-T V.42 反転多項式 0xEDB88320)。
const CRC32_TABLES: [[u32; 256]; 4] = {
    let mut t = [[0u32; 256]; 4];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
            k += 1;
        }
        t[0][i] = c;
        i += 1;
    }
    let mut j = 1usize;
    while j < 4 {
        let mut i = 0usize;
        while i < 256 {
            let prev = t[j - 1][i];
            t[j][i] = (prev >> 8) ^ t[0][(prev & 0xFF) as usize];
            i += 1;
        }
        j += 1;
    }
    t
};

fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    let (chunks, rem) = data.as_chunks::<4>();
    for &b in chunks {
        crc ^= u32::from_le_bytes(b);
        crc = CRC32_TABLES[3][(crc & 0xFF) as usize]
            ^ CRC32_TABLES[2][((crc >> 8) & 0xFF) as usize]
            ^ CRC32_TABLES[1][((crc >> 16) & 0xFF) as usize]
            ^ CRC32_TABLES[0][(crc >> 24) as usize];
    }
    for &byte in rem {
        crc = (crc >> 8) ^ CRC32_TABLES[0][((crc ^ byte as u32) & 0xFF) as usize];
    }
    crc
}

/// PNG チャンクの CRC32 (ITU-T V.42 多項式、反転版 0xEDB88320)。
/// Ogg で使う CRC32 (`ogg.rs`) とは多項式が異なるため共用しない。
/// W6: `pak.rs` の NPK1 エントリ整合性チェックが同じ多項式 (zip/PNG 系) を必要とするため
/// `pub(crate)` にしてここから再利用している（テスト済みの実装を重複させないため）。
pub(crate) fn crc32(data: &[u8]) -> u32 {
    crc32_update(0xFFFF_FFFF, data) ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::png::PngEncoder;
    use image::ImageEncoder;

    /// image クレートで PNG にエンコードし、自作デコーダの出力と
    /// image クレートのデコード結果をピクセル単位で突き合わせる。
    fn roundtrip_rgba(w: u32, h: u32, rgba: Vec<u8>) {
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&rgba, w, h, image::ExtendedColorType::Rgba8)
            .unwrap();
        let ours = decode(&png).unwrap();
        let theirs = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!((ours.width, ours.height), (w, h));
        assert_eq!(ours.rgba, theirs.as_raw().as_slice(), "ピクセル不一致");
        assert_eq!(ours.rgba, rgba, "元データと不一致");
    }

    fn lcg(seed: &mut u64) -> u8 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 33) as u8
    }

    #[test]
    fn decode_noise_rgba() {
        // ノイズは動的ハフマン + 全フィルタを強く踏む
        let (w, h) = (73u32, 41u32);
        let mut seed = 42u64;
        let rgba: Vec<u8> = (0..w * h * 4).map(|_| lcg(&mut seed)).collect();
        roundtrip_rgba(w, h, rgba);
    }

    #[test]
    fn decode_gradient_rgba() {
        // グラデーションは Sub/Up/Paeth フィルタが選ばれやすい
        let (w, h) = (128u32, 96u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[(x * 2) as u8, (y * 2) as u8, ((x + y) % 256) as u8, 255]);
            }
        }
        roundtrip_rgba(w, h, rgba);
    }

    #[test]
    fn decode_flat_color() {
        // 均一色は長い一致 (LZ77 距離コピー) を踏む
        roundtrip_rgba(200, 150, vec![120; 200 * 150 * 4]);
    }

    #[test]
    fn decode_rgb_without_alpha() {
        let (w, h) = (64u32, 48u32);
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgb.extend_from_slice(&[(x * 4) as u8, (y * 5) as u8, 200]);
            }
        }
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        let ours = decode(&png).unwrap();
        let theirs = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(ours.rgba, theirs.as_raw().as_slice());
    }

    #[test]
    fn decode_grayscale() {
        let (w, h) = (50u32, 30u32);
        let gray: Vec<u8> = (0..w * h).map(|i| (i % 256) as u8).collect();
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&gray, w, h, image::ExtendedColorType::L8)
            .unwrap();
        let ours = decode(&png).unwrap();
        let theirs = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(ours.rgba, theirs.as_raw().as_slice());
    }

    #[test]
    fn decode_gray_alpha() {
        let (w, h) = (33u32, 17u32);
        let mut la = Vec::new();
        for i in 0..w * h {
            la.extend_from_slice(&[(i % 256) as u8, ((i * 7) % 256) as u8]);
        }
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&la, w, h, image::ExtendedColorType::La8)
            .unwrap();
        let ours = decode(&png).unwrap();
        let theirs = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(ours.rgba, theirs.as_raw().as_slice());
    }

    #[test]
    fn decode_generated_game_assets() {
        // gen-assets が生成した実アセットも自作デコーダで一致すること
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/sample/assets");
        if !root.exists() {
            return; // アセット未生成の環境ではスキップ
        }
        for rel in [
            "bg/room.png",
            "bg/street.png",
            "fg/alice_normal.png",
            "rule/wipe.png",
        ] {
            let path = root.join(rel);
            let bytes = std::fs::read(&path).unwrap();
            let ours = decode(&bytes).unwrap();
            let theirs = image::load_from_memory(&bytes).unwrap().to_rgba8();
            assert_eq!(
                ours.rgba,
                theirs.as_raw().as_slice(),
                "{rel} のデコード結果が不一致"
            );
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"not a png").is_err());
        let mut broken = SIGNATURE.to_vec();
        broken.extend_from_slice(&[0, 0, 0, 1]);
        assert!(decode(&broken).is_err());
    }

    /// 自作エンコーダ → 自作デコーダで往復し、ピクセルが一致すること。
    fn encode_decode_roundtrip(w: u32, h: u32, rgba: Vec<u8>) {
        let png = encode(w, h, &rgba);
        let decoded = decode(&png).unwrap();
        assert_eq!((decoded.width, decoded.height), (w, h));
        assert_eq!(decoded.rgba, rgba, "エンコード→デコードでピクセルが不一致");
    }

    #[test]
    fn encode_roundtrip_noise() {
        let (w, h) = (73u32, 41u32);
        let mut seed = 99u64;
        let rgba: Vec<u8> = (0..w * h * 4).map(|_| lcg(&mut seed)).collect();
        encode_decode_roundtrip(w, h, rgba);
    }

    #[test]
    fn encode_roundtrip_flat() {
        encode_decode_roundtrip(16, 8, vec![7; 16 * 8 * 4]);
    }

    #[test]
    fn encode_roundtrip_with_transparency() {
        let (w, h) = (10u32, 10u32);
        let mut rgba = Vec::new();
        for i in 0..w * h {
            rgba.extend_from_slice(&[(i % 256) as u8, 0, 255, (i * 3 % 256) as u8]);
        }
        encode_decode_roundtrip(w, h, rgba);
    }

    #[test]
    fn encode_roundtrip_multi_stored_block() {
        // 1 行あたり >65535 バイトになる大きな画像で stored ブロック分割を踏む。
        let (w, h) = (20000u32, 2u32);
        let mut seed = 7u64;
        let rgba: Vec<u8> = (0..w * h * 4).map(|_| lcg(&mut seed)).collect();
        encode_decode_roundtrip(w, h, rgba);
    }

    #[test]
    fn encode_readable_by_image_crate() {
        // image クレートでも読めること（PNG ファイルとしての妥当性の裏取り）。
        let (w, h) = (12u32, 9u32);
        let mut seed = 3u64;
        let rgba: Vec<u8> = (0..w * h * 4).map(|_| lcg(&mut seed)).collect();
        let png = encode(w, h, &rgba);
        let theirs = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(theirs.as_raw().as_slice(), rgba.as_slice());
    }

    #[test]
    fn encode_generated_game_assets_readable_by_image_crate() {
        // 回帰テスト: 我々の inflate.rs は不完全 (degenerate) なハフマン木でも
        // 許容してしまうため、自作デコーダとの round-trip だけでは
        // ハフマン符号長構築の不備 (Kraft 不等式の非等号) を検出できない。
        // image クレート (png クレート経由、zlib 準拠の厳密なデコーダ) で
        // 実際にゲーム背景相当の大きな画像を再デコードできることを確認する。
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/sample/assets");
        if !root.exists() {
            return;
        }
        for rel in [
            "bg/room.png",
            "bg/street.png",
            "rule/circle.png",
            "rule/wipe.png",
        ] {
            let path = root.join(rel);
            if !path.exists() {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let decoded = decode(&bytes).unwrap();
            let png = encode(decoded.width, decoded.height, &decoded.rgba);
            let theirs = image::load_from_memory(&png)
                .unwrap_or_else(|e| {
                    panic!("{rel} の再エンコード結果を image クレートが読めない: {e:#}")
                })
                .to_rgba8();
            assert_eq!(
                theirs.as_raw().as_slice(),
                decoded.rgba.as_slice(),
                "{rel} のピクセルが不一致"
            );
        }
    }

    #[test]
    fn encode_smaller_than_stored() {
        // 実圧縮 (deflate) は無圧縮 (stored) より必ず小さいか同等であること。
        let (w, h) = (128u32, 96u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[(x * 2) as u8, (y * 2) as u8, ((x + y) % 256) as u8, 255]);
            }
        }
        let raw = build_filtered_raw(w, h, &rgba);
        let compressed_len = zlib_compress(&raw).len();
        let stored_len = zlib_compress_stored(&raw).len();
        assert!(
            compressed_len < stored_len,
            "圧縮後 ({compressed_len}) が無圧縮 ({stored_len}) より小さくなっていない"
        );
    }

    #[test]
    fn compression_ratio_report_sample_assets() {
        // W4 品質基準: sample の PNG 群を自作デコード→再エンコードし、
        // stored (無圧縮) 比のサイズと所要時間を報告する（アセット差し替えはしない）。
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/sample/assets");
        if !root.exists() {
            return;
        }
        let mut files = Vec::new();
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "png") {
                    out.push(path);
                }
            }
        }
        walk(&root, &mut files);
        if files.is_empty() {
            return;
        }

        let mut total_stored = 0u64;
        let mut total_compressed = 0u64;
        let mut total_image_crate = 0u64;
        let mut total_elapsed = std::time::Duration::ZERO;
        for path in &files {
            let bytes = std::fs::read(path).unwrap();
            let decoded = decode(&bytes).unwrap();
            let raw = build_filtered_raw(decoded.width, decoded.height, &decoded.rgba);
            let start = std::time::Instant::now();
            let compressed = zlib_compress(&raw);
            let per_file = start.elapsed();
            total_elapsed += per_file;
            let stored = zlib_compress_stored(&raw);

            // 参照値: image クレート (png crate 経由、既定圧縮設定) での再エンコードサイズ。
            let mut theirs_png = Vec::new();
            PngEncoder::new(&mut theirs_png)
                .write_image(
                    &decoded.rgba,
                    decoded.width,
                    decoded.height,
                    image::ExtendedColorType::Rgba8,
                )
                .unwrap();

            total_stored += stored.len() as u64;
            total_compressed += compressed.len() as u64;
            total_image_crate += theirs_png.len() as u64;
            eprintln!(
                "  {:?} {}x{} raw={}B -> ours={}B image-crate={}B ({:?})",
                path.file_name().unwrap(),
                decoded.width,
                decoded.height,
                raw.len(),
                compressed.len(),
                theirs_png.len(),
                per_file
            );
        }
        eprintln!(
            "[W4 圧縮率レポート] {} 枚, stored={}B ours={}B ({:.1}%) image-crate参考={}B ({:.1}%) elapsed={:?}",
            files.len(),
            total_stored,
            total_compressed,
            100.0 * total_compressed as f64 / total_stored as f64,
            total_image_crate,
            100.0 * total_compressed as f64 / total_image_crate as f64,
            total_elapsed
        );
        assert!(
            total_compressed < total_stored,
            "圧縮後合計が無圧縮合計を上回っている"
        );
    }
    // --- 外部入力の堅牢化（R1） ---

    fn ihdr_png(width: u32, height: u32, color_type: u8, idat: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&SIGNATURE);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, color_type, 0, 0, 0]);
        write_chunk(&mut out, b"IHDR", &ihdr);
        write_chunk(&mut out, b"IDAT", idat);
        write_chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn huge_ihdr_dimensions_are_errors_not_overflow_panics() {
        // 中身が空の zlib ストリーム (stored, 0 バイト)。h * (stride + 1) が usize を溢れる寸法。
        let empty_zlib = zlib_compress_stored(&[]);
        for (w, h) in [
            (u32::MAX, u32::MAX),
            (u32::MAX, 1),
            (1, u32::MAX),
            (65535, 65535),
            (0x4000_0000, 0x4000_0000),
        ] {
            for color_type in [0u8, 2, 3, 4, 6] {
                let png = ihdr_png(w, h, color_type, &empty_zlib);
                assert!(
                    decode(&png).is_err(),
                    "{w}x{h} color_type={color_type} が Err にならない"
                );
            }
        }
    }

    #[test]
    fn deflate_bomb_in_idat_is_not_expanded_beyond_the_expected_size() {
        // 1x1 の PNG に 16MB ぶんに展開される IDAT。期待サイズ (5 バイト) を超える展開はしない。
        let bomb = zlib_compress(&vec![0u8; 16 << 20]);
        let png = ihdr_png(1, 1, 6, &bomb);
        let r = crate::test_support::with_alloc_limit(1 << 20, || decode(&png));
        assert!(r.is_err() || r.unwrap().rgba.len() == 4);
    }
    // --- 破壊 fuzz（R1） ---

    fn png_palette_bytes() -> Vec<u8> {
        let (w, h) = (9usize, 7usize);
        let mut raw = Vec::new();
        for y in 0..h {
            raw.push(0u8);
            raw.extend((0..w).map(|x| ((x + y) % 4) as u8));
        }
        let mut out = Vec::new();
        out.extend_from_slice(&SIGNATURE);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&(w as u32).to_be_bytes());
        ihdr.extend_from_slice(&(h as u32).to_be_bytes());
        ihdr.extend_from_slice(&[8, 3, 0, 0, 0]);
        write_chunk(&mut out, b"IHDR", &ihdr);
        write_chunk(
            &mut out,
            b"PLTE",
            &[0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255],
        );
        write_chunk(&mut out, b"tRNS", &[255, 128]);
        write_chunk(&mut out, b"IDAT", &zlib_compress(&raw));
        write_chunk(&mut out, b"IEND", &[]);
        out
    }

    fn png_via_image_crate(
        w: u32,
        h: u32,
        color: image::ExtendedColorType,
        channels: usize,
    ) -> Vec<u8> {
        let mut seed = 7u64;
        let px: Vec<u8> = (0..w as usize * h as usize * channels)
            .map(|_| lcg(&mut seed))
            .collect();
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&px, w, h, color)
            .unwrap();
        png
    }

    #[test]
    fn fuzz_corrupted_png_never_panics_or_overallocates() {
        let mut seed = 3u64;
        let rgba: Vec<u8> = (0..13 * 9 * 4).map(|_| lcg(&mut seed)).collect();
        let bases = [
            png_via_image_crate(13, 9, image::ExtendedColorType::Rgba8, 4),
            png_via_image_crate(11, 8, image::ExtendedColorType::Rgb8, 3),
            png_via_image_crate(10, 10, image::ExtendedColorType::L8, 1),
            png_via_image_crate(10, 6, image::ExtendedColorType::La8, 2),
            png_palette_bytes(),
            encode(13, 9, &rgba),
        ];
        for (i, base) in bases.iter().enumerate() {
            assert!(decode(base).is_ok(), "基準入力 {i} がデコードできる");
            crate::test_support::fuzz("png", 0x2222 + i as u64, 700, base, |d| {
                if let Ok(img) = decode(d) {
                    assert_eq!(img.rgba.len(), img.width as usize * img.height as usize * 4);
                }
            });
        }
    }
}
