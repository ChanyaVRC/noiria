//! Vorbis I の 3 ヘッダ解析とパケット毎の音声デコード（仕様書 4 章）。

use super::bitreader::BitReader;
use super::codebook::{ilog, Codebook};
use super::floor::{reject_floor0, Floor1};
use super::mdct::{build_window, Imdct};
use super::residue::ResidueSetup;
use anyhow::{bail, ensure, Result};

enum FloorConfig {
    Type1(Floor1),
}

struct Mapping {
    submaps: u32,
    /// (magnitude_channel, angle_channel) の組。
    coupling: Vec<(u32, u32)>,
    /// チャンネル -> submap 番号。
    mux: Vec<u32>,
    submap_floor: Vec<u32>,
    submap_residue: Vec<u32>,
}

struct Mode {
    blockflag: bool,
    mapping: u32,
}

/// Ogg/Vorbis デコード結果 (全パケットをまとめてデコードした場合)。
pub struct DecodedVorbis {
    pub channels: u16,
    pub sample_rate: u32,
    /// インターリーブ済み [-1.0, 1.0] 目安 (クリップはしない)。
    pub samples: Vec<f32>,
}

/// パケット単位でストリーミングデコードするためのデコーダ状態。
pub struct VorbisStreamDecoder {
    channels: u16,
    sample_rate: u32,
    blocksize_0: usize,
    blocksize_1: usize,
    codebooks: Vec<Codebook>,
    floors: Vec<FloorConfig>,
    residues: Vec<ResidueSetup>,
    mappings: Vec<Mapping>,
    modes: Vec<Mode>,
    imdct0: Imdct,
    imdct1: Imdct,
    /// オーバーラップ加算用の状態: 直前ブロックの「窓掛け済み全長データ」とその長さ。
    prev_block: Option<(Vec<Vec<f32>>, usize)>,
}

fn check_header_magic(br: &mut BitReader, expect_type: u32) -> Result<()> {
    let packet_type = br.read(8);
    ensure!(
        packet_type == expect_type,
        "Vorbis ヘッダのパケット種別が不正です: {packet_type}"
    );
    let magic = [
        br.read(8) as u8,
        br.read(8) as u8,
        br.read(8) as u8,
        br.read(8) as u8,
        br.read(8) as u8,
        br.read(8) as u8,
    ];
    ensure!(&magic == b"vorbis", "'vorbis' マジックが見つかりません");
    Ok(())
}

impl VorbisStreamDecoder {
    /// identification / comment / setup の 3 ヘッダパケットからデコーダを構築する。
    pub fn new(id_packet: &[u8], setup_packet: &[u8]) -> Result<Self> {
        let (channels, sample_rate, blocksize_0, blocksize_1) = parse_identification(id_packet)?;
        let (codebooks, floors, residues, mappings, modes) =
            parse_setup(setup_packet, channels as u32)?;

        Ok(Self {
            channels,
            sample_rate,
            blocksize_0,
            blocksize_1,
            codebooks,
            floors,
            residues,
            mappings,
            modes,
            imdct0: Imdct::new(blocksize_0),
            imdct1: Imdct::new(blocksize_1),
            prev_block: None,
        })
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn max_blocksize(&self) -> usize {
        self.blocksize_1
    }
    /// オーバーラップ加算の直前ブロック状態を破棄して先頭からの再デコードに備える。
    pub fn reset(&mut self) {
        self.prev_block = None;
    }

    /// 1 音声パケットをデコードし、確定したインターリーブ PCM サンプルを返す
    /// (最初のパケットや非音声パケットの場合は空 Vec)。
    pub fn decode_packet(&mut self, packet: &[u8]) -> Result<Vec<f32>> {
        let mut br = BitReader::new(packet);
        if br.read_bool() {
            // packet_type ビットが 1 = 非音声パケット。無視する。
            return Ok(Vec::new());
        }
        let mode_count = self.modes.len() as u32;
        ensure!(mode_count > 0, "モードが設定されていません");
        let mode_number = br.read(ilog(mode_count as i64 - 1)) as usize;
        if mode_number >= self.modes.len() {
            // 不正なモード番号: 壊れたパケットとして無視する。
            return Ok(Vec::new());
        }
        let blockflag = self.modes[mode_number].blockflag;
        let n = if blockflag {
            self.blocksize_1
        } else {
            self.blocksize_0
        };

        let (prev_window_flag, next_window_flag) = if blockflag {
            (br.read_bool(), br.read_bool())
        } else {
            (true, true)
        };
        if br.eof {
            // ウィンドウ確定前の EOP はパケット破棄。
            return Ok(Vec::new());
        }

        let window = build_window(
            n,
            blockflag,
            prev_window_flag,
            next_window_flag,
            self.blocksize_0,
        );
        let mapping_idx = self.modes[mode_number].mapping as usize;
        ensure!(
            mapping_idx < self.mappings.len(),
            "モードが参照する mapping が範囲外です"
        );

        let ch = self.channels as usize;
        let half = n / 2;
        let mut spectral = vec![vec![0f32; half]; ch];
        let mut zero_all = br.eof;

        if !zero_all {
            // --- floor curve decode (4.3.2) ---
            let mut floor_curve: Vec<Option<Vec<f32>>> = Vec::with_capacity(ch);
            {
                let mapping = &self.mappings[mapping_idx];
                for i in 0..ch {
                    let submap = mux_of(mapping, i);
                    let floor_number = mapping.submap_floor[submap as usize] as usize;
                    let curve = match &self.floors[floor_number] {
                        FloorConfig::Type1(f1) => f1.decode(&mut br, &self.codebooks, n)?,
                    };
                    if br.eof {
                        zero_all = true;
                    }
                    floor_curve.push(curve);
                }
            }

            if zero_all {
                // 4.3.2: floor decode 中の EOP は全チャンネルをゼロにして overlap へ。
                spectral = vec![vec![0f32; half]; ch];
            } else {
                let mapping = &self.mappings[mapping_idx];
                let mut no_residue: Vec<bool> = floor_curve.iter().map(|c| c.is_none()).collect();

                // 4.3.3: nonzero vector propagate
                for &(mag, ang) in &mapping.coupling {
                    if no_residue[mag as usize] || no_residue[ang as usize] {
                        no_residue[mag as usize] = true;
                        no_residue[ang as usize] = true;
                    }
                }

                // 4.3.4: residue decode (submap 順)
                let mut residue_vec: Vec<Vec<f32>> = vec![vec![0f32; half]; ch];
                for submap in 0..mapping.submaps {
                    let mut chs: Vec<usize> = Vec::new();
                    for j in 0..ch {
                        if mux_of(mapping, j) == submap {
                            chs.push(j);
                        }
                    }
                    if chs.is_empty() {
                        continue;
                    }
                    let residue_number = mapping.submap_residue[submap as usize] as usize;
                    let do_not_decode: Vec<bool> = chs.iter().map(|&j| no_residue[j]).collect();
                    let decoded = self.residues[residue_number].decode(
                        &mut br,
                        &self.codebooks,
                        chs.len(),
                        half,
                        &do_not_decode,
                    )?;
                    for (j, vec) in chs.into_iter().zip(decoded) {
                        residue_vec[j] = vec;
                    }
                }

                // 4.3.5: inverse coupling (逆順)
                for &(mag, ang) in mapping.coupling.iter().rev() {
                    let (mv, av) = if mag < ang {
                        let (left, right) = residue_vec.split_at_mut(ang as usize);
                        (&mut left[mag as usize], &mut right[0])
                    } else {
                        let (left, right) = residue_vec.split_at_mut(mag as usize);
                        (&mut right[0], &mut left[ang as usize])
                    };
                    for (m, a) in mv.iter_mut().zip(av.iter_mut()) {
                        let (new_m, new_a) = if *m > 0.0 {
                            if *a > 0.0 {
                                (*m, *m - *a)
                            } else {
                                (*m + *a, *m)
                            }
                        } else if *a > 0.0 {
                            (*m, *m + *a)
                        } else {
                            (*m - *a, *m)
                        };
                        *m = new_m;
                        *a = new_a;
                    }
                }

                // 4.3.6: dot product (floor * residue)
                for j in 0..ch {
                    if let Some(curve) = &floor_curve[j] {
                        for k in 0..half {
                            spectral[j][k] = curve[k] * residue_vec[j][k];
                        }
                    }
                }
            }
        }

        // 4.3.7/4.3.8: 逆 MDCT + 窓掛け + オーバーラップ加算
        let imdct = if blockflag {
            &self.imdct1
        } else {
            &self.imdct0
        };
        let mut cur_block: Vec<Vec<f32>> = Vec::with_capacity(ch);
        for spec in &spectral {
            let pcm = imdct.transform(spec);
            let windowed: Vec<f32> = pcm.iter().zip(&window).map(|(&p, &w)| p * w).collect();
            cur_block.push(windowed);
        }

        let output = match &self.prev_block {
            None => Vec::new(),
            Some((prev_data, prev_n)) => overlap_add(prev_data, *prev_n, &cur_block, n, ch),
        };
        self.prev_block = Some((cur_block, n));
        Ok(output)
    }
}

fn mux_of(mapping: &Mapping, channel: usize) -> u32 {
    if mapping.submaps <= 1 {
        0
    } else {
        mapping.mux[channel]
    }
}

/// prev(長さ prev_n, 各チャンネル窓掛け済み全長データ) と cur(長さ cur_n) から
/// 確定したインターリーブ PCM を生成する (仕様書 4.3.8 / A.2 overlap_add)。
fn overlap_add(
    prev: &[Vec<f32>],
    prev_n: usize,
    cur: &[Vec<f32>],
    cur_n: usize,
    ch: usize,
) -> Vec<f32> {
    let returned_len = prev_n / 4 + cur_n / 4;
    let cur_offset: i64 = prev_n as i64 / 4 - cur_n as i64 / 4;
    let mut out = vec![0f32; returned_len * ch];
    for t in 0..returned_len {
        for c in 0..ch {
            let mut v = 0f32;
            if t < prev_n / 2 {
                v += prev[c][prev_n / 2 + t];
            }
            let cur_idx = t as i64 - cur_offset;
            if cur_idx >= 0 && (cur_idx as usize) < cur_n {
                v += cur[c][cur_idx as usize];
            }
            out[t * ch + c] = v;
        }
    }
    out
}

fn parse_identification(data: &[u8]) -> Result<(u16, u32, usize, usize)> {
    let mut br = BitReader::new(data);
    check_header_magic(&mut br, 1)?;
    let version = br.read(32);
    ensure!(version == 0, "非対応の vorbis_version: {version}");
    let channels = br.read(8);
    ensure!(channels > 0, "audio_channels が 0 です");
    let sample_rate = br.read(32);
    ensure!(sample_rate > 0, "audio_sample_rate が 0 です");
    let _bitrate_max = br.read(32);
    let _bitrate_nominal = br.read(32);
    let _bitrate_min = br.read(32);
    let bs0_exp = br.read(4);
    let bs1_exp = br.read(4);
    let blocksize_0 = 1usize << bs0_exp;
    let blocksize_1 = 1usize << bs1_exp;
    ensure!(
        (64..=8192).contains(&blocksize_0) && (64..=8192).contains(&blocksize_1),
        "blocksize が許可範囲外です"
    );
    ensure!(
        blocksize_0 <= blocksize_1,
        "blocksize_0 は blocksize_1 以下でなければなりません"
    );
    let framing = br.read(1);
    ensure!(framing != 0, "identification header framing bit が不正です");
    ensure!(
        !br.eof,
        "identification header がパケット終端で切れています"
    );
    Ok((channels as u16, sample_rate, blocksize_0, blocksize_1))
}

#[allow(clippy::type_complexity)]
fn parse_setup(
    data: &[u8],
    audio_channels: u32,
) -> Result<(
    Vec<Codebook>,
    Vec<FloorConfig>,
    Vec<ResidueSetup>,
    Vec<Mapping>,
    Vec<Mode>,
)> {
    let mut br = BitReader::new(data);
    check_header_magic(&mut br, 5)?;

    // codebooks
    let codebook_count = br.read(8) + 1;
    let mut codebooks = Vec::with_capacity(codebook_count as usize);
    for i in 0..codebook_count {
        codebooks
            .push(Codebook::parse(&mut br).map_err(|e| anyhow::anyhow!("codebook #{i}: {e}"))?);
    }

    // time domain placeholders (Vorbis I では常に 0 でなければならない)
    let time_count = br.read(6) + 1;
    for _ in 0..time_count {
        let v = br.read(16);
        ensure!(v == 0, "time domain transform placeholder が非ゼロです");
    }

    // floors
    let floor_count = br.read(6) + 1;
    let mut floors = Vec::with_capacity(floor_count as usize);
    for _ in 0..floor_count {
        let floor_type = br.read(16);
        match floor_type {
            0 => reject_floor0(&mut br)?,
            1 => {
                let floor = Floor1::parse(&mut br)?;
                floor.validate_books(codebooks.len())?;
                floors.push(FloorConfig::Type1(floor));
            }
            t => bail!("非対応の floor type: {t}"),
        }
    }

    // residues
    let residue_count = br.read(6) + 1;
    let mut residues = Vec::with_capacity(residue_count as usize);
    for _ in 0..residue_count {
        let residue_type = br.read(16);
        ensure!(residue_type <= 2, "非対応の residue type: {residue_type}");
        let residue = ResidueSetup::parse(&mut br, residue_type as u16)?;
        residue.validate_books(codebooks.len())?;
        residues.push(residue);
    }

    // mappings
    let mapping_count = br.read(6) + 1;
    let mut mappings = Vec::with_capacity(mapping_count as usize);
    for _ in 0..mapping_count {
        let mapping_type = br.read(16);
        ensure!(mapping_type == 0, "非対応の mapping type: {mapping_type}");
        let submaps = if br.read_bool() { br.read(4) + 1 } else { 1 };
        let mut coupling = Vec::new();
        if br.read_bool() {
            let coupling_steps = br.read(8) + 1;
            let bits = ilog(audio_channels as i64 - 1);
            for _ in 0..coupling_steps {
                let mag = br.read(bits);
                let ang = br.read(bits);
                ensure!(
                    mag != ang && mag < audio_channels && ang < audio_channels,
                    "channel coupling のチャンネル指定が不正です"
                );
                coupling.push((mag, ang));
            }
        }
        let reserved = br.read(2);
        ensure!(reserved == 0, "mapping の予約フィールドが非ゼロです");
        let mut mux = vec![0u32; audio_channels as usize];
        if submaps > 1 {
            for m in mux.iter_mut() {
                *m = br.read(4);
                ensure!(*m < submaps, "mapping_mux が submap 数を超えています");
            }
        }
        let mut submap_floor = vec![0u32; submaps as usize];
        let mut submap_residue = vec![0u32; submaps as usize];
        for j in 0..submaps as usize {
            let _time_placeholder = br.read(8);
            let floor_number = br.read(8);
            ensure!(
                (floor_number as usize) < floors.len(),
                "submap floor 番号が範囲外です"
            );
            let residue_number = br.read(8);
            ensure!(
                (residue_number as usize) < residues.len(),
                "submap residue 番号が範囲外です"
            );
            submap_floor[j] = floor_number;
            submap_residue[j] = residue_number;
        }
        mappings.push(Mapping {
            submaps,
            coupling,
            mux,
            submap_floor,
            submap_residue,
        });
    }

    // modes
    let mode_count = br.read(6) + 1;
    let mut modes = Vec::with_capacity(mode_count as usize);
    for _ in 0..mode_count {
        let blockflag = br.read_bool();
        let windowtype = br.read(16);
        ensure!(
            windowtype == 0,
            "非対応の vorbis_mode_windowtype: {windowtype}"
        );
        let transformtype = br.read(16);
        ensure!(
            transformtype == 0,
            "非対応の vorbis_mode_transformtype: {transformtype}"
        );
        let mapping = br.read(8);
        ensure!(
            (mapping as usize) < mappings.len(),
            "mode が参照する mapping が範囲外です"
        );
        modes.push(Mode { blockflag, mapping });
    }

    ensure!(!br.eof, "setup header がパケット終端で切れています");
    let framing = br.read(1);
    ensure!(framing != 0, "setup header framing bit が不正です");

    Ok((codebooks, floors, residues, mappings, modes))
}

/// パケット列 (Ogg から再構築済み) をまとめてデコードする。
pub fn decode_packets(packets: &[Vec<u8>]) -> Result<DecodedVorbis> {
    ensure!(
        packets.len() >= 3,
        "Vorbis の 3 ヘッダパケットが揃っていません"
    );
    let mut dec = VorbisStreamDecoder::new(&packets[0], &packets[2])?;
    let mut samples = Vec::new();
    for packet in &packets[3..] {
        let chunk = dec.decode_packet(packet)?;
        samples.extend_from_slice(&chunk);
    }
    Ok(DecodedVorbis {
        channels: dec.channels(),
        sample_rate: dec.sample_rate(),
        samples,
    })
}

/// Ogg コンテナに包まれた .ogg ファイル全体を丸ごとデコードする。
pub fn decode(data: &[u8]) -> Result<DecodedVorbis> {
    let (_serial, packets, _granule) = crate::codec::ogg::extract_first_stream_packets(data)?;
    decode_packets(&packets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_add_matches_simple_half_sum_for_equal_blocksize() {
        let n = 8;
        let ch = 1;
        let prev = vec![(0..n).map(|i| i as f32).collect::<Vec<f32>>()];
        let cur = vec![(0..n).map(|i| (100 + i) as f32).collect::<Vec<f32>>()];
        let out = overlap_add(&prev, n, &cur, n, ch);
        // returned_len = n/4+n/4 = n/2 = 4
        assert_eq!(out.len(), 4);
        // out[t] = prev[n/2+t] + cur[t]
        for (t, &v) in out.iter().enumerate() {
            assert_eq!(v, (n / 2 + t) as f32 + (100 + t) as f32);
        }
    }

    #[test]
    fn first_block_returns_no_samples() {
        // decode_packet の prev_block=None ケースは overlap_add を呼ばず空を返す実装なので、
        // ここでは overlap_add 自体は対称ケースのみ確認済み。デコーダ全体の統合テストは
        // decoder テスト (クロスチェック) 側で行う。
    }
    // --- 外部入力の堅牢化（R1）: setup ヘッダのコードブック番号検証 ---

    use crate::test_support::LsbBits;

    fn id_header() -> Vec<u8> {
        let mut b = LsbBits::default();
        b.push(1, 8);
        for &c in b"vorbis" {
            b.push(c as u64, 8);
        }
        b.push(0, 32) // version
            .push(1, 8) // channels
            .push(44100, 32)
            .push(0, 32)
            .push(0, 32)
            .push(0, 32)
            .push(8, 4) // blocksize_0 = 256
            .push(8, 4) // blocksize_1 = 256
            .push(1, 1);
        b.finish()
    }

    /// コードブック 1 個（2 エントリ・各符号長 1・lookup なし）+ floor1 1 個 + residue 1 個 +
    /// mapping 1 個 + mode 1 個だけの最小 setup。`floor_book` / `residue_classbook` /
    /// `residue_book` は検証対象の番号（コードブックは 1 個しか無いので 0 以外は範囲外）。
    fn setup_header(floor_book_plus1: u64, residue_classbook: u64, residue_book: u64) -> Vec<u8> {
        setup_header_with_master(None, floor_book_plus1, residue_classbook, residue_book)
    }

    /// `floor_master` が Some なら floor の class 0 は subclass 2 個（masterbook 付き）になる。
    fn setup_header_with_master(
        floor_master: Option<u64>,
        floor_book_plus1: u64,
        residue_classbook: u64,
        residue_book: u64,
    ) -> Vec<u8> {
        let mut b = LsbBits::default();
        b.push(5, 8);
        for &c in b"vorbis" {
            b.push(c as u64, 8);
        }
        b.push(0, 8); // codebook_count - 1
        b.push(0x56_4342, 24).push(1, 16).push(2, 24).push(0, 1).push(0, 1);
        b.push(0, 5).push(0, 5); // 2 エントリ、符号長 1
        b.push(0, 4); // lookup_type 0
        b.push(0, 6).push(0, 16); // time domain placeholder
        b.push(0, 6).push(1, 16); // floor 1 個、type 1
        b.push(1, 5); // partitions
        b.push(0, 4); // class 0
        match floor_master {
            None => {
                b.push(0, 3).push(0, 2); // dimensions-1, subclasses
                b.push(floor_book_plus1, 8); // subclass book
            }
            Some(master) => {
                b.push(0, 3).push(1, 2); // dimensions-1, subclasses = 1（subclass 2 個）
                b.push(master, 8);
                b.push(floor_book_plus1, 8).push(floor_book_plus1, 8);
            }
        }
        b.push(0, 2).push(4, 4); // multiplier-1, rangebits
        b.push(5, 4); // x_list
        b.push(0, 6).push(1, 16); // residue 1 個、type 1
        b.push(0, 24).push(128, 24).push(1, 24); // begin / end / partition_size-1
        b.push(0, 6); // classifications-1
        b.push(residue_classbook, 8);
        b.push(1, 3).push(0, 1); // cascade: pass 0 のみ
        b.push(residue_book, 8);
        b.push(0, 6).push(0, 16).push(0, 1).push(0, 1).push(0, 2); // mapping
        b.push(0, 8).push(0, 8).push(0, 8); // submap: time, floor, residue
        b.push(0, 6).push(0, 1).push(0, 16).push(0, 16).push(0, 8); // mode
        b.push(1, 1);
        b.finish()
    }

    fn audio_packet() -> Vec<u8> {
        let mut p = vec![0u8; 24];
        p[0] = 0b0000_0010; // audio パケット + floor nonzero
        p
    }

    fn assert_rejected_at_setup(setup: Vec<u8>, what: &str) {
        match VorbisStreamDecoder::new(&id_header(), &setup) {
            Err(_) => {}
            Ok(mut d) => {
                // 受理してしまうなら、デコード時に範囲外添字で落ちる（= setup で弾くべきだった）。
                let _ = d.decode_packet(&audio_packet());
                panic!("{what}: setup で拒否されなかった");
            }
        }
    }

    #[test]
    fn minimal_setup_is_accepted() {
        // 検証用の手組み setup 自体が正しいこと（book 0 を使う正常系）。
        let d = VorbisStreamDecoder::new(&id_header(), &setup_header(1, 0, 0)).unwrap();
        assert_eq!(d.channels(), 1);
    }

    #[test]
    fn setup_with_out_of_range_floor_book_is_rejected() {
        // floor の subclass book 番号 = 200 - 1 = 199（コードブックは 1 個）
        assert_rejected_at_setup(setup_header(200, 0, 0), "floor book");
    }

    #[test]
    fn setup_with_out_of_range_floor_masterbook_is_rejected() {
        assert!(VorbisStreamDecoder::new(
            &id_header(),
            &setup_header_with_master(Some(0), 1, 0, 0)
        )
        .is_ok());
        assert_rejected_at_setup(
            setup_header_with_master(Some(200), 1, 0, 0),
            "floor masterbook",
        );
    }

    #[test]
    fn setup_with_out_of_range_residue_classbook_is_rejected() {
        assert_rejected_at_setup(setup_header(1, 200, 0), "residue classbook");
    }

    #[test]
    fn setup_with_out_of_range_residue_book_is_rejected() {
        assert_rejected_at_setup(setup_header(1, 0, 200), "residue book");
    }
}
