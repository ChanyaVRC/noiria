//! シーングラフ。krkrZ 同様に fore/back の 2 ページ構成。
//! ここは純データ。描画は compose.rs、変更はスクリプトスレッドの ops が行う。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::pixmap::Pixmap;
use crate::text::RasterText;
use crate::transition::Transition;

/// レイヤの位置/不透明度アニメーション（KAG の [move] 相当）。
/// 描画時にトランジション/リベールと同じ流儀で評価する（純関数）。状態遷移
/// （完了時に確定値を書き戻す等）は呼び出し側（レンダラ/ops）の責務。
#[derive(Clone, Debug)]
pub struct Tween {
    pub from_x: i32,
    pub from_y: i32,
    pub from_opacity: u8,
    pub to_x: i32,
    pub to_y: i32,
    pub to_opacity: u8,
    pub start: Instant,
    pub duration: Duration,
    /// KAG の accel 準拠: 0 = 線形、>0 = 加速（滑り出しが遅く終盤が速い）、
    /// <0 = 減速（滑り出しが速く終盤が遅い）。
    pub accel: f32,
    pub spline: Option<SplineSegment>,
}

impl Tween {
    /// 経過に対する線形進捗 (0..1)。
    pub fn progress(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        (now.duration_since(self.start).as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0)
    }

    pub fn is_finished(&self, now: Instant) -> bool {
        self.progress(now) >= 1.0
    }

    /// 線形進捗 t (0..1) へイージングを適用する。
    /// accel > 0: t を (1+accel) 乗して立ち上がりを遅く・終盤を速くする（ease-in）。
    /// accel < 0: 終端から測った (1-t) を (1-accel) 乗して立ち上がりを速く・終盤を
    ///            遅くする（ease-out）。accel = 0 はそのまま（線形）。
    fn eased(&self, t: f32) -> f32 {
        if self.accel > 0.0 {
            t.powf(1.0 + self.accel)
        } else if self.accel < 0.0 {
            1.0 - (1.0 - t).powf(1.0 - self.accel)
        } else {
            t
        }
    }

    pub fn current_x(&self, now: Instant) -> i32 {
        let d = self.eased(self.progress(now));
        match self.spline {
            Some(spline) => spline.current_x(self.from_x, self.to_x, d),
            None => lerp_i32(self.from_x, self.to_x, d),
        }
    }

    pub fn current_y(&self, now: Instant) -> i32 {
        let d = self.eased(self.progress(now));
        match self.spline {
            Some(spline) => spline.current_y(self.from_y, self.to_y, d),
            None => lerp_i32(self.from_y, self.to_y, d),
        }
    }

    pub fn current_opacity(&self, now: Instant) -> u8 {
        lerp_u8(
            self.from_opacity,
            self.to_opacity,
            self.eased(self.progress(now)),
        )
    }
}

fn lerp_i32(a: i32, b: i32, t: f32) -> i32 {
    // 差は i32 に収まらないことがある（i32::MIN から正の値へ動かす等）ので i64 で取る。
    (a as f32 + (i64::from(b) - i64::from(a)) as f32 * t).round() as i32
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplineSegment {
    x_z: [f64; 2],
    y_z: [f64; 2],
}

impl SplineSegment {
    fn current_x(self, from: i32, to: i32, d: f32) -> i32 {
        interpolate_spline(from, to, self.x_z, d)
    }

    fn current_y(self, from: i32, to: i32, d: f32) -> i32 {
        interpolate_spline(from, to, self.y_z, d)
    }
}

fn interpolate_spline(from: i32, to: i32, z: [f64; 2], d: f32) -> i32 {
    let d = f64::from(d);
    ((((z[1] - z[0]) * d + 3.0 * z[0]) * d
        + ((f64::from(to) - f64::from(from)) - (2.0 * z[0] + z[1])))
        * d
        + from as f64)
        .round() as i32
}

pub fn natural_spline_segments(points: &[(i32, i32)]) -> Vec<SplineSegment> {
    let x: Vec<i32> = points.iter().map(|point| point.0).collect();
    let y: Vec<i32> = points.iter().map(|point| point.1).collect();
    let x_z = natural_spline_coefficients(&x);
    let y_z = natural_spline_coefficients(&y);
    (0..points.len().saturating_sub(1))
        .map(|i| SplineSegment {
            x_z: [x_z[i], x_z[i + 1]],
            y_z: [y_z[i], y_z[i + 1]],
        })
        .collect()
}

fn natural_spline_coefficients(points: &[i32]) -> Vec<f64> {
    let n = points.len();
    let mut z = vec![0.0; n];
    if n < 3 {
        return z;
    }
    let mut delta = vec![0.0; n];
    let mut tmp = vec![0.0; n];
    for i in 0..n - 1 {
        delta[i + 1] = f64::from(points[i + 1]) - f64::from(points[i]);
    }
    z[1] = delta[2] - delta[1];
    tmp[1] = 4.0;
    for i in 1..n - 2 {
        let inv = 1.0 / tmp[i];
        z[i + 1] = delta[i + 2] - delta[i + 1] - z[i] * inv;
        tmp[i + 1] = 4.0 - inv;
    }
    for i in (1..=n - 2).rev() {
        z[i] = (z[i] - z[i + 1]) / tmp[i];
    }
    z
}

/// レイヤの「見える幅」（左端から何画素ぶん合成するか）のアニメーション。位置/不透明度の
/// `Tween` とは独立に持つ（アンダーラインが左から伸びる、塗りが伸びる等のモダン UI 用）。
#[derive(Clone, Debug)]
pub struct CropTween {
    pub from: u32,
    pub to: u32,
    pub start: Instant,
    pub duration: Duration,
    /// `Tween::accel` と同じ（0 = 線形、>0 = 加速、<0 = 減速）。
    pub accel: f32,
}

impl CropTween {
    fn progress(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        (now.duration_since(self.start).as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0)
    }

    pub fn is_finished(&self, now: Instant) -> bool {
        self.progress(now) >= 1.0
    }

    pub fn current(&self, now: Instant) -> u32 {
        let t = self.progress(now);
        let d = if self.accel > 0.0 {
            t.powf(1.0 + self.accel)
        } else if self.accel < 0.0 {
            1.0 - (1.0 - t).powf(1.0 - self.accel)
        } else {
            t
        };
        (self.from as f32 + (self.to as f32 - self.from as f32) * d)
            .round()
            .max(0.0) as u32
    }
}

/// [move path=]（W5）: 経路の 1 区間ぶんの目標値。KAG の
/// `path=(x1,y1,o1)(x2,y2,o2)...` を TS 側でパースしたものが、区間ごとの
/// duration/accel を伴ってここへ渡ってくる。
#[derive(Clone, Debug)]
pub struct Waypoint {
    pub x: i32,
    pub y: i32,
    pub opacity: u8,
    pub duration: Duration,
    pub accel: f32,
    pub spline: Option<SplineSegment>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    Image,
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Fore,
    Back,
}

/// テキストレイヤのリベール（文字送り）状態。
#[derive(Clone, Debug)]
pub struct TextReveal {
    /// このグリフ番号より前は常に表示（追記前のぶん）
    pub from_glyph: usize,
    /// リベール開始時刻。None = 全表示済み
    pub start: Option<Instant>,
    /// 1 秒あたりの表示文字数。0 以下は即時表示
    pub cps: f32,
}

impl TextReveal {
    /// 現時点で表示すべきグリフ数（全体に対する上限インデックス）を返す。
    pub fn visible_glyphs(&self, total: usize, now: Instant) -> usize {
        match self.start {
            None => total,
            Some(start) => {
                if self.cps <= 0.0 {
                    return total;
                }
                let elapsed = now.duration_since(start).as_secs_f32();
                let shown = (elapsed * self.cps) as usize;
                self.from_glyph.saturating_add(shown).min(total)
            }
        }
    }

    pub fn is_done(&self, total: usize, now: Instant) -> bool {
        self.visible_glyphs(total, now) >= total
    }
}

#[derive(Clone, Debug)]
pub struct TextContent {
    pub raster: Arc<RasterText>,
    pub reveal: TextReveal,
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub kind: LayerKind,
    /// 画像レイヤの絵、またはテキストレイヤの背景ボックス。
    pub pixmap: Option<Arc<Pixmap>>,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub opacity: u8,
    pub visible: bool,
    pub text: Option<TextContent>,
    /// セーブ再構築用: 画像の読み込み元パス（ゲームルート相対）
    pub image_path: Option<String>,
    /// 進行中の位置/不透明度アニメーション（KAG の [move]）。
    pub tween: Option<Tween>,
    /// [move path=]（W5）: `tween` の次に続く区間列（折れ線の残り）。`tween` が完了する
    /// たびに先頭を取り出して新しい `tween` として開始する。単区間の [move] は常に空のまま
    /// （既存動作は完全に不変）。
    pub tween_queue: Vec<Waypoint>,
    /// 画像を左端から何画素ぶん合成するか（None = 全幅）。テキストには効かない。
    pub crop_w: Option<u32>,
    /// `crop_w` のアニメーション（`layer_move` の `toCropW`）。
    pub crop_tween: Option<CropTween>,
}

impl Layer {
    /// 表示中かつ文字送りが進行中か。
    pub fn reveal_active(&self, now: Instant) -> bool {
        self.visible
            && self.text.as_ref().is_some_and(|t| {
                t.reveal.start.is_some() && !t.reveal.is_done(t.raster.glyphs.len(), now)
            })
    }

    pub fn new(kind: LayerKind) -> Self {
        Self {
            kind,
            pixmap: None,
            x: 0,
            y: 0,
            z: 0,
            opacity: 255,
            visible: false,
            text: None,
            image_path: None,
            tween: None,
            tween_queue: Vec::new(),
            crop_w: None,
            crop_tween: None,
        }
    }

    /// 描画時刻 `now` での「見える幅」（アニメーション中はその時点の値）。
    pub fn effective_crop(&self, now: Instant) -> Option<u32> {
        match &self.crop_tween {
            Some(t) => Some(t.current(now)),
            None => self.crop_w,
        }
    }
}

/// 1 ページぶんのレイヤ集合。id は生成順の連番（Scene が採番）。
pub type LayerMap = BTreeMap<u32, Layer>;

#[derive(Clone)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub fore: LayerMap,
    pub back: LayerMap,
    pub transition: Option<Transition>,
    /// 画面揺れ (quake): (振幅x, 振幅y, 開始, 継続ms)
    pub quake: Option<(i32, i32, Instant, u32)>,
    next_id: u32,
    /// ops が変更するたびにインクリメント。レンダラの再描画判定に使う。
    pub revision: u64,
}

impl Scene {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            fore: BTreeMap::new(),
            back: BTreeMap::new(),
            transition: None,
            quake: None,
            next_id: 1,
            revision: 0,
        }
    }

    pub fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// レイヤは krkrZ 同様、常に fore/back の両ページに対で存在する。
    pub fn alloc_layer(&mut self, kind: LayerKind) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.fore.insert(id, Layer::new(kind));
        self.back.insert(id, Layer::new(kind));
        self.bump();
        id
    }

    pub fn layer_mut(&mut self, page: Page, id: u32) -> Option<&mut Layer> {
        match page {
            Page::Fore => self.fore.get_mut(&id),
            Page::Back => self.back.get_mut(&id),
        }
    }

    pub fn layer(&self, page: Page, id: u32) -> Option<&Layer> {
        match page {
            Page::Fore => self.fore.get(&id),
            Page::Back => self.back.get(&id),
        }
    }

    pub fn free_layer(&mut self, id: u32) {
        self.fore.remove(&id);
        self.back.remove(&id);
        self.bump();
    }

    /// KAG の backlay: fore の内容を back へコピーする。
    /// layer_id が Some ならそのレイヤのみ、None なら全レイヤ。
    pub fn backlay(&mut self, layer_id: Option<u32>) {
        match layer_id {
            Some(id) => {
                if let Some(l) = self.fore.get(&id) {
                    self.back.insert(id, l.clone());
                }
            }
            None => {
                self.back = self.fore.clone();
            }
        }
        self.bump();
    }

    /// KAG の [copylay]（W3、W5 で `copy_position` を追加）: 別レイヤ間で内容
    /// （絵/テキスト/不透明度/表示、`copy_position` が true なら位置も）をコピーする。
    /// `backlay` と異なり対象は異なる id 同士（同じ id も可）。z とアニメーション (tween) は
    /// 常にコピーしない（コピー先レイヤの重なり順・進行中アニメーションを壊さないため）。
    ///
    /// `copy_position`: メッセージレイヤはネイティブ位置 (x/y) を TS 側の `MessageLayer.cfg`
    /// が別途権威的に管理しており（`applyMessageLayerConfig` が再適用する）、ここで x/y を
    /// 上書きすると次に cfg が再適用されるまで両者がズレたままになる既知バグがあった
    /// （W4 で発見、`docs/implementation-notes.md`「2026-07-11 W5」参照）。呼び出し側
    /// （`tags.ts` の copylay ハンドラ）はコピー先がメッセージレイヤの場合に false を渡す。
    pub fn copy_layer(
        &mut self,
        src_page: Page,
        src_id: u32,
        dst_page: Page,
        dst_id: u32,
        copy_position: bool,
    ) {
        let Some(src) = self.layer(src_page, src_id).cloned() else {
            return;
        };
        if let Some(dst) = self.layer_mut(dst_page, dst_id) {
            dst.pixmap = src.pixmap;
            dst.text = src.text;
            dst.image_path = src.image_path;
            if copy_position {
                dst.x = src.x;
                dst.y = src.y;
            }
            dst.opacity = src.opacity;
            dst.visible = src.visible;
        }
        self.bump();
    }

    /// トランジション完了時: back ページを fore へ昇格する。back には複製が残る。
    pub fn swap_after_transition(&mut self) {
        self.fore = self.back.clone();
        self.transition = None;
        self.bump();
    }

    /// 現在アニメーション（トランジション/リベール/quake/move）が進行中か。
    pub fn is_animating(&self, now: Instant) -> bool {
        if self.transition.is_some() {
            return true;
        }
        if let Some((_, _, start, ms)) = self.quake {
            if now.duration_since(start).as_millis() < ms as u128 {
                return true;
            }
        }
        if self.fore.values().any(|l| {
            l.tween.as_ref().is_some_and(|t| !t.is_finished(now))
                || !l.tween_queue.is_empty()
                || l.crop_tween.as_ref().is_some_and(|t| !t.is_finished(now))
        }) {
            return true;
        }
        self.fore.values().any(|l| l.reveal_active(now))
    }

    /// 完了した tween を基本値 (x/y/opacity) へ確定して外す。レンダラが毎周期呼ぶ。
    /// [move path=]（W5）: 区間が完了しても `tween_queue` に次の区間が残っていれば、
    /// そこで確定した値を新しい区間の from としてそのまま次の tween を開始する
    /// （区間をまたいでも継ぎ目なく繋がる）。単区間の [move]（`tween_queue` が常に空）は
    /// 従来と完全に同じ「確定して外すだけ」の挙動になる。
    pub fn settle_finished_tweens(&mut self, now: Instant) {
        let mut changed = false;
        for layer in self.fore.values_mut().chain(self.back.values_mut()) {
            if layer.crop_tween.as_ref().is_some_and(|t| t.is_finished(now)) {
                let t = layer.crop_tween.take().unwrap();
                layer.crop_w = Some(t.to);
                changed = true;
            }
            while layer.tween.as_ref().is_some_and(|t| t.is_finished(now)) {
                let t = layer.tween.take().unwrap();
                layer.x = t.to_x;
                layer.y = t.to_y;
                layer.opacity = t.to_opacity;
                changed = true;
                if layer.tween_queue.is_empty() {
                    break;
                }
                let wp = layer.tween_queue.remove(0);
                layer.tween = Some(Tween {
                    from_x: layer.x,
                    from_y: layer.y,
                    from_opacity: layer.opacity,
                    to_x: wp.x,
                    to_y: wp.y,
                    to_opacity: wp.opacity,
                    start: t.start + t.duration,
                    duration: wp.duration,
                    accel: wp.accel,
                    spline: wp.spline,
                });
            }
        }
        if changed {
            self.bump();
        }
    }

    /// 全レイヤの tween を即座に完了させる（スキップモード用）。[move path=]（W5）で
    /// `tween_queue` に区間が残っている場合は、最終区間の目標値まで一気に確定する
    /// （途中の区間を経由しない）。`tween_queue` が空の従来ケースは完全に不変。
    pub fn skip_all_tweens(&mut self) {
        let mut changed = false;
        for layer in self.fore.values_mut().chain(self.back.values_mut()) {
            if let Some(t) = layer.crop_tween.take() {
                layer.crop_w = Some(t.to);
                changed = true;
            }
            if let Some(t) = layer.tween.take() {
                let (x, y, opacity) = match layer.tween_queue.last() {
                    Some(last) => (last.x, last.y, last.opacity),
                    None => (t.to_x, t.to_y, t.to_opacity),
                };
                layer.x = x;
                layer.y = y;
                layer.opacity = opacity;
                layer.tween_queue.clear();
                changed = true;
            }
        }
        if changed {
            self.bump();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition::{TransMethod, Transition};
    use std::time::Duration;

    #[test]
    fn layers_exist_on_both_pages() {
        let mut scene = Scene::new(4, 4);
        let id = scene.alloc_layer(LayerKind::Image);
        assert!(scene.layer(Page::Fore, id).is_some());
        assert!(scene.layer(Page::Back, id).is_some());
        scene.free_layer(id);
        assert!(scene.layer(Page::Fore, id).is_none());
        assert!(scene.layer(Page::Back, id).is_none());
    }

    #[test]
    fn backlay_copies_fore_to_back() {
        let mut scene = Scene::new(4, 4);
        let a = scene.alloc_layer(LayerKind::Image);
        let b = scene.alloc_layer(LayerKind::Image);
        scene.layer_mut(Page::Fore, a).unwrap().x = 11;
        scene.layer_mut(Page::Fore, b).unwrap().x = 22;
        scene.layer_mut(Page::Back, a).unwrap().x = 0;

        // 単一レイヤの backlay
        scene.backlay(Some(a));
        assert_eq!(scene.layer(Page::Back, a).unwrap().x, 11);
        assert_eq!(scene.layer(Page::Back, b).unwrap().x, 0);

        // 全レイヤの backlay
        scene.backlay(None);
        assert_eq!(scene.layer(Page::Back, b).unwrap().x, 22);
    }

    #[test]
    fn copy_layer_copies_content_but_not_z_or_tween() {
        let mut scene = Scene::new(4, 4);
        let src = scene.alloc_layer(LayerKind::Text);
        let dst = scene.alloc_layer(LayerKind::Text);
        {
            let s = scene.layer_mut(Page::Fore, src).unwrap();
            s.x = 10;
            s.y = 20;
            s.z = 99;
            s.opacity = 128;
            s.visible = true;
            s.image_path = Some("assets/fg/icon.png".into());
        }
        {
            let d = scene.layer_mut(Page::Fore, dst).unwrap();
            d.z = 5;
            d.visible = false;
        }
        scene.copy_layer(Page::Fore, src, Page::Fore, dst, true);
        let d = scene.layer(Page::Fore, dst).unwrap();
        assert_eq!(
            (d.x, d.y, d.opacity, d.visible),
            (10, 20, 128, true),
            "内容がコピーされていない"
        );
        assert_eq!(d.image_path.as_deref(), Some("assets/fg/icon.png"));
        assert_eq!(d.z, 5, "z はコピー対象外のはず");
    }

    #[test]
    fn copy_layer_missing_src_is_a_noop() {
        let mut scene = Scene::new(4, 4);
        let dst = scene.alloc_layer(LayerKind::Text);
        scene.copy_layer(Page::Fore, 9999, Page::Fore, dst, true);
        assert_eq!(scene.layer(Page::Fore, dst).unwrap().x, 0);
    }

    /// [copylay] 既知バグの再現テスト（W4 で発見、W5 で修正）: コピー先がメッセージレイヤの
    /// 場合、`copy_position=false` を渡すとコピー先の x/y（TS 側 `MessageLayer.cfg` が管理する
    /// ネイティブ位置）が変化しないことを確認する。内容 (pixmap/text/opacity/visible) は
    /// これまで通りコピーされる。`copy_position=true`（一般レイヤ向け、既存 2 テストが検証済み）
    /// との対比で、フラグが実際に位置コピーだけを分離できていることを示す。
    #[test]
    fn copy_layer_can_exclude_position_reproducing_message_layer_bug() {
        let mut scene = Scene::new(4, 4);
        let src = scene.alloc_layer(LayerKind::Text); // message1 相当（top=90 等、別位置）
        let dst = scene.alloc_layer(LayerKind::Text); // message0 相当（cfg 管理下の既定位置）
        {
            let s = scene.layer_mut(Page::Fore, src).unwrap();
            s.x = 999;
            s.y = 90;
            s.opacity = 200;
            s.visible = true;
            s.image_path = Some("assets/fg/msgbox_src.png".into());
        }
        {
            let d = scene.layer_mut(Page::Fore, dst).unwrap();
            d.x = 40; // message0 の cfg.left 相当（変化してはいけない）
            d.y = 566; // message0 の cfg.top 相当（変化してはいけない）
        }
        scene.copy_layer(Page::Fore, src, Page::Fore, dst, false);
        let d = scene.layer(Page::Fore, dst).unwrap();
        assert_eq!(
            (d.x, d.y),
            (40, 566),
            "copy_position=false ではコピー先の位置が変化してはいけない"
        );
        assert_eq!(
            d.opacity, 200,
            "位置以外の内容は copy_position に関わらずコピーされるはず"
        );
        assert_eq!(d.image_path.as_deref(), Some("assets/fg/msgbox_src.png"));
    }

    #[test]
    fn swap_after_transition_promotes_back() {
        let mut scene = Scene::new(4, 4);
        let id = scene.alloc_layer(LayerKind::Image);
        scene.layer_mut(Page::Back, id).unwrap().x = 99;
        scene.transition = Some(Transition {
            method: TransMethod::Crossfade,
            start: Instant::now(),
            duration: Duration::from_millis(1),
            skip: true,
        });
        scene.swap_after_transition();
        assert_eq!(scene.layer(Page::Fore, id).unwrap().x, 99);
        // back にも複製が残る（連続トランジションのため）
        assert_eq!(scene.layer(Page::Back, id).unwrap().x, 99);
        assert!(scene.transition.is_none());
    }

    #[test]
    fn revision_bumps_on_mutation() {
        let mut scene = Scene::new(4, 4);
        let r0 = scene.revision;
        let id = scene.alloc_layer(LayerKind::Image);
        assert!(scene.revision > r0);
        let r1 = scene.revision;
        scene.backlay(None);
        assert!(scene.revision > r1);
        let r2 = scene.revision;
        scene.free_layer(id);
        assert!(scene.revision > r2);
    }

    #[test]
    fn reveal_visible_glyphs_over_time() {
        let start = Instant::now();
        let reveal = TextReveal {
            from_glyph: 5,
            start: Some(start),
            cps: 10.0, // 100ms/文字
        };
        // 0ms 時点: 追記前の 5 グリフのみ
        assert_eq!(reveal.visible_glyphs(20, start), 5);
        // 500ms 時点: 5 + 5
        assert_eq!(
            reveal.visible_glyphs(20, start + Duration::from_millis(500)),
            10
        );
        // 十分経過後: 全部 (上限クランプ)
        assert_eq!(
            reveal.visible_glyphs(20, start + Duration::from_secs(60)),
            20
        );
        assert!(reveal.is_done(20, start + Duration::from_secs(60)));
        assert!(!reveal.is_done(20, start));
    }

    #[test]
    fn reveal_none_means_complete() {
        let reveal = TextReveal {
            from_glyph: 0,
            start: None,
            cps: 10.0,
        };
        assert_eq!(reveal.visible_glyphs(7, Instant::now()), 7);
        assert!(reveal.is_done(7, Instant::now()));
    }

    #[test]
    fn animating_states() {
        let mut scene = Scene::new(4, 4);
        let now = Instant::now();
        assert!(!scene.is_animating(now));

        scene.transition = Some(Transition {
            method: TransMethod::Crossfade,
            start: now,
            duration: Duration::from_secs(1),
            skip: false,
        });
        assert!(scene.is_animating(now));
        scene.transition = None;

        scene.quake = Some((5, 5, now, 300));
        assert!(scene.is_animating(now));
        assert!(!scene.is_animating(now + Duration::from_millis(400)));
    }

    fn tween(accel: f32) -> Tween {
        Tween {
            from_x: 0,
            from_y: 100,
            from_opacity: 0,
            to_x: 100,
            to_y: 200,
            to_opacity: 200,
            start: Instant::now(),
            duration: Duration::from_millis(100),
            accel,
            spline: None,
        }
    }

    #[test]
    fn tween_linear_midpoint() {
        let t = tween(0.0);
        let mid = t.start + Duration::from_millis(50);
        assert_eq!(t.current_x(mid), 50);
        assert_eq!(t.current_y(mid), 150);
        assert_eq!(t.current_opacity(mid), 100);
        assert!(!t.is_finished(mid));
        assert!(t.is_finished(t.start + Duration::from_millis(100)));
    }

    #[test]
    fn natural_spline_uses_kag_coefficients_and_keeps_opacity_linear() {
        let segments = natural_spline_segments(&[(0, 0), (100, -100), (0, 0)]);
        assert_eq!(segments.len(), 2);
        let start = Instant::now();
        let first = Tween {
            from_x: 0,
            from_y: 0,
            from_opacity: 0,
            to_x: 100,
            to_y: -100,
            to_opacity: 200,
            start,
            duration: Duration::from_millis(100),
            accel: 0.0,
            spline: Some(segments[0]),
        };
        let mid = start + Duration::from_millis(50);
        assert_eq!((first.current_x(mid), first.current_y(mid)), (69, -69));
        assert_ne!(first.current_x(mid), 50);
        assert_eq!(first.current_opacity(mid), 100);
        assert_eq!((first.current_x(start), first.current_y(start)), (0, 0));
        assert_eq!(
            (
                first.current_x(start + Duration::from_millis(100)),
                first.current_y(start + Duration::from_millis(100))
            ),
            (100, -100)
        );

        let second = Tween {
            from_x: 100,
            from_y: -100,
            from_opacity: 200,
            to_x: 0,
            to_y: 0,
            to_opacity: 255,
            start,
            duration: Duration::from_millis(100),
            accel: 0.0,
            spline: Some(segments[1]),
        };
        assert_eq!(
            (second.current_x(start), second.current_y(start)),
            (100, -100)
        );
        assert_eq!(
            (
                second.current_x(start + Duration::from_millis(100)),
                second.current_y(start + Duration::from_millis(100))
            ),
            (0, 0)
        );

        let four_point_segments = natural_spline_segments(&[(0, 0), (100, 0), (100, 0), (0, 0)]);
        let middle = Tween {
            from_x: 100,
            from_y: 0,
            from_opacity: 255,
            to_x: 100,
            to_y: 0,
            to_opacity: 255,
            start,
            duration: Duration::from_millis(100),
            accel: 0.0,
            spline: Some(four_point_segments[1]),
        };
        assert_eq!(middle.current_x(mid), 115);
    }

    #[test]
    fn tween_accelerate_starts_slower_than_linear() {
        // accel > 0: 序盤は線形より遅れ、終盤で追いつく（ease-in）。
        let t = tween(1.0); // t^2
        let quarter = t.start + Duration::from_millis(25);
        assert!(
            t.current_x(quarter) < 25,
            "ease-in は序盤で線形より遅いはず"
        );
        let end = t.start + Duration::from_millis(100);
        assert_eq!(t.current_x(end), 100);
    }

    #[test]
    fn tween_decelerate_starts_faster_than_linear() {
        // accel < 0: 序盤は線形より速く進み、終盤で緩やかになる（ease-out）。
        let t = tween(-1.0);
        let quarter = t.start + Duration::from_millis(25);
        assert!(
            t.current_x(quarter) > 25,
            "ease-out は序盤で線形より速いはず"
        );
        let end = t.start + Duration::from_millis(100);
        assert_eq!(t.current_x(end), 100);
    }

    #[test]
    fn settle_finished_tweens_writes_to_values_and_clears() {
        let mut scene = Scene::new(4, 4);
        let id = scene.alloc_layer(LayerKind::Image);
        let t = tween(0.0);
        let start = t.start;
        scene.layer_mut(Page::Fore, id).unwrap().tween = Some(t);

        // 未完了のうちは確定しない。
        scene.settle_finished_tweens(start + Duration::from_millis(50));
        assert!(scene.layer(Page::Fore, id).unwrap().tween.is_some());

        // 完了後は to 値が基本値へ書き戻され、tween が外れる。
        scene.settle_finished_tweens(start + Duration::from_millis(100));
        let layer = scene.layer(Page::Fore, id).unwrap();
        assert!(layer.tween.is_none());
        assert_eq!((layer.x, layer.y, layer.opacity), (100, 200, 200));
    }

    #[test]
    fn skip_all_tweens_forces_immediate_completion() {
        let mut scene = Scene::new(4, 4);
        let id = scene.alloc_layer(LayerKind::Image);
        scene.layer_mut(Page::Fore, id).unwrap().tween = Some(tween(0.0));
        let start = Instant::now();

        // 開始直後でも skip すれば to 値へ即確定する。
        scene.skip_all_tweens();
        let layer = scene.layer(Page::Fore, id).unwrap();
        assert!(layer.tween.is_none());
        assert_eq!((layer.x, layer.y, layer.opacity), (100, 200, 200));
        let _ = start;
    }

    #[test]
    fn is_animating_true_while_tween_active() {
        let mut scene = Scene::new(4, 4);
        let id = scene.alloc_layer(LayerKind::Image);
        let t = tween(0.0);
        let start = t.start;
        scene.layer_mut(Page::Fore, id).unwrap().tween = Some(t);

        assert!(scene.is_animating(start + Duration::from_millis(50)));
        assert!(!scene.is_animating(start + Duration::from_millis(100)));
    }

    // ---- [move path=]（W5/B12）: 区間列（Waypoint） ----

    /// 2 区間 (leg1: 0→100 opacity 0→200 / leg2: 100→300 opacity 200→50) を持つ
    /// tween_queue を手で組み立てるヘルパ。`layer_move_path`（TS/ops 層）の呼び出しを
    /// 経由せずシーン層の継ぎ目ロジックだけを検証する。
    fn two_leg_layer(scene: &mut Scene, start: Instant) -> u32 {
        let id = scene.alloc_layer(LayerKind::Image);
        let layer = scene.layer_mut(Page::Fore, id).unwrap();
        layer.x = 0;
        layer.y = 0;
        layer.opacity = 0;
        layer.tween = Some(Tween {
            from_x: 0,
            from_y: 0,
            from_opacity: 0,
            to_x: 100,
            to_y: 0,
            to_opacity: 200,
            start,
            duration: Duration::from_millis(100),
            accel: 0.0,
            spline: None,
        });
        layer.tween_queue = vec![Waypoint {
            x: 300,
            y: 0,
            opacity: 50,
            duration: Duration::from_millis(100),
            accel: 0.0,
            spline: None,
        }];
        id
    }

    #[test]
    fn settle_finished_tweens_chains_to_next_waypoint_seamlessly() {
        let mut scene = Scene::new(4, 4);
        let start = Instant::now();
        let id = two_leg_layer(&mut scene, start);

        // leg1 の完了時点（100ms）: leg1 の to 値へ確定しつつ、即座に leg2 の tween が
        // 開始されている（tween_queue から取り出されて空になる）はず。
        scene.settle_finished_tweens(start + Duration::from_millis(100));
        {
            let layer = scene.layer(Page::Fore, id).unwrap();
            assert_eq!(
                (layer.x, layer.opacity),
                (100, 200),
                "leg1 の to 値まで進んでいるはず"
            );
            assert!(
                layer.tween.is_some(),
                "leg2 の tween が続けて開始されているはず"
            );
            assert!(layer.tween_queue.is_empty());
        }

        // leg2 の中間点（150ms = leg1終了+50ms）: leg1 to (100) と leg2 to (300) の中間。
        scene.settle_finished_tweens(start + Duration::from_millis(150));
        let mid_x = scene
            .layer(Page::Fore, id)
            .unwrap()
            .tween
            .as_ref()
            .unwrap()
            .current_x(start + Duration::from_millis(150));
        assert!(
            (150..=250).contains(&mid_x),
            "leg2 の中間点付近のはず, got {mid_x}"
        );

        // leg2 の完了時点（200ms）: 最終区間の to 値へ確定し、tween が外れる。
        scene.settle_finished_tweens(start + Duration::from_millis(200));
        let layer = scene.layer(Page::Fore, id).unwrap();
        assert!(layer.tween.is_none());
        assert_eq!(
            (layer.x, layer.opacity),
            (300, 50),
            "leg2 の to 値（最終目標）まで進むはず"
        );
    }

    #[test]
    fn spline_waypoint_survives_settle_and_skip_reaches_final_endpoint() {
        let mut scene = Scene::new(4, 4);
        let start = Instant::now();
        let segments = natural_spline_segments(&[(0, 0), (100, 100), (0, 0)]);
        let id = scene.alloc_layer(LayerKind::Image);
        let layer = scene.layer_mut(Page::Fore, id).unwrap();
        layer.tween = Some(Tween {
            from_x: 0,
            from_y: 0,
            from_opacity: 0,
            to_x: 100,
            to_y: 100,
            to_opacity: 100,
            start,
            duration: Duration::from_millis(100),
            accel: 0.0,
            spline: Some(segments[0]),
        });
        layer.tween_queue.push(Waypoint {
            x: 0,
            y: 0,
            opacity: 200,
            duration: Duration::from_millis(100),
            accel: 0.0,
            spline: Some(segments[1]),
        });

        scene.settle_finished_tweens(start + Duration::from_millis(100));
        let layer = scene.layer(Page::Fore, id).unwrap();
        let second = layer.tween.as_ref().unwrap();
        assert_eq!(second.spline, Some(segments[1]));
        assert_eq!(
            (
                second.current_x(start + Duration::from_millis(150)),
                second.current_y(start + Duration::from_millis(150))
            ),
            (69, 69)
        );

        scene.skip_all_tweens();
        let layer = scene.layer(Page::Fore, id).unwrap();
        assert!(layer.tween.is_none());
        assert!(layer.tween_queue.is_empty());
        assert_eq!((layer.x, layer.y, layer.opacity), (0, 0, 200));
    }

    #[test]
    fn skip_all_tweens_jumps_to_final_waypoint_not_first_leg() {
        let mut scene = Scene::new(4, 4);
        let start = Instant::now();
        let id = two_leg_layer(&mut scene, start);

        // leg1 の開始直後に skip しても、最終区間 (leg2) の to 値まで一気に確定するはず
        // （tween_queue が空の従来ケースでは leg1 の to 値がそのまま最終値になるので不変）。
        scene.skip_all_tweens();
        let layer = scene.layer(Page::Fore, id).unwrap();
        assert!(layer.tween.is_none());
        assert!(layer.tween_queue.is_empty());
        assert_eq!(
            (layer.x, layer.opacity),
            (300, 50),
            "最終 waypoint まで一気に完了するはず"
        );
    }

    #[test]
    fn is_animating_true_while_tween_queue_pending_even_if_current_leg_finished() {
        let mut scene = Scene::new(4, 4);
        let start = Instant::now();
        two_leg_layer(&mut scene, start);

        // settle_finished_tweens を呼ばずに leg1 完了時刻を渡した場合でも、tween_queue に
        // 続きが残っていることを is_animating が拾えることを確認する（防御的チェック）。
        assert!(scene.is_animating(start + Duration::from_millis(100)));
    }

    #[test]
    fn single_leg_move_behavior_is_unchanged_when_queue_is_empty() {
        // tween_queue が空の従来ケース（既存 move）は settle/skip とも旧来通りの挙動になる
        // ことの回帰確認（既存テストと同種のシナリオを Waypoint 導入後にも再確認する）。
        let mut scene = Scene::new(4, 4);
        let id = scene.alloc_layer(LayerKind::Image);
        let t = tween(0.0);
        let start = t.start;
        scene.layer_mut(Page::Fore, id).unwrap().tween = Some(t);

        scene.settle_finished_tweens(start + Duration::from_millis(100));
        let layer = scene.layer(Page::Fore, id).unwrap();
        assert!(layer.tween.is_none());
        assert_eq!((layer.x, layer.y, layer.opacity), (100, 200, 200));
    }
}

#[cfg(test)]
mod crop_tests {
    use super::*;
    use crate::compose::{compose_frame, ComposeBufs};
    use std::sync::Arc;

    fn white(w: u32, h: u32) -> Arc<Pixmap> {
        let mut p = Pixmap::new(w, h);
        p.fill([255, 255, 255, 255]);
        Arc::new(p)
    }

    fn crop_tween(from: u32, to: u32, accel: f32, start: Instant) -> CropTween {
        CropTween {
            from,
            to,
            start,
            duration: Duration::from_millis(100),
            accel,
        }
    }

    #[test]
    fn crop_tween_interpolates_linearly_and_eases() {
        let start = Instant::now();
        let lin = crop_tween(0, 100, 0.0, start);
        assert_eq!(lin.current(start), 0);
        assert_eq!(lin.current(start + Duration::from_millis(50)), 50);
        assert_eq!(lin.current(start + Duration::from_millis(100)), 100);
        assert!(lin.is_finished(start + Duration::from_millis(100)));
        // 減速（出現向き）は序盤が速い、加速は序盤が遅い
        let out = crop_tween(0, 100, -2.0, start);
        let inn = crop_tween(0, 100, 2.0, start);
        let mid = start + Duration::from_millis(50);
        assert!(out.current(mid) > 50 && inn.current(mid) < 50);
        // 縮む方向
        let shrink = crop_tween(100, 20, 0.0, start);
        assert_eq!(shrink.current(start + Duration::from_millis(50)), 60);
    }

    #[test]
    fn composes_only_the_cropped_width_and_settle_commits_the_target() {
        let mut scene = Scene::new(10, 2);
        let id = scene.alloc_layer(LayerKind::Image);
        let start = Instant::now();
        {
            let l = scene.layer_mut(Page::Fore, id).unwrap();
            l.pixmap = Some(white(10, 2));
            l.visible = true;
            l.crop_w = Some(0);
            l.crop_tween = Some(crop_tween(0, 10, 0.0, start));
        }
        let mut bufs = ComposeBufs::new(10, 2);
        let mut out = Pixmap::new(10, 2);
        let px = |p: &Pixmap, x: usize| p.data[x * 4 + 3];
        compose_frame(&scene, start, &mut bufs, &mut out);
        assert_eq!(px(&out, 0), 255, "alpha は不透明な黒背景 (幅 0 なら画素は黒のまま)");
        assert_eq!(out.data[0], 0, "幅 0 では何も重ならない");
        compose_frame(&scene, start + Duration::from_millis(50), &mut bufs, &mut out);
        assert_eq!(out.data[4 * 4], 255, "x=4 は見える幅 5 の内側（白）");
        assert_eq!(out.data[7 * 4], 0, "x=7 は見える幅の外（黒のまま）");
        assert!(scene.is_animating(start + Duration::from_millis(50)));
        // 完了前は確定しない。完了後に settle すると crop_w が目標値になり tween は外れる。
        scene.settle_finished_tweens(start + Duration::from_millis(50));
        assert!(scene.fore[&id].crop_tween.is_some());
        scene.settle_finished_tweens(start + Duration::from_millis(100));
        assert!(scene.fore[&id].crop_tween.is_none());
        assert_eq!(scene.fore[&id].crop_w, Some(10));
        assert!(!scene.is_animating(start + Duration::from_millis(100)));
        compose_frame(&scene, start + Duration::from_millis(200), &mut bufs, &mut out);
        assert_eq!(out.data[9 * 4], 255);
    }

    #[test]
    fn skip_all_tweens_commits_the_crop_target() {
        let mut scene = Scene::new(4, 1);
        let id = scene.alloc_layer(LayerKind::Image);
        let start = Instant::now();
        scene.layer_mut(Page::Fore, id).unwrap().crop_tween = Some(crop_tween(0, 4, 0.0, start));
        scene.skip_all_tweens();
        let l = &scene.fore[&id];
        assert!(l.crop_tween.is_none());
        assert_eq!(l.crop_w, Some(4));
    }

    #[test]
    fn blit_over_clipped_none_equals_blit_over() {
        let src = white(3, 3);
        let mut a = Pixmap::new(6, 6);
        let mut b = Pixmap::new(6, 6);
        a.blit_over(&src, 1, 1, 200);
        b.blit_over_clipped(&src, 1, 1, 200, None);
        assert_eq!(a, b);
        let mut c = Pixmap::new(6, 6);
        c.blit_over_clipped(&src, 1, 1, 255, Some(2));
        assert_eq!(c.data[(6 + 2) * 4], 255);
        assert_eq!(c.data[(6 + 3) * 4], 0, "3 列目は切れる");
    }
    #[test]
    fn reveal_with_huge_cps_saturates_instead_of_overflowing() {
        let now = Instant::now();
        let start = now.checked_sub(Duration::from_secs(1)).unwrap();
        for cps in [f32::INFINITY, f32::MAX, 1e30] {
            let r = TextReveal {
                from_glyph: 5,
                start: Some(start),
                cps,
            };
            assert_eq!(r.visible_glyphs(100, now), 100, "cps={cps}");
            assert!(r.is_done(100, now));
        }
    }
}
