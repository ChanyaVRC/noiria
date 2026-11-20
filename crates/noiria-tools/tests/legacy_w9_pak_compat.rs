//! W9 (LPC 拡張前) 形式の pak が新デコーダ (LPC 拡張後の
//! `noiria_core::codec::flac_subset::decode_wav`) でも引き続き読めることを検証する
//! 後方互換テスト。`docs/implementation-notes.md` 2026-07-13 wav-flac LPC 拡張 参照。
//!
//! ## なぜ現行の `noiria_tools::flac_enc` を使わないのか
//! 現行のエンコーダは LPC 拡張後の実装であり、そのまま使うと「新エンコーダの出力を
//! 新デコーダで読む」テストにしかならず、「**旧**フォーマットとの互換性」を検証した
//! ことにならない。そのため、本ファイルには W9 時点の固定予測のみのビット詰めロジック
//! （サブフレーム種別 0=CONSTANT/1=VERBATIM/2=FIXED のみ、LPC 種別3 は一切出力しない）を
//! **凍結したスナップショットとして独立に複製**してある。今後 `flac_enc.rs` 側の実装が
//! どう変わっても、このファイルは変更せず「W9 が実際に出力していたバイト列」を再現し
//! 続ける（そのため通常のリファクタリング対象にはしない）。
//!
//! 一度生成したビット列は `tests/fixtures/w9_legacy_fixed_only.bin` に固定フィクスチャとして
//! 保存済み。`regenerate_legacy_fixture`（`#[ignore]`）はそのフィクスチャがどう作られたかの
//! 記録を兼ねる再生成用ユーティリティで、通常の `cargo test` では実行されない。

use noiria_core::codec::flac_subset::decode_wav;

const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/w9_legacy_fixed_only.bin"
);

/// 固定フィクスチャが復元すべき PCM サンプル列（mono, 3000 フレーム、ランプ波形）。
/// `regenerate_legacy_fixture` が同じ式でフィクスチャを生成した。
fn expected_samples() -> Vec<i16> {
    (0..3000i32).map(|i| ((i % 300) - 150) as i16).collect()
}

#[test]
fn legacy_w9_fixed_only_blob_still_decodes_with_current_decoder() {
    let blob = std::fs::read(FIXTURE_PATH).expect("固定フィクスチャが読めません");
    let wav = decode_wav(&blob).expect("W9形式(fixedのみ)のblobが新デコーダで読めるはず");
    let samples: Vec<i16> = wav
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    assert_eq!(
        samples,
        expected_samples(),
        "旧フォーマットの復元サンプルが一致しません"
    );
}

// ---------------------------------------------------------------------
// 以下、W9 時点（LPC拡張前）の固定予測のみのビット詰めロジックを凍結して複製したもの。
// `crates/noiria-tools/src/flac_enc.rs` の実装とは意図的に同期させない
// （このファイルの目的は「今のエンコーダ」ではなく「当時のエンコーダの出力」の再現）。
// ---------------------------------------------------------------------

const LEGACY_MAGIC: &[u8; 4] = b"NFC1";
const LEGACY_BLOCK_SIZE: usize = 4096;
const LEGACY_SUBFRAME_CONSTANT: u64 = 0;
const LEGACY_SUBFRAME_VERBATIM: u64 = 1;
const LEGACY_SUBFRAME_FIXED: u64 = 2;

struct LegacyBitWriter {
    bytes: Vec<u8>,
    cur: u8,
    nbits: u32,
}
impl LegacyBitWriter {
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

fn legacy_zigzag(r: i64) -> u64 {
    ((r << 1) ^ (r >> 63)) as u64
}

fn legacy_fixed_predict(order: usize, block: &[i16], i: usize) -> i64 {
    let x = |d: usize| block[i - d] as i64;
    match order {
        0 => 0,
        1 => x(1),
        2 => 2 * x(1) - x(2),
        3 => 3 * x(1) - 3 * x(2) + x(3),
        4 => 4 * x(1) - 6 * x(2) + 4 * x(3) - x(4),
        _ => unreachable!(),
    }
}

fn legacy_fixed_order_cost(block: &[i16], order: usize) -> Option<(u64, u32)> {
    let n = block.len() - order;
    if n == 0 {
        return None;
    }
    let mut zz = Vec::with_capacity(n);
    let mut sum_z: u64 = 0;
    for i in order..block.len() {
        let pred = legacy_fixed_predict(order, block, i);
        let residual = block[i] as i64 - pred;
        let z = legacy_zigzag(residual);
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
    let header_bits = 2 + 3 + 5 + (order as u64) * 16;
    Some((header_bits + payload_bits, k))
}

enum LegacyCandidate {
    Verbatim,
    Fixed { order: usize, k: u32 },
}

fn legacy_encode_block(writer: &mut LegacyBitWriter, block: &[i16]) {
    if block.iter().all(|&s| s == block[0]) {
        writer.write_bits(LEGACY_SUBFRAME_CONSTANT, 2);
        writer.write_bits(block[0] as u16 as u64, 16);
        return;
    }
    let max_order = 4.min(block.len() - 1);
    let mut best_cost = 2 + 16 * block.len() as u64;
    let mut best = LegacyCandidate::Verbatim;
    for order in 0..=max_order {
        if let Some((cost, k)) = legacy_fixed_order_cost(block, order) {
            if cost < best_cost {
                best_cost = cost;
                best = LegacyCandidate::Fixed { order, k };
            }
        }
    }
    match best {
        LegacyCandidate::Verbatim => {
            writer.write_bits(LEGACY_SUBFRAME_VERBATIM, 2);
            for &s in block {
                writer.write_bits(s as u16 as u64, 16);
            }
        }
        LegacyCandidate::Fixed { order, k } => {
            writer.write_bits(LEGACY_SUBFRAME_FIXED, 2);
            writer.write_bits(order as u64, 3);
            writer.write_bits(k as u64, 5);
            for &s in &block[..order] {
                writer.write_bits(s as u16 as u64, 16);
            }
            for i in order..block.len() {
                let pred = legacy_fixed_predict(order, block, i);
                let residual = block[i] as i64 - pred;
                let zz = legacy_zigzag(residual);
                writer.write_rice(zz, k);
            }
        }
    }
}

fn legacy_encode_channel(writer: &mut LegacyBitWriter, samples: &[i16]) {
    let mut i = 0;
    while i < samples.len() {
        let end = (i + LEGACY_BLOCK_SIZE).min(samples.len());
        legacy_encode_block(writer, &samples[i..end]);
        i = end;
    }
}

/// フィクスチャ生成ユーティリティ（記録用、通常は実行しない）。
/// 実行すると `tests/fixtures/w9_legacy_fixed_only.bin` を上書き生成する。
/// フォーマット自体（CONSTANT/VERBATIM/FIXED のビット詰め）は W9 のまま一切変更していない
/// ため、いつ再生成しても同じバイト列になるはず。
#[test]
#[ignore]
fn regenerate_legacy_fixture() {
    let samples = expected_samples();
    let mut writer = LegacyBitWriter::new();
    legacy_encode_channel(&mut writer, &samples);
    let bitstream = writer.finish();

    let mut blob = Vec::new();
    blob.extend_from_slice(LEGACY_MAGIC);
    blob.extend_from_slice(&1u16.to_le_bytes()); // channels = 1 (mono)
    blob.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    blob.extend_from_slice(&0u32.to_le_bytes()); // prefix_len = 0
    blob.extend_from_slice(&0u32.to_le_bytes()); // suffix_len = 0
    blob.extend_from_slice(&bitstream);

    // 生成したものが現行デコーダで正しく読めることをその場でも確認してから書き出す。
    let wav = decode_wav(&blob).expect("生成直後の legacy blob が decode できるはず");
    let restored: Vec<i16> = wav
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    assert_eq!(restored, samples);

    std::fs::write(FIXTURE_PATH, &blob).expect("フィクスチャの書き出しに失敗しました");
    eprintln!("wrote {} bytes to {FIXTURE_PATH}", blob.len());
}
