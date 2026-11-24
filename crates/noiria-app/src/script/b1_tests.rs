//! ウェーブ B1（ランタイムの出荷品質化）の回帰テスト。ハーネスは `tag_harness.rs`。
//!
//! 入力手順（キー押下/離し・右クリック・ウィンドウを閉じる操作）や「メニュー表示中の待ち」を
//! 含むため、`Step` 列で駆動する。すべて V8/tsint の両エンジンで同じ結論になることを確認する
//! （配布版の既定エンジンは tsint）。

use super::tag_harness::{
    assert_log_contains, assert_log_count, assert_log_not_contains, printed, run_tsint_steps,
    run_v8_steps, Setup, Step,
};

/// 両エンジンで同じ手順を実行し、それぞれのログに `check` を適用する（第 2 引数はエンジン名）。
fn on_both(frag: &str, setup: Setup, steps: &[Step], check: fn(&[String], &str)) {
    check(&run_v8_steps(frag, setup, steps), "v8");
    check(&run_tsint_steps(frag, setup, steps), "tsint");
}

const CLICK: Step = Step::Click(10.0, 10.0);
/// 無害なキー（どのバインドにも当たらない）。静止を待つ同期点として使う。
const SYNC: Step = Step::Key("F24");

fn count(log: &[String], needle: &str) -> usize {
    log.iter().filter(|l| l.contains(needle)).count()
}

// =====================================================================
// 1. sf の保存漏れ / WindowClose
// =====================================================================

/// 最後のラベル以降に [eval] で立てた sf のフラグが、次のクリック待ちに入った時点で保存される。
#[test]
fn b1_sf_changed_after_last_label_is_saved_when_entering_click_wait() {
    let frag = r#"[eval exp="sf.ending = 1"]A[l]"#;
    on_both(
        frag,
        Setup::default(),
        &[
            SYNC,
            Step::Check(|log| {
                // ラベル通過時の 1 回 + クリック待ちへ入る時の 1 回。
                assert_eq!(count(log, "save_write sys"), 2, "{log:#?}");
            }),
            CLICK,
        ],
        |_, _| {},
    );
}

/// sf が変わっていなければクリック待ちのたびにディスクへ書かない（dirty 判定）。
#[test]
fn b1_sys_is_not_rewritten_when_sf_is_unchanged() {
    on_both(
        "A[l]B[l]C[l]D[l]",
        Setup::default(),
        &[CLICK, CLICK, CLICK, CLICK],
        |log, e| {
            // ラベル通過の 1 回と、[close] で区間を既読にする 1 回だけ。
            assert_eq!(count(log, "save_write sys"), 2, "{e}: {log:#?}");
        },
    );
}

/// 別シナリオへ jump した時点で、読み終えた区間を既読にして sf を保存する（読み込みの後に保存）。
#[test]
fn b1_jump_to_another_scenario_marks_read_and_saves_sys() {
    let setup = Setup {
        extra_scenarios: &[("next.ks", "*n\nNEXT[l]\n[close]\n")],
        ..Setup::default()
    };
    on_both(
        "A[l][jump storage=\"next.ks\" target=*n]",
        setup,
        &[CLICK, CLICK],
        |log, e| {
            // [jump] のメッセージ消去の後、遷移先の最初の文を出す前に sys が保存される。
            let clear = log.iter().position(|l| l == "text_clear 3").unwrap();
            let next = log
                .iter()
                .position(|l| l.contains(&printed("NEXT")))
                .unwrap_or_else(|| panic!("{e}: {log:#?}"));
            assert!(
                log[clear..next].iter().any(|l| l == "save_write sys"),
                "{e}: シナリオ遷移で sys が保存されていない: {log:#?}"
            );
        },
    );
}

/// ウィンドウを閉じる操作（key:"WindowClose"）は、[s] 待ち中でも sf を保存して終了する。
#[test]
fn b1_window_close_saves_sf_and_quits_while_waiting_at_s() {
    let frag = r#"[link exp="sf.late = 1"]はい[endlink][s]"#;
    on_both(
        frag,
        Setup::default(),
        // リンクの exp で [s] 待ち「中」に sf を変える → 閉じる操作。
        &[Step::Click(90.0, 525.0), Step::Key("WindowClose")],
        |log, e| {
            assert_eq!(
                count(log, "save_write sys"),
                2,
                "{e}: ラベル通過 + ウィンドウを閉じる時の保存: {log:#?}"
            );
            assert_log_contains(log, "quit");
        },
    );
}

/// メニュー表示中でも、[wait] 中でも、ウィンドウを閉じる操作で終了できる。
#[test]
fn b1_window_close_works_in_menu_and_during_wait() {
    on_both(
        "[wait time=60000]",
        Setup::default(),
        &[Step::Key("WindowClose")],
        |log, _| assert_log_contains(log, "quit"),
    );
    on_both(
        "A[l]",
        Setup::default(),
        &[SYNC, Step::Key("Escape"), Step::Key("WindowClose")],
        |log, _| {
            assert_log_contains(log, "save_list");
            assert_log_contains(log, "quit");
        },
    );
}

// =====================================================================
// 3. Ctrl 長押しスキップ
// =====================================================================

const SKIP_PROBE: &str = r#"[clickskip enabled=false]A[l][emb exp="'X1='+sys.player.mode.skip"][waitclick][emb exp="'X2='+sys.player.mode.skip"][waitclick]"#;

/// Control を押した瞬間にクリック待ちを抜けてスキップへ入り、離すと解除される。
#[test]
fn b1_ctrl_hold_skips_and_release_stops() {
    on_both(
        SKIP_PROBE,
        Setup::default(),
        &[
            SYNC,
            Step::Key("Control"),
            Step::KeyUp("Control"),
            CLICK,
            CLICK,
        ],
        |log, _| {
            assert_log_contains(log, &printed("X1=true"));
            assert_log_contains(log, &printed("X2=false"));
            assert_log_not_contains(log, &printed("X2=true"));
        },
    );
}

/// S キーのトグルでスキップ中だった場合、Control を離してもスキップは続く（従来どおり S が主導）。
#[test]
fn b1_ctrl_release_keeps_skip_toggled_by_s() {
    on_both(
        SKIP_PROBE,
        Setup::default(),
        &[
            SYNC,
            Step::Key("s"),
            Step::Key("Control"),
            Step::KeyUp("Control"),
            CLICK,
            CLICK,
        ],
        |log, _| {
            assert_log_contains(log, &printed("X1=true"));
            assert_log_contains(log, &printed("X2=true"));
        },
    );
}

// =====================================================================
// 4. 既読判定 / 未読もスキップ設定
// =====================================================================

/// 既読化はラベルに入った時ではなく、その区間を最後まで通過した時（次のラベル到達）。
#[test]
fn b1_label_is_marked_read_only_after_passing_its_section() {
    let frag = r#"[emb exp="'R0='+JSON.stringify(sf.readLabels)"][l]
*second
[emb exp="'R1='+JSON.stringify(sf.readLabels)"][l]"#;
    on_both(frag, Setup::default(), &[CLICK, CLICK], |log, _| {
        assert_log_contains(log, &printed("R0={}"));
        assert_log_contains(log, &printed(r#"R1={"test.ks*start":true}"#));
    });
}

/// 既読スキップ中に未読ラベルへ入るとスキップが止まり、そのラベルの文でクリック待ちになる。
#[test]
fn b1_skip_stops_at_unread_label() {
    let frag = r#"[eval exp="sys.player.mode.skip = true"]A[l]
*b
B[l]
C[l]"#;
    on_both(
        frag,
        Setup::default(),
        &[
            SYNC,
            Step::Check(|log| {
                assert_log_contains(log, &printed("B"));
                assert_log_not_contains(log, &printed("C"));
            }),
            CLICK,
            CLICK,
        ],
        |log, _| assert_log_contains(log, &printed("C")),
    );
}

/// 既読のラベルならスキップは止まらず、クリック無しで最後まで進む。
#[test]
fn b1_skip_passes_read_label_without_stopping() {
    let frag = r#"[eval exp="sf.readLabels['test.ks*b'] = true"][eval exp="sys.player.mode.skip = true"]A[l]
*b
B[l]
C[l]"#;
    on_both(frag, Setup::default(), &[], |log, _| {
        assert_log_contains(log, &printed("C"));
    });
}

/// `settings.skipUnread` が真なら未読区間でも止まらない（既定は偽）。
#[test]
fn b1_skip_unread_setting_lets_skip_run_through_unread_labels() {
    let frag = r#"[emb exp="'DEF='+sys.player.settings.skipUnread"][eval exp="sys.player.settings.skipUnread = true"][eval exp="sys.player.mode.skip = true"]A[l]
*b
B[l]
C[l]"#;
    on_both(frag, Setup::default(), &[], |log, _| {
        assert_log_contains(log, &printed("DEF=false"));
        assert_log_contains(log, &printed("C"));
    });
}

/// 旧 sys.json（skipUnread を持たない設定）は既定値で補完され、他の設定値は保たれる。
#[test]
fn b1_legacy_sys_json_without_skip_unread_is_completed() {
    let frag =
        r#"[emb exp="'V='+sys.player.settings.bgmVol+',U='+sys.player.settings.skipUnread"]"#;
    let setup = Setup {
        sys_json: Some(
            r#"{"readLabels":{},"config":{"bgmVol":30,"seVol":80,"voiceVol":80,"cps":20,"autoWaitMs":1800}}"#,
        ),
        ..Setup::default()
    };
    on_both(frag, setup, &[], |log, _| {
        assert_log_contains(log, &printed("V=30,U=false"));
    });
    let setup = Setup {
        sys_json: Some(r#"{"config":{"skipUnread":true}}"#),
        ..Setup::default()
    };
    on_both(frag, setup, &[], |log, _| {
        assert_log_contains(log, &printed("V=80,U=true"));
    });
}

/// 壊れた sys.json でも黙って落ちず既定値で起動する（中身は .bak へ退避される。savedata.rs のテスト参照）。
#[test]
fn b1_corrupt_sys_json_starts_with_defaults() {
    let frag = r#"[emb exp="'V='+sys.player.settings.bgmVol"]"#;
    let setup = Setup {
        sys_json: Some(r#"{"readLabels": {"a*b": tr"#),
        ..Setup::default()
    };
    on_both(frag, setup, &[], |log, _| {
        assert_log_contains(log, &printed("V=80"));
    });
}

// =====================================================================
// 5. [rclick call=] フック
// =====================================================================

/// 右クリックの call は表示中の文を消さない。サブルーチンが [cm] しても、[return] で元の文を復元し、
/// 元の [l] の待ちへ戻る（待ちを飛ばして次の文へ進まない）。
#[test]
fn b1_rclick_call_restores_message_and_rewaits_after_return() {
    let frag = "[rclick call=true target=*menu]\nHELLO[l]\nWORLD[l]\n[jump target=*fin]\n*menu\n[cm]MENU[l]\n[return]\n*fin\n";
    on_both(
        frag,
        Setup::default(),
        &[
            SYNC,
            Step::RightClick(10.0, 10.0),
            CLICK, // MENU の [l]
            CLICK, // 復元された HELLO の [l]（待ちに戻っている）
            CLICK, // WORLD の [l]
        ],
        |log, e| {
            assert_eq!(
                count(log, &printed("HELLO")),
                2,
                "{e}: 元の表示 + 復元の 2 回: {log:#?}"
            );
            let pos = |needle: &str, nth: usize| -> usize {
                log.iter()
                    .enumerate()
                    .filter(|(_, l)| l.contains(needle))
                    .nth(nth)
                    .map(|(i, _)| i)
                    .unwrap_or_else(|| panic!("{e}: {needle} が無い: {log:#?}"))
            };
            let hello1 = pos(&printed("HELLO"), 0);
            let menu = pos(&printed("MENU"), 0);
            let hello2 = pos(&printed("HELLO"), 1);
            let world = pos(&printed("WORLD"), 0);
            assert!(
                hello1 < menu && menu < hello2 && hello2 < world,
                "{e}: {log:#?}"
            );
        },
    );
}

/// サブルーチンがメッセージに触れなければ、復元の再描画は起きない（元の表示がそのまま残る）。
#[test]
fn b1_rclick_call_leaves_message_alone_when_subroutine_does_not_touch_it() {
    let frag = "[rclick call=true target=*menu]\nHELLO[l]\n[jump target=*fin]\n*menu\n[eval exp=\"f.menu = 1\"][return]\n*fin\n";
    on_both(
        frag,
        Setup::default(),
        &[SYNC, Step::RightClick(10.0, 10.0), CLICK],
        |log, e| {
            assert_eq!(count(log, &printed("HELLO")), 1, "{e}: {log:#?}");
            // 呼び出し時にメッセージを消していない（直後の [jump] による消去 1 回だけ）。
            assert_eq!(count(log, "text_clear 3"), 1, "{e}: {log:#?}");
        },
    );
}

// =====================================================================
// 6. メニュー/バックログ表示中はゲームを止める
// =====================================================================

/// [wait] 中にメニューを開くと、閉じるまで [wait] は満了しない（閉じた後に残りを待って進む）。
#[test]
fn b1_menu_pauses_wait() {
    let frag = "[wait time=900]AFTER[l]";
    on_both(
        frag,
        Setup::default(),
        &[
            SYNC,
            Step::KeyNow("Escape"),
            Step::Sleep(1500),
            Step::Check(|log| {
                assert_log_contains(log, "save_list");
                assert_log_not_contains(log, &printed("AFTER"));
            }),
            Step::KeyNow("Escape"),
            Step::Sleep(1800),
            Step::Check(|log| assert_log_contains(log, &printed("AFTER"))),
            CLICK,
        ],
        |_, _| {},
    );
}

/// オート送り: メニュー表示中は待ち時間を数えず、閉じた後に次の文へ進む。
#[test]
fn b1_menu_pauses_auto_mode() {
    let frag = r#"[eval exp="sys.player.autoWaitMs = 900"][eval exp="sys.player.mode.auto = true"]A[l]B[l]"#;
    on_both(
        frag,
        Setup::default(),
        &[
            SYNC,
            Step::KeyNow("Escape"),
            Step::Sleep(1500),
            Step::Check(|log| {
                assert_log_contains(log, &printed("A"));
                assert_log_not_contains(log, &printed("B"));
            }),
            Step::KeyNow("Escape"),
            Step::Sleep(1800),
            Step::Check(|log| assert_log_contains(log, &printed("B"))),
        ],
        |_, _| {},
    );
}

/// オート送りの待ち時間は「全文を表示し終えてから」数え、基本待ち（ここでは 600ms）+ 文字数 x 係数（基本待ちの 1/40 =
/// 15ms、加算は基本待ちの 2 倍まで）になる。短い台詞・長い台詞・上限に達する台詞・文字送り中の台詞で、
/// 次の本文 B が出る時刻が式どおりであることを両エンジンで確かめる。
fn auto_case(head: &str, text: &str, not_yet_ms: u64, by_ms: u64) {
    let frag = format!(
        r#"S[l][eval exp="sys.player.autoWaitMs = 600"][eval exp="sys.player.mode.auto = true"]{head}{text}[l]B[l]"#
    );
    // 最初のクリックで本文が出る（起動の遅れが時刻に混ざらない）
    let steps = [
        CLICK,
        Step::Sleep(not_yet_ms),
        Step::Check(|log| assert_log_not_contains(log, &printed("B"))),
        Step::Sleep(by_ms - not_yet_ms),
        Step::Check(|log| assert_log_contains(log, &printed("B"))),
    ];
    on_both(&frag, Setup::default(), &steps, |_, _| {});
}

#[test]
fn b1_auto_wait_scales_with_text_length_and_counts_from_the_last_glyph() {
    // 4 字: 600 + 4*15 = 660ms
    auto_case("", &"あ".repeat(4), 400, 1300);
    // 30 字: 600 + 450 = 1050ms
    auto_case("", &"あ".repeat(30), 800, 1800);
    // 120 字: 加算は上限 1200ms で 1800ms
    auto_case("", &"あ".repeat(120), 1500, 2700);
    // 文字送り中（20 字を cps 50 = 400ms）: 表示し終えてから 600 + 300 = 900ms、合計 1300ms。
    // 本文の表示開始から数えていれば 1000ms 時点で B が出てしまう。
    auto_case("[endnowait][delay speed=20]", &"あ".repeat(20), 1000, 2000);
}

/// スキップ中にメニューを開くと、閉じるまで本文が進まない（スキップ状態は保持され、閉じると再開）。
#[test]
fn b1_menu_pauses_skip_and_resumes_after_close() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static PRINTS_WHILE_OPEN: AtomicUsize = AtomicUsize::new(0);
    let mut frag = String::from(r#"[eval exp="sys.player.mode.skip = true"]"#);
    for i in 0..60 {
        frag.push_str(&format!("L{i}[l]\n"));
    }
    on_both(
        &frag,
        Setup::default(),
        &[
            Step::Sleep(500),
            Step::KeyNow("Escape"),
            Step::Sleep(500),
            Step::Check(|log| {
                PRINTS_WHILE_OPEN.store(count(log, "text_print"), Ordering::SeqCst);
                assert_log_contains(log, "save_list");
            }),
            Step::Sleep(1200),
            Step::Check(|log| {
                assert_eq!(
                    count(log, "text_print"),
                    PRINTS_WHILE_OPEN.load(Ordering::SeqCst),
                    "メニュー表示中に本文が進んでいる"
                );
            }),
            Step::KeyNow("Escape"),
        ],
        // 閉じた後はスキップが再開して最後の行まで進む（クリック無しで [close] へ到達する）。
        |log, _| assert_log_contains(log, &printed("L59")),
    );
}

// =====================================================================
// 7. 欠落コンテンツ
// =====================================================================

/// 存在しないシナリオへの jump: error ログ + メッセージ窓に表示 + クリック待ち。落下実行しない。
#[test]
fn b1_jump_to_missing_scenario_shows_message_and_halts() {
    let frag = "before[l][jump storage=\"nosuch.ks\"]FALLTHROUGH[l]";
    on_both(
        frag,
        Setup::default(),
        &[CLICK, CLICK, Step::Key("WindowClose")],
        |log, _| {
            assert_log_contains(log, "log error シナリオ 'nosuch.ks' が見つかりません");
            assert_log_contains(log, &printed("シナリオ 'nosuch.ks' が見つかりません"));
            assert_log_not_contains(log, &printed("FALLTHROUGH"));
            assert_log_contains(log, "quit");
        },
    );
}

/// 存在しないラベルへの jump/call も同様に停止する（次のコマンドへ落下しない）。
#[test]
fn b1_jump_and_call_to_missing_label_halt_instead_of_falling_through() {
    for frag in [
        "before[l][jump target=*nolabel]FALLTHROUGH[l]",
        "before[l][call target=*nolabel]FALLTHROUGH[l]",
    ] {
        on_both(
            frag,
            Setup::default(),
            &[CLICK, CLICK, Step::Key("WindowClose")],
            |log, _| {
                assert_log_contains(log, "log error ラベル '*nolabel' が見つかりません");
                assert_log_contains(log, "text: \"ラベル '*nolabel' が見つかりません");
                assert_log_not_contains(log, &printed("FALLTHROUGH"));
            },
        );
    }
}

/// 停止後も入力は生きており（メニューが開く）、閉じる操作で終了できる。
#[test]
fn b1_halt_on_missing_content_still_accepts_menu() {
    let frag = "[jump storage=\"nosuch.ks\"]";
    on_both(
        frag,
        Setup::default(),
        &[SYNC, CLICK, Step::Key("Escape"), Step::Key("WindowClose")],
        |log, _| {
            assert_log_contains(log, "save_list");
            assert_log_contains(log, "quit");
        },
    );
}

// =====================================================================
// 8. メッセージフォント
// =====================================================================

/// [font face=] は以降のテキストのセグメントへ family を載せ、[resetfont]/[deffont] で戻る。
#[test]
fn b1_font_face_applies_per_segment_and_resets() {
    let frag = r#"[font face="Zz Face"]A[resetfont]B[deffont face="Zz Def"][font face="user"]C[resetfont]D"#;
    on_both(frag, Setup::default(), &[], |log, e| {
        let line = |t: &str| -> String {
            log.iter()
                .find(|l| l.contains("text_print") && l.contains(&printed(t)))
                .unwrap_or_else(|| panic!("{e}: {t} が無い: {log:#?}"))
                .clone()
        };
        assert!(
            line("A").contains(r#"font_family: Some("Zz Face")"#),
            "{e}: {}",
            line("A")
        );
        assert!(
            line("B").contains("font_family: None"),
            "{e}: {}",
            line("B")
        );
        // face="user" は既定へ戻す。
        assert!(
            line("C").contains("font_family: None"),
            "{e}: {}",
            line("C")
        );
        // [deffont face=] で更新した既定へ [resetfont] で戻る。
        assert!(
            line("D").contains(r#"font_family: Some("Zz Def")"#),
            "{e}: {}",
            line("D")
        );
    });
}

/// KagOptions.message.fontFamily はメッセージレイヤの text.style に届く。指定が無ければ既定のまま。
#[test]
fn b1_message_font_family_option_reaches_text_style() {
    let setup = Setup {
        player_opts: r#"message: { fontFamily: "Zz Opt Font" }"#,
        ..Setup::default()
    };
    on_both("A[l]", setup, &[CLICK], |log, _| {
        assert_log_contains(
            log,
            "text_style 3 align=Left line_height=1.6 font_family=Zz Opt Font",
        );
    });
    on_both("A[l]", Setup::default(), &[CLICK], |log, _| {
        assert_log_contains(
            log,
            "text_style 3 align=Left line_height=1.6 font_family=Yu Gothic UI",
        );
    });
}

// =====================================================================
// 9. ボイス
// =====================================================================

/// 次の行へ送るクリックでボイスを止める。[playvoice] 直後の待ちの間は止めない。
#[test]
fn b1_click_to_advance_stops_playing_voice() {
    let frag = r#"[playvoice storage="v1.ogg"]A[l]B[l]"#;
    on_both(
        frag,
        Setup::default(),
        &[
            SYNC,
            Step::Check(|log| {
                assert_log_contains(log, "audio_play voice");
                // [playvoice] 自身の「前の音を止める」1 回だけ（待ち中は止めていない）。
                assert_eq!(count(log, "audio_stop voice"), 1, "{log:#?}");
            }),
            CLICK,
            SYNC,
            Step::Check(|log| {
                assert_eq!(
                    count(log, "audio_stop voice"),
                    2,
                    "送りのクリックで止まる: {log:#?}"
                );
            }),
            CLICK,
        ],
        |log, e| {
            // ボイス未再生の 2 回目の送りでは余計な stop を出さない。
            assert_eq!(count(log, "audio_stop voice"), 2, "{e}: {log:#?}");
        },
    );
}

/// ボイスを鳴らしていなければ、送りで audio stop を発行しない。
#[test]
fn b1_advance_without_voice_does_not_issue_voice_stop() {
    on_both("A[l]B[l]", Setup::default(), &[CLICK, CLICK], |log, _| {
        assert_log_count(log, "audio_stop voice", 0);
    });
}
