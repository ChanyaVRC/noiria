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

use serde::{Deserialize, Serialize};

use crate::font::registry::{self, FontKey, FontSource};
use crate::font::{rasterize, size_q6, Font, GlyphCache};
use crate::pixmap::Pixmap;

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
    pub pixmap: Pixmap,
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

/// 総バイト数で上限を持つインライン画像 LRU。`has_image(&self)` の公開 API を維持しつつ
/// アクセス時刻を更新するため、時刻だけ `AtomicU64` にしている（`TextEngine` の `Sync` も保つ）。
struct ImageCache {
    map: HashMap<String, (Pixmap, AtomicU64)>,
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

    fn get(&self, path: &str) -> Option<&Pixmap> {
        self.map.get(path).map(|(p, t)| {
            t.store(self.next_tick(), Ordering::Relaxed);
            p
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
        self.map.insert(path, (pixmap, AtomicU64::new(tick)));
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
    /// ゲームローカルフォント（`fonts/*.ttf|ttc`、W1）。既定は空。`load_game_fonts` で
    /// 差し替えると、以降の family 解決・グリフフォールバックの両方で `registry`（システム
    /// フォント）より先に参照される。
    game_registry: registry::FontRegistry,
    fonts: HashMap<FontKey, LoadedFont>,
    /// 「ベースフォントに無いグリフ」をレジストリ全体から探した結果のキャッシュ。
    /// 走査コストの高いフォールバック探索を文字単位で償却する。
    glyph_fallback_cache: HashMap<char, FontKey>,
    /// どのフォントにも無かった文字（ネガティブキャッシュ）。無いと、その文字が出るたびに全システム
    /// フォントを読み直して探し直す。
    glyph_missing: HashSet<char>,
    /// `[graph]`（インライン画像, W3）用の画像キャッシュ。キーは解決済みパス。
    /// text.rs は assets への依存を増やしたくない（設計判断、
    /// `docs/implementation-notes.md` 2026-07-11 W3 参照）ため、実際のファイル I/O
    /// （`GameDir::load_image`）は呼び出し側（`engine_state::EngineState::text_print`）が
    /// 行い、ロード済みの `Pixmap` をこのキャッシュへ渡すだけにする。
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
            fonts: HashMap::new(),
            glyph_fallback_cache: HashMap::new(),
            glyph_missing: HashSet::new(),
            image_cache: ImageCache::new(IMAGE_CACHE_MAX_BYTES),
        }
    }

    /// ゲームローカルフォント（`fonts/*.ttf|ttc`、W1）を登録する。呼び出し側
    /// （`engine_state.rs`）が `GameDir::list_dir("fonts")` + `read_bytes` で集めた
    /// バイト列をファイル名昇順で渡す想定（登録順がそのままフォールバック優先度になる）。
    /// フォントが 1 つも無い/全て解析失敗でも空レジストリになるだけで、システムフォント
    /// のみだった従来の挙動へ自然にフォールバックする。
    pub fn load_game_fonts(&mut self, files: Vec<Vec<u8>>) {
        self.game_registry = registry::FontRegistry::from_game_font_bytes(&files);
        // 既存キャッシュ（フォントごと・グリフごと）はゲームフォント追加前の解決結果を
        // 引きずる可能性があるためクリアする（通常は起動直後の 1 回しか呼ばれない）。
        self.fonts.clear();
        self.glyph_fallback_cache.clear();
        self.glyph_missing.clear();
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

    /// 指定キーのフォントを（未ロードならディスク/メモリから）取得する。
    /// `FontSource::Path` はディスクから読み、`FontSource::Bytes`（ゲームローカル
    /// フォント）は既にメモリ上にあるバイト列をそのまま使う（W6 の暗号化 pak 読込でも
    /// 同じ経路をそのまま使える設計）。
    fn font_mut(&mut self, key: &FontKey) -> Option<&mut LoadedFont> {
        if !self.fonts.contains_key(key) {
            let font = Self::load_font(key)?;
            self.insert_font(key, font);
        }
        self.fonts.get_mut(key)
    }

    fn load_font(key: &FontKey) -> Option<Font> {
        let bytes = match &key.source {
            FontSource::Path(path) => std::fs::read(path).ok()?,
            FontSource::Bytes(data) => data.0.to_vec(),
        };
        Font::from_bytes(bytes, key.index).ok()
    }

    fn insert_font(&mut self, key: &FontKey, font: Font) {
        self.fonts.insert(
            key.clone(),
            LoadedFont {
                font,
                cache: GlyphCache::new(GLYPH_CACHE_CAPACITY),
            },
        );
    }

    /// `key` のフォントが `c` のグリフを持つか調べ、持つならそのフォントを保持して glyph id を返す。
    /// 持たないフォントは（未ロードなら）保持しない。フォールバック探索が全システムフォントを
    /// 読み込んで `fonts` に残し続けないため（cmap を引くだけで足りる）。
    fn probe_glyph(&mut self, key: &FontKey, c: char) -> Option<u16> {
        if let Some(loaded) = self.fonts.get(key) {
            return loaded.font.glyph_id(c).filter(|&id| id != 0);
        }
        let font = Self::load_font(key)?;
        let id = font.glyph_id(c).filter(|&id| id != 0)?;
        self.insert_font(key, font);
        Some(id)
    }

    /// style.font_family からこの断片列の基準フォント（行メトリクスに使う）を解決する。
    /// 要求 family のゲームフォント → ゲームフォント先頭（登録順。family 名が一致しなくても
    /// システムフォントより優先） → 要求 family のシステムフォント → 固定フォールバック
    /// チェーン → レジストリの先頭、の順で試す。ゲームが同梱フォントを持つなら、既定の
    /// family（"Yu Gothic UI"）がたまたま存在する Windows と存在しない Linux で結果が
    /// 食い違わないよう、同梱フォントを常に最優先にしている。
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

    /// 文字 `c` を描画できるフォント＋グリフ ID を探す。
    /// 1) `base` にあればそれを使う（通常経路）。
    /// 2) 文字単位フォールバックキャッシュ。
    /// 3) ゲームローカルフォントを登録順に試す（システムフォントより優先、W1）。
    /// 4) 固定フォールバックチェーン（per-OS）を順に試す。
    /// 5) レジストリ全体（ゲーム→システム）を走査する（結果は文字単位でキャッシュ）。
    ///
    /// どのフォントにも無ければ `None`（その文字は幅 0 で無視される）。
    fn glyph_for_char(&mut self, base: &FontKey, c: char) -> Option<(FontKey, u16)> {
        if let Some(loaded) = self.font_mut(base) {
            if let Some(id) = loaded.font.glyph_id(c).filter(|&id| id != 0) {
                return Some((base.clone(), id));
            }
        }
        if self.glyph_missing.contains(&c) {
            return None;
        }
        if let Some(cached) = self.glyph_fallback_cache.get(&c).cloned() {
            if let Some(id) = self.probe_glyph(&cached, c) {
                return Some((cached, id));
            }
        }
        let game_candidates: Vec<FontKey> = self.game_registry.iter_all().cloned().collect();
        for key in game_candidates {
            if key == *base {
                continue;
            }
            if let Some(id) = self.probe_glyph(&key, c) {
                self.glyph_fallback_cache.insert(c, key.clone());
                return Some((key, id));
            }
        }
        for fam in FALLBACK_CHAIN {
            let Some(key) = self.registry.resolve(fam) else {
                continue;
            };
            if key == *base {
                continue;
            }
            if let Some(id) = self.probe_glyph(&key, c) {
                self.glyph_fallback_cache.insert(c, key.clone());
                return Some((key, id));
            }
        }
        // 最終手段: レジストリ全体（ゲーム→システムの順）を走査する。
        // 初回のみ高コストだが結果はキャッシュされる。
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
            if let Some(id) = self.probe_glyph(&key, c) {
                self.glyph_fallback_cache.insert(c, key.clone());
                return Some((key, id));
            }
        }
        self.glyph_missing.insert(c);
        None
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
            pixmap: Option<Pixmap>,
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
                    let pad = style.outline_width as i32;
                    resolved.push(Resolved {
                        ch: '\0',
                        is_break: false,
                        advance_px: pixmap.width as f32,
                        left: pad,
                        top: pixmap.height as i32 - pad,
                        pixmap: Some(pixmap.clone()),
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

                let Some((font_key, gid)) = self.glyph_for_char(&seg_base_key, ch) else {
                    // どのフォントにもグリフが無い: 幅 0 として無視する。
                    continue;
                };
                let Some(loaded) = self.font_mut(&font_key) else {
                    continue;
                };
                let scale = seg_size / loaded.font.units_per_em() as f32;
                let advance_px = loaded.font.advance(gid) as f32 * scale;
                let size_key = size_q6(seg_size);
                let mask = match loaded.cache.get(gid, size_key) {
                    Some(m) => m.clone(),
                    None => {
                        let outline = loaded.font.outline(gid).unwrap_or_default();
                        let m = rasterize(&outline, scale);
                        loaded.cache.get_or_insert_with(gid, size_key, || m.clone());
                        m
                    }
                };

                let pixmap = if mask.width > 0 && mask.height > 0 {
                    Some(rasterize_glyph(
                        mask.width,
                        mask.height,
                        &mask.data,
                        fill,
                        seg_edge_color,
                        style.outline_width,
                        seg_bold,
                    ))
                } else {
                    None
                };

                resolved.push(Resolved {
                    ch,
                    is_break: false,
                    advance_px,
                    left: mask.left,
                    top: mask.top,
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
        let pad = style.outline_width as i32;
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
            pixmap: Pixmap,
            left: i32,
            top: i32,
            advance_px: f32,
        }
        let mut rchars: Vec<RChar> = Vec::new();
        let mut total_w = 0.0f32;
        for ch in text.chars() {
            let Some((font_key, gid)) = self.glyph_for_char(base_key, ch) else {
                continue;
            };
            let Some(loaded) = self.font_mut(&font_key) else {
                continue;
            };
            let scale = ruby_font_size / loaded.font.units_per_em() as f32;
            let advance_px = loaded.font.advance(gid) as f32 * scale;
            let size_key = size_q6(ruby_font_size);
            let mask = match loaded.cache.get(gid, size_key) {
                Some(m) => m.clone(),
                None => {
                    let outline = loaded.font.outline(gid).unwrap_or_default();
                    let m = rasterize(&outline, scale);
                    loaded.cache.get_or_insert_with(gid, size_key, || m.clone());
                    m
                }
            };
            total_w += advance_px;
            if mask.width == 0 || mask.height == 0 {
                continue;
            }
            let pixmap = rasterize_glyph(
                mask.width,
                mask.height,
                &mask.data,
                style.color,
                [0, 0, 0, 0],
                0,
                false,
            );
            rchars.push(RChar {
                pixmap,
                left: mask.left,
                top: mask.top,
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

/// [`locate`]/[`indent`]/[`endindent`] タグ用の制御トークン。`assign_lines` 内で
/// pen_x/line_i/indent_x を直接操作するダミー文字として扱う（グリフを持たず、
/// 対応する `Placement` の値は呼び出し側で使われない）。
#[derive(Clone, Copy, Debug)]
enum CharControl {
    /// `[locate x= y=]`: pen_x を x へ、y 指定時は line_i も上書きする。
    Locate { pen_x: f32, line_i: Option<usize> },
    /// `[indent]`: 現在の pen_x を以降の折返し行の行頭 x として記憶する。
    IndentSet,
    /// `[endindent]`: 折返し行頭 x を 0 に戻す。
    IndentClear,
}

/// [`assign_lines`] への 1 文字分の入力（実フォントから独立させた最小情報）。
#[derive(Clone, Copy)]
struct LineChar {
    ch: char,
    advance_px: f32,
    is_break: bool,
    control: Option<CharControl>,
}

/// [`assign_lines`] が 1 文字ごとに返す配置結果。
#[derive(Clone, Copy)]
struct Placement {
    line_i: usize,
    pen_x: f32,
}

/// 折返し＋禁則処理の中核。`chars` と 1:1 対応する `Placement` 列を返す
/// （`'\n'` の要素にもダミーの `Placement` を返すが、呼び出し側では使わない）。
///
/// 実フォントのメトリクス（グリフ形状・ラスタライズ）から独立した純粋関数なので、
/// 禁則処理のロジック単体を合成した advance 値でユニットテストできる。
fn assign_lines(chars: &[LineChar], box_w: f32) -> Vec<Placement> {
    let mut out: Vec<Placement> = Vec::with_capacity(chars.len());
    let mut pen_x = 0.0f32;
    let mut line_i = 0usize;
    let mut line_start_idx = 0usize;
    // [indent] で記憶した「以降の折返し行の行頭 x」。[endindent] で 0.0 に戻る。
    let mut indent_x = 0.0f32;

    for &LineChar {
        ch,
        advance_px,
        is_break,
        control,
    } in chars
    {
        if let Some(ctl) = control {
            // [locate]/[indent]/[endindent] 制御トークン。グリフを持たないダミー文字として
            // 1 要素分だけ消費し、pen_x/line_i/indent_x を直接操作する。
            match ctl {
                CharControl::Locate {
                    pen_x: new_pen_x,
                    line_i: new_line_i,
                } => {
                    if let Some(li) = new_line_i {
                        line_i = li;
                    }
                    pen_x = new_pen_x;
                    line_start_idx = out.len() + 1;
                }
                CharControl::IndentSet => indent_x = pen_x,
                CharControl::IndentClear => indent_x = 0.0,
            }
            out.push(Placement { line_i, pen_x });
            continue;
        }

        if is_break {
            line_i += 1;
            pen_x = indent_x;
            line_start_idx = out.len() + 1;
            out.push(Placement { line_i, pen_x });
            continue;
        }

        // 折返しは常にグリフ単位（旧 cosmic-text 実装の Wrap::Glyph に相当）。
        let would_exceed = pen_x > 0.0 && (pen_x + advance_px) > box_w;
        if would_exceed {
            if is_line_start_forbidden(ch) {
                // ぶら下げ: 行頭禁則文字は折り返さず現在行の末尾に置く
                // （box_w を若干超えることを許容する）。
            } else {
                // 道連れ: 現在行の末尾から連続する行末禁則文字（開き括弧等）を数え、
                // それらをまとめて新しい行へ送ってから折り返す。
                let mut pull_from = out.len();
                while pull_from > line_start_idx && is_line_end_forbidden(chars[pull_from - 1].ch) {
                    pull_from -= 1;
                }
                line_i += 1;
                let mut new_pen = indent_x;
                for (i, place) in out.iter_mut().enumerate().skip(pull_from) {
                    place.line_i = line_i;
                    place.pen_x = new_pen;
                    new_pen += chars[i].advance_px;
                }
                pen_x = new_pen;
                line_start_idx = pull_from;
            }
        }

        out.push(Placement { line_i, pen_x });
        pen_x += advance_px;
    }

    out
}

fn alignment_offsets(
    chars: &[LineChar],
    placements: &[Placement],
    box_w: f32,
    align: TextAlign,
) -> HashMap<usize, f32> {
    if align == TextAlign::Left {
        return HashMap::new();
    }

    let mut widths = HashMap::<usize, f32>::new();
    let mut located_lines = HashSet::<usize>::new();
    for (ch, place) in chars.iter().zip(placements) {
        if matches!(ch.control, Some(CharControl::Locate { .. })) {
            located_lines.insert(place.line_i);
        }
        if ch.is_break || ch.control.is_some() {
            continue;
        }
        let end_x = place.pen_x + ch.advance_px;
        let width = widths.entry(place.line_i).or_insert(0.0);
        *width = width.max(end_x);
    }

    widths
        .into_iter()
        .map(|(line_i, width)| {
            let remaining = (box_w - width).max(0.0);
            let offset = if located_lines.contains(&line_i) {
                0.0
            } else {
                match align {
                    TextAlign::Left => 0.0,
                    TextAlign::Center => remaining / 2.0,
                    TextAlign::Right => remaining,
                }
            };
            (line_i, offset)
        })
        .collect()
}

/// 行頭禁則文字（この文字の直前で折り返す代わりに前行末へぶら下げる）。
/// JIS X 4051 の全規則ではなく、句読点・閉じ括弧類・小書き仮名・長音符・繰返し記号・
/// 一部の約物という実用上の最小セット。
fn is_line_start_forbidden(c: char) -> bool {
    matches!(
        c,
        '。' | '、'
            | '．'
            | ','
            | '，'
            | '.'
            | '」'
            | '』'
            | '】'
            | '｝'
            | '］'
            | '〉'
            | '》'
            | '〕'
            | '’'
            | '”'
            | '〟'
            | '＞'
            | '≫'
            | '）'
            | ')'
            | 'ぁ'
            | 'ぃ'
            | 'ぅ'
            | 'ぇ'
            | 'ぉ'
            | 'っ'
            | 'ゃ'
            | 'ゅ'
            | 'ょ'
            | 'ゎ'
            | 'ァ'
            | 'ィ'
            | 'ゥ'
            | 'ェ'
            | 'ォ'
            | 'ッ'
            | 'ャ'
            | 'ュ'
            | 'ョ'
            | 'ヮ'
            | '゛'
            | '゜'
            | 'ー'
            | '～'
            | '!'
            | '?'
            | '！'
            | '？'
            | ':'
            | ';'
            | '：'
            | '；'
            | '・'
            | '…'
            | '‥'
    )
}

/// 行末禁則文字（この文字が行末に来る場合、続く文字と一緒に次行へ道連れにする）。
fn is_line_end_forbidden(c: char) -> bool {
    matches!(
        c,
        '（' | '('
            | '「'
            | '『'
            | '【'
            | '｛'
            | '［'
            | '〈'
            | '《'
            | '〔'
            | '‘'
            | '“'
            | '〝'
            | '＜'
            | '≪'
    )
}

/// グリフの 8bit カバレッジマスクから、縁取り込みのプリマルチプライ済み絵を作る。
/// （自作ラスタライザ [`crate::font::rasterize`] は常にアルファのみのマスクを返すため、
/// cosmic-text 時代にあった「カラー画像（絵文字等）」の分岐は不要になった。）
/// `bold`: 疑似太字。本体合成ループでのみ左隣ピクセルのカバレッジを拾う
/// （`coverage(x,y) = max(mask(x,y), mask(x-1,y))`）。縁取りループには適用しない
/// （設計判断 5）。advance は呼び出し側で変更しないため、キャンバス拡張も不要。
fn rasterize_glyph(
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

    // 1) 縁取り: 各ピクセルについて周囲 (半径 pad) のマスク最大値をとる
    if outline_width > 0 && outline_color[3] > 0 {
        for oy in 0..oh {
            for ox in 0..ow {
                let mut m = 0u32;
                for dy in -pad..=pad {
                    for dx in -pad..=pad {
                        if dx * dx + dy * dy > pad * pad {
                            continue;
                        }
                        m = m.max(mask_at(ox - pad + dx, oy - pad + dy));
                        if m == 255 {
                            break;
                        }
                    }
                    if m == 255 {
                        break;
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

    // 2) 本体を over 合成（bold 時は左隣ピクセルのカバレッジも拾って合成する）
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

#[cfg(test)]
mod tests {
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
        let distinct_y: std::collections::HashSet<i32> =
            r.glyphs.iter().map(|g| g.y / 10).collect();
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
                .flat_map(|g| g.pixmap.data.as_chunks::<4>().0.iter().map(move |p| p[c] as u32))
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
                .flat_map(|g| g.pixmap.data.as_chunks::<4>().0.iter().map(move |p| p[1] as u32))
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
        e.load_game_fonts(vec![crate::font::tests::minimal_font_bytes("Missing Glyph Probe")]);
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
        assert!(t.elapsed().as_millis() < 500, "2 回目の描画が遅い: {:?}", t.elapsed());
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
}
