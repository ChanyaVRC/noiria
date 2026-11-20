//! ウェーブ B3（モダン UI: 市販 ADV 風のプレイヤー向け UI）の回帰テスト。ハーネスは `tag_harness.rs`。
//!
//! モダン UI は `KagOptions.ui = "modern"` のオプトイン。ここでは「新しい描画プリミティブ（panel /
//! 拡縮つき画像読み込み / 見える幅の tween `cropTo`）が V8/tsint で同じ ops 列になる」「選択肢がボタンとして出て
//! クリックで選べる」「専用タイトルの項目で進む」「ギャラリーが sf のフラグで解放を出し分ける」
//! 「メニューのカード/サムネイル寸法」を両エンジンで確認する。座標は 1280x720 の設計座標
//! （`runtime/kag/mhud.ts` / `mtitle.ts` / `mmenu.ts` の定数と対応）:
//! - 選択肢ボタン: 幅 720 x 高さ 64、間隔 16、中心 y=322 に縦並び。2 択は y=282 / 362（x=640）
//! - タイトル項目: x=104.. の縦並び、1 つ目 (200,372)、2 つ目 (200,434)（間隔 62）
//! - セーブ一覧のカード 0 は (72,164) 起点の 360x262（中心 (252,295)）

use super::tag_harness::{
    assert_log_contains, assert_log_count, assert_log_not_contains, printed, run_tsint_steps,
    run_v8_steps, Setup, Step,
};

fn on_both(frag: &str, setup: Setup, steps: &[Step], check: fn(&[String], &str)) {
    check(&run_v8_steps(frag, setup, steps), "v8");
    check(&run_tsint_steps(frag, setup, steps), "tsint");
}

/// 次へ送るクリック。モダン UI のクリック待ち記号は一定間隔で ops を出し続けて静止しないので、
/// `Step::Click`（静止待ち）ではなく `ClickNow` を、直前の `Sleep` と組み合わせて使う。
const NEXT: Step = Step::ClickNow(10.0, 10.0);
const MODERN: &str = r#"ui: "modern","#;

// =====================================================================
// 新しい ops（panel / loadImageScaled / toCropW・cropW）
// =====================================================================

const PRIMITIVES_PRELUDE: &str = r#"
import * as api from "noiria:api";
const id = api.layer.create("image");
api.layer.panel(id, { w: 40, h: 12, radius: 4, top: [10, 20, 30, 0], bottom: [10, 20, 30, 200], border: [255, 255, 255, 80], borderW: 1, shadow: { blur: 6, dy: 2, alpha: 0.4 }, pad: 8, ramp: 6 }, "fore");
const img = api.layer.create("image");
const size = api.layer.loadImageScaled(img, "assets/solid.png", 8, 6, "fore");
api.sys.log("B3SIZE=" + JSON.stringify(size));
api.layer.set(id, { x: 5, y: 5, visible: true }, "fore");
api.layer.setCrop(id, 0, "fore");
api.layer.move(id, { toX: 9, toOpacity: 200, timeMs: 400, accel: -2 }, "fore");
api.layer.cropTo(id, 30, 400, -2, "fore");
try {
  api.layer.loadImageScaled(img, "assets/solid.png", 0, 6, "fore");
  api.sys.log("B3ZERO=ok");
} catch (e) {
  api.sys.log("B3ZERO=err");
}
"#;

/// `layer.panel`（影の pad 込みの寸法）・`loadImageScaled`（寸法 0 は拒否）・`setCrop`/`cropTo` は、V8 と tsint で
/// 同じ ops 列（寸法・引数）になる。
#[test]
fn b3_new_layer_primitives_agree_on_both_engines() {
    let setup = Setup {
        prelude: PRIMITIVES_PRELUDE,
        ..Setup::default()
    };
    let check: fn(&[String], &str) = |log, e| {
        assert_log_contains(log, "layer_panel");
        assert!(
            log.iter().any(|l| l.contains("layer_panel") && l.contains("40x12") && l.contains("pad=8")),
            "{e}: panel の寸法/pad が ops に出ていない: {log:#?}"
        );
        assert_log_contains(log, "layer_load_image_scaled");
        assert!(
            log.iter().any(|l| l == "log info B3SIZE=[8,6]"),
            "{e}: 拡縮後の寸法が返っていない: {log:#?}"
        );
        assert!(
            log.iter().any(|l| l == "log info B3ZERO=err"),
            "{e}: 寸法 0 は拒否される: {log:#?}"
        );
        assert!(
            log.iter().any(|l| l == "layer_set_crop 1 fore 0"),
            "{e}: setCrop が届いていない: {log:#?}"
        );
        assert!(
            log.iter()
                .any(|l| l.contains("layer_crop_move") && l.contains("to=30") && l.contains("time_ms=400")),
            "{e}: cropTo が届いていない: {log:#?}"
        );
    };
    check(&run_v8_steps("A[l]", setup, &[Step::Sleep(500), NEXT]), "v8");
    check(&run_tsint_steps("A[l]", setup, &[Step::Sleep(500), NEXT]), "tsint");
}

// =====================================================================
// 純関数（両エンジンで同じ結果）
// =====================================================================

const PURE_PRELUDE: &str = r#"
import * as api from "noiria:api";
import { cleanChoiceLabel, estimateWidth } from "noiria:kag/mui";
import { bandPanel, BAND_H, COL_W, BAND_FONT } from "noiria:kag/mhud";
const j = (v: unknown) => JSON.stringify(v);
api.sys.log("B3A=" + j([cleanChoiceLabel("　▶ ひなたと一緒に配る "), cleanChoiceLabel("  ▷ 先輩"), cleanChoiceLabel("そのまま"), cleanChoiceLabel("　▶ ")]));
api.sys.log("B3B=" + j([estimateWidth("", 20), estimateWidth("ab", 10), estimateWidth("AB", 10), estimateWidth("あい", 10)]));
api.sys.log("B3C=" + j([bandPanel([6, 9, 24, 236]), BAND_H, COL_W, BAND_FONT]));
"#;

/// 選択肢の装飾（先頭の空白・▶）の除去、描画幅の見積もり、帯のパネル指定は両エンジンで同じ。
#[test]
fn b3_pure_helpers_agree_on_both_engines() {
    let setup = Setup {
        prelude: PURE_PRELUDE,
        ..Setup::default()
    };
    let check: fn(&[String], &str) = |log, e| {
        let want = [
            r#"B3A=["ひなたと一緒に配る","先輩","そのまま",""]"#,
            "B3B=[0,11,13,20]",
            r#"B3C=[{"top":[6,9,24,0],"bottom":[6,9,24,236],"ramp":128},320,880,26]"#,
        ];
        for w in want {
            assert!(
                log.iter().any(|l| l == &format!("log info {w}")),
                "{e}: `{w}` が出ていない: {log:#?}"
            );
        }
    };
    check(&run_v8_steps("A[l]", setup, &[Step::Sleep(500), NEXT]), "v8");
    check(&run_tsint_steps("A[l]", setup, &[Step::Sleep(500), NEXT]), "tsint");
}

// =====================================================================
// 会話画面の帯・選択肢ボタン
// =====================================================================

/// modern では本文窓が下部の全幅の帯（グラデーションのパネル）になり、箱型の `layer_msgbox` は本文窓に使われない。
#[test]
fn b3_modern_message_window_is_a_full_width_band_panel() {
    on_both(
        "A[l]",
        Setup {
            player_opts: MODERN,
            ..Setup::default()
        },
        &[Step::Sleep(600), NEXT],
        |log, e| {
            assert!(
                log.iter().any(|l| l.contains("layer_panel") && l.contains("1280x320")),
                "{e}: 全幅 x 320 の帯が描かれていない: {log:#?}"
            );
            assert_log_not_contains(log, "layer_msgbox");
        },
    );
}

const CHOICE_BODY: &str =
    "ASK[r]\n[link target=*a]\u{3000}▶ 左のえらび[endlink][r]\n[link target=*b]\u{3000}▶ 右のえらび[endlink]\n[s]\n*a\nGOT_A[l]\n[jump target=*z]\n*b\nGOT_B[l]\n*z";

/// choices.style = "buttons"（modern 既定）: `[link]` の文字は窓に見せず（透明色で印字）、画面中央のボタンの
/// クリックで選べる。2 択のボタン中心は y=282 / 362。
#[test]
fn b3_choice_buttons_hide_link_text_and_pick_by_click() {
    on_both(
        CHOICE_BODY,
        Setup {
            player_opts: MODERN,
            ..Setup::default()
        },
        &[
            Step::Sleep(1300),
            Step::Check(|log| {
                // 本文の「ASK」は見えるが、リンクの文字は完全透明で印字される
                assert!(
                    log.iter().any(|l| l.contains("text_print") && l.contains(&printed("ASK"))),
                    "{log:#?}"
                );
                assert!(
                    log.iter().any(|l| l.contains("text_print")
                        && l.contains("左のえらび")
                        && l.contains("color: Some([0, 0, 0, 0])")),
                    "リンク文字は透明色で印字される: {log:#?}"
                );
                // ボタンの文字は装飾（先頭の全角空白・▶）なしで別レイヤへ出る
                assert!(
                    log.iter().any(|l| l.contains("text_print") && l.contains(&printed("左のえらび"))),
                    "{log:#?}"
                );
            }),
            Step::ClickNow(640.0, 362.0),
            Step::Sleep(1000),
            NEXT,
        ],
        |log, e| {
            assert_log_contains(log, &printed("GOT_B"));
            assert_log_not_contains(log, &printed("GOT_A"));
            assert!(log.iter().any(|l| l.contains("layer_panel") && l.contains("720x64")), "{e}: {log:#?}");
        },
    );
}

/// ボタンの外側（本文の帯の上など）のクリックでは選ばれない。↓ キー → Enter で 1 つ目を選べる。
#[test]
fn b3_choice_buttons_ignore_stray_clicks_and_support_keyboard() {
    on_both(
        CHOICE_BODY,
        Setup {
            player_opts: MODERN,
            ..Setup::default()
        },
        &[
            Step::Sleep(1300),
            Step::ClickNow(1150.0, 600.0),
            Step::ClickNow(60.0, 360.0),
            Step::Sleep(300),
            Step::Check(|log| {
                assert_log_not_contains(log, &printed("GOT_A"));
                assert_log_not_contains(log, &printed("GOT_B"));
            }),
            Step::KeyNow("ArrowDown"),
            Step::Sleep(200),
            Step::KeyNow("Enter"),
            Step::Sleep(1000),
            NEXT,
        ],
        |log, _e| {
            assert_log_contains(log, &printed("GOT_A"));
            assert_log_not_contains(log, &printed("GOT_B"));
        },
    );
}

/// 既定（classic）は従来どおり本文中の ▶ 行のまま（透明化もボタンも出ない）。
#[test]
fn b3_classic_choices_stay_inline_by_default() {
    on_both(
        CHOICE_BODY,
        Setup::default(),
        &[Step::Sleep(600), Step::ClickNow(160.0, 560.0), Step::Sleep(300), NEXT],
        |log, e| {
            assert_log_not_contains(log, "color: Some([0, 0, 0, 0])");
            assert_log_not_contains(log, "720x64");
            assert!(log.iter().any(|l| l.contains(&printed("GOT_A"))), "{e}: インラインのリンクは従来どおり選べる");
        },
    );
}

// =====================================================================
// 専用タイトル画面・ギャラリー
// =====================================================================

const TITLE_OPTS: &str = r#"ui: "modern", titleScreen: { name: "T", menu: [{ label: "START", sub: "s", target: "*go" }, { label: "CONTINUE", sub: "c", action: "continue" }] },"#;

/// `[titlescreen]`: メニューの項目（START）をクリックするとその `target` へ進む。項目はメッセージ窓を使わない
/// 専用レイアウト（テキストレイヤ）で、CONTINUE はセーブが無いと無効（クリックしても何も起きない）。
#[test]
fn b3_titlescreen_start_item_jumps_and_continue_is_disabled_without_saves() {
    on_both(
        "[titlescreen]\n*go\nSTARTED[l]",
        Setup {
            player_opts: TITLE_OPTS,
            ..Setup::default()
        },
        &[
            Step::Sleep(2200),
            Step::ClickNow(200.0, 434.0), // CONTINUE（セーブ無しで無効）
            Step::Sleep(600),
            Step::Check(|log| {
                assert_log_not_contains(log, "thumb_capture"); // 読み込みメニューは開かない
                assert_log_not_contains(log, &printed("STARTED"));
            }),
            Step::ClickNow(200.0, 372.0), // START
            Step::Sleep(1200),
            NEXT,
        ],
        |log, e| {
            assert_log_contains(log, &printed("STARTED"));
            assert!(
                log.iter().any(|l| l.contains(&printed("T")) && l.contains("size: Some(80.0)")),
                "{e}: ロゴ文字が大きく出る: {log:#?}"
            );
        },
    );
}

/// セーブがあれば CONTINUE が有効になり、クリックでロードメニュー（一覧の読み込み）が開く。
#[test]
fn b3_titlescreen_continue_opens_load_menu_when_a_save_exists() {
    let save = r#"{"version":2,"pos":{"file":"test.ks","label":"start"},"f":{},"layers":[],"bgm":null,"name":null,"date":"d","title":"T","savedAt":"2026/01/01 00:00","excerpt":"x","thumb":false}"#;
    on_both(
        "[titlescreen]\n*go\nSTARTED[l]",
        Setup {
            player_opts: TITLE_OPTS,
            slots: &[("save1", save)],
            ..Setup::default()
        },
        &[
            Step::Sleep(2200),
            Step::ClickNow(200.0, 434.0),
            Step::Sleep(1000),
            Step::Check(|log| {
                assert_log_contains(log, "thumb_capture"); // ロードメニューを開く直前の撮影
                assert_log_contains(log, "layer_panel");
            }),
            Step::KeyNow("Escape"),
            Step::Sleep(700),
            Step::ClickNow(200.0, 372.0),
            Step::Sleep(1200),
            NEXT,
        ],
        |log, _e| {
            assert_log_contains(log, &printed("STARTED"));
        },
    );
}

const GALLERY_OPTS: &str = r#"ui: "modern", gallery: [{ flag: "endA", image: "assets/solid.png", title: "ENDA" }, { flag: "endB", image: "assets/solid.png", title: "ENDB" }],"#;

/// ギャラリー: sf のフラグが真の枠だけ CG（拡縮つきの画像）が読まれ、偽の枠は読まれない（ロック表示）。
#[test]
fn b3_gallery_loads_images_only_for_unlocked_flags() {
    on_both(
        "[eval exp=\"sys.gallery()\"]\nW[l]",
        Setup {
            player_opts: GALLERY_OPTS,
            sys_json: Some(r#"{"endA":true,"readLabels":{}}"#),
            ..Setup::default()
        },
        &[
            Step::Sleep(1300),
            Step::Check(|log| {
                assert_log_count(log, "layer_load_image_scaled", 1);
                assert!(
                    log.iter()
                        .any(|l| l.contains("layer_load_image_scaled") && l.contains("assets/solid.png") && l.contains("336x189")),
                    "解放済みのサムネイルは 336x189 へ拡縮される: {log:#?}"
                );
                assert_log_contains(log, &printed("ENDA"));
                assert_log_not_contains(log, &printed("ENDB")); // 未解放の名前は出さない（？？？）
            }),
            Step::KeyNow("Escape"),
            Step::Sleep(700),
            NEXT,
        ],
        |log, _e| {
            assert_log_contains(log, &printed("W"));
        },
    );
}

// =====================================================================
// メニュー（セーブ一覧のカード・大きいサムネイル・設定）
// =====================================================================

/// modern のセーブ一覧: 3 列 x 2 行のカード。空きスロット（カード 0）のクリックで保存され、サムネイルは大きめ
/// （384x216）に撮って保存し、カードには 336x189 へ拡縮して読む。
#[test]
fn b3_modern_save_menu_cards_and_large_thumbnail() {
    on_both(
        "A[l]",
        Setup {
            player_opts: MODERN,
            ..Setup::default()
        },
        &[
            Step::Sleep(600),
            Step::KeyNow("Escape"),
            Step::Sleep(1000),
            Step::ClickNow(252.0, 295.0),
            Step::Sleep(700),
            Step::Check(|log| {
                assert_log_contains(log, "thumb_capture 384x216");
                assert_log_contains(log, "thumb_save save1");
                assert_log_contains(log, "save_write save1");
                assert_log_contains(log, &printed("セーブしました"));
            }),
            Step::KeyNow("Escape"),
            Step::Sleep(700),
            NEXT,
        ],
        |log, e| {
            // 保存後にカードのサムネイルを拡縮して読み直している
            assert!(
                log.iter().any(|l| l.contains("layer_load_save_thumb") && l.contains("save1 336x189")),
                "{e}: {log:#?}"
            );
        },
    );
}

/// 設定: 右カラムの「文字の速さ」セグメント（4 つ）の 3 つ目（速い）をクリックすると cps が 60 になり、
/// メニューを閉じたあとに出る本文はその速さで文字送りされる。
#[test]
fn b3_modern_settings_segmented_control_changes_value() {
    on_both(
        "A[l]\n[endnowait]\nB[l]",
        Setup {
            player_opts: MODERN,
            ..Setup::default()
        },
        &[
            Step::Sleep(600),
            Step::KeyNow("Escape"),
            Step::Sleep(800),
            Step::ClickNow(996.0, 72.0), // CONFIG タブ
            Step::Sleep(1000),
            Step::ClickNow(1078.0, 224.0), // 文字の速さ: 速い
            Step::Sleep(400),
            Step::KeyNow("Escape"),
            Step::Sleep(700),
            NEXT, // A を送る → B が新しい速さで出る
            Step::Sleep(800),
            NEXT,
        ],
        |log, e| {
            assert!(
                log.iter().any(|l| l.contains("text_print") && l.contains(&printed("B")) && l.contains("cps=60")),
                "{e}: 設定を変えた速さで本文が出る: {log:#?}"
            );
        },
    );
}

const THUMB_SAVE: &str = r#"{"version":2,"pos":{"file":"test.ks","label":"start"},"f":{},"layers":[],"bgm":null,"name":null,"date":"d","title":"T","savedAt":"2026/01/01 00:00","excerpt":"x","thumb":true}"#;

/// セーブ一覧は、カードの枠・文字・サムネイルを全部作り終えてから一括で現す（できた順に見えると、枠だけ・画像だけが
/// 先に出て「重い画面」に見える）。サムネイルの読み込み中は表示の tween（不透明度 255 へ）を始めず、
/// 読み終えたあとの最初の表示開始より後ろに、パネルの描画・文字の組み・画像の読み込みを残さない。
#[test]
fn b3_modern_save_menu_builds_hidden_then_reveals_everything_at_once() {
    on_both(
        "A[l]",
        Setup {
            player_opts: MODERN,
            slots: &[("save1", THUMB_SAVE), ("save2", THUMB_SAVE), ("save3", THUMB_SAVE)],
            ..Setup::default()
        },
        &[
            Step::Sleep(600),
            Step::KeyNow("Escape"),
            Step::Sleep(1000),
            Step::KeyNow("Escape"),
            Step::Sleep(700),
            NEXT,
        ],
        |log, e| {
            // 中身は即時に現れる（フェード無し）: 不透明度 255 を直接置く `layer_set_props` が「現す」操作
            let is_reveal = |l: &String| l.contains("layer_set_props") && l.contains("opacity: Some(255)");
            let loads: Vec<usize> = log
                .iter()
                .enumerate()
                .filter(|(_, l)| l.contains("layer_load_save_thumb") && l.contains("336x189"))
                .map(|(i, _)| i)
                .collect();
            assert!(loads.len() >= 3, "{e}: 使用中のスロットのサムネイルを読む: {log:#?}");
            let (first, last) = (loads[0], *loads.last().unwrap());
            assert!(
                !log[first..last].iter().any(is_reveal),
                "{e}: サムネイルを読み込んでいる間に表示を始めていない: {log:#?}"
            );
            let first_reveal = last
                + log[last..]
                    .iter()
                    .position(is_reveal)
                    .unwrap_or_else(|| panic!("{e}: 読み込み後に表示を始める: {log:#?}"));
            assert!(
                !log[first_reveal..].iter().any(|l| {
                    l.contains("layer_panel") || l.contains("layer_load_save_thumb") || (l.contains("text_print") && !l.contains("append=true"))
                }),
                "{e}: 表示を始めた後に作り直しが残っていない: {log:#?}"
            );
            let reveals = log[first_reveal..].iter().filter(|l| is_reveal(l)).count();
            assert!(reveals >= 12, "{e}: 6 枚ぶんのカード面とページ送りが一緒に現れる: {reveals}");
        },
    );
}

/// 履歴は開くときに最新の 40 件だけを組み（全件を組むと 200 件で開くのが重い）、上へ読み進めたら（ここでは Home）
/// 古い履歴を足して組み直す。足したあとは先頭の履歴まで含む。
#[test]
fn b3_modern_backlog_lays_out_only_the_latest_entries_until_you_scroll_up() {
    let mut frag = String::new();
    for i in 0..100 {
        frag.push_str(&format!("LN{i:03}[p][cm]\n"));
    }
    frag.push_str("[showhistory]\nDONE[l]");
    let mut steps = vec![Step::Sleep(600)];
    for _ in 0..100 {
        steps.push(NEXT);
        steps.push(Step::Sleep(40));
    }
    steps.extend([
        Step::Sleep(800),
        Step::Check(|log| {
            let prints: Vec<&String> = log.iter().filter(|l| l.contains("text_print") && l.contains("locate: Some")).collect();
            assert_eq!(prints.len(), 1, "開いた直後は 1 回だけ組む: {prints:#?}");
            assert!(prints[0].contains("LN099") && prints[0].contains("LN060"), "最新の 40 件を含む");
            assert!(!prints[0].contains("LN059"), "それより古い履歴はまだ組まない");
        }),
        Step::KeyNow("Home"),
        Step::Sleep(500),
        Step::KeyNow("Escape"),
        Step::Sleep(500),
        NEXT,
    ]);
    on_both(&frag, Setup { player_opts: MODERN, ..Setup::default() }, &steps, |log, e| {
        let prints: Vec<&String> = log.iter().filter(|l| l.contains("text_print") && l.contains("locate: Some")).collect();
        assert_eq!(prints.len(), 2, "{e}: Home で古い履歴を足して組み直す: {}", prints.len());
        assert!(prints[1].contains("LN000") && prints[1].contains("LN099"), "{e}: 組み直しは全履歴を含む");
    });
}
