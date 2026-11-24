//! グリフラスタライザ (F2): [`super::Outline`]（フォント単位座標の閉路群）から
//! アンチエイリアス済み 8bit カバレッジマスク [`GlyphMask`] を生成する。
//! ヒンティングは行わず、スケーリング＋スキャンライン AA のみで塗る。
//!
//! ## 採用したカバレッジ計算方式
//! Y 方向は固定 16 分割のサブスキャンライン ([`SUBSAMPLES_Y`]) で走査し、各サブ
//! スキャンラインでは全エッジとの交点を求めて **nonzero winding rule**
//! （上向き +1 / 下向き -1、合計が 0 でなければ内側）でカバー区間 (spans) を求め、
//! X 方向はその区間の端に位置するピクセルへ厳密な面積（被覆率）を積算するハイブリッ
//! ド方式を採る。ピクセルごとの最終カバレッジは 16 個のサブスキャンライン寄与
//! （各 1/16 重み）の総和。
//!
//! 検討した代替案:
//! - **完全な解析的手法**（signed-area accumulation。font-rs や FreeType の
//!   smooth rasterizer が使う方式）はピクセル単位・トラペゾイド単位で厳密面積を
//!   計算でき最速だが、頂点がちょうどピクセル境界に乗る場合や水平エッジ、走査開始/
//!   終了行の端数処理などエッジケースの実装が複雑で、自作パーサ由来の座標
//!   （ヒンティングなし・任意精度）に対する検証コストが高いと判断し、今回は採らな
//!   かった。
//! - **単純な NxN 均等スーパーサンプリング**（例: 4x4 の in/out 判定を平均）は実装
//!   が単純だが、X 方向も離散化されるため小さいグリフで量子化ノイズ（縦エッジの
//!   ガタつき）が出やすく、同等の品質を得るには N を大きくする必要がありコストが
//!   増える。
//!
//! 採用方式は X 方向を解析的に扱うことで横方向のガタつきを排除しつつ、Y 方向だけ
//! サブサンプリングすることで実装をシンプルに保てる。16 分割は 26px 前後の日本語
//! グリフの目視品質（開発時点では本モジュールの cosmic-text クロスチェックテストで
//! IoU >= 0.85 を確認していた。F3 で cosmic-text を削除した後は `tests.rs` の
//! `text_engine_matches_direct_rasterize_*` が同じ自作パイプライン内での
//! 経路間一致を検証している）を満たすのに十分な精度がありながら、エッジ数十〜
//! 数百程度のグリフでは実行コストも無視できる小ささ（1 グリフあたりマイクロ秒オーダー）。
//!
//! ## ヒンティングについて
//! ヒンティング（グリッドフィッティング、TrueType `fpgm`/`prep`/`glyf` 命令列の
//! 実行）は実装しない。理由:
//! - Noiria の UI テキストは任意サイズ・アンチエイリアス表示が前提で、埋め込み
//!   ビットマップに頼る設計ではない（krkrZ 系ノベルエンジンの慣習と同じ）。
//! - ヒンティングは低解像度ディスプレイ向けの技術で、現代の高DPI環境では効果が
//!   薄く、TrueType 命令インタプリタの自作コストに見合わない。
//! - アンチエイリアスのみでも欧文・和文とも実用的な可読性が得られることは、
//!   ヒンティングなしの AA レンダラである cosmic-text (swash) を本エンジンが
//!   F3 で自作スタックへ置き換えるまで実際に使ってきた実績で確認済み。

use std::collections::HashMap;

use super::{Contour, Outline, Point, Segment};

/// 1 サブピクセル行あたりの垂直方向サブサンプル数。
const SUBSAMPLES_Y: usize = 16;
/// 2 次ベジェの適応分割の最大誤差 (px)。
const FLATTEN_MAX_ERROR: f32 = 0.1;
/// 適応分割の最大再帰深度（異常な制御点でも無限再帰しないための安全弁）。
const FLATTEN_MAX_DEPTH: u32 = 10;
/// マスクの幅・高さの上限 (px)。unitsPerEm が極端に小さい（座標が大きく見える）壊れたフォントで
/// 数万 px 四方のマスクを確保させない。これを超えるグリフは描かない（空マスク）。
const MAX_MASK_DIM: i64 = 4096;

/// ラスタライズ済みグリフのカバレッジマスク。
///
/// `left` / `top` は cosmic-text (swash) の `Placement` と同じ規約:
/// - `left`: ペン位置(x=0)から見た bitmap 左端の x オフセット（負もありうる）。
/// - `top`: ベースライン(y=0)から bitmap 上端までの上方向距離（フォントが
///   ベースラインより上に描かれる通常のグリフでは正）。
///
/// 描画側は `draw_x = pen_x + left`, `draw_y = baseline_y - top` で配置する
/// （`crate::text` の `img.placement` の使い方と同一）。
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphMask {
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    /// 8bit カバレッジ、row-major, width*height バイト（アルファのみ、色は持たない）。
    pub data: Vec<u8>,
}

impl GlyphMask {
    fn empty() -> Self {
        Self {
            width: 0,
            height: 0,
            left: 0,
            top: 0,
            data: Vec::new(),
        }
    }

    /// (x, y) のカバレッジ (0-255)。範囲外は 0。
    pub fn coverage_at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return 0;
        }
        self.data[y as usize * self.width as usize + x as usize]
    }
}

/// フォント単位座標から (行スペース: 原点=左上, y 下向き) のピクセル座標へ変換する。
#[inline]
fn to_row_space(p: Point, scale: f32, left: i32, top: i32) -> (f32, f32) {
    (p.x * scale - left as f32, top as f32 - p.y * scale)
}

/// 半開区間 [y_top, y_bot) を持つ有向エッジ。winding は交差時の寄与 (+1/-1)。
struct Edge {
    x_top: f32,
    y_top: f32,
    x_bot: f32,
    y_bot: f32,
    winding: f32,
}

fn add_edge(edges: &mut Vec<Edge>, p0: (f32, f32), p1: (f32, f32)) {
    // 水平エッジは走査線と交差しないので寄与ゼロ（nonzero rule では無視してよい）。
    if (p0.1 - p1.1).abs() < 1e-6 {
        return;
    }
    if p0.1 < p1.1 {
        edges.push(Edge {
            x_top: p0.0,
            y_top: p0.1,
            x_bot: p1.0,
            y_bot: p1.1,
            winding: 1.0,
        });
    } else {
        edges.push(Edge {
            x_top: p1.0,
            y_top: p1.1,
            x_bot: p0.0,
            y_bot: p0.1,
            winding: -1.0,
        });
    }
}

/// 2 次ベジェを直線群へ適応分割する。曲率（弦からの制御点距離）が
/// `FLATTEN_MAX_ERROR` 以下になるまで再帰的に半分割する。`out` には終点だけを
/// 追加していく（始点は呼び出し側が既に push 済みという前提）。
fn flatten_quad(
    p0: (f32, f32),
    ctrl: (f32, f32),
    p1: (f32, f32),
    out: &mut Vec<(f32, f32)>,
    depth: u32,
) {
    if depth >= FLATTEN_MAX_DEPTH || is_flat(p0, ctrl, p1) {
        out.push(p1);
        return;
    }
    let p01 = midpoint(p0, ctrl);
    let p12 = midpoint(ctrl, p1);
    let p012 = midpoint(p01, p12);
    flatten_quad(p0, p01, p012, out, depth + 1);
    flatten_quad(p012, p12, p1, out, depth + 1);
}

#[inline]
fn midpoint(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}

/// 制御点から弦 (p0-p1) までの距離が `FLATTEN_MAX_ERROR` 以下なら「平坦」とみなす。
fn is_flat(p0: (f32, f32), ctrl: (f32, f32), p1: (f32, f32)) -> bool {
    let dx = p1.0 - p0.0;
    let dy = p1.1 - p0.1;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-12 {
        // 弦がほぼ点: 制御点自体の乖離で判定する。
        let ex = ctrl.0 - p0.0;
        let ey = ctrl.1 - p0.1;
        return (ex * ex + ey * ey) <= FLATTEN_MAX_ERROR * FLATTEN_MAX_ERROR;
    }
    let cross = (ctrl.0 - p0.0) * dy - (ctrl.1 - p0.1) * dx;
    let dist_sq = (cross * cross) / len_sq;
    dist_sq <= FLATTEN_MAX_ERROR * FLATTEN_MAX_ERROR
}

fn contour_points_bbox(outline: &Outline, scale: f32) -> Option<(f32, f32, f32, f32)> {
    let mut xmin = f32::INFINITY;
    let mut xmax = f32::NEG_INFINITY;
    let mut ymin = f32::INFINITY;
    let mut ymax = f32::NEG_INFINITY;
    let mut any = false;
    for c in &outline.contours {
        for seg in &c.segments {
            let pts: [Point; 3] = match *seg {
                Segment::Line(a, b) => [a, b, b],
                Segment::Quad(a, ctrl, b) => [a, ctrl, b],
            };
            for p in pts {
                any = true;
                let x = p.x * scale;
                let y = p.y * scale;
                xmin = xmin.min(x);
                xmax = xmax.max(x);
                ymin = ymin.min(y);
                ymax = ymax.max(y);
            }
        }
    }
    if !any {
        return None;
    }
    Some((xmin, ymin, xmax, ymax))
}

/// 1 つの閉路を行スペースの折れ線 (polyline) へ変換する。
fn contour_to_polyline(
    contour: &Contour,
    scale: f32,
    left: i32,
    top: i32,
    poly: &mut Vec<(f32, f32)>,
) {
    poly.clear();
    for seg in &contour.segments {
        match *seg {
            Segment::Line(p0, p1) => {
                if poly.is_empty() {
                    poly.push(to_row_space(p0, scale, left, top));
                }
                poly.push(to_row_space(p1, scale, left, top));
            }
            Segment::Quad(p0, ctrl, p1) => {
                let rp0 = to_row_space(p0, scale, left, top);
                let rctrl = to_row_space(ctrl, scale, left, top);
                let rp1 = to_row_space(p1, scale, left, top);
                if poly.is_empty() {
                    poly.push(rp0);
                }
                flatten_quad(rp0, rctrl, rp1, poly, 0);
            }
        }
    }
}

/// 区間 [xa, xb) （行スペース、ピクセル単位）の被覆を `weight` 倍して `row` に積算する。
fn accumulate_span(row: &mut [f32], xa: f32, xb: f32, weight: f32) {
    let width = row.len();
    let xa = xa.max(0.0);
    let xb = xb.min(width as f32);
    if xb <= xa {
        return;
    }
    let c0 = xa.floor() as usize;
    let c1 = (xb.ceil() as usize).min(width);
    if c1 <= c0 {
        return;
    }
    if c1 - c0 == 1 {
        row[c0] += (xb - xa) * weight;
        return;
    }
    // 先頭の部分ピクセル
    row[c0] += (1.0 - (xa - c0 as f32)) * weight;
    // 完全に覆われる中間ピクセル
    for v in row.iter_mut().take(c1 - 1).skip(c0 + 1) {
        *v += weight;
    }
    // 末尾の部分ピクセル
    let last = c1 - 1;
    row[last] += (xb - last as f32) * weight;
}

/// アウトラインをラスタライズする。`scale` は `px_per_em / units_per_em`。
///
/// 輪郭が空（スペース等）の場合は 0x0 の [`GlyphMask`] を返す。
pub fn rasterize(outline: &Outline, scale: f32) -> GlyphMask {
    if outline.contours.is_empty() {
        return GlyphMask::empty();
    }
    let Some((xmin, ymin, xmax, ymax)) = contour_points_bbox(outline, scale) else {
        return GlyphMask::empty();
    };

    let left = xmin.floor() as i32;
    let top = ymax.ceil() as i32;
    let right = xmax.ceil() as i32;
    let bottom = ymin.floor() as i32;
    let (width, height) = (right as i64 - left as i64, top as i64 - bottom as i64);
    if width > MAX_MASK_DIM || height > MAX_MASK_DIM {
        return GlyphMask::empty();
    }
    let width = width.max(0) as u32;
    let height = height.max(0) as u32;
    if width == 0 || height == 0 {
        return GlyphMask {
            width: 0,
            height: 0,
            left,
            top,
            data: Vec::new(),
        };
    }

    // 1) 全閉路を行スペースの折れ線へ変換し、有向エッジ列を組み立てる。
    let mut edges: Vec<Edge> = Vec::with_capacity(outline.contours.len() * 16);
    let mut poly: Vec<(f32, f32)> = Vec::with_capacity(64);
    for contour in &outline.contours {
        contour_to_polyline(contour, scale, left, top, &mut poly);
        if poly.len() < 2 {
            continue;
        }
        for w in poly.windows(2) {
            add_edge(&mut edges, w[0], w[1]);
        }
        // 閉路は暗黙にクローズされる規約だが、数値誤差で始終点が僅かにずれる
        // 場合に備えて閉じるエッジを明示的に足しておく（一致していれば水平/縮退
        // エッジとして add_edge 内で無視される）。
        let (Some(&first), Some(&last)) = (poly.first(), poly.last()) else {
            continue;
        };
        if (first.0 - last.0).abs() > 1e-4 || (first.1 - last.1).abs() > 1e-4 {
            add_edge(&mut edges, last, first);
        }
    }

    // 2) y_top 順に並べ、各ピクセル行と重なるエッジだけをアクティブ集合に保って走査する。
    edges.sort_by(|a, b| a.y_top.total_cmp(&b.y_top));
    let w = width as usize;
    let h = height as usize;
    let mut acc = vec![0f32; w * h];
    let weight = 1.0 / SUBSAMPLES_Y as f32;
    let mut active: Vec<&Edge> = Vec::with_capacity(32);
    let mut next_edge = 0usize;
    let mut crossings: Vec<(f32, f32)> = Vec::with_capacity(16);
    for row_i in 0..h {
        let row_y0 = row_i as f32;
        let row_y1 = (row_i + 1) as f32;
        active.retain(|e| e.y_bot > row_y0);
        while next_edge < edges.len() && edges[next_edge].y_top < row_y1 {
            let e = &edges[next_edge];
            if e.y_bot > row_y0 {
                active.push(e);
            }
            next_edge += 1;
        }
        if active.len() < 2 {
            continue;
        }
        let row = &mut acc[row_i * w..(row_i + 1) * w];
        for k in 0..SUBSAMPLES_Y {
            let y = row_y0 + (k as f32 + 0.5) / SUBSAMPLES_Y as f32;
            crossings.clear();
            for e in &active {
                if y >= e.y_top && y < e.y_bot {
                    let t = (y - e.y_top) / (e.y_bot - e.y_top);
                    let x = e.x_top + (e.x_bot - e.x_top) * t;
                    crossings.push((x, e.winding));
                }
            }
            if crossings.len() < 2 {
                continue;
            }
            if crossings.len() <= 16 {
                for i in 1..crossings.len() {
                    let cur = crossings[i];
                    let mut j = i;
                    while j > 0 && crossings[j - 1].0 > cur.0 {
                        crossings[j] = crossings[j - 1];
                        j -= 1;
                    }
                    crossings[j] = cur;
                }
            } else {
                crossings
                    .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            }
            let mut sum = 0f32;
            let mut prev_x = crossings[0].0;
            for &(x, wind) in &crossings {
                if sum != 0.0 {
                    accumulate_span(row, prev_x, x, weight);
                }
                sum += wind;
                prev_x = x;
            }
        }
    }

    let mut data = Vec::with_capacity(w * h);
    for v in acc {
        data.push((v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
    }
    GlyphMask {
        width,
        height,
        left,
        top,
        data,
    }
}

/// px_per_em を 1/64px 単位に量子化したキャッシュキー用の値 (26.6 固定小数と同じ粒度)。
pub fn size_q6(px_per_em: f32) -> u32 {
    (px_per_em.max(0.0) * 64.0).round() as u32
}

/// (glyph_id, size_q6) をキーにした固定容量の LRU キャッシュ。F3 でのグリフ再利用に使う。
/// 1 フォントにつき 1 インスタンスを想定（キーに font_id を含まないため、複数フォント
/// で共有する場合は呼び出し側でフォントごとにインスタンスを分けること）。
pub struct GlyphCache {
    capacity: usize,
    map: HashMap<(u16, u32), (GlyphMask, u64)>,
    tick: u64,
}

impl GlyphCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            map: HashMap::new(),
            tick: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get(&self, glyph: u16, size_q6: u32) -> Option<&GlyphMask> {
        self.map.get(&(glyph, size_q6)).map(|(m, _)| m)
    }

    /// キャッシュ済みならそれを返し、無ければ `make` でラスタライズして格納する。
    pub fn get_or_insert_with(
        &mut self,
        glyph: u16,
        size_q6: u32,
        make: impl FnOnce() -> GlyphMask,
    ) -> &GlyphMask {
        let key = (glyph, size_q6);
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        if let Some((_, t)) = self.map.get_mut(&key) {
            *t = tick;
        } else {
            if self.map.len() >= self.capacity {
                if let Some((&evict, _)) = self.map.iter().min_by_key(|(_, (_, t))| *t) {
                    self.map.remove(&evict);
                }
            }
            self.map.insert(key, (make(), tick));
        }
        &self.map.get(&key).expect("直前に挿入済み").0
    }
}

#[cfg(test)]
mod tests;
