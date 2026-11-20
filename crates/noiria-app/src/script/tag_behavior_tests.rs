//! KAG タグ振る舞いテスト（タグ→ops 変換の回帰スイート）。
//! ハーネス本体は `tag_harness.rs`。フラグメント設計・カバレッジ照合の方針は
//! `docs/implementation-notes.md`「2026-07-11 タグ振る舞いテストスイート」参照。
//!
//! 各テスト関数はカテゴリ単位（`docs/kag-tag-compat.md` のウェーブ区分に対応）でまとめ、
//! カテゴリに属する✅タグそれぞれについて最低 1 つの観測可能な効果を検証する。観測手段は
//! 3 通り: (a) `EngineState::op_log`（レイヤ/テキスト/オーディオ/動画/トランジション ops の
//! 呼び出し列）、(b) `api.sys.log(...)` の実行証跡（`EngineState::sys_log` 経由で op_log にも
//! 記録される。多くの W2/W4/W6 タグはこれが唯一の観測可能な効果）、(c) `[emb exp="..."]`
//! による `KagPlayer` 内部状態の直接読み出し（`sys.player.xxx` は実行時には TS の
//! `private` 修飾に関わらず参照できるため、状態フラグ系タグの検証に使う）。
//!
//! ハーネスのゲームは `charaLayerCount: 1` で起動するため、既定のレイヤ名→ネイティブ ID は
//! `base`=1, `"0"`=2, `message0`=3（追加レイヤは 4 以降）。

use super::tag_harness::{
    assert_log_contains, assert_log_count, assert_log_not_contains, assert_logs_match, printed,
    run_tsint, run_v8,
};

// =====================================================================
// スモークテスト（ハーネス自体の健全性）
// =====================================================================

#[test]
fn smoke_image_tag_emits_layer_load_image() {
    let frag = r#"[image layer=base storage="assets/solid.png"]"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "layer_load_image");
    assert_log_contains(&log, "assets/solid.png");
}

// =====================================================================
// V8/tsint ops 列自動比較
// =====================================================================

/// 同じ断片を V8 と tsint の両エンジンで実行し、発行された ops 列（ログ行）の完全一致を検証する。
/// 断片は同期タグ中心（実時間 wait への依存が無い）ものを選び、順序まで含めて決定論的に
/// 一致することを要求する。
#[test]
fn v8_and_tsint_emit_identical_ops_for_sync_fragment() {
    let frag = r#"
[image layer=base storage="assets/solid.png" left=10 top=20]
[font color=0xff0000]Red[resetfont]
[eval exp="f.x = 21"]
[emb exp="f.x * 2"]
[if exp="f.x == 21"]Branch21[else]WrongBranch[endif]
[playse storage="click.wav" buf=1]
[backlay]
[quake time=10]
[stopquake]
[cm]
"#;
    let v8_log = run_v8(frag, &[]);
    let tsint_log = run_tsint(frag, &[]);
    assert_logs_match(&v8_log, &tsint_log);
    // 比較自体が空振りしていないことの確認（両方空でも一致してしまうため）。
    assert_log_contains(&v8_log, &printed("Branch21"));
    assert_log_contains(&v8_log, "audio_play se1 assets/se/click.wav");
}

/// クリック入力（リンク選択）を含む断片でも両エンジンの ops 列が一致することを検証する。
#[test]
fn v8_and_tsint_emit_identical_ops_for_link_click_fragment() {
    let frag = "[link target=*after]はい[endlink][s]\n*after\nおわり[l]";
    let clicks = [(90.0, 525.0), (10.0, 10.0)];
    let v8_log = run_v8(frag, &clicks);
    let tsint_log = run_tsint(frag, &clicks);
    assert_logs_match(&v8_log, &tsint_log);
    assert_log_contains(&v8_log, &printed("おわり"));
}

// =====================================================================
// 回帰テスト
// =====================================================================

/// リンククリックで click.wav の play がちょうど 1 回だけ発行されることを確認する
/// （ユーザー報告の二重再生バグの回帰テスト、`docs/implementation-notes.md`
/// 「2026-07-11 選択肢クリック音の二重再生修正」参照）。
#[test]
fn regression_link_click_plays_click_wav_exactly_once() {
    let frag = "[link target=*after]はい[endlink][s]\n*after\nおわり[l]";
    let log_v8 = run_v8(frag, &[(90.0, 525.0), (10.0, 10.0)]);
    assert_log_count(&log_v8, "click.wav", 1);
    let log_tsint = run_tsint(frag, &[(90.0, 525.0), (10.0, 10.0)]);
    assert_log_count(&log_tsint, "click.wav", 1);
}

/// [playse buf=0]+[playse buf=1] は別チャンネル (se0/se1) で 2 回発行される
/// （多重 SE バッファの回帰テスト、W2）。
#[test]
fn regression_playse_buf_uses_distinct_channels() {
    let frag = r#"[playse storage="a.wav" buf=0][playse storage="b.wav" buf=1]"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "audio_play se0 assets/se/a.wav");
    assert_log_contains(&log, "audio_play se1 assets/se/b.wav");
    let count = log
        .iter()
        .filter(|l| l.starts_with("audio_play se"))
        .count();
    assert_eq!(
        count, 2,
        "buf=0/buf=1 は別チャンネルで 2 回再生されるはず: {log:#?}"
    );
}

// =====================================================================
// 既存 (対応済み) タグ: フロー制御
// =====================================================================

#[test]
fn flow_control_tags() {
    let frag = r#"
[eval exp="f.x = 1"]
[if exp="f.x == 1"][emb exp="'IFTRUE'"][elsif exp="f.x == 2"][emb exp="'ELSIFBRANCH'"][else][emb exp="'ELSEBRANCH'"][endif]
[macro name=addone][eval exp="f.y = (f.y||0) + 1"][endmacro]
[addone][addone]
[emb exp="f.y"]
[call target=*flowsub]
[emb exp="'AFTERCALL'"]
[jump target=*flowdone]
*flowsub
[emb exp="'INSUB'"]
[return]
*flowdone
[cancelskip]
"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, &printed("IFTRUE"));
    assert_log_not_contains(&log, "ELSIFBRANCH");
    assert_log_not_contains(&log, "ELSEBRANCH");
    assert_log_contains(&log, &printed("2")); // macro を 2 回実行 → f.y == 2
    assert_log_contains(&log, &printed("INSUB"));
    assert_log_contains(&log, &printed("AFTERCALL")); // return が呼び出し直後へ戻る
}

// =====================================================================
// 既存 (対応済み) タグ: メッセージ基本
// =====================================================================

#[test]
fn message_basics_tags() {
    let frag = r#"
Alpha[r]Beta[l]
Gamma[p]
[er]
AfterEr[ct]
[cm]
[wait time=10]
[history output=false][emb exp="JSON.stringify(sys.player.backlogEnabled)"]
[history output=true]
[name text="キャラ"][emb exp="JSON.stringify(sys.player.cur.nameLabel)"]
[name][emb exp="JSON.stringify(sys.player.cur.nameLabel)"]
[delay speed=20][emb exp="sys.player.cps"]
[nowait][endnowait]AfterNW
"#;
    let log = run_v8(frag, &[(10.0, 10.0), (10.0, 10.0)]);
    assert_log_contains(&log, &printed("Alpha"));
    assert_log_contains(&log, &printed("Beta"));
    assert_log_contains(&log, &printed("Gamma"));
    assert_log_contains(&log, &printed("AfterEr"));
    assert_log_contains(&log, "text_clear 3"); // [er]/[ct]/[cm]
    assert_log_contains(&log, &printed("false")); // history output=false
    assert_log_contains(&log, &printed("\"キャラ\"")); // [name text=]
    assert_log_contains(&log, &printed("null")); // [name]（属性なし）で名前欄を外す
    assert_log_contains(&log, &printed("50")); // [delay speed=20] → cps = 1000/20 = 50
                                               // [endnowait] 後の本文は cps>0 で印字される（[nowait] の解除確認。直前の [delay speed=20]
                                               // により cps=50 になっている — [delay] の効果の実地確認も兼ねる）。
    assert!(
        log.iter()
            .any(|l| l.contains("cps=50") && l.contains(&printed("AfterNW"))),
        "[endnowait] 後の本文が cps=50（delay speed=20 の設定値）で印字されるはず: {log:#?}"
    );
}

// =====================================================================
// 既存 (対応済み) タグ: 選択肢 (link/endlink)
// =====================================================================

#[test]
fn link_tags() {
    let frag = "[link target=*linkafter]はい[endlink][s]\n*linkafter\n[emb exp=\"'LINKED'\"]";
    let log = run_v8(frag, &[(90.0, 525.0)]);
    assert_log_contains(&log, &printed("はい"));
    assert_log_contains(&log, "text_rects");
    assert_log_contains(&log, &printed("LINKED"));
}

// =====================================================================
// 既存 (対応済み) タグ: 画像レイヤ・move・quake
// =====================================================================

#[test]
fn layer_image_move_quake_tags() {
    let frag = r#"
[image layer=base storage="assets/solid.png" left=10 top=20]
[move layer=base x=100 y=50 time=10]
[wm]
[backlay layer=base]
[freeimage layer=base]
[quake time=10]
[wq]
[stopquake]
[position left=50 top=60 width=1000 height=200]
"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "layer_load_image 1 fore assets/solid.png");
    assert_log_contains(&log, "layer_move 1 fore");
    assert_log_contains(&log, "backlay 1");
    assert_log_contains(&log, "layer_clear_image 1 fore");
    assert_log_contains(&log, "quake ax=10 ay=10");
    assert_log_contains(&log, "quake_stop");
    assert_log_contains(
        &log,
        "layer_set_props 3 fore LayerProps { x: Some(50), y: Some(60)",
    );
}

// =====================================================================
// 既存 (対応済み) タグ: 音声基本
// =====================================================================

#[test]
fn audio_basics_tags() {
    let frag = r#"
[playbgm storage="main.wav"]
[stopbgm]
[playbgm storage="main.wav"]
[fadeoutbgm time=100]
[fadebgm volume=50]
[playse storage="click.wav"]
[stopse]
[ws]
[playvoice storage="line1.wav"]
[stopvoice]
"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "audio_play bgm assets/bgm/main.wav");
    assert_log_contains(&log, "audio_stop bgm fade=0");
    assert_log_contains(&log, "audio_stop bgm fade=100");
    assert_log_contains(&log, "audio_volume bgm");
    assert_log_contains(&log, "audio_play se0 assets/se/click.wav");
    assert_log_contains(&log, "audio_stop se0 fade=0");
    assert_log_contains(&log, "audio_play voice assets/voice/line1.wav");
    assert_log_contains(&log, "audio_stop voice fade=0");
}

// =====================================================================
// 既存 (対応済み) タグ: トランジション基本
// =====================================================================

#[test]
fn trans_basics_tags() {
    let frag = r#"[trans method=crossfade time=10][wt][emb exp="'TRANSDONE'"]"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "trans_begin crossfade 10");
    assert_log_contains(&log, &printed("TRANSDONE")); // [wt] がハングせず完了する
}

// =====================================================================
// 既存 (対応済み) タグ: 動画基本
// =====================================================================

#[test]
fn video_basics_tags() {
    let frag = r#"[video layer=0 storage="assets/clip.avi"][wv][stopvideo]"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "video_play 2 assets/clip.avi loop=false");
    assert_log_contains(&log, "video_stop");
}

#[test]
fn video_seek_events_segment_loop_and_wp_match_in_both_engines() {
    let frag = r#"
[video layer=0 storage="assets/clip.avi" frame=1 position=0]
[wv]
[openvideo layer=0 storage="assets/clip.avi"]
[video layer=0 position=10]
[video layer=0 frame=1 position=0]
[videoevent frame=1]
[videosegloop start=0 end=2]
[preparevideo layer=0]
[wp for=prepare]
[playvideo layer=0]
[wp for=period]
[wp for=segLoop]
[cancelvideoevent]
[cancelvideosegloop]
[wv]
[video layer=0 storage="assets/clip.avi" loop=true]
[wp for=loop]
[stopvideo]
[wp for=period]
[trace exp="'B4_STOPPED_WP'"]
[openvideo layer=0 storage="assets/clip.avi"]
[preparevideo layer=0]
[wp]
[video layer=0 slot=1 frame=0]
[videoevent slot=1 frame=0]
[videosegloop slot=1 start=0 end=2]
[cancelvideoevent slot=1]
[cancelvideosegloop slot=1]
[video layer=0 frame=-1]
[video layer=0 frame=1.5]
[video layer=0 position=-1]
[videoevent frame=-1]
[videoevent frame=1.5]
[videosegloop start=-1 end=2]
[videosegloop start=2 end=1]
[wp slot=-1]
[wp for=unknown]
[openvideo slot=1 layer=0 storage="assets/clip.avi"]
[preparevideo slot=1 layer=0 storage="assets/clip.avi"]
[playvideo slot=1 layer=0 storage="assets/clip.avi"]
[pausevideo slot=1]
[resumevideo slot=1]
[rewindvideo slot=1]
[stopvideo slot=1]
[wv slot=1]
[trace exp="'B4_VIDEO_DONE'"]
"#;
    let v8_log = run_v8(frag, &[]);
    let tsint_log = run_tsint(frag, &[]);
    assert_logs_match(&v8_log, &tsint_log);

    assert_log_contains(
        &v8_log,
        "video_play_at 2 assets/clip.avi loop=false frame=Some(1) position=None",
    );
    assert_log_contains(&v8_log, "video_seek_position 10");
    assert_log_contains(&v8_log, "video_seek_frame 1");
    assert_log_not_contains(&v8_log, "video_seek_position 0");
    assert_log_contains(&v8_log, "video_set_period_event 1");
    assert_log_contains(&v8_log, "video_set_segment_loop 0 2");
    assert_log_contains(&v8_log, "video_prepare_opened 2");
    assert_log_contains(&v8_log, "video_cancel_period_event");
    assert_log_contains(&v8_log, "video_cancel_segment_loop");
    assert_log_contains(&v8_log, "log warn [video] slot=1");
    assert_log_contains(&v8_log, "log warn [videoevent] slot=1");
    assert_log_contains(&v8_log, "log warn [videosegloop] slot=1");
    assert_log_contains(&v8_log, "log warn [cancelvideoevent] slot=1");
    assert_log_contains(&v8_log, "log warn [cancelvideosegloop] slot=1");
    assert_log_contains(&v8_log, "log warn [video] frame=-1");
    assert_log_contains(&v8_log, "log warn [video] frame=1.5");
    assert_log_contains(&v8_log, "log warn [video] position=-1");
    assert_log_count(&v8_log, "video_seek_frame 1", 1);
    assert_log_count(&v8_log, "video_seek_position 10", 1);
    assert_log_count(&v8_log, "video_set_segment_loop 0 2", 1);
    assert_log_count(&v8_log, "video_cancel_period_event", 1);
    assert_log_count(&v8_log, "video_cancel_segment_loop", 1);
    assert_log_contains(&v8_log, "log warn [videoevent] frame=-1");
    assert_log_contains(&v8_log, "log warn [videoevent] frame=1.5");
    assert_log_contains(&v8_log, "log warn [videosegloop] start=-1");
    assert_log_contains(&v8_log, "log warn [videosegloop] start=2");
    assert_log_contains(&v8_log, "log warn [wp] slot=-1");
    assert_log_contains(&v8_log, "log warn [wp] for=unknown");
    for tag in [
        "openvideo",
        "preparevideo",
        "playvideo",
        "pausevideo",
        "resumevideo",
        "rewindvideo",
        "stopvideo",
        "wv",
    ] {
        assert_log_contains(&v8_log, &format!("log warn [{tag}] slot=1"));
    }
    assert_log_contains(&v8_log, "log info B4_STOPPED_WP");
    assert_log_contains(&v8_log, "log info B4_VIDEO_DONE");
}

#[test]
fn video_wp_wait_is_interrupted_by_jump_in_both_engines() {
    let frag = r#"
[click target=*after]
[video layer=0 storage="assets/clip.avi" loop=true]
[wp for=period]
[trace exp="'WP_OLD_PATH'"]
*after
[stopvideo]
[trace exp="'WP_JUMP_DONE'"]
"#;
    let clicks = [(10.0, 10.0)];
    let v8_log = run_v8(frag, &clicks);
    let tsint_log = run_tsint(frag, &clicks);
    assert_logs_match(&v8_log, &tsint_log);
    assert_log_not_contains(&v8_log, "WP_OLD_PATH");
    assert_log_contains(&v8_log, "log info WP_JUMP_DONE");
}

// =====================================================================
// 既存 (対応済み) タグ: システム基本
// =====================================================================

#[test]
fn system_basics_tags() {
    let frag = r#"
[title name="新しいタイトル"]
[autowait time=2500][emb exp="sys.player.autoWaitMs"]
[screenshot slot=shot1]
[close]
"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "set_title 新しいタイトル");
    assert_log_contains(&log, &printed("2500"));
    assert_log_contains(&log, "screenshot shot1");
    assert_log_contains(&log, "quit"); // [close]
}

// =====================================================================
// W1: 文字装飾系タグ
// =====================================================================

#[test]
fn w1_text_decoration_tags() {
    let frag = r#"
[font color=0xff0000 size=30]RedBig[resetfont]
[style bold=true]BoldSeg[resetstyle]
[deffont color=0x00ff00]
[resetfont]GreenByDefault
[deffont color=0xffffff]
[defstyle bold=true]
[resetstyle]BoldByDefault
[defstyle bold=false]
[resetfont][resetstyle]
[ruby text="るび"]本文
[ch text="Aあ"]
[indent]Indented[endindent]
[locate x=50 y=60]Located
"#;
    let log = run_v8(frag, &[]);
    assert!(
        log.iter().any(|l| l.contains(&printed("RedBig"))
            && l.contains("color: Some([255, 0, 0, 255])")
            && l.contains("size: Some(30.0)")),
        "[font color=0xff0000 size=30] の色・サイズが乗るはず: {log:#?}"
    );
    assert!(
        log.iter()
            .any(|l| l.contains(&printed("BoldSeg")) && l.contains("bold: Some(true)")),
        "[style bold=true] のセグメントが太字のはず: {log:#?}"
    );
    assert!(
        log.iter().any(|l| l.contains(&printed("GreenByDefault"))
            && l.contains("color: Some([0, 255, 0, 255])")),
        "[deffont color=] 後の [resetfont] は新しい既定色になるはず: {log:#?}"
    );
    assert!(
        log.iter()
            .any(|l| l.contains(&printed("BoldByDefault")) && l.contains("bold: Some(true)")),
        "[defstyle bold=] 後の [resetstyle] は新しい既定太字になるはず: {log:#?}"
    );
    assert!(
        log.iter()
            .any(|l| l.contains(&printed("本文")) && l.contains("ruby: Some(\"るび\")")),
        "[ruby] が次の printText に乗るはず: {log:#?}"
    );
    assert_log_contains(&log, &printed("A")); // [ch] は先頭 1 コードポイントのみ
    assert_log_not_contains(&log, &printed("Aあ"));
    assert_log_contains(&log, "indent: Some(true)");
    assert_log_contains(&log, "indent: Some(false)");
    assert_log_contains(&log, "locate: Some(LocateArgs { x: 50.0, y: Some(60.0) })");
}

#[test]
fn w1_line_style_tags_are_layer_local_and_match_between_engines() {
    let frag = r#"
[style align=center linespacing=10.4]
[emb exp="JSON.stringify(sys.player.cur.currentLineStyle)"]
[defstyle align=right linespacing=-100]
[emb exp="JSON.stringify(sys.player.cur.currentLineStyle)"]
[resetstyle]
[cm][er]
[emb exp="JSON.stringify(sys.player.cur.currentLineStyle)"]
[style align=diagonal linespacing=Infinity]
[laycount messages=2]
[current layer=message1]
[style align=center linespacing=0]
[emb exp="JSON.stringify([sys.player.msgLayers[0].currentLineStyle, sys.player.msgLayers[1].currentLineStyle])"]
"#;
    let v8_log = run_v8(frag, &[]);
    let tsint_log = run_tsint(frag, &[]);
    assert_logs_match(&v8_log, &tsint_log);

    assert_log_contains(&v8_log, "text_style 3 align=Center line_height=2");
    assert_log_contains(&v8_log, "text_style 3 align=Right line_height=0.03846154");
    assert_log_contains(&v8_log, "text_style 4 align=Center line_height=1.6");
    assert_log_contains(
        &v8_log,
        &printed(r#"{"align":"center","lineSpacing":10.4}"#),
    );
    assert_log_contains(&v8_log, &printed(r#"{"align":"right","lineSpacing":-100}"#));
    assert_log_contains(
        &v8_log,
        &printed(r#"[{"align":"right","lineSpacing":-100},{"align":"center","lineSpacing":0}]"#),
    );
    assert_log_contains(&v8_log, "[style] align=diagonal は不正なため無視します");
    assert_log_contains(
        &v8_log,
        "[style] linespacing=Infinity は有限値でないため無視します",
    );
}

#[test]
fn w1_old_save_defaults_line_styles_in_both_engines() {
    let frag = r#"
[style align=right linespacing=10.4]
[eval exp="f.old = sys.player.makeFullSnapshot(); f.old.msgLayers[0].currentLineStyle = undefined; f.old.msgLayers[0].defaultLineStyle = undefined; sys.player.tempSlots.set(-13, f.old)"]
[if exp="!sf.b13OldSaveLoaded"]
[eval exp="sf.b13OldSaveLoaded = true"]
[style align=center linespacing=4]
[tempload place=-13]
[endif]
[emb exp="JSON.stringify(sys.player.cur.currentLineStyle)"]
[emb exp="JSON.stringify(sys.player.cur.defaultLineStyle)"]
"#;
    let v8_log = run_v8(frag, &[]);
    let tsint_log = run_tsint(frag, &[]);
    assert_logs_match(&v8_log, &tsint_log);

    assert_log_contains(&v8_log, "text_style 3 align=Left line_height=1.6");
    assert_log_count(&v8_log, &printed(r#"{"align":"left","lineSpacing":0}"#), 2);
}

#[test]
fn w1_line_style_reflow_refreshes_the_append_boundary_in_both_engines() {
    let frag = r#"
[defstyle linespacing=-30]
A[r]B[r]C[r]D[r]E[r]F[r]G[r]H[r]I[r]J[r]K[r]L[resetstyle]
[emb exp="sys.player.api.text.print(sys.player.cur.layerId, [{text:'M',color:null}], true, 0).added"]
[cm]
[style linespacing=0]
A[r]B[r]C[r]D[r]E[r]F[r]G[r]H[r]I[r]J[r]K[r]L[style linespacing=-30]
[emb exp="sys.player.api.text.print(sys.player.cur.layerId, [{text:'N',color:null}], true, 0).added"]
"#;
    let v8_log = run_v8(frag, &[]);
    let tsint_log = run_tsint(frag, &[]);
    assert_logs_match(&v8_log, &tsint_log);

    let reflow_pairs = v8_log
        .windows(2)
        .filter(|pair| {
            pair[0].contains("text_style 3 align=Left") && pair[1].contains("text_reflow 3")
        })
        .count();
    assert_eq!(
        reflow_pairs, 2,
        "style/resetstyle の直後に再レイアウトされていません"
    );
    assert_log_count(&v8_log, &printed("1"), 2);
}

#[test]
fn w1_line_style_does_not_reflow_an_empty_layer() {
    let frag = "[style align=center][resetstyle]";
    let v8_log = run_v8(frag, &[]);
    let tsint_log = run_tsint(frag, &[]);
    assert_logs_match(&v8_log, &tsint_log);

    assert_log_not_contains(&v8_log, "text_reflow 3");
}

#[test]
fn w1_line_style_reflow_preserves_active_reveal_in_both_engines() {
    let frag = r#"
[endnowait]
ABCDE[style align=center]
[position width=500 height=180]
[emb exp="sys.player.api.text.isDone(sys.player.cur.layerId)"]
"#;
    let v8_log = run_v8(frag, &[]);
    let tsint_log = run_tsint(frag, &[]);
    assert_logs_match(&v8_log, &tsint_log);

    assert_log_count(&v8_log, "text_reflow 3", 2);
    assert_log_contains(&v8_log, &printed("false"));
}

#[test]
fn w1_invalid_native_line_style_values_match_between_engines() {
    let frag = r#"
[style linespacing=1e308]
[iscript]
f.alignRejected = false;
try {
  sys.player.api.text.style(sys.player.cur.layerId, { align: "diagonal" });
} catch (e) {
  f.alignRejected = true;
}
f.lineHeightRejected = false;
try {
  sys.player.api.text.style(sys.player.cur.layerId, { lineHeight: 1e308 });
} catch (e) {
  f.lineHeightRejected = true;
}
[endscript]
[emb exp="JSON.stringify([f.alignRejected, f.lineHeightRejected, sys.player.cur.currentLineStyle])"]
"#;
    let v8_log = run_v8(frag, &[]);
    let tsint_log = run_tsint(frag, &[]);
    assert_logs_match(&v8_log, &tsint_log);

    assert_log_contains(
        &v8_log,
        "[style] linespacing=1e308 は単精度の有限範囲外のため無視します",
    );
    assert_log_contains(
        &v8_log,
        &printed(r#"[true,true,{"align":"left","lineSpacing":0}]"#),
    );
    assert_log_count(&v8_log, "text_style 3", 1);
    assert_log_not_contains(&v8_log, "text_reflow 3");
}

// =====================================================================
// W2: 音声系タグ
// =====================================================================

#[test]
fn w2_audio_tags() {
    let frag = r#"
[bgmopt volume=40]
[seopt volume=60]
[fadeinbgm storage="main.wav" time=100]
[xchgbgm storage="second.wav" time=100]
[fadepausebgm time=50]
[resumebgm]
[pausebgm]
[setbgmstop storage=test.ks target=*w2done]
[clearbgmstop]
[wb]
[wl]
[fadeinse storage="click.wav" time=50]
[fadese buf=0 volume=30 time=50]
[fadeoutse buf=0 time=50]
[wf]
[entersebuf]
*w2done
"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "log info [bgmopt] volume=40");
    assert_log_contains(&log, "log info [seopt] volume=60");
    assert_log_contains(&log, "audio_play bgm assets/bgm/main.wav loop=true");
    assert_log_contains(&log, "log info [xchgbgm]");
    assert_log_contains(&log, "audio_play bgm2 assets/bgm/second.wav"); // xchgbgm が bgm2 へトグル
    assert_log_contains(&log, "audio_stop bgm fade=100"); // xchgbgm が旧チャンネルをフェードアウト
    assert_log_contains(&log, "log info [fadepausebgm] time=50");
    assert_log_contains(&log, "audio_fade_to bgm2 vol=0 ms=50 then=pause");
    assert_log_contains(&log, "log info [resumebgm]");
    assert_log_contains(&log, "audio_resume bgm2");
    assert_log_contains(&log, "log info [pausebgm]");
    assert_log_contains(&log, "audio_pause bgm2");
    assert_log_contains(&log, "log info [setbgmstop] storage=test.ks target=*w2done");
    assert_log_contains(&log, "log info [clearbgmstop]");
    assert_log_contains(&log, "audio_play se0 assets/se/click.wav");
    assert_log_contains(&log, "log info [fadese] buf=0 volume=30");
    assert_log_contains(&log, "audio_fade_to se0"); // fadese
    assert_log_contains(&log, "audio_stop se0 fade=50"); // fadeoutse
}

// =====================================================================
// W3: 複数メッセージレイヤ系タグ
// =====================================================================

#[test]
fn w3_multi_message_layer_tags() {
    let frag = r#"
[laycount messages=2]
[current layer=message1][emb exp="sys.player.current.index"]
[layopt layer=message1 left=10 top=10 visible=true]
[current layer=message0]
[graph storage="assets/solid.png"]
[hch text="タテチュウヨコ"]
[ptext layer=message0 text="PTEXT_CONTENT" x=5 y=5]
[copylay srclayer=message1 destlayer=message0]
[autowc enabled=true ch="、" time=10]
テスト、文章[l]
[autowc enabled=false]
[resetwait]
[wc n=1]
[hidemessage]
"#;
    let log = run_v8(frag, &[(10.0, 10.0)]);
    assert_log_contains(&log, "layer_new text -> 4"); // laycount で message1 のレイヤが追加される
    assert_log_contains(&log, &printed("1")); // current.index が 1 に切り替わる
    assert_log_contains(
        &log,
        "layer_set_props 4 fore LayerProps { x: Some(10), y: Some(10)",
    );
    assert_log_contains(&log, "inline_image: Some(\"assets/solid.png\")"); // [graph]
    assert_log_contains(&log, "log warn [hch]"); // 平文フォールバック warn
    assert_log_contains(&log, &printed("タテチュウヨコ")); // [hch] のフォールバック表示
    assert_log_contains(&log, "PTEXT_CONTENT");
    assert_log_contains(&log, "layer_copy 4/fore -> 3/fore copy_position=false"); // メッセージレイヤ宛は位置非コピー
    assert_log_contains(&log, &printed("テスト、")); // [autowc] が「、」ごとに分割 print する
    assert_log_contains(
        &log,
        "layer_set_props 3 fore LayerProps { x: None, y: None, z: None, opacity: None, visible: Some(false)",
    ); // [hidemessage]
}

#[test]
fn w3_position_resize_reflows_the_addressed_inactive_message_layer() {
    let frag = r#"
[laycount messages=2]
[current layer=message1]
INACTIVE_LAYER_TEXT
[current layer=message0]
[position layer=message1 width=240 height=180]
"#;
    let v8_log = run_v8(frag, &[]);
    let tsint_log = run_tsint(frag, &[]);
    assert_logs_match(&v8_log, &tsint_log);

    assert_log_count(&v8_log, "text_reflow 4", 1);
    assert_log_not_contains(&v8_log, "text_reflow 3");
}

// =====================================================================
// W4: UI・ボタン・クリック制御系タグ
// =====================================================================

#[test]
fn w4_ui_tags() {
    let frag = r#"
[button graphic="assets/button.png" target=*w4done x=200 y=10]
[checkbox name="f.chk" x=200 y=50]
[edit name="f.edittext" x=200 y=90]
[commit]
[click storage=test.ks target=*w4done]
[cclick][emb exp="JSON.stringify(sys.player.eventJumps.click)"]
[wheel storage=test.ks target=*w4done]
[cwheel][emb exp="JSON.stringify(sys.player.eventJumps.wheel)"]
[locklink][emb exp="JSON.stringify(sys.player.linksLocked)"]
[unlocklink][emb exp="JSON.stringify(sys.player.linksLocked)"]
[clickskip enabled=false][emb exp="JSON.stringify(sys.player.clickSkipEnabled)"]
[clickskip enabled=true]
[nextskip]
[cancelautomode]
[rclick enabled=false][emb exp="JSON.stringify(sys.player.rclickAction)"]
[rclick]
[glyph line="assets/solid.png" left=1 top=2]
[timeout time=60000 storage=test.ks target=*w4done]
[ctimeout]
[waitclick]
[ask text="質問？"]
[input title="t" prompt="p" initial="x" name="f.inputval"]
*w4done
[emb exp="'W4DONE'"]
"#;
    let log = run_v8(frag, &[(10.0, 10.0), (10.0, 10.0), (10.0, 10.0)]);
    assert_log_contains(&log, "log info [button] graphic=assets/button.png");
    assert_log_contains(&log, "layer_blit_image"); // button の状態スライス描画
    assert_log_contains(&log, "log info [checkbox] name=f.chk");
    assert_log_contains(&log, "log info [edit] name=f.edittext");
    assert_log_contains(&log, "log info [commit]");
    assert_log_contains(&log, "log info [click] storage=test.ks target=*w4done");
    assert_log_contains(&log, "log info [cclick]");
    assert_log_contains(&log, &printed("null")); // cclick 後は eventJumps.click が null
    assert_log_contains(&log, "log info [wheel] storage=test.ks target=*w4done");
    assert_log_contains(&log, "log info [cwheel]");
    assert_log_contains(&log, "log info [locklink]");
    assert_log_contains(&log, &printed("true")); // linksLocked
    assert_log_contains(&log, "log info [unlocklink]");
    assert_log_contains(&log, "log info [clickskip] enabled=false");
    assert_log_contains(&log, &printed("false")); // clickSkipEnabled
    assert_log_contains(&log, "log info [nextskip]");
    assert_log_contains(&log, "log info [cancelautomode]");
    assert_log_contains(&log, "log info [rclick] enabled=false");
    assert_log_contains(&log, &printed("\"disabled\""));
    assert_log_contains(&log, "log info [rclick] 既定に戻す");
    assert_log_contains(&log, "log info [glyph] line=assets/solid.png");
    assert_log_contains(&log, "log info [timeout] time=60000");
    assert_log_contains(&log, "log info [ctimeout]");
    assert_log_contains(&log, "log info [waitclick] 待機開始");
    assert_log_contains(&log, "log info [waitclick] クリックを受信");
    assert_log_contains(&log, "log info [ask] text=質問？");
    assert_log_contains(&log, "log info [ask] answer=");
    assert_log_contains(&log, "log info [input] title=t");
    assert_log_contains(&log, "log info [input] 終了");
    assert_log_contains(&log, &printed("W4DONE"));
}

/// [waittrig] は `sys.player.trigger(name)` の呼び出しで発火する（KAG の `kag.trigger()` とは
/// 異なる Noiria 独自の発火経路、`docs/kag-tag-compat.md` 参照）。ここでは eval で
/// 「50ms 後にトリガを発火する」デタッチした Promise を仕込んでから [waittrig] で待つ
/// （先に trigger してしまうと waitTrigger は「既に発火済み」を拾わない仕様のため）。
#[test]
fn w4_waittrig_fires_via_sys_player_trigger() {
    let frag = r#"
[eval exp="void sys.player.wait(50).then(() => sys.player.trigger('mytrig'))"]
[waittrig name=mytrig]
[emb exp="'TRIGGERED'"]
"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "log info [waittrig] name=mytrig 待機開始");
    assert_log_contains(&log, "log info [waittrig] name=mytrig 発火");
    assert_log_contains(&log, &printed("TRIGGERED"));
}

// =====================================================================
// W5: トランジション/アニメ/画像操作系タグ
// =====================================================================

#[test]
fn w5_transition_anim_image_tags() {
    let frag = r#"
[animstart layer=0 storage="assets/blink.asd"]
[wa layer=0]
[animstart layer=0 storage="assets/blink.asd"]
[animstop layer=0]
[move layer=0 path="(50,50)(100,100,200)" time=20]
[stopmove]
[pimage layer=0 storage="assets/solid.png" x=1 y=1]
[trans method=scroll from=right time=10][wt]
[stoptrans]
[image layer=base storage="assets/solid.png" clip="0,0,2,2" fliplr=true flipud=true grayscale=true]
[emb exp="'W5DONE'"]
"#;
    let log = run_v8(frag, &[]);
    // [animstart]: .asd のセル画像をレイヤ 0 (id=2) へ読み込む（2 セル以上 = wa 完走の証跡）
    assert!(
        log.iter()
            .filter(|l| l.contains("layer_load_image 2 fore assets/solid.png"))
            .count()
            >= 2,
        "[animstart] がセル画像を 2 回以上ロードするはず（loop=false 完走）: {log:#?}"
    );
    assert_log_contains(&log, "layer_move_path_distance 2 fore legs=2"); // [move path=]
    assert_log_contains(&log, "layer_move_skip"); // [stopmove]
    assert_log_contains(&log, "layer_blit_image 2 fore assets/solid.png dx=1 dy=1"); // [pimage]
    assert_log_contains(&log, "trans_begin scroll 10");
    assert_log_contains(&log, "trans_skip"); // [stoptrans]
    assert_log_contains(
        &log,
        "layer_load_image_ex 1 fore assets/solid.png clip=Some(BlitClip { x: 0, y: 0, w: 2, h: 2 }) flipH=true flipV=true gray=true",
    );
    assert_log_contains(&log, &printed("W5DONE")); // [wa]/[wt] がハングせず完走
}

#[test]
fn b12_move_spline_validation_and_ops_match_between_engines() {
    let frag = r#"
[move layer=0 path="(100,100,100)(0,0,200)" spline=true time=200]
[stopmove]
[move layer=0 path="(50,50)" spline=false time=100]
[stopmove]
[move layer=0 path="(25,25)" time=100]
[stopmove]
[move layer=0 path="(10,10)" spline=true time=100]
"#;
    let v8_log = run_v8(frag, &[]);
    let tsint_log = run_tsint(frag, &[]);
    assert_logs_match(&v8_log, &tsint_log);
    assert_log_contains(
        &v8_log,
        "layer_move_path_distance 2 fore legs=2 spline=true totalMs=200",
    );
    assert_log_count(
        &v8_log,
        "layer_move_path_distance 2 fore legs=1 spline=false totalMs=100",
        2,
    );
    assert_log_contains(
        &v8_log,
        "log warn [move] spline=true の path= には少なくとも 2 点が必要です",
    );
    assert_log_count(&v8_log, "layer_move_path_distance 2 fore", 3);
}

#[test]
fn b12_tempload_cancels_active_path_and_pending_waypoints_in_both_engines() {
    let frag = r#"
[tempsave place=-12]
[if exp="!sf.b12PathRestoreDone"]
[move layer=0 path="(100,0)(300,0)" spline=true time=100000]
[eval exp="sf.b12PathWasMoving = sys.player.api.layer.moving(); sf.b12PathRestoreDone = true"]
[tempload place=-12]
[endif]
[emb exp="JSON.stringify([sf.b12PathWasMoving, sys.player.api.layer.moving()])"]
[wm]
[emb exp="'B12_PATH_RESTORED'"]
"#;
    let v8_log = run_v8(frag, &[]);
    let tsint_log = run_tsint(frag, &[]);
    assert_logs_match(&v8_log, &tsint_log);

    assert_log_contains(
        &v8_log,
        "layer_move_path_distance 2 fore legs=2 spline=true",
    );
    assert_log_contains(&v8_log, "layer_move_cancel 2 fore");
    assert_log_contains(&v8_log, "layer_move_cancel 2 back");
    assert_log_contains(&v8_log, &printed("[true,false]"));
    assert_log_contains(&v8_log, &printed("B12_PATH_RESTORED"));
}

// =====================================================================
// W6: ストア系・スクリプト系・履歴系・動画拡張タグ
// =====================================================================

/// 巻き戻し系タグ（tempload/goback/gotostart/load）は「復元元を記録した直後」まで pc を
/// 巻き戻すため、ガード無しで並べると無限ループになる（`games/sample/first.ks` の W6 デモと
/// 同じ問題）。復元で巻き戻されない `sf` のフラグでガードする（`f` は復元対象なのでガードに
/// 使えない。`docs/implementation-notes.md`「2026-07-11 W6」参照）。
#[test]
fn w6_store_script_history_video_tags() {
    let frag = r#"
[eval exp="f.x = 1"]
[tempsave place=1]
[if exp="!sf.g1"][eval exp="sf.g1 = true"][eval exp="f.x = 2"][tempload place=1][endif]
[emb exp="f.x"]
[record]
[record]
[if exp="!sf.g2"][eval exp="sf.g2 = true"][goback][endif]
[startanchor]
[if exp="!sf.g3"][eval exp="sf.g3 = true"][gotostart][endif]
[locksnapshot]
[unlocksnapshot]
[store place=1]
[save place=2]
[if exp="!sf.g4"][eval exp="sf.g4 = true"][load place=2][endif]
[copybookmark src=2 dest=3]
[erasebookmark place=3]
[disablestore enabled=true]
[disablestore enabled=false]
[clearvar][emb exp="JSON.stringify(f)"]
[eval exp="sf.probe = 1"]
[clearsysvar][emb exp="JSON.stringify(sf.probe)"]
[iscript]
f.fromIscript = 42;
[endscript]
[emb exp="f.fromIscript"]
[ignore exp="true"]
[emb exp="'SHOULD_NOT_APPEAR'"]
[endignore]
[ignore exp="false"]
[emb exp="'SHOULD_APPEAR'"]
[endignore]
[erasemacro name=nonexistent]
[trace exp="'TRACE_MSG'"]
[hr]
[hact]HotspotText[endhact]
[openvideo layer=0 storage="assets/clip.avi"]
[preparevideo layer=0 storage="assets/clip.avi"]
[playvideo layer=0]
[pausevideo]
[resumevideo]
[rewindvideo]
[emb exp="'W6DONE'"]
"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "log info [tempsave] place=1");
    assert_log_contains(&log, "save_write temp1"); // place>=0 はディスクの temp{place} スロットへ（W7）
    assert_log_contains(&log, "log info [tempload] place=1");
    assert_log_contains(&log, "save_read temp1");
    // tempload が f.x を 2 → 1 へ巻き戻す（復元後の再実行で emb が 1 を印字する）
    assert_log_contains(&log, &printed("1"));
    assert_log_contains(&log, "log info [record]");
    assert_log_contains(&log, "log info [goback]");
    assert_log_not_contains(&log, "log warn [goback]");
    // startanchor/gotostart は W7 で KAG3 準拠に修正（trail ではなく専用アンカースロット）。
    assert_log_contains(&log, "log info [startanchor] enabled=true");
    assert_log_contains(&log, "log info [gotostart]");
    assert_log_not_contains(&log, "log warn [gotostart]");
    assert_log_contains(&log, "log info [locksnapshot]");
    assert_log_contains(&log, "log info [unlocksnapshot]");
    assert_log_contains(&log, "log info [store] place=1");
    assert_log_contains(&log, "save_write store1"); // [store] は store{N} スロットへ書く
    assert_log_contains(&log, "log info [save] place=2");
    assert_log_contains(&log, "save_write save2");
    assert_log_contains(&log, "log info [load] place=2");
    assert_log_contains(&log, "save_read save2");
    assert_log_contains(&log, "log info [copybookmark] save2 -> save3");
    assert_log_contains(&log, "save_write save3");
    // erasebookmark は W7 で真の削除（save_delete）に変更（旧実装は空 JSON 書込みだった）。
    assert_log_contains(&log, "save_delete save3");
    assert_log_contains(&log, "log info [erasebookmark] save3");
    assert_log_contains(&log, "log info [disablestore] enabled=true");
    assert_log_contains(&log, "log info [disablestore] enabled=false");
    assert_log_contains(&log, "log info [clearvar]");
    assert_log_contains(&log, &printed("{}")); // clearvar 直後は f が空
    assert_log_contains(&log, "log info [clearsysvar]");
    assert_log_contains(&log, &printed("undefined")); // clearsysvar が sf.probe を消す
    assert_log_contains(&log, &printed("42")); // [iscript] が f.fromIscript を設定
    assert_log_not_contains(&log, "SHOULD_NOT_APPEAR"); // [ignore exp=true] がブロックを読み飛ばす
    assert_log_contains(&log, "SHOULD_APPEAR"); // [ignore exp=false] は素通り
    assert_log_contains(&log, "log info [erasemacro] name=nonexistent");
    assert_log_contains(&log, "TRACE_MSG");
    assert_log_contains(&log, &printed("HotspotText")); // [hact] は本文をそのまま表示
    assert_log_contains(&log, "log warn [hact]");
    assert_log_contains(&log, "log info [openvideo] layer=0 storage=assets/clip.avi");
    assert_log_contains(&log, "video_open 2 assets/clip.avi");
    assert_log_contains(
        &log,
        "log info [preparevideo] layer=0 storage=assets/clip.avi",
    );
    assert_log_contains(&log, "video_prepare 2 assets/clip.avi");
    assert_log_contains(&log, "log info [playvideo] layer=0");
    assert_log_contains(&log, "video_play_prepared 2");
    assert_log_contains(&log, "log info [pausevideo]");
    assert_log_contains(&log, "video_pause");
    assert_log_contains(&log, "log info [resumevideo]");
    assert_log_contains(&log, "video_resume");
    assert_log_contains(&log, "log info [rewindvideo]");
    assert_log_contains(&log, "video_rewind");
    assert_log_contains(&log, &printed("W6DONE"));
}

// =====================================================================
// W7: KAG3 意味論差異の解消 (2026-07-12)
// =====================================================================

/// [timeout] は KAG3 準拠に修正された: クリック待ち（waitAdvance/waitForJump）中でなければ
/// 発火しない。満了時にクリック待ち外だった場合は「満了済み」を記録するだけにし、次に
/// waitAdvance()/waitForJump() へ入った時点で即発火する（前半）。既に停止（[s]/[l]/[p]）に
/// 入っている間に満了する従来どおりのケースも変わらず動く（後半、旧サンプルの
/// 「[timeout]→[s]」パターン）。クリックは一切使わず、実時間の待ちだけで両方を検証する。
#[test]
fn w7_timeout_fires_only_during_click_wait() {
    let frag = r#"
[timeout time=80 target=*w7_deferred]
[wait time=250]
[emb exp="'AFTER_WAIT_BEFORE_L'"]
[l]
*w7_deferred
[emb exp="'DEFERRED_FIRED'"]
[timeout time=80 target=*w7_immediate]
[s]
*w7_immediate
[emb exp="'IMMEDIATE_FIRED'"]
"#;
    let log = run_v8(frag, &[]);
    // [wait]（クリック待ちではない）の間に満了しても、直後の emb はそのまま実行される
    // （まだジャンプしていない証拠）。
    assert_log_contains(&log, &printed("AFTER_WAIT_BEFORE_L"));
    // その次の [l]（クリック待ちへ入った瞬間）で、満了済みタイマーが即発火してジャンプする
    // （クリックは 1 回も送っていないので、[l] が実際にクリックを待っていたら
    // ハーネスの 10 秒デッドラインでこのテスト自体がタイムアウトする）。
    assert_log_contains(&log, &printed("DEFERRED_FIRED"));
    // 2 回目の [timeout] は直後に [s] へ入るため、旧実装と同じ「即発火」のまま
    // （クリックなしでも進む）。
    assert_log_contains(&log, &printed("IMMEDIATE_FIRED"));
}

/// [startanchor]/[gotostart] は W7 で KAG3 準拠に修正: [startanchor]（既定 enabled=true）は
/// 実行時点の完全スナップショットを専用アンカースロットへ記録し、[gotostart] はそこへ戻る
/// （[record]/[goback] の trail とは独立）。[startanchor enabled=false] はアンカーを破棄し、
/// その後の [gotostart] は warn するだけで何も起きないことを検証する。
#[test]
fn w7_startanchor_gotostart_use_dedicated_anchor_not_trail() {
    // [startanchor] は「その時点の完全スナップショット」（f を含む）を記録するので、
    // gotostart は変数もアンカー時点まで巻き戻す。カウンタにしておくと、もし f の復元が
    // 効いていなければ 2 (0+1+1) になってしまうところ、正しく巻き戻れば 1 (0+1, 巻き戻り後に
    // もう一度だけ +1) になる、という形で pc とデータ両方の復元を一度に検証できる。
    let frag = r#"
[eval exp="f.n = 0"]
[startanchor]
[eval exp="f.n = f.n + 1"]
[if exp="!sf.done"][eval exp="sf.done = true"][gotostart][endif]
[emb exp="'N=' + f.n"]
[startanchor enabled=false]
[gotostart]
"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "log info [startanchor] enabled=true");
    assert_log_contains(&log, "log info [gotostart]");
    assert_log_contains(&log, &printed("N=1"));
    assert_log_not_contains(&log, &printed("N=2"));
    assert_log_contains(&log, "log info [startanchor] enabled=false");
    // アンカーを破棄した後の [gotostart] は warn するだけ（trail 由来の「記録がありません」
    // ではなく「アンカーが設定されていません」に文言が変わっている）。
    assert_log_contains(&log, "log warn [gotostart] アンカーが設定されていません");
}

/// [tempsave]/[tempload] は W7 で place の符号によりディスク/メモリを使い分けるよう修正。
/// place>=0 はディスクの "temp{place}" スロット（save_write/save_read）、負数は従来通り
/// メモリのみ（ops を一切発行しない）。
#[test]
fn w7_tempsave_tempload_disk_for_nonnegative_memory_for_negative() {
    // tempload は「tempsave 実行直後の pc」まで巻き戻すため、tempsave と tempload の間に
    // ある `[eval f.x=99]` は巻き戻し後にもう一度実行されてしまう（既存の goback/gotostart
    // と同じ pc 復元の仕組み）。sf ガードで if ブロックごと 1 回だけに絞ることで、2 回目の
    // 通過では f.x を書き換える eval 自体をスキップし、「復元された値」を素直に検証できる
    // ようにする（`games/sample` の W6 デモ・上の w7_startanchor テストと同じ手法）。
    let frag = r#"
[eval exp="f.x = 1"]
[tempsave place=0]
[if exp="!sf.g0"][eval exp="sf.g0 = true"][eval exp="f.x = 99"][tempload place=0][endif]
[emb exp="'DISK_' + f.x"]
[eval exp="f.y = 1"]
[tempsave place=-1]
[if exp="!sf.g1"][eval exp="sf.g1 = true"][eval exp="f.y = 99"][tempload place=-1][endif]
[emb exp="'MEMORY_' + f.y"]
"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "save_write temp0"); // place>=0 → ディスク
    assert_log_contains(&log, "save_read temp0");
    assert_log_not_contains(&log, "temp-1"); // 負数 place はディスクに一切触れない（メモリのみ）
    assert_log_contains(&log, &printed("DISK_1")); // place=0（ディスク）からの復元
    assert_log_contains(&log, &printed("MEMORY_1")); // place=-1（メモリ）からの復元
}

#[test]
fn b14_bgm_position_round_trips_full_snapshots_in_both_engines() {
    let frag = r#"
[playbgm storage="assets/bgm/main.wav"]
[eval exp="f.positioned = sys.player.makeFullSnapshot(); sf.b14captured = f.positioned.bgm.positionMs; f.positioned.bgm.positionMs = 2345; sys.player.tempSlots.set(-14, f.positioned)"]
[if exp="!sf.b14positioned"][eval exp="sf.b14positioned = true"][tempload place=-14][endif]
[eval exp="f.old = sys.player.makeFullSnapshot(); f.old.bgm = { path: f.old.bgm.path, volume: f.old.bgm.volume, paused: false }; sys.player.tempSlots.set(-15, f.old)"]
[if exp="!sf.b14old"][eval exp="sf.b14old = true"][tempload place=-15][endif]
[eval exp="f.negative = sys.player.makeFullSnapshot(); f.negative.bgm.positionMs = -10; sys.player.tempSlots.set(-17, f.negative)"]
[if exp="!sf.b14negative"][eval exp="sf.b14negative = true"][tempload place=-17][endif]
[eval exp="f.nonfinite = sys.player.makeFullSnapshot(); f.nonfinite.bgm.positionMs = 1 / 0; sys.player.tempSlots.set(-18, f.nonfinite)"]
[if exp="!sf.b14nonfinite"][eval exp="sf.b14nonfinite = true"][tempload place=-18][endif]
[eval exp="f.nonnumber = sys.player.makeFullSnapshot(); f.nonnumber.bgm.positionMs = '2345'; sys.player.tempSlots.set(-19, f.nonnumber)"]
[if exp="!sf.b14nonnumber"][eval exp="sf.b14nonnumber = true"][tempload place=-19][endif]
[eval exp="f.paused = sys.player.makeFullSnapshot(); f.paused.bgm.positionMs = 2345; f.paused.bgm.paused = true; sys.player.tempSlots.set(-16, f.paused)"]
[if exp="!sf.b14paused"][eval exp="sf.b14paused = true"][tempload place=-16][endif]
[emb exp="'CAPTURED_' + sf.b14captured"]
[emb exp="'B14_DONE'"]
"#;
    let v8_log = run_v8(frag, &[]);
    let tsint_log = run_tsint(frag, &[]);
    assert_logs_match(&v8_log, &tsint_log);
    assert_log_contains(
        &v8_log,
        "audio_play bgm assets/bgm/main.wav loop=true vol=0.8 fade=300 start=2345",
    );
    assert_log_contains(
        &v8_log,
        "audio_play bgm assets/bgm/main.wav loop=true vol=0.8 fade=300 start=0",
    );
    assert_log_contains(
        &v8_log,
        "audio_play bgm assets/bgm/main.wav loop=true vol=0.8 fade=0 start=2345 paused=true",
    );
    assert_log_count(&v8_log, "start=2345", 2);
    assert_log_count(&v8_log, "start=0", 5);
    assert_log_not_contains(&v8_log, "audio_pause bgm");
    assert_log_contains(&v8_log, &printed("CAPTURED_0"));
    assert_log_contains(&v8_log, &printed("B14_DONE"));
}

/// [record]/[goback] の履歴リング (trail) は W7 でセーブデータへ永続化されるよう修正。
/// [save]/[load] をまたいでも goback が効くことを確認する（trail はディスクへ書かれ、
/// [load] のときだけ復元される。goback/gotostart/tempload では trail に触れない）。
///
/// [load] はラベル境界（ここでは唯一のラベル `*start`）からの再生のため、ロード後は
/// フラグメント全体が最初から再実行される（既存の W6 デモと同じ仕組み）。`[record]` 自体も
/// `sf` ガードで 1 回だけに絞らないと、再実行時に新しい trail エントリが積まれてしまい
/// 「ロードで trail が正しく復元されたか」を検出できなくなる（trail が空でも再実行時の
/// [record] がエントリを作ってしまい goback の warn が出ずテストが誤って通ってしまう）ため、
/// [record] にも `sf` ガードを掛けている。
#[test]
fn w7_trail_persists_across_save_and_load() {
    let frag = r#"
[if exp="!sf.recorded"][eval exp="sf.recorded = true"][record][endif]
[save place=9]
[if exp="!sf.loaded"][eval exp="sf.loaded = true"][load place=9][endif]
[if exp="!sf.backed"][eval exp="sf.backed = true"][goback][endif]
[emb exp="'W7_TRAIL_DONE'"]
"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "save_write save9");
    assert_log_contains(&log, "save_read save9");
    assert_log_contains(&log, "log info [goback]");
    // load 後の再実行では [record] が sf ガードでスキップされるため、goback が warn せずに
    // 成功するのは「ロードが trail をディスクから正しく復元した」場合のみ。
    assert_log_not_contains(&log, "log warn [goback]");
    assert_log_contains(&log, &printed("W7_TRAIL_DONE"));
}

/// [waittrig] は W7 で `kag` グローバル（KAG3 標準、`sys.player` と同一インスタンス）からも
/// 発火できるよう修正。後方互換の `sys.player.trigger` に加え `kag.trigger` でも動くことを
/// 確認する（`docs/kag-tag-compat.md`「waittrig」参照）。
#[test]
fn w7_waittrig_fires_via_kag_global() {
    let frag = r#"
[eval exp="void kag.wait(50).then(() => kag.trigger('kagtrig'))"]
[waittrig name=kagtrig]
[emb exp="'KAG_TRIGGERED'"]
"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "log info [waittrig] name=kagtrig 待機開始");
    assert_log_contains(&log, "log info [waittrig] name=kagtrig 発火");
    assert_log_contains(&log, &printed("KAG_TRIGGERED"));
}

/// [erasebookmark] は W7 で真の削除（新規 op `sys.saveDelete`）に修正。削除後は
/// `sys.saveRead` が null を返す（旧実装は空 JSON "{}" を書き込むだけで、スロット自体は
/// 残り続けていた）。`kag.api.sys.saveRead` 経由で確認する（kag グローバルの副次検証も兼ねる）。
#[test]
fn w7_erasebookmark_truly_deletes_the_slot() {
    let frag = r#"
[save place=5]
[erasebookmark place=5]
[emb exp="JSON.stringify(kag.api.sys.saveRead('save5'))"]
"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(&log, "save_write save5");
    assert_log_contains(&log, "save_delete save5");
    assert_log_contains(&log, &printed("null")); // 削除後は読めない
}

/// [pimage] は W7 で KAG3 標準の `sx=/sy=/sw=/sh=`（コピー元矩形）を `clip=` の別名として
/// 受理するよう修正。
#[test]
fn w7_pimage_accepts_sx_sy_sw_sh_as_clip_alias() {
    let frag = r#"[pimage layer=0 storage="assets/solid.png" sx=1 sy=1 sw=2 sh=2 x=3 y=4]"#;
    let log = run_v8(frag, &[]);
    assert_log_contains(
        &log,
        "layer_blit_image 2 fore assets/solid.png dx=3 dy=4 clip=Some(BlitClip { x: 1, y: 1, w: 2, h: 2 })",
    );
}

/// [glyph] は W7 で修正: fix=false（既定）はクリック待ち開始時のペン位置（最終グリフの右隣）
/// へ追従する。本文が無い場合はメッセージウィンドウ左上マージン位置へフォールバックする
/// （ここではそのフォールバック経路を検証する）。fix=true は従来通り固定位置
/// （left=/top= 省略時はウィンドウ右下）のまま。
#[test]
fn w7_glyph_fix_false_default_follows_pen_fix_true_stays_fixed() {
    let frag = r#"
[glyph line="assets/solid.png"]
[l]
テスト[glyph line="assets/solid.png" fix=true]
[l]
"#;
    let log = run_v8(frag, &[(5.0, 5.0), (5.0, 5.0)]);
    // fix=false・本文なし: メッセージウィンドウ (left=40 top=493 marginX=30 marginY=18) の
    // 左上マージン位置 (70, 511) へフォールバックする。
    assert_log_contains(
        &log,
        "layer_set_props 4 fore LayerProps { x: Some(70), y: Some(511)",
    );
    // fix=true: 本文があっても固定位置（ウィンドウ右下、left=40+1200-40=1200, top=493+203-40=656）。
    assert_log_contains(
        &log,
        "layer_set_props 4 fore LayerProps { x: Some(1200), y: Some(656)",
    );
}

/// [showhistory] は既存のバックログ UI（H キーと同じ実体）を開く。オーバーレイの
/// テキストレイヤが作成・表示され、クリックで閉じてシナリオが続行することを検証する。
#[test]
fn w6_showhistory_opens_and_click_closes_backlog() {
    let frag = "履歴に残る本文[r]\n[showhistory]\n[emb exp=\"'AFTERHISTORY'\"]";
    let log = run_v8(frag, &[(10.0, 10.0)]);
    // オーバーレイ用のテキストレイヤ (id=4) が作られる。
    assert_log_contains(&log, "layer_new text -> 4");
    assert_log_contains(&log, "履歴に残る本文");
    assert_log_contains(&log, &printed("AFTERHISTORY")); // クリックで閉じて続行した
}

// =====================================================================
// ➖ 対象外スタブ 10 種: warn 1 回 + ops 無発行
// =====================================================================

#[test]
fn stub_tags_warn_once_and_emit_no_ops() {
    let frag = r#"
[trace exp="'STUB_MARK_BEGIN'"]
[loadplugin][loadplugin]
[mapimage][mapimage]
[mapaction][mapaction]
[mapdisable][mapdisable]
[mappfont][mappfont]
[videolayer][videolayer]
[clearvideolayer][clearvideolayer]
[setbgmlabel][setbgmlabel]
[clearbgmlabel][clearbgmlabel]
[cursor][cursor]
"#;
    let log = run_v8(frag, &[]);
    let stub_names = [
        "loadplugin",
        "mapimage",
        "mapaction",
        "mapdisable",
        "mappfont",
        "videolayer",
        "clearvideolayer",
        "setbgmlabel",
        "clearbgmlabel",
        "cursor",
    ];
    for name in stub_names {
        let warns = log
            .iter()
            .filter(|l| l.contains(&format!("log warn [{name}]")))
            .count();
        assert_eq!(
            warns, 1,
            "スタブタグ `{name}` の warn がちょうど1回のはず（2回呼んでいる）: {log:#?}"
        );
    }
    // スタブは ops を一切発行しない: マーカー（trace）以降、[close] の quit までの間に
    // log 以外の ops（layer_/text_/audio_/video_/trans_ 系）が 1 件も無いことを確認する
    // （起動時の初期化 ops はマーカーより前に出るため除外できる）。
    let begin = log
        .iter()
        .position(|l| l.contains("STUB_MARK_BEGIN"))
        .expect("マーカー（trace）が見つからない");
    let non_log_ops: Vec<_> = log[begin + 1..]
        .iter()
        // [close] は読了したラベル区間を既読にして sf を保存してから quit する（save_write sys）。
        .filter(|l| !l.starts_with("log ") && *l != "quit" && *l != "save_write sys")
        .collect();
    assert!(
        non_log_ops.is_empty(),
        "スタブタグが ops を発行しています: {non_log_ops:#?}"
    );
}

// =====================================================================
// カバレッジ照合: ✅151 タグが全てこのファイルのフラグメント断片群に出現することを
// 機械照合する（`crates/noiria-app/tests/kag_tag_compat.rs` と同形式、W7 の「表とコードの
// 機械照合」に倣う恒久テスト）。断片は全てこのファイル内の生文字列リテラルに集約されている
// ので、このソースファイル自体をテキストとして走査すれば十分（新規のデータファイルは作らない
// 設計、CLAUDE.md の「単純に済むところを複雑にしない」方針）。今後タグを増やしたのに対応する
// フラグメントを書き忘れると、このテストが「タグ名がソース中に見つからない」形で fail する。
// =====================================================================

#[test]
fn all_ok_tags_from_compat_table_appear_in_fragment_corpus() {
    let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let compat_md = std::fs::read_to_string(workspace_root.join("docs/kag-tag-compat.md"))
        .expect("docs/kag-tag-compat.md を読み込めません");
    let this_source = std::fs::read_to_string(
        workspace_root.join("crates/noiria-app/src/script/tag_behavior_tests.rs"),
    )
    .expect("tag_behavior_tests.rs を読み込めません（自己走査用）");

    // docs/kag-tag-compat.md の「## 対応表」セクションから ✅ 行のタグ名だけを抜き出す
    // （`crates/noiria-app/tests/kag_tag_compat.rs::parse_compat_table` と同じ簡易パーサ）。
    let mut ok_tags: Vec<String> = Vec::new();
    let mut in_section = false;
    for line in compat_md.lines() {
        if line.starts_with("## 対応表") {
            in_section = true;
            continue;
        }
        if line.starts_with("## ウェーブ割当") {
            break;
        }
        if !in_section {
            continue;
        }
        let line = line.trim();
        if !line.starts_with("| `") {
            continue;
        }
        let cols: Vec<&str> = line.split('|').map(|c| c.trim()).collect();
        if cols.len() < 3 {
            continue;
        }
        let tag = cols[1].trim_matches('`').to_string();
        let status = cols[2];
        if status.contains('✅') {
            ok_tags.push(tag);
        }
    }
    assert_eq!(
        ok_tags.len(),
        151,
        "✅ タグ数が151ではありません: {}",
        ok_tags.len()
    );

    // このファイル中の `[tagname` 出現を「`[` の直後から次の非識別子文字まで」をタグ名として
    // 抽出する（実際の KAG パーサのタグ名抽出と同じ考え方。`[l]` と `[link]` のような前方一致の
    // 混同を避けるため、部分文字列一致ではなく完全なトークン一致にする）。
    let mut found_tags: std::collections::HashSet<String> = std::collections::HashSet::new();
    let bytes = this_source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end > start {
                found_tags.insert(this_source[start..end].to_string());
            }
        }
        i += 1;
    }

    let missing: Vec<&String> = ok_tags
        .iter()
        .filter(|t| !found_tags.contains(*t))
        .collect();
    assert!(
        missing.is_empty(),
        "✅ タグのうちフラグメント断片に出現しないものがあります（テスト漏れ）: {missing:#?}"
    );
}
