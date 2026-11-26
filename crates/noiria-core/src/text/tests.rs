use super::*;

fn engine() -> TextEngine {
    TextEngine::new()
}

fn seg(text: &str) -> TextSegment {
    TextSegment {
        text: text.into(),
        ..Default::default()
    }
}

#[test]
fn cjk_glyphs_rasterize() {
    let mut e = engine();
    let style = TextStyle::default();
    let r = e.raster_segments(&[seg("こんにちは、世界。")], &style);
    assert!(r.glyphs.len() >= 9, "グリフ数: {}", r.glyphs.len());
    // 各グリフに不透明ピクセルがある
    for g in &r.glyphs {
        assert!(
            g.pixmap.data.iter().any(|&b| b != 0),
            "空のグリフがあります (byte_start={})",
            g.byte_start
        );
    }
}

#[test]
fn byte_offsets_are_monotonic_and_valid() {
    let mut e = engine();
    let style = TextStyle::default();
    let text = "abcあいう\nかきく";
    let r = e.raster_segments(&[seg(text)], &style);
    let full: String = text.into();
    let mut prev = 0usize;
    for g in &r.glyphs {
        assert!(g.byte_start >= prev, "バイト位置が逆行");
        assert!(g.byte_end <= full.len());
        assert!(full.is_char_boundary(g.byte_start));
        prev = g.byte_start;
    }
}

#[test]
fn wrapping_stays_in_box() {
    let mut e = engine();
    let style = TextStyle {
        box_w: 200,
        box_h: 400,
        font_size: 24.0,
        outline_width: 0,
        ..Default::default()
    };
    let r = e.raster_segments(
        &[seg("長い日本語のテキストは箱の幅で折り返されます")],
        &style,
    );
    assert!(r.glyphs.len() > 5);
    for g in &r.glyphs {
        assert!(
            g.x >= -2 && (g.x as u32) < style.box_w + 2,
            "x={} が box_w 超過",
            g.x
        );
    }
    // 複数行に折り返されている
    let max_line = r.glyphs.iter().map(|g| g.line_i).max().unwrap_or(0);
    let distinct_y: std::collections::HashSet<i32> = r.glyphs.iter().map(|g| g.y / 10).collect();
    assert!(
        max_line >= 1 || distinct_y.len() > 1,
        "折返しが発生していない"
    );
}

#[test]
fn appending_keeps_prefix_glyph_count() {
    // 追記時キャッシュ (ops_text) の前提: 前方テキストのグリフ数は追記で不変
    let mut e = engine();
    let style = TextStyle::default();
    let a = e.raster_segments(&[seg("こんにちは")], &style);
    let b = e.raster_segments(&[seg("こんにちは"), seg("、追記分です")], &style);
    assert!(b.glyphs.len() > a.glyphs.len());
    for (ga, gb) in a.glyphs.iter().zip(b.glyphs.iter()) {
        assert_eq!(ga.byte_start, gb.byte_start);
        assert_eq!((ga.x, ga.y), (gb.x, gb.y), "追記で既存グリフ位置が動いた");
    }
}

#[test]
fn color_override_changes_pixels() {
    let mut e = engine();
    let style = TextStyle {
        outline_width: 0,
        ..Default::default()
    };
    let white = e.raster_segments(&[seg("あ")], &style);
    let red = e.raster_segments(
        &[TextSegment {
            text: "あ".into(),
            color: Some([255, 0, 0, 255]),
            ..Default::default()
        }],
        &style,
    );
    let sum = |r: &RasterText, c: usize| -> u32 {
        r.glyphs
            .iter()
            .flat_map(|g| {
                g.pixmap
                    .data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(move |p| p[c] as u32)
            })
            .sum()
    };
    // 赤指定は G 成分が大きく減る
    assert!(
        sum(&red, 1) < sum(&white, 1) / 4,
        "色指定が反映されていない"
    );
}

#[test]
fn outline_grows_glyph() {
    let mut e = engine();
    let plain = TextStyle {
        outline_width: 0,
        ..Default::default()
    };
    let outlined = TextStyle {
        outline_width: 3,
        ..Default::default()
    };
    let a = e.raster_segments(&[seg("あ")], &plain);
    let b = e.raster_segments(&[seg("あ")], &outlined);
    assert_eq!(b.glyphs[0].pixmap.width, a.glyphs[0].pixmap.width + 6);
    assert_eq!(b.glyphs[0].pixmap.height, a.glyphs[0].pixmap.height + 6);
}

// -----------------------------------------------------------------
// 禁則処理: assign_lines の純粋ロジック単体テスト（実フォントに依存しない）。
// -----------------------------------------------------------------

fn line_chars(pairs: &[(char, f32)]) -> Vec<LineChar> {
    pairs
        .iter()
        .map(|&(ch, advance_px)| LineChar {
            ch,
            advance_px,
            is_break: false,
            control: None,
        })
        .collect()
}

fn offset_for(offsets: &HashMap<usize, f32>, line_i: usize) -> f32 {
    offsets.get(&line_i).copied().unwrap_or(0.0)
}

#[test]
fn align_offsets_are_exact_for_left_center_and_right() {
    let chars = line_chars(&[('a', 10.0), ('b', 20.0)]);
    let placements = assign_lines(&chars, 100.0);

    let left = alignment_offsets(&chars, &placements, 100.0, TextAlign::Left);
    let center = alignment_offsets(&chars, &placements, 100.0, TextAlign::Center);
    let right = alignment_offsets(&chars, &placements, 100.0, TextAlign::Right);

    assert_eq!(offset_for(&left, 0), 0.0);
    assert_eq!(offset_for(&center, 0), 35.0);
    assert_eq!(offset_for(&right, 0), 70.0);
}

#[test]
fn wrapped_lines_are_aligned_independently() {
    let chars = line_chars(&[('a', 60.0), ('b', 60.0), ('c', 20.0)]);
    let placements = assign_lines(&chars, 100.0);
    let offsets = alignment_offsets(&chars, &placements, 100.0, TextAlign::Center);

    assert_eq!(placements[0].line_i, 0);
    assert_eq!(placements[1].line_i, 1);
    assert_eq!(placements[2].line_i, 1);
    assert_eq!(offset_for(&offsets, 0), 20.0);
    assert_eq!(offset_for(&offsets, 1), 10.0);
}

#[test]
fn hard_broken_lines_are_aligned_independently() {
    let chars = vec![
        LineChar {
            ch: 'a',
            advance_px: 30.0,
            is_break: false,
            control: None,
        },
        LineChar {
            ch: '\n',
            advance_px: 0.0,
            is_break: true,
            control: None,
        },
        LineChar {
            ch: 'b',
            advance_px: 10.0,
            is_break: false,
            control: None,
        },
    ];
    let placements = assign_lines(&chars, 100.0);
    let offsets = alignment_offsets(&chars, &placements, 100.0, TextAlign::Right);

    assert_eq!(offset_for(&offsets, 0), 70.0);
    assert_eq!(offset_for(&offsets, 1), 90.0);
}

#[test]
fn overfull_line_alignment_is_clamped_to_zero() {
    let chars = line_chars(&[('a', 120.0)]);
    let placements = assign_lines(&chars, 100.0);

    for align in [TextAlign::Center, TextAlign::Right] {
        let offsets = alignment_offsets(&chars, &placements, 100.0, align);
        assert_eq!(offset_for(&offsets, 0), 0.0);
    }
}

#[test]
fn locate_disables_alignment_but_indent_does_not() {
    let with_locate = vec![
        LineChar {
            ch: 'a',
            advance_px: 10.0,
            is_break: false,
            control: None,
        },
        LineChar {
            ch: '\0',
            advance_px: 0.0,
            is_break: false,
            control: Some(CharControl::Locate {
                pen_x: 40.0,
                line_i: None,
            }),
        },
        LineChar {
            ch: 'b',
            advance_px: 10.0,
            is_break: false,
            control: None,
        },
    ];
    let locate_placements = assign_lines(&with_locate, 100.0);
    let locate_offsets =
        alignment_offsets(&with_locate, &locate_placements, 100.0, TextAlign::Center);
    assert_eq!(locate_placements[2].pen_x, 40.0);
    assert_eq!(offset_for(&locate_offsets, 0), 0.0);

    let with_indent = vec![
        LineChar {
            ch: 'a',
            advance_px: 10.0,
            is_break: false,
            control: None,
        },
        LineChar {
            ch: '\0',
            advance_px: 0.0,
            is_break: false,
            control: Some(CharControl::IndentSet),
        },
        LineChar {
            ch: 'b',
            advance_px: 10.0,
            is_break: false,
            control: None,
        },
    ];
    let indent_placements = assign_lines(&with_indent, 100.0);
    let indent_offsets =
        alignment_offsets(&with_indent, &indent_placements, 100.0, TextAlign::Center);
    assert_eq!(offset_for(&indent_offsets, 0), 40.0);
}

#[test]
fn missing_align_deserializes_as_left() {
    let style: TextStyle = serde_json::from_str("{}").unwrap();
    assert_eq!(style.align, TextAlign::Left);
}

#[test]
fn kinsoku_hangs_leading_punctuation() {
    // "あいう" で 30px ちょうど。続く "。" は行頭禁則文字なのでぶら下がり、
    // "う" と同じ行に残るはず。次の "え" は新しい行へ。
    let chars = line_chars(&[
        ('あ', 10.0),
        ('い', 10.0),
        ('う', 10.0),
        ('。', 10.0),
        ('え', 10.0),
    ]);
    let placements = assign_lines(&chars, 30.0);
    assert_eq!(
        placements[2].line_i, placements[3].line_i,
        "「。」が前行にぶら下がっていない"
    );
    assert!(
        placements[3].pen_x >= 30.0,
        "ぶら下げた文字が box_w を超えて配置されていない: {}",
        placements[3].pen_x
    );
    assert_eq!(
        placements[4].line_i,
        placements[3].line_i + 1,
        "「え」が新しい行に来ていない"
    );
    assert_eq!(placements[4].pen_x, 0.0);
}

#[test]
fn kinsoku_pulls_back_trailing_open_bracket() {
    // "あいう「" で 40px。"「" は行末禁則文字なので、続く "え" とまとめて
    // 次の行へ道連れになるはず（"あいう" だけが元の行に残る）。
    let chars = line_chars(&[
        ('あ', 10.0),
        ('い', 10.0),
        ('う', 10.0),
        ('「', 10.0),
        ('え', 10.0),
    ]);
    let placements = assign_lines(&chars, 30.0);
    assert_eq!(
        placements[3].line_i, placements[4].line_i,
        "「「」が道連れで次行送りになっていない"
    );
    assert_ne!(
        placements[2].line_i, placements[3].line_i,
        "「う」まで巻き込まれてしまった"
    );
    assert_eq!(
        placements[3].pen_x, 0.0,
        "道連れ後の「「」は新しい行の先頭に来るはず"
    );
}

#[test]
fn kinsoku_chain_of_closing_brackets_pulled_back_together() {
    // 行末禁則文字が連続する場合（"（" の直後に "「"）も、両方まとめて次行送りになる。
    let chars = line_chars(&[
        ('あ', 10.0),
        ('い', 10.0),
        ('（', 10.0),
        ('「', 10.0),
        ('う', 10.0),
    ]);
    let placements = assign_lines(&chars, 30.0);
    assert_eq!(placements[2].line_i, placements[3].line_i);
    assert_eq!(placements[3].line_i, placements[4].line_i);
    assert_ne!(placements[1].line_i, placements[2].line_i);
}

#[test]
fn explicit_newline_resets_pen_and_increments_line() {
    let chars = vec![
        LineChar {
            ch: 'あ',
            advance_px: 10.0,
            is_break: false,
            control: None,
        },
        LineChar {
            ch: '\n',
            advance_px: 0.0,
            is_break: true,
            control: None,
        },
        LineChar {
            ch: 'い',
            advance_px: 10.0,
            is_break: false,
            control: None,
        },
    ];
    let placements = assign_lines(&chars, 1000.0);
    assert_eq!(placements[0].line_i, 0);
    assert_eq!(placements[2].line_i, 1);
    assert_eq!(placements[2].pen_x, 0.0);
}

// -----------------------------------------------------------------
// 禁則処理: 実フォント経由の統合テスト（行頭ぶら下げが実際に発生することを、
// 無制限幅で測った位置から box_w を逆算して font に依存せず確認する）。
// -----------------------------------------------------------------

#[test]
fn kinsoku_integration_hangs_period_at_line_start() {
    let mut e = engine();
    let base_style = TextStyle {
        box_w: 100_000,
        box_h: 4000,
        font_size: 24.0,
        outline_width: 0,
        ..Default::default()
    };
    let text = "あいうえおかきくけこ。さしすせそ";
    let measured = e.raster_segments(&[seg(text)], &base_style);
    let punct_byte = text.find('。').expect("「。」が見つからない");
    let punct_x = measured
        .glyphs
        .iter()
        .find(|g| g.byte_start == punct_byte)
        .map(|g| g.x)
        .expect("「。」の glyph が見つからない");
    // "。" を置くとちょうど超過する幅に box_w を設定する。
    let style = TextStyle {
        box_w: punct_x.max(1) as u32,
        ..base_style
    };
    let r = e.raster_segments(&[seg(text)], &style);

    let prev_byte = text[..punct_byte].char_indices().last().unwrap().0;
    let line_of = |byte: usize| {
        r.glyphs
            .iter()
            .find(|g| g.byte_start == byte)
            .map(|g| g.line_i)
    };
    assert_eq!(
        line_of(punct_byte),
        line_of(prev_byte),
        "行頭禁則文字「。」が前行にぶら下がっていない"
    );
}

#[test]
fn fallback_renders_japanese_with_unknown_family() {
    let mut e = engine();
    let style = TextStyle {
        font_family: "NoSuchFontFamily_QWERTY_12345".into(),
        ..Default::default()
    };
    let r = e.raster_segments(&[seg("あ")], &style);
    assert!(
        r.glyphs
            .iter()
            .any(|g| g.pixmap.data.iter().any(|&b| b != 0)),
        "存在しない family 指定時にフォールバックで日本語が描画されていません"
    );
}

// -----------------------------------------------------------------
// 配布対応 W1: ゲームローカルフォント（`load_game_fonts`）の優先解決
// -----------------------------------------------------------------

#[test]
fn game_font_resolves_by_unique_family_name_before_any_fallback() {
    // どのシステムフォントとも衝突しないユニークな family 名を、ゲームフォント
    // 側にだけ登録する。`resolve_base` が `game_registry.resolve(requested)` を
    // 真っ先に試すため、システムフォント側の解決を一切経由せずに描画できるはず。
    let mut e = engine();
    let bytes = crate::font::tests::minimal_font_bytes("Noiria Game Font Unique 12345");
    e.load_game_fonts(vec![bytes]);
    let style = TextStyle {
        font_family: "Noiria Game Font Unique 12345".into(),
        outline_width: 0,
        font_size: 28.0,
        ..Default::default()
    };
    // 手組みフォントの cmap は 'A'..'D' だけを glyph 1..4 へマップする（font/tests.rs
    // cmap_table 参照）。'A' は glyph1（100,100)-(600,600) の正方形。
    let r = e.raster_segments(&[seg("A")], &style);
    assert_eq!(
        r.glyphs.len(),
        1,
        "ゲームフォント経由でグリフが描画されていない"
    );
    assert!(
        r.glyphs[0].pixmap.data.iter().any(|&b| b != 0),
        "描画されたグリフが空ピクセルだった"
    );
}

#[test]
fn game_font_is_used_as_ultimate_fallback_even_without_family_name_match() {
    // 要求 family がゲームフォント・システムフォントいずれとも一致しない場合でも、
    // `resolve_base` は OS 名チェーンより先に `game_registry.any()`（登録順の先頭 =
    // 唯一のゲームフォント）を試す。
    let mut e = engine();
    let bytes = crate::font::tests::minimal_font_bytes("Some Arbitrary Game Font Name");
    e.load_game_fonts(vec![bytes]);
    let style = TextStyle {
        font_family: "RequestedNameThatMatchesNothing_98765".into(),
        outline_width: 0,
        font_size: 28.0,
        ..Default::default()
    };
    let r = e.raster_segments(&[seg("A")], &style);
    assert_eq!(
        r.glyphs.len(),
        1,
        "family 名不一致時にゲームフォントへフォールバックしていない"
    );
}

/// 既定の family 名（"Yu Gothic UI"）がシステムにあっても、同梱フォントがあれば同梱側を使う
/// （従来は Windows だけシステム側が先に当たり、family が無い Linux だけ同梱が使われた）。
#[test]
fn bundled_font_beats_system_font_for_default_family() {
    let bytes = crate::font::tests::minimal_font_bytes("Zz Bundled Only Font 777");
    let mut bundled = engine();
    bundled.load_game_fonts(vec![bytes]);
    let unspecified = TextStyle {
        outline_width: 0,
        ..Default::default()
    };
    let named = TextStyle {
        font_family: "Zz Bundled Only Font 777".into(),
        outline_width: 0,
        ..Default::default()
    };
    let by_default = bundled.raster_segments(&[seg("A")], &unspecified);
    let by_name = bundled.raster_segments(&[seg("A")], &named);
    assert_eq!(by_default.glyphs.len(), 1);
    assert_eq!(
        by_default.glyphs[0].pixmap.data, by_name.glyphs[0].pixmap.data,
        "未指定の family でも同梱フォントが選ばれるはず"
    );
    let sys = engine().raster_segments(&[seg("A")], &unspecified);
    if let Some(g) = sys.glyphs.first() {
        assert_ne!(
            g.pixmap.data, by_default.glyphs[0].pixmap.data,
            "システムフォントの A と同じ見た目になっている（同梱が無視された）"
        );
    }
}

/// 要求 family がどのフォントとも一致しなくても、同梱フォントがシステムフォントより優先される
/// （システムに実在する family を明示指定しても同じ）。
#[test]
fn bundled_font_beats_system_font_for_explicit_non_matching_family() {
    let bytes = crate::font::tests::minimal_font_bytes("Zz Bundled Only Font 778");
    let mut e = engine();
    e.load_game_fonts(vec![bytes]);
    for family in ["Meiryo", "Noto Sans CJK JP", "NoSuchFamily_1"] {
        let style = TextStyle {
            font_family: family.into(),
            outline_width: 0,
            ..Default::default()
        };
        let key = e.resolve_base(&style.font_family).unwrap();
        assert!(
            matches!(key.source, FontSource::Bytes(_)),
            "{family} 指定で同梱フォントが選ばれていない"
        );
    }
}

/// `TextSegment::font_family`（[font face=]）はそのセグメントだけ別の同梱フォントへ切り替える。
#[test]
fn segment_font_family_switches_font_per_segment() {
    let small = crate::font::tests::minimal_font_bytes_with_upem("Zz Seg Small", 1000);
    let big = crate::font::tests::minimal_font_bytes_with_upem("Zz Seg Big", 500);
    let mut e = engine();
    e.load_game_fonts(vec![small, big]);
    let style = TextStyle {
        outline_width: 0,
        font_size: 28.0,
        ..Default::default()
    };
    let mut big_seg = seg("A");
    big_seg.font_family = Some("Zz Seg Big".into());
    let mut unknown_seg = seg("A");
    unknown_seg.font_family = Some("Zz Unknown".into());
    let r = e.raster_segments(&[seg("A"), big_seg, unknown_seg], &style);
    assert_eq!(r.glyphs.len(), 3);
    let w: Vec<u32> = r.glyphs.iter().map(|g| g.pixmap.width).collect();
    assert!(
        w[1] > w[0],
        "face 指定のセグメントだけ別フォント（2 倍の大きさ）になるはず: {w:?}"
    );
    assert_eq!(w[0], w[2], "未知の face は基準フォントへ戻る: {w:?}");
    // 行メトリクスは基準フォントのまま: 3 つとも同じ行。
    assert!(r.glyphs.iter().all(|g| g.line_i == 0));
}

#[test]
fn without_game_fonts_behavior_is_unchanged() {
    // ゲームフォント未登録（既定状態）では `game_registry` が空のため、`resolve_base`/
    // `glyph_for_char` の game_registry 呼び出しは即 None を返し、従来どおりシステム
    // フォント経由の解決になる（Windows の既存挙動が変わらないことの回帰確認）。
    let mut e = engine();
    let style = TextStyle::default();
    let r = e.raster_segments(&[seg("あ")], &style);
    assert!(
        r.glyphs
            .iter()
            .any(|g| g.pixmap.data.iter().any(|&b| b != 0)),
        "ゲームフォント未登録時に日本語が描画できなくなっている（回帰）"
    );
}

// -----------------------------------------------------------------
// Wave 1: 文字装飾系タグ（font/style/ruby/locate/indent 等）
// -----------------------------------------------------------------

#[test]
fn mixed_font_sizes_share_baseline() {
    // 設計判断 8: line_advance はボックス基準 font_size で固定。セグメントごとの
    // size 違いは、そのセグメント自身のグリフ拡大にのみ影響し、同じ行にある
    // 他セグメントのベースライン (baseline_y) には影響してはいけない。
    let mut e = engine();
    let style = TextStyle {
        outline_width: 0,
        ..Default::default()
    };
    let alone = e.raster_segments(&[seg("A")], &style);
    let mixed = e.raster_segments(
        &[
            TextSegment {
                text: "A".into(),
                ..Default::default()
            },
            TextSegment {
                text: "B".into(),
                size: Some(style.font_size * 2.0),
                ..Default::default()
            },
        ],
        &style,
    );
    assert_eq!(alone.glyphs.len(), 1);
    assert!(mixed.glyphs.len() >= 2);
    assert_eq!(
        alone.glyphs[0].y, mixed.glyphs[0].y,
        "サイズの大きい後続セグメントによって先頭グリフのベースラインが変わった"
    );
    assert_eq!(
        mixed.glyphs[0].line_i, mixed.glyphs[1].line_i,
        "折返しが発生してしまった（テスト前提が崩れている）"
    );
}

#[test]
fn ruby_glyphs_centered_above_segment() {
    let mut e = engine();
    let style = TextStyle {
        outline_width: 0,
        align: TextAlign::Center,
        ..Default::default()
    };
    let text = "今日";
    let text_len = text.len();
    let r = e.raster_segments(
        &[TextSegment {
            text: text.into(),
            ruby: Some("きょう".into()),
            ..Default::default()
        }],
        &style,
    );
    let base_glyphs: Vec<&GlyphRaster> = r
        .glyphs
        .iter()
        .filter(|g| g.byte_start < text_len)
        .collect();
    let ruby_glyphs: Vec<&GlyphRaster> = r
        .glyphs
        .iter()
        .filter(|g| g.byte_start == text_len)
        .collect();
    assert_eq!(base_glyphs.len(), 2, "本文「今日」のグリフ数が想定と違う");
    assert_eq!(ruby_glyphs.len(), 3, "ルビ「きょう」のグリフ数が想定と違う");

    let center = |gs: &[&GlyphRaster]| -> i32 {
        let min_x = gs.iter().map(|g| g.x).min().unwrap();
        let max_x = gs
            .iter()
            .map(|g| g.x + g.pixmap.width as i32)
            .max()
            .unwrap();
        (min_x + max_x) / 2
    };
    let base_center = center(&base_glyphs);
    let ruby_center = center(&ruby_glyphs);
    assert!(
        (ruby_center - base_center).abs() <= 4,
        "ルビが本文セグメントの中心からずれている: base={base_center} ruby={ruby_center}"
    );

    let base_top = base_glyphs.iter().map(|g| g.y).min().unwrap();
    let ruby_top = ruby_glyphs.iter().map(|g| g.y).min().unwrap();
    assert!(
        ruby_top < base_top,
        "ルビが本文より上に配置されていない: base_top={base_top} ruby_top={ruby_top}"
    );

    // reveal の Vec 順序前提: ルビは本文セグメントの直後に挿入されている。
    let last_base_idx = r
        .glyphs
        .iter()
        .rposition(|g| g.byte_start < text_len)
        .unwrap();
    let first_ruby_idx = r
        .glyphs
        .iter()
        .position(|g| g.byte_start == text_len)
        .unwrap();
    assert_eq!(
        first_ruby_idx,
        last_base_idx + 1,
        "ルビが本文セグメントの直後に挿入されていない"
    );
}

#[test]
fn indent_applies_to_wrapped_lines() {
    // 「あいう」(10px×3=30px) の直後に [indent] を発行し、続く「え」で box_w=35 を
    // 超えて折返しが起きるとき、新しい行の先頭 pen_x が indent 値 (30.0) と一致するはず。
    let chars = vec![
        LineChar {
            ch: 'あ',
            advance_px: 10.0,
            is_break: false,
            control: None,
        },
        LineChar {
            ch: 'い',
            advance_px: 10.0,
            is_break: false,
            control: None,
        },
        LineChar {
            ch: 'う',
            advance_px: 10.0,
            is_break: false,
            control: None,
        },
        LineChar {
            ch: '\0',
            advance_px: 0.0,
            is_break: false,
            control: Some(CharControl::IndentSet),
        },
        LineChar {
            ch: 'え',
            advance_px: 10.0,
            is_break: false,
            control: None,
        },
        LineChar {
            ch: 'お',
            advance_px: 10.0,
            is_break: false,
            control: None,
        },
    ];
    let placements = assign_lines(&chars, 35.0);
    assert_eq!(
        placements[3].pen_x, 30.0,
        "indent セット時点の pen_x が想定と違う"
    );
    assert_ne!(
        placements[4].line_i, placements[2].line_i,
        "折返しが発生していない"
    );
    assert_eq!(
        placements[4].pen_x, 30.0,
        "折返し後の行頭 x が indent 値になっていない"
    );
}

#[test]
fn bold_extends_coverage_left() {
    let mut e = engine();
    let style = TextStyle {
        outline_width: 0,
        ..Default::default()
    };
    let plain = e.raster_segments(&[seg("あ")], &style);
    let bold = e.raster_segments(
        &[TextSegment {
            text: "あ".into(),
            bold: Some(true),
            ..Default::default()
        }],
        &style,
    );
    assert_eq!(plain.glyphs.len(), 1);
    assert_eq!(bold.glyphs.len(), 1);
    let coverage = |r: &RasterText| -> u32 {
        r.glyphs[0]
            .pixmap
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3] as u32)
            .sum()
    };
    assert!(
        coverage(&bold) > coverage(&plain),
        "bold でカバレッジが増えていない: plain={} bold={}",
        coverage(&plain),
        coverage(&bold)
    );
    // advance もキャンバスサイズも変えない簡略実装（設計判断 5）。
    assert_eq!(bold.glyphs[0].pixmap.width, plain.glyphs[0].pixmap.width);
    assert_eq!(bold.glyphs[0].pixmap.height, plain.glyphs[0].pixmap.height);
}

#[test]
fn edge_color_overrides_style_outline() {
    let mut e = engine();
    let style = TextStyle {
        outline_width: 3,
        outline_color: [0, 0, 0, 200],
        ..Default::default()
    };
    let default_outline = e.raster_segments(&[seg("あ")], &style);
    let overridden = e.raster_segments(
        &[TextSegment {
            text: "あ".into(),
            edge_color: Some([0, 255, 0, 200]),
            ..Default::default()
        }],
        &style,
    );
    let sum_green = |r: &RasterText| -> u32 {
        r.glyphs
            .iter()
            .flat_map(|g| {
                g.pixmap
                    .data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(move |p| p[1] as u32)
            })
            .sum()
    };
    assert!(
        sum_green(&overridden) > sum_green(&default_outline) + 1000,
        "edge_color が style.outline_color を上書きしていない: default={} overridden={}",
        sum_green(&default_outline),
        sum_green(&overridden)
    );
}

// -----------------------------------------------------------------
// Wave 3: 複数メッセージレイヤ（[graph] インライン画像）
// -----------------------------------------------------------------

fn tiny_image(w: u32, h: u32) -> Pixmap {
    let mut p = Pixmap::new(w, h);
    p.fill([255, 128, 0, 255]);
    p
}

#[test]
fn inline_image_renders_as_one_glyph_with_baseline_bottom_alignment() {
    let mut e = engine();
    e.cache_image("icon.png".into(), tiny_image(20, 12));
    let style = TextStyle {
        outline_width: 0,
        ..Default::default()
    };
    // 別セグメントとして送る（printImage が独立した print 呼び出しで追記する実際の
    // 使い方に合わせる）: resolved 順は登場順どおり "A" → 画像。
    let r = e.raster_segments(
        &[
            seg("A"),
            TextSegment {
                text: "".into(),
                inline_image: Some("icon.png".into()),
                ..Default::default()
            },
        ],
        &style,
    );
    // "A" と画像で 2 グリフ扱い
    assert_eq!(
        r.glyphs.len(),
        2,
        "インライン画像がグリフとして扱われていない"
    );
    let img = &r.glyphs[1];
    assert_eq!(
        (img.pixmap.width, img.pixmap.height),
        (20, 12),
        "画像サイズがそのまま反映されていない"
    );
    let text_glyph = &r.glyphs[0];
    // ベースライン下端揃え: 画像の下端 y は本文グリフの下端付近に来るはず
    // (baseline_y は共通。画像は top=height なので y = baseline_y - height)
    let text_bottom = text_glyph.y + text_glyph.pixmap.height as i32;
    let img_bottom = img.y + img.pixmap.height as i32;
    assert!(
        (img_bottom - text_bottom).abs() <= 2,
        "画像がベースライン下端に揃っていない: text_bottom={text_bottom} img_bottom={img_bottom}"
    );
}

#[test]
fn inline_image_unknown_path_is_ignored_as_zero_width() {
    let mut e = engine();
    let style = TextStyle::default();
    let r = e.raster_segments(
        &[TextSegment {
            text: "A".into(),
            inline_image: Some("missing.png".into()),
            ..Default::default()
        }],
        &style,
    );
    assert_eq!(r.glyphs.len(), 1, "未キャッシュの画像パスは無視されるはず");
}

#[test]
fn appending_keeps_prefix_glyph_count_with_inline_image() {
    // 追記時キャッシュ (ops_text) の前提: [graph] を含む場合でも前方グリフは不変。
    let mut e = engine();
    e.cache_image("icon.png".into(), tiny_image(16, 16));
    let style = TextStyle::default();
    let seg_a = TextSegment {
        text: "こんにちは".into(),
        inline_image: Some("icon.png".into()),
        ..Default::default()
    };
    let a = e.raster_segments(std::slice::from_ref(&seg_a), &style);
    let b = e.raster_segments(&[seg_a, seg("、追記分です")], &style);
    assert!(b.glyphs.len() > a.glyphs.len());
    for (ga, gb) in a.glyphs.iter().zip(b.glyphs.iter()) {
        assert_eq!(ga.byte_start, gb.byte_start);
        assert_eq!((ga.x, ga.y), (gb.x, gb.y), "追記で既存グリフ位置が動いた");
    }
}
/// どのフォントにも無い文字が出るたびに全システムフォントを読み込んで保持し続けない
/// （U+10FFFE は非文字で、実在のフォントは cmap に持たない）。
#[test]
fn char_missing_from_every_font_does_not_load_the_whole_registry() {
    let mut e = engine();
    e.load_game_fonts(vec![crate::font::tests::minimal_font_bytes(
        "Missing Glyph Probe",
    )]);
    let style = TextStyle {
        font_family: "Missing Glyph Probe".into(),
        outline_width: 0,
        font_size: 28.0,
        ..Default::default()
    };
    let text = "A\u{10FFFE}B\u{10FFFE}\u{10FFFE}";
    let first = e.raster_segments(&[seg(text)], &style);
    assert!(
        e.fonts.len() <= 2,
        "グリフ探索のためにフォントを {} 個も保持している",
        e.fonts.len()
    );
    // 2 回目以降は探索をやり直さない（結果は同じ・フォント数も増えない）。
    let t = std::time::Instant::now();
    let second = e.raster_segments(&[seg(text)], &style);
    assert!(e.fonts.len() <= 2);
    assert_eq!(first.glyphs.len(), second.glyphs.len());
    assert!(
        t.elapsed().as_millis() < 500,
        "2 回目の描画が遅い: {:?}",
        t.elapsed()
    );
}

#[test]
fn image_cache_evicts_lru_and_updates_recency_on_has_image_and_render() {
    let mut e = engine();
    let one = 8 * 8 * 4;
    e.image_cache = ImageCache::new(2 * one);

    e.cache_image("a.png".into(), tiny_image(8, 8));
    e.cache_image("b.png".into(), tiny_image(8, 8));
    assert_eq!(e.image_cache.len(), 2);
    assert_eq!(e.image_cache.bytes(), 2 * one);

    assert!(e.has_image("a.png"));
    e.cache_image("c.png".into(), tiny_image(8, 8));
    assert_eq!(e.image_cache.len(), 2);
    assert_eq!(e.image_cache.bytes(), 2 * one);
    assert!(!e.has_image("b.png"));
    assert!(e.has_image("a.png"));
    assert!(e.has_image("c.png"));

    let r = e.raster_segments(
        &[TextSegment {
            text: "".into(),
            inline_image: Some("a.png".into()),
            ..Default::default()
        }],
        &TextStyle::default(),
    );
    assert_eq!(r.glyphs.len(), 1);
    e.cache_image("d.png".into(), tiny_image(8, 8));
    assert_eq!(e.image_cache.len(), 2);
    assert_eq!(e.image_cache.bytes(), 2 * one);
    assert!(!e.has_image("c.png"));
    assert!(e.has_image("a.png"));
    assert!(e.has_image("d.png"));

    e.cache_image("a.png".into(), tiny_image(32, 32));
    assert!(!e.has_image("a.png"));
    assert_eq!(e.image_cache.len(), 1);
    assert_eq!(e.image_cache.bytes(), one);
}

#[test]
fn fast_rasterize_glyph_matches_naive_reference_pixel_for_pixel() {
    fn naive(
        w: u32,
        h: u32,
        data: &[u8],
        fill: [u8; 4],
        outline_color: [u8; 4],
        outline_width: u32,
        bold: bool,
    ) -> Pixmap {
        let pad = outline_width as i32;
        let ow = w as i32 + pad * 2;
        let oh = h as i32 + pad * 2;
        let mut out = Pixmap::new(ow as u32, oh as u32);
        let mask_at = |x: i32, y: i32| -> u32 {
            if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                return 0;
            }
            data[(y as usize) * (w as usize) + (x as usize)] as u32
        };
        if outline_width > 0 && outline_color[3] > 0 {
            for oy in 0..oh {
                for ox in 0..ow {
                    let mut m = 0u32;
                    for dy in -pad..=pad {
                        for dx in -pad..=pad {
                            if dx * dx + dy * dy <= pad * pad {
                                m = m.max(mask_at(ox - pad + dx, oy - pad + dy));
                            }
                        }
                    }
                    if m == 0 {
                        continue;
                    }
                    let a = m * outline_color[3] as u32 / 255;
                    let idx = (oy as usize * ow as usize + ox as usize) * 4;
                    out.data[idx] = (outline_color[0] as u32 * a / 255) as u8;
                    out.data[idx + 1] = (outline_color[1] as u32 * a / 255) as u8;
                    out.data[idx + 2] = (outline_color[2] as u32 * a / 255) as u8;
                    out.data[idx + 3] = a as u8;
                }
            }
        }
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let m = if bold {
                    mask_at(x, y).max(mask_at(x - 1, y))
                } else {
                    mask_at(x, y)
                };
                if m == 0 {
                    continue;
                }
                let a = m * fill[3] as u32 / 255;
                let (sr, sg, sb, sa) = (
                    fill[0] as u32 * a / 255,
                    fill[1] as u32 * a / 255,
                    fill[2] as u32 * a / 255,
                    a,
                );
                let didx = ((y + pad) as usize * ow as usize + (x + pad) as usize) * 4;
                let inv = 255 - sa;
                out.data[didx] = (sr + out.data[didx] as u32 * inv / 255).min(255) as u8;
                out.data[didx + 1] = (sg + out.data[didx + 1] as u32 * inv / 255).min(255) as u8;
                out.data[didx + 2] = (sb + out.data[didx + 2] as u32 * inv / 255).min(255) as u8;
                out.data[didx + 3] = (sa + out.data[didx + 3] as u32 * inv / 255).min(255) as u8;
            }
        }
        out
    }

    let mut seed = 0x1234_5678u64;
    for &(w, h) in &[(1u32, 1u32), (3, 7), (19, 23), (32, 28)] {
        let data: Vec<u8> = (0..(w * h))
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                if (seed >> 56) < 40 {
                    (seed >> 33) as u8
                } else {
                    0
                }
            })
            .collect();
        for &pad in &[0u32, 1, 2, 3, 5, 14] {
            for &bold in &[false, true] {
                let got = rasterize_glyph(
                    w,
                    h,
                    &data,
                    [240, 200, 120, 230],
                    [10, 20, 40, 180],
                    pad,
                    bold,
                );
                let want = naive(
                    w,
                    h,
                    &data,
                    [240, 200, 120, 230],
                    [10, 20, 40, 180],
                    pad,
                    bold,
                );
                assert_eq!(got.data, want.data, "w={w} h={h} pad={pad} bold={bold}");
            }
        }
    }
}

#[test]
fn rasterize_glyph_clamps_huge_outline_width() {
    let mask = [255u8; 4]; // 2x2
    let clamped = rasterize_glyph(
        2,
        2,
        &mask,
        [255; 4],
        [0, 0, 0, 255],
        MAX_OUTLINE_WIDTH,
        false,
    );
    let huge = rasterize_glyph(2, 2, &mask, [255; 4], [0, 0, 0, 255], u32::MAX, false);
    assert_eq!(huge.width, 2 + MAX_OUTLINE_WIDTH * 2);
    assert_eq!(huge.height, 2 + MAX_OUTLINE_WIDTH * 2);
    assert_eq!(huge.data, clamped.data);
}
