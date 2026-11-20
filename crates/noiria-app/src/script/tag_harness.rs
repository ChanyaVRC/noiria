//! KAG タグ振る舞いテストハーネス。
//!
//! 小さな .ks シナリオ断片を実際の `KagPlayer`（V8/tsint 両エンジン、`crate::script::spawn`/
//! `crate::script::tsint_engine::spawn` という本番と同じエントリポイント）で実行し、
//! `EngineState::op_log`（`docs/implementation-notes.md`「2026-07-11 タグ振る舞いテスト
//! スイート」参照）に記録された ops 呼び出し列を回収して返す。
//!
//! ## 決定論化の方式
//!
//! - クリック等の入力は `InputEvent` を `tokio::mpsc` チャンネルへ 1 つずつ送る。
//!   `runtime/api.ts` の `InputHub.pump()` は常設の単一ループでチャンネルを FIFO に汲み出し、
//!   その時点で登録済みのハンドラへだけ配送する（後から `[l]`/リンク待ちが `api.input.on()`
//!   する前に消費されてしまうと、そのクリックは永久に失われる）。V8 isolate 生成 +
//!   `runtime/kag/*.ts` 群のトランスパイルだけで実時間 200ms 前後かかることが実機デバッグで
//!   判明したため、固定の短い sleep では最初のクリックがエンジン起動より先に届いて
//!   取りこぼす。代わりに [`wait_for_quiescence`] で「`op_log` が非空になり、かつ一定時間
//!   伸びなくなった」＝スクリプトが次の待ちに到達して静止したとみなせるまで待ってから
//!   1 クリックずつ送る（合成イベント + 時間制御での決定論化）。
//! - 送り終えたクリックを使い切った後は送信側を `drop` してチャンネルを閉じる。KagPlayer が
//!   想定より多くクリックを要求した場合は `op_input_next_event` が `kind: "quit"` を返し、
//!   `InputHub.pump()` が停止する。この場合は後述のウォッチドッグのタイムアウトでテスト失敗
//!   として顕在化する（クリック数を数え間違えたフラグメントのバグを検知しやすくする）。
//! - シナリオ断片は必ず `[close]` で終える。`[close]` は `api.sys.quit()`（`UserEvent::Quit`）
//!   を送るだけで `KagPlayer.running` を false にはしない（本番は外側の Win32/ヘッドレス
//!   メインループが Quit を見てプロセスごと終了する設計）ため、本ハーネスでは
//!   `UserEvent::Quit` を受信した時点で ops ログを回収して打ち切る。スクリプト用スレッド
//!   自体は（`idleUntilJump` が `sys.wait(60)` を無限ポーリングし続けるため）バックグラウンドに
//!   残り続けるが、意図的に `join` しない（1 テストあたり 1 スレッドを残す程度は許容する
//!   トレードオフとして記録する）。
//! - タイムアウト（既定 10 秒）を超えても `UserEvent::Quit` が届かない場合はハングではなく
//!   `panic!` で明示的にテスト失敗させる（クリック数不足等のフラグメントのバグを検知しやすくする）。
//!
//! ## V8/tsint 比較
//!
//! 同じ断片・同じクリック列を [`run_v8`] と [`run_tsint`] の両方に通し、`compare_logs` で
//! 突き合わせる。ops ログの書式は両エンジンで完全に同一（`EngineState` を共用しているため）
//! なので、行ごとの完全一致を期待できる。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver};
use noiria_core::assets::{save_png, GameDir};
use noiria_core::audio::AudioShared;
use noiria_core::pixmap::Pixmap;
use noiria_core::scene::Scene;
use tokio::sync::Notify;

use crate::bridge::{InputEvent, SharedScene, UserEvent, WakeSink};
use crate::script::engine_state::SharedOpLog;
use crate::script::EngineCtx;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Engine {
    V8,
    Tsint,
}

static RUN_ENGINE_LOCK: Mutex<()> = Mutex::new(());

/// 一意な使い捨てゲームディレクトリ（drop で自動削除、既存テストの `TempDir` パターンと同じ）。
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("noiria-tagtest-{tag}-{ns}-{n}"));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

static FIXTURE_CLIP_2FRAMES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/clip_2frames.avi"
));

/// 全カテゴリのフラグメントが共通で使うアセット一式を書き出す。
/// `storage=` にはスラッシュ入りの相対パスをそのまま渡す設計にしている
/// （`runtime/kag/player.ts` の `resolveStorage` は `/` を含む storage を kind 別
/// プレフィックスなしでそのまま使うため、fg/bg/se/bgm/rule 等の分類を気にせず 1 か所に置ける）。
fn write_common_fixtures(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("assets")).unwrap();

    // 汎用の単色 4x4 画像（[image]/[graph]/[pimage]/[animstart] 等で使い回す）。
    let mut solid = Pixmap::new(4, 4);
    solid.fill([200, 100, 50, 255]);
    save_png(&solid, &dir.join("assets/solid.png")).unwrap();

    // 3 状態ボタン画像（12x4、赤/緑/青を横に並べたもの。既存の
    // `button_manager_hit_test_and_state_transitions_in_v8` と同じフィクスチャ）。
    let mut sprite = Pixmap::new(12, 4);
    sprite.fill([0, 0, 0, 0]);
    let red = Pixmap::from_rgba_straight(4, 4, &[255, 0, 0, 255].repeat(4 * 4));
    let green = Pixmap::from_rgba_straight(4, 4, &[0, 255, 0, 255].repeat(4 * 4));
    let blue = Pixmap::from_rgba_straight(4, 4, &[0, 0, 255, 255].repeat(4 * 4));
    sprite.blit_over(&red, 0, 0, 255);
    sprite.blit_over(&green, 4, 0, 255);
    sprite.blit_over(&blue, 8, 0, 255);
    save_png(&sprite, &dir.join("assets/button.png")).unwrap();

    // universal トランジション用ルール画像（単色でも `rule_plane_from_rgba` は通る）。
    let mut rule = Pixmap::new(4, 4);
    rule.fill([128, 128, 128, 255]);
    save_png(&rule, &dir.join("assets/rule.png")).unwrap();

    // [animstart] 用 .asd 最小サブセット（2 セル、loop=false）。セルのパスはスラッシュ入り
    // （`resolveStorage` がプレフィックスを付けずそのまま使う形式）にする。
    std::fs::write(
        dir.join("assets/blink.asd"),
        "loop=false\n0,assets/solid.png,20\n1,assets/solid.png,20\n",
    )
    .unwrap();

    // [openvideo]/[preparevideo]/[playvideo]/[video] 用の固定フィクスチャ（2 フレーム）。
    std::fs::write(dir.join("assets/clip.avi"), FIXTURE_CLIP_2FRAMES).unwrap();
}

/// 入力以外のゲーム側の追加設定（既定は何も足さない）。
#[derive(Clone, Copy, Default)]
pub(crate) struct Setup<'a> {
    /// `new KagPlayer({ charaLayerCount: 1, <ここ> })` に足すオプション（例: `message: { fontFamily: "X" }`）。
    pub player_opts: &'a str,
    /// `scenario/` へ追加で置くシナリオ (ファイル名, 中身)。
    pub extra_scenarios: &'a [(&'a str, &'a str)],
    /// 起動前に `sys` スロット（sys.json）へ書いておく中身（前回プレイ済みの状態を再現する）。
    pub sys_json: Option<&'a str>,
    /// 起動前にセーブ領域へ書いておくスロット (スロット名, JSON)。旧版が書いたセーブの再現に使う。
    pub slots: &'a [(&'a str, &'a str)],
    /// `main.ts` の `new KagPlayer` の前に挿入する TS（import 文を含めてよい）。純関数の検証を
    /// `api.sys.log(...)` 経由で op_log に出し、V8/tsint の両方で同じ結果になることを見る。
    pub prelude: &'a str,
}

/// 1 手ぶんの入力/確認。`Click`/`RightClick`/`Key`/`KeyUp` は直前までのスクリプト動作が
/// 静止してから送る（`wait_for_quiescence`）。`Sleep` は実時間待ち、`Check` はその時点の ops ログに
/// 対する途中アサーション。
#[derive(Clone, Copy)]
pub(crate) enum Step {
    Click(f64, f64),
    RightClick(f64, f64),
    Key(&'static str),
    /// 静止を待たずに今すぐキーを送る（スキップ中のように ops が出続けて静止しない状況用）。
    KeyNow(&'static str),
    /// 静止を待たずに今すぐ左クリックを送る（モダン UI のクリック待ち記号は一定間隔で ops を出し続けるため、
    /// `Click` の静止待ちが成立しない。直前に `Sleep` で待ってから使う）。
    ClickNow(f64, f64),
    KeyUp(&'static str),
    Sleep(u64),
    Check(fn(&[String])),
}

fn write_scenario(dir: &std::path::Path, ks_body: &str, setup: Setup) {
    let main_ts = format!(
        "import {{ KagPlayer }} from \"noiria:kag\";\n\
         {}\n\
         const player = new KagPlayer({{ charaLayerCount: 1, {} }});\n\
         await player.run(\"test.ks\");\n",
        setup.prelude, setup.player_opts
    );
    std::fs::write(dir.join("main.ts"), main_ts).unwrap();
    std::fs::create_dir_all(dir.join("scenario")).unwrap();
    for (name, body) in setup.extra_scenarios {
        std::fs::write(dir.join("scenario").join(name), body).unwrap();
    }
    // [nowait]: 既定で瞬間表示にしておく。文字送り (cps) のリベール待ちが無いと
    // [l]/[p] は「1 クリック=送り」に単純化でき、テスト側のクリック数を数えやすくなる
    // （さもないと KAG の「1 回目でリベール完了・2 回目で送り」挙動を毎回考慮する必要がある）。
    // 文字送り自体を検証したいフラグメントは内部で明示的に [endnowait] すればよい。
    let body = format!("*start\n[nowait]\n{ks_body}\n[close]\n");
    std::fs::write(dir.join("scenario/test.ks"), body).unwrap();
}

fn write_game_json(dir: &std::path::Path, id: &str) {
    std::fs::write(
        dir.join("game.json"),
        format!(r#"{{"title":"t","id":"{id}","main":"main.ts","width":1280,"height":720}}"#),
    )
    .unwrap();
}

/// 実行結果。op_log に加え、フラグメント実行に使ったゲームディレクトリの絶対パス
/// （アサーション側で `read_text`/`file_exists` 系タグの相対パスを組み立てる際に使う）。
pub(crate) struct RunOutcome {
    pub log: Vec<String>,
}

/// `op_log` が非空になり、かつその長さが `QUIET_MS` の間変化しなくなるまで待つ（クリック
/// 送信前の同期点、モジュール冒頭コメント参照）。`len == 0` の間は「V8 isolate 生成 +
/// `runtime/kag/*.ts` 群のトランスパイルがまだ終わっていないだけ」であって「静止した」
/// わけではないので、安定判定のタイマーを起動しない（実機デバッグで踏んだ罠: これを
/// 区別しないと、起動処理中のたまたまの無音区間を「もう待ち状態」と誤認してクリックを
/// 早撃ちしてしまい、`[l]` のハンドラ登録前にイベントを消費されて永久ハングする）。
/// `deadline` を超えたら諦めて即座に返す（実際の失敗検知は呼び出し元の `[close]` 到達
/// ウォッチドッグに任せる）。
fn wait_for_quiescence(op_log: &SharedOpLog, deadline: Instant) {
    // 80ms では負荷の高いマシン・テスト並列実行時に「まだ実行中なのに静止」と誤判定する
    // 散発フレークが複数回報告されたため 160ms へ引き上げ（2026-07-12。スイート全体 +数秒の代償）。
    const QUIET_MS: u64 = 160;
    const POLL_MS: u64 = 20;
    let mut last_len = 0usize;
    let mut stable_since: Option<Instant> = None;
    loop {
        std::thread::sleep(Duration::from_millis(POLL_MS));
        let len = op_log.lock().unwrap().len();
        if len == 0 {
            // まだ起動処理中: 安定判定を開始しない。
        } else if len != last_len {
            last_len = len;
            stable_since = Some(Instant::now());
        } else if stable_since.is_some_and(|s| s.elapsed() >= Duration::from_millis(QUIET_MS)) {
            return;
        }
        if Instant::now() >= deadline {
            return;
        }
    }
}

/// [`run_v8`]/[`run_tsint`] 共通の駆動ロジック。
fn run_engine(engine: Engine, ks_body: &str, clicks: &[(f64, f64)]) -> RunOutcome {
    let steps: Vec<Step> = clicks.iter().map(|&(x, y)| Step::Click(x, y)).collect();
    run_engine_steps(engine, ks_body, Setup::default(), &steps)
}

fn input_event(
    kind: &'static str,
    x: f64,
    y: f64,
    button: Option<&'static str>,
    key: Option<&str>,
) -> InputEvent {
    InputEvent {
        kind,
        x,
        y,
        button,
        key: key.map(str::to_string),
        delta: None,
        t: 0.0,
    }
}

/// [`run_engine`] の一般形: クリック以外のキー/右クリック/待ち/途中確認を含む手順で駆動する。
fn run_engine_steps(engine: Engine, ks_body: &str, setup: Setup, steps: &[Step]) -> RunOutcome {
    // wait_for_quiescence は実時間の静止区間から待機到達を推定するため、並行実行による
    // スケジューリング遅延がハンドラ登録前のクリック早撃ちを再発させる。
    let _run_guard = RUN_ENGINE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let tag = match engine {
        Engine::V8 => "v8",
        Engine::Tsint => "tsint",
    };
    let dir = TempDir::new(tag);
    let id = format!(
        "noiria-tagtest-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    write_game_json(dir.path(), &id);
    write_scenario(dir.path(), ks_body, setup);
    write_common_fixtures(dir.path());
    if let Some(json) = setup.sys_json {
        noiria_core::savedata::write_slot(&id, "sys", json).expect("sys スロットの事前書き込み");
    }
    for (slot, json) in setup.slots {
        noiria_core::savedata::write_slot(&id, slot, json).expect("セーブスロットの事前書き込み");
    }

    let (game_dir, config) = GameDir::open(dir.path()).expect("GameDir::open");
    let scene: SharedScene = Arc::new(std::sync::Mutex::new(Scene::new(
        config.width,
        config.height,
    )));
    let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();

    let (user_tx, user_rx): (_, Receiver<UserEvent>) = unbounded();
    let (audio_tx, audio_rx) = unbounded();
    drop(audio_rx); // ヘッドレス: オーディオスレッドは起動しない（W2 の既存方針と同じ）

    let op_log: SharedOpLog = Arc::new(Mutex::new(Vec::new()));
    let trans_notify = Arc::new(Notify::new());

    let ctx = EngineCtx {
        scene: scene.clone(),
        proxy: WakeSink::Channel(user_tx),
        input_rx,
        trans_notify: trans_notify.clone(),
        audio_tx,
        audio_shared: Arc::new(AudioShared::default()),
        game_dir,
        config,
        start: Instant::now(),
        clock: crate::clock::Clock::Real,
        input_sync: crate::bridge::InputSync::new(),
        tick_sync: crate::bridge::TickSync::new(),
        pending_waits: crate::bridge::PendingWaits::new(),
        op_log: Some(op_log.clone()),
    };

    // ミニレンダラポンプ: 本番の描画ループ（headless.rs / win32.rs）が担っている
    // 「完了したトランジションの swap + trans_notify 通知 + 完了 tween の確定」を、
    // 合成（compose_frame）抜きで一定間隔で行う（これが無いと [wt]/[wm] がハーネス上で
    // 永久に完了しない）。停止フラグで Quit 受信後に自然消滅する。
    let pump_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let scene = scene.clone();
        let trans_notify = trans_notify.clone();
        let pump_stop = pump_stop.clone();
        std::thread::Builder::new()
            .name("noiria-tagtest-pump".into())
            .spawn(move || {
                while !pump_stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let now = Instant::now();
                    {
                        let mut scene = scene.lock().unwrap();
                        if scene
                            .transition
                            .as_ref()
                            .is_some_and(|t| t.is_finished(now))
                        {
                            scene.swap_after_transition();
                            trans_notify.notify_waiters();
                        }
                        scene.settle_finished_tweens(now);
                    }
                    std::thread::sleep(Duration::from_millis(15));
                }
            })
            .expect("ミニレンダラポンプの起動に失敗");
    }

    let deadline = Instant::now() + Duration::from_secs(10);

    // 本番と同じエントリポイント（`main.rs` が使うのと同じ `spawn` 関数）でスクリプトスレッドを
    // 起動する。JoinHandle は意図的に drop する（モジュール冒頭コメント参照）。
    let _handle = match engine {
        Engine::V8 => crate::script::spawn(ctx),
        Engine::Tsint => crate::script::tsint_engine::spawn(ctx),
    };

    // クリックは全部を一括で流し込まない: `runtime/api.ts` の `InputHub.pump()` は
    // 常設の単一ループでチャンネルを FIFO に汲み出し、その時点で登録済みのハンドラへだけ
    // 配送する（後から `[l]`/リンク待ちが `api.input.on()` する前に消費されてしまうと、
    // そのクリックは永久に失われる）。V8 isolate 生成 + `runtime/kag/*.ts` 群のトランスパイル
    // だけで実時間 200ms 前後かかることが実測でわかったため、固定の短い sleep では最初の
    // クリックがエンジン起動より先に届いてしまい取りこぼす（実機デバッグで確認済み）。
    // 代わりに `op_log` が一定時間（`QUIET_MS`）伸びなくなる＝スクリプトが次の待ちに
    // 到達して静止したとみなせるまで待ってから送る（合成イベント + 時間制御での決定論化）。
    // 「起爆」用のダミーイベントを 1 発送る。deno_core の `run_event_loop` は起動直後、
    // 外部から一度もイベントが来ないうちは「まだ処理できる同期的な続き（[l] の待ちハンドラ
    // 登録等）」があっても進まないことが実機デバッグで判明した（最初の 1 発が常に
    // 「消費」されてしまう）ため、実際のクリック列より前に捨て駒を送る。
    // 種別は `kind: "key"` + どのキーバインドにもマッチしない "F24" にする（`kind: "move"` だと
    // updateHover → linkAt → text_rects が「消費されたタイミング次第で」発行され、V8/tsint の
    // ops 列比較が非決定的になる。key はどのハンドラでも完全に無反応 = ops 無発行で決定論的）。
    let _ = input_tx.send(InputEvent {
        kind: "key",
        x: 0.0,
        y: 0.0,
        button: None,
        key: Some("F24".to_string()),
        delta: None,
        t: 0.0,
    });

    for step in steps {
        match *step {
            Step::Sleep(ms) => std::thread::sleep(Duration::from_millis(ms)),
            Step::Check(check) => check(&op_log.lock().unwrap().clone()),
            Step::Click(x, y) => {
                wait_for_quiescence(&op_log, deadline);
                let _ = input_tx.send(input_event("click", x, y, Some("left"), None));
            }
            Step::RightClick(x, y) => {
                wait_for_quiescence(&op_log, deadline);
                let _ = input_tx.send(input_event("click", x, y, Some("right"), None));
            }
            Step::Key(k) => {
                wait_for_quiescence(&op_log, deadline);
                let _ = input_tx.send(input_event("key", 0.0, 0.0, None, Some(k)));
            }
            Step::KeyNow(k) => {
                let _ = input_tx.send(input_event("key", 0.0, 0.0, None, Some(k)));
            }
            Step::ClickNow(x, y) => {
                let _ = input_tx.send(input_event("click", x, y, Some("left"), None));
            }
            Step::KeyUp(k) => {
                wait_for_quiescence(&op_log, deadline);
                let _ = input_tx.send(input_event("keyup", 0.0, 0.0, None, Some(k)));
            }
        }
    }
    // 送信済みのクリックを使い切った後は channel を閉じておく（想定より多くクリックを
    // 要求するバグを検知しやすくするため。全体のハング防止は下の deadline が担う）。
    drop(input_tx);

    loop {
        // deadline は Redraw が届き続ける経路（動画/アニメ/文字送りのループ等）でも必ず
        // 評価する（Err 分岐の中だけで見ると Redraw の連打で永久にタイムアウトしない）。
        if Instant::now() >= deadline {
            let log = op_log.lock().unwrap().clone();
            panic!(
                "フラグメント実行がタイムアウトしました（[close] 未到達。\
                 クリック数不足や無限待ちの疑い、engine={engine:?}）。\
                 ここまでの op_log ({} 件):\n{log:#?}\nフラグメント:\n{ks_body}",
                log.len()
            );
        }
        match user_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(UserEvent::Quit) => break,
            Ok(_) => {}
            Err(_) => {}
        }
    }

    pump_stop.store(true, std::sync::atomic::Ordering::Relaxed);

    // ベストエフォートのセーブディレクトリ掃除: フラグメント実行は起動時の autosave 等で
    // 実際の %APPDATA%\Noiria\<一意なゲームID>\save へ書き込む（本番と同じ経路を通す設計の
    // 副作用）。ゲーム ID はテストごとに一意なので、放置するとゴミディレクトリが蓄積する。
    if let Ok(save_dir) = noiria_core::savedata::save_dir(&id) {
        if let Some(game_root) = save_dir.parent() {
            let _ = std::fs::remove_dir_all(game_root);
        }
    }

    let final_log = op_log.lock().unwrap().clone();
    RunOutcome { log: final_log }
}

/// V8 エンジンでフラグメントを実行し、ops ログを返す。
pub(crate) fn run_v8(ks_body: &str, clicks: &[(f64, f64)]) -> Vec<String> {
    run_engine(Engine::V8, ks_body, clicks).log
}

/// tsint エンジンでフラグメントを実行し、ops ログを返す。
pub(crate) fn run_tsint(ks_body: &str, clicks: &[(f64, f64)]) -> Vec<String> {
    run_engine(Engine::Tsint, ks_body, clicks).log
}

/// 手順（クリック/キー/待ち/途中確認）つきで V8 実行し、ops ログを返す。
pub(crate) fn run_v8_steps(ks_body: &str, setup: Setup, steps: &[Step]) -> Vec<String> {
    run_engine_steps(Engine::V8, ks_body, setup, steps).log
}

/// 手順つきで tsint 実行し、ops ログを返す。
pub(crate) fn run_tsint_steps(ks_body: &str, setup: Setup, steps: &[Step]) -> Vec<String> {
    run_engine_steps(Engine::Tsint, ks_body, setup, steps).log
}

/// `text_print` の Debug 表記（`text: "..."`）に合わせた「この文字列が印字された」needle を作る。
pub(crate) fn printed(text: &str) -> String {
    format!("text: {text:?}")
}

/// ops ログの行が `needle`（部分文字列）を含む行を少なくとも 1 つ含むことを確認する。
pub(crate) fn assert_log_contains(log: &[String], needle: &str) {
    assert!(
        log.iter().any(|l| l.contains(needle)),
        "op_log に `{needle}` を含む行が見つかりません。\nop_log ({} 件):\n{log:#?}",
        log.len()
    );
}

/// ops ログの行が `needle` を含む行を持たないことを確認する（スタブ/無効化タグ等）。
pub(crate) fn assert_log_not_contains(log: &[String], needle: &str) {
    assert!(
        !log.iter().any(|l| l.contains(needle)),
        "op_log に `{needle}` を含む行が意図せず存在します。\nop_log ({} 件):\n{log:#?}",
        log.len()
    );
}

/// ops ログの行が `needle` を含む行をちょうど `n` 回持つことを確認する
/// （二重再生バグの回帰テスト等、回数そのものが観測対象の場合に使う）。
pub(crate) fn assert_log_count(log: &[String], needle: &str, n: usize) {
    let count = log.iter().filter(|l| l.contains(needle)).count();
    assert_eq!(
        count, n,
        "op_log 中 `{needle}` を含む行の件数が期待と異なります（期待 {n} 件、実際 {count} 件）。\nop_log ({} 件):\n{log:#?}",
        log.len()
    );
}

/// V8/tsint の ops ログが完全一致することを確認する（懸案だった「V8/tsint ops 列自動比較」）。
pub(crate) fn assert_logs_match(v8_log: &[String], tsint_log: &[String]) {
    assert_eq!(
        v8_log, tsint_log,
        "V8 と tsint の ops ログが一致しません。\nV8 ({} 件):\n{v8_log:#?}\ntsint ({} 件):\n{tsint_log:#?}",
        v8_log.len(),
        tsint_log.len()
    );
}
