//! raster モジュールのテスト。
//!
//! - `square_area_matches_expected`: 整数境界の正方形 → 期待ピクセル数と誤差 2% 未満で一致。
//! - `donut_hole_is_excluded_by_nonzero_winding`: 内側に逆回りの正方形 → 穴が抜ける
//!   （nonzero winding rule の検証）。
//! - `triangle_diagonal_edge_has_partial_coverage`: 斜辺のあるピクセルで
//!   0 と 255 の間の階調が出る（アンチエイリアスの検証）。
//! - `text_engine_matches_direct_rasterize_*`: 実システムフォント（存在すれば）で、
//!   `text.rs` の `TextEngine` 経由のグリフラスタが `font::rasterize` を直接呼んだ
//!   場合と完全一致することを確認する自己無矛盾性テスト（F3 で cosmic-text を削除
//!   したため、他ライブラリとのクロスチェックはできなくなった代わりに、
//!   同じ自作パイプラインの 2 つの経由路が食い違っていないかを検証する）。

use super::*;

fn seg_line(p0: (f32, f32), p1: (f32, f32)) -> Segment {
    Segment::Line(Point { x: p0.0, y: p0.1 }, Point { x: p1.0, y: p1.1 })
}

fn square_outline(x0: f32, y0: f32, x1: f32, y1: f32) -> Outline {
    // 反時計回り（フォント空間 y-up での CCW）。
    let contour = Contour {
        segments: vec![
            seg_line((x0, y0), (x1, y0)),
            seg_line((x1, y0), (x1, y1)),
            seg_line((x1, y1), (x0, y1)),
            seg_line((x0, y1), (x0, y0)),
        ],
    };
    Outline {
        contours: vec![contour],
    }
}

fn square_outline_cw(x0: f32, y0: f32, x1: f32, y1: f32) -> Contour {
    // 時計回り（`square_outline` と逆方向）。ドーナツの穴を作るのに使う。
    Contour {
        segments: vec![
            seg_line((x0, y0), (x0, y1)),
            seg_line((x0, y1), (x1, y1)),
            seg_line((x1, y1), (x1, y0)),
            seg_line((x1, y0), (x0, y0)),
        ],
    }
}

fn coverage_sum(mask: &GlyphMask) -> f64 {
    mask.data.iter().map(|&v| v as f64 / 255.0).sum()
}

#[test]
fn square_area_matches_expected() {
    // 20x20 の正方形、scale=1.0（整数境界なのでエッジはピクセル境界にちょうど乗る）。
    let outline = square_outline(0.0, 0.0, 20.0, 20.0);
    let mask = rasterize(&outline, 1.0);
    assert_eq!(mask.width, 20);
    assert_eq!(mask.height, 20);

    let expected_area = 400.0;
    let actual_area = coverage_sum(&mask);
    let err = (actual_area - expected_area).abs() / expected_area;
    assert!(
        err < 0.02,
        "面積誤差が大きすぎます: expected={expected_area} actual={actual_area} err={err}"
    );

    // 整数境界の軸並行正方形は内部が完全に不透明、外部は完全に透明になるはず。
    for &v in &mask.data {
        assert!(v == 0 || v == 255, "整数境界の正方形に中間値が出た: {v}");
    }
    // 内部の 1 点が不透明であることも直接確認する。
    assert_eq!(mask.coverage_at(10, 10), 255);
}

#[test]
fn donut_hole_is_excluded_by_nonzero_winding() {
    // 外側 40x40 (CCW) + 内側 20x20 (CW、逆回り) => 中抜きドーナツ。
    let mut outline = square_outline(0.0, 0.0, 40.0, 40.0);
    outline
        .contours
        .push(square_outline_cw(10.0, 10.0, 30.0, 30.0));

    let mask = rasterize(&outline, 1.0);
    assert_eq!(mask.width, 40);
    assert_eq!(mask.height, 40);

    // 穴の中心は透明。
    assert_eq!(mask.coverage_at(20, 20), 0, "穴の中心が抜けていません");
    // 外周の帯は不透明。
    assert_eq!(mask.coverage_at(2, 2), 255, "外周が塗られていません");
    assert_eq!(mask.coverage_at(20, 2), 255, "上辺の帯が塗られていません");

    let expected_area = 40.0 * 40.0 - 20.0 * 20.0; // 1200
    let actual_area = coverage_sum(&mask);
    let err = (actual_area - expected_area).abs() / expected_area;
    assert!(
        err < 0.02,
        "ドーナツの面積誤差が大きすぎます: expected={expected_area} actual={actual_area}"
    );
}

#[test]
fn single_square_fill_independent_of_winding_direction() {
    // 単一の閉路であれば時計回り/反時計回りどちらでも塗りは同じになるはず
    // (nonzero rule は絶対値でなく非ゼロ判定なので)。
    let ccw = Outline {
        contours: vec![square_outline(0.0, 0.0, 10.0, 10.0).contours.remove(0)],
    };
    let cw = Outline {
        contours: vec![square_outline_cw(0.0, 0.0, 10.0, 10.0)],
    };
    let mask_ccw = rasterize(&ccw, 1.0);
    let mask_cw = rasterize(&cw, 1.0);
    assert_eq!(mask_ccw.data, mask_cw.data);
}

#[test]
fn triangle_diagonal_edge_has_partial_coverage() {
    // 直角三角形。斜辺 (20,0)->(0,20) 付近のピクセルは部分被覆になるはず。
    let contour = Contour {
        segments: vec![
            seg_line((0.0, 0.0), (20.0, 0.0)),
            seg_line((20.0, 0.0), (0.0, 20.0)),
            seg_line((0.0, 20.0), (0.0, 0.0)),
        ],
    };
    let outline = Outline {
        contours: vec![contour],
    };
    let mask = rasterize(&outline, 1.0);

    let has_full = mask.data.contains(&255);
    let has_empty = mask.data.contains(&0);
    let has_partial = mask.data.iter().any(|&v| v > 0 && v < 255);
    assert!(has_full, "完全被覆のピクセルがありません");
    assert!(has_empty, "完全に空のピクセルがありません");
    assert!(
        has_partial,
        "アンチエイリアスの中間階調が出ていません: {:?}",
        mask.data
    );

    // 直角三角形の面積は 200、AA の丸めがあっても大きくは外れないはず。
    let expected_area = 200.0;
    let actual_area = coverage_sum(&mask);
    let err = (actual_area - expected_area).abs() / expected_area;
    assert!(
        err < 0.05,
        "三角形の面積誤差が大きすぎます: expected={expected_area} actual={actual_area}"
    );
}

#[test]
fn empty_outline_produces_empty_mask() {
    let outline = Outline::default();
    let mask = rasterize(&outline, 1.0);
    assert_eq!(mask.width, 0);
    assert_eq!(mask.height, 0);
    assert!(mask.data.is_empty());
}

#[test]
fn glyph_mask_placement_matches_bbox() {
    // 原点から離れた矩形: left/top がフォント空間の bbox から正しく導出されているか。
    let outline = square_outline(5.0, -3.0, 15.0, 7.0);
    let mask = rasterize(&outline, 2.0); // scale=2.0 -> [10,-6]..[30,14]
    assert_eq!(mask.left, 10);
    assert_eq!(mask.top, 14);
    assert_eq!(mask.width, 20);
    assert_eq!(mask.height, 20);
}

#[test]
fn glyph_cache_evicts_lru_beyond_capacity() {
    let mut cache = GlyphCache::new(2);
    let mk = |v: u8| GlyphMask {
        width: 1,
        height: 1,
        left: 0,
        top: 0,
        data: vec![v],
    };
    cache.get_or_insert_with(1, 100, || mk(1));
    cache.get_or_insert_with(2, 100, || mk(2));
    assert_eq!(cache.len(), 2);
    // glyph 1 を再アクセスして最近使用にする。
    cache.get_or_insert_with(1, 100, || mk(1));
    // 3つ目を入れると LRU (glyph 2) が追い出される。
    cache.get_or_insert_with(3, 100, || mk(3));
    assert_eq!(cache.len(), 2);
    assert!(
        cache.get(1, 100).is_some(),
        "最近使用した glyph 1 は残っているはず"
    );
    assert!(
        cache.get(2, 100).is_none(),
        "LRU の glyph 2 は追い出されているはず"
    );
    assert!(cache.get(3, 100).is_some());
}

#[test]
fn size_q6_quantizes_to_1_64px() {
    assert_eq!(size_q6(1.0), 64);
    assert_eq!(size_q6(26.0), 26 * 64);
    assert_eq!(size_q6(0.0), 0);
}

// ---------------------------------------------------------------------
// text.rs (TextEngine) 経由のレイアウト・ラスタライズが、font::rasterize を
// 直接呼んだ場合と完全一致することを確認する自己無矛盾性テスト。
// 実システムフォントが無い環境ではスキップする。
// ---------------------------------------------------------------------

use crate::font::Font;
use crate::text::{TextEngine, TextSegment, TextStyle};

/// `path` のフォント (index 0) を直接読み込んで `font::rasterize` した結果と、
/// `TextEngine::raster_segments` に family_name をそのまま渡して得たグリフの
/// アルファチャンネルが一致するか検証する。両方とも同じ自作ラスタライザを通る
/// はずなので、outline_width=0・color=白不透明であれば完全一致するはず
/// （`rasterize_glyph` は `fill=[255,255,255,255]` のとき alpha = mask とほぼ等しくなる）。
/// フォントが存在しない、あるいはそのグリフが無い場合は None（呼び出し側でスキップ扱い）。
fn assert_engine_matches_direct(path: &str, ch: char) -> Option<()> {
    let p = std::path::Path::new(path);
    if !p.exists() {
        return None;
    }
    let bytes = std::fs::read(p).ok()?;
    let font = Font::from_bytes(bytes, 0).ok()?;
    let gid = font.glyph_id(ch).filter(|&g| g != 0)?;
    let outline = font.outline(gid).ok()?;
    if outline.contours.is_empty() {
        return None;
    }
    let size = 32.0f32;
    let scale = size / font.units_per_em() as f32;
    let direct = rasterize(&outline, scale);
    if direct.width == 0 || direct.height == 0 {
        return None;
    }

    let mut engine = TextEngine::new();
    let style = TextStyle {
        font_family: font.family_name().to_string(),
        font_size: size,
        outline_width: 0,
        color: [255, 255, 255, 255],
        box_w: 300,
        box_h: 300,
        ..Default::default()
    };
    let r = engine.raster_segments(
        &[TextSegment {
            text: ch.to_string(),
            ..Default::default()
        }],
        &style,
    );
    let g = r.glyphs.first()?;
    let alpha: Vec<u8> = g.pixmap.data.as_chunks::<4>().0.iter().map(|px| px[3]).collect();

    assert_eq!(
        g.pixmap.width, direct.width,
        "{path} '{ch}': 幅が一致しません"
    );
    assert_eq!(
        g.pixmap.height, direct.height,
        "{path} '{ch}': 高さが一致しません"
    );
    assert_eq!(alpha, direct.data, "{path} '{ch}': マスクが一致しません");
    Some(())
}

#[test]
fn text_engine_matches_direct_rasterize_msgothic() {
    let mut checked = 0;
    for ch in ['A', 'あ', '永'] {
        match assert_engine_matches_direct(r"C:\Windows\Fonts\msgothic.ttc", ch) {
            Some(()) => checked += 1,
            None => eprintln!(
                "msgothic.ttc '{ch}' はスキップされました（未インストールまたはグリフなし）"
            ),
        }
    }
    if checked == 0 {
        eprintln!("msgothic.ttc が見つからないため、このテストは実質スキップされました");
    }
}

#[test]
fn text_engine_matches_direct_rasterize_yugothic_or_meiryo() {
    let mut checked = 0;
    for path in [
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
    ] {
        for ch in ['A', 'あ'] {
            match assert_engine_matches_direct(path, ch) {
                Some(()) => checked += 1,
                None => eprintln!("{path} '{ch}' はスキップされました"),
            }
        }
    }
    if checked == 0 {
        eprintln!(
            "YuGothM.ttc / meiryo.ttc が見つからないため、このテストは実質スキップされました"
        );
    }
}

#[test]
fn text_engine_matches_direct_rasterize_arial() {
    if assert_engine_matches_direct(r"C:\Windows\Fonts\arial.ttf", 'A').is_none() {
        eprintln!("arial.ttf が見つからないためスキップされました");
    }
}
