//! loca / glyf テーブル: グリフ ID からアウトライン（線分 + 2次ベジェの閉路群）を取り出す。
//!
//! 単純グリフは「オンカーブ点 / オフカーブ（制御）点」の列で表現される。
//! 連続する 2 個のオフカーブ点の間には、暗黙のオンカーブ点（両者の中点）が
//! 存在するものとして補間する（TrueType の仕様どおり）。
//! 複合グリフはコンポーネント（子グリフ参照 + アフィン変換）の列として
//! 表現され、ネストしうる（深さ制限つきで再帰的に解決する）。

use std::borrow::Cow;

use anyhow::{ensure, Result};

use super::reader::Reader;
use super::GlyfStorage;

/// フォント単位座標系の点。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// 輪郭を構成するセグメント。始点は「直前のセグメントの終点」と暗黙に共有する
/// （＝各セグメントの最初の要素は前のセグメントの最後の要素と一致する)。
#[derive(Clone, Copy, Debug)]
pub enum Segment {
    /// p0 -> p1 の直線
    Line(Point, Point),
    /// p0 -> ctrl -> p1 の 2 次ベジェ曲線
    Quad(Point, Point, Point),
}

/// 閉路 1 本（塗りつぶしの境界の 1 ループ）。
#[derive(Clone, Debug, Default)]
pub struct Contour {
    pub segments: Vec<Segment>,
}

/// グリフ 1 個分のアウトライン（複数の閉路からなる）。
#[derive(Clone, Debug, Default)]
pub struct Outline {
    pub contours: Vec<Contour>,
}

const MAX_COMPOSITE_DEPTH: u32 = 8;

/// 1 グリフのアウトライン取得で展開してよいグリフ（複合の成分を含む）の総数。深さ制限だけだと
/// 各段で成分を増やすだけで 16^8 回の再帰ができてしまう。実在のフォントの複合グリフは
/// 成分が数個〜数十個。
const MAX_COMPOSITE_COMPONENTS: u32 = 256;

// simple glyph flags
const ON_CURVE_POINT: u8 = 0x01;
const X_SHORT_VECTOR: u8 = 0x02;
const Y_SHORT_VECTOR: u8 = 0x04;
const REPEAT_FLAG: u8 = 0x08;
const X_IS_SAME_OR_POSITIVE: u8 = 0x10;
const Y_IS_SAME_OR_POSITIVE: u8 = 0x20;

// composite glyph flags
const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const ARGS_ARE_XY_VALUES: u16 = 0x0002;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;

/// loca テーブルを解決する。要素数は `numGlyphs + 1`、値は glyf テーブル先頭からの
/// バイトオフセット（short format は 2 倍済みの実オフセットに正規化してある）。
pub(crate) fn parse_loca(data: &[u8], num_glyphs: u16, long_format: bool) -> Result<Vec<u32>> {
    let n = (num_glyphs as usize)
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("numGlyphs が大きすぎます"))?;
    let mut r = Reader::new(data);
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        if long_format {
            out.push(r.u32()?);
        } else {
            out.push((r.u16()? as u32) * 2);
        }
    }
    Ok(out)
}

/// 指定グリフのアウトラインを取り出す。
pub(crate) fn outline_for_glyph(
    glyf: &GlyfStorage,
    loca: &[u32],
    glyph_id: u16,
    num_glyphs: u16,
) -> Result<Outline> {
    let mut budget = MAX_COMPOSITE_COMPONENTS;
    outline_recursive(glyf, loca, glyph_id, num_glyphs, 0, &mut budget)
}

fn outline_recursive(
    glyf: &GlyfStorage,
    loca: &[u32],
    glyph_id: u16,
    num_glyphs: u16,
    depth: u32,
    budget: &mut u32,
) -> Result<Outline> {
    ensure!(
        depth <= MAX_COMPOSITE_DEPTH,
        "複合グリフのネストが深すぎます（循環参照の疑い）"
    );
    ensure!(
        *budget > 0,
        "複合グリフの成分数が多すぎます（上限 {MAX_COMPOSITE_COMPONENTS}）"
    );
    *budget -= 1;
    ensure!(
        (glyph_id as usize) < num_glyphs as usize,
        "グリフ ID {glyph_id} が範囲外です (numGlyphs={num_glyphs})"
    );
    let gi = glyph_id as usize;
    ensure!(gi + 1 < loca.len(), "loca テーブルが不正です");
    let start = loca[gi];
    let end = loca[gi + 1];
    if end <= start {
        // 空のグリフ（スペースなど）。輪郭なしで正常。
        return Ok(Outline::default());
    }
    let g: Cow<'_, [u8]> = glyf.read_range(start, end - start)?;
    let mut r = Reader::new(&g);
    let number_of_contours = r.i16()?;
    let _x_min = r.i16()?;
    let _y_min = r.i16()?;
    let _x_max = r.i16()?;
    let _y_max = r.i16()?;

    if number_of_contours >= 0 {
        parse_simple_glyph(&mut r, number_of_contours as usize)
    } else {
        parse_composite_glyph(&mut r, glyf, loca, num_glyphs, depth, budget)
    }
}

fn parse_simple_glyph(r: &mut Reader, num_contours: usize) -> Result<Outline> {
    let mut end_pts = Vec::with_capacity(num_contours);
    for _ in 0..num_contours {
        end_pts.push(r.u16()?);
    }
    let num_points = match end_pts.last() {
        Some(&last) => last as usize + 1,
        None => 0,
    };

    let instruction_len = r.u16()? as usize;
    r.skip(instruction_len)?;

    // flags は RLE 圧縮されている（REPEAT_FLAG + リピート回数）。
    let mut flags = Vec::with_capacity(num_points);
    while flags.len() < num_points {
        let f = r.u8()?;
        flags.push(f);
        if f & REPEAT_FLAG != 0 {
            let repeat = r.u8()?;
            for _ in 0..repeat {
                if flags.len() >= num_points {
                    break;
                }
                flags.push(f);
            }
        }
    }
    ensure!(
        flags.len() == num_points,
        "flags 展開後の点数が endPtsOfContours と一致しません"
    );

    let mut xs = Vec::with_capacity(num_points);
    let mut x: i32 = 0;
    for &f in &flags {
        if f & X_SHORT_VECTOR != 0 {
            let dx = r.u8()? as i32;
            x += if f & X_IS_SAME_OR_POSITIVE != 0 {
                dx
            } else {
                -dx
            };
        } else if f & X_IS_SAME_OR_POSITIVE == 0 {
            x += r.i16()? as i32;
        }
        // 両方の条件に当てはまらない場合は差分 0（前の点と同じ x）
        xs.push(x);
    }

    let mut ys = Vec::with_capacity(num_points);
    let mut y: i32 = 0;
    for &f in &flags {
        if f & Y_SHORT_VECTOR != 0 {
            let dy = r.u8()? as i32;
            y += if f & Y_IS_SAME_OR_POSITIVE != 0 {
                dy
            } else {
                -dy
            };
        } else if f & Y_IS_SAME_OR_POSITIVE == 0 {
            y += r.i16()? as i32;
        }
        ys.push(y);
    }

    let mut contours = Vec::with_capacity(num_contours);
    let mut start = 0usize;
    for &end in &end_pts {
        let end = end as usize;
        ensure!(
            end < num_points && end >= start,
            "endPtsOfContours が不正です"
        );
        let pts: Vec<(f32, f32, bool)> = (start..=end)
            .map(|i| (xs[i] as f32, ys[i] as f32, flags[i] & ON_CURVE_POINT != 0))
            .collect();
        contours.push(build_contour(&pts));
        start = end + 1;
    }
    Ok(Outline { contours })
}

/// オンカーブ/オフカーブ点の列を線分 + 2 次ベジェのセグメント列に正規化する。
/// 開始点は必ずオンカーブになるよう回転させる（全点オフカーブの場合は
/// 最初と最後の点の中点を合成の開始点として補う）。
fn build_contour(points: &[(f32, f32, bool)]) -> Contour {
    let n = points.len();
    if n < 2 {
        return Contour::default();
    }

    let mut rotated: Vec<(f32, f32, bool)> = Vec::with_capacity(n + 1);
    if let Some(start_idx) = points.iter().position(|p| p.2) {
        rotated.extend_from_slice(&points[start_idx..]);
        rotated.extend_from_slice(&points[..start_idx]);
    } else {
        // 全点オフカーブ: 最初と最後の中点を開始点として合成する。
        let mx = (points[0].0 + points[n - 1].0) / 2.0;
        let my = (points[0].1 + points[n - 1].1) / 2.0;
        rotated.push((mx, my, true));
        rotated.extend_from_slice(points);
    }

    let m = rotated.len();
    let to_point = |p: (f32, f32, bool)| Point { x: p.0, y: p.1 };
    let mut segments = Vec::new();
    let mut prev_on = to_point(rotated[0]);
    let mut pending_off: Option<Point> = None;

    // rotated[0] (オンカーブ) まで戻ってきて閉じるため 1..=m を回す。
    for idx in 1..=m {
        let p = if idx < m { rotated[idx] } else { rotated[0] };
        if p.2 {
            let pt = to_point(p);
            match pending_off.take() {
                Some(ctrl) => segments.push(Segment::Quad(prev_on, ctrl, pt)),
                None => segments.push(Segment::Line(prev_on, pt)),
            }
            prev_on = pt;
        } else {
            let off = to_point(p);
            match pending_off.take() {
                Some(prev_ctrl) => {
                    // 連続するオフカーブ点間の暗黙のオンカーブ中点
                    let mid = Point {
                        x: (prev_ctrl.x + off.x) / 2.0,
                        y: (prev_ctrl.y + off.y) / 2.0,
                    };
                    segments.push(Segment::Quad(prev_on, prev_ctrl, mid));
                    prev_on = mid;
                    pending_off = Some(off);
                }
                None => pending_off = Some(off),
            }
        }
    }

    Contour { segments }
}

fn parse_composite_glyph(
    r: &mut Reader,
    glyf: &GlyfStorage,
    loca: &[u32],
    num_glyphs: u16,
    depth: u32,
    budget: &mut u32,
) -> Result<Outline> {
    let mut contours = Vec::new();
    loop {
        let flags = r.u16()?;
        let glyph_index = r.u16()?;

        let (dx, dy) = if flags & ARG_1_AND_2_ARE_WORDS != 0 {
            let a = r.i16()? as f32;
            let b = r.i16()? as f32;
            if flags & ARGS_ARE_XY_VALUES != 0 {
                (a, b)
            } else {
                (0.0, 0.0)
            }
        } else {
            let a = r.i8()? as f32;
            let b = r.i8()? as f32;
            if flags & ARGS_ARE_XY_VALUES != 0 {
                (a, b)
            } else {
                (0.0, 0.0)
            }
        };
        // ARGS_ARE_XY_VALUES が立っていない場合はポイントマッチング指定（未対応）。
        // オフセット 0,0 として扱い、グリフ自体の解決は継続する。

        let (a, b, c, d) = if flags & WE_HAVE_A_SCALE != 0 {
            let s = r.f2dot14()?;
            (s, 0.0, 0.0, s)
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            let sx = r.f2dot14()?;
            let sy = r.f2dot14()?;
            (sx, 0.0, 0.0, sy)
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            let a = r.f2dot14()?;
            let b = r.f2dot14()?;
            let c = r.f2dot14()?;
            let d = r.f2dot14()?;
            (a, b, c, d)
        } else {
            (1.0, 0.0, 0.0, 1.0)
        };

        let child = outline_recursive(glyf, loca, glyph_index, num_glyphs, depth + 1, budget)?;
        for contour in child.contours {
            let segments = contour
                .segments
                .into_iter()
                .map(|seg| transform_segment(seg, a, b, c, d, dx, dy))
                .collect();
            contours.push(Contour { segments });
        }

        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }
    Ok(Outline { contours })
}

/// OpenType 複合グリフの変換規則: x' = a*x + c*y + dx, y' = b*x + d*y + dy
fn transform_point(p: Point, a: f32, b: f32, c: f32, d: f32, dx: f32, dy: f32) -> Point {
    Point {
        x: a * p.x + c * p.y + dx,
        y: b * p.x + d * p.y + dy,
    }
}

fn transform_segment(seg: Segment, a: f32, b: f32, c: f32, d: f32, dx: f32, dy: f32) -> Segment {
    match seg {
        Segment::Line(p0, p1) => Segment::Line(
            transform_point(p0, a, b, c, d, dx, dy),
            transform_point(p1, a, b, c, d, dx, dy),
        ),
        Segment::Quad(p0, ctrl, p1) => Segment::Quad(
            transform_point(p0, a, b, c, d, dx, dy),
            transform_point(ctrl, a, b, c, d, dx, dy),
            transform_point(p1, a, b, c, d, dx, dy),
        ),
    }
}
