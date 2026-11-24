//! ChaCha20 (RFC 8439) ブロック関数 + カウンタモードのストリーム暗号。
//!
//! nonce は 12 バイト・カウンタは 32bit LE という RFC 8439 形式（Bernstein 原案の
//! 8 バイト nonce/64bit カウンタ形式ではない）。`pak.rs` の NPK1 暗号化 pak が使う。
//! XOR ベースのストリーム暗号なので暗号化と復号は同一演算 (`apply_keystream`) で行える。

const CONSTANTS: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]; // "expand 32-byte k"

#[inline(always)]
fn add4(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    [
        a[0].wrapping_add(b[0]),
        a[1].wrapping_add(b[1]),
        a[2].wrapping_add(b[2]),
        a[3].wrapping_add(b[3]),
    ]
}

#[inline(always)]
fn xor_rot4(a: [u32; 4], b: [u32; 4], r: u32) -> [u32; 4] {
    [
        (a[0] ^ b[0]).rotate_left(r),
        (a[1] ^ b[1]).rotate_left(r),
        (a[2] ^ b[2]).rotate_left(r),
        (a[3] ^ b[3]).rotate_left(r),
    ]
}

#[inline(always)]
fn quarter_round_rows(r0: &mut [u32; 4], r1: &mut [u32; 4], r2: &mut [u32; 4], r3: &mut [u32; 4]) {
    *r0 = add4(*r0, *r1);
    *r3 = xor_rot4(*r3, *r0, 16);
    *r2 = add4(*r2, *r3);
    *r1 = xor_rot4(*r1, *r2, 12);
    *r0 = add4(*r0, *r1);
    *r3 = xor_rot4(*r3, *r0, 8);
    *r2 = add4(*r2, *r3);
    *r1 = xor_rot4(*r1, *r2, 7);
}

#[inline(always)]
fn init_base(key: &[u8; 32], nonce: &[u8; 12]) -> [[u32; 4]; 4] {
    [
        CONSTANTS,
        [
            u32::from_le_bytes(key[0..4].try_into().unwrap()),
            u32::from_le_bytes(key[4..8].try_into().unwrap()),
            u32::from_le_bytes(key[8..12].try_into().unwrap()),
            u32::from_le_bytes(key[12..16].try_into().unwrap()),
        ],
        [
            u32::from_le_bytes(key[16..20].try_into().unwrap()),
            u32::from_le_bytes(key[20..24].try_into().unwrap()),
            u32::from_le_bytes(key[24..28].try_into().unwrap()),
            u32::from_le_bytes(key[28..32].try_into().unwrap()),
        ],
        [
            0,
            u32::from_le_bytes(nonce[0..4].try_into().unwrap()),
            u32::from_le_bytes(nonce[4..8].try_into().unwrap()),
            u32::from_le_bytes(nonce[8..12].try_into().unwrap()),
        ],
    ]
}

#[inline(always)]
fn block_rows(base: &[[u32; 4]; 4], counter: u32) -> [[u32; 4]; 4] {
    let i0 = base[0];
    let i1 = base[1];
    let i2 = base[2];
    let i3 = [counter, base[3][1], base[3][2], base[3][3]];
    let (mut r0, mut r1, mut r2, mut r3) = (i0, i1, i2, i3);
    for _ in 0..10 {
        quarter_round_rows(&mut r0, &mut r1, &mut r2, &mut r3);
        r1 = [r1[1], r1[2], r1[3], r1[0]];
        r2 = [r2[2], r2[3], r2[0], r2[1]];
        r3 = [r3[3], r3[0], r3[1], r3[2]];
        quarter_round_rows(&mut r0, &mut r1, &mut r2, &mut r3);
        r1 = [r1[3], r1[0], r1[1], r1[2]];
        r2 = [r2[2], r2[3], r2[0], r2[1]];
        r3 = [r3[1], r3[2], r3[3], r3[0]];
    }
    [
        add4(r0, i0),
        add4(r1, i1),
        add4(r2, i2),
        add4(r3, i3),
    ]
}

#[inline(always)]
fn rows_to_bytes(rows: &[[u32; 4]; 4]) -> [u8; 64] {
    let mut out = [0u8; 64];
    for (r, row) in rows.iter().enumerate() {
        for (c, &w) in row.iter().enumerate() {
            let p = (r * 4 + c) * 4;
            out[p..p + 4].copy_from_slice(&w.to_le_bytes());
        }
    }
    out
}

/// ChaCha20 ブロック関数 (RFC 8439 2.3)。1 ブロック = 64 バイトのキーストリームを生成する。
/// `key_check`（pak ヘッダの鍵検証値）が counter=0・nonce 全ゼロで直接呼ぶため公開している。
pub fn block(key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> [u8; 64] {
    let base = init_base(key, nonce);
    rows_to_bytes(&block_rows(&base, counter))
}

/// `data` を、ストリーム上のバイトオフセット `byte_offset` から始まる区間とみなして
/// キーストリームを XOR 適用する（暗号化・復号共通。ChaCha20 は XOR ベースのため）。
///
/// `byte_offset` はブロック境界に揃っていなくてよい（`byte_offset / 64` がブロック
/// カウンタ、`byte_offset % 64` がブロック内オフセットになる）。pak エントリの
/// 途中バイトから復号する `PakEntryReader` のシーク用に必要な性質。
///
/// カウンタは 32bit なので、扱えるストリーム長は `byte_offset` 起点から最大
/// 2^32 ブロック（256GiB）。ゲームアセット用途では十分。
pub fn apply_keystream(key: &[u8; 32], nonce: &[u8; 12], byte_offset: u64, data: &mut [u8]) {
    if data.is_empty() {
        return;
    }
    let base = init_base(key, nonce);
    let mut counter = (byte_offset / 64) as u32;
    let intra = (byte_offset % 64) as usize;
    let mut pos = 0usize;

    if intra != 0 {
        let ks = rows_to_bytes(&block_rows(&base, counter));
        let take = (64 - intra).min(data.len());
        for i in 0..take {
            data[i] ^= ks[intra + i];
        }
        pos += take;
        counter = counter.wrapping_add(1);
    }

    let (chunks, rest) = data[pos..].as_chunks_mut::<64>();
    for chunk in chunks {
        let rows = block_rows(&base, counter);
        for (r, row) in rows.iter().enumerate() {
            for (c, &ks_w) in row.iter().enumerate() {
                let p = (r * 4 + c) * 4;
                let w = u32::from_le_bytes(chunk[p..p + 4].try_into().unwrap()) ^ ks_w;
                chunk[p..p + 4].copy_from_slice(&w.to_le_bytes());
            }
        }
        counter = counter.wrapping_add(1);
    }

    if !rest.is_empty() {
        let ks = rows_to_bytes(&block_rows(&base, counter));
        for (d, &k) in rest.iter_mut().zip(&ks) {
            *d ^= k;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8439 §2.3.2 の既知ベクタ。
    /// key = 00:01:...:1f, nonce = 00:00:00:09:00:00:00:4a:00:00:00:00, counter = 1。
    /// OpenSSL 3.5 (`openssl enc -chacha20`, IV = counter(LE 4B) + nonce(12B)) で
    /// ゼロ 64 バイトを暗号化して独立に生成・確認した値（キーストリームそのもの）。
    #[test]
    fn rfc8439_section_2_3_2_block_vector() {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = i as u8;
        }
        let nonce: [u8; 12] = [
            0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x4a, 0x00, 0x00, 0x00, 0x00,
        ];
        let ks = block(&key, &nonce, 1);
        let expected: [u8; 64] = [
            0x10, 0xf1, 0xe7, 0xe4, 0xd1, 0x3b, 0x59, 0x15, 0x50, 0x0f, 0xdd, 0x1f, 0xa3, 0x20,
            0x71, 0xc4, 0xc7, 0xd1, 0xf4, 0xc7, 0x33, 0xc0, 0x68, 0x03, 0x04, 0x22, 0xaa, 0x9a,
            0xc3, 0xd4, 0x6c, 0x4e, 0xd2, 0x82, 0x64, 0x46, 0x07, 0x9f, 0xaa, 0x09, 0x14, 0xc2,
            0xd7, 0x05, 0xd9, 0x8b, 0x02, 0xa2, 0xb5, 0x12, 0x9c, 0xd1, 0xde, 0x16, 0x4e, 0xb9,
            0xcb, 0xd0, 0x83, 0xe8, 0xa2, 0x50, 0x3c, 0x4e,
        ];
        assert_eq!(ks, expected);
    }

    /// RFC 8439 §2.4.2 の既知ベクタ（全文暗号化）。
    /// key = 00:01:...:1f, nonce = 00:00:00:00:00:00:00:4a:00:00:00:00, 初期 counter = 1。
    /// ciphertext は上と同じ方法（OpenSSL 独立実装）で確認済み。
    #[test]
    fn rfc8439_section_2_4_2_encryption_vector() {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = i as u8;
        }
        let nonce: [u8; 12] = [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4a, 0x00, 0x00, 0x00, 0x00,
        ];
        let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let mut buf = plaintext.to_vec();
        // 初期 counter = 1 == バイトオフセット 64 (= 1 ブロック目) から開始するのと同義。
        apply_keystream(&key, &nonce, 64, &mut buf);

        let expected: [u8; 114] = [
            0x6e, 0x2e, 0x35, 0x9a, 0x25, 0x68, 0xf9, 0x80, 0x41, 0xba, 0x07, 0x28, 0xdd, 0x0d,
            0x69, 0x81, 0xe9, 0x7e, 0x7a, 0xec, 0x1d, 0x43, 0x60, 0xc2, 0x0a, 0x27, 0xaf, 0xcc,
            0xfd, 0x9f, 0xae, 0x0b, 0xf9, 0x1b, 0x65, 0xc5, 0x52, 0x47, 0x33, 0xab, 0x8f, 0x59,
            0x3d, 0xab, 0xcd, 0x62, 0xb3, 0x57, 0x16, 0x39, 0xd6, 0x24, 0xe6, 0x51, 0x52, 0xab,
            0x8f, 0x53, 0x0c, 0x35, 0x9f, 0x08, 0x61, 0xd8, 0x07, 0xca, 0x0d, 0xbf, 0x50, 0x0d,
            0x6a, 0x61, 0x56, 0xa3, 0x8e, 0x08, 0x8a, 0x22, 0xb6, 0x5e, 0x52, 0xbc, 0x51, 0x4d,
            0x16, 0xcc, 0xf8, 0x06, 0x81, 0x8c, 0xe9, 0x1a, 0xb7, 0x79, 0x37, 0x36, 0x5a, 0xf9,
            0x0b, 0xbf, 0x74, 0xa3, 0x5b, 0xe6, 0xb4, 0x0b, 0x8e, 0xed, 0xf2, 0x78, 0x5e, 0x42,
            0x87, 0x4d,
        ];
        assert_eq!(buf.as_slice(), &expected[..]);

        // XOR は自己逆演算: 同じキーストリームをもう一度当てれば平文に戻る。
        apply_keystream(&key, &nonce, 64, &mut buf);
        assert_eq!(buf, plaintext);
    }

    #[test]
    fn counter_seek_block_aligned_split_matches_single_call() {
        let key = [7u8; 32];
        let nonce = [3u8; 12];
        let total = 64 * 5; // 5 ブロック分
        let mut seed = 42u64;
        let mut lcg = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as u8
        };
        let data: Vec<u8> = (0..total).map(|_| lcg()).collect();

        let mut whole = data.clone();
        apply_keystream(&key, &nonce, 0, &mut whole);

        // ブロック境界 (64*2) で 2 回に分けて適用する。
        let mut split = data.clone();
        let (first, rest) = split.split_at_mut(128);
        apply_keystream(&key, &nonce, 0, first);
        apply_keystream(&key, &nonce, 128, rest);

        assert_eq!(whole, split);
    }

    #[test]
    fn counter_seek_non_aligned_split_matches_single_call() {
        let key = [9u8; 32];
        let nonce = [11u8; 12];
        let total = 300usize; // ブロック境界を跨ぐ半端な長さ
        let mut seed = 999u64;
        let mut lcg = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as u8
        };
        let data: Vec<u8> = (0..total).map(|_| lcg()).collect();

        let mut whole = data.clone();
        apply_keystream(&key, &nonce, 0, &mut whole);

        // ブロックの途中 (17, 64*2+30) で分割する非アライン分割。
        for split_at in [17usize, 64 + 5, 128 + 30, 250] {
            let mut split = data.clone();
            let (first, rest) = split.split_at_mut(split_at);
            apply_keystream(&key, &nonce, 0, first);
            apply_keystream(&key, &nonce, split_at as u64, rest);
            assert_eq!(whole, split, "split_at={split_at}");
        }
    }

    #[test]
    fn encryption_is_its_own_inverse() {
        let key = [5u8; 32];
        let nonce = [1u8; 12];
        let plain = b"noiria pak roundtrip test data, arbitrary length!!";
        let mut buf = plain.to_vec();
        apply_keystream(&key, &nonce, 0, &mut buf);
        assert_ne!(buf.as_slice(), &plain[..]);
        apply_keystream(&key, &nonce, 0, &mut buf);
        assert_eq!(buf.as_slice(), &plain[..]);
    }
}
