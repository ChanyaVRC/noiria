//! FLAC 風サブセットコーデックの**デコード側**（配布対応 W9、pak codec=2 "wav-flac"）。
//!
//! ## 位置づけ
//! pak 内の WAV アセットを可逆圧縮する独自コーデック。**本家 FLAC とのビットストリーム
//! 互換は無い**（pak 内部専用の独自フォーマット。`docs/implementation-notes.md`
//! 2026-07-12 配布対応 W9 参照）。
//!
//! エンコーダ (`noiria-tools/src/flac_enc.rs`) とは**意図的に別クレートで完全に独立実装**
//! している。`noiria-core` は `noiria-tools` に依存できない（依存方向が逆）ため、この分離は
//! ビルドグラフによっても強制される。関数・ヘルパーの共有は一切なく、ビット詰め方向・
//! ジグザグ符号化・fixed predictor 係数といった仕様はこのファイルとエンコーダ側の両方に
//! それぞれ独立して記述してある（W4 の教訓: 対称な実装は round-trip のバグを隠す）。
//! このファイルは decode のみを提供し、encode ロジックは一切含まない。
//!
//! ## コンテナ形式（マジック "NFC1"）
//! ```text
//! [4]  magic = b"NFC1"
//! [2]  channels (u16 LE, 1 or 2)
//! [4]  num_frames (u32 LE, サンプルフレーム数 = 1 サンプル/チャンネルのセット数)
//! [4]  prefix_len (u32 LE)
//! [prefix_len]  prefix バイト列（元 WAV ファイルの先頭から data チャンクのペイロード
//!               直前まで — data チャンクの id+len ヘッダ 8 バイトを含む — 生バイトそのまま）
//! [4]  suffix_len (u32 LE)
//! [suffix_len]  suffix バイト列（data ペイロード直後からファイル末尾まで、生バイトそのまま）
//! [残り]  ビットストリーム（バイト境界から開始）: channel 0 の全ブロック →
//!         （2ch の場合）channel 1 の全ブロック
//! ```
//! `decode_wav` はこのバイト列だけから元 WAV ファイルの**完全なバイト列**を復元する
//! （prefix + 復元 PCM (LE 16bit interleaved) + suffix を単純に連結するだけ。
//! WAV のチャンク構造そのものは一切解釈しない — 誤解釈によるバグの余地をゼロにする設計）。
//!
//! ## チャンネルのビットストリーム
//! `BLOCK_SIZE = 4096` サンプル固定のブロックに分割する（最終ブロックのみ端数）。
//! 各ブロックは以下のいずれかのサブフレームとして符号化される:
//! ```text
//! [2 bits] サブフレーム種別: 0=CONSTANT, 1=VERBATIM, 2=FIXED, 3=LPC
//! CONSTANT: [16 bits] signed サンプル値 1 個（ブロック全体がこの値の繰り返し）
//! VERBATIM: ブロック内サンプル数ぶんの [16 bits] signed 生サンプル
//! FIXED:    [3 bits] 予測次数 (0-4) + [5 bits] Rice パラメータ k (0-30)
//!           + 次数ぶんの [16 bits] warmup 生サンプル
//!           + (ブロックサンプル数-次数) 個の Rice 符号（ジグザグ符号化した残差）
//! LPC:      [3 bits] 予測次数-1 (0-7 → 次数 1-8)
//!           + [4 bits] 係数精度 precision (1-15 bit幅)
//!           + [5 bits] シフト量 shift (0-31)
//!           + [5 bits] Rice パラメータ k (0-30)
//!           + 次数ぶんの [precision bits] signed 量子化係数（2の補数、MSB-first）
//!           + 次数ぶんの [16 bits] warmup 生サンプル
//!           + (ブロックサンプル数-次数) 個の Rice 符号（ジグザグ符号化した残差）
//! ```
//! Rice 符号 = unary(quotient)（'0'×quotient + 終端 '1'） + k bits の remainder（k>0 の場合）。
//! fixed/LPC predictor は各ブロックの先頭（warmup 分）でリセットされ、ブロックをまたいだ履歴は
//! 参照しない（エンコーダ側もブロックごとに独立して残差を計算する設計と対応している）。
//! LPC 予測は `pred = (sum_{j=0}^{order-1} coeff[j] * x[i-1-j]) >> shift` を**整数演算のみ**
//! （i64 の積和 + 算術右シフト）で計算する。係数は量子化済みの整数をビットストリームへ
//! そのまま格納するため、デコーダは浮動小数点を一切使わず、エンコーダが計算した通りの
//! 整数演算を再現するだけで済む（丸め誤差やプラットフォーム差が結果に影響しない設計。
//! `docs/implementation-notes.md` 2026-07-13 wav-flac LPC 拡張 参照）。
//! 旧フォーマット（fixed のみ、サブフレーム種別 0-2 のみを使う W9 時点の pak）は
//! サブフレーム種別フィールドの空きコード（旧実装が出力しなかった 3）を LPC に割り当てて
//! いるため、コンテナのバージョンフィールドを増やさずにそのまま後方互換になる
//! （旧 pak は 3 を含まないので新デコーダでも従来通りに読める）。
//!
//! ## ステレオの扱い（L/R 独立、mid-side は不採用）
//! channels==2 の場合も L/R チャンネルを完全に独立して符号化する。mid-side
//! チャンネル間デコリレーションを試作・実測したが、`games/sample` の実アセット（手続き
//! 生成シンセの BGM、チャンネル間相関が低い意図的なステレオ幅付けと推測される）では
//! 圧縮率が全く改善しなかった（実測: 独立 L/R のみ 42.2% 削減、mid-side 込み 42.2% 削減で
//! ほぼ誤差の範囲）ため、mid-side 特有の複雑さ（side チャンネルが 17bit 幅になり生値の
//! ビット幅を可変にする必要がある等）を正当化する実利が確認できず、シンプルな独立実装に
//! 留めた（`docs/implementation-notes.md` 2026-07-12 配布対応 W9 参照。CLAUDE.md
//! 「簡潔さを優先」との整合）。

use anyhow::{bail, ensure, Result};

const MAGIC: &[u8; 4] = b"NFC1";
const BLOCK_SIZE: usize = 4096;

const SUBFRAME_CONSTANT: u64 = 0;
const SUBFRAME_VERBATIM: u64 = 1;
const SUBFRAME_FIXED: u64 = 2;
const SUBFRAME_LPC: u64 = 3;

/// 1 ブロックが最低限必要とするビット数（種別 2bit + CONSTANT の値 16bit）。これで
/// 申告された `num_frames` がビットストリームの長さと矛盾していないかを確認する。
const MIN_BLOCK_BITS: u64 = 18;

/// pak codec=2 (wav-flac) のバイト列から、元の WAV ファイルバイト列を完全に復元する。
pub fn decode_wav(blob: &[u8]) -> Result<Vec<u8>> {
    decode_wav_limited(blob, usize::MAX)
}

/// 復元する WAV のサイズが `max_out` バイトを超えるなら、確保・展開の前に `Err` にする
/// [`decode_wav`]。pak のようにインデックスが展開後サイズを知っている呼び出し側が使う。
pub fn decode_wav_limited(blob: &[u8], max_out: usize) -> Result<Vec<u8>> {
    ensure!(
        blob.len() >= 10,
        "wav-flac ブロブが短すぎます（ヘッダ不足）"
    );
    ensure!(&blob[0..4] == MAGIC, "wav-flac マジックバイトが不一致です");
    let channels = u16::from_le_bytes([blob[4], blob[5]]) as usize;
    ensure!(
        (1..=2).contains(&channels),
        "wav-flac の channels が不正です: {channels}"
    );
    let num_frames = u32::from_le_bytes([blob[6], blob[7], blob[8], blob[9]]) as usize;

    let mut pos = 10usize;
    let prefix_len = read_u32(blob, &mut pos)? as usize;
    ensure!(
        pos + prefix_len <= blob.len(),
        "wav-flac の prefix がブロブ範囲外です"
    );
    let prefix = &blob[pos..pos + prefix_len];
    pos += prefix_len;

    let suffix_len = read_u32(blob, &mut pos)? as usize;
    ensure!(
        pos + suffix_len <= blob.len(),
        "wav-flac の suffix がブロブ範囲外です"
    );
    let suffix = &blob[pos..pos + suffix_len];
    pos += suffix_len;

    let stream = &blob[pos..];
    let blocks = num_frames.div_ceil(BLOCK_SIZE) as u64 * channels as u64;
    ensure!(
        blocks * MIN_BLOCK_BITS <= stream.len() as u64 * 8,
        "wav-flac のフレーム数 {num_frames} がビットストリームの長さと矛盾しています"
    );
    let out_len = (prefix.len() as u64)
        .saturating_add(suffix.len() as u64)
        .saturating_add(num_frames as u64 * channels as u64 * 2);
    ensure!(
        out_len <= max_out as u64,
        "wav-flac の復元サイズ {out_len} が上限 {max_out} を超えます"
    );

    let mut reader = BitReader::new(stream);
    let mut channel_samples: Vec<Vec<i16>> = Vec::with_capacity(channels);
    for _ in 0..channels {
        channel_samples.push(decode_channel(&mut reader, num_frames)?);
    }

    let mut out = Vec::with_capacity(out_len as usize);
    out.extend_from_slice(prefix);
    for i in 0..num_frames {
        for samples in &channel_samples {
            out.extend_from_slice(&samples[i].to_le_bytes());
        }
    }
    out.extend_from_slice(suffix);
    Ok(out)
}

fn read_u32(data: &[u8], pos: &mut usize) -> Result<u32> {
    ensure!(
        *pos + 4 <= data.len(),
        "wav-flac ヘッダが壊れています（範囲外読み取り）"
    );
    let b = &data[*pos..*pos + 4];
    let v = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    *pos += 4;
    Ok(v)
}

fn decode_channel(reader: &mut BitReader, num_frames: usize) -> Result<Vec<i16>> {
    let mut out = Vec::with_capacity(num_frames);
    let mut remaining = num_frames;
    while remaining > 0 {
        let block_len = remaining.min(BLOCK_SIZE);
        decode_block(reader, block_len, &mut out)?;
        remaining -= block_len;
    }
    Ok(out)
}

fn decode_block(reader: &mut BitReader, block_len: usize, out: &mut Vec<i16>) -> Result<()> {
    let kind = reader.read_bits(2)?;
    match kind {
        SUBFRAME_CONSTANT => {
            let v = reader.read_bits(16)? as u16 as i16;
            out.extend(std::iter::repeat_n(v, block_len));
        }
        SUBFRAME_VERBATIM => {
            for _ in 0..block_len {
                let v = reader.read_bits(16)? as u16 as i16;
                out.push(v);
            }
        }
        SUBFRAME_FIXED => {
            let order = reader.read_bits(3)? as usize;
            ensure!(order <= 4, "wav-flac の予測次数が不正です: {order}");
            let k = reader.read_bits(5)? as u32;
            ensure!(k <= 30, "wav-flac の Rice パラメータが不正です: {k}");
            ensure!(
                order <= block_len,
                "wav-flac の予測次数がブロック長を超えています"
            );

            for _ in 0..order {
                let v = reader.read_bits(16)? as u16 as i16;
                out.push(v);
            }
            for _ in order..block_len {
                let zz = reader.read_rice(k)?;
                let residual = unzigzag(zz);
                let pred = fixed_predict(order, out);
                let x = residual.wrapping_add(pred);
                ensure!(
                    (i16::MIN as i64..=i16::MAX as i64).contains(&x),
                    "wav-flac の復元サンプルが16bit範囲外です（壊れたデータの可能性）"
                );
                out.push(x as i16);
            }
        }
        SUBFRAME_LPC => {
            let order = reader.read_bits(3)? as usize + 1; // 3bit 符号は次数-1 (0-7 -> 次数 1-8)
            let precision = reader.read_bits(4)? as u32;
            ensure!(
                (1..=15).contains(&precision),
                "wav-flac のLPC係数精度が不正です: {precision}"
            );
            let shift = reader.read_bits(5)? as u32; // 0..=31
            let k = reader.read_bits(5)? as u32;
            ensure!(k <= 30, "wav-flac の Rice パラメータが不正です: {k}");
            ensure!(
                order <= block_len,
                "wav-flac のLPC予測次数がブロック長を超えています"
            );

            let mut coeffs = Vec::with_capacity(order);
            for _ in 0..order {
                let raw = reader.read_bits(precision)?;
                coeffs.push(sign_extend(raw, precision));
            }
            for _ in 0..order {
                let v = reader.read_bits(16)? as u16 as i16;
                out.push(v);
            }
            for _ in order..block_len {
                let zz = reader.read_rice(k)?;
                let residual = unzigzag(zz);
                let pred = lpc_predict(&coeffs, shift, out);
                let x = residual.wrapping_add(pred);
                ensure!(
                    (i16::MIN as i64..=i16::MAX as i64).contains(&x),
                    "wav-flac の復元サンプルが16bit範囲外です（壊れたデータの可能性）"
                );
                out.push(x as i16);
            }
        }
        other => bail!("wav-flac の未知サブフレーム種別です: {other}"),
    }
    Ok(())
}

/// `raw`（下位 `bits` ビットに値が入っている）を 2 の補数として符号拡張する。
/// `bits` は 1..=63 を想定（本コーデックの precision フィールドは最大 15）。
fn sign_extend(raw: u64, bits: u32) -> i64 {
    let shift = 64 - bits;
    ((raw << shift) as i64) >> shift
}

/// `out` の末尾側（直近に復元したサンプル）から LPC 予測値を計算する。
/// `pred = (sum_{j=0}^{order-1} coeff[j] * x[i-1-j]) >> shift` を i64 の整数演算のみで行う
/// （エンコーダ側と完全に独立に書かれた実装だが、この整数演算の仕様自体は一致させる必要が
/// ある。浮動小数点は一切使わない — `docs/implementation-notes.md` 2026-07-13 参照）。
fn lpc_predict(coeffs: &[i64], shift: u32, out: &[i16]) -> i64 {
    let n = out.len();
    let mut acc: i64 = 0;
    for (j, &c) in coeffs.iter().enumerate() {
        acc += c * out[n - 1 - j] as i64;
    }
    acc >> shift
}

/// `out` の末尾側（直近に復元したサンプル）から fixed predictor の予測値を計算する。
/// ブロック先頭の warmup サンプルは呼び出し前に既に `out` へ push 済みという前提で、
/// `order` 個の履歴を `out` の末尾から読む（ブロックをまたいだ履歴は参照しない設計だが、
/// warmup が直前に push されているため `out` 全体の末尾を見るだけで自然にブロック内で閉じる）。
fn fixed_predict(order: usize, out: &[i16]) -> i64 {
    let n = out.len();
    let x = |d: usize| out[n - d] as i64;
    match order {
        0 => 0,
        1 => x(1),
        2 => 2 * x(1) - x(2),
        3 => 3 * x(1) - 3 * x(2) + x(3),
        4 => 4 * x(1) - 6 * x(2) + 4 * x(3) - x(4),
        _ => unreachable!("order は呼び出し側で 0..=4 の範囲を検証済み"),
    }
}

fn unzigzag(zz: u64) -> i64 {
    ((zz >> 1) as i64) ^ -((zz & 1) as i64)
}

/// MSB-first ビットリーダー。
struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_pos: u32, // 0..8、次に読む bit のインデックス（MSB=0）
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    fn read_bit(&mut self) -> Result<u64> {
        ensure!(
            self.byte_pos < self.data.len(),
            "wav-flac ビットストリームの範囲外読み取りです（壊れたデータの可能性）"
        );
        let byte = self.data[self.byte_pos];
        let bit = (byte >> (7 - self.bit_pos)) & 1;
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        Ok(bit as u64)
    }

    fn read_bits(&mut self, n: u32) -> Result<u64> {
        let mut v = 0u64;
        for _ in 0..n {
            v = (v << 1) | self.read_bit()?;
        }
        Ok(v)
    }

    fn read_unary(&mut self) -> Result<u64> {
        let mut q = 0u64;
        loop {
            if self.read_bit()? == 1 {
                return Ok(q);
            }
            q += 1;
            ensure!(
                q < 1_000_000_000,
                "wav-flac の unary 符号が異常に長すぎます（壊れたデータの可能性）"
            );
        }
    }

    fn read_rice(&mut self, k: u32) -> Result<u64> {
        let q = self.read_unary()?;
        let r = if k > 0 { self.read_bits(k)? } else { 0 };
        Ok((q << k) | r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_magic() {
        let mut blob = vec![0u8; 20];
        blob[0..4].copy_from_slice(b"XXXX");
        assert!(decode_wav(&blob).is_err());
    }

    #[test]
    fn rejects_too_short() {
        assert!(decode_wav(&[0u8; 4]).is_err());
    }

    #[test]
    fn rejects_bad_channel_count() {
        let mut blob = vec![0u8; 10];
        blob[0..4].copy_from_slice(MAGIC);
        blob[4..6].copy_from_slice(&3u16.to_le_bytes());
        assert!(decode_wav(&blob).is_err());
    }

    #[test]
    fn bit_reader_round_trips_written_pattern() {
        // BitReader だけを単独で検証: 手組みしたビット列を期待通り読めること。
        // 0b101_00110 -> 3bits=0b101(5), 5bits=0b00110(6)
        let byte = 0b101_00110u8;
        let bytes = [byte];
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
        assert_eq!(r.read_bits(5).unwrap(), 0b00110);
    }

    #[test]
    fn unzigzag_matches_known_values() {
        assert_eq!(unzigzag(0), 0);
        assert_eq!(unzigzag(1), -1);
        assert_eq!(unzigzag(2), 1);
        assert_eq!(unzigzag(3), -2);
        assert_eq!(unzigzag(4), 2);
    }

    #[test]
    fn sign_extend_matches_known_values() {
        // 2bit 幅: 0b01 = 1, 0b11 = -1, 0b10 = -2
        assert_eq!(sign_extend(0b01, 2), 1);
        assert_eq!(sign_extend(0b11, 2), -1);
        assert_eq!(sign_extend(0b10, 2), -2);
        // 4bit 幅: 0b0111 = 7, 0b1000 = -8
        assert_eq!(sign_extend(0b0111, 4), 7);
        assert_eq!(sign_extend(0b1000, 4), -8);
    }

    #[test]
    fn lpc_predict_uses_integer_shift_only() {
        // history = [8] (x[i-1]=8), coeff=[1], shift=1 -> pred = (1*8)>>1 = 4
        assert_eq!(lpc_predict(&[1], 1, &[8]), 4);
        // history 末尾から order 個: coeff=[2,-1], shift=0, out=[..,10,3] -> x[i-1]=3, x[i-2]=10
        // pred = 2*3 + (-1)*10 = -4
        assert_eq!(lpc_predict(&[2, -1], 0, &[10, 3]), -4);
    }

    #[test]
    fn decode_constant_block_all_same_value() {
        // 手組み: channels=1, num_frames=5, prefix/suffix 空、1 ブロック CONSTANT=-100
        let mut blob = Vec::new();
        blob.extend_from_slice(MAGIC);
        blob.extend_from_slice(&1u16.to_le_bytes());
        blob.extend_from_slice(&5u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes()); // prefix_len
        blob.extend_from_slice(&0u32.to_le_bytes()); // suffix_len

        let mut w = BitWriterForTest::new();
        w.write_bits(SUBFRAME_CONSTANT, 2);
        w.write_bits((-100i16) as u16 as u64, 16);
        blob.extend_from_slice(&w.finish());

        let wav = decode_wav(&blob).unwrap();
        assert_eq!(wav.len(), 5 * 2);
        for chunk in wav.as_chunks::<2>().0 {
            assert_eq!(i16::from_le_bytes([chunk[0], chunk[1]]), -100);
        }
    }

    #[test]
    fn decode_lpc_block_matches_manual_prediction() {
        // 手組み: channels=1, num_frames=4, order=1, precision=2bit, shift=1, coeff=+1, k=0。
        // 期待系列: x0=8(warmup), x1=5(pred=4,resid=+1), x2=2(pred=2,resid=0), x3=1(pred=1,resid=0)
        // ここでの pred は前サンプルの整数演算 (coeff*x)>>shift だけで手計算した値
        // （エンコーダとは無関係、デコーダの整数演算だけを直接検証する）。
        let mut blob = Vec::new();
        blob.extend_from_slice(MAGIC);
        blob.extend_from_slice(&1u16.to_le_bytes());
        blob.extend_from_slice(&4u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes()); // prefix_len
        blob.extend_from_slice(&0u32.to_le_bytes()); // suffix_len

        let mut w = BitWriterForTest::new();
        w.write_bits(SUBFRAME_LPC, 2);
        w.write_bits(0, 3); // order_code = 0 -> order = 1
        w.write_bits(2, 4); // precision = 2
        w.write_bits(1, 5); // shift = 1
        w.write_bits(0, 5); // k = 0
        w.write_bits(0b01, 2); // coeff[0] = +1 (2の補数2bit)
        w.write_bits(8u16 as u64, 16); // warmup x0 = 8
        w.write_bits(0b001u64, 3); // residual1: zigzag(+1)=2 -> unary(2)='001'
        w.write_bits(0b1u64, 1); // residual2: zigzag(0)=0 -> unary(0)='1'
        w.write_bits(0b1u64, 1); // residual3: zigzag(0)=0 -> unary(0)='1'
        blob.extend_from_slice(&w.finish());

        let wav = decode_wav(&blob).unwrap();
        let samples: Vec<i16> = wav
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(samples, vec![8, 5, 2, 1]);
    }

    #[test]
    fn decode_lpc_block_handles_negative_coefficient() {
        // order=1, precision=2bit, shift=0, coeff=-1 (2の補数 '11')、k=0。
        // x0=5(warmup), pred=(-1*5)>>0=-5、residual=0 -> x1=-5（負係数の符号拡張を検証）。
        let mut blob = Vec::new();
        blob.extend_from_slice(MAGIC);
        blob.extend_from_slice(&1u16.to_le_bytes());
        blob.extend_from_slice(&2u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());

        let mut w = BitWriterForTest::new();
        w.write_bits(SUBFRAME_LPC, 2);
        w.write_bits(0, 3); // order = 1
        w.write_bits(2, 4); // precision = 2
        w.write_bits(0, 5); // shift = 0
        w.write_bits(0, 5); // k = 0
        w.write_bits(0b11, 2); // coeff[0] = -1
        w.write_bits(5u16 as u64, 16); // warmup x0 = 5
        w.write_bits(0b1u64, 1); // residual: zigzag(0)=0 -> unary(0)='1'
        blob.extend_from_slice(&w.finish());

        let wav = decode_wav(&blob).unwrap();
        let samples: Vec<i16> = wav
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(samples, vec![5, -5]);
    }

    /// テスト専用の最小ビットライター（本体の `BitReader` とは無関係の独立実装。
    /// デコーダ単体テストで期待バイト列を手組みするためだけに使う）。
    struct BitWriterForTest {
        bytes: Vec<u8>,
        cur: u8,
        nbits: u32,
    }
    impl BitWriterForTest {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                cur: 0,
                nbits: 0,
            }
        }
        fn write_bits(&mut self, value: u64, n: u32) {
            for i in (0..n).rev() {
                let bit = ((value >> i) & 1) as u8;
                self.cur = (self.cur << 1) | bit;
                self.nbits += 1;
                if self.nbits == 8 {
                    self.bytes.push(self.cur);
                    self.cur = 0;
                    self.nbits = 0;
                }
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
    // --- 外部入力の堅牢化（R1） ---

    #[test]
    fn declared_frame_count_beyond_the_stream_is_an_error_without_preallocating() {
        // num_frames = u32::MAX（チャンネルあたり 8GB 相当）を名乗るが、ビットストリームは 16 バイト。
        let mut blob = Vec::new();
        blob.extend_from_slice(MAGIC);
        blob.extend_from_slice(&2u16.to_le_bytes());
        blob.extend_from_slice(&u32::MAX.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&[0u8; 16]);
        let r = crate::test_support::with_alloc_limit(1 << 20, || decode_wav(&blob));
        assert!(r.is_err());
    }
    /// CONSTANT / VERBATIM / FIXED / LPC の 4 種を 1 チャンネルに並べた blob（4096 x 3 + 50 フレーム）。
    fn mixed_blob() -> Vec<u8> {
        let mut blob = Vec::new();
        blob.extend_from_slice(MAGIC);
        blob.extend_from_slice(&1u16.to_le_bytes());
        blob.extend_from_slice(&(3 * 4096u32 + 50).to_le_bytes());
        blob.extend_from_slice(&4u32.to_le_bytes());
        blob.extend_from_slice(b"PFX!");
        blob.extend_from_slice(&0u32.to_le_bytes());

        let mut w = BitWriterForTest::new();
        w.write_bits(SUBFRAME_CONSTANT, 2);
        w.write_bits(1234, 16);
        w.write_bits(SUBFRAME_VERBATIM, 2);
        for i in 0..4096u64 {
            w.write_bits((i * 7) & 0x3FF, 16);
        }
        w.write_bits(SUBFRAME_FIXED, 2);
        w.write_bits(2, 3); // order
        w.write_bits(2, 5); // k
        w.write_bits(100, 16);
        w.write_bits(101, 16);
        for _ in 2..4096 {
            w.write_bits(0b1, 1); // unary 0
            w.write_bits(0b00, 2); // remainder 0 -> 残差 0（2 次予測で線形に増える）
        }
        w.write_bits(SUBFRAME_LPC, 2);
        w.write_bits(1, 3); // order 2
        w.write_bits(4, 4); // precision
        w.write_bits(2, 5); // shift
        w.write_bits(1, 5); // k
        w.write_bits(0b0100, 4); // coeff[0] = 4（shift 2 で x[i-1] そのもの）
        w.write_bits(0b0000, 4); // coeff[1] = 0
        w.write_bits(10, 16);
        w.write_bits(12, 16);
        for _ in 2..50 {
            w.write_bits(1, 1); // unary 0
            w.write_bits(0, 1); // remainder 0
        }
        blob.extend_from_slice(&w.finish());
        blob
    }

    #[test]
    fn mixed_blob_decodes() {
        let wav = decode_wav(&mixed_blob()).unwrap();
        assert_eq!(wav.len(), 4 + (3 * 4096 + 50) * 2);
    }

    #[test]
    fn fuzz_corrupted_wav_flac_never_panics_or_overallocates() {
        let base = mixed_blob();
        assert!(decode_wav(&base).is_ok());
        crate::test_support::fuzz("wav-flac", 0x5555, 2500, &base, |d| {
            let _ = decode_wav(d);
            let _ = decode_wav_limited(d, 1 << 20);
        });
    }
}
