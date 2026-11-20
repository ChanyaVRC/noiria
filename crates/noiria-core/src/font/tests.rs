//! font モジュールのテスト。
//!
//! - `synthetic_font_parses_exact_known_values` ほか: 手組みした最小の TTF
//!   バイト列を解析し、既知の座標・advance・cmap と完全一致することを確認する
//!   （パーサ自体の正しさをバイトレベルで担保する）。
//! - `cmap_format12_...`: format 12 (全域 cmap) を直接検証する。
//! - `fuzz_corrupted_bytes_never_panics`: LCG でバイト列を破壊し、Err/Ok のみ
//!   （panic なし）であることを 1000 ケース確認する。
//! - `system_font_*`: 実システムフォント（存在すれば）でのスモークテスト。

use super::*;

// ---------------------------------------------------------------------
// 手組み TTF ビルダー
// ---------------------------------------------------------------------

struct FontBuilder {
    tables: Vec<([u8; 4], Vec<u8>)>,
}

impl FontBuilder {
    fn new() -> Self {
        Self { tables: Vec::new() }
    }

    fn add(&mut self, tag: &[u8; 4], data: Vec<u8>) {
        self.tables.push((*tag, data));
    }

    fn build(self) -> Vec<u8> {
        let num_tables = self.tables.len() as u16;
        let mut out = Vec::new();
        out.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // sfnt version
        out.extend_from_slice(&num_tables.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // searchRange
        out.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
        out.extend_from_slice(&0u16.to_be_bytes()); // rangeShift

        let dir_start = out.len();
        let dir_size = 16 * self.tables.len();
        let mut offset = dir_start + dir_size;
        let mut dir = Vec::new();
        let mut body = Vec::new();
        for (tag, data) in &self.tables {
            let padded = data.len().div_ceil(4) * 4;
            dir.extend_from_slice(tag);
            dir.extend_from_slice(&0u32.to_be_bytes()); // checksum（未検証）
            dir.extend_from_slice(&(offset as u32).to_be_bytes());
            dir.extend_from_slice(&(data.len() as u32).to_be_bytes());
            body.extend_from_slice(data);
            body.resize(body.len() + (padded - data.len()), 0);
            offset += padded;
        }
        out.extend_from_slice(&dir);
        out.extend_from_slice(&body);
        out
    }
}

fn head_table(units_per_em: u16) -> Vec<u8> {
    let mut t = vec![0u8; 54];
    t[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    t[18..20].copy_from_slice(&units_per_em.to_be_bytes());
    t[40..42].copy_from_slice(&1000i16.to_be_bytes()); // xMax（適当）
    t[42..44].copy_from_slice(&1000i16.to_be_bytes()); // yMax（適当）
    t[50..52].copy_from_slice(&1i16.to_be_bytes()); // indexToLocFormat = long
    t
}

fn maxp_table(num_glyphs: u16) -> Vec<u8> {
    let mut t = vec![0u8; 32];
    t[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    t[4..6].copy_from_slice(&num_glyphs.to_be_bytes());
    t
}

fn hhea_table(ascender: i16, descender: i16, line_gap: i16, number_of_h_metrics: u16) -> Vec<u8> {
    let mut t = vec![0u8; 36];
    t[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    t[4..6].copy_from_slice(&ascender.to_be_bytes());
    t[6..8].copy_from_slice(&descender.to_be_bytes());
    t[8..10].copy_from_slice(&line_gap.to_be_bytes());
    t[34..36].copy_from_slice(&number_of_h_metrics.to_be_bytes());
    t
}

fn hmtx_table(entries: &[(u16, i16)]) -> Vec<u8> {
    let mut t = Vec::with_capacity(entries.len() * 4);
    for &(advance, lsb) in entries {
        t.extend_from_slice(&advance.to_be_bytes());
        t.extend_from_slice(&lsb.to_be_bytes());
    }
    t
}

fn os2_table(
    fs_selection: u16,
    typo_ascender: i16,
    typo_descender: i16,
    typo_line_gap: i16,
) -> Vec<u8> {
    let mut t = vec![0u8; 78];
    t[58..62].copy_from_slice(b"TEST");
    t[62..64].copy_from_slice(&fs_selection.to_be_bytes());
    t[64..66].copy_from_slice(&0x0041u16.to_be_bytes());
    t[66..68].copy_from_slice(&0x0044u16.to_be_bytes());
    t[68..70].copy_from_slice(&typo_ascender.to_be_bytes());
    t[70..72].copy_from_slice(&typo_descender.to_be_bytes());
    t[72..74].copy_from_slice(&typo_line_gap.to_be_bytes());
    t
}

fn name_table(family: &str) -> Vec<u8> {
    let mut storage = Vec::new();
    for u in family.encode_utf16() {
        storage.extend_from_slice(&u.to_be_bytes());
    }
    let mut t = Vec::new();
    t.extend_from_slice(&0u16.to_be_bytes()); // format 0
    t.extend_from_slice(&1u16.to_be_bytes()); // count
    let storage_offset: u16 = 6 + 12; // ヘッダ(6) + レコード1個(12)
    t.extend_from_slice(&storage_offset.to_be_bytes());
    t.extend_from_slice(&3u16.to_be_bytes()); // platformID = Windows
    t.extend_from_slice(&1u16.to_be_bytes()); // encodingID = Unicode BMP (UTF-16BE)
    t.extend_from_slice(&0x0409u16.to_be_bytes()); // languageID = en-US
    t.extend_from_slice(&1u16.to_be_bytes()); // nameID = Family
    t.extend_from_slice(&(storage.len() as u16).to_be_bytes());
    t.extend_from_slice(&0u16.to_be_bytes()); // offset within storage
    t.extend_from_slice(&storage);
    t
}

/// 'A'..'D' (0x41..0x44) -> glyph 1..4 の固定マッピングを持つ format4 cmap。
fn cmap_table() -> Vec<u8> {
    let mut sub = Vec::new();
    sub.extend_from_slice(&4u16.to_be_bytes()); // format
    let length_pos = sub.len();
    sub.extend_from_slice(&0u16.to_be_bytes()); // length（後で埋める）
    sub.extend_from_slice(&0u16.to_be_bytes()); // language
    sub.extend_from_slice(&4u16.to_be_bytes()); // segCountX2 (segCount=2)
    sub.extend_from_slice(&0u16.to_be_bytes()); // searchRange
    sub.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
    sub.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
                                                // endCode
    sub.extend_from_slice(&0x0044u16.to_be_bytes());
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes());
    // reservedPad
    sub.extend_from_slice(&0u16.to_be_bytes());
    // startCode
    sub.extend_from_slice(&0x0041u16.to_be_bytes());
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes());
    // idDelta（cp - 0x41 + 1 になるようにする）
    let delta0 = 1i16.wrapping_sub(0x0041);
    sub.extend_from_slice(&(delta0 as u16).to_be_bytes());
    sub.extend_from_slice(&1u16.to_be_bytes());
    // idRangeOffset
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    let len = sub.len() as u16;
    sub[length_pos..length_pos + 2].copy_from_slice(&len.to_be_bytes());

    let mut t = Vec::new();
    t.extend_from_slice(&0u16.to_be_bytes()); // version
    t.extend_from_slice(&1u16.to_be_bytes()); // numTables
    t.extend_from_slice(&3u16.to_be_bytes()); // platformID = Windows
    t.extend_from_slice(&1u16.to_be_bytes()); // encodingID = BMP
    t.extend_from_slice(&12u32.to_be_bytes()); // offset = header(4) + record(8)
    t.extend_from_slice(&sub);
    t
}

fn loca_table_long(offsets: &[u32]) -> Vec<u8> {
    let mut t = Vec::with_capacity(offsets.len() * 4);
    for &o in offsets {
        t.extend_from_slice(&o.to_be_bytes());
    }
    t
}

fn bbox_of(points: &[(i32, i32)]) -> (i32, i32, i32, i32) {
    let mut xmin = i32::MAX;
    let mut ymin = i32::MAX;
    let mut xmax = i32::MIN;
    let mut ymax = i32::MIN;
    for &(x, y) in points {
        xmin = xmin.min(x);
        ymin = ymin.min(y);
        xmax = xmax.max(x);
        ymax = ymax.max(y);
    }
    (xmin, ymin, xmax, ymax)
}

/// 単純グリフ（全点オンカーブ・直線のみ）のバイト列を組み立てる。
fn simple_glyph_bytes(points: &[(i32, i32)]) -> Vec<u8> {
    let n = points.len();
    let (xmin, ymin, xmax, ymax) = bbox_of(points);
    let mut t = Vec::new();
    t.extend_from_slice(&1i16.to_be_bytes()); // numberOfContours
    t.extend_from_slice(&(xmin as i16).to_be_bytes());
    t.extend_from_slice(&(ymin as i16).to_be_bytes());
    t.extend_from_slice(&(xmax as i16).to_be_bytes());
    t.extend_from_slice(&(ymax as i16).to_be_bytes());
    t.extend_from_slice(&((n - 1) as u16).to_be_bytes()); // endPtsOfContours[0]
    t.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
    t.extend(std::iter::repeat_n(0x01u8, n)); // ON_CURVE_POINT, x/y とも明示的な 16bit 差分
    let mut px = 0i32;
    for &(x, _) in points {
        t.extend_from_slice(&((x - px) as i16).to_be_bytes());
        px = x;
    }
    let mut py = 0i32;
    for &(_, y) in points {
        t.extend_from_slice(&((y - py) as i16).to_be_bytes());
        py = y;
    }
    t
}

const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const ARGS_ARE_XY_VALUES: u16 = 0x0002;
const WE_HAVE_A_SCALE: u16 = 0x0008;

fn composite_translate_bytes(glyph_index: u16, dx: i16, dy: i16) -> Vec<u8> {
    let mut t = Vec::new();
    t.extend_from_slice(&(-1i16).to_be_bytes()); // numberOfContours = -1 (composite)
    t.extend_from_slice(&[0u8; 8]); // bbox（未検証なので 0 で可）
    let flags = ARG_1_AND_2_ARE_WORDS | ARGS_ARE_XY_VALUES;
    t.extend_from_slice(&flags.to_be_bytes());
    t.extend_from_slice(&glyph_index.to_be_bytes());
    t.extend_from_slice(&dx.to_be_bytes());
    t.extend_from_slice(&dy.to_be_bytes());
    t
}

fn composite_scale_bytes(glyph_index: u16, scale_f2dot14_raw: i16) -> Vec<u8> {
    let mut t = Vec::new();
    t.extend_from_slice(&(-1i16).to_be_bytes());
    t.extend_from_slice(&[0u8; 8]);
    let flags = ARG_1_AND_2_ARE_WORDS | ARGS_ARE_XY_VALUES | WE_HAVE_A_SCALE;
    t.extend_from_slice(&flags.to_be_bytes());
    t.extend_from_slice(&glyph_index.to_be_bytes());
    t.extend_from_slice(&0i16.to_be_bytes()); // dx
    t.extend_from_slice(&0i16.to_be_bytes()); // dy
    t.extend_from_slice(&scale_f2dot14_raw.to_be_bytes());
    t
}

/// 固定の 5 グリフ構成:
/// 0=.notdef(空) 1=正方形 2=三角形 3=正方形の平行移動複合 4=三角形の 0.5 倍縮小複合
fn build_glyf_and_loca() -> (Vec<u8>, Vec<u32>) {
    let glyph1 = simple_glyph_bytes(&[(100, 100), (100, 600), (600, 600), (600, 100)]);
    let glyph2 = simple_glyph_bytes(&[(200, 200), (500, 200), (350, 700)]);
    let glyph3 = composite_translate_bytes(1, 800, 50);
    let glyph4 = composite_scale_bytes(2, 8192); // F2Dot14(8192) = 0.5

    let mut glyf = Vec::new();
    let mut loca = vec![0u32, 0u32]; // glyph0: 空（start==end）
    glyf.extend_from_slice(&glyph1);
    loca.push(glyf.len() as u32);
    glyf.extend_from_slice(&glyph2);
    loca.push(glyf.len() as u32);
    glyf.extend_from_slice(&glyph3);
    loca.push(glyf.len() as u32);
    glyf.extend_from_slice(&glyph4);
    loca.push(glyf.len() as u32);

    (glyf, loca)
}

struct Os2Opts {
    use_typo_metrics: bool,
    typo_ascender: i16,
    typo_descender: i16,
    typo_line_gap: i16,
}

struct FontOpts {
    units_per_em: u16,
    family: &'static str,
    ascender: i16,
    descender: i16,
    line_gap: i16,
    os2: Option<Os2Opts>,
}

fn build_font(opts: &FontOpts) -> Vec<u8> {
    let (glyf, loca_offsets) = build_glyf_and_loca();
    let num_glyphs: u16 = 5;
    let hmtx = hmtx_table(&[(500, 0), (600, 10), (700, 20), (900, 30), (450, 5)]);

    let mut b = FontBuilder::new();
    b.add(b"head", head_table(opts.units_per_em));
    b.add(b"maxp", maxp_table(num_glyphs));
    b.add(
        b"hhea",
        hhea_table(opts.ascender, opts.descender, opts.line_gap, num_glyphs),
    );
    b.add(b"hmtx", hmtx);
    b.add(b"cmap", cmap_table());
    b.add(b"loca", loca_table_long(&loca_offsets));
    b.add(b"glyf", glyf);
    b.add(b"name", name_table(opts.family));
    if let Some(o) = &opts.os2 {
        let mut fs_selection = 0u16;
        if o.use_typo_metrics {
            fs_selection |= 0x0080;
        }
        b.add(
            b"OS/2",
            os2_table(
                fs_selection,
                o.typo_ascender,
                o.typo_descender,
                o.typo_line_gap,
            ),
        );
    }
    b.build()
}

/// `font::registry` のテストが再利用する最小フォントビルダー（`.notdef` + 数グリフ +
/// 有効な name/cmap テーブルを含み、`Font::from_bytes` が成功する構成）。
pub(crate) fn minimal_font_bytes(family: &'static str) -> Vec<u8> {
    build_font(&FontOpts {
        units_per_em: 1000,
        family,
        ascender: 800,
        descender: -200,
        line_gap: 0,
        os2: None,
    })
}

/// [`minimal_font_bytes`] と同じ輪郭で units_per_em だけ変えたもの（同じ文字が別の大きさで
/// 描かれるので、どちらのフォントが選ばれたかをグリフ寸法で区別できる）。
pub(crate) fn minimal_font_bytes_with_upem(family: &'static str, units_per_em: u16) -> Vec<u8> {
    build_font(&FontOpts {
        units_per_em,
        family,
        ascender: 800,
        descender: -200,
        line_gap: 0,
        os2: None,
    })
}

/// `font::registry` のテストが再利用する TTC 組み立て（[`minimal_font_bytes`] 複数を
/// 1 つの TTC バイト列にまとめる）。
pub(crate) fn wrap_as_ttc_for_test(fonts: &[Vec<u8>]) -> Vec<u8> {
    wrap_as_ttc(fonts)
}

/// sfnt のテーブルディレクトリに書かれた各テーブルのオフセットは（TTC 内でも）
/// ファイル先頭からの絶対オフセットである。`build_font` はオフセット 0 起点で
/// 単体ファイルとして直に組み立てているため、TTC に埋め込んで別の位置へずらす際は
/// 埋め込み先の絶対位置 (`base`) を内部のオフセットフィールドへ加算し直す必要がある。
fn rebase_offset_table(font: &mut [u8], base: u32) {
    let num_tables = u16::from_be_bytes([font[4], font[5]]) as usize;
    for i in 0..num_tables {
        let off_pos = 12 + 16 * i + 8;
        let cur = u32::from_be_bytes([
            font[off_pos],
            font[off_pos + 1],
            font[off_pos + 2],
            font[off_pos + 3],
        ]);
        let new_off = cur + base;
        font[off_pos..off_pos + 4].copy_from_slice(&new_off.to_be_bytes());
    }
}

fn wrap_as_ttc(fonts: &[Vec<u8>]) -> Vec<u8> {
    let mut header = Vec::new();
    header.extend_from_slice(b"ttcf");
    header.extend_from_slice(&1u16.to_be_bytes()); // majorVersion
    header.extend_from_slice(&0u16.to_be_bytes()); // minorVersion
    header.extend_from_slice(&(fonts.len() as u32).to_be_bytes());
    let table_start = header.len();
    for _ in fonts {
        header.extend_from_slice(&0u32.to_be_bytes()); // placeholder
    }
    let body_start = header.len();
    let mut body = Vec::new();
    let mut offsets = Vec::new();
    for f in fonts {
        let base = (body_start + body.len()) as u32;
        offsets.push(base);
        let mut fbytes = f.clone();
        rebase_offset_table(&mut fbytes, base);
        body.extend_from_slice(&fbytes);
    }
    for (i, off) in offsets.iter().enumerate() {
        header[table_start + i * 4..table_start + i * 4 + 4].copy_from_slice(&off.to_be_bytes());
    }
    header.extend_from_slice(&body);
    header
}

// ---------------------------------------------------------------------
// 合成フォントによる既知値との完全一致テスト
// ---------------------------------------------------------------------

#[test]
fn synthetic_font_parses_exact_known_values() {
    let opts = FontOpts {
        units_per_em: 1000,
        family: "Test Square Font",
        ascender: 800,
        descender: -200,
        line_gap: 90,
        os2: None,
    };
    let bytes = build_font(&opts);
    let font = Font::from_bytes(bytes, 0).expect("合成フォントの解析に失敗");

    assert_eq!(font.family_name(), "Test Square Font");
    assert_eq!(font.units_per_em(), 1000);
    assert_eq!(font.ascent(), 800);
    assert_eq!(font.descent(), -200);
    assert_eq!(font.line_gap(), 90);
    assert_eq!(font.num_glyphs(), 5);

    assert_eq!(font.glyph_id('A'), Some(1));
    assert_eq!(font.glyph_id('B'), Some(2));
    assert_eq!(font.glyph_id('C'), Some(3));
    assert_eq!(font.glyph_id('D'), Some(4));
    assert_eq!(
        font.glyph_id('Z'),
        None,
        "マップされていない文字は None のはず"
    );

    assert_eq!(font.advance(0), 500);
    assert_eq!(font.advance(1), 600);
    assert_eq!(font.advance(2), 700);
    assert_eq!(font.advance(3), 900);
    assert_eq!(font.advance(4), 450);
    assert_eq!(
        font.advance(9999),
        450,
        "範囲外グリフは最後の advance を引き継ぐはず"
    );

    // glyph0: .notdef は空
    let notdef = font.outline(0).unwrap();
    assert!(notdef.contours.is_empty());

    fn assert_line_loop(outline: &Outline, expected: &[(Point, Point)]) {
        assert_eq!(outline.contours.len(), 1);
        let segs = &outline.contours[0].segments;
        assert_eq!(segs.len(), expected.len());
        for (seg, (p0, p1)) in segs.iter().zip(expected.iter()) {
            match seg {
                Segment::Line(a, b) => {
                    assert_eq!(a, p0);
                    assert_eq!(b, p1);
                }
                Segment::Quad(..) => panic!("直線のみのはずが Quad が出た"),
            }
        }
    }

    // glyph1: 正方形
    let p = |x: f32, y: f32| Point { x, y };
    assert_line_loop(
        &font.outline(1).unwrap(),
        &[
            (p(100.0, 100.0), p(100.0, 600.0)),
            (p(100.0, 600.0), p(600.0, 600.0)),
            (p(600.0, 600.0), p(600.0, 100.0)),
            (p(600.0, 100.0), p(100.0, 100.0)),
        ],
    );

    // glyph2: 三角形
    assert_line_loop(
        &font.outline(2).unwrap(),
        &[
            (p(200.0, 200.0), p(500.0, 200.0)),
            (p(500.0, 200.0), p(350.0, 700.0)),
            (p(350.0, 700.0), p(200.0, 200.0)),
        ],
    );

    // glyph3: 正方形を (800, 50) 平行移動した複合グリフ
    assert_line_loop(
        &font.outline(3).unwrap(),
        &[
            (p(900.0, 150.0), p(900.0, 650.0)),
            (p(900.0, 650.0), p(1400.0, 650.0)),
            (p(1400.0, 650.0), p(1400.0, 150.0)),
            (p(1400.0, 150.0), p(900.0, 150.0)),
        ],
    );

    // glyph4: 三角形を 0.5 倍縮小した複合グリフ
    assert_line_loop(
        &font.outline(4).unwrap(),
        &[
            (p(100.0, 100.0), p(250.0, 100.0)),
            (p(250.0, 100.0), p(175.0, 350.0)),
            (p(175.0, 350.0), p(100.0, 100.0)),
        ],
    );
}

#[test]
fn os2_typo_metrics_used_when_use_typo_metrics_flag_set() {
    let opts = FontOpts {
        units_per_em: 1000,
        family: "OS2 Typo Font",
        ascender: 1000,
        descender: -300,
        line_gap: 0,
        os2: Some(Os2Opts {
            use_typo_metrics: true,
            typo_ascender: 750,
            typo_descender: -250,
            typo_line_gap: 120,
        }),
    };
    let font = Font::from_bytes(build_font(&opts), 0).unwrap();
    assert_eq!(font.ascent(), 750);
    assert_eq!(font.descent(), -250);
    assert_eq!(font.line_gap(), 120);
}

#[test]
fn hhea_metrics_used_when_os2_present_but_flag_unset() {
    let opts = FontOpts {
        units_per_em: 1000,
        family: "HHEA Font",
        ascender: 1000,
        descender: -300,
        line_gap: 10,
        os2: Some(Os2Opts {
            use_typo_metrics: false,
            typo_ascender: 1,
            typo_descender: 1,
            typo_line_gap: 1,
        }),
    };
    let font = Font::from_bytes(build_font(&opts), 0).unwrap();
    assert_eq!(font.ascent(), 1000);
    assert_eq!(font.descent(), -300);
    assert_eq!(font.line_gap(), 10);
}

#[test]
fn hhea_metrics_used_when_os2_absent() {
    let opts = FontOpts {
        units_per_em: 1000,
        family: "No OS2 Font",
        ascender: 900,
        descender: -100,
        line_gap: 5,
        os2: None,
    };
    let font = Font::from_bytes(build_font(&opts), 0).unwrap();
    assert_eq!(font.ascent(), 900);
    assert_eq!(font.descent(), -100);
    assert_eq!(font.line_gap(), 5);
}

#[test]
fn ttc_collection_selects_font_by_index() {
    let opts_a = FontOpts {
        units_per_em: 1000,
        family: "TTC Font A",
        ascender: 800,
        descender: -200,
        line_gap: 0,
        os2: None,
    };
    let opts_b = FontOpts {
        units_per_em: 2048,
        family: "TTC Font B",
        ascender: 900,
        descender: -100,
        line_gap: 0,
        os2: None,
    };
    let ttc = wrap_as_ttc(&[build_font(&opts_a), build_font(&opts_b)]);

    let a = Font::from_bytes(ttc.clone(), 0).unwrap();
    assert_eq!(a.family_name(), "TTC Font A");
    assert_eq!(a.units_per_em(), 1000);

    let b = Font::from_bytes(ttc.clone(), 1).unwrap();
    assert_eq!(b.family_name(), "TTC Font B");
    assert_eq!(b.units_per_em(), 2048);

    assert!(
        Font::from_bytes(ttc, 2).is_err(),
        "存在しないインデックスは Err のはず"
    );
}

#[test]
fn otto_cff_is_rejected_explicitly() {
    let mut data = Vec::new();
    data.extend_from_slice(b"OTTO");
    data.extend_from_slice(&[0u8; 8]); // numTables 等（内容は読まれない）
    let err = match Font::from_bytes(data, 0) {
        Ok(_) => panic!("OTTO は Err になるはず"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("OTTO") || msg.contains("OpenType"),
        "エラーメッセージに 'OTTO' 非対応の旨がない: {msg}"
    );
}

#[test]
fn garbage_data_returns_err_not_panic() {
    assert!(Font::from_bytes(vec![], 0).is_err());
    assert!(Font::from_bytes(vec![0u8; 3], 0).is_err());
    assert!(Font::from_bytes(b"not a font at all, just garbage bytes".to_vec(), 0).is_err());
}

#[test]
fn cmap_format12_resolves_bmp_and_supplementary_plane() {
    // format12 のみを持つ cmap テーブルを直接組み立てて検証する（BMP 外の解決確認）。
    let mut sub = Vec::new();
    sub.extend_from_slice(&12u16.to_be_bytes()); // format
    sub.extend_from_slice(&0u16.to_be_bytes()); // reserved
    sub.extend_from_slice(&0u32.to_be_bytes()); // length（未検証）
    sub.extend_from_slice(&0u32.to_be_bytes()); // language
    sub.extend_from_slice(&2u32.to_be_bytes()); // numGroups
                                                // group0: 'A'..'C' (0x41..0x43) -> glyph 10..12
    sub.extend_from_slice(&0x0041u32.to_be_bytes());
    sub.extend_from_slice(&0x0043u32.to_be_bytes());
    sub.extend_from_slice(&10u32.to_be_bytes());
    // group1: 補助面 U+20000..U+20002 -> glyph 100..102
    sub.extend_from_slice(&0x0002_0000u32.to_be_bytes());
    sub.extend_from_slice(&0x0002_0002u32.to_be_bytes());
    sub.extend_from_slice(&100u32.to_be_bytes());

    let mut t = Vec::new();
    t.extend_from_slice(&0u16.to_be_bytes()); // version
    t.extend_from_slice(&1u16.to_be_bytes()); // numTables
    t.extend_from_slice(&3u16.to_be_bytes()); // platformID = Windows
    t.extend_from_slice(&10u16.to_be_bytes()); // encodingID = 全域 Unicode
    t.extend_from_slice(&12u32.to_be_bytes()); // offset = header(4) + record(8)
    t.extend_from_slice(&sub);

    let table = cmap::parse_cmap(&t).expect("format12 cmap の解析に失敗");
    assert_eq!(table.lookup('A'), Some(10));
    assert_eq!(table.lookup('C'), Some(12));
    assert_eq!(table.lookup('\u{20001}'), Some(101));
    assert_eq!(table.lookup('D'), None);
}

// ---------------------------------------------------------------------
// fuzz テスト: LCG でバイト列を破壊し、panic せず Err/Ok のいずれかを返すこと
// ---------------------------------------------------------------------

fn lcg_next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed
}

#[test]
fn fuzz_corrupted_bytes_never_panics() {
    let opts = FontOpts {
        units_per_em: 1000,
        family: "Fuzz Font",
        ascender: 800,
        descender: -200,
        line_gap: 90,
        os2: Some(Os2Opts {
            use_typo_metrics: true,
            typo_ascender: 750,
            typo_descender: -250,
            typo_line_gap: 100,
        }),
    };
    let base = build_font(&opts);
    let mut seed = 0x1234_5678_9abc_def0u64;

    for _ in 0..1000u32 {
        let mut data = base.clone();
        let n_mutations = 1 + (lcg_next(&mut seed) % 8) as usize;
        for _ in 0..n_mutations {
            if data.is_empty() {
                break;
            }
            let idx = (lcg_next(&mut seed) as usize) % data.len();
            data[idx] = (lcg_next(&mut seed) & 0xFF) as u8;
        }
        // 一定確率でファイルの切り詰めも混ぜる（長さフィールドと実データの不整合を突く）
        if lcg_next(&mut seed).is_multiple_of(10) && data.len() > 4 {
            let cut = 4 + (lcg_next(&mut seed) as usize) % (data.len() - 4);
            data.truncate(cut);
        }

        // 破壊されたデータが Err になるのは正常（panic しないことだけを確認する）。
        if let Ok(font) = Font::from_bytes(data, 0) {
            // パース成功時も、後続 API 呼び出しが panic しないことを確認する。
            let n = font.num_glyphs();
            for g in 0..n.min(8) {
                let _ = font.outline(g);
                let _ = font.advance(g);
            }
            for c in ['A', 'B', 'Z', 'あ'] {
                let _ = font.glyph_id(c);
            }
        }
    }
}

// ---------------------------------------------------------------------
// 実システムフォントでのスモークテスト（未インストール環境ではスキップ）
// ---------------------------------------------------------------------

fn load_system_font(path: &str, index: u32) -> Option<Font> {
    let p = std::path::Path::new(path);
    if !p.exists() {
        return None;
    }
    let bytes = std::fs::read(p).ok()?;
    match Font::from_bytes(bytes, index) {
        Ok(f) => Some(f),
        Err(e) => panic!("{path} の解析に失敗しました: {e}"),
    }
}

fn assert_reasonable_coords(outline: &Outline, units_per_em: u16) {
    let limit = 4.0 * units_per_em as f32;
    for contour in &outline.contours {
        for seg in &contour.segments {
            let pts: Vec<Point> = match *seg {
                Segment::Line(a, b) => vec![a, b],
                Segment::Quad(a, ctrl, b) => vec![a, ctrl, b],
            };
            for p in pts {
                assert!(
                    p.x.abs() <= limit && p.y.abs() <= limit,
                    "座標が妥当域を超えています: {p:?}"
                );
            }
        }
    }
}

fn assert_common_font_properties(font: &Font, expect_cjk: bool) {
    assert!(!font.family_name().is_empty(), "family_name が空です");
    assert!(font.units_per_em() > 0);

    let a_id = font
        .glyph_id('A')
        .filter(|&g| g != 0)
        .unwrap_or_else(|| panic!("'A' の glyph_id が引けません"));
    assert!(font.advance(a_id) > 0, "'A' の advance が 0 です");
    let outline = font.outline(a_id).expect("'A' の outline 抽出に失敗");
    assert!(!outline.contours.is_empty(), "'A' の輪郭数が 0 です");
    assert_reasonable_coords(&outline, font.units_per_em());

    if expect_cjk {
        for c in ['あ', '漢'] {
            let id = font
                .glyph_id(c)
                .filter(|&g| g != 0)
                .unwrap_or_else(|| panic!("'{c}' の glyph_id が引けません"));
            assert!(font.advance(id) > 0, "'{c}' の advance が 0 です");
            let outline = font.outline(id).expect("outline 抽出に失敗");
            assert!(!outline.contours.is_empty(), "'{c}' の輪郭数が 0 です");
            assert_reasonable_coords(&outline, font.units_per_em());
        }
    }
}

#[test]
fn system_font_msgothic_ttc() {
    let Some(font) = load_system_font(r"C:\Windows\Fonts\msgothic.ttc", 0) else {
        eprintln!("msgothic.ttc が見つからないためスキップ");
        return;
    };
    assert_common_font_properties(&font, true);
}

#[test]
fn system_font_yugothic_or_meiryo_ttc() {
    let font = load_system_font(r"C:\Windows\Fonts\YuGothM.ttc", 0)
        .or_else(|| load_system_font(r"C:\Windows\Fonts\meiryo.ttc", 0));
    let Some(font) = font else {
        eprintln!("YuGothM.ttc / meiryo.ttc が見つからないためスキップ");
        return;
    };
    assert_common_font_properties(&font, true);
}

#[test]
fn system_font_arial_ttf() {
    let Some(font) = load_system_font(r"C:\Windows\Fonts\arial.ttf", 0) else {
        eprintln!("arial.ttf が見つからないためスキップ");
        return;
    };
    assert_common_font_properties(&font, false);
}

// ---------------------------------------------------------------------
// R1: 外部入力（フォントファイル）の堅牢化
// ---------------------------------------------------------------------

/// 任意のグリフ列（先頭は空グリフ）から最小フォントを組む。cmap は 'A'..'D' -> 1..4。
fn custom_font(units_per_em: u16, glyphs: &[Vec<u8>]) -> Vec<u8> {
    let mut glyf = Vec::new();
    let mut loca = vec![0u32];
    for g in glyphs {
        glyf.extend_from_slice(g);
        while glyf.len() % 4 != 0 {
            glyf.push(0);
        }
        loca.push(glyf.len() as u32);
    }
    let n = glyphs.len() as u16;
    let hmtx: Vec<(u16, i16)> = (0..n).map(|_| (500u16, 0i16)).collect();
    let mut b = FontBuilder::new();
    b.add(b"head", head_table(units_per_em));
    b.add(b"maxp", maxp_table(n));
    b.add(b"hhea", hhea_table(800, -200, 0, n));
    b.add(b"hmtx", hmtx_table(&hmtx));
    b.add(b"cmap", cmap_table());
    b.add(b"loca", loca_table_long(&loca));
    b.add(b"glyf", glyf);
    b.add(b"name", name_table("Custom"));
    b.build()
}

/// 同じ子グリフを `refs` 回参照する複合グリフ。
fn composite_of(child: u16, refs: usize) -> Vec<u8> {
    let mut t = Vec::new();
    t.extend_from_slice(&(-1i16).to_be_bytes());
    t.extend_from_slice(&[0u8; 8]);
    for i in 0..refs {
        let mut flags = ARG_1_AND_2_ARE_WORDS | ARGS_ARE_XY_VALUES;
        if i + 1 < refs {
            flags |= 0x0020; // MORE_COMPONENTS
        }
        t.extend_from_slice(&flags.to_be_bytes());
        t.extend_from_slice(&child.to_be_bytes());
        t.extend_from_slice(&0i16.to_be_bytes());
        t.extend_from_slice(&0i16.to_be_bytes());
    }
    t
}

#[test]
fn registry_indexing_a_ttc_with_a_huge_font_count_finishes_quickly() {
    let mut ttc = Vec::new();
    ttc.extend_from_slice(b"ttcf");
    ttc.extend_from_slice(&1u16.to_be_bytes());
    ttc.extend_from_slice(&0u16.to_be_bytes());
    ttc.extend_from_slice(&u32::MAX.to_be_bytes());
    ttc.extend_from_slice(&[0u8; 64]);
    let t = std::time::Instant::now();
    let reg = registry::FontRegistry::from_game_font_bytes(&[ttc]);
    assert!(reg.is_empty());
    assert!(t.elapsed().as_secs() < 5, "TTC の巨大な numFonts で長時間ループした");
}

#[test]
fn composite_glyph_expansion_is_bounded_by_a_component_budget() {
    // g1..g8 は直下の子を 4 回参照する複合。深さは 8 以内だが、葉は 4^8 = 65536 回展開される。
    let mut glyphs = vec![Vec::new()];
    for k in 1..=8u16 {
        glyphs.push(composite_of(k + 1, 4));
    }
    glyphs.push(simple_glyph_bytes(&[(100, 100), (100, 600), (600, 100)]));
    let font = Font::from_bytes(custom_font(1000, &glyphs), 0).unwrap();
    assert!(font.outline(9).is_ok());
    assert!(font.outline(8).is_ok(), "小さな複合は展開できる");
    assert!(font.outline(1).is_err(), "成分数の予算を超える複合は Err");
}

#[test]
fn glyph_mask_with_absurd_dimensions_is_rejected() {
    // 座標 0..30000 の正方形を 2 倍 = 60000x60000 px のマスクになる。
    let glyphs = vec![
        Vec::new(),
        simple_glyph_bytes(&[(0, 0), (0, 30000), (30000, 30000), (30000, 0)]),
    ];
    let font = Font::from_bytes(custom_font(16, &glyphs), 0).unwrap();
    let outline = font.outline(1).unwrap();
    let mask = crate::test_support::with_alloc_limit(256 << 20, || rasterize(&outline, 2.0));
    assert!(
        mask.data.len() <= 4096 * 4096,
        "マスクが {}x{} で確保されている",
        mask.width,
        mask.height
    );
}

#[test]
fn glyph_mask_within_the_limit_is_still_rendered() {
    let glyphs = vec![
        Vec::new(),
        simple_glyph_bytes(&[(0, 0), (0, 1000), (1000, 1000), (1000, 0)]),
    ];
    let font = Font::from_bytes(custom_font(1000, &glyphs), 0).unwrap();
    let mask = rasterize(&font.outline(1).unwrap(), 2.0);
    assert_eq!((mask.width, mask.height), (2000, 2000));
}


// ---------------------------------------------------------------------
// R1: 破壊 fuzz（バイト反転・切り詰め・長さフィールド改変。確保上限つき）
// ---------------------------------------------------------------------

/// 成功したフォントに対して、後続 API（アウトライン・ラスタ・advance・cmap 引き）を一通り呼ぶ。
fn exercise_font(font: &Font) {
    let n = font.num_glyphs();
    let upem = font.units_per_em().max(1) as f32;
    for g in 0..n.min(24) {
        if let Ok(outline) = font.outline(g) {
            let _ = rasterize(&outline, 26.0 / upem);
        }
        let _ = font.advance(g);
    }
    for c in ['A', 'B', 'D', 'あ', '\u{20001}', '\u{10FFFF}'] {
        let _ = font.glyph_id(c);
    }
}

#[test]
fn fuzz_corrupted_font_never_panics_or_overallocates() {
    let plain = build_font(&FontOpts {
        units_per_em: 1000,
        family: "Fuzz Font",
        ascender: 800,
        descender: -200,
        line_gap: 90,
        os2: Some(Os2Opts {
            use_typo_metrics: true,
            typo_ascender: 750,
            typo_descender: -250,
            typo_line_gap: 100,
        }),
    });
    // 複合グリフの入れ子・多数の成分を含むフォント。
    let mut glyphs = vec![Vec::new()];
    for k in 1..=4u16 {
        glyphs.push(composite_of(k + 1, 3));
    }
    glyphs.push(simple_glyph_bytes(&[(100, 100), (100, 600), (600, 100)]));
    glyphs.push(simple_glyph_bytes(&[(0, 0), (0, 900), (900, 900), (900, 0)]));
    let composite = custom_font(1000, &glyphs);
    for (i, base) in [plain, composite].iter().enumerate() {
        assert!(Font::from_bytes(base.clone(), 0).is_ok(), "基準のフォント {i} が解析できる");
        crate::test_support::fuzz("font", 0xC0DE + i as u64, 2000, base, |d| {
            if let Ok(font) = Font::from_bytes(d.to_vec(), 0) {
                exercise_font(&font);
            }
        });
    }
}

#[test]
fn fuzz_corrupted_cmap_never_panics_or_overallocates() {
    let format12 = {
        let mut sub = Vec::new();
        sub.extend_from_slice(&12u16.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&(16u32 + 24).to_be_bytes());
        sub.extend_from_slice(&0u32.to_be_bytes());
        sub.extend_from_slice(&2u32.to_be_bytes());
        for (s, e, g) in [(0x41u32, 0x43u32, 10u32), (0x20000, 0x20001, 100)] {
            sub.extend_from_slice(&s.to_be_bytes());
            sub.extend_from_slice(&e.to_be_bytes());
            sub.extend_from_slice(&g.to_be_bytes());
        }
        let mut t = Vec::new();
        t.extend_from_slice(&0u16.to_be_bytes());
        t.extend_from_slice(&1u16.to_be_bytes());
        t.extend_from_slice(&3u16.to_be_bytes());
        t.extend_from_slice(&10u16.to_be_bytes());
        t.extend_from_slice(&12u32.to_be_bytes());
        t.extend_from_slice(&sub);
        t
    };
    for (i, base) in [cmap_table(), format12].iter().enumerate() {
        assert!(cmap::parse_cmap(base).is_ok(), "基準の cmap {i} が解析できる");
        crate::test_support::fuzz("cmap", 0xCAB0 + i as u64, 3000, base, |d| {
            if let Ok(t) = cmap::parse_cmap(d) {
                for c in ['A', 'D', 'あ', '\u{20001}', '\u{10FFFF}', '\0'] {
                    let _ = t.lookup(c);
                }
            }
        });
    }
}

#[test]
fn fuzz_corrupted_ttc_never_panics_or_overallocates() {
    let f1 = minimal_font_bytes("Fuzz TTC One");
    let f2 = minimal_font_bytes("Fuzz TTC Two");
    let base = wrap_as_ttc(&[f1, f2]);
    assert_eq!(
        registry::FontRegistry::from_game_font_bytes(std::slice::from_ref(&base)).len(),
        2
    );
    crate::test_support::fuzz("ttc", 0x77C0, 1500, &base, |d| {
        let reg = registry::FontRegistry::from_game_font_bytes(&[d.to_vec()]);
        let _ = reg.resolve("Fuzz TTC One");
        for index in 0..3 {
            if let Ok(font) = Font::from_bytes(d.to_vec(), index) {
                exercise_font(&font);
            }
        }
    });
}
