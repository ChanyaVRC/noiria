//! zlib (RFC1950) / DEFLATE (RFC1951) の自作エンコーダ。
//! `inflate.rs` (デコーダ) と対をなす。外部非依存化ロードマップの一歩目。
//!
//! - LZ77: 窓 32KB、3 バイトハッシュ+ハッシュチェーン探索（上限 ~128 チェーン、
//!   good-match で打ち切り）、1 段 lazy matching（zlib level 6 相当）。
//! - ハフマン: ブロック単位（~128KB）で固定/動的ハフマンのビット数を見積り
//!   小さい方を採用。動的はコード長制限 15（コード長符号は 7）の canonical
//!   Huffman。符号長は package-merge (coin-collector) アルゴリズムで構築し、
//!   深さ制限下でも常に Kraft 不等式を等号で満たす（degenerate にならない）。
//!   圧縮不能なブロックは stored にフォールバックする。

use super::inflate::{adler32, CLEN_ORDER, DIST_BASE, DIST_EXTRA, LENGTH_BASE, LENGTH_EXTRA};

const LENGTH_LUT: [u8; 259] = {
    let mut t = [0u8; 259];
    let mut i = 0usize;
    while i < 29 {
        let start = LENGTH_BASE[i] as usize;
        let end = if i + 1 < 29 {
            LENGTH_BASE[i + 1] as usize
        } else {
            259
        };
        let mut l = start;
        while l < end {
            t[l] = i as u8;
            l += 1;
        }
        i += 1;
    }
    t
};

const DIST_LUT_LOW: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 0usize;
    while i < 16 {
        let start = DIST_BASE[i] as usize - 1;
        let end = DIST_BASE[i + 1] as usize - 1;
        let mut d = start;
        while d < end {
            t[d] = i as u8;
            d += 1;
        }
        i += 1;
    }
    t
};

const DIST_LUT_HIGH: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 16usize;
    while i < 30 {
        let start = (DIST_BASE[i] as usize - 1) >> 7;
        let end = if i + 1 < 30 {
            (DIST_BASE[i + 1] as usize - 1) >> 7
        } else {
            256
        };
        let mut d = start;
        while d < end {
            t[d] = i as u8;
            d += 1;
        }
        i += 1;
    }
    t
};

/// 長さ (3..=258) から (シンボル, 追加ビット数, 追加ビット値) を求める。
#[inline(always)]
fn length_code(len: usize) -> (u16, u8, u16) {
    let i = LENGTH_LUT[len] as usize;
    (
        257 + i as u16,
        LENGTH_EXTRA[i],
        (len - LENGTH_BASE[i] as usize) as u16,
    )
}

/// 距離 (1..=32768) から (シンボル, 追加ビット数, 追加ビット値) を求める。
#[inline(always)]
fn dist_code(dist: usize) -> (u16, u8, u16) {
    let d = dist - 1;
    let i = if d < 256 {
        DIST_LUT_LOW[d] as usize
    } else {
        DIST_LUT_HIGH[d >> 7] as usize
    };
    (
        i as u16,
        DIST_EXTRA[i],
        (dist - DIST_BASE[i] as usize) as u16,
    )
}

// ---------------------------------------------------------------------
// LZ77
// ---------------------------------------------------------------------

const WINDOW: usize = 32768;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const MAX_CHAIN: usize = 128;
const NICE_LEN: usize = 128; // これ以上の一致が見つかったらチェーン探索を打ち切る
const GOOD_LEN: usize = 8; // これ以上の一致が見つかったら残り探索数を絞る (zlib level6 相当)
const MAX_LAZY: usize = 16; // 一致長がこれ以上なら lazy 先読みをせず即採用する (zlib level6 相当)
const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MASK: u32 = (HASH_SIZE as u32) - 1;

#[derive(Clone, Copy)]
enum Token {
    Literal(u8),
    Match { len: u16, dist: u16 },
}

fn token_input_len(t: &Token) -> usize {
    match t {
        Token::Literal(_) => 1,
        Token::Match { len, .. } => *len as usize,
    }
}

struct Lz77State<'a> {
    data: &'a [u8],
    head: Vec<i32>,
    prev: Vec<i32>,
}

impl<'a> Lz77State<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            head: vec![-1; HASH_SIZE],
            prev: vec![-1; data.len().max(1)],
        }
    }

    fn hash_at(&self, pos: usize) -> u32 {
        let d = self.data;
        let mut h = 0u32;
        h = ((h << 5) ^ d[pos] as u32) & HASH_MASK;
        h = ((h << 5) ^ d[pos + 1] as u32) & HASH_MASK;
        h = ((h << 5) ^ d[pos + 2] as u32) & HASH_MASK;
        h
    }

    fn insert(&mut self, pos: usize) {
        if pos + MIN_MATCH > self.data.len() {
            return;
        }
        let h = self.hash_at(pos) as usize;
        self.prev[pos] = self.head[h];
        self.head[h] = pos as i32;
    }

    /// pos から始まる最良の一致 (len, dist) を探す（未挿入の pos より前だけを見る）。
    fn find_match(&self, pos: usize) -> Option<(usize, usize)> {
        let limit = (self.data.len() - pos).min(MAX_MATCH);
        if limit < MIN_MATCH {
            return None;
        }
        let h = self.hash_at(pos) as usize;
        let mut cand = self.head[h];
        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        let mut steps = 0usize;
        let mut max_steps = MAX_CHAIN;
        while cand >= 0 && steps < max_steps {
            let c = cand as usize;
            let dist = pos - c;
            if dist > WINDOW {
                break;
            }
            if best_len >= limit {
                break;
            }
            // best_len 位置のバイトが一致しない候補は現在の最良を更新できないので
            // 全比較を省略する（zlib longest_match と同じ枝刈り）。
            let d = self.data;
            if d[c + best_len] == d[pos + best_len] && d[c] == d[pos] {
                let mut l = 1usize;
                while l + 8 <= limit {
                    let a = u64::from_le_bytes(d[c + l..c + l + 8].try_into().unwrap());
                    let b = u64::from_le_bytes(d[pos + l..pos + l + 8].try_into().unwrap());
                    let diff = a ^ b;
                    if diff != 0 {
                        l += (diff.trailing_zeros() >> 3) as usize;
                        break;
                    }
                    l += 8;
                }
                while l < limit && d[c + l] == d[pos + l] {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_dist = dist;
                    if best_len >= NICE_LEN {
                        break;
                    }
                    if best_len >= GOOD_LEN {
                        // 十分な一致が見つかったら残り探索数を絞る（zlib と同じ発想）。
                        max_steps = max_steps.min(steps + MAX_CHAIN / 4);
                    }
                }
            }
            cand = self.prev[c];
            steps += 1;
        }
        if best_len >= MIN_MATCH {
            Some((best_len, best_dist))
        } else {
            None
        }
    }
}

/// LZ77 でトークン列を作る（1 段 lazy matching）。
fn lz77_encode(data: &[u8]) -> Vec<Token> {
    let mut st = Lz77State::new(data);
    let mut tokens = Vec::new();
    let n = data.len();
    let mut pos = 0usize;
    while pos < n {
        let m1 = st.find_match(pos);
        st.insert(pos);
        match m1 {
            None => {
                tokens.push(Token::Literal(data[pos]));
                pos += 1;
            }
            Some((len1, dist1)) => {
                if len1 < MAX_LAZY && pos + 1 + MIN_MATCH <= n {
                    let m2 = st.find_match(pos + 1);
                    st.insert(pos + 1);
                    if let Some((len2, dist2)) = m2 {
                        if len2 > len1 {
                            // pos は文字として出し、pos+1 のより良い一致を採用する
                            tokens.push(Token::Literal(data[pos]));
                            tokens.push(Token::Match {
                                len: len2 as u16,
                                dist: dist2 as u16,
                            });
                            for p in (pos + 2)..(pos + 1 + len2) {
                                st.insert(p);
                            }
                            pos = pos + 1 + len2;
                            continue;
                        }
                    }
                    // pos の一致を採用（pos+1 は既に挿入済み）
                    tokens.push(Token::Match {
                        len: len1 as u16,
                        dist: dist1 as u16,
                    });
                    for p in (pos + 2)..(pos + len1) {
                        st.insert(p);
                    }
                    pos += len1;
                } else {
                    tokens.push(Token::Match {
                        len: len1 as u16,
                        dist: dist1 as u16,
                    });
                    for p in (pos + 1)..(pos + len1) {
                        st.insert(p);
                    }
                    pos += len1;
                }
            }
        }
    }
    tokens
}

// ---------------------------------------------------------------------
// ビット出力
// ---------------------------------------------------------------------

struct BitWriter {
    out: Vec<u8>,
    cur: u64,
    nbits: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            cur: 0,
            nbits: 0,
        }
    }

    /// LSB ファーストで n ビット書く（n <= 32）。DEFLATE の「符号以外」の
    /// データ要素（extra bits・LEN/HLIT 等）および反転済みハフマン符号はこの並びになる。
    #[inline(always)]
    fn write_bits(&mut self, val: u32, n: u32) {
        debug_assert!(n <= 32);
        if n == 0 {
            return;
        }
        self.cur |= (val as u64) << self.nbits;
        self.nbits += n;
        if self.nbits >= 32 {
            self.out.extend_from_slice(&(self.cur as u32).to_le_bytes());
            self.cur >>= 32;
            self.nbits -= 32;
        }
    }

    #[inline(always)]
    fn write_bit(&mut self, b: u32) {
        self.write_bits(b, 1);
    }

    /// 事前にビット反転済みのハフマン符号を書き込む。
    #[inline(always)]
    fn write_huffman_code(&mut self, rev_code: u16, len: u8) {
        self.write_bits(rev_code as u32, len as u32);
    }

    /// 次のバイト境界へ 0 パディングする（stored ブロック用）。
    fn align(&mut self) {
        while self.nbits >= 8 {
            self.out.push((self.cur & 0xFF) as u8);
            self.cur >>= 8;
            self.nbits -= 8;
        }
        if self.nbits > 0 {
            self.out.push((self.cur & 0xFF) as u8);
            self.cur = 0;
            self.nbits = 0;
        }
    }

    fn write_raw_byte(&mut self, b: u8) {
        debug_assert_eq!(self.nbits, 0);
        self.out.push(b);
    }

    fn write_raw_slice(&mut self, bytes: &[u8]) {
        debug_assert_eq!(self.nbits, 0);
        self.out.extend_from_slice(bytes);
    }

    fn finish(mut self) -> Vec<u8> {
        self.align();
        self.out
    }
}

// ---------------------------------------------------------------------
// ハフマン符号長の構築（package-merge / coin-collector アルゴリズム）
// ---------------------------------------------------------------------
//
// 「頻度ソート + 深さ制限の単純調整」は Kraft 不等式をちょうど満たす
// （sum 2^-len == 1）ことを保証できず、深い符号を丸め込んだ際に degenerate
// な（不完全な）ハフマン木を生成してしまうケースがあった（image クレート等の
// 厳密なデコーダで "BadCodeLengthHuffmanTree" として拒否される）。
// package-merge は深さ制限付きでも構築そのものによって最適かつ厳密に
// 完全な符号長を与えるため、これに切り替えた。

/// 頻度配列から canonical Huffman の符号長を求める（各長さは 1..=max_length）。
/// package-merge (coin-collector) アルゴリズムで、深さ制限下でも常に
/// Kraft 不等式を等号で満たす（degenerate にならない）符号長を構築する。
fn build_lengths(freqs: &[u64], max_length: u8) -> Vec<u8> {
    let n = freqs.len();
    let mut lengths = vec![0u8; n];
    let mut active: Vec<usize> = (0..n).filter(|&i| freqs[i] > 0).collect();
    if active.is_empty() {
        return lengths;
    }
    if active.len() == 1 {
        lengths[active[0]] = 1;
        return lengths;
    }
    // 頻度昇順にソート（同頻度はシンボル番号昇順でタイブレークし決定的にする）。
    active.sort_by(|&a, &b| freqs[a].cmp(&freqs[b]).then(a.cmp(&b)));
    let m = active.len();
    let max_len = max_length as usize;

    #[derive(Clone, Copy)]
    struct Pkg {
        weight: u64,
        /// 0..m は葉 (active 内インデックス)、m.. は nodes 内の合成ノード
        node: u32,
    }

    let leaves: Vec<Pkg> = (0..m)
        .map(|i| Pkg {
            weight: freqs[active[i]],
            node: i as u32,
        })
        .collect();

    let mut nodes: Vec<(u32, u32)> = Vec::with_capacity(m * max_len);
    let mut list = leaves.clone();
    let mut packaged = Vec::with_capacity(m);
    let mut merged = Vec::with_capacity(2 * m);
    for _level in 2..=max_len {
        let usable = list.len() - (list.len() % 2);
        packaged.clear();
        let mut i = 0;
        while i < usable {
            let node_id = (m + nodes.len()) as u32;
            nodes.push((list[i].node, list[i + 1].node));
            packaged.push(Pkg {
                weight: list[i].weight + list[i + 1].weight,
                node: node_id,
            });
            i += 2;
        }
        merged.clear();
        let (mut pi, mut li) = (0usize, 0usize);
        while pi < packaged.len() && li < leaves.len() {
            if packaged[pi].weight <= leaves[li].weight {
                merged.push(packaged[pi]);
                pi += 1;
            } else {
                merged.push(leaves[li]);
                li += 1;
            }
        }
        merged.extend_from_slice(&packaged[pi..]);
        merged.extend_from_slice(&leaves[li..]);
        std::mem::swap(&mut list, &mut merged);
    }

    // 最終レベルの重み最小 (2m-2) 件を選び、DAG を逆順に伝播して各葉の出現回数を集計する。
    let take = (2 * m - 2).min(list.len());
    debug_assert_eq!(
        take,
        2 * m - 2,
        "max_length が小さすぎて package-merge が完了できない"
    );
    let mut refs = vec![0u32; m + nodes.len()];
    for pkg in &list[..take] {
        refs[pkg.node as usize] += 1;
    }
    for i in (0..nodes.len()).rev() {
        let r = refs[m + i];
        if r > 0 {
            let (a, b) = nodes[i];
            refs[a as usize] += r;
            refs[b as usize] += r;
        }
    }
    for (i, &sym) in active.iter().enumerate() {
        lengths[sym] = refs[i] as u8;
    }
    lengths
}

/// 符号長配列から canonical Huffman のビット反転済み符号語を求める (RFC1951 3.2.2)。
/// 送信時に LSB ファーストの `write_bits` へそのまま渡せる形にしておく。
fn build_codes(lengths: &[u8]) -> Vec<u16> {
    let mut bl_count = [0u32; 16];
    for &l in lengths {
        if l > 0 {
            bl_count[l as usize] += 1;
        }
    }
    let mut next_code = [0u32; 16];
    let mut code = 0u32;
    for bits in 1..16 {
        code = (code + bl_count[bits - 1]) << 1;
        next_code[bits] = code;
    }
    let mut codes = vec![0u16; lengths.len()];
    for (sym, &l) in lengths.iter().enumerate() {
        if l > 0 {
            let c = next_code[l as usize] as u16;
            next_code[l as usize] += 1;
            codes[sym] = c.reverse_bits() >> (16 - l);
        }
    }
    codes
}

fn fixed_litlen_lengths() -> [u8; 288] {
    let mut l = [0u8; 288];
    l[0..144].fill(8);
    l[144..256].fill(9);
    l[256..280].fill(7);
    l[280..288].fill(8);
    l
}

fn fixed_dist_lengths() -> [u8; 30] {
    [5u8; 30]
}

/// 符号長配列 (RFC1951 3.2.7) を 16/17/18 のランレングス符号列に変換する。
/// 各要素は (シンボル, 追加ビット数, 追加ビット値)。
fn rle_code_lengths(lens: &[u8]) -> Vec<(u8, u8, u16)> {
    let mut out = Vec::new();
    let n = lens.len();
    let mut i = 0;
    while i < n {
        let val = lens[i];
        let mut run = 1;
        while i + run < n && lens[i + run] == val {
            run += 1;
        }
        if val == 0 {
            let mut r = run;
            while r >= 11 {
                let c = r.min(138);
                out.push((18u8, 7u8, (c - 11) as u16));
                r -= c;
            }
            while r >= 3 {
                let c = r.min(10);
                out.push((17u8, 3u8, (c - 3) as u16));
                r -= c;
            }
            for _ in 0..r {
                out.push((0u8, 0u8, 0u16));
            }
        } else {
            out.push((val, 0u8, 0u16));
            let mut r = run - 1;
            while r >= 3 {
                let c = r.min(6);
                out.push((16u8, 2u8, (c - 3) as u16));
                r -= c;
            }
            for _ in 0..r {
                out.push((val, 0u8, 0u16));
            }
        }
        i += run;
    }
    out
}

fn weighted_bits(freqs: &[u64], lengths: &[u8]) -> u64 {
    freqs.iter().zip(lengths).map(|(&f, &l)| f * l as u64).sum()
}

// ---------------------------------------------------------------------
// ブロック符号化
// ---------------------------------------------------------------------

const BLOCK_SIZE: usize = 128 * 1024;

fn emit_stored_blocks(bw: &mut BitWriter, raw: &[u8], is_final: bool) {
    let n = raw.len();
    let subblocks = n.div_ceil(65535).max(1);
    for i in 0..subblocks {
        let start = i * 65535;
        let end = (start + 65535).min(n);
        let chunk = &raw[start..end];
        let sub_final = is_final && i == subblocks - 1;
        bw.write_bit(sub_final as u32);
        bw.write_bits(0, 2); // BTYPE = 00 (stored)
        bw.align();
        let len = chunk.len() as u16;
        bw.write_raw_byte((len & 0xFF) as u8);
        bw.write_raw_byte((len >> 8) as u8);
        let nlen = !len;
        bw.write_raw_byte((nlen & 0xFF) as u8);
        bw.write_raw_byte((nlen >> 8) as u8);
        bw.write_raw_slice(chunk);
    }
}

fn emit_tokens(
    bw: &mut BitWriter,
    tokens: &[Token],
    lit_codes: &[u16],
    lit_lengths: &[u8],
    dist_codes: &[u16],
    dist_lengths: &[u8],
) {
    for t in tokens {
        match *t {
            Token::Literal(b) => {
                bw.write_huffman_code(lit_codes[b as usize], lit_lengths[b as usize]);
            }
            Token::Match { len, dist } => {
                let (lsym, lextra, lval) = length_code(len as usize);
                bw.write_huffman_code(lit_codes[lsym as usize], lit_lengths[lsym as usize]);
                if lextra > 0 {
                    bw.write_bits(lval as u32, lextra as u32);
                }
                let (dsym, dextra, dval) = dist_code(dist as usize);
                bw.write_huffman_code(dist_codes[dsym as usize], dist_lengths[dsym as usize]);
                if dextra > 0 {
                    bw.write_bits(dval as u32, dextra as u32);
                }
            }
        }
    }
    bw.write_huffman_code(lit_codes[256], lit_lengths[256]);
}

fn encode_block(bw: &mut BitWriter, tokens: &[Token], raw: &[u8], is_final: bool) {
    let mut lit_freq = [0u64; 286];
    let mut dist_freq = [0u64; 30];
    lit_freq[256] = 1; // EOB
    let mut extra_bits_total = 0u64;
    for t in tokens {
        match *t {
            Token::Literal(b) => lit_freq[b as usize] += 1,
            Token::Match { len, dist } => {
                let (lsym, lextra, _) = length_code(len as usize);
                lit_freq[lsym as usize] += 1;
                extra_bits_total += lextra as u64;
                let (dsym, dextra, _) = dist_code(dist as usize);
                dist_freq[dsym as usize] += 1;
                extra_bits_total += dextra as u64;
            }
        }
    }
    if dist_freq.iter().all(|&f| f == 0) {
        // RFC1951: 距離符号は 1 つ以上必要（未使用でもダミーを立てる）。
        dist_freq[0] = 1;
    }

    let hlit = lit_freq
        .iter()
        .enumerate()
        .rev()
        .find(|&(_, &f)| f > 0)
        .map(|(i, _)| i + 1)
        .unwrap()
        .max(257);
    let hdist = dist_freq
        .iter()
        .enumerate()
        .rev()
        .find(|&(_, &f)| f > 0)
        .map(|(i, _)| i + 1)
        .unwrap()
        .max(1);

    let dyn_lit_lengths = build_lengths(&lit_freq[..hlit], 15);
    let dyn_dist_lengths = build_lengths(&dist_freq[..hdist], 15);

    let combined: Vec<u8> = dyn_lit_lengths
        .iter()
        .chain(dyn_dist_lengths.iter())
        .copied()
        .collect();
    let rle = rle_code_lengths(&combined);
    let mut cl_freq = [0u64; 19];
    for &(sym, _, _) in &rle {
        cl_freq[sym as usize] += 1;
    }
    let cl_lengths = build_lengths(&cl_freq, 7);
    let cl_codes = build_codes(&cl_lengths);
    let hclen = CLEN_ORDER
        .iter()
        .enumerate()
        .rev()
        .find(|&(_, &s)| cl_lengths[s] > 0)
        .map(|(i, _)| i + 1)
        .unwrap_or(4)
        .max(4);

    let mut dyn_header_bits: u64 = 5 + 5 + 4 + (hclen as u64) * 3;
    for &(sym, extra, _) in &rle {
        dyn_header_bits += cl_lengths[sym as usize] as u64 + extra as u64;
    }

    let dynamic_body_bits = weighted_bits(&lit_freq[..hlit], &dyn_lit_lengths)
        + weighted_bits(&dist_freq[..hdist], &dyn_dist_lengths)
        + extra_bits_total;
    let dynamic_total = 3 + dyn_header_bits + dynamic_body_bits;

    let fixed_lit_lengths = fixed_litlen_lengths();
    let fixed_dist_lengths_arr = fixed_dist_lengths();
    let fixed_body_bits = weighted_bits(&lit_freq, &fixed_lit_lengths[..286])
        + weighted_bits(&dist_freq, &fixed_dist_lengths_arr)
        + extra_bits_total;
    let fixed_total = 3 + fixed_body_bits;

    let stored_subblocks = raw.len().div_ceil(65535).max(1) as u64;
    let stored_total = stored_subblocks * 40 + raw.len() as u64 * 8;

    if stored_total <= dynamic_total && stored_total <= fixed_total {
        emit_stored_blocks(bw, raw, is_final);
    } else if fixed_total <= dynamic_total {
        bw.write_bit(is_final as u32);
        bw.write_bits(0b01, 2);
        let lit_codes = build_codes(&fixed_lit_lengths);
        let dist_codes = build_codes(&fixed_dist_lengths_arr);
        emit_tokens(
            bw,
            tokens,
            &lit_codes,
            &fixed_lit_lengths,
            &dist_codes,
            &fixed_dist_lengths_arr,
        );
    } else {
        bw.write_bit(is_final as u32);
        bw.write_bits(0b10, 2);
        bw.write_bits((hlit - 257) as u32, 5);
        bw.write_bits((hdist - 1) as u32, 5);
        bw.write_bits((hclen - 4) as u32, 4);
        for &sym in CLEN_ORDER.iter().take(hclen) {
            bw.write_bits(cl_lengths[sym] as u32, 3);
        }
        for &(sym, extra, val) in &rle {
            bw.write_huffman_code(cl_codes[sym as usize], cl_lengths[sym as usize]);
            if extra > 0 {
                bw.write_bits(val as u32, extra as u32);
            }
        }
        let lit_codes = build_codes(&dyn_lit_lengths);
        let dist_codes = build_codes(&dyn_dist_lengths);
        emit_tokens(
            bw,
            tokens,
            &lit_codes,
            &dyn_lit_lengths,
            &dist_codes,
            &dyn_dist_lengths,
        );
    }
}

/// 生 DEFLATE (RFC1951) ストリームへ圧縮する。
pub fn deflate_compress(data: &[u8]) -> Vec<u8> {
    let tokens = lz77_encode(data);
    let mut bw = BitWriter::new();
    if tokens.is_empty() {
        encode_block(&mut bw, &[], data, true);
        return bw.finish();
    }

    let mut blocks: Vec<(usize, usize, &[Token])> = Vec::new();
    let mut byte_pos = 0usize;
    let mut blk_start_tok = 0usize;
    let mut blk_start_byte = 0usize;
    for (i, t) in tokens.iter().enumerate() {
        byte_pos += token_input_len(t);
        if byte_pos - blk_start_byte >= BLOCK_SIZE {
            blocks.push((blk_start_byte, byte_pos, &tokens[blk_start_tok..=i]));
            blk_start_tok = i + 1;
            blk_start_byte = byte_pos;
        }
    }
    if blk_start_tok < tokens.len() {
        blocks.push((blk_start_byte, byte_pos, &tokens[blk_start_tok..]));
    }

    let n_blocks = blocks.len();
    for (idx, (s, e, toks)) in blocks.into_iter().enumerate() {
        encode_block(&mut bw, toks, &data[s..e], idx == n_blocks - 1);
    }
    bw.finish()
}

/// zlib (RFC1950) ラッパで包んだ DEFLATE ストリームを作る。
pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 8);
    out.extend_from_slice(&[0x78, 0x9C]); // CMF=8(窓32K)/CINFO=7, FLG (FLEVEL=2 既定相当)
    out.extend_from_slice(&deflate_compress(data));
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::inflate::{inflate, zlib_decompress};

    fn roundtrip(data: &[u8]) {
        let raw = deflate_compress(data);
        assert_eq!(
            inflate(&raw).unwrap(),
            data,
            "raw deflate round-trip 不一致 (len={})",
            data.len()
        );
        let z = zlib_compress(data);
        assert_eq!(
            zlib_decompress(&z).unwrap(),
            data,
            "zlib round-trip 不一致 (len={})",
            data.len()
        );
    }

    fn lcg(seed: &mut u64) -> u8 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 33) as u8
    }

    #[test]
    fn empty_input() {
        roundtrip(&[]);
    }

    #[test]
    fn single_byte() {
        roundtrip(&[42]);
    }

    #[test]
    fn all_same_byte_short() {
        roundtrip(&[7; 5]);
    }

    #[test]
    fn all_same_byte_long_crosses_window() {
        // 32KB 窓境界を大きく跨ぐ均一データ
        roundtrip(&vec![0xABu8; 200_000]);
    }

    #[test]
    fn random_incompressible() {
        let mut seed = 1234u64;
        let data: Vec<u8> = (0..70_000).map(|_| lcg(&mut seed)).collect();
        roundtrip(&data);
    }

    #[test]
    fn periodic_repeat_crosses_window_boundary() {
        // 周期 97 バイトのパターンを 40000 バイト超まで繰り返す
        let unit: Vec<u8> = (0..97u32).map(|i| (i * 37 % 251) as u8).collect();
        let mut data = Vec::new();
        while data.len() < 45_000 {
            data.extend_from_slice(&unit);
        }
        roundtrip(&data);
    }

    #[test]
    fn japanese_text() {
        let para = "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。\
                     何でも薄暗いじめじめした所でニャーニャー泣いていた事だけは記憶している。"
            .repeat(500);
        roundtrip(para.as_bytes());
    }

    #[test]
    fn long_match_over_258() {
        // 単一マッチの上限 258 を超える長さの反復（複数マッチトークンに分割される）
        let mut data = vec![1u8, 2, 3];
        data.extend(std::iter::repeat_n(9u8, 1000));
        roundtrip(&data);
    }

    #[test]
    fn real_png_raw_pixels() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/sample/assets");
        if !root.exists() {
            return;
        }
        for rel in ["bg/room.png", "fg/alice_normal.png"] {
            let path = root.join(rel);
            if !path.exists() {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let decoded = super::super::png::decode(&bytes).unwrap();
            roundtrip(&decoded.rgba);
        }
    }

    #[test]
    fn multi_block_boundary() {
        // BLOCK_SIZE (128KiB) を跨ぐ、圧縮性のある擬似ランダムデータ
        let mut seed = 99u64;
        let mut data = Vec::new();
        while data.len() < 300_000 {
            if lcg(&mut seed).is_multiple_of(3) {
                let unit: Vec<u8> = (0..20).map(|_| lcg(&mut seed)).collect();
                for _ in 0..5 {
                    data.extend_from_slice(&unit);
                }
            } else {
                data.push(lcg(&mut seed));
            }
        }
        roundtrip(&data);
    }

    #[test]
    fn property_random_mixed_many_cases() {
        // 圧縮性のあり/なしが混在するデータで数百ケースの round-trip
        let mut seed = 555u64;
        for case in 0..300u32 {
            seed = seed.wrapping_add(case as u64 * 7919 + 1);
            let len = (lcg(&mut seed) as usize) * 40 + (lcg(&mut seed) as usize);
            let mut data = Vec::with_capacity(len);
            while data.len() < len {
                let choice = lcg(&mut seed) % 4;
                match choice {
                    0 => data.push(lcg(&mut seed)),
                    1 => {
                        let run_len = 1 + (lcg(&mut seed) as usize % 40);
                        let b = lcg(&mut seed);
                        for _ in 0..run_len {
                            data.push(b);
                        }
                    }
                    2 => {
                        let unit_len = 1 + (lcg(&mut seed) as usize % 12);
                        let unit: Vec<u8> = (0..unit_len).map(|_| lcg(&mut seed)).collect();
                        let reps = 1 + (lcg(&mut seed) as usize % 8);
                        for _ in 0..reps {
                            data.extend_from_slice(&unit);
                        }
                    }
                    _ => data.push(0),
                }
            }
            data.truncate(len);
            roundtrip(&data);
        }
    }

    // ---- 境界ケースのユニットテスト ----

    #[test]
    fn length_code_boundaries() {
        assert_eq!(length_code(3), (257, 0, 0));
        assert_eq!(length_code(258), (285, 0, 0));
        let (sym, extra, _) = length_code(257);
        assert_eq!(sym, 284);
        assert_eq!(extra, 5);
    }

    #[test]
    fn dist_code_boundaries() {
        assert_eq!(dist_code(1), (0, 0, 0));
        let (sym, extra, val) = dist_code(32768);
        assert_eq!(sym, 29);
        assert_eq!(extra, 13);
        assert_eq!(val, 8191);
    }

    #[test]
    fn build_lengths_single_symbol() {
        let mut freq = [0u64; 10];
        freq[3] = 42;
        let lengths = build_lengths(&freq, 15);
        assert_eq!(lengths[3], 1);
        assert!(lengths.iter().enumerate().all(|(i, &l)| i == 3 || l == 0));
    }

    #[test]
    fn build_lengths_uniform_frequency() {
        let freq = [10u64; 8];
        let lengths = build_lengths(&freq, 15);
        // 均等頻度なら理想的には全シンボルが同じ符号長 (log2(8)=3) に近い
        assert!(lengths.iter().all(|&l| (1..=15).contains(&l)));
        assert_kraft_valid(&lengths, 15);
    }

    #[test]
    fn build_lengths_depth_limit_kicks_in() {
        // フィボナッチ的な頻度分布は Huffman 木を極端に偏らせ、深さが
        // max_length を超えやすい（symbol 数が十分あれば深さ制限が必ず効く）。
        let mut fib = vec![1u64, 1u64];
        for i in 2..24 {
            let next = fib[i - 1] + fib[i - 2];
            fib.push(next);
        }
        let lengths = build_lengths(&fib, 15);
        assert!(
            lengths.iter().all(|&l| l <= 15),
            "深さ制限を超えている符号長がある"
        );
        assert_kraft_valid(&lengths, 15);

        // コード長符号アルファベット (RFC1951 上限 7) でも同様に効く
        let fib19 = &fib[..19];
        let cl_lengths = build_lengths(fib19, 7);
        assert!(cl_lengths.iter().all(|&l| l <= 7));
        assert_kraft_valid(&cl_lengths, 7);
    }

    /// Kraft 不等式 (sum 2^-len <= 1) を満たすことを確認するヘルパ。
    fn assert_kraft_valid(lengths: &[u8], max_len: u8) {
        let mut sum: u64 = 0;
        for &l in lengths {
            if l > 0 {
                sum += 1u64 << (max_len - l);
            }
        }
        assert!(sum <= 1u64 << max_len, "Kraft 不等式違反: sum={sum}");
    }

    #[test]
    fn all_symbols_equal_frequency_roundtrip() {
        // 0..=255 を同数ずつ、疑似ランダムに並べて LZ 一致をほぼ起こさせない
        let mut seed = 2024u64;
        let mut data = Vec::new();
        for _ in 0..20 {
            let mut chunk: Vec<u8> = (0..=255u8).collect();
            for i in (1..chunk.len()).rev() {
                let j = (lcg(&mut seed) as usize) % (i + 1);
                chunk.swap(i, j);
            }
            data.extend_from_slice(&chunk);
        }
        roundtrip(&data);
    }

    #[test]
    fn zlib_header_is_valid() {
        let z = zlib_compress(b"hello world");
        assert_eq!(z[0] & 0x0f, 8);
        assert_eq!(((z[0] as u16) << 8 | z[1] as u16) % 31, 0);
    }
}
