//! ChaCha20 (RFC 8439) ブロック関数 + カウンタモードのストリーム暗号。
//!
//! nonce は 12 バイト・カウンタは 32bit LE という RFC 8439 形式（Bernstein 原案の
//! 8 バイト nonce/64bit カウンタ形式ではない）。`pak.rs` の NPK1 暗号化 pak が使う。
//! XOR ベースのストリーム暗号なので暗号化と復号は同一演算 (`apply_keystream`) で行える。

const CONSTANTS: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]; // "expand 32-byte k"

/// RFC 8439 3.1 のクォーターラウンド。
fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] ^= s[a];
    s[d] = s[d].rotate_left(16);

    s[c] = s[c].wrapping_add(s[d]);
    s[b] ^= s[c];
    s[b] = s[b].rotate_left(12);

    s[a] = s[a].wrapping_add(s[b]);
    s[d] ^= s[a];
    s[d] = s[d].rotate_left(8);

    s[c] = s[c].wrapping_add(s[d]);
    s[b] ^= s[c];
    s[b] = s[b].rotate_left(7);
}

/// ChaCha20 ブロック関数 (RFC 8439 2.3)。1 ブロック = 64 バイトのキーストリームを生成する。
/// `key_check`（pak ヘッダの鍵検証値）が counter=0・nonce 全ゼロで直接呼ぶため公開している。
pub fn block(key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> [u8; 64] {
    let mut state = [0u32; 16];
    state[0..4].copy_from_slice(&CONSTANTS);
    for i in 0..8 {
        state[4 + i] = u32::from_le_bytes(key[i * 4..i * 4 + 4].try_into().unwrap());
    }
    state[12] = counter;
    for i in 0..3 {
        state[13 + i] = u32::from_le_bytes(nonce[i * 4..i * 4 + 4].try_into().unwrap());
    }
    let initial = state;

    for _ in 0..10 {
        // 列ラウンド
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        // 対角ラウンド
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }

    let mut out = [0u8; 64];
    for i in 0..16 {
        let word = state[i].wrapping_add(initial[i]);
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
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
    let mut counter = (byte_offset / 64) as u32;
    let mut intra = (byte_offset % 64) as usize;
    let mut pos = 0usize;
    while pos < data.len() {
        let ks = block(key, nonce, counter);
        let avail = 64 - intra;
        let take = avail.min(data.len() - pos);
        for i in 0..take {
            data[pos + i] ^= ks[intra + i];
        }
        pos += take;
        counter = counter.wrapping_add(1);
        intra = 0;
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
