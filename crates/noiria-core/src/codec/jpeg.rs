//! JPEG デコーダ（自作）。ベースライン DCT (SOF0)・ハフマン符号のみ対応。
//! プログレッシブ (SOF2) や算術符号は非対応。出力は straight RGBA8。

use anyhow::{bail, ensure, Context, Result};

pub struct DecodedJpeg {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// MSB ファーストのビットリーダ（0xFF00 のバイトスタッフィングを処理）。
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bits: u32,
    nbits: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos,
            bits: 0,
            nbits: 0,
        }
    }

    fn fill(&mut self) -> Result<()> {
        while self.nbits <= 24 {
            if self.pos >= self.data.len() {
                // パディング (EOI 手前) は 1 詰め扱い
                self.bits |= 0xFF << (24 - self.nbits);
                self.nbits += 8;
                continue;
            }
            let b = self.data[self.pos];
            if b == 0xFF {
                let next = self.data.get(self.pos + 1).copied().unwrap_or(0xD9);
                if next == 0x00 {
                    self.pos += 2; // スタッフィング
                } else {
                    // マーカー (RST/EOI): 消費せず 1 詰め
                    self.bits |= 0xFF << (24 - self.nbits);
                    self.nbits += 8;
                    continue;
                }
            } else {
                self.pos += 1;
            }
            self.bits |= (b as u32) << (24 - self.nbits);
            self.nbits += 8;
        }
        Ok(())
    }

    fn peek16(&mut self) -> Result<u16> {
        self.fill()?;
        Ok((self.bits >> 16) as u16)
    }

    fn consume(&mut self, n: u32) {
        self.bits <<= n;
        self.nbits -= n;
    }

    fn get_bits(&mut self, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        self.fill()?;
        let v = self.bits >> (32 - n);
        self.consume(n);
        Ok(v)
    }

    /// RST マーカー境界でリセットする。
    fn sync_restart(&mut self) -> Result<()> {
        self.bits = 0;
        self.nbits = 0;
        // pos をマーカーまで進める
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xFF {
                let m = self.data[self.pos + 1];
                if (0xD0..=0xD7).contains(&m) {
                    self.pos += 2;
                    return Ok(());
                }
                if m == 0x00 {
                    self.pos += 2;
                    continue;
                }
                bail!("RST マーカーが見つかりません (found FF{m:02X})");
            }
            self.pos += 1;
        }
        bail!("RST マーカーの探索中にデータが尽きました");
    }
}

/// ハフマン表: (符号長ごとの個数, 値) から高速テーブルなしの逐次デコード。
struct Huffman {
    /// mincode/maxcode/valptr 方式 (ITU-T T.81 F.2.2.3)
    mincode: [i32; 17],
    maxcode: [i32; 17],
    valptr: [i32; 17],
    values: Vec<u8>,
}

impl Huffman {
    fn new(counts: &[u8; 16], values: Vec<u8>) -> Self {
        let mut mincode = [0i32; 17];
        let mut maxcode = [-1i32; 17];
        let mut valptr = [0i32; 17];
        let mut code = 0i32;
        let mut k = 0i32;
        for len in 1..=16usize {
            valptr[len] = k;
            mincode[len] = code;
            code += counts[len - 1] as i32;
            k += counts[len - 1] as i32;
            maxcode[len] = code - 1;
            code <<= 1;
        }
        Self {
            mincode,
            maxcode,
            valptr,
            values,
        }
    }

    fn decode(&self, br: &mut BitReader) -> Result<u8> {
        let peek = br.peek16()? as i32;
        let mut code = 0i32;
        for len in 1..=16usize {
            code = peek >> (16 - len);
            if self.maxcode[len] >= 0 && code <= self.maxcode[len] && code >= self.mincode[len] {
                br.consume(len as u32);
                let idx = (self.valptr[len] + code - self.mincode[len]) as usize;
                return self.values.get(idx).copied().context("ハフマン値が範囲外");
            }
        }
        bail!("不正なハフマン符号: {code:016b}");
    }
}

/// 符号付き振幅の拡張 (T.81 F.2.2.1 EXTEND)
fn extend(v: u32, n: u32) -> i32 {
    if n == 0 {
        return 0;
    }
    if v < (1 << (n - 1)) {
        v as i32 - (1 << n) + 1
    } else {
        v as i32
    }
}

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// IDCT 用テーブル: T[x][u] = c(u) * cos((2x+1)uπ/16) * 0.5
fn idct_table() -> &'static [[f32; 8]; 8] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<[[f32; 8]; 8]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [[0f32; 8]; 8];
        for (x, row) in t.iter_mut().enumerate() {
            for (u, v) in row.iter_mut().enumerate() {
                let c = if u == 0 {
                    std::f32::consts::FRAC_1_SQRT_2
                } else {
                    1.0
                };
                *v = c
                    * (((2 * x + 1) as f32) * (u as f32) * std::f32::consts::PI / 16.0).cos()
                    * 0.5;
            }
        }
        t
    })
}

/// 8x8 逆 DCT（分離型・テーブル参照）。
fn idct8x8(block: &[f32; 64], out: &mut [f32; 64]) {
    let t = idct_table();
    let mut tmp = [0f32; 64];
    // 行方向
    for y in 0..8 {
        for x in 0..8 {
            let mut s = 0f32;
            for u in 0..8 {
                s += t[x][u] * block[y * 8 + u];
            }
            tmp[y * 8 + x] = s;
        }
    }
    // 列方向
    for x in 0..8 {
        for y in 0..8 {
            let mut s = 0f32;
            for v in 0..8 {
                s += t[y][v] * tmp[v * 8 + x];
            }
            out[y * 8 + x] = s;
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// SOS で指定される DC/AC ハフマン表番号
    td: usize,
    ta: usize,
    dc_pred: i32,
}

/// デコードを受け付ける最大画素数（8192x8192。RGBA 出力で 256MB）。SOF の 16bit 幅・高さが
/// 申告するだけで 4G 画素ぶんのプレーンを確保させない。
const MAX_PIXELS: usize = 1 << 26;

pub fn decode(data: &[u8]) -> Result<DecodedJpeg> {
    ensure!(
        data.len() > 4 && data[0] == 0xFF && data[1] == 0xD8,
        "JPEG SOI がありません"
    );

    let mut qt: [[u16; 64]; 4] = [[0; 64]; 4];
    let mut dc_tables: Vec<Option<Huffman>> = (0..4).map(|_| None).collect();
    let mut ac_tables: Vec<Option<Huffman>> = (0..4).map(|_| None).collect();
    let mut comps: Vec<Component> = Vec::new();
    let (mut width, mut height) = (0usize, 0usize);
    let mut restart_interval = 0usize;

    let mut pos = 2usize;
    loop {
        ensure!(pos + 4 <= data.len(), "マーカーの途中でデータが尽きました");
        ensure!(data[pos] == 0xFF, "マーカーが同期していません");
        let marker = data[pos + 1];
        pos += 2;
        if marker == 0xD9 {
            bail!("画像データの前に EOI が現れました");
        }
        let len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        ensure!(len >= 2 && pos + len <= data.len(), "セグメント長が不正");
        let seg = &data[pos + 2..pos + len];
        let seg_end = pos + len;

        match marker {
            0xDB => {
                // DQT
                let mut p = 0;
                while p < seg.len() {
                    let pq = seg[p] >> 4;
                    let tq = (seg[p] & 15) as usize;
                    ensure!(tq < 4, "量子化テーブル番号が不正");
                    p += 1;
                    let need = if pq == 0 { 64 } else { 128 };
                    ensure!(p + need <= seg.len(), "DQT が短すぎます");
                    for i in 0..64 {
                        if pq == 0 {
                            qt[tq][i] = seg[p + i] as u16;
                        } else {
                            qt[tq][i] = u16::from_be_bytes([seg[p + 2 * i], seg[p + 2 * i + 1]]);
                        }
                    }
                    p += if pq == 0 { 64 } else { 128 };
                }
            }
            0xC4 => {
                // DHT
                let mut p = 0;
                while p + 17 <= seg.len() {
                    let class = seg[p] >> 4;
                    let id = (seg[p] & 15) as usize;
                    ensure!(id < 4, "ハフマン表番号が不正");
                    let mut counts = [0u8; 16];
                    counts.copy_from_slice(&seg[p + 1..p + 17]);
                    let total: usize = counts.iter().map(|&c| c as usize).sum();
                    ensure!(p + 17 + total <= seg.len(), "DHT が短すぎます");
                    let values = seg[p + 17..p + 17 + total].to_vec();
                    let table = Huffman::new(&counts, values);
                    if class == 0 {
                        dc_tables[id] = Some(table);
                    } else {
                        ac_tables[id] = Some(table);
                    }
                    p += 17 + total;
                }
            }
            0xC0 => {
                // SOF0 (baseline)
                ensure!(seg.len() >= 6, "SOF が短すぎます");
                ensure!(seg[0] == 8, "ビット精度 {} は非対応", seg[0]);
                height = u16::from_be_bytes([seg[1], seg[2]]) as usize;
                width = u16::from_be_bytes([seg[3], seg[4]]) as usize;
                let n = seg[5] as usize;
                ensure!(n == 1 || n == 3, "コンポーネント数 {n} は非対応");
                ensure!(seg.len() >= 6 + n * 3, "SOF のコンポーネント記述が短すぎます");
                comps.clear();
                for c in 0..n {
                    let b = &seg[6 + c * 3..9 + c * 3];
                    let (h, v, tq) = ((b[1] >> 4) as usize, (b[1] & 15) as usize, b[2] as usize);
                    ensure!(
                        (1..=4).contains(&h) && (1..=4).contains(&v),
                        "サンプリング比が不正"
                    );
                    ensure!(tq < 4, "量子化テーブル番号が不正");
                    comps.push(Component {
                        id: b[0],
                        h,
                        v,
                        tq,
                        ..Default::default()
                    });
                }
            }
            0xC1 | 0xC2 | 0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                bail!(
                    "非対応の JPEG 形式 (SOF{:X})。ベースラインのみ対応",
                    marker & 0x0F
                );
            }
            0xDD => {
                ensure!(seg.len() >= 2, "DRI が短すぎます");
                restart_interval = u16::from_be_bytes([seg[0], seg[1]]) as usize;
            }
            0xDA => {
                // SOS
                ensure!(!seg.is_empty(), "SOS が短すぎます");
                let ns = seg[0] as usize;
                ensure!(ns == comps.len(), "SOS のコンポーネント数が不一致");
                ensure!(seg.len() > ns * 2, "SOS が短すぎます");
                for s in 0..ns {
                    let cid = seg[1 + s * 2];
                    let tables = seg[2 + s * 2];
                    let comp = comps
                        .iter_mut()
                        .find(|c| c.id == cid)
                        .context("SOS のコンポーネント ID が不明")?;
                    comp.td = (tables >> 4) as usize;
                    comp.ta = (tables & 15) as usize;
                    ensure!(comp.td < 4 && comp.ta < 4, "ハフマン表番号が不正");
                }
                // エントロピー符号化データ本体をデコード
                return decode_scan(
                    data,
                    seg_end,
                    width,
                    height,
                    &comps,
                    &qt,
                    &dc_tables,
                    &ac_tables,
                    restart_interval,
                );
            }
            _ => {} // APPn / COM 等は無視
        }
        pos = seg_end;
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_scan(
    data: &[u8],
    scan_start: usize,
    width: usize,
    height: usize,
    comps_in: &[Component],
    qt: &[[u16; 64]; 4],
    dc_tables: &[Option<Huffman>],
    ac_tables: &[Option<Huffman>],
    restart_interval: usize,
) -> Result<DecodedJpeg> {
    ensure!(width > 0 && height > 0, "画像サイズが不正");
    ensure!(
        width * height <= MAX_PIXELS,
        "画像が大きすぎます: {width}x{height}"
    );
    let mut comps: Vec<Component> = comps_in.to_vec();
    let hmax = comps.iter().map(|c| c.h).max().unwrap_or(1);
    let vmax = comps.iter().map(|c| c.v).max().unwrap_or(1);
    ensure!(
        (1..=4).contains(&hmax) && (1..=4).contains(&vmax),
        "サンプリング比が不正"
    );

    let mcu_w = hmax * 8;
    let mcu_h = vmax * 8;
    let mcus_x = width.div_ceil(mcu_w);
    let mcus_y = height.div_ceil(mcu_h);

    // コンポーネントごとのプレーン（MCU グリッドに切り上げたサイズ）
    let mut planes: Vec<Vec<u8>> = comps
        .iter()
        .map(|c| vec![0u8; (mcus_x * c.h * 8) * (mcus_y * c.v * 8)])
        .collect();

    let mut br = BitReader::new(data, scan_start);
    let mut block = [0f32; 64];
    let mut pixels = [0f32; 64];
    let mut mcu_count = 0usize;

    for my in 0..mcus_y {
        for mx in 0..mcus_x {
            if restart_interval > 0 && mcu_count > 0 && mcu_count.is_multiple_of(restart_interval) {
                br.sync_restart()?;
                for c in comps.iter_mut() {
                    c.dc_pred = 0;
                }
            }
            mcu_count += 1;

            for (ci, comp) in comps.iter_mut().enumerate() {
                let dc = dc_tables[comp.td]
                    .as_ref()
                    .context("DC ハフマン表がありません")?;
                let ac = ac_tables[comp.ta]
                    .as_ref()
                    .context("AC ハフマン表がありません")?;
                let q = &qt[comp.tq];
                let plane_w = mcus_x * comp.h * 8;

                for by in 0..comp.v {
                    for bx in 0..comp.h {
                        // --- 1 ブロックのデコード ---
                        block.fill(0.0);
                        // DC
                        let t = dc.decode(&mut br)? as u32;
                        ensure!(t <= 11, "DC カテゴリが不正");
                        let diff = extend(br.get_bits(t)?, t);
                        comp.dc_pred = comp.dc_pred.wrapping_add(diff);
                        block[0] = comp.dc_pred as f32 * q[0] as f32;
                        // AC
                        let mut k = 1usize;
                        while k < 64 {
                            let rs = ac.decode(&mut br)?;
                            let r = (rs >> 4) as usize;
                            let s = (rs & 15) as u32;
                            if s == 0 {
                                if r == 15 {
                                    k += 16; // ZRL
                                    continue;
                                }
                                break; // EOB
                            }
                            k += r;
                            ensure!(k < 64, "AC 係数が範囲外");
                            let v = extend(br.get_bits(s)?, s);
                            block[ZIGZAG[k]] = v as f32 * q[k] as f32;
                            k += 1;
                        }
                        idct8x8(&block, &mut pixels);
                        // 書き込み（レベルシフト + クランプ）
                        let ox = (mx * comp.h + bx) * 8;
                        let oy = (my * comp.v + by) * 8;
                        for y in 0..8 {
                            let row = (oy + y) * plane_w + ox;
                            for x in 0..8 {
                                planes[ci][row + x] =
                                    (pixels[y * 8 + x] + 128.0).clamp(0.0, 255.0) as u8;
                            }
                        }
                    }
                }
            }
        }
    }

    // YCbCr → RGBA（クロマは最近傍アップサンプル）
    let mut rgba = Vec::with_capacity(width * height * 4);
    if comps.len() == 1 {
        let plane_w = mcus_x * comps[0].h * 8;
        for y in 0..height {
            for x in 0..width {
                let g = planes[0][y * plane_w + x];
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
        }
    } else {
        let yw = mcus_x * comps[0].h * 8;
        let cbw = mcus_x * comps[1].h * 8;
        let crw = mcus_x * comps[2].h * 8;
        for py in 0..height {
            for px in 0..width {
                // 輝度も含めて各成分を自身のサンプリング比で引く（輝度がクロマより粗い JPEG でも
                // 範囲内。輝度が最大サンプリングの通常の JPEG では px / py と同じ位置になる）。
                let ly = py * comps[0].v / vmax;
                let lx = px * comps[0].h / hmax;
                let yy = planes[0][ly * yw + lx] as f32;
                let cy = py * comps[1].v / vmax;
                let cx = px * comps[1].h / hmax;
                let cb = planes[1][cy * cbw + cx] as f32 - 128.0;
                let cy2 = py * comps[2].v / vmax;
                let cx2 = px * comps[2].h / hmax;
                let cr = planes[2][cy2 * crw + cx2] as f32 - 128.0;
                let r = (yy + 1.402 * cr).clamp(0.0, 255.0) as u8;
                let g = (yy - 0.344136 * cb - 0.714136 * cr).clamp(0.0, 255.0) as u8;
                let b = (yy + 1.772 * cb).clamp(0.0, 255.0) as u8;
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
    }

    Ok(DecodedJpeg {
        width: width as u32,
        height: height as u32,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;

    /// image クレートでエンコードし、自作デコーダと image クレートのデコードを
    /// 誤差許容つきで比較する（JPEG はデコーダ間で丸めが異なるため完全一致しない）。
    fn roundtrip(w: u32, h: u32, rgb: &[u8], quality: u8, max_diff: i32, mean_limit: f64) {
        let mut jpg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpg, quality)
            .encode(rgb, w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        let ours = decode(&jpg).unwrap();
        let theirs = image::load_from_memory(&jpg).unwrap().to_rgba8();
        assert_eq!((ours.width, ours.height), (w, h));

        let mut total = 0i64;
        let mut worst = 0i32;
        for (a, b) in ours.rgba.as_chunks::<4>().0.iter().zip(theirs.as_chunks::<4>().0) {
            for c in 0..3 {
                let d = (a[c] as i32 - b[c] as i32).abs();
                worst = worst.max(d);
                total += d as i64;
            }
        }
        let mean = total as f64 / (w as f64 * h as f64 * 3.0);
        assert!(
            worst <= max_diff,
            "最大誤差 {worst} が許容 {max_diff} を超過 (mean={mean:.2})"
        );
        assert!(
            mean <= mean_limit,
            "平均誤差 {mean:.2} が許容 {mean_limit} を超過"
        );
    }

    fn gradient_rgb(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&[
                    (x * 255 / w.max(1)) as u8,
                    (y * 255 / h.max(1)) as u8,
                    ((x + y) % 256) as u8,
                ]);
            }
        }
        v
    }

    #[test]
    fn decode_gradient_high_quality() {
        let (w, h) = (128u32, 96u32);
        roundtrip(w, h, &gradient_rgb(w, h), 95, 8, 2.0);
    }

    #[test]
    fn decode_gradient_mid_quality() {
        let (w, h) = (160u32, 120u32);
        roundtrip(w, h, &gradient_rgb(w, h), 70, 12, 3.0);
    }

    #[test]
    fn decode_flat_color() {
        let (w, h) = (64u32, 64u32);
        let rgb = vec![180u8; (w * h * 3) as usize];
        roundtrip(w, h, &rgb, 85, 6, 1.0);
    }

    #[test]
    fn decode_odd_size() {
        // MCU 境界に合わないサイズ（パディング処理の検証）
        let (w, h) = (33u32, 17u32);
        roundtrip(w, h, &gradient_rgb(w, h), 90, 10, 2.5);
    }

    #[test]
    fn decode_photo_like() {
        // 疑似写真: 低周波 + 少しの高周波
        let (w, h) = (200u32, 150u32);
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let fx = x as f32 / w as f32 * std::f32::consts::TAU;
                let fy = y as f32 / h as f32 * std::f32::consts::TAU;
                rgb.extend_from_slice(&[
                    ((fx.sin() * 0.4 + 0.5) * 255.0) as u8,
                    ((fy.cos() * 0.4 + 0.5) * 255.0) as u8,
                    (((fx + fy).sin() * 0.3 + 0.5) * 255.0) as u8,
                ]);
            }
        }
        roundtrip(w, h, &rgb, 80, 12, 3.0);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"not a jpeg").is_err());
        assert!(decode(&[0xFF, 0xD8, 0xFF, 0xD9]).is_err());
    }

    // --- 外部入力の堅牢化（R1）: 細工したセグメントで panic / 過大確保しないこと ---

    fn small_jpeg() -> Vec<u8> {
        let (w, h) = (16u32, 16u32);
        let mut jpg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpg, 85)
            .encode(&gradient_rgb(w, h), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        jpg
    }

    /// マーカー `m` のセグメント先頭（FF の位置）を返す。
    fn marker_pos(jpg: &[u8], m: u8) -> usize {
        let mut pos = 2;
        loop {
            assert_eq!(jpg[pos], 0xFF);
            if jpg[pos + 1] == m {
                return pos;
            }
            pos += 2 + u16::from_be_bytes([jpg[pos + 2], jpg[pos + 3]]) as usize;
        }
    }

    #[test]
    fn truncated_segment_bodies_are_errors_not_panics() {
        let cases: [&[u8]; 5] = [
            // 空の SOF / DRI / SOS（本体が 0 バイト）
            &[0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x02],
            &[0xFF, 0xD8, 0xFF, 0xDD, 0x00, 0x02],
            &[0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02],
            // 本体が 3 バイトしかない DQT（8bit 精度なら 65 バイト要る）
            &[0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x05, 0x00, 0x01, 0x02],
            // コンポーネント数 3 を名乗るが成分記述が無い SOF
            &[
                0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x08, 0x08, 0x00, 0x10, 0x00, 0x10, 0x03,
            ],
        ];
        for (i, c) in cases.iter().enumerate() {
            assert!(decode(c).is_err(), "case {i} が Err にならない");
        }
    }

    #[test]
    fn sof_quant_table_selector_out_of_range_is_error() {
        let mut jpg = small_jpeg();
        let sof = marker_pos(&jpg, 0xC0);
        jpg[sof + 4 + 6 + 2] = 7; // 成分 0 の tq
        assert!(decode(&jpg).is_err());
    }

    #[test]
    fn sos_huffman_table_selector_out_of_range_is_error() {
        let mut jpg = small_jpeg();
        let sos = marker_pos(&jpg, 0xDA);
        jpg[sos + 4 + 1 + 1] = 0x77; // 成分 0 の td/ta
        assert!(decode(&jpg).is_err());
    }

    #[test]
    fn sof_zero_sampling_factor_is_error() {
        for comp in 0..3 {
            let mut jpg = small_jpeg();
            let sof = marker_pos(&jpg, 0xC0);
            jpg[sof + 4 + 6 + comp * 3 + 1] = 0x00; // h = v = 0
            assert!(decode(&jpg).is_err(), "成分 {comp} の h=v=0 が Err にならない");
        }
    }

    #[test]
    fn declared_huge_dimensions_do_not_allocate_gigabytes() {
        let mut jpg = small_jpeg();
        let sof = marker_pos(&jpg, 0xC0);
        jpg[sof + 5..sof + 9].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // 65535 x 65535
        let r = crate::test_support::with_alloc_limit(64 << 20, || decode(&jpg));
        assert!(r.is_err());
    }
    // --- 破壊 fuzz（R1）: panic しない / 1 回の確保が上限を超えない ---

    fn jpeg_bytes(w: u32, h: u32, color: image::ExtendedColorType, quality: u8) -> Vec<u8> {
        let px = match color {
            image::ExtendedColorType::L8 => (0..w * h).map(|i| (i * 7 % 256) as u8).collect(),
            _ => gradient_rgb(w, h),
        };
        let mut jpg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpg, quality)
            .encode(&px, w, h, color)
            .unwrap();
        jpg
    }

    #[test]
    fn fuzz_corrupted_jpeg_never_panics_or_overallocates() {
        for (i, base) in [
            jpeg_bytes(33, 17, image::ExtendedColorType::Rgb8, 80),
            jpeg_bytes(16, 16, image::ExtendedColorType::L8, 60),
        ]
        .iter()
        .enumerate()
        {
            assert!(decode(base).is_ok(), "基準入力がデコードできる");
            crate::test_support::fuzz("jpeg", 0x1111 + i as u64, 2500, base, |d| {
                if let Ok(img) = decode(d) {
                    assert_eq!(img.rgba.len(), img.width as usize * img.height as usize * 4);
                }
            });
        }
    }
    /// 全ブロックが「DC 差分 0・AC は EOB」だけの 16x16 JPEG（成分ごとの標本化比を指定できる）。
    /// DC / AC のハフマン表は「長さ 1 の符号 '0' が 1 つ」だけなので、スキャンはゼロ詰めでよい。
    fn flat_jpeg(hv: [u8; 3]) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        j.extend_from_slice(&[0xFF, 0xDB, 0x00, 67, 0x00]);
        j.extend(std::iter::repeat_n(1u8, 64));
        j.extend_from_slice(&[0xFF, 0xC0, 0x00, 17, 8, 0, 16, 0, 16, 3]);
        for (i, v) in hv.iter().enumerate() {
            j.extend_from_slice(&[i as u8 + 1, *v, 0]);
        }
        for class_id in [0x00u8, 0x10] {
            j.extend_from_slice(&[0xFF, 0xC4, 0x00, 20, class_id, 1]);
            j.extend_from_slice(&[0u8; 15]);
            j.push(0x00);
        }
        j.extend_from_slice(&[0xFF, 0xDA, 0x00, 12, 3, 1, 0x00, 2, 0x00, 3, 0x00, 0, 63, 0]);
        j.extend(std::iter::repeat_n(0u8, 16));
        j.extend_from_slice(&[0xFF, 0xD9]);
        j
    }

    #[test]
    fn luma_sampled_coarser_than_chroma_is_decoded_within_bounds() {
        // 通常の 4:2:0（輝度が最大標本化）。
        let normal = decode(&flat_jpeg([0x22, 0x11, 0x11])).unwrap();
        assert_eq!((normal.width, normal.height), (16, 16));
        // 輝度 1x1、クロマ 2x2（仕様上は合法）。輝度プレーンを px / py で直接引くと範囲外になる。
        let odd = decode(&flat_jpeg([0x11, 0x22, 0x22])).unwrap();
        assert_eq!(odd.rgba.len(), 16 * 16 * 4);
    }
}
