//! 純関数コンポジタ。Scene のスナップショットを 1 フレームの Pixmap に合成する。
//! 状態遷移（トランジション完了時のページスワップ等）は呼び出し側の責務。

use std::sync::Arc;
use std::time::Instant;

use crate::pixmap::Pixmap;
use crate::scene::{Layer, LayerMap, Scene};
use crate::text::RasterText;
use crate::transition::TransMethod;

/// 再利用する作業バッファ（毎フレームの確保を避ける）。
/// `fore` は通常描画時に `PrefixCache` のチェックポイントとして共用し、`back` は
/// トランジション中のみ確保・終了時に解放することで、常駐する全画面バッファ数を最小化する。
pub struct ComposeBufs {
    fore: Pixmap,
    back: Pixmap,
    cached_revision: Option<u64>,
    prefix: PrefixCache,
}

struct LayerSig {
    id: u32,
    x: i32,
    y: i32,
    opacity: u8,
    crop: Option<u32>,
    pixmap: Option<Arc<Pixmap>>,
    text: Option<(Arc<RasterText>, u32)>,
}

impl LayerSig {
    fn of(id: u32, layer: &Layer, now: Instant, offset: (i32, i32)) -> Self {
        let (ex, ey, eo) = effective_transform(layer, now);
        LayerSig {
            id,
            x: ex + offset.0,
            y: ey + offset.1,
            opacity: eo,
            crop: layer.effective_crop(now),
            pixmap: layer.pixmap.clone(),
            text: layer.text.as_ref().map(|t| {
                let total = t.raster.glyphs.len();
                (t.raster.clone(), t.reveal.visible_glyphs(total, now) as u32)
            }),
        }
    }

    fn same(&self, o: &LayerSig) -> bool {
        self.id == o.id
            && self.x == o.x
            && self.y == o.y
            && self.opacity == o.opacity
            && self.crop == o.crop
            && match (&self.pixmap, &o.pixmap) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
            && match (&self.text, &o.text) {
                (None, None) => true,
                (Some((a, n)), Some((b, m))) => Arc::ptr_eq(a, b) && n == m,
                _ => false,
            }
    }
}

struct PrefixCache {
    prev: Vec<LayerSig>,
    cur: Vec<LayerSig>,
    order: Vec<(i32, u32)>,
    prev_offset: (i32, i32),
    /// `fore` バッファが保持している下層チェックポイントの層数（0 = 無効）。
    len: usize,
}

impl ComposeBufs {
    pub fn new(_width: u32, _height: u32) -> Self {
        Self {
            fore: Pixmap::new(0, 0),
            back: Pixmap::new(0, 0),
            cached_revision: None,
            prefix: PrefixCache {
                prev: Vec::new(),
                cur: Vec::new(),
                order: Vec::new(),
                prev_offset: (0, 0),
                len: 0,
            },
        }
    }
}

fn ensure_surface(buf: &mut Pixmap, w: u32, h: u32) {
    if buf.width != w || buf.height != h {
        *buf = Pixmap::new(w, h);
    }
}

/// ページ内容が時刻依存（リベール進行中/quake 中）かどうか。
/// 時刻依存ならページ合成をキャッシュできない。
fn pages_time_dependent(scene: &Scene, now: Instant) -> bool {
    if scene.quake.is_some() {
        return true;
    }
    scene.fore.values().chain(scene.back.values()).any(|l| {
        l.reveal_active(now)
            || l.tween.as_ref().is_some_and(|t| !t.is_finished(now))
            || l.crop_tween.as_ref().is_some_and(|t| !t.is_finished(now))
    })
}

/// レイヤの実効 x/y/opacity。tween 進行中はその時点の補間値、それ以外は基本値。
fn effective_transform(layer: &Layer, now: Instant) -> (i32, i32, u8) {
    match &layer.tween {
        Some(t) => (t.current_x(now), t.current_y(now), t.current_opacity(now)),
        None => (layer.x, layer.y, layer.opacity),
    }
}

/// シーンを out へ合成する。
pub fn compose_frame(scene: &Scene, now: Instant, bufs: &mut ComposeBufs, out: &mut Pixmap) {
    let quake = quake_offset(scene, now);

    match &scene.transition {
        Some(trans) => {
            bufs.prefix.len = 0;
            ensure_surface(&mut bufs.fore, out.width, out.height);
            ensure_surface(&mut bufs.back, out.width, out.height);
            let cacheable = !pages_time_dependent(scene, now);
            if !cacheable || bufs.cached_revision != Some(scene.revision) {
                compose_page(&scene.fore, now, quake, &mut bufs.fore);
                compose_page(&scene.back, now, quake, &mut bufs.back);
                bufs.cached_revision = if cacheable {
                    Some(scene.revision)
                } else {
                    None
                };
            }
            let t = trans.progress(now);
            match &trans.method {
                TransMethod::Crossfade => out.lerp_from(&bufs.fore, &bufs.back, t),
                TransMethod::Universal { rule, vague } => {
                    out.universal_from(&bufs.fore, &bufs.back, rule, t, *vague)
                }
                TransMethod::Scroll { from } => out.scroll_from(&bufs.fore, &bufs.back, *from, t),
            }
        }
        None => {
            bufs.cached_revision = None;
            if !bufs.back.data.is_empty() {
                bufs.back = Pixmap::new(0, 0);
            }
            compose_page_cached(
                &scene.fore,
                now,
                quake,
                out,
                &mut bufs.prefix,
                &mut bufs.fore,
            );
        }
    }
}

fn quake_offset(scene: &Scene, now: Instant) -> (i32, i32) {
    let Some(&(ax, ay, start, ms)) = scene.quake.as_deref() else {
        return (0, 0);
    };
    let elapsed = now.duration_since(start).as_millis() as u32;
    if elapsed >= ms {
        return (0, 0);
    }
    // 減衰付きの擬似ランダム揺れ（決定的）
    let t = elapsed as f32 / 1000.0;
    let decay = 1.0 - (elapsed as f32 / ms as f32);
    let dx = (t * 73.0).sin() * ax as f32 * decay;
    let dy = (t * 97.0).cos() * ay as f32 * decay;
    (dx as i32, dy as i32)
}

fn compose_page(layers: &LayerMap, now: Instant, offset: (i32, i32), out: &mut Pixmap) {
    out.fill([0, 0, 0, 255]);
    let mut sorted: Vec<(&u32, &Layer)> = layers.iter().filter(|(_, l)| l.visible).collect();
    sorted.sort_by_key(|(id, l)| (l.z, **id));
    for (_, layer) in sorted {
        draw_layer(out, layer, now, offset);
    }
}

fn draw_layer(out: &mut Pixmap, layer: &Layer, now: Instant, offset: (i32, i32)) {
    let (ex, ey, eo) = effective_transform(layer, now);
    let lx = ex + offset.0;
    let ly = ey + offset.1;
    if let Some(pixmap) = &layer.pixmap {
        out.blit_over_clipped(pixmap, lx, ly, eo, layer.effective_crop(now));
    }
    if let Some(text) = &layer.text {
        let total = text.raster.glyphs.len();
        let visible = text.reveal.visible_glyphs(total, now);
        for g in text.raster.glyphs.iter().take(visible) {
            out.blit_over(
                &g.pixmap,
                lx + text.raster.margin_x + g.x,
                ly + text.raster.margin_y + g.y,
                eo,
            );
        }
    }
}

/// `compose_page` と同じ結果を、変わっていない下層を再利用して描く（`PrefixCache` 参照）。
fn compose_page_cached(
    layers: &LayerMap,
    now: Instant,
    offset: (i32, i32),
    out: &mut Pixmap,
    cache: &mut PrefixCache,
    checkpoint: &mut Pixmap,
) {
    cache.order.clear();
    for (&id, l) in layers {
        if l.visible {
            cache.order.push((l.z, id));
        }
    }
    cache.order.sort_unstable();

    cache.cur.clear();
    for &(_, id) in &cache.order {
        let l = &layers[&id];
        cache.cur.push(LayerSig::of(id, l, now, offset));
    }

    let same_size = checkpoint.width == out.width && checkpoint.height == out.height;
    let common = if cache.prev_offset == offset {
        cache
            .cur
            .iter()
            .zip(cache.prev.iter())
            .take_while(|(a, b)| a.same(b))
            .count()
    } else {
        0
    };
    let start = if same_size && cache.len > 0 && cache.len <= common {
        out.data.copy_from_slice(&checkpoint.data);
        cache.len
    } else {
        out.fill([0, 0, 0, 255]);
        0
    };
    let mut saved = start == common && start > 0;
    for (i, &(_, id)) in cache.order.iter().enumerate().skip(start) {
        if i == common && common > 0 {
            ensure_surface(checkpoint, out.width, out.height);
            checkpoint.data.copy_from_slice(&out.data);
            cache.len = common;
            saved = true;
        }
        draw_layer(out, &layers[&id], now, offset);
    }
    if !saved && common >= cache.order.len() && common > 0 {
        ensure_surface(checkpoint, out.width, out.height);
        checkpoint.data.copy_from_slice(&out.data);
        cache.len = common;
        saved = true;
    }
    if !saved {
        cache.len = 0;
    }
    std::mem::swap(&mut cache.prev, &mut cache.cur);
    cache.prev_offset = offset;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{LayerKind, Page, Scene};
    use crate::transition::{TransMethod, Transition};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Arc<Pixmap> {
        let mut p = Pixmap::new(w, h);
        p.fill(rgba);
        Arc::new(p)
    }

    fn px(out: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * out.width + x) * 4) as usize;
        [
            out.data[i],
            out.data[i + 1],
            out.data[i + 2],
            out.data[i + 3],
        ]
    }

    /// 下層の合成を再利用する描画（`PrefixCache`）は、毎回ゼロから描いた結果と常に画素単位で一致する。
    /// 中間の層の変更・最上位への追加・下位の絵の差し替え・削除・非表示・見える幅の変更を順に行って確かめる。
    #[test]
    fn prefix_cache_matches_a_fresh_compose_after_every_kind_of_change() {
        let (w, h) = (24u32, 16u32);
        let mut scene = Scene::new(w, h);
        let ids: Vec<u32> = (0..6)
            .map(|_| scene.alloc_layer(LayerKind::Image))
            .collect();
        for (k, id) in ids.iter().enumerate() {
            let l = scene.layer_mut(Page::Fore, *id).unwrap();
            l.pixmap = Some(solid(
                10 + k as u32,
                8,
                [40 * k as u8, 200 - 20 * k as u8, 90, 128 + 20 * k as u8],
            ));
            l.x = 2 * k as i32;
            l.y = k as i32;
            l.z = k as i32;
            l.visible = true;
        }
        let mut cached = ComposeBufs::new(w, h);
        let mut out = Pixmap::new(w, h);
        let now = Instant::now();
        let mut check = |scene: &Scene, what: &str| {
            compose_frame(scene, now, &mut cached, &mut out);
            let mut fresh = ComposeBufs::new(w, h);
            let mut want = Pixmap::new(w, h);
            compose_frame(scene, now, &mut fresh, &mut want);
            assert_eq!(out.data, want.data, "{what}");
        };
        check(&scene, "初回");
        check(&scene, "変更なし（チェックポイントの再利用）");
        scene.layer_mut(Page::Fore, ids[3]).unwrap().opacity = 100;
        check(&scene, "中間の層の不透明度");
        scene.layer_mut(Page::Fore, ids[5]).unwrap().x = 9;
        check(&scene, "最上位の移動");
        check(&scene, "変更なし");
        scene.layer_mut(Page::Fore, ids[1]).unwrap().pixmap =
            Some(solid(12, 9, [250, 10, 10, 255]));
        check(&scene, "下位の絵の差し替え");
        let extra = scene.alloc_layer(LayerKind::Image);
        {
            let l = scene.layer_mut(Page::Fore, extra).unwrap();
            l.pixmap = Some(solid(6, 6, [1, 2, 3, 200]));
            l.z = 50;
            l.visible = true;
        }
        check(&scene, "最上位へ追加");
        scene.layer_mut(Page::Fore, ids[2]).unwrap().visible = false;
        check(&scene, "中間を非表示");
        scene.layer_mut(Page::Fore, ids[4]).unwrap().crop_w = Some(5);
        check(&scene, "見える幅");
        scene.layer_mut(Page::Fore, extra).unwrap().visible = false;
        check(&scene, "最上位を消す");
        scene.layer_mut(Page::Fore, ids[0]).unwrap().z = 7;
        check(&scene, "z の入れ替え");
        check(&scene, "最後に変更なし");
    }

    #[test]
    fn z_order_and_visibility() {
        let mut scene = Scene::new(4, 4);
        let bg = scene.alloc_layer(LayerKind::Image);
        let fg = scene.alloc_layer(LayerKind::Image);
        let hidden = scene.alloc_layer(LayerKind::Image);
        {
            let l = scene.layer_mut(Page::Fore, bg).unwrap();
            l.pixmap = Some(solid(4, 4, [255, 0, 0, 255]));
            l.z = 0;
            l.visible = true;
        }
        {
            let l = scene.layer_mut(Page::Fore, fg).unwrap();
            l.pixmap = Some(solid(2, 2, [0, 255, 0, 255]));
            l.z = 10;
            l.visible = true;
        }
        {
            let l = scene.layer_mut(Page::Fore, hidden).unwrap();
            l.pixmap = Some(solid(4, 4, [0, 0, 255, 255]));
            l.z = 20;
            l.visible = false;
        }
        let mut bufs = ComposeBufs::new(4, 4);
        let mut out = Pixmap::new(4, 4);
        compose_frame(&scene, Instant::now(), &mut bufs, &mut out);
        assert_eq!(px(&out, 0, 0), [0, 255, 0, 255]); // 前面の緑
        assert_eq!(px(&out, 3, 3), [255, 0, 0, 255]); // 背景の赤
    }

    #[test]
    fn crossfade_midpoint() {
        let mut scene = Scene::new(2, 2);
        let id = scene.alloc_layer(LayerKind::Image);
        {
            let l = scene.layer_mut(Page::Fore, id).unwrap();
            l.pixmap = Some(solid(2, 2, [0, 0, 0, 255]));
            l.visible = true;
        }
        {
            let l = scene.layer_mut(Page::Back, id).unwrap();
            l.pixmap = Some(solid(2, 2, [255, 255, 255, 255]));
            l.visible = true;
        }
        let start = Instant::now();
        scene.transition = Some(Box::new(Transition {
            method: TransMethod::Crossfade,
            start,
            duration: Duration::from_millis(100),
            skip: false,
        }));
        let mut bufs = ComposeBufs::new(2, 2);
        let mut out = Pixmap::new(2, 2);
        compose_frame(
            &scene,
            start + Duration::from_millis(50),
            &mut bufs,
            &mut out,
        );
        let p = px(&out, 0, 0);
        assert!((p[0] as i32 - 127).abs() <= 2, "mid gray, got {:?}", p);
    }

    #[test]
    fn scroll_transition_midpoint_shows_new_page_entering_from_left() {
        use crate::pixmap::ScrollDir;

        let mut scene = Scene::new(4, 4);
        let id = scene.alloc_layer(LayerKind::Image);
        {
            let l = scene.layer_mut(Page::Fore, id).unwrap();
            l.pixmap = Some(solid(4, 4, [255, 0, 0, 255])); // 旧ページ = 赤
            l.visible = true;
        }
        {
            let l = scene.layer_mut(Page::Back, id).unwrap();
            l.pixmap = Some(solid(4, 4, [0, 0, 255, 255])); // 新ページ = 青
            l.visible = true;
        }
        let start = Instant::now();
        scene.transition = Some(Box::new(Transition {
            method: TransMethod::Scroll {
                from: ScrollDir::Left,
            },
            start,
            duration: Duration::from_millis(100),
            skip: false,
        }));
        let mut bufs = ComposeBufs::new(4, 4);
        let mut out = Pixmap::new(4, 4);
        compose_frame(
            &scene,
            start + Duration::from_millis(50),
            &mut bufs,
            &mut out,
        );
        // t=0.5: 新ページ(青)が左半分を覆い、右半分はまだ旧ページ(赤)のはず。
        assert_eq!(px(&out, 0, 0), [0, 0, 255, 255]);
        assert_eq!(px(&out, 3, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn tween_moves_and_fades_layer_mid_animation() {
        use crate::scene::Tween;

        let mut scene = Scene::new(4, 4);
        let id = scene.alloc_layer(LayerKind::Image);
        let start = Instant::now();
        {
            let l = scene.layer_mut(Page::Fore, id).unwrap();
            l.pixmap = Some(solid(1, 1, [255, 0, 0, 255]));
            l.x = 0;
            l.y = 0;
            l.opacity = 0;
            l.visible = true;
            l.tween = Some(Box::new(Tween {
                from_x: 0,
                from_y: 0,
                from_opacity: 0,
                to_x: 2,
                to_y: 0,
                to_opacity: 255,
                start,
                duration: Duration::from_millis(100),
                accel: 0.0,
                spline: None,
            }));
        }
        let mut bufs = ComposeBufs::new(4, 4);
        let mut out = Pixmap::new(4, 4);

        // 中間点: x=1 付近、opacity 半分程度に見える。
        compose_frame(
            &scene,
            start + Duration::from_millis(50),
            &mut bufs,
            &mut out,
        );
        let mid = px(&out, 1, 0);
        assert!(
            mid[0] > 60 && mid[0] < 200,
            "中間点の赤成分が半透明程度のはず, got {:?}",
            mid
        );

        // 完了後: to 値 (x=2, opacity=255) で不透明に描画される。
        compose_frame(
            &scene,
            start + Duration::from_millis(200),
            &mut bufs,
            &mut out,
        );
        assert_eq!(px(&out, 2, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn opacity_blend() {
        let mut scene = Scene::new(1, 1);
        let bg = scene.alloc_layer(LayerKind::Image);
        {
            let l = scene.layer_mut(Page::Fore, bg).unwrap();
            l.pixmap = Some(solid(1, 1, [0, 0, 255, 255]));
            l.visible = true;
        }
        let fg = scene.alloc_layer(LayerKind::Image);
        {
            let l = scene.layer_mut(Page::Fore, fg).unwrap();
            l.pixmap = Some(solid(1, 1, [255, 0, 0, 255]));
            l.z = 1;
            l.opacity = 128;
            l.visible = true;
        }
        let mut bufs = ComposeBufs::new(1, 1);
        let mut out = Pixmap::new(1, 1);
        compose_frame(&scene, Instant::now(), &mut bufs, &mut out);
        let p = px(&out, 0, 0);
        assert!((p[0] as i32 - 128).abs() <= 2);
        assert!((p[2] as i32 - 127).abs() <= 2);
    }
}
