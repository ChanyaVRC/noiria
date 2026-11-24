//! zlib (RFC1950) / DEFLATE (RFC1951) の自作デコーダ。
//! 外部非依存化ロードマップの一歩目。PNG デコーダ (png.rs) が使う。

use std::sync::OnceLock;

use anyhow::{bail, ensure, Result};

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    nbits: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            buf: 0,
            nbits: 0,
        }
    }

    #[inline(always)]
    fn refill(&mut self) {
        if self.nbits >= 56 {
            return;
        }
        if self.pos + 8 <= self.data.len() {
            let bytes = (63 - self.nbits) >> 3;
            let s = &self.data[self.pos..self.pos + 8];
            let v = u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]);
            let mask = (1u64 << (bytes * 8)) - 1;
            self.buf |= (v & mask) << self.nbits;
            self.pos += bytes as usize;
            self.nbits += bytes * 8;
        } else {
            while self.nbits <= 56 && self.pos < self.data.len() {
                self.buf |= (self.data[self.pos] as u64) << self.nbits;
                self.pos += 1;
                self.nbits += 8;
            }
        }
    }

    /// LSB ファーストで 1 ビット読む。
    #[inline(always)]
    fn bit(&mut self) -> Result<u32> {
        if self.nbits == 0 {
            self.refill();
            ensure!(self.nbits > 0, "DEFLATE ストリームが途中で終わっています");
        }
        let b = (self.buf & 1) as u32;
        self.buf >>= 1;
        self.nbits -= 1;
        Ok(b)
    }

    #[inline(always)]
    fn bits(&mut self, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        if self.nbits < n {
            self.refill();
            ensure!(self.nbits >= n, "DEFLATE ストリームが途中で終わっています");
        }
        let v = (self.buf & ((1u64 << n) - 1)) as u32;
        self.buf >>= n;
        self.nbits -= n;
        Ok(v)
    }

    /// 次のバイト境界へ移動し、バッファ済みバイトを `pos` へ戻す（stored ブロック用）。
    fn align(&mut self) {
        let drop = self.nbits & 7;
        self.buf >>= drop;
        self.nbits -= drop;
        let rewind = (self.nbits >> 3) as usize;
        self.pos -= rewind;
        self.buf = 0;
        self.nbits = 0;
    }

    fn read_u16le(&mut self) -> Result<u16> {
        debug_assert_eq!(self.nbits, 0);
        ensure!(
            self.pos + 2 <= self.data.len(),
            "DEFLATE ストリームが途中で終わっています"
        );
        let v = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    fn read_slice(&mut self, len: usize) -> Result<&'a [u8]> {
        debug_assert_eq!(self.nbits, 0);
        ensure!(
            self.pos + len <= self.data.len(),
            "DEFLATE ストリームが途中で終わっています"
        );
        let s = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok(s)
    }
}

const FAST_BITS: u32 = 9;
const FAST_SIZE: usize = 1 << FAST_BITS;
const FAST_MASK: u64 = (FAST_SIZE as u64) - 1;

/// カノニカルハフマン表（9bit 直接参照テーブル + 10..=15bit 用フォールバック）。
struct Huffman {
    /// 下位 8bit = 符号長 (0 なら未登録/10bit以上)、上位 = シンボル
    fast: [u32; FAST_SIZE],
    count: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn from_lengths(lengths: &[u8]) -> Result<Self> {
        let mut count = [0u16; 16];
        for &l in lengths {
            ensure!(l <= 15, "符号長が不正です");
            count[l as usize] += 1;
        }
        count[0] = 0;
        // 過剰な符号割り当てを検査
        let mut left = 1i32;
        for &c in &count[1..16] {
            left <<= 1;
            left -= c as i32;
            ensure!(left >= 0, "ハフマン符号長の割当が過剰です");
        }
        // 長さごとのシンボル開始オフセット
        let mut offs = [0u16; 16];
        for len in 1..15 {
            offs[len + 1] = offs[len] + count[len];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }

        // 9bit 以下の符号を LSB ファーストの高速テーブルに展開する。
        let mut fast = [0u32; FAST_SIZE];
        let mut code = 0u32;
        let mut idx = 0usize;
        for len in 1..=FAST_BITS {
            let cnt = count[len as usize] as usize;
            let step = 1usize << len;
            for _ in 0..cnt {
                let sym = symbols[idx] as u32;
                idx += 1;
                let rev = ((code as u16).reverse_bits() >> (16 - len)) as usize;
                let entry = (sym << 8) | len;
                let mut k = rev;
                while k < FAST_SIZE {
                    fast[k] = entry;
                    k += step;
                }
                code += 1;
            }
            code <<= 1;
        }

        Ok(Self {
            fast,
            count,
            symbols,
        })
    }

    #[inline(always)]
    fn decode(&self, br: &mut BitReader) -> Result<u16> {
        if br.nbits < 15 {
            br.refill();
        }
        let entry = self.fast[(br.buf & FAST_MASK) as usize];
        let len = entry & 0xFF;
        if len != 0 && br.nbits >= len {
            br.buf >>= len;
            br.nbits -= len;
            return Ok((entry >> 8) as u16);
        }
        self.decode_slow(br)
    }

    #[cold]
    fn decode_slow(&self, br: &mut BitReader) -> Result<u16> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for len in 1..=15 {
            code |= br.bit()? as i32;
            let cnt = self.count[len] as i32;
            if code - first < cnt {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += cnt;
            first = (first + cnt) << 1;
            code <<= 1;
        }
        bail!("不正なハフマン符号");
    }
}

fn fixed_huffman() -> &'static (Huffman, Huffman) {
    static FIXED: OnceLock<(Huffman, Huffman)> = OnceLock::new();
    FIXED.get_or_init(|| {
        let mut ll = [0u8; 288];
        ll[0..144].fill(8);
        ll[144..256].fill(9);
        ll[256..280].fill(7);
        ll[280..288].fill(8);
        let litlen = Huffman::from_lengths(&ll).expect("RFC 1951 固定 litlen ハフマン表は常に有効");
        let dist =
            Huffman::from_lengths(&[5u8; 30]).expect("RFC 1951 固定 dist ハフマン表は常に有効");
        (litlen, dist)
    })
}

pub(super) const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
pub(super) const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
pub(super) const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
pub(super) const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
pub(super) const CLEN_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn inflate_block(
    br: &mut BitReader,
    out: &mut Vec<u8>,
    max_out: usize,
    litlen: &Huffman,
    dist: &Huffman,
) -> Result<()> {
    loop {
        let sym = litlen.decode(br)?;
        match sym {
            0..=255 => {
                ensure!(
                    out.len() < max_out,
                    "展開後サイズが上限 {max_out} を超えます"
                );
                out.push(sym as u8)
            }
            256 => return Ok(()),
            257..=285 => {
                let i = (sym - 257) as usize;
                let len = LENGTH_BASE[i] as usize + br.bits(LENGTH_EXTRA[i] as u32)? as usize;
                let dsym = dist.decode(br)? as usize;
                ensure!(dsym < 30, "不正な距離シンボル");
                let d = DIST_BASE[dsym] as usize + br.bits(DIST_EXTRA[dsym] as u32)? as usize;
                ensure!(d <= out.len(), "距離が出力より大きい");
                ensure!(
                    len <= max_out - out.len(),
                    "展開後サイズが上限 {max_out} を超えます"
                );
                let start = out.len() - d;
                if d >= len {
                    out.extend_from_within(start..start + len);
                } else if d == 1 {
                    let b = out[start];
                    out.resize(out.len() + len, b);
                } else {
                    let mut remaining = len;
                    let mut copy_len = d;
                    while remaining > 0 {
                        let step = copy_len.min(remaining);
                        out.extend_from_within(start..start + step);
                        remaining -= step;
                        copy_len += step;
                    }
                }
            }
            _ => bail!("不正なリテラル/長さシンボル: {sym}"),
        }
    }
}

/// 生 DEFLATE ストリームを展開する。
pub fn inflate(data: &[u8]) -> Result<Vec<u8>> {
    inflate_limited(data, usize::MAX)
}

/// 展開後サイズが `max_out` バイトを超えたら `Err` にする [`inflate`]（1032:1 に達する
/// DEFLATE 爆弾で巨大確保させない）。期待サイズが分かる呼び出し側（PNG の IDAT、pak エントリ）が使う。
pub fn inflate_limited(data: &[u8], max_out: usize) -> Result<Vec<u8>> {
    let hint = data.len().saturating_mul(3).min(max_out).min(4 << 20);
    inflate_limited_with_capacity(data, max_out, hint)
}

pub(crate) fn inflate_limited_with_capacity(
    data: &[u8],
    max_out: usize,
    capacity_hint: usize,
) -> Result<Vec<u8>> {
    let mut br = BitReader::new(data);
    let safe_cap = capacity_hint
        .min(data.len().saturating_mul(16).max(4096))
        .min(max_out);
    let mut out = Vec::with_capacity(safe_cap);
    loop {
        let bfinal = br.bit()?;
        let btype = br.bits(2)?;
        match btype {
            0 => {
                // 無圧縮ブロック
                br.align();
                let len = br.read_u16le()?;
                let nlen = br.read_u16le()?;
                ensure!(len == !nlen, "stored ブロックの LEN/NLEN 不一致");
                ensure!(
                    len as usize <= max_out - out.len(),
                    "展開後サイズが上限 {max_out} を超えます"
                );
                out.extend_from_slice(br.read_slice(len as usize)?);
            }
            1 => {
                // 固定ハフマン
                let (litlen, dist) = fixed_huffman();
                inflate_block(&mut br, &mut out, max_out, litlen, dist)?;
            }
            2 => {
                // 動的ハフマン
                let hlit = br.bits(5)? as usize + 257;
                let hdist = br.bits(5)? as usize + 1;
                let hclen = br.bits(4)? as usize + 4;
                let mut clen = [0u8; 19];
                for i in 0..hclen {
                    clen[CLEN_ORDER[i]] = br.bits(3)? as u8;
                }
                let cl_huff = Huffman::from_lengths(&clen)?;
                let mut lengths = [0u8; 286 + 32];
                let total_lens = hlit + hdist;
                let mut i = 0;
                while i < total_lens {
                    let sym = cl_huff.decode(&mut br)?;
                    match sym {
                        0..=15 => {
                            lengths[i] = sym as u8;
                            i += 1;
                        }
                        16 => {
                            ensure!(i > 0, "repeat の先頭に前符号がありません");
                            let prev = lengths[i - 1];
                            let n = 3 + br.bits(2)? as usize;
                            ensure!(i + n <= total_lens, "符号長の繰り返しが範囲外");
                            lengths[i..i + n].fill(prev);
                            i += n;
                        }
                        17 => {
                            let n = 3 + br.bits(3)? as usize;
                            i += n;
                        }
                        18 => {
                            let n = 11 + br.bits(7)? as usize;
                            i += n;
                        }
                        _ => bail!("不正な符号長シンボル"),
                    }
                }
                ensure!(i == total_lens, "符号長テーブルが範囲を超過");
                ensure!(lengths[256] != 0, "終端シンボルの符号がありません");
                let litlen = Huffman::from_lengths(&lengths[..hlit])?;
                let dist = Huffman::from_lengths(&lengths[hlit..total_lens])?;
                inflate_block(&mut br, &mut out, max_out, &litlen, &dist)?;
            }
            _ => bail!("不正なブロックタイプ"),
        }
        if bfinal == 1 {
            return Ok(out);
        }
    }
}

pub fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for chunk in data.chunks(5552) {
        let (blocks, rem) = chunk.as_chunks::<16>();
        for blk in blocks {
            for &x in blk {
                a += x as u32;
                b += a;
            }
        }
        for &x in rem {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// zlib ラッパ (RFC1950) を展開し、adler32 を検証する。
pub fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>> {
    zlib_decompress_limited(data, usize::MAX)
}

/// 展開後サイズが `max_out` バイトを超えたら `Err` にする [`zlib_decompress`]。
pub fn zlib_decompress_limited(data: &[u8], max_out: usize) -> Result<Vec<u8>> {
    let hint = data.len().saturating_mul(3).min(max_out).min(4 << 20);
    zlib_decompress_limited_with_capacity(data, max_out, hint)
}

pub(crate) fn zlib_decompress_limited_with_capacity(
    data: &[u8],
    max_out: usize,
    capacity_hint: usize,
) -> Result<Vec<u8>> {
    ensure!(data.len() >= 6, "zlib ストリームが短すぎます");
    let cmf = data[0];
    let flg = data[1];
    ensure!(cmf & 0x0f == 8, "非対応の zlib 圧縮方式");
    ensure!(
        (((cmf as u16) << 8) | flg as u16).is_multiple_of(31),
        "zlib ヘッダ検査に失敗"
    );
    ensure!(flg & 0x20 == 0, "プリセット辞書は非対応");
    let body = &data[2..data.len() - 4];
    let out = inflate_limited_with_capacity(body, max_out, capacity_hint)?;
    let expected = u32::from_be_bytes([
        data[data.len() - 4],
        data[data.len() - 3],
        data[data.len() - 2],
        data[data.len() - 1],
    ]);
    ensure!(adler32(&out) == expected, "adler32 チェックサム不一致");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_block() {
        // 手組みの zlib stored ブロック: "hello"
        let payload = b"hello";
        let mut z = vec![0x78, 0x01, 0x01, 0x05, 0x00, 0xFA, 0xFF];
        z.extend_from_slice(payload);
        z.extend_from_slice(&adler32(payload).to_be_bytes());
        assert_eq!(zlib_decompress(&z).unwrap(), payload);
    }

    #[test]
    fn adler32_known_value() {
        // "Wikipedia" の adler32 は 0x11E60398 (既知値)
        assert_eq!(adler32(b"Wikipedia"), 0x11E60398);
    }

    #[test]
    fn broken_checksum_rejected() {
        let payload = b"hello";
        let mut z = vec![0x78, 0x01, 0x01, 0x05, 0x00, 0xFA, 0xFF];
        z.extend_from_slice(payload);
        z.extend_from_slice(&(adler32(payload) ^ 1).to_be_bytes());
        assert!(zlib_decompress(&z).is_err());
    }
    // --- R1: 出力上限つきの展開 ---

    #[test]
    fn limited_inflate_stops_at_the_limit_and_accepts_exact_size() {
        let data = vec![7u8; 100_000];
        let z = crate::codec::deflate::zlib_compress(&data);
        assert_eq!(zlib_decompress_limited(&z, data.len()).unwrap(), data);
        assert!(zlib_decompress_limited(&z, data.len() - 1).is_err());
        let raw = crate::codec::deflate::deflate_compress(&data);
        assert_eq!(inflate_limited(&raw, data.len()).unwrap(), data);
        assert!(inflate_limited(&raw, 10).is_err());
        assert!(inflate_limited(&raw, 0).is_err());
        // stored ブロックも上限を守る。
        let mut stored = vec![0x01, 0x05, 0x00, 0xFA, 0xFF];
        stored.extend_from_slice(b"hello");
        assert_eq!(inflate_limited(&stored, 5).unwrap(), b"hello");
        assert!(inflate_limited(&stored, 4).is_err());
    }
    #[test]
    fn fuzz_corrupted_deflate_never_panics_or_overallocates() {
        let text = "the quick brown fox jumps over the lazy dog. ".repeat(40);
        let mut seed = 5u64;
        let noise: Vec<u8> = (0..600)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                (seed >> 40) as u8
            })
            .collect();
        let mut stored = vec![0x01, 0x05, 0x00, 0xFA, 0xFF];
        stored.extend_from_slice(b"hello");
        let bases = [
            crate::codec::deflate::deflate_compress(text.as_bytes()),
            crate::codec::deflate::deflate_compress(&noise),
            stored,
        ];
        for (i, base) in bases.iter().enumerate() {
            assert!(inflate(base).is_ok(), "基準入力 {i} が展開できる");
            crate::test_support::fuzz("inflate", 0x3333 + i as u64, 1500, base, |d| {
                let _ = inflate(d);
                let _ = inflate_limited(d, 4096);
            });
        }
        let z = crate::codec::deflate::zlib_compress(text.as_bytes());
        assert!(zlib_decompress(&z).is_ok());
        crate::test_support::fuzz("zlib", 0x3399, 1500, &z, |d| {
            let _ = zlib_decompress(d);
            let _ = zlib_decompress_limited(d, 1000);
        });
    }
}
