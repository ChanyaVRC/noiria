//! セーブデータの読み書き。中身の構造はスクリプト側 (TS) が決める JSON 文字列で、
//! ネイティブ側は保存場所の解決と入出力だけを担当する。

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;

static SAVE_DIR_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// セーブディレクトリを丸ごと差し替える（`--save-dir`）。自動検証が実ユーザーのセーブ
/// （sf.* の到達済みエンド等を含む）を汚さないための隔離用。プロセスで 1 回だけ設定できる。
pub fn set_save_dir_override(dir: PathBuf) {
    let _ = SAVE_DIR_OVERRIDE.set(dir);
}

fn override_dir() -> Option<PathBuf> {
    SAVE_DIR_OVERRIDE.get().cloned().or_else(|| {
        std::env::var_os("NOIRIA_SAVE_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    })
}

/// %APPDATA%\Noiria\<game_id>\save\（`--save-dir` / 環境変数 `NOIRIA_SAVE_DIR` 指定時はそのディレクトリ）
pub fn save_dir(game_id: &str) -> Result<PathBuf> {
    if let Some(dir) = override_dir() {
        fs::create_dir_all(&dir)?;
        return Ok(dir);
    }
    let dirs = directories::ProjectDirs::from("", "Noiria", game_id)
        .context("セーブディレクトリを解決できません")?;
    let dir = dirs.data_dir().join("save");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn check_slot_name(slot: &str) -> Result<()> {
    anyhow::ensure!(
        !slot.is_empty()
            && slot
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "不正なスロット名: {slot}"
    );
    Ok(())
}

fn slot_path(game_id: &str, slot: &str) -> Result<PathBuf> {
    check_slot_name(slot)?;
    Ok(save_dir(game_id)?.join(format!("{slot}.json")))
}

/// 一時ファイルへ書いてから rename で差し替える（書き込み中のクラッシュ/電源断で
/// 既存セーブを半端なファイルに置き換えないため。同一ディレクトリ内の rename は
/// Windows/Linux とも既存ファイルを原子的に置換する）。
pub fn write_slot(game_id: &str, slot: &str, json: &str) -> Result<()> {
    use std::io::Write;
    let path = slot_path(game_id, slot)?;
    let tmp = path.with_extension("json.tmp");
    let write_tmp = || -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()
    };
    if let Err(e) = write_tmp() {
        let _ = fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("セーブに失敗: {}", path.display()));
    }
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        anyhow::Error::new(e).context(format!("セーブに失敗: {}", path.display()))
    })
}

/// 中身が（UTF-8 / JSON として）壊れていれば `<slot>.json.bak` へ退避して `None` を返す（黙って
/// 捨てると次の保存で上書きされ原因調査もできなくなるため）。退避先に既存の `.bak` があれば上書きする。
/// 退避するのは「読めたが壊れていた」場合だけ: 共有違反・権限エラーなど読めなかっただけで正常な
/// セーブを退避すると、セーブを失い、以前の `.bak` も潰してしまうので `Err` で返す。
pub fn read_slot(game_id: &str, slot: &str) -> Result<Option<String>> {
    let path = slot_path(game_id, slot)?;
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("セーブを読めません: {}", path.display()))
        }
    };
    let valid = String::from_utf8(bytes)
        .ok()
        .filter(|t| serde_json::from_str::<serde::de::IgnoredAny>(t).is_ok());
    if let Some(text) = valid {
        return Ok(Some(text));
    }
    let bak = path.with_extension("json.bak");
    match fs::rename(&path, &bak) {
        Ok(()) => log::error!(
            "セーブ '{slot}' が壊れていたため {} へ退避しました（読み込みは空として続行）",
            bak.display()
        ),
        Err(e) => log::error!(
            "セーブ '{slot}' が壊れていますが退避にも失敗しました ({}): {e}",
            path.display()
        ),
    }
    Ok(None)
}

/// スロットを真に削除する（[erasebookmark] の KAG3 準拠実装、W7）。存在しなければ no-op。
/// サムネイル PNG（あれば）も合わせて削除する（失敗しても無視する: 削除自体は成功扱い）。
pub fn delete_slot(game_id: &str, slot: &str) -> Result<()> {
    let path = slot_path(game_id, slot)?;
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("削除に失敗: {}", path.display()))?;
    }
    if let Ok(thumb) = thumb_path(game_id, slot) {
        let _ = fs::remove_file(thumb);
    }
    Ok(())
}

#[derive(Serialize)]
pub struct SlotInfo {
    pub slot: String,
    pub json: String,
}

/// 全スロットの中身を返す（セーブファイルは小さい前提）。
pub fn list_slots(game_id: &str) -> Result<Vec<SlotInfo>> {
    let dir = save_dir(game_id)?;
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(slot) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // 読めない（非 UTF-8 等）スロットが 1 つあっても一覧全体を失敗させない。
        let Ok(json) = fs::read_to_string(&path) else {
            continue;
        };
        out.push(SlotInfo {
            slot: slot.to_string(),
            json,
        });
    }
    out.sort_by(|a, b| a.slot.cmp(&b.slot));
    Ok(out)
}

/// サムネイルの規定寸法（セーブ一覧の 1 枚ぶん。保存時にこの大きさへ縮小する）。
pub const THUMB_W: u32 = 240;
pub const THUMB_H: u32 = 135;

/// サムネイル PNG（`<slot>.png`）を書く。スロット名は `slot_path` と同じ検証を通す
/// （パストラバーサル不可）。
///
/// `write_slot` と同じく一時ファイルへ書いてから rename で差し替える（先読みスレッドが読んでいる最中の
/// 上書きで、読み手が半端な PNG を見ないように）。
pub fn write_thumb(game_id: &str, slot: &str, thumb: &crate::pixmap::Pixmap) -> Result<()> {
    let path = thumb_path(game_id, slot)?;
    let tmp = path.with_extension("png.tmp");
    if let Err(e) = crate::assets::save_png(thumb, &tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        anyhow::Error::new(e).context(format!("サムネイル保存に失敗: {}", path.display()))
    })
}

/// サムネイルを読み、`max_w`x`max_h` に収まらなければアスペクト比を保って縮小して返す
/// （旧版が書いた画面サイズそのままの PNG もこの大きさで表示できるように）。
/// ファイルが無ければ `None`。スロット名は検証する。
pub fn read_thumb(
    game_id: &str,
    slot: &str,
    max_w: u32,
    max_h: u32,
) -> Result<Option<crate::pixmap::Pixmap>> {
    check_slot_name(slot)?;
    let path = thumb_path(game_id, slot)?;
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("サムネイルを読めません: {}", path.display()))?;
    let png = crate::codec::png::decode(&bytes)
        .with_context(|| format!("サムネイルの PNG が不正です: {}", path.display()))?;
    let mut pixmap = crate::pixmap::Pixmap::from_rgba_straight(png.width, png.height, &png.rgba);
    if pixmap.width > max_w || pixmap.height > max_h {
        let scale = (max_w as f64 / pixmap.width as f64).min(max_h as f64 / pixmap.height as f64);
        let w = ((pixmap.width as f64 * scale).round() as u32).max(1);
        let h = ((pixmap.height as f64 * scale).round() as u32).max(1);
        pixmap = pixmap.resize_area(w, h);
    }
    Ok(Some(pixmap))
}

/// スクリーンショットサムネイルの保存先。スロット名は `slot_path` と同じ検証を通す（スクリプトの
/// 文字列がそのまま渡るので、`../../x` や絶対パスでセーブ領域の外へ書かせない）。
pub fn thumb_path(game_id: &str, slot: &str) -> Result<PathBuf> {
    check_slot_name(slot)?;
    Ok(save_dir(game_id)?.join(format!("{slot}.png")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ID: &str = "noiria-test-savedata";

    fn cleanup() {
        if let Ok(dir) = save_dir(TEST_ID) {
            let _ = fs::remove_dir_all(dir.parent().unwrap_or(&dir));
        }
    }

    #[test]
    fn roundtrip_and_list() {
        cleanup();
        assert_eq!(read_slot(TEST_ID, "slot1").unwrap(), None);

        write_slot(TEST_ID, "slot1", r#"{"pos":1}"#).unwrap();
        write_slot(TEST_ID, "slot2", r#"{"pos":2}"#).unwrap();
        assert_eq!(
            read_slot(TEST_ID, "slot1").unwrap().unwrap(),
            r#"{"pos":1}"#
        );

        // 上書き
        write_slot(TEST_ID, "slot1", r#"{"pos":9}"#).unwrap();
        assert_eq!(
            read_slot(TEST_ID, "slot1").unwrap().unwrap(),
            r#"{"pos":9}"#
        );

        let slots = list_slots(TEST_ID).unwrap();
        let names: Vec<&str> = slots.iter().map(|s| s.slot.as_str()).collect();
        assert_eq!(names, vec!["slot1", "slot2"]);
        cleanup();
    }

    #[test]
    fn rejects_bad_slot_names() {
        assert!(write_slot(TEST_ID, "../evil", "{}").is_err());
        assert!(write_slot(TEST_ID, "a/b", "{}").is_err());
        assert!(write_slot(TEST_ID, "a\\b", "{}").is_err());
        assert!(read_slot(TEST_ID, "..").is_err());
    }

    /// [erasebookmark]（W7）が使う真の削除。存在しないスロットの削除は no-op で、
    /// 削除後は list_slots からも消える（＝セーブメニューが「空きスロット」表示に戻る）。
    /// `TEST_ID` を他のテストと共有すると並列実行時にディレクトリ操作が競合するため、
    /// このテスト専用の game_id を使う。
    #[test]
    fn deletes_slot_and_missing_is_a_noop() {
        const DELETE_TEST_ID: &str = "noiria-test-savedata-delete";
        let cleanup_delete = || {
            if let Ok(dir) = save_dir(DELETE_TEST_ID) {
                let _ = fs::remove_dir_all(dir.parent().unwrap_or(&dir));
            }
        };
        cleanup_delete();
        // 存在しないスロットを消しても成功する（no-op）。
        assert!(delete_slot(DELETE_TEST_ID, "ghost").is_ok());

        write_slot(DELETE_TEST_ID, "slot1", r#"{"pos":1}"#).unwrap();
        write_slot(DELETE_TEST_ID, "slot2", r#"{"pos":2}"#).unwrap();
        assert_eq!(
            read_slot(DELETE_TEST_ID, "slot1").unwrap().unwrap(),
            r#"{"pos":1}"#
        );

        delete_slot(DELETE_TEST_ID, "slot1").unwrap();
        assert_eq!(
            read_slot(DELETE_TEST_ID, "slot1").unwrap(),
            None,
            "削除後は読めない"
        );
        let remaining = list_slots(DELETE_TEST_ID).unwrap();
        let names: Vec<&str> = remaining.iter().map(|s| s.slot.as_str()).collect();
        assert_eq!(
            names,
            vec!["slot2"],
            "削除済みスロットは list_slots からも消える"
        );

        // 二重削除も no-op。
        assert!(delete_slot(DELETE_TEST_ID, "slot1").is_ok());
        cleanup_delete();
    }

    #[test]
    fn write_leaves_no_temp_file_and_overwrites_atomically() {
        const ID: &str = "noiria-test-savedata-atomic";
        let cleanup_atomic = || {
            if let Ok(dir) = save_dir(ID) {
                let _ = fs::remove_dir_all(dir.parent().unwrap_or(&dir));
            }
        };
        cleanup_atomic();
        write_slot(ID, "sys", r#"{"a":1}"#).unwrap();
        write_slot(ID, "sys", r#"{"a":2}"#).unwrap();
        assert_eq!(read_slot(ID, "sys").unwrap().unwrap(), r#"{"a":2}"#);
        let dir = save_dir(ID).unwrap();
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["sys.json".to_string()], "一時ファイルが残っている");
        cleanup_atomic();
    }

    /// 書き込み途中で落ちて残った一時ファイルは、一覧に混ざらず次回保存で上書きされる。
    #[test]
    fn stale_temp_file_is_ignored_and_replaced() {
        const ID: &str = "noiria-test-savedata-stale-tmp";
        let cleanup_stale = || {
            if let Ok(dir) = save_dir(ID) {
                let _ = fs::remove_dir_all(dir.parent().unwrap_or(&dir));
            }
        };
        cleanup_stale();
        write_slot(ID, "slot1", r#"{"ok":true}"#).unwrap();
        let dir = save_dir(ID).unwrap();
        fs::write(dir.join("slot1.json.tmp"), "{\"trunc").unwrap();
        assert_eq!(list_slots(ID).unwrap().len(), 1);
        write_slot(ID, "slot1", r#"{"ok":2}"#).unwrap();
        assert_eq!(read_slot(ID, "slot1").unwrap().unwrap(), r#"{"ok":2}"#);
        assert!(!dir.join("slot1.json.tmp").exists());
        cleanup_stale();
    }

    /// 壊れた JSON は黙って捨てず `.bak` へ退避する（元のファイルは消え、以後は空扱い）。
    #[test]
    fn corrupt_slot_is_moved_to_bak_not_discarded() {
        const ID: &str = "noiria-test-savedata-corrupt";
        let cleanup_corrupt = || {
            if let Ok(dir) = save_dir(ID) {
                let _ = fs::remove_dir_all(dir.parent().unwrap_or(&dir));
            }
        };
        cleanup_corrupt();
        let dir = save_dir(ID).unwrap();
        fs::write(dir.join("sys.json"), "{\"readLabels\": {\"a*b\": tr").unwrap();
        assert_eq!(read_slot(ID, "sys").unwrap(), None);
        assert!(!dir.join("sys.json").exists());
        assert_eq!(
            fs::read_to_string(dir.join("sys.json.bak")).unwrap(),
            "{\"readLabels\": {\"a*b\": tr",
            "壊れた中身が .bak に保全されている"
        );
        // 退避後の書き込み・読み出しは通常どおり。
        write_slot(ID, "sys", "{}").unwrap();
        assert_eq!(read_slot(ID, "sys").unwrap().unwrap(), "{}");
        assert!(dir.join("sys.json.bak").exists(), "新しい保存で .bak は消えない");
        cleanup_corrupt();
    }

    #[test]
    fn non_utf8_slot_is_quarantined_and_list_survives() {
        const ID: &str = "noiria-test-savedata-nonutf8";
        let cleanup_nonutf8 = || {
            if let Ok(dir) = save_dir(ID) {
                let _ = fs::remove_dir_all(dir.parent().unwrap_or(&dir));
            }
        };
        cleanup_nonutf8();
        let dir = save_dir(ID).unwrap();
        fs::write(dir.join("bad.json"), [0xff, 0xfe, 0x00]).unwrap();
        write_slot(ID, "good", "{}").unwrap();
        let names: Vec<String> = list_slots(ID).unwrap().into_iter().map(|s| s.slot).collect();
        assert_eq!(names, vec!["good".to_string()]);
        assert_eq!(read_slot(ID, "bad").unwrap(), None);
        assert!(dir.join("bad.json.bak").exists());
        cleanup_nonutf8();
    }

    fn thumb_test_dir(id: &str) {
        if let Ok(dir) = save_dir(id) {
            let _ = fs::remove_dir_all(dir.parent().unwrap_or(&dir));
        }
    }

    /// 書いたサムネイルを別実装のデコーダ（image クレート）で読んで画素を確かめ、自作デコーダ経由の
    /// `read_thumb` とも一致させる（エンコーダ/デコーダの共犯バグを避ける）。
    #[test]
    fn thumb_roundtrip_is_cross_checked_with_image_crate() {
        const ID: &str = "noiria-test-savedata-thumb";
        thumb_test_dir(ID);
        let mut px = crate::pixmap::Pixmap::new(THUMB_W, THUMB_H);
        px.fill([200, 40, 80, 255]);
        write_thumb(ID, "save1", &px).unwrap();

        let path = thumb_path(ID, "save1").unwrap();
        let ext = image::open(&path).unwrap().to_rgba8();
        assert_eq!((ext.width(), ext.height()), (THUMB_W, THUMB_H));
        assert_eq!(ext.get_pixel(5, 5).0, [200, 40, 80, 255]);

        let ours = read_thumb(ID, "save1", THUMB_W, THUMB_H).unwrap().unwrap();
        assert_eq!((ours.width, ours.height), (THUMB_W, THUMB_H));
        assert_eq!(&ours.data[..4], &[200, 40, 80, 255]);
        thumb_test_dir(ID);
    }

    /// 画面サイズのままの旧サムネイルは、上限に収まるよう縮小して読める。無いスロットは None。
    #[test]
    fn read_thumb_downsizes_legacy_fullsize_and_missing_is_none() {
        const ID: &str = "noiria-test-savedata-thumb-legacy";
        thumb_test_dir(ID);
        assert!(read_thumb(ID, "save9", THUMB_W, THUMB_H).unwrap().is_none());
        let mut big = crate::pixmap::Pixmap::new(1280, 720);
        big.fill([1, 2, 3, 255]);
        crate::assets::save_png(&big, &thumb_path(ID, "save2").unwrap()).unwrap();
        let t = read_thumb(ID, "save2", THUMB_W, THUMB_H).unwrap().unwrap();
        assert_eq!((t.width, t.height), (240, 135));
        assert_eq!(&t.data[..4], &[1, 2, 3, 255]);
        // 壊れた PNG は黙って空にせずエラー（呼び出し側が空表示にする）。
        fs::write(thumb_path(ID, "save3").unwrap(), b"not a png").unwrap();
        assert!(read_thumb(ID, "save3", THUMB_W, THUMB_H).is_err());
        thumb_test_dir(ID);
    }

    #[test]
    fn thumb_slot_names_cannot_escape_the_save_dir() {
        let px = crate::pixmap::Pixmap::new(2, 2);
        for bad in ["../evil", "a/b", r"a\b", "..", "", "C:evil"] {
            assert!(write_thumb("noiria-test-savedata-thumb-bad", bad, &px).is_err(), "{bad}");
            assert!(read_thumb("noiria-test-savedata-thumb-bad", bad, 8, 8).is_err(), "{bad}");
        }
        thumb_test_dir("noiria-test-savedata-thumb-bad");
    }
    // --- R1: thumb_path の検証 / 一時的な読み込み失敗での退避 / サムネイルの原子書き込み ---

    /// `screenshot` op はスクリプトの文字列をそのまま `thumb_path` へ渡すため、ここで弾かないと
    /// セーブ領域の外へ PNG を書ける。
    #[test]
    fn thumb_path_rejects_names_that_escape_the_save_dir() {
        for bad in [
            "../../x",
            "../evil",
            "a/b",
            r"a\b",
            "..",
            "",
            "C:evil",
            "/etc/passwd",
            r"C:\Windows\x",
        ] {
            assert!(thumb_path("noiria-test-savedata-thumb-path", bad).is_err(), "{bad}");
        }
        let ok = thumb_path("noiria-test-savedata-thumb-path", "save_1-a").unwrap();
        assert_eq!(ok.file_name().unwrap(), "save_1-a.png");
        thumb_test_dir("noiria-test-savedata-thumb-path");
    }

    /// 読めなかった（IO エラー）ことと、読めたが壊れていることを区別する。前者で正常なセーブを
    /// `.bak` へ退避すると、一時的な共有違反や権限エラーでセーブを失い、以前の `.bak` も潰す。
    #[test]
    fn read_failure_is_not_treated_as_corruption() {
        const ID: &str = "noiria-test-savedata-readfail";
        thumb_test_dir(ID);
        let dir = save_dir(ID).unwrap();
        // 「存在するが読めない」を OS 非依存に作る: スロット名のパスがディレクトリ。
        fs::create_dir(dir.join("sys.json")).unwrap();
        fs::write(dir.join("sys.json.bak"), "previous backup").unwrap();
        assert!(read_slot(ID, "sys").is_err(), "IO エラーは Err で返す");
        assert!(dir.join("sys.json").is_dir(), "読めなかっただけのものを退避してはいけない");
        assert_eq!(
            fs::read_to_string(dir.join("sys.json.bak")).unwrap(),
            "previous backup",
            "以前の .bak が上書きされた"
        );
        thumb_test_dir(ID);
    }

    /// サムネイルを書き換え続けている間、先読みスレッド相当の読み手が半端な PNG を見ない。
    #[test]
    fn thumb_rewrite_is_never_observed_half_written() {
        const ID: &str = "noiria-test-savedata-thumb-atomic";
        thumb_test_dir(ID);
        let path = thumb_path(ID, "s1").unwrap();
        let mut a = crate::pixmap::Pixmap::new(THUMB_W, THUMB_H);
        a.fill([10, 20, 30, 255]);
        write_thumb(ID, "s1", &a).unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let stop = stop.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                let mut bad = 0usize;
                let mut reads = 0usize;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    if let Ok(bytes) = fs::read(&path) {
                        reads += 1;
                        if crate::codec::png::decode(&bytes).is_err() {
                            bad += 1;
                        }
                    }
                }
                (bad, reads)
            })
        };
        let mut b = crate::pixmap::Pixmap::new(THUMB_W, THUMB_H);
        for i in 0..60u8 {
            b.fill([i, 255 - i, i.wrapping_mul(3), 255]);
            write_thumb(ID, "s1", &b).unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let (bad, reads) = reader.join().unwrap();
        assert_eq!(bad, 0, "半端な PNG を {bad}/{reads} 回読んだ");
        let names: Vec<String> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["s1.png".to_string()], "一時ファイルが残っている");
        thumb_test_dir(ID);
    }
}
