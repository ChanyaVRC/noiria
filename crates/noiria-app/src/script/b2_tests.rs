//! ウェーブ B2（プレイヤー向けシステム UI 層）の回帰テスト。ハーネスは `tag_harness.rs`。
//!
//! メニュー/ダイアログ/常設ボタンは入力手順で駆動し、V8/tsint の両エンジンで同じ結論になることを
//! 確認する（配布版の既定エンジンは tsint）。座標は 1280x720 の設計座標（`runtime/kag/menu.ts` の
//! レイアウト定数と対応）:
//! - タブ: セーブ (130,45) / ロード (300,45) / 設定 (450,45)、閉じる (1160,45)
//! - ページ選択: 先頭ページの 1 番 (248,102)。カード i の左上は (60 + 592*(i%2), 138 + 158*(i/2))、
//!   サイズ 568x146（カード 0 は (300,200)）。削除ボタンはカード 0 で (590,158)
//! - 確認ダイアログ: はい (500,420) / いいえ (700,420)
//! - フッタ: 1 つ目 (150,670)、2 つ目 (370,670)

use super::tag_harness::{
    assert_log_contains, assert_log_count, assert_log_not_contains, printed, run_tsint_steps,
    run_v8_steps, Setup, Step,
};

fn on_both(frag: &str, setup: Setup, steps: &[Step], check: fn(&[String], &str)) {
    check(&run_v8_steps(frag, setup, steps), "v8");
    check(&run_tsint_steps(frag, setup, steps), "tsint");
}

const CLICK: Step = Step::Click(10.0, 10.0);
const SYNC: Step = Step::Key("F24");
const CARD0: Step = Step::Click(300.0, 200.0);
const YES: Step = Step::Click(500.0, 420.0);
const NO: Step = Step::Click(700.0, 420.0);

fn count(log: &[String], needle: &str) -> usize {
    log.iter().filter(|l| l.contains(needle)).count()
}

/// メッセージ窓（レイヤ 3）へ `text` が印字された回数（メニューの抜粋など他レイヤの同名テキストを数えない）。
fn msg_prints(log: &[String], text: &str) -> usize {
    log.iter()
        .filter(|l| l.starts_with("text_print 3 ") && l.contains(&printed(text)))
        .count()
}

fn pos(log: &[String], needle: &str) -> usize {
    log.iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` が無い: {log:#?}"))
}

// =====================================================================
// 純関数（両エンジンで同じ結果）
// =====================================================================

const PURE_PRELUDE: &str = r#"
import * as api from "noiria:api";
import { slotsOfPage, slotLabel, parseSlotMeta, makeExcerpt, sliderFromX, moveGrid, PAGE_COUNT } from "noiria:kag/menu";
import { layoutSystemBar, SYS_BUTTON_ORDER } from "noiria:kag/ui";
import { computeNamePlateCfg, restoreBacklog } from "noiria:kag/player";
const j = (v: unknown) => JSON.stringify(v);
api.sys.log("B2A=" + j([slotsOfPage(0), slotsOfPage(1), slotsOfPage(5), slotsOfPage(6), slotsOfPage(-1), PAGE_COUNT]));
api.sys.log("B2B=" + j([slotLabel("auto"), slotLabel("quick"), slotLabel("save3"), slotLabel("save12"), slotLabel("weird")]));
const legacy = '{"version":2,"title":"T","date":"2020/1/2 3:04:05"}';
const fresh = '{"title":"C","savedAt":"2026/11/19 12:34","excerpt":"ex","thumb":true,"date":"x"}';
api.sys.log("B2C=" + j([parseSlotMeta("save1", legacy), parseSlotMeta("save2", fresh), parseSlotMeta("save3", "{broken"), parseSlotMeta("save4", null)]));
api.sys.log("B2D=" + j([makeExcerpt("あ\nい  う", 10), makeExcerpt("0123456789ABC", 10), makeExcerpt("", 5)]));
const t = { x: 100, y: 0, w: 400, h: 10 };
api.sys.log("B2E=" + j([sliderFromX(100, t), sliderFromX(300, t), sliderFromX(500, t), sliderFromX(90, t), sliderFromX(520, t), sliderFromX(149, t)]));
api.sys.log("B2F=" + j([moveGrid(0, 6, 2, 1, 0), moveGrid(1, 6, 2, 1, 0), moveGrid(0, 6, 2, 0, 1), moveGrid(4, 6, 2, 0, 1), moveGrid(5, 6, 2, -1, 0), moveGrid(0, 2, 2, 0, 1)]));
const rs = layoutSystemBar(SYS_BUTTON_ORDER, 1224, 476);
let ok = true;
let prevRight = -1;
rs.forEach((r) => { if (r.y + r.h > 476 || r.x < prevRight || r.x < 0) ok = false; prevRight = r.x + r.w; });
api.sys.log("B2G=" + j([rs.length, ok, rs[0].x, rs[rs.length - 1].x + rs[rs.length - 1].w]));
const msg = { left: 40, top: 480, width: 1200, height: 216, radius: 14, color: [10, 12, 30, 195], fontSize: 26, marginX: 30, marginY: 18, rows: 4 };
api.sys.log("B2H=" + j([computeNamePlateCfg(msg, "アリス").color, computeNamePlateCfg(msg, "アリス", { color: [1, 2, 3, 4] }).color]));
api.sys.log("B2I=" + j([restoreBacklog(undefined), restoreBacklog("x"), restoreBacklog([{ text: "a", name: "N", voice: "v" }, { text: 1 }, null, { text: "b", rule: true }])]));
"#;

/// スロット配置・ラベル・メタ情報の抽出（旧セーブ/壊れた JSON を含む）・抜粋・スライダー・グリッド移動・
/// 常設ボタンの配置・名前欄の色・履歴の復元は、V8 と tsint で同じ結果になる。
#[test]
fn b2_pure_helpers_agree_on_both_engines() {
    let setup = Setup {
        prelude: PURE_PRELUDE,
        ..Setup::default()
    };
    on_both("A[l]", setup, &[CLICK], |log, e| {
        let want = [
            r#"B2A=[["auto","quick"],["save1","save2","save3","save4","save5","save6"],["save25","save26","save27","save28","save29","save30"],[],[],6]"#,
            r#"B2B=["AUTO","QUICK","No.03","No.12","weird"]"#,
            r#"B2C=[{"slot":"save1","used":true,"savedAt":"2020/1/2 3:04:05","chapter":"T","excerpt":"","thumb":false},{"slot":"save2","used":true,"savedAt":"2026/11/19 12:34","chapter":"C","excerpt":"ex","thumb":true},{"slot":"save3","used":false,"savedAt":"","chapter":"","excerpt":"","thumb":false},{"slot":"save4","used":false,"savedAt":"","chapter":"","excerpt":"","thumb":false}]"#,
            r#"B2D=["あ い う","0123456789…",""]"#,
            "B2E=[0,50,100,0,100,10]",
            "B2F=[1,null,2,null,4,null]",
            "B2G=[8,true,691,1224]",
            "B2H=[[38,40,70,225],[1,2,3,4]]",
            r#"B2I=[[],[],[{"text":"a","name":"N","voice":"v"},{"text":"b","name":null,"voice":null,"rule":true}]]"#,
        ];
        for w in want {
            assert!(
                log.iter().any(|l| l == &format!("log info {w}")),
                "{e}: `{w}` が出ていない: {log:#?}"
            );
        }
    });
}

const EXCERPT_PRELUDE: &str = r#"
import * as api from "noiria:api";
import { makeExcerpt, closeExcerpt, unclosedBrackets, parseSlotMeta } from "noiria:kag/menu";
import { fitExcerpt, fitWidth } from "noiria:kag/mmenu";
const j = (v: unknown) => JSON.stringify(v);
api.sys.log("BX1=" + j([
  makeExcerpt("「遠慮しとく。俺は夜型なんだ。星は夜にしか見えないからな」", 10),
  makeExcerpt("「あ」とても長い地の文が続いていく", 5),
  makeExcerpt("「彼は『星』を見て言った」と", 5),
  makeExcerpt("（そうだ。「行こう」と思った。もう迷わない）", 8),
  makeExcerpt("「短い」", 10),
  makeExcerpt("地の文だけの長い文章が続いている", 6),
  makeExcerpt("(ascii (nested) text) more", 9),
]));
api.sys.log("BX2=" + j([unclosedBrackets("「彼は『星"), unclosedBrackets("「閉じた」"), unclosedBrackets("」だけ"), unclosedBrackets("")]));
const old = JSON.stringify({ title: "T", excerpt: "「遠慮しとく。俺は夜型なんだ。星は夜にしか見え…" });
const full = JSON.stringify({ title: "T", excerpt: "「そのまま」" });
const plain = JSON.stringify({ title: "T", excerpt: "地の文の抜粋…" });
api.sys.log("BX3=" + j([
  parseSlotMeta("save1", old).excerpt,
  parseSlotMeta("save2", full).excerpt,
  parseSlotMeta("save3", plain).excerpt,
  closeExcerpt("『二重「入れ子…"),
]));
api.sys.log("BX4=" + j([
  fitWidth("あいうえお", 10, 30),
  fitWidth("あい", 10, 30),
  fitExcerpt("「あいうえおかきくけこ」と言った", 10, 70),
  fitExcerpt("短い", 10, 70),
  fitExcerpt("（「あいうえおかきくけこ」）", 10, 90),
]));
"#;

/// 抜粋を長さで切っても、開き括弧だけが残らない（閉じ括弧を … の後ろへ補う。既に閉じているものや括弧の無い
/// 地の文には足さない）。旧版が保存した「切れて閉じていない抜粋」も、読み込み時に閉じる。V8/tsint で同じ結果。
#[test]
fn b2_excerpt_closes_brackets_that_truncation_would_leave_open() {
    let setup = Setup {
        prelude: EXCERPT_PRELUDE,
        ..Setup::default()
    };
    on_both("A[l]", setup, &[CLICK], |log, e| {
        let want = [
            r#"BX1=["「遠慮しとく。俺は夜…」","「あ」とて…","「彼は『星…』」","（そうだ。「行こ…」）","「短い」","地の文だけの…","(ascii (n…))"]"#,
            r#"BX2=["』」","","",""]"#,
            r#"BX3=["「遠慮しとく。俺は夜型なんだ。星は夜にしか見え…」","「そのまま」","地の文の抜粋…","『二重「入れ子…」』"]"#,
            r#"BX4=["あい…","あい","「あいうえ…」","短い","（「あいうえ…」）"]"#,
        ];
        for w in want {
            assert!(
                log.iter().any(|l| l == &format!("log info {w}")),
                "{e}: `{w}` が出ていない: {log:#?}"
            );
        }
    });
}

// =====================================================================
// F5/F9 のトースト
// =====================================================================

/// F5/F9 は画面上にトーストを出し、約 1.2 秒で消える（[wait] 等の進行は妨げない）。保存が無ければ F9 はその旨を出す。
#[test]
fn b2_quick_save_and_load_show_toasts_that_expire() {
    on_both(
        "A[l]B[l]",
        Setup::default(),
        &[
            SYNC,
            Step::Key("F9"),
            Step::Sleep(300),
            Step::Check(|log| {
                assert_log_contains(log, &printed("クイックセーブがありません"));
                assert_log_not_contains(log, "save_write quick"); // 読むだけ（書かない）
            }),
            Step::Sleep(1500),
            Step::Key("F5"),
            Step::Sleep(300),
            Step::Check(|log| {
                assert_log_contains(log, "thumb_save quick");
                assert_log_contains(log, "save_write quick");
                assert_log_contains(log, &printed("クイックセーブしました"));
                // 前の通知は 1.2 秒で消えている（レイヤが解放されている）。
                assert!(count(log, "layer_free") >= 1, "{log:#?}");
            }),
            Step::Sleep(1500),
            Step::Key("F9"),
            Step::Sleep(300),
            Step::Check(|log| assert_log_contains(log, &printed("クイックロードしました"))),
            CLICK,
            CLICK,
        ],
        |log, e| {
            // ロード後はラベルから再生し直すので A が 2 回出る。
            assert_eq!(msg_prints(log, "A"), 2, "{e}: {log:#?}");
        },
    );
}

/// トーストは待ち（[wait]）を延ばさない・止めない。
#[test]
fn b2_toast_does_not_block_wait() {
    on_both(
        "[wait time=900]AFTER[l]",
        Setup::default(),
        &[
            SYNC,
            Step::KeyNow("F5"),
            Step::Sleep(1500),
            Step::Check(|log| assert_log_contains(log, &printed("AFTER"))),
            CLICK,
        ],
        |_, _| {},
    );
}

// =====================================================================
// システムメニュー（セーブ/ロード/削除/設定/タイトル）
// =====================================================================

/// メニュー上のセーブは、メニューを開く直前の画面を撮ったサムネイルを使う（開く時に 1 回だけ撮る）。
/// 空きスロットは確認なしで保存、使用済みスロットの上書きは確認ダイアログを挟み、「いいえ」では保存しない。
#[test]
fn b2_menu_save_uses_pre_menu_thumbnail_and_confirms_overwrite() {
    on_both(
        "A[l]",
        Setup::default(),
        &[
            SYNC,
            Step::Key("Escape"),
            CARD0, // 空きスロット: 確認なしで保存
            CARD0, // 使用済み: 確認 → いいえ
            NO,
            CARD0, // もう一度 → はい
            YES,
            Step::Key("Escape"),
            CLICK,
        ],
        |log, e| {
            assert_eq!(count(log, "thumb_capture"), 1, "{e}: メニューを開く時の 1 回だけ撮る: {log:#?}");
            assert!(pos(log, "thumb_capture") < pos(log, "save_list"), "{e}: 一覧より前に撮る");
            assert_eq!(count(log, "save_write save1"), 2, "{e}: 1 回目 + 「はい」の上書き: {log:#?}");
            assert_eq!(count(log, "thumb_save save1"), 2, "{e}");
            assert_log_contains(log, &printed("No.01 を上書きしますか？"));
            assert_log_contains(log, &printed("セーブしました"));
        },
    );
}

/// ロードタブはサムネイル付きのスロットを一覧し、確認の後でロードする（保存時のラベルから再生し直す）。
#[test]
fn b2_menu_load_lists_thumbnail_and_loads_after_confirm() {
    on_both(
        "A[l]B[l]",
        Setup::default(),
        &[
            SYNC,
            Step::Key("Escape"),
            CARD0, // save1 へ保存
            Step::Key("Escape"),
            CLICK, // A → B
            Step::Key("Escape"),
            Step::Click(300.0, 45.0), // ロードタブ
            Step::Click(248.0, 102.0), // ページ 1
            CARD0,
            YES,
            CLICK, // ロード後の A → B
            CLICK, // B → 終了
        ],
        |log, e| {
            assert_log_contains(log, "layer_load_save_thumb");
            assert_log_contains(log, "save_read save1");
            // 保存時のサムネイルが実際に読めている（読めなければ「NO IMAGE」の枠になる）。
            assert_log_not_contains(log, &printed("NO IMAGE"));
            assert_log_contains(log, &printed("No.01 をロードしますか？\n現在の進行状況は失われます"));
            assert_eq!(msg_prints(log, "A"), 2, "{e}: ロードで先頭から再生し直す: {log:#?}");
            assert_log_not_contains(log, "log error");
        },
    );
}

/// スロットの削除は確認の後で行い、サムネイルと合わせて消える（空きスロット表示へ戻る）。
#[test]
fn b2_menu_delete_slot_confirms_then_deletes() {
    on_both(
        "A[l]",
        Setup::default(),
        &[
            SYNC,
            Step::Key("Escape"),
            CARD0,
            Step::Click(590.0, 158.0), // 削除ボタン
            NO,
            Step::Click(590.0, 158.0),
            YES,
            Step::Key("Escape"),
            CLICK,
        ],
        |log, e| {
            assert_eq!(count(log, "save_delete save1"), 1, "{e}: 「いいえ」では消さない: {log:#?}");
            assert_log_contains(log, &printed("No.01 を削除しますか？"));
        },
    );
}

/// ロードするとセーブ時点（ラベル通過時点）の履歴へ戻る。ロード前に履歴を空にしておいても復元される。
#[test]
fn b2_load_restores_backlog_from_save_time() {
    let frag = "[name text=ひなた]こんにちは[p][cm]\n*second\n[save place=3]\n[if exp=\"!sf.loaded\"][eval exp=\"sf.loaded = true\"][eval exp=\"sys.player.backlog = []\"][load place=3][endif]\n[emb exp=\"'BL='+JSON.stringify(sys.player.backlog)\"][l]";
    on_both(frag, Setup::default(), &[CLICK, CLICK], |log, _| {
        assert_log_contains(log, r#"BL=[{\"text\":\"こんにちは\",\"name\":\"ひなた\",\"voice\":null}]"#);
    });
}

const LEGACY_SAVE: &str = r#"{"version":2,"pos":{"file":"test.ks","label":"start"},"f":{},"layers":[],"bgm":null,"name":null,"date":"2020/1/2 3:04:05","title":"OLD CHAPTER","msgLayers":[],"current":0}"#;

/// 旧版が書いたセーブ（保存時刻・抜粋・サムネイル・履歴が無い）も一覧に読める（メタは空欄/画像なし）。
/// ロードもでき、履歴は空で補完される。
#[test]
fn b2_legacy_save_without_meta_lists_and_loads() {
    let setup = Setup {
        slots: &[("save1", LEGACY_SAVE)],
        ..Setup::default()
    };
    on_both(
        "A[l][emb exp=\"'BL='+sys.player.backlog.length\"]",
        setup,
        &[
            SYNC,
            Step::Key("Escape"),
            Step::Click(300.0, 45.0),
            Step::Click(248.0, 102.0),
            SYNC,
            Step::Check(|log| {
                assert_log_contains(log, &printed("2020/1/2 3:04:05\n"));
                assert_log_contains(log, &printed("OLD CHAPTER\n"));
                assert_log_contains(log, &printed("NO IMAGE"));
                assert_log_not_contains(log, "layer_load_save_thumb");
            }),
            CARD0,
            YES,
            CLICK,
        ],
        |log, e| {
            assert_log_contains(log, &printed("BL=0"));
            assert_log_not_contains(log, "log error");
            assert_eq!(msg_prints(log, "A"), 2, "{e}: {log:#?}");
        },
    );
}

/// 設定タブはキーボードで操作でき（↑↓ で行、←→ で値）、窓の濃さを変えると窓が描き直される。
/// 「初期設定に戻す」は確認の後で既定へ戻す。変更は sys へ保存される。
#[test]
fn b2_settings_keyboard_adjusts_values_and_reset_restores_defaults() {
    let probe = "A[l][emb exp=\"'S='+sys.player.settings.bgmVol+','+sys.player.settings.msgOpacity+','+sys.player.settings.skipUnread\"][l]";
    on_both(
        probe,
        Setup::default(),
        &[
            SYNC,
            Step::Key("Escape"),
            Step::Key("Tab"),
            Step::Key("Tab"), // 設定タブ
            Step::Key("ArrowRight"), // BGM 90
            Step::Key("ArrowDown"),
            Step::Key("ArrowDown"),
            Step::Key("ArrowDown"),
            Step::Key("ArrowDown"),
            Step::Key("ArrowDown"), // 行 5: 未読もスキップ
            Step::Key("Enter"), // する
            Step::Key("ArrowDown"), // 行 6: 窓の濃さ
            Step::Key("ArrowLeft"), // 90
            SYNC,
            Step::Check(|log| {
                assert!(count(log, "layer_msgbox 3 fore") >= 2, "窓が描き直される: {log:#?}");
            }),
            Step::Key("Escape"),
            CLICK,
            CLICK,
        ],
        |log, e| {
            assert_log_contains(log, &printed("S=90,90,true"));
            // 最初のラベル通過の保存 + 設定変更ごとの保存。
            assert!(count(log, "save_write sys") >= 3, "{e}: {log:#?}");
        },
    );
    on_both(
        probe,
        Setup::default(),
        &[
            SYNC,
            Step::Key("Escape"),
            Step::Key("Tab"),
            Step::Key("Tab"),
            Step::Key("ArrowRight"), // BGM 90
            Step::Key("ArrowDown"),
            Step::Key("ArrowDown"),
            Step::Key("ArrowDown"),
            Step::Key("ArrowDown"),
            Step::Key("ArrowDown"),
            Step::Key("ArrowDown"),
            Step::Key("ArrowDown"), // 行 7: 初期設定に戻す
            Step::Key("Enter"),
            Step::Key("ArrowLeft"), // 確認のフォーカスを「はい」へ
            Step::Key("Enter"),
            Step::Key("Escape"),
            CLICK,
            CLICK,
        ],
        |log, _| assert_log_contains(log, &printed("S=80,100,false")),
    );
}

/// タイトルが指定されたゲームでは、メニューの「タイトルへ戻る」が確認の後にタイトルのラベルへ飛ぶ。
/// 指定が無ければその項目は出ない。
#[test]
fn b2_menu_back_to_title_jumps_after_confirm() {
    let frag = "[jump target=*body]\n*title\n[emb exp=\"'AT_TITLE'\"][l]\n*body\nBODY[l]";
    let setup = Setup {
        player_opts: r#"title: { storage: "test.ks", target: "*title" }"#,
        ..Setup::default()
    };
    on_both(
        frag,
        setup,
        &[
            SYNC,
            Step::Key("Escape"),
            Step::Click(150.0, 670.0), // タイトルへ戻る
            SYNC,
            Step::Check(|log| assert_log_contains(log, &printed("タイトルへ戻りますか？\n保存していない進行状況は失われます"))),
            YES,
            CLICK, // AT_TITLE の待ち
            CLICK, // その先の BODY
        ],
        |log, _| assert_log_contains(log, &printed("AT_TITLE")),
    );
    on_both("A[l]", Setup::default(), &[SYNC, Step::Key("Escape"), Step::Key("Escape"), CLICK], |log, _| {
        assert_log_not_contains(log, "タイトルへ戻る");
        assert_log_contains(log, "ゲームを終了");
    });
}

/// 「ゲームを終了」は確認の後で終了する。
#[test]
fn b2_menu_quit_asks_for_confirmation() {
    on_both(
        "A[l]",
        Setup::default(),
        &[SYNC, Step::Key("Escape"), Step::Click(150.0, 670.0), NO, Step::Click(150.0, 670.0), YES],
        |log, _| {
            assert_eq!(count(log, "ゲームを終了しますか？"), 2, "{log:#?}");
            assert_log_contains(log, "quit");
        },
    );
}

// =====================================================================
// オートセーブ
// =====================================================================

/// 選択肢（リンクのある [s]）が出た時点で専用スロット auto へ保存する。無効化タグで止められる。
#[test]
fn b2_choice_screen_autosaves_unless_disabled() {
    let choice = "本文[p][cm]\n[link target=*a]はい[endlink][r][link target=*b]いいえ[endlink]\n[s]\n*a\nA[l][close]\n*b\nB[l][close]";
    on_both(choice, Setup::default(), &[CLICK, SYNC, Step::Click(90.0, 525.0), CLICK], |log, e| {
        assert_log_contains(log, "save_write auto");
        assert_log_contains(log, "thumb_save auto");
        assert_eq!(count(log, "save_write auto"), 1, "{e}");
    });
    let off = format!("[autosave enabled=false]{choice}");
    on_both(&off, Setup::default(), &[CLICK, SYNC, Step::Click(90.0, 525.0), CLICK], |log, _| {
        assert_log_not_contains(log, "save_write auto");
    });
    // リンクの無い [s]（ボタン待ち等）では保存しない。
    on_both("A[s]", Setup::default(), &[SYNC, Step::Key("WindowClose")], |log, _| {
        assert_log_not_contains(log, "save_write auto");
    });
}

// =====================================================================
// 常設システムボタン
// =====================================================================

const BAR_OPT: Setup = Setup {
    player_opts: "systemButtons: true",
    extra_scenarios: &[],
    sys_json: None,
    slots: &[],
    prelude: "",
};

/// 既定（systemButtons 未指定）ではバーのレイヤを作らない。有効にすると 8 個のボタンが出る。
#[test]
fn b2_system_buttons_are_off_by_default_and_listed_when_enabled() {
    on_both("A[l]", Setup::default(), &[CLICK], |log, _| {
        assert_log_not_contains(log, &printed("Q.SAVE"));
        assert_log_count(log, "layer_new text", 1);
    });
    on_both("A[l]", BAR_OPT, &[CLICK], |log, _| {
        for label in ["Q.SAVE", "Q.LOAD", "SAVE", "LOAD", "AUTO", "SKIP", "LOG", "MENU"] {
            assert_log_contains(log, &printed(label));
        }
    });
}

/// バーのボタン上のクリックは本文送りにならない。ボタンは窓の上辺より上（y < 480）にある。
#[test]
fn b2_bar_click_does_not_advance_text_and_bar_sits_above_window() {
    on_both(
        "A[l]B[l]",
        BAR_OPT,
        &[
            SYNC,
            Step::Click(800.0, 462.0), // Q.LOAD（保存が無いのでトーストだけ）
            Step::Sleep(300),
            Step::Check(|log| {
                assert_log_contains(log, &printed("クイックセーブがありません"));
                assert_log_not_contains(log, &printed("B"));
            }),
            CLICK,
            CLICK,
        ],
        |log, e| {
            assert_log_contains(log, &printed("B"));
            // バー（z=106）の 8 個のレイヤは、すべて窓の上辺（高さ 203 の窓で top=493）より上に収まる。
            let tops: Vec<i32> = log
                .iter()
                .filter(|l| l.starts_with("layer_set_props") && l.contains(" fore ") && l.contains("z: Some(106)"))
                .map(|l| {
                    let at = l.find("y: Some(").expect("y が無い") + "y: Some(".len();
                    l[at..].split(')').next().unwrap().parse::<i32>().unwrap()
                })
                .collect();
            assert_eq!(tops.len(), 8, "{e}: {log:#?}");
            assert!(tops.iter().all(|y| y + 28 < 493), "{e}: バーが窓内に入っている: {tops:?}");
            assert!(tops.iter().all(|y| *y == 493 - 4 - 28), "{e}: 窓の 4px 上に下端: {tops:?}");
        },
    );
}

/// AUTO はクリック待ちの最中に押しても効き（待ちをやり直してオートで進む）、押下がクリック送りにならない。
#[test]
fn b2_bar_auto_works_during_click_wait() {
    on_both(
        "A[l]B[l]",
        BAR_OPT,
        &[
            SYNC,
            Step::Click(1010.0, 462.0), // AUTO
            Step::Sleep(2600),
            Step::Check(|log| assert_log_contains(log, &printed("B"))),
        ],
        |_, _| {},
    );
}

/// メッセージ窓が隠れている間（右クリック・[layopt visible=false]・[sysbar visible=false]）はバーも隠れ、
/// その位置のクリックは通常のクリック（本文送り）として扱われる。
#[test]
fn b2_bar_hides_with_message_window_and_sysbar_tag() {
    for frag in [
        "[layopt layer=message0 visible=false]A[l]B[l]",
        "[sysbar visible=false]A[l]B[l]",
    ] {
        on_both(frag, BAR_OPT, &[SYNC, Step::Click(800.0, 462.0), CLICK], |log, _| {
            // 1 回目のクリック（バーの位置）で既に B まで進んでいる = バーに奪われていない。
            assert_log_contains(log, &printed("B"));
            assert_log_not_contains(log, "クイックセーブがありません");
        });
    }
    // 右クリックで窓を隠している間は、バーの位置のクリックは「窓を戻す」だけ（AUTO は入らない）。
    on_both(
        "A[l]B[l]",
        BAR_OPT,
        &[
            SYNC,
            Step::RightClick(10.0, 10.0),
            Step::Click(1010.0, 462.0),
            Step::Sleep(2400),
            Step::Check(|log| assert_log_not_contains(log, &printed("B"))),
            CLICK,
            CLICK,
        ],
        |_, _| {},
    );
}

// =====================================================================
// メッセージ窓のスキン
// =====================================================================

/// [position] の color= / marginl,t,r,b= / frame= は cfg へ反映される（color は rgb のみ、アルファは維持）。
#[test]
fn b2_position_color_margins_and_frame_attributes() {
    let frag = "[position color=0xff8000 marginl=50 marginr=60 margint=10 marginb=20][emb exp=\"JSON.stringify(sys.player.msgLayers[0].cfg)\"][l]";
    on_both(frag, Setup::default(), &[CLICK], |log, _| {
        for n in [r#"\"marginX\":50"#, r#"\"marginR\":60"#, r#"\"marginY\":10"#, r#"\"marginB\":20"#, r#"\"color\":[255,128,0,195]"#] {
            assert_log_contains(log, n);
        }
    });
}

/// frame= は窓と一体: 右クリックの非表示に追従し、frame="" で解除（レイヤ解放）される。
/// 窓の濃さ設定は frame 画像の不透明度に掛かる。
#[test]
fn b2_message_frame_follows_visibility_opacity_and_clears() {
    let setup = Setup {
        player_opts: r#"message: { frame: "assets/solid.png" }"#,
        sys_json: Some(r#"{"config":{"msgOpacity":50}}"#),
        ..Setup::default()
    };
    on_both(
        "A[l][position frame=\"\"]B[l]",
        setup,
        &[
            SYNC,
            Step::Check(|log| {
                assert_log_contains(log, "layer_load_image 4 fore assets/solid.png");
                assert_log_contains(log, "opacity: Some(128)"); // 255 * 50%
            }),
            Step::RightClick(10.0, 10.0),
            SYNC,
            Step::Check(|log| {
                assert_log_contains(
                    log,
                    "layer_set_props 4 fore LayerProps { x: None, y: None, z: None, opacity: None, visible: Some(false) }",
                );
            }),
            Step::RightClick(10.0, 10.0),
            CLICK,
            CLICK,
        ],
        |log, _| assert_log_contains(log, "layer_free 4"),
    );
}

/// 窓の濃さ（msgOpacity）は窓色のアルファに掛かる。100% は色をそのまま使う。
#[test]
fn b2_message_opacity_scales_window_alpha() {
    let setup = Setup {
        sys_json: Some(r#"{"config":{"msgOpacity":50}}"#),
        ..Setup::default()
    };
    on_both(
        "[emb exp=\"JSON.stringify(sys.player.windowColor([10,20,30,200]))\"][l]",
        setup,
        &[CLICK],
        |log, _| assert_log_contains(log, "[10,20,30,100]"),
    );
    on_both(
        "[emb exp=\"JSON.stringify(sys.player.windowColor([10,20,30,200]))\"][l]",
        Setup::default(),
        &[CLICK],
        |log, _| assert_log_contains(log, "[10,20,30,200]"),
    );
}

/// 名前欄の文字色は KagOptions.namePlate.textColor。未指定は従来どおり（色なし = 既定色）。
#[test]
fn b2_name_plate_text_color_option() {
    let setup = Setup {
        player_opts: "namePlate: { textColor: [1, 2, 3, 255] }",
        ..Setup::default()
    };
    on_both("[name text=アリス]A[l]", setup, &[CLICK], |log, _| {
        assert_log_contains(log, "color: Some([1, 2, 3, 255])");
    });
    on_both("[name text=アリス]A[l]", Setup::default(), &[CLICK], |log, _| {
        assert_log_not_contains(log, "color: Some([1, 2, 3, 255])");
    });
}

// =====================================================================
// バックログ
// =====================================================================

/// 履歴には記録時の [name]（話者名）とボイスのパスが入る。
#[test]
fn b2_backlog_records_speaker_name_and_voice() {
    let frag = "[name text=ひなた][playvoice storage=\"v1.ogg\"]こんにちは[p][cm][name]地の文[p][cm][emb exp=\"JSON.stringify(sys.player.backlog)\"][l]";
    on_both(frag, Setup::default(), &[CLICK, CLICK, CLICK], |log, _| {
        assert_log_contains(log, r#"\"text\":\"こんにちは\",\"name\":\"ひなた\",\"voice\":\"assets/voice/v1.ogg\""#);
        assert_log_contains(log, r#"\"text\":\"地の文\",\"name\":null,\"voice\":null"#);
    });
}

/// 履歴のボイスつきの行はクリックで再生される（閉じない）。それ以外のクリックで閉じる。
#[test]
fn b2_backlog_click_replays_voice_and_blank_click_closes() {
    let frag = "[name text=ひなた][playvoice storage=\"v1.ogg\"]こんにちは[p][cm][showhistory]AFTER[l]";
    on_both(
        frag,
        Setup::default(),
        &[
            CLICK, // [p]
            SYNC,
            Step::Click(95.0, 112.0), // 履歴の本文の 1 文字目 → ボイス再生
            SYNC,
            Step::Check(|log| {
                assert_eq!(count(log, "audio_play voice assets/voice/v1.ogg"), 2, "{log:#?}");
                assert_log_not_contains(log, &printed("AFTER"));
            }),
            Step::Click(1000.0, 500.0), // 余白 → 閉じる
            CLICK,
        ],
        |log, _| assert_log_contains(log, &printed("AFTER")),
    );
}

/// 履歴は ↑↓ でスクロールでき、見出しに位置（末尾の件番号/総数）が出る。
#[test]
fn b2_backlog_keys_scroll_and_header_shows_position() {
    let mut frag = String::new();
    for i in 0..12 {
        frag.push_str(&format!("L{i}[p][cm]\n"));
    }
    frag.push_str("[showhistory]\nDONE[l]");
    let clicks: Vec<Step> = (0..12).map(|_| CLICK).collect();
    let mut steps: Vec<Step> = clicks;
    steps.extend([
        SYNC,
        Step::Key("ArrowUp"),
        Step::Key("ArrowUp"),
        SYNC,
        Step::Check(|log| {
            // 全角空白は Debug 表記で \u{3000} になる。
            assert_log_contains(log, r"履歴\u{3000}12 / 12");
            assert_log_contains(log, r"履歴\u{3000}11 / 12");
            assert_log_contains(log, r"履歴\u{3000}10 / 12");
        }),
        Step::Key("PageDown"),
        Step::Key("Escape"),
        CLICK,
    ]);
    on_both(&frag, Setup::default(), &steps, |log, _| assert_log_contains(log, &printed("DONE")));
}

// =====================================================================
// 選択肢の操作性
// =====================================================================

const LINKS: &str = "[link target=*a]あ[endlink][r][link target=*b]い[endlink]
[s]
*a
[emb exp=\"'GOT_A'\"][l][close]
*b
[emb exp=\"'GOT_B'\"][l][close]";

/// ↑↓ でリンクを選び Enter で決定できる（端はループ）。
#[test]
fn b2_links_are_selectable_by_keyboard() {
    on_both(LINKS, Setup::default(), &[SYNC, Step::Key("ArrowDown"), Step::Key("ArrowDown"), Step::Key("Enter"), CLICK], |log, _| {
        assert_log_contains(log, &printed("GOT_B"));
        assert_log_not_contains(log, &printed("GOT_A"));
    });
    on_both(LINKS, Setup::default(), &[SYNC, Step::Key("ArrowDown"), Step::Key("ArrowDown"), Step::Key("ArrowUp"), Step::Key("Enter"), CLICK], |log, _| {
        assert_log_contains(log, &printed("GOT_A"));
    });
    // 未選択で ↑ は末尾から。
    on_both(LINKS, Setup::default(), &[SYNC, Step::Key("ArrowUp"), Step::Key("Enter"), CLICK], |log, _| {
        assert_log_contains(log, &printed("GOT_B"));
    });
}

/// sound.hover は選択の移動で、sound.click は決定で鳴る。未指定なら hover は鳴らず click は従来の click.wav。
#[test]
fn b2_link_sounds_follow_sound_option() {
    let setup = Setup {
        player_opts: r#"sound: { hover: "h.wav", click: "c.wav" }"#,
        ..Setup::default()
    };
    on_both(LINKS, setup, &[SYNC, Step::Key("ArrowDown"), Step::Key("Enter"), CLICK], |log, _| {
        assert_eq!(count(log, "audio_play se assets/se/h.wav"), 1, "{log:#?}");
        assert_eq!(count(log, "audio_play se assets/se/c.wav"), 1, "{log:#?}");
        assert_log_not_contains(log, "assets/se/click.wav");
    });
    on_both(LINKS, Setup::default(), &[SYNC, Step::Key("ArrowDown"), Step::Key("Enter"), CLICK], |log, _| {
        assert_eq!(count(log, "audio_play se"), 1, "hover は鳴らさない: {log:#?}");
        assert_log_contains(log, "audio_play se assets/se/click.wav");
    });
}
