//! Xvfb 上でのみ実走する統合テスト。`NOIRIA_X11_TEST_DISPLAY`（例: ":99"）が
//! 未設定なら即座に skip する（Windows・ヘッドレス CI・WSL でも Xvfb を用意
//! していない環境を壊さない）。**ユーザーのデスクトップ（WSLg :0 含む）には
//! 一切接続しない**（`DISPLAY` ではなく `NOIRIA_X11_TEST_DISPLAY` だけを見る）。
//!
//! 複数のテストが同一 Xvfb 上でウィンドウを生成・スタッキングするため、
//! `cargo test ... -- --test-threads=1` での直列実行を前提とする
//! （`scripts/wsl-test.sh` 参照）。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::unbounded;
use tokio::sync::{mpsc::unbounded_channel, Notify};

use super::super::{conn, proto};
use super::{intern, run_windowed, WindowedParams};

fn test_display() -> Option<String> {
    std::env::var("NOIRIA_X11_TEST_DISPLAY")
        .ok()
        .filter(|s| !s.is_empty())
}

macro_rules! require_xvfb {
    () => {
        match test_display() {
            Some(d) => d,
            None => {
                eprintln!("NOIRIA_X11_TEST_DISPLAY 未設定のため skip します");
                return;
            }
        }
    };
}

fn connect_and_handshake(display: &str) -> (proto::XConn, conn::Setup) {
    let spec = conn::parse_display(display).unwrap();
    let stream = conn::connect_socket(spec.display).unwrap();
    let (auth_name, auth_data) = conn::find_mit_cookie(spec.display).unwrap_or_default();
    let mut stream = stream;
    let setup = conn::handshake(&mut stream, &auth_name, &auth_data).unwrap();
    let c = proto::XConn::new(stream, setup.resource_id_base, setup.resource_id_mask);
    (c, setup)
}

/// ①接続 + ハンドシェイク: visual / max-request-length の妥当性。
#[test]
fn handshake_reports_sane_visual_and_max_request_length() {
    let display = require_xvfb!();
    let (_c, setup) = connect_and_handshake(&display);
    assert!(
        setup.depth == 24 || setup.depth == 32,
        "depth={}",
        setup.depth
    );
    assert!(
        setup.max_request_length >= 4096,
        "max_request_length={}",
        setup.max_request_length
    );
    assert_ne!(setup.root, 0);
    assert!(setup.max_keycode >= setup.min_keycode);
}

/// ②ウィンドウ生成 → 帯境界を跨ぐパターン画像を PutImage → GetImage 読み戻し
/// でピクセル一致。
#[test]
fn put_image_then_get_image_roundtrip_across_band_boundary() {
    let display = require_xvfb!();
    let (mut c, setup) = connect_and_handshake(&display);

    let wid = c.alloc_id();
    let gc = c.alloc_id();
    let w: u32 = 1280;
    // 帯サイズは 1280 幅で 40〜50 行程度になる想定なので、100 行あれば
    // 複数バンドをまたぐ。
    let h: u32 = 100;
    let create = proto::req_create_window(
        wid,
        setup.root,
        0,
        0,
        w as u16,
        h as u16,
        setup.depth,
        setup.root_visual,
        0,
        0,
    );
    c.send_request(&create).unwrap();
    c.send_request(&proto::req_create_gc(gc, wid)).unwrap();
    c.send_request(&proto::req_map_window(wid)).unwrap();

    let mut pixels = vec![0u32; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let r = x % 256;
            let g = y % 256;
            let b = (x + y) % 256;
            pixels[(y * w + x) as usize] = (r << 16) | (g << 8) | b;
        }
    }
    proto::put_image_banded(
        &mut c,
        wid,
        gc,
        &pixels,
        (w, h),
        setup.depth,
        setup.max_request_length,
    )
    .unwrap();

    let readback = proto::get_image_pixels(&mut c, wid, 0, 0, w as u16, h as u16).unwrap();
    assert_eq!(readback.len(), pixels.len());
    assert_eq!(
        readback, pixels,
        "PutImage→GetImage のピクセルが一致しません"
    );
}

/// root の直接の子のうち、スタッキング上最も上（= QueryTree の最後の要素、
/// X11 は bottom→top の順で返す）にあるものを「直近に作られたウィンドウ」
/// とみなす。直列実行前提（--test-threads=1）なので他テストとの競合は無い。
fn top_child_window(spec: conn::DisplaySpec) -> Option<u32> {
    let mut stream = conn::connect_socket(spec.display).ok()?;
    let (auth_name, auth_data) = conn::find_mit_cookie(spec.display).unwrap_or_default();
    let setup = conn::handshake(&mut stream, &auth_name, &auth_data).ok()?;
    let mut c = proto::XConn::new(stream, setup.resource_id_base, setup.resource_id_mask);
    let reply = c
        .send_request_and_wait(&proto::req_query_tree(setup.root))
        .ok()?;
    let children = proto::parse_query_tree_children(&reply).ok()?;
    children.last().copied()
}

/// 前のテストのウィンドウは、そのクライアントの切断後に X サーバが非同期で破棄する。直後に
/// `top_child_window` で「新しいウィンドウ」を探すと、破棄前の古い窓を掴んで BadDrawable になるので、
/// ウィンドウを作るテストは先に root の子が空になるのを待つ（直列実行前提）。
fn wait_for_clean_root(display: &str) {
    let spec = conn::parse_display(display).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while top_child_window(spec).is_some() {
        assert!(Instant::now() < deadline, "root に古いウィンドウが残っています");
        thread::sleep(Duration::from_millis(50));
    }
}

/// ④SendEvent で WM_DELETE_WINDOW ClientMessage を自送 → スクリプトへ `WindowClose` が届き、
/// 応答が無ければ猶予（`CLOSE_GRACE`）後にホストが強制終了する。
#[test]
fn client_message_wm_delete_window_forwards_window_close_then_forces_quit() {
    let display = require_xvfb!();
    wait_for_clean_root(&display);
    std::env::set_var("NOIRIA_X11_TEST_DISPLAY", &display);

    let scene = Arc::new(Mutex::new(noiria_core::scene::Scene::new(320, 240)));
    let (input_tx, mut input_rx) = unbounded_channel();
    let trans_notify = Arc::new(Notify::new());
    let (_user_tx, user_rx) = unbounded();
    let start = Instant::now();

    let params = WindowedParams {
        title: "noiria x11 close test".to_string(),
        game_w: 320,
        game_h: 240,
        scene,
        input_tx,
        trans_notify,
        start,
        user_rx,
        icon: crate::icon::AppIcon::from_optional_png(None),
        visible: false,
    };

    let handle = thread::spawn(move || run_windowed(params));

    let spec = conn::parse_display(&display).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let wid = loop {
        if let Some(w) = top_child_window(spec) {
            break w;
        }
        assert!(
            Instant::now() < deadline,
            "ウィンドウがタイムアウトまでに現れませんでした"
        );
        thread::sleep(Duration::from_millis(50));
    };

    let (mut c, _setup) = connect_and_handshake(&display);
    let wm_protocols = intern(&mut c, "WM_PROTOCOLS").unwrap();
    let wm_delete_window = intern(&mut c, "WM_DELETE_WINDOW").unwrap();
    let event = proto::build_client_message32(wid, wm_protocols, wm_delete_window);
    // event-mask=0: 宛先クライアントへ直接配送（selected-input mask を無視、
    // WM が close protocol を送るときの標準的なやり方）。
    let t0 = Instant::now();
    c.send_request(&proto::req_send_event(wid, false, 0, &event))
        .unwrap();

    let close = recv_input(&mut input_rx);
    assert_eq!((close.kind, close.key.as_deref()), ("key", Some("WindowClose")));
    thread::sleep(Duration::from_millis(700));
    assert!(!handle.is_finished(), "猶予内に閉じてはならない");
    let result = handle
        .join()
        .expect("run_windowed スレッドが panic しました");
    result.expect("run_windowed がエラーを返しました（ClientMessage クローズ経路）");
    assert!(
        t0.elapsed() >= crate::input::CLOSE_GRACE - Duration::from_millis(100),
        "猶予より早く閉じた: {:?}",
        t0.elapsed()
    );
}

/// ③ `_NET_WM_ICON` プロパティが正しく設定されていることを GetProperty で読み戻して確認する
/// （先頭サイズが 16x16・先頭ピクセルの ARGB が `AppIcon` のリサイズ結果と一致）。
#[test]
fn net_wm_icon_property_matches_app_icon() {
    let display = require_xvfb!();
    wait_for_clean_root(&display);
    std::env::set_var("NOIRIA_X11_TEST_DISPLAY", &display);

    let scene = Arc::new(Mutex::new(noiria_core::scene::Scene::new(320, 240)));
    let (input_tx, _input_rx) = unbounded_channel();
    let trans_notify = Arc::new(Notify::new());
    let (_user_tx, user_rx) = unbounded();
    let start = Instant::now();

    let params = WindowedParams {
        title: "noiria x11 icon test".to_string(),
        game_w: 320,
        game_h: 240,
        scene,
        input_tx,
        trans_notify,
        start,
        user_rx,
        icon: crate::icon::AppIcon::from_optional_png(None),
        visible: false,
    };

    let handle = thread::spawn(move || run_windowed(params));

    let spec = conn::parse_display(&display).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let wid = loop {
        if let Some(w) = top_child_window(spec) {
            break w;
        }
        assert!(
            Instant::now() < deadline,
            "ウィンドウがタイムアウトまでに現れませんでした"
        );
        thread::sleep(Duration::from_millis(50));
    };

    let (mut c, _setup) = connect_and_handshake(&display);
    let net_wm_icon = intern(&mut c, "_NET_WM_ICON").unwrap();
    let values =
        proto::get_property_u32s(&mut c, wid, net_wm_icon, proto::XA_CARDINAL, 8192).unwrap();
    assert!(values.len() > 2, "_NET_WM_ICON プロパティが空です");
    assert_eq!(values[0], 16, "先頭サイズの width が16ではありません");
    assert_eq!(values[1], 16, "先頭サイズの height が16ではありません");

    let expected = crate::icon::AppIcon::from_optional_png(None).image(16);
    let (r, g, b, a) = (
        expected.rgba[0] as u32,
        expected.rgba[1] as u32,
        expected.rgba[2] as u32,
        expected.rgba[3] as u32,
    );
    let expected_argb = (a << 24) | (r << 16) | (g << 8) | b;
    assert_eq!(
        values[2], expected_argb,
        "先頭ピクセルの ARGB が一致しません"
    );

    // クリーンアップ: WM_DELETE_WINDOW で終了させる。
    let wm_protocols = intern(&mut c, "WM_PROTOCOLS").unwrap();
    let wm_delete_window = intern(&mut c, "WM_DELETE_WINDOW").unwrap();
    let event = proto::build_client_message32(wid, wm_protocols, wm_delete_window);
    let _ = c.send_request(&proto::req_send_event(wid, false, 0, &event));

    let result = handle
        .join()
        .expect("run_windowed スレッドが panic しました");
    result.expect("run_windowed がエラーを返しました（アイコンテスト）");
}

fn noiria_bin_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_noiria") {
        return p.into();
    }
    // integration test 用の env が無い場合のフォールバック（自テストバイナリの
    // 隣の target ディレクトリから探す）。
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // deps/
    p.pop(); // <profile>/
    p.push("noiria");
    p
}

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bgrx_pixels_to_png(pixels: &[u32], w: u32, h: u32, path: &std::path::Path) {
    let mut rgba = vec![0u8; pixels.len() * 4];
    for (i, px) in pixels.iter().enumerate() {
        let b = (px & 0xFF) as u8;
        let g = ((px >> 8) & 0xFF) as u8;
        let r = ((px >> 16) & 0xFF) as u8;
        rgba[i * 4] = r;
        rgba[i * 4 + 1] = g;
        rgba[i * 4 + 2] = b;
        rgba[i * 4 + 3] = 255;
    }
    let pm = noiria_core::pixmap::Pixmap::from_rgba_straight(w, h, &rgba);
    noiria_core::assets::save_png(&pm, path).expect("PNG 保存に失敗");
}

/// ⑤ `run_windowed` を games/sample で実際に走らせ、クラッシュしないことと
/// 日本語タイトルを含む実ゲーム画面が描画されることを確認する（実バイナリを
/// サブプロセスとして起動し、実際の main.rs windowed 経路を通す）。
/// スクリーンショットは GetImage で取得して `target/x11-smoke/` へ保存する
/// （目視確認用に /mnt/c 側へコピーする運用は wsl-test.sh 側で行う）。
#[test]
fn games_sample_smoke_with_screenshot() {
    let display = require_xvfb!();
    wait_for_clean_root(&display);
    let bin = noiria_bin_path();
    assert!(
        bin.exists(),
        "noiria バイナリが見つかりません: {}",
        bin.display()
    );

    let mut child = std::process::Command::new(&bin)
        .arg("games/sample")
        .current_dir(repo_root())
        .env("NOIRIA_X11_TEST_DISPLAY", &display)
        .env_remove("DISPLAY")
        .spawn()
        .expect("noiria バイナリの起動に失敗しました");

    let spec = conn::parse_display(&display).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let wid = loop {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("noiria が早期終了しました: {status:?}");
        }
        if let Some(w) = top_child_window(spec) {
            break w;
        }
        assert!(
            Instant::now() < deadline,
            "ウィンドウがタイムアウトまでに現れませんでした"
        );
        thread::sleep(Duration::from_millis(100));
    };

    // 数フレーム進行させてから撮影する。
    thread::sleep(Duration::from_secs(2));

    let (mut c, _setup) = connect_and_handshake(&display);
    let pixels =
        proto::get_image_pixels(&mut c, wid, 0, 0, 1280, 720).expect("GetImage に失敗しました");
    let out_dir = repo_root().join("target/x11-smoke");
    std::fs::create_dir_all(&out_dir).unwrap();
    bgrx_pixels_to_png(
        &pixels,
        1280,
        720,
        &out_dir.join("games_sample_screenshot.png"),
    );

    // 10 秒間クラッシュしないことを確認する。
    let ten_sec_deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < ten_sec_deadline {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("noiria が実行中に終了しました: {status:?}");
        }
        thread::sleep(Duration::from_millis(200));
    }

    // WM_DELETE_WINDOW で正常終了させる（失敗したら kill でフォールバック）。
    let wm_protocols = intern(&mut c, "WM_PROTOCOLS").unwrap();
    let wm_delete_window = intern(&mut c, "WM_DELETE_WINDOW").unwrap();
    let event = proto::build_client_message32(wid, wm_protocols, wm_delete_window);
    let _ = c.send_request(&proto::req_send_event(wid, false, 0, &event));

    let close_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        if Instant::now() >= close_deadline {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

// ---- ウィンドウスケーリング / フルスクリーン / キー入力（合成イベント）----

use crate::bridge::InputEvent;
use tokio::sync::mpsc::UnboundedReceiver;

fn recv_input(rx: &mut UnboundedReceiver<InputEvent>) -> InputEvent {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Ok(ev) = rx.try_recv() {
            return ev;
        }
        assert!(Instant::now() < deadline, "InputEvent を受信できませんでした");
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_no_input(rx: &mut UnboundedReceiver<InputEvent>) {
    thread::sleep(Duration::from_millis(200));
    assert!(rx.try_recv().is_err(), "想定外の InputEvent が届いています");
}

/// 別クライアントが他人のウィンドウを `width`×`height` にリサイズする（ConfigureWindow）。
fn req_configure_size(window: u32, width: u32, height: u32) -> Vec<u8> {
    let mut req = vec![12u8, 0, 5, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&(0x0004u16 | 0x0008).to_le_bytes()); // width | height
    req.extend_from_slice(&[0, 0]);
    req.extend_from_slice(&width.to_le_bytes());
    req.extend_from_slice(&height.to_le_bytes());
    req
}

/// ChangeWindowAttributes(event-mask)。偽の WM として root の SubstructureRedirect を握るのに使う。
fn req_select_input(window: u32, mask: u32) -> Vec<u8> {
    let mut req = vec![2u8, 0, 4, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&0x0800u32.to_le_bytes()); // CWEventMask
    req.extend_from_slice(&mask.to_le_bytes());
    req
}

fn key_event(code: u8, keycode: u8, time: u32, state: u16) -> [u8; 32] {
    let mut ev = [0u8; 32];
    ev[0] = code;
    ev[1] = keycode;
    ev[4..8].copy_from_slice(&time.to_le_bytes());
    ev[28..30].copy_from_slice(&state.to_le_bytes());
    ev
}

fn button_event(button: u8, x: i16, y: i16) -> [u8; 32] {
    let mut ev = [0u8; 32];
    ev[0] = proto::EV_BUTTON_PRESS;
    ev[1] = button;
    ev[24..26].copy_from_slice(&x.to_le_bytes());
    ev[26..28].copy_from_slice(&y.to_le_bytes());
    ev
}

fn find_keycode(display: &str, keysym: u32) -> u8 {
    let (mut c, setup) = connect_and_handshake(display);
    let count = setup.max_keycode - setup.min_keycode + 1;
    let reply = c
        .send_request_and_wait(&proto::req_get_keyboard_mapping(setup.min_keycode, count))
        .unwrap();
    let km = super::keymap::parse_reply(setup.min_keycode, &reply).unwrap();
    (setup.min_keycode..=setup.max_keycode)
        .find(|&kc| km.keysym(kc, 0) == keysym)
        .unwrap_or_else(|| panic!("keysym 0x{keysym:X} のキーコードが Xvfb のキーマップに無い"))
}

struct Running {
    handle: thread::JoinHandle<anyhow::Result<()>>,
    wid: u32,
    input_rx: UnboundedReceiver<InputEvent>,
    c: proto::XConn,
}

/// 320x240 の赤一色シーンで `run_windowed`（Xvfb 上、可視）を起動する。
fn spawn_red_window(display: &str) -> Running {
    std::env::set_var("NOIRIA_X11_TEST_DISPLAY", display);
    wait_for_clean_root(display);
    let mut scene = noiria_core::scene::Scene::new(320, 240);
    let id = scene.alloc_layer(noiria_core::scene::LayerKind::Image);
    let mut pm = noiria_core::pixmap::Pixmap::new(320, 240);
    pm.fill([255, 0, 0, 255]);
    {
        let layer = scene
            .layer_mut(noiria_core::scene::Page::Fore, id)
            .unwrap();
        layer.pixmap = Some(Arc::new(pm));
        layer.visible = true;
    }
    scene.bump();
    let scene = Arc::new(Mutex::new(scene));
    let (input_tx, input_rx) = unbounded_channel();
    let (_user_tx, user_rx) = unbounded();
    let params = WindowedParams {
        title: "noiria x11 scale test".to_string(),
        game_w: 320,
        game_h: 240,
        scene,
        input_tx,
        trans_notify: Arc::new(Notify::new()),
        start: Instant::now(),
        user_rx,
        icon: crate::icon::AppIcon::from_optional_png(None),
        visible: true,
    };
    let handle = thread::spawn(move || run_windowed(params));
    let spec = conn::parse_display(display).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let wid = loop {
        if let Some(w) = top_child_window(spec) {
            break w;
        }
        assert!(Instant::now() < deadline, "ウィンドウが現れません");
        thread::sleep(Duration::from_millis(50));
    };
    let (c, _setup) = connect_and_handshake(display);
    Running {
        handle,
        wid,
        input_rx,
        c,
    }
}

impl Running {
    fn send(&mut self, ev: &[u8; 32]) {
        self.c
            .send_request(&proto::req_send_event(self.wid, false, 0, ev))
            .unwrap();
    }

    /// スクリプト死亡扱い（入力受信口を捨てる）にしてから WM_DELETE_WINDOW で即終了させる。
    fn finish(mut self) {
        drop(self.input_rx);
        let wm_protocols = intern(&mut self.c, "WM_PROTOCOLS").unwrap();
        let wm_delete = intern(&mut self.c, "WM_DELETE_WINDOW").unwrap();
        let ev = proto::build_client_message32(self.wid, wm_protocols, wm_delete);
        let _ = self
            .c
            .send_request(&proto::req_send_event(self.wid, false, 0, &ev));
        self.handle.join().unwrap().unwrap();
    }
}

/// リサイズに追随してレターボックス拡縮される（黒帯 + 中身は赤）。マウス座標も逆変換される。
#[test]
fn resize_letterboxes_frame_and_inverse_maps_mouse() {
    let display = require_xvfb!();
    let mut r = spawn_red_window(&display);
    let red = 0x00FF_0000u32;

    // 800x240: 左右 240px の黒帯、中央 320 幅が赤。
    r.c.send_request(&req_configure_size(r.wid, 800, 240)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let px = proto::get_image_pixels(&mut r.c, r.wid, 0, 120, 800, 1).unwrap();
        let ok = px[239] & 0xFFFFFF == 0
            && px[240] & 0xFFFFFF == red
            && px[559] & 0xFFFFFF == red
            && px[560] & 0xFFFFFF == 0;
        if ok {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "レターボックスが描画されません: [239]={:06X} [240]={:06X} [559]={:06X} [560]={:06X}",
            px[239] & 0xFFFFFF,
            px[240] & 0xFFFFFF,
            px[559] & 0xFFFFFF,
            px[560] & 0xFFFFFF
        );
        thread::sleep(Duration::from_millis(50));
    }

    // マウス: 中央 (400,120) → ゲーム (160,120)、帯の左端 (240,0) → (0,0)、黒帯上は範囲外。
    let ev = button_event(1, 400, 120);
    r.send(&ev);
    let c = recv_input(&mut r.input_rx);
    assert_eq!((c.kind, c.x, c.y), ("click", 160.0, 120.0));
    let ev = button_event(1, 240, 0);
    r.send(&ev);
    let c = recv_input(&mut r.input_rx);
    assert_eq!((c.x, c.y), (0.0, 0.0));
    let ev = button_event(1, 10, 120);
    r.send(&ev);
    assert!(recv_input(&mut r.input_rx).x < 0.0);

    // 2 倍 (640x480): 全面が赤で、マウスは 1/2 倍。
    r.c.send_request(&req_configure_size(r.wid, 640, 480)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let px = proto::get_image_pixels(&mut r.c, r.wid, 0, 0, 640, 480).unwrap();
        if px[0] & 0xFFFFFF == red && px[640 * 480 - 1] & 0xFFFFFF == red {
            break;
        }
        assert!(Instant::now() < deadline, "拡大描画されません");
        thread::sleep(Duration::from_millis(50));
    }
    let ev = button_event(1, 320, 240);
    r.send(&ev);
    let c = recv_input(&mut r.input_rx);
    assert_eq!((c.x, c.y), (160.0, 120.0));
    r.finish();
}

/// F11 / Alt+Enter は WM（ここでは偽 WM として root の SubstructureRedirect を握る別クライアント）
/// へ `_NET_WM_STATE` fullscreen の toggle を依頼し、スクリプトへは何も送らない。
#[test]
fn f11_and_alt_enter_request_wm_fullscreen_toggle() {
    let display = require_xvfb!();
    let mut r = spawn_red_window(&display);
    let f11 = find_keycode(&display, super::keymap::KS_F11);
    let ret = find_keycode(&display, super::keymap::KS_RETURN);

    // 偽 WM: root の SubstructureRedirect を握り、届く ClientMessage を読む。
    let (mut wm, wm_setup) = connect_and_handshake(&display);
    let net_wm_state = intern(&mut wm, "_NET_WM_STATE").unwrap();
    let fs_atom = intern(&mut wm, "_NET_WM_STATE_FULLSCREEN").unwrap();
    wm.send_request(&req_select_input(wm_setup.root, 0x0010_0000))
        .unwrap();
    wm.stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();

    let mut expect_toggle = |r: &mut Running, ev: [u8; 32]| {
        r.send(&ev);
        loop {
            match proto::read_packet(&mut wm.stream).expect("偽 WM がイベントを受信できません") {
                proto::Packet::Event { code, data } if code == proto::EV_CLIENT_MESSAGE => {
                    let ty = u32::from_le_bytes(data[8..12].try_into().unwrap());
                    let d0 = u32::from_le_bytes(data[12..16].try_into().unwrap());
                    let d1 = u32::from_le_bytes(data[16..20].try_into().unwrap());
                    assert_eq!(ty, net_wm_state);
                    assert_eq!((d0, d1), (2, fs_atom), "toggle + fullscreen のはず");
                    return;
                }
                _ => continue,
            }
        }
    };
    expect_toggle(&mut r, key_event(proto::EV_KEY_PRESS, f11, 100, 0));
    // Alt(Mod1)+Return
    expect_toggle(&mut r, key_event(proto::EV_KEY_PRESS, ret, 200, 0x8));
    // どちらもスクリプトへは渡らない（Return の keyup も、keydown を送っていないので出ない）。
    let ev = key_event(proto::EV_KEY_RELEASE, ret, 210, 0x8);
    r.send(&ev);
    assert_no_input(&mut r.input_rx);
    r.finish();
}

/// オートリピート（同一時刻の KeyRelease+KeyPress 対）は送らず、初回押下と最後のキーアップだけ送る。
#[test]
fn autorepeat_pairs_are_suppressed_and_keyup_is_delivered() {
    let display = require_xvfb!();
    let mut r = spawn_red_window(&display);
    let ctrl = find_keycode(&display, 0xFFE3);

    let ev = key_event(proto::EV_KEY_PRESS, ctrl, 1000, 0);
    r.send(&ev);
    let d = recv_input(&mut r.input_rx);
    assert_eq!((d.kind, d.key.as_deref()), ("key", Some("Control")));
    for t in [1500u32, 1530, 1560] {
        let rel = key_event(proto::EV_KEY_RELEASE, ctrl, t, 0x4);
        let prs = key_event(proto::EV_KEY_PRESS, ctrl, t, 0x4);
        r.send(&rel);
        r.send(&prs);
    }
    assert_no_input(&mut r.input_rx);
    let ev = key_event(proto::EV_KEY_RELEASE, ctrl, 2000, 0x4);
    r.send(&ev);
    let u = recv_input(&mut r.input_rx);
    assert_eq!((u.kind, u.key.as_deref()), ("keyup", Some("Control")));
    assert_no_input(&mut r.input_rx);
    r.finish();
}

/// スクリプトが死んでいる（入力受信口が閉じている）なら、閉じる操作は即終了する。
#[test]
fn wm_delete_window_with_dead_script_quits_immediately() {
    let display = require_xvfb!();
    let r = spawn_red_window(&display);
    let t0 = Instant::now();
    r.finish();
    assert!(t0.elapsed() < crate::input::CLOSE_GRACE, "{:?}", t0.elapsed());
}

fn fake_setup(px: (u16, u16), mm: (u16, u16)) -> conn::Setup {
    conn::Setup {
        root: 0,
        screen_px: px,
        screen_mm: mm,
        root_visual: 0,
        depth: 24,
        resource_id_base: 0,
        resource_id_mask: 0,
        max_request_length: 0,
        min_keycode: 8,
        max_keycode: 255,
    }
}

/// 初期サイズ: 96dpi 相当（Xvfb など）は等倍のまま、高 DPI は倍率を掛けて画面の 92% に収める。
#[test]
fn initial_size_follows_screen_dpi_and_fits_the_screen() {
    use super::initial_size;
    // Xvfb 既定（96dpi）: 画面と同じ 1280x720 でも縮めない。
    assert_eq!(initial_size(&fake_setup((1280, 720), (339, 191)), 1280, 720), (1280, 720));
    // 丸め誤差で 97dpi になっても等倍扱い（108 未満は 96）。
    assert_eq!(initial_size(&fake_setup((1280, 720), (335, 188)), 1280, 720), (1280, 720));
    // 4K・192dpi: 2 倍が画面に収まるのでそのまま。
    assert_eq!(initial_size(&fake_setup((3840, 2160), (508, 286)), 1280, 720), (2560, 1440));
    // 1080p・144dpi: 1.5 倍は画面の 92% を超えるので、アスペクトを保って縮める。
    let (w, h) = initial_size(&fake_setup((1920, 1080), (339, 191)), 1280, 720);
    assert!(w <= 1920 * 92 / 100 && h <= 1080 * 92 / 100, "{w}x{h}");
    assert!((w as f64 / h as f64 - 1280.0 / 720.0).abs() < 0.01, "{w}x{h}");
    assert!(w > 1280, "拡大されるはず: {w}x{h}");
    // 寸法不明（0）でも panic せず等倍。
    assert_eq!(initial_size(&fake_setup((0, 0), (0, 0)), 1280, 720), (1280, 720));
}
