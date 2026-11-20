//! FLAC 風サブセットコーデックの**エンコード側**（配布対応 W9、`noiria-pack`/`noiria-export`
//! の pack 時に WAV → pak codec=2 "wav-flac" へ変換するために使う）。
//!
//! デコード側 (`noiria_core::codec::flac_subset::decode_wav`) とは**意図的に別クレートで
//! 完全に独立実装**している。`noiria-core` は `noiria-tools` に依存できない（依存方向が逆）
//! ため、この分離はビルドグラフによっても強制される。関数・ヘルパーの共有は一切なく、
//! bitstream のビット詰め方向・ジグザグ符号化・fixed predictor 係数はこのファイルと
//! デコーダ側それぞれに独立して記述している（W4 の教訓: 対称な実装は round-trip の
//! バグを隠す）。コンテナ形式の詳細は `flac_subset.rs` のドキュメントコメント（"NFC1"）参照。
//! このファイルは encode のみを提供し、decode ロジックは一切含まない。
//!
//! ## 対応範囲とフォールバック方針
//! 16bit PCM / 1-2ch のみに対応する。以下はすべて `None` を返し、呼び出し側
//! (`noiria_tools::pack::pack_dir`) は元の WAV バイト列のまま従来の DEFLATE/stored
//! ルートへフォールバックする:
//! - RIFF/WAVE ヘッダ不正、fmt/data チャンクが見つからない
//! - フォーマットタグが PCM (1) 以外、bits が 16 以外、channels が 1/2 以外
//! - data チャンク長が 16bit×channels のフレーム境界に揃っていない（破損/非標準 WAV）
//! - 変換結果 (wav-flac blob) が元の WAV バイト列より縮まない
//!
//! ## ステレオの扱い（L/R 独立を採用、mid-side は不採用）
//! L/R 完全独立で符号化する（mid-side 等のチャンネル間デコリレーションは行わない）。
//! **選定理由**: 当初 mid-side（`mid=(L+R)>>1`, `side=L-R`）も試作したが、`games/sample`
//! の実アセット（手続き生成シンセの BGM、`docs/implementation-notes.md` によれば
//! チャンネルごとに異なる LFO 等で意図的にステレオ幅を持たせている）で実測したところ
//! **圧縮率が全く改善しなかった**（独立 L/R のみ: WAV 部 -42.2%、mid-side 込みで
//! 良い方を自動選択: 同じく -42.2%、全ファイルで独立モードが選ばれ side チャンネルの
//! 17bit 化ヘッダ分だけ僅かに悪化した）。mid-side は L/R 独立よりもコード量・ビット幅の
//! 可変性（side が 17bit になる）が増える一方、実データでの効果がゼロだったため、
//! CLAUDE.md の「簡潔さを優先・実利のない複雑化をしない」方針に従い独立実装のまま
//! 据え置いた。fixed predictor + Rice はチャンネル内の時間相関だけで実用上妥当な圧縮率
//! （games/sample 実測 42.2% 削減、計画が見込んだ 40〜60% の範囲内）が得られている
//! （`docs/implementation-notes.md` 2026-07-12 配布対応 W9 参照）。
//!
//! ## LPC 予測（W9 LPC 拡張）
//! fixed predictor（次数 0-4）に加え、Levinson-Durbin 法で導出した次数 1-8 の LPC 係数を
//! 候補に加え、ブロックごとに実測ビット数が最小になる候補（CONSTANT/VERBATIM/FIXED/LPC）を
//! 全探索で選ぶ。LPC 係数は f64 で計算した後「精度 15bit + シフト量」方式で整数へ量子化し、
//! ビットストリームには量子化後の整数だけを格納する。デコーダは量子化済み係数を読んで
//! 整数演算（積和 + 右シフト）だけで予測するため、浮動小数点の丸めは round-trip の結果に
//! 一切影響しない。詳細な設計判断は `docs/implementation-notes.md` 2026-07-13
//! wav-flac LPC 拡張を参照。

const MAGIC: &[u8; 4] = b"NFC1";
const BLOCK_SIZE: usize = 4096;

const SUBFRAME_CONSTANT: u64 = 0;
const SUBFRAME_VERBATIM: u64 = 1;
const SUBFRAME_FIXED: u64 = 2;
const SUBFRAME_LPC: u64 = 3;

/// LPC 係数の量子化精度（bit 幅）。固定値 15bit を採用（4bit フィールドで表現できる最大値
/// で、量子化誤差をほぼ無視できるレベルに抑える）。次数ごとに精度を探索する案もあったが、
/// 精度を固定して探索次元を増やさない方が単純で、実測でも精度を下げる恩恵は小さかった
/// （`docs/implementation-notes.md` 2026-07-13 wav-flac LPC 拡張 参照）。
const LPC_PRECISION: u32 = 15;
/// LPC 予測次数の上限（1〜8）。
const MAX_LPC_ORDER: usize = 8;

/// WAV ファイルバイト列を受け取り、pak codec=2 用のバイト列 (wav-flac blob) を返す。
/// 対応外フォーマット、または変換結果が元サイズより縮まない場合は `None`。
pub fn try_encode(wav_bytes: &[u8]) -> Option<Vec<u8>> {
    let parsed = parse_wav_for_flac(wav_bytes)?;
    let frame_bytes = parsed.channels as usize * 2;
    if parsed.data_len % frame_bytes != 0 {
        return None; // 16bit フレーム境界に揃っていない（破損/非標準）
    }
    let num_frames = parsed.data_len / frame_bytes;

    let mut channel_samples: Vec<Vec<i16>> =
        vec![Vec::with_capacity(num_frames); parsed.channels as usize];
    let pcm = &wav_bytes[parsed.data_start..parsed.data_start + parsed.data_len];
    for (i, chunk) in pcm.as_chunks::<2>().0.iter().enumerate() {
        let ch = i % parsed.channels as usize;
        channel_samples[ch].push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }

    let mut writer = BitWriter::new();
    for samples in &channel_samples {
        encode_channel(&mut writer, samples);
    }
    let bitstream = writer.finish();

    let prefix = &wav_bytes[..parsed.data_start];
    let suffix = &wav_bytes[parsed.data_start + parsed.data_len..];

    let mut out = Vec::with_capacity(14 + prefix.len() + suffix.len() + bitstream.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&parsed.channels.to_le_bytes());
    out.extend_from_slice(&(num_frames as u32).to_le_bytes());
    out.extend_from_slice(&(prefix.len() as u32).to_le_bytes());
    out.extend_from_slice(prefix);
    out.extend_from_slice(&(suffix.len() as u32).to_le_bytes());
    out.extend_from_slice(suffix);
    out.extend_from_slice(&bitstream);

    if out.len() < wav_bytes.len() {
        Some(out)
    } else {
        None
    }
}

struct ParsedWav {
    channels: u16,
    data_start: usize,
    data_len: usize,
}

/// fmt/data チャンクの位置だけを最小限にスキャンする。`noiria_core::codec::wav` とは
/// **独立実装**（あちらは全チャンクを f32 サンプル列へデコードするが、こちらは
/// 「data ペイロードのバイト範囲」と「channels/bits」だけが要る、目的が異なる別の
/// パーサ）。fmt が data より後にあるファイルにも対応するため、両方見つかるまで
/// チャンク列全体を走査する。16bit PCM / 1-2ch 以外は `None`。
fn parse_wav_for_flac(data: &[u8]) -> Option<ParsedWav> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u16)> = None; // (format tag, channels, bits)
    let mut data_range: Option<(usize, usize)> = None;

    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let len = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().ok()?) as usize;
        let start = pos + 8;
        if start.checked_add(len)? > data.len() {
            return None;
        }
        match id {
            b"fmt " => {
                if len < 16 {
                    return None;
                }
                let chunk = &data[start..start + len];
                let format = u16::from_le_bytes([chunk[0], chunk[1]]);
                let channels = u16::from_le_bytes([chunk[2], chunk[3]]);
                let bits = u16::from_le_bytes([chunk[14], chunk[15]]);
                fmt = Some((format, channels, bits));
            }
            b"data" => {
                data_range = Some((start, len));
            }
            _ => {}
        }
        pos = start + len + (len & 1);
    }

    let (format, channels, bits) = fmt?;
    let (data_start, data_len) = data_range?;
    if format != 1 || bits != 16 || !(1..=2).contains(&channels) {
        return None;
    }
    Some(ParsedWav {
        channels,
        data_start,
        data_len,
    })
}

fn encode_channel(writer: &mut BitWriter, samples: &[i16]) {
    let mut i = 0;
    while i < samples.len() {
        let end = (i + BLOCK_SIZE).min(samples.len());
        encode_block(writer, &samples[i..end]);
        i = end;
    }
}

#[derive(Debug)]
enum Candidate {
    Verbatim,
    Fixed {
        order: usize,
        k: u32,
    },
    Lpc {
        order: usize,
        shift: u32,
        coeffs: Vec<i32>,
        k: u32,
    },
}

/// ブロックの内容から、実測ビット数が最小になるサブフレーム候補を選ぶ。
/// CONSTANT の高速パス以外は「候補を全部実際にコスト計算して比較する」総当たりで、
/// fixed 0-4 と LPC 1-8 の全次数を含む（`docs/implementation-notes.md` 2026-07-13 参照）。
/// テストから直接呼べるように `encode_block` から分離してある。
fn best_candidate(block: &[i16]) -> Candidate {
    let max_fixed_order = 4.min(block.len() - 1);
    let mut best_cost = 2 + 16 * block.len() as u64;
    let mut best = Candidate::Verbatim;

    for order in 0..=max_fixed_order {
        if let Some((cost, k)) = fixed_order_cost(block, order) {
            if cost < best_cost {
                best_cost = cost;
                best = Candidate::Fixed { order, k };
            }
        }
    }

    let max_lpc_order = MAX_LPC_ORDER.min(block.len().saturating_sub(1));
    if max_lpc_order >= 1 {
        let windowed = welch_windowed(block);
        let autoc = autocorrelate(&windowed, max_lpc_order);
        if autoc[0] > 0.0 {
            for (idx, coeffs_f64) in levinson_durbin(&autoc, max_lpc_order).iter().enumerate() {
                let order = idx + 1;
                if !coeffs_f64.iter().all(|c| c.is_finite()) {
                    continue; // 数値的に不安定な係数（極端な入力）は候補から除外
                }
                let (shift, coeffs) = quantize_lpc_coeffs(coeffs_f64, LPC_PRECISION);
                if let Some((cost, k)) = lpc_order_cost(block, order, &coeffs, shift) {
                    if cost < best_cost {
                        best_cost = cost;
                        best = Candidate::Lpc {
                            order,
                            shift,
                            coeffs,
                            k,
                        };
                    }
                }
            }
        }
    }

    best
}

fn encode_block(writer: &mut BitWriter, block: &[i16]) {
    if block.iter().all(|&s| s == block[0]) {
        writer.write_bits(SUBFRAME_CONSTANT, 2);
        writer.write_bits(block[0] as u16 as u64, 16);
        return;
    }

    match best_candidate(block) {
        Candidate::Verbatim => {
            writer.write_bits(SUBFRAME_VERBATIM, 2);
            for &s in block {
                writer.write_bits(s as u16 as u64, 16);
            }
        }
        Candidate::Fixed { order, k } => {
            writer.write_bits(SUBFRAME_FIXED, 2);
            writer.write_bits(order as u64, 3);
            writer.write_bits(k as u64, 5);
            for &s in &block[..order] {
                writer.write_bits(s as u16 as u64, 16);
            }
            for i in order..block.len() {
                let pred = fixed_predict(order, block, i);
                let residual = block[i] as i64 - pred;
                let zz = zigzag(residual);
                writer.write_rice(zz, k);
            }
        }
        Candidate::Lpc {
            order,
            shift,
            coeffs,
            k,
        } => {
            writer.write_bits(SUBFRAME_LPC, 2);
            writer.write_bits((order - 1) as u64, 3);
            writer.write_bits(LPC_PRECISION as u64, 4);
            writer.write_bits(shift as u64, 5);
            writer.write_bits(k as u64, 5);
            let mask = (1u64 << LPC_PRECISION) - 1;
            for &c in &coeffs {
                writer.write_bits((c as i64 as u64) & mask, LPC_PRECISION);
            }
            for &s in &block[..order] {
                writer.write_bits(s as u16 as u64, 16);
            }
            for i in order..block.len() {
                let pred = lpc_predict(&coeffs, shift, block, i);
                let residual = block[i] as i64 - pred;
                let zz = zigzag(residual);
                writer.write_rice(zz, k);
            }
        }
    }
}

/// LPC 用の窓関数（Welch 窓）を適用したサンプル列を f64 で返す。自己相関の計算だけに使い、
/// 実際に符号化する残差計算は非窓掛けの生サンプルで行う（窓はブロック端での予測誤差推定の
/// 精度を上げるための自己相関側だけの前処理、FLAC 本家と同様の一般的な手法）。
fn welch_windowed(block: &[i16]) -> Vec<f64> {
    let n = block.len();
    if n <= 1 {
        return block.iter().map(|&s| s as f64).collect();
    }
    let half = (n - 1) as f64 / 2.0;
    (0..n)
        .map(|i| {
            let x = (i as f64 - half) / half;
            block[i] as f64 * (1.0 - x * x)
        })
        .collect()
}

/// 窓掛け済みサンプルから lag 0..=max_lag の自己相関を計算する。
fn autocorrelate(windowed: &[f64], max_lag: usize) -> Vec<f64> {
    let n = windowed.len();
    let mut r = vec![0.0; max_lag + 1];
    for (lag, slot) in r.iter_mut().enumerate() {
        let mut sum = 0.0;
        for i in lag..n {
            sum += windowed[i] * windowed[i - lag];
        }
        *slot = sum;
    }
    r
}

/// Levinson-Durbin 再帰。自己相関 `autoc`（lag 0..=max_order）から、次数 1..=max_order
/// それぞれの LPC 係数（f64）を計算して返す（`result[order-1]` が長さ `order` の係数列）。
/// `pred(x[i]) = sum_j result[order-1][j] * x[i-1-j]` という規約（本ファイルの
/// `lpc_predict`/デコーダ側 `lpc_predict` と同じ規約）。
fn levinson_durbin(autoc: &[f64], max_order: usize) -> Vec<Vec<f64>> {
    let mut err = autoc[0];
    let mut lpc = vec![0.0f64; max_order];
    let mut results = Vec::with_capacity(max_order);
    for i in 0..max_order {
        let mut acc = autoc[i + 1];
        for j in 0..i {
            acc -= lpc[j] * autoc[i - j];
        }
        let k = if err.abs() > 1e-9 { acc / err } else { 0.0 };
        let mut new_lpc = lpc.clone();
        new_lpc[i] = k;
        for j in 0..i {
            new_lpc[j] = lpc[j] - k * lpc[i - 1 - j];
        }
        lpc = new_lpc;
        err *= 1.0 - k * k;
        if err <= 0.0 {
            err = 1e-9; // 浮動小数の誤差で非正になるのを防ぐガード
        }
        results.push(lpc[..=i].to_vec());
    }
    results
}

/// f64 の LPC 係数を「精度 precision bit + シフト量」方式で量子化する（FLAC 本家と同様）。
/// 誤差フィードバック付き丸め（各係数の丸め誤差を次の係数へ持ち越す）で量子化誤差の蓄積を
/// 抑える。戻り値は (shift, 量子化係数の配列)。
fn quantize_lpc_coeffs(coeffs: &[f64], precision: u32) -> (u32, Vec<i32>) {
    let cmax = coeffs.iter().fold(0.0f64, |a, &c| a.max(c.abs()));
    if cmax <= 0.0 {
        return (0, vec![0i32; coeffs.len()]);
    }
    let max_val = (1i64 << (precision - 1)) - 1;
    let min_val = -(1i64 << (precision - 1));

    let mut shift = 0u32;
    while shift < 31 && cmax * (1u64 << (shift + 1)) as f64 <= max_val as f64 {
        shift += 1;
    }

    let mut error = 0.0f64;
    let mut quantized = Vec::with_capacity(coeffs.len());
    for &c in coeffs {
        let scaled = c * (1i64 << shift) as f64 + error;
        let v = (scaled.round() as i64).clamp(min_val, max_val);
        error = scaled - v as f64;
        quantized.push(v as i32);
    }
    (shift, quantized)
}

/// `order` 次 LPC 予測を適用した際の総コスト（bit 数、ヘッダ込み）と Rice パラメータ k を
/// 返す。`fixed_order_cost` と同じ k 推定式・外れ値安全弁を使う（独立した関数として個別に
/// 実装しており、共有はしていない）。
fn lpc_order_cost(block: &[i16], order: usize, coeffs: &[i32], shift: u32) -> Option<(u64, u32)> {
    let n = block.len() - order;
    if n == 0 {
        return None;
    }
    let mut zz = Vec::with_capacity(n);
    let mut sum_z: u64 = 0;
    for i in order..block.len() {
        let pred = lpc_predict(coeffs, shift, block, i);
        let residual = block[i] as i64 - pred;
        let z = zigzag(residual);
        sum_z += z;
        zz.push(z);
    }

    let mut k = 0u32;
    while k < 30 && ((n as u64) << k) < sum_z {
        k += 1;
    }

    const MAX_QUOTIENT: u64 = 1 << 20;
    let mut payload_bits: u64 = 0;
    for &z in &zz {
        let q = z >> k;
        if q > MAX_QUOTIENT {
            return None;
        }
        payload_bits += q + 1 + k as u64;
    }
    let header_bits =
        2 + 3 + 4 + 5 + 5 + (order as u64) * LPC_PRECISION as u64 + (order as u64) * 16;
    Some((header_bits + payload_bits, k))
}

/// `pred = (sum_{j=0}^{order-1} coeff[j] * x[i-1-j]) >> shift` を i64 の整数演算のみで計算する。
/// デコーダ側 (`noiria_core::codec::flac_subset::lpc_predict`) とは独立に実装しているが、
/// 演算の仕様（積和の順序・シフトの方向）はビットストリームの意味として一致させている。
fn lpc_predict(coeffs: &[i32], shift: u32, block: &[i16], i: usize) -> i64 {
    let mut acc: i64 = 0;
    for (j, &c) in coeffs.iter().enumerate() {
        acc += c as i64 * block[i - 1 - j] as i64;
    }
    acc >> shift
}

/// order 次固定予測を適用した際の総コスト（bit 数、ヘッダ込み）と採用する Rice
/// パラメータ k を返す。unary 符号が病的に長くなる外れ値がある場合は候補から除外する
/// （`None`）— この場合 `encode_block` は他の候補（最終的には常に valid な VERBATIM）
/// を採用するので、エンコード結果が無限に膨らむことはない。
fn fixed_order_cost(block: &[i16], order: usize) -> Option<(u64, u32)> {
    let n = block.len() - order;
    if n == 0 {
        return None;
    }
    let mut zz = Vec::with_capacity(n);
    let mut sum_z: u64 = 0;
    for i in order..block.len() {
        let pred = fixed_predict(order, block, i);
        let residual = block[i] as i64 - pred;
        let z = zigzag(residual);
        sum_z += z;
        zz.push(z);
    }

    // k の推定: 平均 quotient がおよそ 1 未満になる最小の k（`n << k >= sum_z`）。
    // 全探索よりずっと安く、実用上十分近い k が得られる（`docs/implementation-notes.md`
    // 2026-07-12 配布対応 W9 参照: パーティション無しの単一 k という簡素な設計の一部）。
    let mut k = 0u32;
    while k < 30 && ((n as u64) << k) < sum_z {
        k += 1;
    }

    const MAX_QUOTIENT: u64 = 1 << 20; // 病的な外れ値に対する安全弁（unary 符号の暴走防止）
    let mut payload_bits: u64 = 0;
    for &z in &zz {
        let q = z >> k;
        if q > MAX_QUOTIENT {
            return None;
        }
        payload_bits += q + 1 + k as u64;
    }
    let header_bits = 2 + 3 + 5 + (order as u64) * 16;
    Some((header_bits + payload_bits, k))
}

fn fixed_predict(order: usize, block: &[i16], i: usize) -> i64 {
    let x = |d: usize| block[i - d] as i64;
    match order {
        0 => 0,
        1 => x(1),
        2 => 2 * x(1) - x(2),
        3 => 3 * x(1) - 3 * x(2) + x(3),
        4 => 4 * x(1) - 6 * x(2) + 4 * x(3) - x(4),
        _ => unreachable!("order は呼び出し側で 0..=4 に制限している"),
    }
}

fn zigzag(r: i64) -> u64 {
    ((r << 1) ^ (r >> 63)) as u64
}

/// MSB-first ビットライター。
struct BitWriter {
    bytes: Vec<u8>,
    cur: u8,
    nbits: u32, // cur に溜まっているビット数 (0..8)
}

impl BitWriter {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            cur: 0,
            nbits: 0,
        }
    }

    fn write_bit(&mut self, bit: u64) {
        self.cur = (self.cur << 1) | (bit as u8 & 1);
        self.nbits += 1;
        if self.nbits == 8 {
            self.bytes.push(self.cur);
            self.cur = 0;
            self.nbits = 0;
        }
    }

    fn write_bits(&mut self, value: u64, n: u32) {
        for i in (0..n).rev() {
            self.write_bit((value >> i) & 1);
        }
    }

    fn write_unary(&mut self, q: u64) {
        for _ in 0..q {
            self.write_bit(0);
        }
        self.write_bit(1);
    }

    fn write_rice(&mut self, zz: u64, k: u32) {
        self.write_unary(zz >> k);
        if k > 0 {
            self.write_bits(zz & ((1u64 << k) - 1), k);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.cur <<= 8 - self.nbits;
            self.bytes.push(self.cur);
        }
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn rejects_non_wav() {
        assert!(try_encode(b"not a wav at all, just some bytes").is_none());
    }

    #[test]
    fn rejects_8bit_and_24bit() {
        let body8 = vec![128u8; 1000];
        assert!(try_encode(&build_wav(1, 44100, 8, &body8)).is_none());

        let mut body24 = Vec::new();
        for _ in 0..500 {
            body24.extend_from_slice(&[0, 0, 1]);
        }
        assert!(try_encode(&build_wav(1, 44100, 24, &body24)).is_none());
    }

    #[test]
    fn rejects_odd_length_data() {
        // 16bit mono なのに data 長が奇数（フレーム境界に揃っていない、破損/非標準ケース）。
        let body = vec![0u8; 1001];
        assert!(try_encode(&build_wav(1, 44100, 16, &body)).is_none());
    }

    #[test]
    fn compresses_constant_silence() {
        let body = vec![0u8; 44100 * 2 * 4]; // 4秒ぶんの無音、16bit mono
        let wav = build_wav(1, 44100, 16, &body);
        let blob = try_encode(&wav).expect("無音は確実に縮むはず");
        assert!(
            blob.len() < wav.len() / 10,
            "無音の圧縮率が低すぎる: {} -> {}",
            wav.len(),
            blob.len()
        );
    }

    #[test]
    fn does_not_shrink_returns_none_for_tiny_input() {
        // 数サンプルだけの極短 PCM はコンテナヘッダのオーバーヘッドで縮まないことがある。
        // その場合 None を返し、呼び出し側のフォールバックに任せるのが正しい。
        let body = [1i16, -1, 2]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect::<Vec<_>>();
        let wav = build_wav(1, 44100, 16, &body);
        // 縮む/縮まないどちらもあり得るが、Some を返す場合は必ず元より小さいことだけ検証する
        // （実際の byte-exact round-trip は noiria-tools/tests/flac_subset_roundtrip.rs 側）。
        if let Some(blob) = try_encode(&wav) {
            assert!(blob.len() < wav.len());
        }
    }

    #[test]
    fn lpc_predict_matches_hand_computed_value() {
        // block=[10,3,99], i=2: history は block[1]=3, block[0]=10（block[2]自体は使わない）。
        // coeffs=[2,-1], shift=0 -> pred = 2*3 + (-1)*10 = -4
        assert_eq!(lpc_predict(&[2, -1], 0, &[10, 3, 99], 2), -4);
    }

    #[test]
    fn quantize_lpc_coeffs_round_trips_approximately() {
        let coeffs = [1.9, -0.95];
        let (shift, q) = quantize_lpc_coeffs(&coeffs, LPC_PRECISION);
        for (orig, &qi) in coeffs.iter().zip(q.iter()) {
            let dequantized = qi as f64 / (1i64 << shift) as f64;
            assert!(
                (dequantized - orig).abs() < 0.01,
                "量子化誤差が大きすぎる: {orig} -> {dequantized} (shift={shift})"
            );
        }
    }

    #[test]
    fn quantize_lpc_coeffs_handles_all_zero_input() {
        let (shift, q) = quantize_lpc_coeffs(&[0.0, 0.0, 0.0], LPC_PRECISION);
        assert_eq!(shift, 0);
        assert_eq!(q, vec![0, 0, 0]);
    }

    #[test]
    fn levinson_durbin_recovers_known_ar1_coefficients() {
        // 完全な AR(1) 過程（r[lag] = r0 * a^lag）の自己相関を人工的に与えると、
        // 次数1の係数は a に一致し、次数2以降の追加係数はほぼ0になるはず
        // （AR(1) には2次以降の予測情報が無いため）。
        let a: f64 = 0.5;
        let autoc: Vec<f64> = (0..=4).map(|lag| a.powi(lag)).collect();
        let results = levinson_durbin(&autoc, 3);
        assert!(
            (results[0][0] - a).abs() < 1e-6,
            "次数1係数: {:?}",
            results[0]
        );
        assert!(
            results[1][1].abs() < 1e-6,
            "次数2の追加係数はほぼ0のはず: {:?}",
            results[1]
        );
        assert!(
            results[2][2].abs() < 1e-6,
            "次数3の追加係数はほぼ0のはず: {:?}",
            results[2]
        );
    }

    #[test]
    fn high_order_correlated_signal_picks_lpc_over_fixed() {
        // シンセ BGM を模した滑らかな複合正弦波（低周波数の合成）。fixed predictor
        // （差分予測）よりも LPC の方が確実に有利になるはずの、強く相関した信号。
        let n = 3000;
        let block: Vec<i16> = (0..n)
            .map(|i| {
                let t = i as f64;
                (5000.0 * (0.017 * t).sin() + 3000.0 * (0.0031 * t).sin()) as i16
            })
            .collect();
        match best_candidate(&block) {
            Candidate::Lpc { order, .. } => {
                assert!((1..=8).contains(&order), "LPC次数が範囲外: {order}");
            }
            other => panic!("高相関信号なのに LPC が選ばれなかった: {other:?}"),
        }
    }
}
