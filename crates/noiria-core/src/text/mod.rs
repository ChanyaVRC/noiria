//! テキストレイアウト＆ラスタライズ (F3)。
//!
//! かつては `cosmic-text` に委譲していたが、`font/` モジュール（F1: TTF/TTC パーサ、
//! F2: スキャンライン AA ラスタライザ）の完成を受け、このモジュールで自作の
//! レイアウトエンジン（1 文字 1 グリフの素朴な折返し + 日本語禁則処理 + フォント
//! フォールバック）へ置き換えた。外部依存は完全に無くなった。
//! 外へは引き続き `RasterText` / `GlyphRaster` という自前の型だけを公開する
//! （`TextEngine::new` / `assert_cjk` / `raster_segments` の意味も維持し、
//! ops 層・compose・TS ランタイムは無変更で動く）。
//!
//! ## シェーピングについて
//! グリフ整形（リガチャ・カーニングペア・複雑スクリプトの合字）は行わない。
//! 1 Unicode スカラ値 = 1 グリフとして、フォントの advance をそのまま並べる
//! 素朴な実装。日本語 UI（等幅に近い全角かな漢字が主体）では視覚的な破綻が
//! ほぼ無く、krkrZ 系ノベルエンジンの標準描画と同等の見た目が得られるため、
//! この単純化は要件を満たすと判断した。
//!
//! ## 禁則処理（日本語）
//! JIS X 4051 の全規則ではなく、実用上重要な最小セットのみ実装する
//! （具体的な文字集合は [`is_line_start_forbidden`] / [`is_line_end_forbidden`] 参照）。
//! - **行頭禁則（ぶら下げ方式）**: 句読点・閉じ括弧・小書き仮名などは行頭に来ては
//!   いけない。折返しが必要になった時点でその文字が行頭禁則文字なら、折り返さず
//!   そのまま前の行の末尾に置く（結果として `box_w` を最大 1 文字分だけ超えることを
//!   許容する）。実装が単純で、address 1 文字だけの超過は表示上ほぼ気にならないため、
//!   「行末几文字を前の行に詰め直す」道連れ方式ではなくこちらを採用した。
//! - **行末禁則（道連れ方式）**: 開き括弧などは行末に来てはいけない。通常の折返しが
//!   発生する直前に、現在行の末尾から連続する行末禁則文字を数え、その分だけ
//!   新しい行へ一緒に送る。
//! - どちらの判定も「現在処理中の文字」と「既に確定済みの直前文字」だけを見る
//!   （先読みしない）ため、テキストを追記しても既存のグリフ位置は変化しない
//!   （`ops_text` の追記キャッシュが前提とする性質、`appending_keeps_prefix_glyph_count`
//!   テストで担保）。
//!
//! 実装の核（禁則の行番号割り当てロジック）は [`assign_lines`] という純粋関数に
//! 切り出してあり、実フォントのメトリクスに依存しないユニットテストで検証できる。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::font::registry::{self, FontKey, FontSource};
use crate::font::{rasterize, size_q6, Font, GlyphCache};
use crate::pixmap::Pixmap;

mod glyph;
mod layout;
#[cfg(test)]
mod tests;

use glyph::rasterize_glyph;
use layout::{alignment_offsets, assign_lines, CharControl, LineChar};

fn default_font_family() -> String {
    "Yu Gothic UI".to_string()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// `TextStyle::outline_width` の上限（px）。異常に大きな値によるグリフ単位の過大確保を防ぐ。
pub const MAX_OUTLINE_WIDTH: u32 = 64;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TextStyle {
    pub font_family: String,
    pub font_size: f32,
    /// 行の高さ（font_size に対する倍率）
    pub line_height: f32,
    /// straight RGBA
    pub color: [u8; 4],
    pub outline_color: [u8; 4],
    pub outline_width: u32,
    /// テキストボックスの幅・高さ（折返し判定に使用）
    pub box_w: u32,
    pub box_h: u32,
    /// レイヤ原点からテキスト原点へのオフセット
    pub margin_x: i32,
    pub margin_y: i32,
    pub align: TextAlign,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font_family: default_font_family(),
            font_size: 28.0,
            line_height: 1.6,
            color: [255, 255, 255, 255],
            outline_color: [0, 0, 0, 200],
            outline_width: 2,
            box_w: 1120,
            box_h: 200,
            margin_x: 0,
            margin_y: 0,
            align: TextAlign::Left,
        }
    }
}

/// [`TextSegment::locate`] の引数。JS 側は `{x, y}` object で送るため、
/// タプルではなく専用 struct にして serde の JSON 表現を object 形に揃える。
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocateArgs {
    pub x: f32,
    /// 省略時（KAG の `[locate x=..]` のみ指定）は現在の行のまま x だけ動かす。
    pub y: Option<f32>,
}

/// 色オーバーライド付きのテキスト断片（リンク表示・文字装飾タグ等に使用）。
/// 新フィールドは全て `None` 既定（`#[serde(default)]`）なので、既存の呼び出し側
/// （V8 は serde 経由で自動吸収、tsint は `segments_from_value` 側で対応）は無変更で動く。
///
/// `shadow`（KAG の font 属性）は対象外（`docs/kag-tag-compat.md` 参照。既存の縁取り合成は
/// 「同一マスクを膨張させて描く」実装で、オフセット位置への二重ブリットが要る影とは
/// 根本的に別パスが要るため今回のスコープ外とした）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TextSegment {
    pub text: String,
    /// None なら style.color
    pub color: Option<[u8; 4]>,
    /// None ならボックス基準の style.font_size（行送りには影響しない。設計判断: 行送りは
    /// 常にボックス基準 font_size で固定し、セグメントごとの size はグリフ拡大にのみ影響する）。
    pub size: Option<f32>,
    /// 疑似太字。左隣ピクセルのカバレッジを拾って合成するだけの簡易実装（advance は不変）。
    pub bold: Option<bool>,
    /// None なら style.outline_color（縁取りの太さ自体は style.outline_width のまま据え置き）。
    pub edge_color: Option<[u8; 4]>,
    /// ルビ文字列。本文セグメント直後にグリフを追加する（reveal の Vec 順序前提を満たすため）。
    /// セグメント途中で折り返される場合は先頭文字位置基準の簡易対応（泣き別れ非対応）。
    pub ruby: Option<String>,
    /// `[locate]` 制御。`text: ""` の制御専用セグメントとして送る想定。
    pub locate: Option<LocateArgs>,
    /// `[indent]`（Some(true)）/ `[endindent]`（Some(false)）制御。
    /// `text: ""` の制御専用セグメントとして送る想定。
    pub indent: Option<bool>,
    /// `[graph storage=...]` 用のインライン画像（W3）。`text: ""` の制御専用セグメントと
    /// して送る想定。値は解決済みパス（呼び出し側が `TextEngine::cache_image` で事前に
    /// ロード・キャッシュ済みであること。未キャッシュのパスは幅 0 として無視される）。
    pub inline_image: Option<String>,
    /// `[font face=]`: このセグメントだけ基準フォントを差し替える family 名。None なら
    /// `style.font_family`。行メトリクス（ascent・行送り）は常にボックス基準フォントのまま
    /// （size と同じ設計: 追記キャッシュが前提とする既存グリフ位置の不変性を保つため）。
    pub font_family: Option<String>,
}

/// ラスタライズ済みグリフ 1 個。
#[derive(Clone, Debug)]
pub struct GlyphRaster {
    /// テキスト原点からの描画位置
    pub x: i32,
    pub y: i32,
    /// 縁取り込みでプリマルチプライ済みの小さな絵
    pub pixmap: Arc<Pixmap>,
    /// 連結テキスト全体におけるバイト開始位置（リンクのヒットテスト用）
    pub byte_start: usize,
    pub byte_end: usize,
    /// 論理行番号
    pub line_i: usize,
}

/// レイアウト＆ラスタライズ結果。Scene に Arc で保持される。
#[derive(Clone, Debug)]
pub struct RasterText {
    pub glyphs: Vec<GlyphRaster>,
    pub margin_x: i32,
    pub margin_y: i32,
}

/// グリフのヒット矩形（テキスト原点基準）。リンク選択肢・メニュー行判定用。
#[derive(Clone, Debug, Serialize)]
pub struct GlyphRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub byte_start: usize,
    pub byte_end: usize,
    pub line_i: usize,
}

impl RasterText {
    pub fn rects(&self) -> Vec<GlyphRect> {
        self.glyphs
            .iter()
            .map(|g| GlyphRect {
                x: g.x,
                y: g.y,
                w: g.pixmap.width,
                h: g.pixmap.height,
                byte_start: g.byte_start,
                byte_end: g.byte_end,
                line_i: g.line_i,
            })
            .collect()
    }
}

/// 開いたフォント 1 個 + そのグリフラスタキャッシュ。
/// `GlyphCache` は 1 フォント 1 インスタンス想定なのでここで対にする。
struct LoadedFont {
    font: Font,
    cache: GlyphCache,
}

/// family 名で見つからない場合に順に試す固定フォールバックチェーン（per-OS、W1）。
/// ゲームローカルフォント（`fonts/*.ttf|ttc`、`TextEngine::load_game_fonts`）はこのチェーン
/// よりさらに前に試される（`resolve_base`/`glyph_for_char` 参照）。
#[cfg(target_os = "windows")]
const FALLBACK_CHAIN: [&str; 4] = ["Yu Gothic UI", "Yu Gothic", "Meiryo", "MS Gothic"];
#[cfg(not(target_os = "windows"))]
const FALLBACK_CHAIN: [&str; 5] = [
    "Noto Sans CJK JP",
    "Noto Sans JP",
    "TakaoGothic",
    "IPAGothic",
    "VL Gothic",
];

/// キャッシュ容量（フォントあたりのグリフ数）。日本語メッセージ 1 画面あたり
/// せいぜい数十〜百数十グリフなので余裕を持たせる。
const GLYPH_CACHE_CAPACITY: usize = 1024;

/// インライン画像キャッシュの総バイト数上限（16 MiB）。
const IMAGE_CACHE_MAX_BYTES: usize = 16 << 20;

/// 装飾（色・縁取り・太字）適用済みグリフキャッシュの上限。
const STYLED_CACHE_MAX_BYTES: usize = 16 << 20;
const STYLED_CACHE_MAX_ENTRIES: usize = 2048;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct StyledGlyphKey {
    font_id: u32,
    size_q6: u32,
    fill: [u8; 4],
    outline_color: [u8; 4],
    outline_width: u32,
    gid: u16,
    bold: bool,
}

struct StyledGlyphCache {
    map: HashMap<StyledGlyphKey, (Arc<Pixmap>, u64)>,
    bytes: usize,
    tick: u64,
}

impl StyledGlyphCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            bytes: 0,
            tick: 0,
        }
    }

    fn get(&mut self, key: &StyledGlyphKey) -> Option<Arc<Pixmap>> {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        self.map.get_mut(key).map(|(p, t)| {
            *t = tick;
            Arc::clone(p)
        })
    }

    fn insert(&mut self, key: StyledGlyphKey, pixmap: Arc<Pixmap>) {
        let sz = pixmap.data.len();
        if sz > STYLED_CACHE_MAX_BYTES {
            return;
        }
        if let Some((old, _)) = self.map.remove(&key) {
            self.bytes -= old.data.len();
        }
        self.tick = self.tick.wrapping_add(1);
        self.map.insert(key, (pixmap, self.tick));
        self.bytes += sz;
        while self.bytes > STYLED_CACHE_MAX_BYTES || self.map.len() > STYLED_CACHE_MAX_ENTRIES {
            let Some(&oldest) = self.map.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| k) else {
                break;
            };
            if let Some((p, _)) = self.map.remove(&oldest) {
                self.bytes -= p.data.len();
            }
        }
    }

    fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }
}

/// 総バイト数で上限を持つインライン画像 LRU。`has_image(&self)` の公開 API を維持しつつ
/// アクセス時刻を更新するため、時刻だけ `AtomicU64` にしている（`TextEngine` の `Sync` も保つ）。
struct ImageCache {
    map: HashMap<String, (Arc<Pixmap>, AtomicU64)>,
    bytes: usize,
    cap_bytes: usize,
    tick: AtomicU64,
}

impl ImageCache {
    fn new(cap_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            bytes: 0,
            cap_bytes,
            tick: AtomicU64::new(0),
        }
    }

    fn next_tick(&self) -> u64 {
        self.tick.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn contains_key(&self, path: &str) -> bool {
        if let Some((_, t)) = self.map.get(path) {
            t.store(self.next_tick(), Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    fn get(&self, path: &str) -> Option<Arc<Pixmap>> {
        self.map.get(path).map(|(p, t)| {
            t.store(self.next_tick(), Ordering::Relaxed);
            Arc::clone(p)
        })
    }

    fn insert(&mut self, path: String, pixmap: Pixmap) {
        let sz = pixmap.data.len();
        if let Some((old, _)) = self.map.remove(&path) {
            self.bytes -= old.data.len();
        }
        if sz > self.cap_bytes {
            return;
        }
        let tick = self.next_tick();
        self.map
            .insert(path, (Arc::new(pixmap), AtomicU64::new(tick)));
        self.bytes += sz;
        while self.bytes > self.cap_bytes {
            let Some(oldest) = self
                .map
                .iter()
                .min_by_key(|(_, (_, t))| t.load(Ordering::Relaxed))
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((p, _)) = self.map.remove(&oldest) {
                self.bytes -= p.data.len();
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    fn bytes(&self) -> usize {
        self.bytes
    }
}

pub struct TextEngine {
    registry: &'static registry::FontRegistry,
    game_registry: registry::FontRegistry,
    font_ids: HashMap<FontKey, u32>,
    fonts: Vec<LoadedFont>,
    glyph_fallback_cache: HashMap<char, u32>,
    glyph_missing: HashSet<char>,
    styled_cache: StyledGlyphCache,
    image_cache: ImageCache,
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEngine {
    /// システムフォントレジストリ（プロセス内で 1 回だけスキャン）を使って初期化する。
    pub fn new() -> Self {
        Self {
            registry: registry::global(),
            game_registry: registry::FontRegistry::empty(),
            font_ids: HashMap::new(),
            fonts: Vec::new(),
            glyph_fallback_cache: HashMap::new(),
            glyph_missing: HashSet::new(),
            styled_cache: StyledGlyphCache::new(),
            image_cache: ImageCache::new(IMAGE_CACHE_MAX_BYTES),
        }
    }

    pub fn load_game_fonts(&mut self, files: Vec<Vec<u8>>) {
        self.game_registry = registry::FontRegistry::from_game_font_bytes(&files);
        self.font_ids.clear();
        self.fonts.clear();
        self.glyph_fallback_cache.clear();
        self.glyph_missing.clear();
        self.styled_cache.clear();
    }

    /// `path` が既に画像キャッシュへロード済みか。
    pub fn has_image(&self, path: &str) -> bool {
        self.image_cache.contains_key(path)
    }

    /// `[graph]` 用にロード済み画像を登録する（呼び出し側が I/O を行う。上記コメント参照）。
    pub fn cache_image(&mut self, path: String, pixmap: Pixmap) {
        self.image_cache.insert(path, pixmap);
    }

    /// CJK グリフが実際にラスタライズできるか確認する。起動時チェック用。
    pub fn assert_cjk(&mut self, style: &TextStyle) -> anyhow::Result<()> {
        let raster = self.raster_segments(
            &[TextSegment {
                text: "あ".into(),
                ..Default::default()
            }],
            style,
        );
        let ok = raster
            .glyphs
            .iter()
            .any(|g| g.pixmap.data.iter().any(|&b| b != 0));
        anyhow::ensure!(
            ok,
            "CJK フォント '{}' のグリフをラスタライズできませんでした。フォント設定を確認してください。",
            style.font_family
        );
        Ok(())
    }

    fn font_id(&mut self, key: &FontKey) -> Option<u32> {
        if let Some(&id) = self.font_ids.get(key) {
            return Some(id);
        }
        let font = Self::load_font(key)?;
        Some(self.insert_font(key, font))
    }

    fn font_mut(&mut self, key: &FontKey) -> Option<&mut LoadedFont> {
        let id = self.font_id(key)?;
        self.fonts.get_mut(id as usize)
    }

    fn load_font(key: &FontKey) -> Option<Font> {
        match &key.source {
            FontSource::Path(path) => Font::from_file(path, key.index).ok(),
            FontSource::Bytes(data) => Font::from_shared_bytes(Arc::clone(&data.0), key.index).ok(),
        }
    }

    fn insert_font(&mut self, key: &FontKey, font: Font) -> u32 {
        let id = self.fonts.len() as u32;
        self.fonts.push(LoadedFont {
            font,
            cache: GlyphCache::new(GLYPH_CACHE_CAPACITY),
        });
        self.font_ids.insert(key.clone(), id);
        id
    }

    /// `key` のフォントが `c` のグリフを持つか調べ、持つならそのフォントを保持して (font_id, glyph_id) を返す。
    /// 持たないフォントは（未ロードなら）保持しない。
    fn probe_glyph(&mut self, key: &FontKey, c: char) -> Option<(u32, u16)> {
        if let Some(&id) = self.font_ids.get(key) {
            let gid = self.fonts[id as usize]
                .font
                .glyph_id(c)
                .filter(|&g| g != 0)?;
            return Some((id, gid));
        }
        let font = Self::load_font(key)?;
        let gid = font.glyph_id(c).filter(|&g| g != 0)?;
        let id = self.insert_font(key, font);
        Some((id, gid))
    }

    fn resolve_base(&self, requested: &str) -> Option<FontKey> {
        if let Some(k) = self.game_registry.resolve(requested) {
            return Some(k);
        }
        if let Some(k) = self.game_registry.any() {
            return Some(k);
        }
        if let Some(k) = self.registry.resolve(requested) {
            return Some(k);
        }
        for fam in FALLBACK_CHAIN {
            if let Some(k) = self.registry.resolve(fam) {
                return Some(k);
            }
        }
        self.registry.any()
    }

    fn glyph_for_char(&mut self, base: &FontKey, c: char) -> Option<(u32, u16)> {
        if let Some(base_id) = self.font_id(base) {
            if let Some(gid) = self.fonts[base_id as usize]
                .font
                .glyph_id(c)
                .filter(|&id| id != 0)
            {
                return Some((base_id, gid));
            }
        }
        if self.glyph_missing.contains(&c) {
            return None;
        }
        if let Some(&cached_id) = self.glyph_fallback_cache.get(&c) {
            if let Some(gid) = self.fonts[cached_id as usize]
                .font
                .glyph_id(c)
                .filter(|&id| id != 0)
            {
                return Some((cached_id, gid));
            }
        }
        let game_candidates: Vec<FontKey> = self.game_registry.iter_all().cloned().collect();
        for key in game_candidates {
            if key == *base {
                continue;
            }
            if let Some((font_id, gid)) = self.probe_glyph(&key, c) {
                self.glyph_fallback_cache.insert(c, font_id);
                return Some((font_id, gid));
            }
        }
        for fam in FALLBACK_CHAIN {
            let Some(key) = self.registry.resolve(fam) else {
                continue;
            };
            if key == *base {
                continue;
            }
            if let Some((font_id, gid)) = self.probe_glyph(&key, c) {
                self.glyph_fallback_cache.insert(c, font_id);
                return Some((font_id, gid));
            }
        }
        let candidates: Vec<FontKey> = self
            .game_registry
            .iter_all()
            .chain(self.registry.iter_all())
            .cloned()
            .collect();
        for key in candidates {
            if key == *base {
                continue;
            }
            if let Some((font_id, gid)) = self.probe_glyph(&key, c) {
                self.glyph_fallback_cache.insert(c, font_id);
                return Some((font_id, gid));
            }
        }
        self.glyph_missing.insert(c);
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn render_styled_glyph(
        &mut self,
        font_id: u32,
        gid: u16,
        font_size: f32,
        fill: [u8; 4],
        outline_color: [u8; 4],
        outline_width: u32,
        bold: bool,
    ) -> (f32, i32, i32, Option<Arc<Pixmap>>) {
        let size_key = size_q6(font_size);
        let styled_key = StyledGlyphKey {
            font_id,
            gid,
            size_q6: size_key,
            fill,
            outline_color,
            outline_width,
            bold,
        };
        let cached_styled = self.styled_cache.get(&styled_key);
        let loaded = &mut self.fonts[font_id as usize];
        let scale = font_size / loaded.font.units_per_em() as f32;
        let advance_px = loaded.font.advance(gid) as f32 * scale;
        let mask = loaded.cache.get_or_insert_with(gid, size_key, || {
            let outline = loaded.font.outline(gid).unwrap_or_default();
            rasterize(&outline, scale)
        });
        let (mask_left, mask_top) = (mask.left, mask.top);
        let pixmap = if mask.width > 0 && mask.height > 0 {
            if let Some(p) = cached_styled {
                Some(p)
            } else {
                let p = Arc::new(rasterize_glyph(
                    mask.width,
                    mask.height,
                    &mask.data,
                    fill,
                    outline_color,
                    outline_width,
                    bold,
                ));
                self.styled_cache.insert(styled_key, Arc::clone(&p));
                Some(p)
            }
        } else {
            None
        };
        (advance_px, mask_left, mask_top, pixmap)
    }

    /// 断片列をレイアウトしてグリフごとのラスタ画像列を返す。
    pub fn raster_segments(&mut self, segments: &[TextSegment], style: &TextStyle) -> RasterText {
        let empty = || RasterText {
            glyphs: Vec::new(),
            margin_x: style.margin_x,
            margin_y: style.margin_y,
        };

        let Some(base_key) = self.resolve_base(&style.font_family) else {
            // インストール済みフォントが 1 つも見つからない（通常あり得ない）。
            return empty();
        };
        let base_ascent_px = {
            let Some(loaded) = self.font_mut(&base_key) else {
                return empty();
            };
            let scale = style.font_size / loaded.font.units_per_em() as f32;
            loaded.font.ascent() as f32 * scale
        };
        let line_advance = style.font_size * style.line_height;

        // ルビ用の縦方向の予約帯。1 文字でもルビ付きセグメントがあれば、行送り自体は
        // 変えずに（純粋関数 assign_lines のロジックには触れない）本文の baseline_y だけ
        // 一律 ruby_extra ぶん下にシフトし、その上の帯にルビを描く（設計判断 1 参照）。
        let any_ruby = segments.iter().any(|s| s.ruby.is_some());
        let ruby_extra = if any_ruby {
            style.font_size * RUBY_SIZE_RATIO * RUBY_LINE_HEIGHT
        } else {
            0.0
        };

        // --- パス 1: 文字ごとにフォント/グリフを解決し、advance とラスタ済み
        //     ピクマップを求める（この時点ではまだ行番号・ペン位置は未確定）。
        struct Resolved {
            ch: char,
            is_break: bool,
            advance_px: f32,
            left: i32,
            top: i32,
            pixmap: Option<Arc<Pixmap>>,
            byte_start: usize,
            byte_end: usize,
            control: Option<CharControl>,
        }

        let mut resolved: Vec<Resolved> = Vec::new();
        // (byte_start, byte_end, ruby_text): セグメント境界。Pass3 完了後のルビ描画に使う。
        let mut ruby_meta: Vec<(usize, usize, String, FontKey)> = Vec::new();
        let mut byte_pos = 0usize;
        for seg in segments {
            let fill = seg.color.unwrap_or(style.color);
            let seg_size = seg.size.unwrap_or(style.font_size);
            let seg_edge_color = seg.edge_color.unwrap_or(style.outline_color);
            let seg_bold = seg.bold.unwrap_or(false);
            let seg_base_key = seg
                .font_family
                .as_deref()
                .and_then(|family| self.resolve_base(family))
                .unwrap_or_else(|| base_key.clone());

            // [locate]/[indent]/[endindent] 制御トークン: 空文字列セグメントとして
            // 送られてくる想定（設計判断 11）。実文字ループの前に 1 個だけダミーを push する。
            if let Some(loc) = &seg.locate {
                // y（px）は raster_segments 側で line_advance を知っているのでここで line_i に
                // 変換する。assign_lines 自体は整数 line_i と pen_x だけを扱う純粋関数のまま。
                let line_i = loc.y.map(|y| (y / line_advance).round().max(0.0) as usize);
                resolved.push(Resolved {
                    ch: '\0',
                    is_break: false,
                    advance_px: 0.0,
                    left: 0,
                    top: 0,
                    pixmap: None,
                    byte_start: byte_pos,
                    byte_end: byte_pos,
                    control: Some(CharControl::Locate {
                        pen_x: loc.x,
                        line_i,
                    }),
                });
            } else if let Some(indent) = seg.indent {
                resolved.push(Resolved {
                    ch: '\0',
                    is_break: false,
                    advance_px: 0.0,
                    left: 0,
                    top: 0,
                    pixmap: None,
                    byte_start: byte_pos,
                    byte_end: byte_pos,
                    control: Some(if indent {
                        CharControl::IndentSet
                    } else {
                        CharControl::IndentClear
                    }),
                });
            } else if let Some(img_path) = &seg.inline_image {
                // [graph]: 1 グリフ扱いのインライン画像（設計判断、W3）。advance=画像幅、
                // ベースライン下端揃え。画像自体は縁取り拡張が無いぶん、Pass3 の
                // `x = pen_x + left - pad` / `y = baseline_y - top - pad` を通常グリフと
                // 同じ式のまま辻褄が合うように left=pad, top=height-pad を逆算しておく
                // （Pass3 のコードは変更しない）。未キャッシュのパスは幅 0 で無視する。
                if let Some(pixmap) = self.image_cache.get(img_path) {
                    let pad = style.outline_width.min(MAX_OUTLINE_WIDTH) as i32;
                    let adv = pixmap.width as f32;
                    let top = pixmap.height as i32 - pad;
                    resolved.push(Resolved {
                        ch: '\0',
                        is_break: false,
                        advance_px: adv,
                        left: pad,
                        top,
                        pixmap: Some(pixmap),
                        byte_start: byte_pos,
                        byte_end: byte_pos,
                        control: None,
                    });
                }
            }

            let seg_byte_start = byte_pos;
            for ch in seg.text.chars() {
                let byte_start = byte_pos;
                byte_pos += ch.len_utf8();
                let byte_end = byte_pos;

                if ch == '\n' {
                    resolved.push(Resolved {
                        ch,
                        is_break: true,
                        advance_px: 0.0,
                        left: 0,
                        top: 0,
                        pixmap: None,
                        byte_start,
                        byte_end,
                        control: None,
                    });
                    continue;
                }

                let Some((font_id, gid)) = self.glyph_for_char(&seg_base_key, ch) else {
                    continue;
                };
                let (advance_px, left, top, pixmap) = self.render_styled_glyph(
                    font_id,
                    gid,
                    seg_size,
                    fill,
                    seg_edge_color,
                    style.outline_width.min(MAX_OUTLINE_WIDTH),
                    seg_bold,
                );

                resolved.push(Resolved {
                    ch,
                    is_break: false,
                    advance_px,
                    left,
                    top,
                    pixmap,
                    byte_start,
                    byte_end,
                    control: None,
                });
            }
            let seg_byte_end = byte_pos;
            if let Some(ruby) = &seg.ruby {
                if seg_byte_end > seg_byte_start {
                    ruby_meta.push((
                        seg_byte_start,
                        seg_byte_end,
                        ruby.clone(),
                        seg_base_key.clone(),
                    ));
                }
            }
        }

        // --- パス 2: 禁則処理込みの折返し（純粋関数、実フォントに依存しない）。
        let line_chars: Vec<LineChar> = resolved
            .iter()
            .map(|r| LineChar {
                ch: r.ch,
                advance_px: r.advance_px,
                is_break: r.is_break,
                control: r.control,
            })
            .collect();
        let placements = assign_lines(&line_chars, style.box_w as f32);
        let line_offsets =
            alignment_offsets(&line_chars, &placements, style.box_w as f32, style.align);

        // --- パス 3: 最終座標を確定し、box_h を超える行は切り捨てる。
        let pad = style.outline_width.min(MAX_OUTLINE_WIDTH) as i32;
        let mut glyphs = Vec::new();
        for (r, place) in resolved.into_iter().zip(placements) {
            let Some(pixmap) = r.pixmap else { continue };
            let line_top = place.line_i as f32 * line_advance;
            // 行の「下端」が箱を超える行は丸ごと描画しない。
            // （上端だけで判定すると、はみ出した行が箱の外へ描かれてしまう）
            if line_top + line_advance > style.box_h as f32 + 0.5 {
                continue;
            }
            let baseline_y = base_ascent_px + place.line_i as f32 * line_advance + ruby_extra;
            let align_x = line_offsets.get(&place.line_i).copied().unwrap_or(0.0);
            glyphs.push(GlyphRaster {
                x: (place.pen_x + align_x).round() as i32 + r.left - pad,
                y: baseline_y.round() as i32 - r.top - pad,
                pixmap,
                byte_start: r.byte_start,
                byte_end: r.byte_end,
                line_i: place.line_i,
            });
        }

        // --- パス 4: ルビ描画。本文セグメントの直後（reveal の Vec 順序前提）に挿入する。
        for (seg_start, seg_end, ruby_text, ruby_base_key) in ruby_meta {
            let Some(first_idx) = glyphs
                .iter()
                .position(|g| g.byte_start >= seg_start && g.byte_start < seg_end)
            else {
                continue; // 折返しカリング等でセグメントのグリフが 1 つも無い
            };
            let base_line_i = glyphs[first_idx].line_i;
            // セグメント途中の折返しは非対応: 先頭文字と同じ行のグリフだけで x 範囲を取る
            // （設計判断 2）。
            let (min_x, max_x) = glyphs
                .iter()
                .filter(|g| {
                    g.byte_start >= seg_start && g.byte_start < seg_end && g.line_i == base_line_i
                })
                .fold((i32::MAX, i32::MIN), |(mn, mx), g| {
                    (mn.min(g.x), mx.max(g.x + g.pixmap.width as i32))
                });
            let Some(last_idx) = glyphs.iter().rposition(|g| {
                g.byte_start >= seg_start && g.byte_start < seg_end && g.line_i == base_line_i
            }) else {
                continue;
            };
            let center_x = (min_x + max_x) / 2;
            let placement = RubyPlacement {
                line_i: base_line_i,
                center_x,
                line_advance,
                byte_pos: seg_end,
            };
            let ruby_glyphs = self.rasterize_ruby(&ruby_base_key, &ruby_text, style, placement);
            for (offset, rg) in ruby_glyphs.into_iter().enumerate() {
                glyphs.insert(last_idx + 1 + offset, rg);
            }
        }

        RasterText {
            glyphs,
            margin_x: style.margin_x,
            margin_y: style.margin_y,
        }
    }

    /// ルビ文字列を `ruby_font_size = style.font_size * RUBY_SIZE_RATIO` でラスタライズし、
    /// `placement.center_x` を中心に配置したグリフ列を返す（`center_x`/`line_i` は本文
    /// セグメントの x 範囲から呼び出し側で求めたもの）。ルビは縁取り無し・本文と同じ色で
    /// 描く簡易実装。（clippy::too_many_arguments 回避のため引数を `RubyPlacement` にまとめた）
    fn rasterize_ruby(
        &mut self,
        base_key: &FontKey,
        text: &str,
        style: &TextStyle,
        placement: RubyPlacement,
    ) -> Vec<GlyphRaster> {
        let RubyPlacement {
            line_i,
            center_x,
            line_advance,
            byte_pos,
        } = placement;
        let ruby_font_size = style.font_size * RUBY_SIZE_RATIO;

        struct RChar {
            pixmap: Arc<Pixmap>,
            left: i32,
            top: i32,
            advance_px: f32,
        }
        let mut rchars: Vec<RChar> = Vec::new();
        let mut total_w = 0.0f32;
        for ch in text.chars() {
            let Some((font_id, gid)) = self.glyph_for_char(base_key, ch) else {
                continue;
            };
            let (advance_px, left, top, pixmap) = self.render_styled_glyph(
                font_id,
                gid,
                ruby_font_size,
                style.color,
                [0, 0, 0, 0],
                0,
                false,
            );
            total_w += advance_px;
            let Some(pixmap) = pixmap else {
                continue;
            };
            rchars.push(RChar {
                pixmap,
                left,
                top,
                advance_px,
            });
        }
        if rchars.is_empty() {
            return Vec::new();
        }

        let ruby_ascent_px = {
            let Some(loaded) = self.font_mut(base_key) else {
                return Vec::new();
            };
            let scale = ruby_font_size / loaded.font.units_per_em() as f32;
            loaded.font.ascent() as f32 * scale
        };
        // ルビの帯は行の上端 (line_i * line_advance) から ruby_extra ぶん。
        // その帯の中に ascent 基準でベースラインを置く。
        let ruby_baseline_y = line_i as f32 * line_advance + ruby_ascent_px;
        let start_x = center_x as f32 - total_w / 2.0;

        let mut out = Vec::with_capacity(rchars.len());
        let mut pen_x = start_x;
        for rc in rchars {
            out.push(GlyphRaster {
                x: pen_x.round() as i32 + rc.left,
                y: ruby_baseline_y.round() as i32 - rc.top,
                pixmap: rc.pixmap,
                byte_start: byte_pos,
                byte_end: byte_pos,
                line_i,
            });
            pen_x += rc.advance_px;
        }
        out
    }
}

/// ルビの文字サイズ（本文 font_size に対する倍率）。
const RUBY_SIZE_RATIO: f32 = 0.45;
/// ルビ用に予約する縦方向の帯の高さ（ルビの文字サイズに対する倍率）。
const RUBY_LINE_HEIGHT: f32 = 1.1;

/// [`TextEngine::rasterize_ruby`] への配置情報（本文セグメントの x 範囲・行番号から
/// 呼び出し側で求めたもの）。引数個数を減らすためのまとめ役。
struct RubyPlacement {
    line_i: usize,
    center_x: i32,
    line_advance: f32,
    byte_pos: usize,
}
