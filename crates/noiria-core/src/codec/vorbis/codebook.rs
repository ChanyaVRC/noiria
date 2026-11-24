//! コードブック (VQ) デコード。仕様書 3 章 “Probability Model and Codebooks”。
//! カノニカルハフマンでエントリ番号を復号し、lookup type 1/2 の場合はそのエントリ番号を
//! オフセットに VQ ベクトルを合成する。

use super::bitreader::BitReader;
use anyhow::{bail, ensure, Result};

/// 仕様書 9.2.1 ilog(x): x の最上位ビット位置 (1-origin)。x<=0 は 0。
pub fn ilog(x: i64) -> u32 {
    let mut x = x;
    let mut n = 0u32;
    while x > 0 {
        n += 1;
        x >>= 1;
    }
    n
}

/// 仕様書 9.2.2 float32_unpack。
fn float32_unpack(x: u32) -> f32 {
    let mantissa = (x & 0x1f_ffff) as i64;
    let sign = x & 0x8000_0000;
    let exponent = ((x & 0x7fe0_0000) >> 21) as i32;
    let mantissa = if sign != 0 { -mantissa } else { mantissa };
    (mantissa as f64 * 2f64.powi(exponent - 788)) as f32
}

/// 仕様書 9.2.3 lookup1_values: r^dims <= entries となる最大の r。
fn lookup1_values(entries: u32, dims: u32) -> u32 {
    if dims == 0 {
        return 0;
    }
    let entries = entries as u64;
    let mut r: u64 = (entries as f64).powf(1.0 / dims as f64).floor() as u64;
    if r == 0 {
        r = 1;
    }
    let pow = |base: u64, exp: u32| -> u64 {
        let mut acc: u64 = 1;
        for _ in 0..exp {
            match acc.checked_mul(base) {
                Some(v) => acc = v,
                None => return u64::MAX,
            }
        }
        acc
    };
    while pow(r + 1, dims) <= entries {
        r += 1;
    }
    while r > 0 && pow(r, dims) > entries {
        r -= 1;
    }
    r as u32
}

const FAST_BITS: u32 = 10;
const FAST_SIZE: usize = 1 << FAST_BITS;

/// カノニカルハフマン復号表（10bit 高速ルックアップテーブル + 2 分木フォールバック）。
///
/// Vorbis のコードワード割当ては entry 番号昇順に貪欲割当てを行うため、
/// 構築時に 2 分木と 10bit 反転テーブルを展開して O(1) 復号できるようにする。
struct Huffman {
    /// 先読み `FAST_BITS` ビット（LSB ファースト）に対するパック済み `(len << 24) | entry`。
    /// `len == 0` は `FAST_BITS` 超（`nodes` を辿る）。`entries <= 1<<20` なので下位 24bit に収まる。
    fast: Box<[u32]>,
    /// 2 分木ノード。正の値は子ノード添字、負の値 `-(entry as i32) - 1` は葉、0 は無効。
    nodes: Box<[[i32; 2]]>,
    /// 仕様書 Errata 20150226: used entry がちょうど1個の特殊ケース。
    single_entry: Option<u32>,
}

impl Huffman {
    fn from_lengths(lengths: &[u8]) -> Result<Self> {
        let used: Vec<u32> = (0..lengths.len() as u32)
            .filter(|&i| lengths[i as usize] != 0)
            .collect();
        if used.len() == 1 {
            let e = used[0];
            ensure!(
                lengths[e as usize] == 1,
                "単一エントリコードブックの符号長は 1 でなければなりません"
            );
            return Ok(Self {
                fast: Box::new([]),
                nodes: Box::new([]),
                single_entry: Some(e),
            });
        }

        let mut open: std::collections::VecDeque<(u8, u64)> = std::collections::VecDeque::new();
        open.push_back((0u8, 0u64));
        let mut fast = vec![0u32; FAST_SIZE];
        let mut nodes: Vec<[i32; 2]> = vec![[0, 0]];
        for &e in &used {
            let l = lengths[e as usize];
            ensure!(l as usize <= 32 && l > 0, "コードブックの符号長が不正です");
            let pos = open
                .iter()
                .position(|&(d, _)| d <= l)
                .ok_or_else(|| anyhow::anyhow!("コードブックのハフマン符号割当が過剰です"))?;
            let (mut d, mut p) = open
                .remove(pos)
                .expect("pos は直前の position() が返した有効な添字");
            while d < l {
                open.insert(pos, (d + 1, 2 * p + 1));
                d += 1;
                p *= 2;
            }
            if (l as u32) <= FAST_BITS {
                // MSB ファーストの符号語 p（長さ l）を LSB ファーストの先読み添字へ反転展開。
                let mut rev = 0usize;
                for i in 0..l {
                    if (p >> (l - 1 - i)) & 1 != 0 {
                        rev |= 1 << i;
                    }
                }
                let packed = e | ((l as u32) << 24);
                let step = 1usize << l;
                let mut idx = rev;
                while idx < FAST_SIZE {
                    fast[idx] = packed;
                    idx += step;
                }
            } else {
                let mut cur = 0usize;
                for i in 0..l {
                    let bit = ((p >> (l - 1 - i)) & 1) as usize;
                    if i + 1 == l {
                        nodes[cur][bit] = -(e as i32) - 1;
                    } else {
                        let next = nodes[cur][bit];
                        if next == 0 {
                            let new_idx = nodes.len();
                            nodes.push([0, 0]);
                            nodes[cur][bit] = new_idx as i32;
                            cur = new_idx;
                        } else {
                            cur = next as usize;
                        }
                    }
                }
            }
        }
        // 完成木チェック: 未使用のまま残っている部分木があれば underspecified。
        ensure!(
            open.is_empty(),
            "コードブックのハフマン木が未完成です (underspecified)"
        );

        Ok(Self {
            fast: fast.into_boxed_slice(),
            nodes: nodes.into_boxed_slice(),
            single_entry: None,
        })
    }

    #[inline]
    fn decode(&self, br: &mut BitReader) -> Result<u32> {
        if let Some(e) = self.single_entry {
            br.read(1); // 値は 0/1 どちらでも許容 (Errata 20150226)
            return Ok(e);
        }
        let peek = br.peek(FAST_BITS) as usize;
        let packed = self.fast[peek];
        let len = packed >> 24;
        if len != 0 {
            br.consume(len);
            if br.eof {
                bail!("パケット終端でコードブック復号が続行できません");
            }
            return Ok(packed & 0x00FF_FFFF);
        }
        let mut cur = 0usize;
        loop {
            let bit = br.read(1) as usize;
            let next = self.nodes[cur][bit];
            if next < 0 {
                if br.eof {
                    bail!("パケット終端でコードブック復号が続行できません");
                }
                return Ok((-next - 1) as u32);
            }
            if br.eof {
                bail!("パケット終端でコードブック復号が続行できません");
            }
            if next == 0 {
                bail!("不正なハフマン符号 (コードブック)");
            }
            cur = next as usize;
        }
    }
}

/// 1 コードブックのエントリ数の上限。仕様上は 24bit だが、実在のエンコーダが作る帳簿は高々
/// 数万。ordered 形式は入力が数バイトでも宣言した数だけの符号長表とハフマン表を作らせられる。
const MAX_ENTRIES: u32 = 1 << 20;

/// VQ ベクトル表（entries * dimensions 個の f32）の要素数の上限（64MB）。
const MAX_VQ_FLOATS: u64 = 1 << 24;

pub struct Codebook {
    pub dimensions: u32,
    huffman: Huffman,
    /// lookup_type 1/2 の場合の VQ ベクトル (entries 個、各 dimensions 要素)。フラット格納。
    lookup: Option<Box<[f32]>>,
}

impl Codebook {
    pub fn parse(br: &mut BitReader) -> Result<Self> {
        let sync = br.read(24);
        ensure!(sync == 0x56_4342, "コードブックの同期パターンが不正です");
        let dimensions = br.read(16);
        let entries = br.read(24);
        let ordered = br.read_bool();
        ensure!(
            entries <= MAX_ENTRIES,
            "コードブックのエントリ数 {entries} が大きすぎます"
        );

        let mut lengths = vec![0u8; entries as usize];
        if !ordered {
            let sparse = br.read_bool();
            for l in lengths.iter_mut() {
                if sparse {
                    if br.read_bool() {
                        *l = br.read(5) as u8 + 1;
                    } else {
                        *l = 0;
                    }
                } else {
                    *l = br.read(5) as u8 + 1;
                }
            }
        } else {
            let mut current_entry: u32 = 0;
            let mut current_length: u32 = br.read(5) + 1;
            while current_entry < entries {
                let bits = ilog((entries - current_entry) as i64);
                let number = br.read(bits);
                ensure!(
                    current_entry + number <= entries,
                    "コードブックの順序付き符号長が範囲を超えます"
                );
                for e in current_entry..current_entry + number {
                    lengths[e as usize] = current_length as u8;
                }
                current_entry += number;
                current_length += 1;
                ensure!(current_length <= 33, "コードブックの符号長が大きすぎます");
            }
        }
        ensure!(
            !br.eof,
            "コードブックの符号長読み取り中にパケット終端に達しました"
        );

        let huffman = Huffman::from_lengths(&lengths)?;

        let lookup_type = br.read(4);
        let lookup = match lookup_type {
            0 => None,
            1 | 2 => {
                let min_value = float32_unpack(br.read(32));
                let delta_value = float32_unpack(br.read(32));
                let value_bits = br.read(4) + 1;
                let sequence_p = br.read_bool();
                // 宣言された寸法で確保する前に、VQ 表の大きさと、multiplicand を読むのに要る
                // ビット数が入力で賄えるかを確かめる。
                ensure!(
                    entries as u64 * dimensions as u64 <= MAX_VQ_FLOATS,
                    "コードブックの VQ 表が大きすぎます ({entries} エントリ x {dimensions} 次元)"
                );
                let lookup_values = if lookup_type == 1 {
                    lookup1_values(entries, dimensions)
                } else {
                    entries * dimensions
                };
                ensure!(
                    lookup_values as u64 * value_bits as u64 <= br.bits_left() as u64,
                    "コードブックの VQ lookup がパケット終端を超えます"
                );
                let mut multiplicands = vec![0u32; lookup_values as usize];
                for m in multiplicands.iter_mut() {
                    *m = br.read(value_bits);
                }
                ensure!(
                    !br.eof,
                    "コードブックの VQ lookup 読み取り中にパケット終端に達しました"
                );

                let mut vectors = vec![0f32; (entries as usize) * (dimensions as usize)];
                if lookup_type == 1 {
                    for entry in 0..entries {
                        let mut last = 0f32;
                        let mut index_divisor: u64 = 1;
                        for j in 0..dimensions {
                            let multiplicand_offset = ((entry as u64 / index_divisor)
                                % lookup_values.max(1) as u64)
                                as usize;
                            let value = multiplicands[multiplicand_offset] as f32 * delta_value
                                + min_value
                                + last;
                            vectors[(entry * dimensions + j) as usize] = value;
                            if sequence_p {
                                last = value;
                            }
                            index_divisor =
                                index_divisor.saturating_mul(lookup_values.max(1) as u64);
                        }
                    }
                } else {
                    for entry in 0..entries {
                        let mut last = 0f32;
                        for j in 0..dimensions {
                            let value = multiplicands[(entry * dimensions + j) as usize] as f32
                                * delta_value
                                + min_value
                                + last;
                            vectors[(entry * dimensions + j) as usize] = value;
                            if sequence_p {
                                last = value;
                            }
                        }
                    }
                }
                Some(vectors.into_boxed_slice())
            }
            _ => bail!("非対応の codebook lookup type: {lookup_type}"),
        };

        Ok(Self {
            dimensions,
            huffman,
            lookup,
        })
    }

    /// スカラーコンテキストでの復号 (エントリ番号そのものを返す)。
    pub fn decode_scalar(&self, br: &mut BitReader) -> Result<u32> {
        self.huffman.decode(br)
    }

    /// VQ コンテキストでの復号 (エントリ番号に対応するベクトルを返す)。
    pub fn decode_vector(&self, br: &mut BitReader) -> Result<&[f32]> {
        let entry = self.huffman.decode(br)?;
        let lookup = self.lookup.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "lookup type 0 のコードブックに対して VQ コンテキストで復号しようとしました"
            )
        })?;
        let start = entry as usize * self.dimensions as usize;
        Ok(&lookup[start..start + self.dimensions as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ilog_examples() {
        // 仕様書 9.2.1 の例そのもの。
        assert_eq!(ilog(0), 0);
        assert_eq!(ilog(1), 1);
        assert_eq!(ilog(2), 2);
        assert_eq!(ilog(3), 2);
        assert_eq!(ilog(4), 3);
        assert_eq!(ilog(7), 3);
        assert_eq!(ilog(-5), 0);
    }

    #[test]
    fn huffman_matches_spec_example() {
        // 仕様書 3.2.1 の例:
        // entry0:len2 entry1:len4 entry2:len4 entry3:len4 entry4:len4 entry5:len2 entry6:len3 entry7:len3
        // -> codewords 00,0100,0101,0110,0111,10,110,111
        let lengths = [2u8, 4, 4, 4, 4, 2, 3, 3];
        let h = Huffman::from_lengths(&lengths).unwrap();

        // ハフマン符号語はビット文字列として「左から順にビットストリームに読まれる」
        // (仕様書 3.2.1 Note)。BitReader::read(1) は LSb から読むので、符号語の各文字を
        // そのまま順にビット位置へ詰めれば、符号語の先頭文字が最初に読まれるビットになる。
        fn bits_to_reader(bits: &str) -> (Vec<u8>, usize) {
            let mut data = vec![0u8; bits.len().div_ceil(8) + 1];
            let mut pos = 0usize;
            for c in bits.chars() {
                if c == '1' {
                    data[pos / 8] |= 1 << (pos % 8);
                }
                pos += 1;
            }
            (data, pos)
        }

        for (bits, expect_entry) in [
            ("00", 0u32),
            ("0100", 1),
            ("0101", 2),
            ("0110", 3),
            ("0111", 4),
            ("10", 5),
            ("110", 6),
            ("111", 7),
        ] {
            let (data, _) = bits_to_reader(bits);
            let mut br = BitReader::new(&data);
            let got = h.decode(&mut br).unwrap();
            assert_eq!(got, expect_entry, "codeword {bits}");
        }
    }

    #[test]
    fn single_entry_codebook_tolerates_either_bit() {
        let lengths = [1u8];
        let h = Huffman::from_lengths(&lengths).unwrap();
        for bit in [0u8, 1u8] {
            let data = [bit];
            let mut br = BitReader::new(&data);
            assert_eq!(h.decode(&mut br).unwrap(), 0);
        }
    }

    #[test]
    fn rejects_overspecified_lengths() {
        // 2 エントリしかないのに両方長さ1 (00,01 のような重複) は破綻した木。
        let lengths = [1u8, 1, 1];
        assert!(Huffman::from_lengths(&lengths).is_err());
    }

    #[test]
    fn lookup1_values_matches_definition() {
        assert_eq!(lookup1_values(256, 1), 256);
        assert_eq!(lookup1_values(256, 2), 16);
        assert_eq!(lookup1_values(100, 2), 10);
        assert_eq!(lookup1_values(99, 2), 9);
    }
    // --- 外部入力の堅牢化（R1）: 宣言された寸法が VQ テーブル確保を暴走させない ---

    use crate::test_support::{with_alloc_limit, LsbBits};

    /// `entries` 個すべてが同じ符号長（完全木になる 2 の冪）の ordered コードブック + lookup。
    fn big_book(entries: u32, dims: u32, lookup_type: u32) -> Vec<u8> {
        assert!(entries.is_power_of_two());
        let len = entries.trailing_zeros();
        let mut b = LsbBits::default();
        b.push(0x56_4342, 24)
            .push(dims as u64, 16)
            .push(entries as u64, 24)
            .push(1, 1) // ordered
            .push((len - 1) as u64, 5)
            .push(entries as u64, ilog(entries as i64));
        b.push(lookup_type as u64, 4)
            .push(0, 32) // min
            .push(0, 32) // delta
            .push(0, 4) // value_bits - 1
            .push(0, 1); // sequence_p
        b.finish()
    }

    #[test]
    fn huge_vq_table_is_an_error_not_a_multi_gigabyte_allocation() {
        for (entries, dims, lookup_type) in [
            (65_536u32, 65_535u32, 1u32), // entries * dims = 約 43 億 float
            (65_536, 65_535, 2),
            (131_072, 65_535, 2), // entries * dims が u32 を溢れる
        ] {
            let data = big_book(entries, dims, lookup_type);
            let r = with_alloc_limit(256 << 20, || {
                let mut br = BitReader::new(&data);
                Codebook::parse(&mut br).map(|_| ())
            });
            assert!(
                r.is_err(),
                "entries={entries} dims={dims} type={lookup_type}"
            );
        }
    }

    #[test]
    fn ordinary_ordered_book_with_vq_lookup_still_parses() {
        // 16 エントリ・2 次元・lookup type 2（multiplicand 32 個 × 4bit）。
        let mut b = LsbBits::default();
        b.push(0x56_4342, 24).push(2, 16).push(16, 24).push(1, 1);
        b.push(3, 5).push(16, ilog(16)); // 16 エントリ全部が符号長 4
        b.push(2, 4).push(0, 32).push(0, 32).push(3, 4).push(0, 1);
        for i in 0..32u64 {
            b.push(i % 16, 4);
        }
        let data = b.finish();
        let mut br = BitReader::new(&data);
        let cb = Codebook::parse(&mut br).unwrap();
        assert_eq!(cb.dimensions, 2);
    }
}
