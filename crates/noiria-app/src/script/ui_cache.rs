//! デコード済み画像・UI 描画結果のキャッシュ（通常画像・パネル・サムネイル・縮小画像）。
//! 同じ背景・立ち絵・パネルを毎回ディスク読込・PNG 復号・再描画しないためのもの。値は
//! `Arc<Pixmap>` なので、ヒットしたレイヤは画素を複製せず共有する（レイヤ側も `Arc<Pixmap>` を持つ）。
//!
//! 無効化: 通常画像・縮小画像はゲーム資産が実行中に変わらない前提でパス等を鍵にする。
//! パネルは「仕様の数値列そのもの」が鍵なので古くならない。サムネイルは鍵にファイルの
//! 更新時刻と長さを含め、`thumb_save` / `save_delete` では明示的にスロットごと捨てる（同一
//! タイムスタンプ内の上書きでも古い絵を出さない）。

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use noiria_core::assets::GameDir;
use noiria_core::pixmap::{checked_dims, Pixmap};
use noiria_core::savedata;

use crate::bridge::LockExt;

/// ゲーム資産の画像を読み、デコード後の寸法を `checked_dims` で検査して返す
/// （巨大な寸法の画像でレイヤ・縮小・合成が確保に失敗して abort するのを防ぐ）。
pub fn load_image_checked(game_dir: &GameDir, path: &str) -> Result<Pixmap, String> {
    let pixmap = game_dir.load_image(path).map_err(|e| e.to_string())?;
    checked_dims(pixmap.width, pixmap.height).map_err(|e| format!("{path}: {e}"))?;
    Ok(pixmap)
}

/// 総バイト数で上限を持つ LRU（エントリ数は数十個なので、追い出しは最古の走査で足りる）。
pub struct PixmapLru<K: Eq + Hash + Clone> {
    map: HashMap<K, (Arc<Pixmap>, u64)>,
    bytes: usize,
    cap_bytes: usize,
    tick: u64,
}

fn size_of(p: &Pixmap) -> usize {
    p.data.len()
}

impl<K: Eq + Hash + Clone> PixmapLru<K> {
    pub fn new(cap_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            bytes: 0,
            cap_bytes,
            tick: 0,
        }
    }

    pub fn get<Q>(&mut self, k: &Q) -> Option<Arc<Pixmap>>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(k).map(|(p, t)| {
            *t = tick;
            p.clone()
        })
    }

    pub fn insert(&mut self, k: K, v: Arc<Pixmap>) {
        let sz = size_of(&v);
        if let Some((old, _)) = self.map.remove(&k) {
            self.bytes -= size_of(&old);
        }
        if sz > self.cap_bytes {
            return;
        }
        self.tick += 1;
        self.map.insert(k, (v, self.tick));
        self.bytes += sz;
        while self.bytes > self.cap_bytes {
            let Some(oldest) = self.map.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((p, _)) = self.map.remove(&oldest) {
                self.bytes -= size_of(&p);
            }
        }
    }

    pub fn remove_where(&mut self, f: impl Fn(&K) -> bool) {
        let doomed: Vec<K> = self.map.keys().filter(|k| f(k)).cloned().collect();
        for k in doomed {
            if let Some((p, _)) = self.map.remove(&k) {
                self.bytes -= size_of(&p);
            }
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

/// `layer.panel` の数値列（`PanelSpec::from_slice` が読むもの）を鍵にする。
pub type PanelKey = Vec<u64>;

pub fn panel_key(spec: &[f64]) -> PanelKey {
    spec.iter().map(|v| v.to_bits()).collect()
}

/// サムネイルの鍵: スロット名・表示寸法（0 = 規定寸法）・ファイルの更新時刻(ns)と長さ。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ThumbKey {
    pub slot: String,
    pub w: u32,
    pub h: u32,
    pub mtime_ns: u128,
    pub len: u64,
}

/// 縮小済み画像（ギャラリーのサムネイル）の鍵: パス・寸法。ゲーム資産は実行中に変わらない。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ScaledKey {
    pub path: String,
    pub w: u32,
    pub h: u32,
}

/// ユニバーサルトランジションのルール輝度プレーン用 LRU（バイト上限付き）。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RuleKey {
    pub path: String,
    pub w: u32,
    pub h: u32,
}

pub struct RuleLru {
    map: HashMap<RuleKey, (Arc<Vec<u8>>, u64)>,
    bytes: usize,
    cap_bytes: usize,
    tick: u64,
}

impl RuleLru {
    pub fn new(cap_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            bytes: 0,
            cap_bytes,
            tick: 0,
        }
    }

    pub fn get(&mut self, k: &RuleKey) -> Option<Arc<Vec<u8>>> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(k).map(|(p, t)| {
            *t = tick;
            p.clone()
        })
    }

    pub fn insert(&mut self, k: RuleKey, v: Arc<Vec<u8>>) {
        let sz = v.len();
        if let Some((old, _)) = self.map.remove(&k) {
            self.bytes -= old.len();
        }
        if sz > self.cap_bytes {
            return;
        }
        self.tick += 1;
        self.map.insert(k, (v, self.tick));
        self.bytes += sz;
        while self.bytes > self.cap_bytes {
            let Some(oldest) = self.map.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((p, _)) = self.map.remove(&oldest) {
                self.bytes -= p.len();
            }
        }
    }
}

pub const IMAGE_CACHE_MAX_BYTES: usize = 64 << 20;

pub struct UiCaches {
    pub panels: PixmapLru<PanelKey>,
    pub thumbs: PixmapLru<ThumbKey>,
    pub scaled: PixmapLru<ScaledKey>,
    pub images: PixmapLru<String>,
    pub rules: RuleLru,
}

impl UiCaches {
    pub fn new() -> Self {
        Self {
            panels: PixmapLru::new(64 << 20),
            thumbs: PixmapLru::new(16 << 20),
            scaled: PixmapLru::new(24 << 20),
            images: PixmapLru::new(IMAGE_CACHE_MAX_BYTES),
            rules: RuleLru::new(16 << 20),
        }
    }

    pub fn forget_slot(&mut self, slot: &str) {
        self.thumbs.remove_where(|k| k.slot == slot);
    }

    /// 縮小画像のキャッシュに `w`x`h` の画像が何枚入るか。これを超えて先読みしても古いものを
    /// 追い出すだけ（かつ先読みのたびに再デコードになる）ので、先読みはこの枚数で打ち切る。
    pub fn scaled_capacity(&self, w: u32, h: u32) -> usize {
        let one = (w as usize).saturating_mul(h as usize).saturating_mul(4).max(1);
        self.scaled.cap_bytes / one
    }
}

fn thumb_key(game_id: &str, slot: &str, w: u32, h: u32) -> Option<ThumbKey> {
    let meta = std::fs::metadata(savedata::thumb_path(game_id, slot).ok()?).ok()?;
    Some(ThumbKey {
        slot: slot.to_string(),
        w,
        h,
        mtime_ns: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos()),
        len: meta.len(),
    })
}

/// セーブ領域のサムネイルを復号して返す（キャッシュにあればそれを共有）。w/h が 0 以外なら
/// ちょうどその寸法へ面積平均で拡縮、0 なら規定寸法に収める従来動作。無い/壊れていれば Err。
pub fn load_thumb_cached(
    cache: &Mutex<UiCaches>,
    game_id: &str,
    slot: &str,
    w: u32,
    h: u32,
) -> Result<Arc<Pixmap>, String> {
    let sized = w != 0 && h != 0;
    let key = thumb_key(game_id, slot, w, h);
    if let Some(p) = key.as_ref().and_then(|k| cache.lock_unpoisoned().thumbs.get(k)) {
        return Ok(p);
    }
    let (max_w, max_h) = if sized {
        (4096, 4096)
    } else {
        (savedata::THUMB_W, savedata::THUMB_H)
    };
    let mut pixmap = savedata::read_thumb(game_id, slot, max_w, max_h)
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| format!("サムネイルがありません: {slot}"))?;
    if sized && (pixmap.width != w || pixmap.height != h) {
        checked_dims(w, h)?;
        pixmap = pixmap.resize_area(w, h);
    }
    let pixmap = Arc::new(pixmap);
    if let Some(k) = key {
        cache.lock_unpoisoned().thumbs.insert(k, pixmap.clone());
    }
    Ok(pixmap)
}

/// ゲーム資産の画像をデコードして返す（キャッシュにあれば `Arc<Pixmap>` を共有）。
pub fn load_image_cached(
    cache: &Mutex<UiCaches>,
    game_dir: &GameDir,
    path: &str,
) -> Result<Arc<Pixmap>, String> {
    if let Some(p) = cache.lock_unpoisoned().images.get(path) {
        return Ok(p);
    }
    let pixmap = Arc::new(load_image_checked(game_dir, path)?);
    cache
        .lock_unpoisoned()
        .images
        .insert(path.to_string(), pixmap.clone());
    Ok(pixmap)
}

/// ゲーム資産の画像を w x h へ面積平均で縮めて返す（キャッシュにあればそれを共有）。
pub fn load_scaled_cached(
    cache: &Mutex<UiCaches>,
    game_dir: &GameDir,
    path: &str,
    w: u32,
    h: u32,
) -> Result<Arc<Pixmap>, String> {
    let key = ScaledKey {
        path: path.to_string(),
        w,
        h,
    };
    let existing_full = {
        let mut guard = cache.lock_unpoisoned();
        if let Some(p) = guard.scaled.get(&key) {
            return Ok(p);
        }
        guard.images.get(path)
    };
    checked_dims(w, h)?;
    let pixmap = if let Some(full) = existing_full {
        if full.width == w && full.height == h {
            full
        } else {
            Arc::new(full.resize_area(w, h))
        }
    } else {
        let decoded = load_image_checked(game_dir, path)?;
        let resized = if decoded.width == w && decoded.height == h {
            decoded
        } else {
            decoded.resize_area(w, h)
        };
        Arc::new(resized)
    };
    cache.lock_unpoisoned().scaled.insert(key, pixmap.clone());
    Ok(pixmap)
}

/// ユニバーサルトランジションのルール画像をスクリーン寸法の輝度プレーンとしてキャッシュする。
pub fn load_rule_cached(
    cache: &Mutex<UiCaches>,
    game_dir: &GameDir,
    path: &str,
    scene_w: u32,
    scene_h: u32,
) -> Result<Arc<Vec<u8>>, String> {
    let key = RuleKey {
        path: path.to_string(),
        w: scene_w,
        h: scene_h,
    };
    if let Some(plane) = cache.lock_unpoisoned().rules.get(&key) {
        return Ok(plane);
    }
    let (w, h, rgba) = game_dir.load_image_rgba(path).map_err(|e| e.to_string())?;
    checked_dims(w, h).map_err(|e| format!("{path}: {e}"))?;
    let plane = Arc::new(noiria_core::transition::rule_plane_from_rgba(
        w, h, &rgba, scene_w, scene_h,
    ));
    cache.lock_unpoisoned().rules.insert(key, plane.clone());
    Ok(plane)
}

enum Job {
    Scaled { paths: Vec<String>, w: u32, h: u32 },
    Thumbs { w: u32, h: u32 },
}

struct Worker {
    tx: Sender<Job>,
    handle: JoinHandle<()>,
}

/// サムネイル・縮小画像の先読みを 1 本のワーカースレッドとキューで行う（以前は呼び出しごとに
/// デタッチスレッドを作っていたので、メニューを開閉するたびにスレッドが積み上がり、終了時にも
/// 止められなかった）。ワーカーは最初の依頼で起動し、drop で停止（実行中の 1 枚が終わるまで待って join）。
pub struct Prefetcher {
    cache: Arc<Mutex<UiCaches>>,
    game_dir: GameDir,
    game_id: String,
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<Worker>>,
    #[cfg(test)]
    spawned: std::sync::atomic::AtomicUsize,
}

impl Prefetcher {
    pub fn new(cache: Arc<Mutex<UiCaches>>, game_dir: GameDir, game_id: String) -> Self {
        Self {
            cache,
            game_dir,
            game_id,
            stop: Arc::new(AtomicBool::new(false)),
            worker: Mutex::new(None),
            #[cfg(test)]
            spawned: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// ゲーム資産の画像を w x h の縮小サムネイルにしておく。キャッシュに入りきらない枚数は先読みしない。
    pub fn scaled(&self, mut paths: Vec<String>, w: u32, h: u32) {
        let capacity = self.cache.lock_unpoisoned().scaled_capacity(w, h);
        paths.truncate(capacity);
        self.submit(Job::Scaled { paths, w, h });
    }

    /// 一覧に出るスロット（`auto` / `quick` / `saveN`）の PNG を先読みする。
    pub fn thumbs(&self, w: u32, h: u32) {
        self.submit(Job::Thumbs { w, h });
    }

    fn submit(&self, job: Job) {
        let mut guard = self.worker.lock_unpoisoned();
        if guard.is_none() {
            let (tx, rx) = channel::<Job>();
            let cache = self.cache.clone();
            let game_dir = self.game_dir.clone();
            let game_id = self.game_id.clone();
            let stop = self.stop.clone();
            let spawned = std::thread::Builder::new()
                .name("noiria-prefetch".into())
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            run_job(&cache, &game_dir, &game_id, &stop, job);
                        }));
                    }
                });
            let Ok(handle) = spawned else { return };
            #[cfg(test)]
            self.spawned.fetch_add(1, Ordering::SeqCst);
            *guard = Some(Worker { tx, handle });
        }
        if let Some(w) = guard.as_ref() {
            let _ = w.tx.send(job);
        }
    }
}

impl Drop for Prefetcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.get_mut().unwrap_or_else(|e| e.into_inner()).take() {
            drop(w.tx);
            let _ = w.handle.join();
        }
    }
}

fn run_job(cache: &Mutex<UiCaches>, game_dir: &GameDir, game_id: &str, stop: &AtomicBool, job: Job) {
    match job {
        Job::Scaled { paths, w, h } => {
            for p in paths {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let _ = load_scaled_cached(cache, game_dir, &p, w, h);
            }
        }
        Job::Thumbs { w, h } => {
            let Ok(dir) = savedata::save_dir(game_id) else {
                return;
            };
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            let mut slots: Vec<String> = entries
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    if p.extension().and_then(|x| x.to_str()) != Some("png") {
                        return None;
                    }
                    let stem = p.file_stem()?.to_str()?.to_string();
                    let numbered = stem
                        .strip_prefix("save")
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
                    (stem == "auto" || stem == "quick" || numbered).then_some(stem)
                })
                .collect();
            slots.sort();
            for slot in slots {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let _ = load_thumb_cached(cache, game_id, &slot, w, h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(w: u32, h: u32) -> Arc<Pixmap> {
        Arc::new(Pixmap::new(w, h))
    }

    #[test]
    fn evicts_least_recently_used_over_the_byte_cap() {
        let mut c: PixmapLru<u32> = PixmapLru::new(3 * 16 * 4);
        c.insert(1, px(4, 4));
        c.insert(2, px(4, 4));
        c.insert(3, px(4, 4));
        assert_eq!(c.len(), 3);
        // 1 を触って最新にしてから 4 を入れると、最古の 2 が落ちる
        assert!(c.get(&1).is_some());
        c.insert(4, px(4, 4));
        assert_eq!(c.len(), 3);
        assert!(c.get(&2).is_none());
        assert!(c.get(&1).is_some() && c.get(&3).is_some() && c.get(&4).is_some());
    }

    #[test]
    fn oversized_entry_is_not_stored_and_replacing_keeps_the_byte_count() {
        let mut c: PixmapLru<u32> = PixmapLru::new(64);
        c.insert(1, px(100, 100));
        assert_eq!(c.len(), 0);
        c.insert(2, px(4, 4));
        c.insert(2, px(4, 4));
        c.insert(3, px(4, 4));
        c.insert(4, px(4, 4));
        assert_eq!(c.len(), 1, "64 バイト上限に 64 バイトのものは 1 つだけ入る");
    }

    #[test]
    fn forget_slot_drops_every_size_of_that_slot_only() {
        let mut c = UiCaches::new();
        let key = |slot: &str, w: u32| ThumbKey {
            slot: slot.to_string(),
            w,
            h: 10,
            mtime_ns: 1,
            len: 2,
        };
        c.thumbs.insert(key("save1", 336), px(2, 2));
        c.thumbs.insert(key("save1", 0), px(2, 2));
        c.thumbs.insert(key("save2", 336), px(2, 2));
        c.forget_slot("save1");
        assert!(c.thumbs.get(&key("save1", 336)).is_none());
        assert!(c.thumbs.get(&key("save1", 0)).is_none());
        assert!(c.thumbs.get(&key("save2", 336)).is_some());
    }

    fn prefetch_game(name: &str, images: usize) -> (GameDir, std::path::PathBuf) {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("noiria-{name}-{ns}"));
        std::fs::create_dir_all(dir.join("img")).unwrap();
        std::fs::write(
            dir.join("game.json"),
            format!(r#"{{"title":"t","id":"noiria-{name}","main":"main.ts"}}"#),
        )
        .unwrap();
        let mut p = Pixmap::new(8, 8);
        p.fill([10, 20, 30, 255]);
        for i in 0..images {
            noiria_core::assets::save_png(&p, &dir.join(format!("img/{i}.png"))).unwrap();
        }
        let (game_dir, _) = GameDir::open(&dir).unwrap();
        (game_dir, dir)
    }

    fn wait_until(limit: std::time::Duration, f: impl Fn() -> bool) -> bool {
        let t = std::time::Instant::now();
        while t.elapsed() < limit {
            if f() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        f()
    }

    /// 先読みの依頼は何度出しても 1 本のワーカーが順に処理する（以前は依頼ごとにデタッチスレッドを作った）。
    #[test]
    fn prefetch_requests_share_one_worker_and_fill_the_cache() {
        let (game_dir, dir) = prefetch_game("prefetch-one-worker", 3);
        let cache = Arc::new(Mutex::new(UiCaches::new()));
        let pf = Prefetcher::new(cache.clone(), game_dir, "noiria-prefetch-one-worker".into());
        let paths: Vec<String> = (0..3).map(|i| format!("img/{i}.png")).collect();
        for _ in 0..50 {
            pf.scaled(paths.clone(), 4, 4);
        }
        assert_eq!(pf.spawned.load(Ordering::SeqCst), 1);
        assert!(wait_until(std::time::Duration::from_secs(10), || {
            cache.lock_unpoisoned().scaled.len() == 3
        }));
        drop(pf);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// drop でワーカーが止まる（join される）。残りの先読みは捨てられ、それ以降キャッシュは増えない。
    #[test]
    fn dropping_the_prefetcher_stops_and_joins_the_worker() {
        let (game_dir, dir) = prefetch_game("prefetch-stop", 40);
        let cache = Arc::new(Mutex::new(UiCaches::new()));
        let pf = Prefetcher::new(cache.clone(), game_dir, "noiria-prefetch-stop".into());
        let paths: Vec<String> = (0..40).map(|i| format!("img/{i}.png")).collect();
        pf.scaled(paths, 4, 4);
        drop(pf);
        let after_drop = cache.lock_unpoisoned().scaled.len();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(cache.lock_unpoisoned().scaled.len(), after_drop, "drop 後も先読みが続いている");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// キャッシュに入りきらない枚数は先読みしない（入れた端から追い出し合って、毎回再デコードになるため）。
    #[test]
    fn scaled_prefetch_is_capped_to_what_the_cache_can_hold() {
        let caches = UiCaches::new();
        // 336x189x4 = 254,016 バイト。24MiB の上限には 98 枚入る。
        assert_eq!(caches.scaled_capacity(336, 189), (24 << 20) / (336 * 189 * 4));
        assert!(caches.scaled_capacity(336, 189) >= 90);
    }

    #[test]
    fn panel_key_distinguishes_specs() {
        assert_eq!(panel_key(&[1.0, 2.0]), panel_key(&[1.0, 2.0]));
        assert_ne!(panel_key(&[1.0, 2.0]), panel_key(&[1.0, 2.5]));
    }

    #[test]
    fn load_image_cached_hits_cache_and_shares_arc() {
        let (game_dir, dir) = prefetch_game("image-cache-hit", 1);
        let cache = Mutex::new(UiCaches::new());

        let first = load_image_cached(&cache, &game_dir, "img/0.png").unwrap();
        std::fs::remove_file(dir.join("img/0.png")).unwrap();
        let second = load_image_cached(&cache, &game_dir, "img/0.png").unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.lock_unpoisoned().images.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn images_cache_evicts_least_recently_used_over_byte_cap() {
        let (game_dir, dir) = prefetch_game("image-cache-evict", 3);
        let mut ui = UiCaches::new();
        ui.images = PixmapLru::new(2 * 8 * 8 * 4);
        let cache = Mutex::new(ui);

        let a = load_image_cached(&cache, &game_dir, "img/0.png").unwrap();
        let _b = load_image_cached(&cache, &game_dir, "img/1.png").unwrap();
        let a2 = load_image_cached(&cache, &game_dir, "img/0.png").unwrap();
        assert!(Arc::ptr_eq(&a, &a2));

        let _c = load_image_cached(&cache, &game_dir, "img/2.png").unwrap();
        let mut guard = cache.lock_unpoisoned();
        assert_eq!(guard.images.len(), 2);
        assert_eq!(guard.images.bytes(), 2 * 8 * 8 * 4);
        assert!(guard.images.get("img/1.png").is_none());
        assert!(guard.images.get("img/0.png").is_some());
        assert!(guard.images.get("img/2.png").is_some());
        drop(guard);
        std::fs::remove_dir_all(&dir).ok();
    }
}
