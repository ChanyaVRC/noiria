//! Ogg コンテナ (RFC 3533) の自作パーサ。ページ構造・CRC32・パケット再構築。
//! `codec/vorbis` レイヤが使う。チェイン/複数論理ストリームの多重化には対応せず、
//! ファイル中で最初に現れる論理ストリーム（先頭ページの serial）のみを対象とする。

use anyhow::{ensure, Context, Result};

/// Ogg の CRC32: 多項式 0x04C11DB7、反射なし、初期値 0、最終 XOR なし。
/// (zlib/PNG の CRC32 とは多項式・反射方向が異なる別物)
const CRC_POLY: u32 = 0x04c1_1db7;

const fn build_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut reg = (i as u32) << 24;
        let mut j = 0;
        while j < 8 {
            if reg & 0x8000_0000 != 0 {
                reg = (reg << 1) ^ CRC_POLY;
            } else {
                reg <<= 1;
            }
            j += 1;
        }
        table[i] = reg;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = build_crc_table();

fn crc32(data: &[u8]) -> u32 {
    let mut reg: u32 = 0;
    for &b in data {
        reg = (reg << 8) ^ CRC_TABLE[(((reg >> 24) as u8) ^ b) as usize];
    }
    reg
}

/// パースされた 1 ページ。`body` は元バッファへの参照。
pub struct Page<'a> {
    pub header_type: u8,
    pub granule_position: i64,
    pub serial: u32,
    pub seq: u32,
    /// セグメントテーブル (lacing values)。
    pub segments: Vec<u8>,
    pub body: &'a [u8],
}

impl<'a> Page<'a> {
    pub fn continued(&self) -> bool {
        self.header_type & 0x01 != 0
    }
    pub fn bos(&self) -> bool {
        self.header_type & 0x02 != 0
    }
    pub fn eos(&self) -> bool {
        self.header_type & 0x04 != 0
    }
}

fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn i64le(b: &[u8], at: usize) -> i64 {
    i64::from_le_bytes([
        b[at],
        b[at + 1],
        b[at + 2],
        b[at + 3],
        b[at + 4],
        b[at + 5],
        b[at + 6],
        b[at + 7],
    ])
}

/// `data` の先頭にある 1 ページをパースする。戻り値はページと消費バイト数。
pub fn parse_page(data: &[u8]) -> Result<(Page<'_>, usize)> {
    ensure!(data.len() >= 27, "Ogg ページヘッダが短すぎます");
    ensure!(&data[0..4] == b"OggS", "Ogg キャプチャパターンがありません");
    let version = data[4];
    ensure!(version == 0, "非対応の Ogg バージョン: {version}");
    let header_type = data[5];
    let granule_position = i64le(data, 6);
    let serial = u32le(data, 14);
    let seq = u32le(data, 18);
    let crc_stored = u32le(data, 22);
    let page_segments = data[26] as usize;
    ensure!(
        data.len() >= 27 + page_segments,
        "セグメントテーブルが途中で切れています"
    );
    let segments: Vec<u8> = data[27..27 + page_segments].to_vec();
    let body_len: usize = segments.iter().map(|&s| s as usize).sum();
    let body_start = 27 + page_segments;
    ensure!(
        data.len() >= body_start + body_len,
        "Ogg ページ本体が途中で切れています"
    );
    let body = &data[body_start..body_start + body_len];

    // CRC は「CRC フィールドを 0 にしたページ全体」に対して計算する。
    let total = body_start + body_len;
    let mut crc_buf = data[0..total].to_vec();
    crc_buf[22] = 0;
    crc_buf[23] = 0;
    crc_buf[24] = 0;
    crc_buf[25] = 0;
    let computed = crc32(&crc_buf);
    ensure!(computed == crc_stored, "Ogg ページの CRC32 が一致しません");

    Ok((
        Page {
            header_type,
            granule_position,
            serial,
            seq,
            segments,
            body,
        },
        total,
    ))
}

/// ファイル全体から、最初に現れる論理ストリーム（先頭ページの serial）のパケット列を
/// 再構築する。戻り値は (serial, パケット列, 最終ページの granule_position)。
pub fn extract_first_stream_packets(data: &[u8]) -> Result<(u32, Vec<Vec<u8>>, i64)> {
    let mut pos = 0usize;
    let mut target_serial: Option<u32> = None;
    let mut packets: Vec<Vec<u8>> = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut have_pending = false;
    let mut last_granule = 0i64;

    while pos < data.len() {
        let (page, consumed) = parse_page(&data[pos..])?;
        pos += consumed;

        if target_serial.is_none() {
            ensure!(
                page.bos(),
                "最初のページが BOS ではありません（複数論理ストリームの先頭欠落）"
            );
            target_serial = Some(page.serial);
        }
        if Some(page.serial) != target_serial {
            // 多重化された別ストリームは V1 スコープ外なので無視する。
            continue;
        }

        if !page.continued() {
            // 前のパケットが宙ぶらりんのままなら破棄する（壊れたストリーム）。
            if have_pending {
                pending.clear();
                have_pending = false;
            }
        } else if !have_pending {
            // continued フラグがあるのに継続対象がない: 先頭ページ欠落等。無視して読み進める。
        }

        let mut off = 0usize;
        for (i, &seg) in page.segments.iter().enumerate() {
            let chunk = &page.body[off..off + seg as usize];
            off += seg as usize;
            pending.extend_from_slice(chunk);
            have_pending = true;
            let is_last_segment = i == page.segments.len() - 1;
            if (seg as usize) < 255 {
                // パケット完結。
                packets.push(std::mem::take(&mut pending));
                have_pending = false;
            } else if is_last_segment {
                // 255 で終わるセグメントはページ境界をまたいで継続する可能性がある。
                // 次ページの continued フラグを見て判断するのでここでは何もしない。
            }
        }

        if page.eos() {
            last_granule = page.granule_position;
            break;
        }
        last_granule = page.granule_position;
    }
    // ストリーム末尾で宙ぶらりんのパケットが残っていたら、切り詰められたものとして採用する。
    if have_pending && !pending.is_empty() {
        packets.push(pending);
    }

    let serial = target_serial.context("Ogg ページが見つかりません")?;
    Ok((serial, packets, last_granule))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 ページ分のバイト列を手組みする（CRC は自動計算）。
    fn build_page(
        header_type: u8,
        granule: i64,
        serial: u32,
        seq: u32,
        segments: &[u8],
        body: &[u8],
    ) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"OggS");
        v.push(0); // version
        v.push(header_type);
        v.extend_from_slice(&granule.to_le_bytes());
        v.extend_from_slice(&serial.to_le_bytes());
        v.extend_from_slice(&seq.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes()); // crc placeholder
        v.push(segments.len() as u8);
        v.extend_from_slice(segments);
        v.extend_from_slice(body);
        let crc = crc32(&v);
        v[22..26].copy_from_slice(&crc.to_le_bytes());
        v
    }

    #[test]
    fn crc32_known_value() {
        // "hello" / "Wikipedia" の Ogg 式 CRC32 (多項式 0x04C11DB7, 反射なし)。
        // Python で独立に同じ多項式・アルゴリズムを再実装して検算した値。
        assert_eq!(crc32(b"hello"), 0xa1de215e);
        assert_eq!(crc32(b"Wikipedia"), 0x823dd7b1);
    }

    #[test]
    fn parses_single_segment_page() {
        let body = b"hello world";
        let page_bytes = build_page(0x02, -1, 1234, 0, &[body.len() as u8], body);
        let (page, consumed) = parse_page(&page_bytes).unwrap();
        assert_eq!(consumed, page_bytes.len());
        assert_eq!(page.serial, 1234);
        assert!(page.bos());
        assert!(!page.eos());
        assert!(!page.continued());
        assert_eq!(page.body, body);
    }

    #[test]
    fn rejects_bad_crc() {
        let body = b"hello world";
        let mut page_bytes = build_page(0x02, -1, 1234, 0, &[body.len() as u8], body);
        let last = page_bytes.len() - 1;
        page_bytes[last] ^= 0xff;
        assert!(parse_page(&page_bytes).is_err());
    }

    #[test]
    fn rejects_bad_capture_pattern() {
        let mut page_bytes = build_page(0x02, -1, 1234, 0, &[3], b"abc");
        page_bytes[0] = b'X';
        assert!(parse_page(&page_bytes).is_err());
    }

    #[test]
    fn reconstructs_single_packet_single_page() {
        let body = b"packet-one";
        let page = build_page(0x02, 0, 42, 0, &[body.len() as u8], body);
        let (serial, packets, _) = extract_first_stream_packets(&page).unwrap();
        assert_eq!(serial, 42);
        assert_eq!(packets, vec![body.to_vec()]);
    }

    #[test]
    fn reconstructs_multiple_packets_on_one_page() {
        // 2 パケット: "ab" (len 2) と "cde" (len 3)
        let mut body = Vec::new();
        body.extend_from_slice(b"ab");
        body.extend_from_slice(b"cde");
        let page = build_page(0x02, 0, 7, 0, &[2, 3], &body);
        let (_, packets, _) = extract_first_stream_packets(&page).unwrap();
        assert_eq!(packets, vec![b"ab".to_vec(), b"cde".to_vec()]);
    }

    #[test]
    fn reconstructs_packet_spanning_two_pages() {
        // 1 パケットが 255 + 10 バイトで 2 ページにまたがる (lacing 255 は継続を示す)。
        let part1 = vec![0xAAu8; 255];
        let part2 = vec![0xBBu8; 10];
        let page1 = build_page(0x02, 0, 9, 0, &[255], &part1);
        let page2 = build_page(0x01, 1, 9, 1, &[10], &part2);
        let mut all = page1.clone();
        all.extend_from_slice(&page2);
        let (_, packets, granule) = extract_first_stream_packets(&all).unwrap();
        assert_eq!(packets.len(), 1);
        let mut expected = part1;
        expected.extend_from_slice(&part2);
        assert_eq!(packets[0], expected);
        assert_eq!(granule, 1);
    }

    #[test]
    fn exact_multiple_of_255_terminates_with_zero_lacing() {
        // パケット長がちょうど 255 の倍数の場合、終端は 0 のラッキング値。
        let part = vec![0xCCu8; 255];
        let page = build_page(0x02, 0, 5, 0, &[255, 0], &part);
        let (_, packets, _) = extract_first_stream_packets(&page).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0], part);
    }

    #[test]
    fn ignores_pages_from_other_serial() {
        let body1 = b"stream-a";
        let body2 = b"stream-b";
        let page1 = build_page(0x02, 0, 1, 0, &[body1.len() as u8], body1);
        let page2 = build_page(0x00, 0, 2, 0, &[body2.len() as u8], body2);
        let mut all = page1;
        all.extend_from_slice(&page2);
        let (serial, packets, _) = extract_first_stream_packets(&all).unwrap();
        assert_eq!(serial, 1);
        assert_eq!(packets, vec![body1.to_vec()]);
    }
    // --- 破壊 fuzz（R1）: Ogg/Vorbis ---

    /// 全ページの CRC を書き直す（崩れたページ構造の手前まで）。CRC 検査で最初のページが弾かれ続けて
    /// Vorbis 本体まで届かないのを避ける。
    fn refix_crcs(d: &mut [u8]) {
        let mut pos = 0usize;
        while pos + 27 <= d.len() && &d[pos..pos + 4] == b"OggS" {
            let nseg = d[pos + 26] as usize;
            if pos + 27 + nseg > d.len() {
                break;
            }
            let body: usize = d[pos + 27..pos + 27 + nseg]
                .iter()
                .map(|&s| s as usize)
                .sum();
            let total = 27 + nseg + body;
            if pos + total > d.len() {
                break;
            }
            d[pos + 22..pos + 26].fill(0);
            let crc = crc32(&d[pos..pos + total]);
            d[pos + 22..pos + 26].copy_from_slice(&crc.to_le_bytes());
            pos += total;
        }
    }

    /// 実ファイルのヘッダ 3 パケット + 先頭の音声 `audio_packets` パケットだけの小さな Ogg を作る
    /// （全体を回すと 1 件が数百 ms かかり、数千件の fuzz に向かない）。
    fn trimmed_stream(full: &[u8], audio_packets: usize) -> Vec<u8> {
        let (serial, packets, _) = extract_first_stream_packets(full).unwrap();
        let lacing = |p: &[u8]| -> Vec<u8> {
            let mut v = vec![255u8; p.len() / 255];
            v.push((p.len() % 255) as u8);
            v
        };
        let mut out = Vec::new();
        for (seq, p) in packets[..3].iter().enumerate() {
            let flag = if seq == 0 { 0x02 } else { 0 };
            out.extend(build_page(flag, 0, serial, seq as u32, &lacing(p), p));
        }
        let (mut segs, mut body) = (Vec::new(), Vec::new());
        for p in &packets[3..3 + audio_packets] {
            segs.extend(lacing(p));
            body.extend_from_slice(p);
        }
        out.extend(build_page(0x04, 0, serial, 3, &segs, &body));
        out
    }

    #[test]
    fn fuzz_corrupted_ogg_vorbis_never_panics_or_overallocates() {
        let mono = include_bytes!("../../tests/vectors_audio/mono2_abashed_en.ogg");
        let stereo =
            include_bytes!("../../tests/vectors_audio/stereo1_kalmykia_anthem_trimmed.ogg");
        for (i, (full, audio, cases)) in [(&mono[..], 10usize, 1000usize), (&stereo[..], 6, 1000)]
            .into_iter()
            .enumerate()
        {
            let base = trimmed_stream(full, audio);
            assert!(
                crate::codec::vorbis::decode(&base).is_ok(),
                "基準入力がデコードできる"
            );
            crate::test_support::fuzz_with(
                "ogg/vorbis",
                0x8888 + i as u64,
                cases,
                |rng| {
                    let mut d = crate::test_support::mutate(&base, rng);
                    if rng.below(10) != 0 {
                        refix_crcs(&mut d);
                    }
                    d
                },
                |d| {
                    let _ = crate::codec::vorbis::decode(d);
                    let _ = extract_first_stream_packets(d);
                },
            );
        }
    }
}
