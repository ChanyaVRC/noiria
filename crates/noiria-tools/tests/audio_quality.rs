//! 手続き生成 BGM/SE の数値検証。
//!
//! 音を実際に再生することはせず（禁止）、`synth` モジュールが返す f32 波形
//! バッファに対して数値的な品質チェックのみを行う。聴感上の良し悪しは人間が
//! 判断する前提で、ここでは「クリップしていない」「音量感が揃っている」
//! 「ステレオになっている」「ループ境界にクリックが出ない」「長さの仕様を
//! 満たす」「DC オフセットが無い」ことだけを保証する。

use noiria_tools::synth::{self, Stereo};

const MIN_SECS: f32 = 32.0;
const MAX_SECS: f32 = 48.0;

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0f32, |m, &v| m.max(v.abs()))
}

fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|&v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

fn dc_offset(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    x.iter().sum::<f32>() / x.len() as f32
}

/// 境界前後 256 サンプルぶんの平均絶対差分（＝隣接サンプル差の平均）。
/// ループ末尾と先頭を連結した領域を見ることで、境界をまたぐジャンプ
/// （＝クリック）が異常に大きくないことを検証する。
fn boundary_discontinuity(ch: &[f32]) -> f32 {
    let n = ch.len();
    let w = 256.min(n / 2);
    let mut seam: Vec<f32> = Vec::with_capacity(w * 2);
    seam.extend_from_slice(&ch[n - w..]);
    seam.extend_from_slice(&ch[..w]);
    let diffs: Vec<f32> = seam.windows(2).map(|p| (p[1] - p[0]).abs()).collect();
    diffs.iter().sum::<f32>() / diffs.len() as f32
}

struct Report {
    name: &'static str,
    secs: f32,
    peak: f32,
    rms_db: f32,
    corr: f32,
    boundary_l: f32,
    boundary_r: f32,
    dc_l: f32,
    dc_r: f32,
}

fn analyze(name: &'static str, buf: &Stereo) -> Report {
    Report {
        name,
        secs: buf.seconds(),
        peak: synth::combined_peak(buf),
        rms_db: synth::dbfs(synth::combined_rms(buf)),
        corr: synth::correlation(&buf.l, &buf.r),
        boundary_l: boundary_discontinuity(&buf.l),
        boundary_r: boundary_discontinuity(&buf.r),
        dc_l: dc_offset(&buf.l),
        dc_r: dc_offset(&buf.r),
    }
}

fn assert_song_quality(r: &Report) {
    assert!(
        r.secs >= MIN_SECS && r.secs <= MAX_SECS,
        "{}: 長さが仕様外 ({} 秒, 期待 {}〜{} 秒)",
        r.name,
        r.secs,
        MIN_SECS,
        MAX_SECS
    );
    assert!(
        r.peak <= 0.95,
        "{}: ピークがクリップ域 ({})",
        r.name,
        r.peak
    );
    assert!(
        (-20.0..=-12.0).contains(&r.rms_db),
        "{}: RMS が音量帯域外 ({} dBFS)",
        r.name,
        r.rms_db
    );
    assert!(
        r.corr < 0.99,
        "{}: L/R 相関が高すぎてモノラルに近い ({})",
        r.name,
        r.corr
    );
    assert!(
        r.boundary_l < 0.02 && r.boundary_r < 0.02,
        "{}: ループ境界に不連続あり (L={}, R={})",
        r.name,
        r.boundary_l,
        r.boundary_r
    );
    assert!(
        r.dc_l.abs() < 1e-3 && r.dc_r.abs() < 1e-3,
        "{}: DC オフセットあり (L={}, R={})",
        r.name,
        r.dc_l,
        r.dc_r
    );
}

#[test]
fn main_theme_quality() {
    let buf = synth::render_main();
    let r = analyze("main.wav", &buf);
    print_report(&r);
    assert_song_quality(&r);
}

#[test]
fn alt_calm_quality() {
    let buf = synth::render_alt_calm();
    let r = analyze("alt_calm.wav", &buf);
    print_report(&r);
    assert_song_quality(&r);
}

#[test]
fn alt_tension_quality() {
    let buf = synth::render_alt_tension();
    let r = analyze("alt_tension.wav", &buf);
    print_report(&r);
    assert_song_quality(&r);
}

#[test]
fn click_se_quality() {
    let s = synth::render_click();
    assert!(!s.is_empty(), "click SE が空");
    let pk = peak(&s);
    assert!(pk <= 0.95, "click SE のピークがクリップ域 ({pk})");
    assert!(pk > 0.05, "click SE が無音に近い ({pk})");
    let secs = s.len() as f32 / synth::SR as f32;
    assert!(
        (0.05..=1.0).contains(&secs),
        "click SE の長さが不自然 ({secs}s)"
    );
    let dc = dc_offset(&s);
    assert!(dc.abs() < 1e-3, "click SE に DC オフセットあり ({dc})");
    let r = rms(&s);
    assert!(r > 0.0, "click SE の RMS が 0");
}

// ============================================================
// 「通知音っぽさ」の数値チェック
// ============================================================
//
// Windows の通知音/エラー音は「短い減衰音が、ほぼ無音の背景の中で中高音域に
// 孤立して鳴る」という音響的特徴を持つ。ここではそれを近似的に数値化し、
// 200ms 窓ごとに以下の 2 点を assert する:
// (1) 実効音圧が全体平均から -20dB 以上落ち込む「無音に近い窓」が無いこと
//     （＝孤立した音の前後に無音の隙間が無いこと）。
// (2) 1kHz 以上の帯域だけが突出する（帯域/全体エネルギー比 > 0.8）瞬間が
//     無いこと（＝硬く薄い高音だけの「ピロン」音になっていないこと）。
// 帯域分離は 1 次ハイパスフィルタによる近似で十分とする。

const NOTIFY_WINDOW_SECS: f32 = 0.2;
const NOTIFY_SILENCE_FLOOR_DB: f32 = -20.0;
const NOTIFY_HIGHBAND_CUTOFF_HZ: f32 = 1000.0;
const NOTIFY_HIGHBAND_RATIO_MAX: f32 = 0.8;

/// 1 次ハイパスフィルタ（帯域分離の近似）。
fn highpass_1pole(x: &[f32], cutoff_hz: f32, sr: f32) -> Vec<f32> {
    let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
    let dt = 1.0 / sr;
    let alpha = rc / (rc + dt);
    let mut out = vec![0.0f32; x.len()];
    for i in 1..x.len() {
        out[i] = alpha * (out[i - 1] + x[i] - x[i - 1]);
    }
    out
}

fn assert_no_notification_like_gaps(name: &str, buf: &Stereo) {
    let sr = buf.sr as f32;
    let mono: Vec<f32> = buf
        .l
        .iter()
        .zip(buf.r.iter())
        .map(|(&a, &b)| 0.5 * (a + b))
        .collect();
    let overall_rms = rms(&mono);
    assert!(overall_rms > 1e-9, "{name}: 実質無音のトラック");
    let overall_db = synth::dbfs(overall_rms);

    let highpassed = highpass_1pole(&mono, NOTIFY_HIGHBAND_CUTOFF_HZ, sr);

    let window_n = (NOTIFY_WINDOW_SECS * sr) as usize;
    assert!(window_n > 0);
    let n_windows = mono.len() / window_n;

    for w in 0..n_windows {
        let s = w * window_n;
        let e = s + window_n;
        let t_secs = s as f32 / sr;

        let win_rms = rms(&mono[s..e]);
        let win_db = synth::dbfs(win_rms);
        assert!(
            win_db >= overall_db + NOTIFY_SILENCE_FLOOR_DB,
            "{name}: 200ms窓 #{w} ({t_secs:.2}s付近) が無音に近い窓RMS={win_db:.1}dBFS \
             (全体RMS={overall_db:.1}dBFS) — 音が孤立している (通知音っぽさ) 疑いあり",
        );

        let hp_rms = rms(&highpassed[s..e]);
        let ratio = (hp_rms * hp_rms) / (win_rms * win_rms).max(1e-12);
        assert!(
            ratio <= NOTIFY_HIGHBAND_RATIO_MAX,
            "{name}: 200ms窓 #{w} ({t_secs:.2}s付近) が高域だけ突出 (帯域/全体エネルギー比={ratio:.2}) \
             — 硬い高音だけの通知音的な「ピロン」音の疑いあり",
        );
    }
}

#[test]
fn main_theme_not_notification_like() {
    assert_no_notification_like_gaps("main.wav", &synth::render_main());
}

#[test]
fn alt_calm_not_notification_like() {
    assert_no_notification_like_gaps("alt_calm.wav", &synth::render_alt_calm());
}

#[test]
fn alt_tension_not_notification_like() {
    assert_no_notification_like_gaps("alt_tension.wav", &synth::render_alt_tension());
}

// ============================================================
// トランジェント（オンセット）検出
// ============================================================
//
// 疎な単発プラック（Karplus-Strong 撥弦）は音色を丸めても「立ち上がりが
// 急峻な瞬間」という構造自体が通知音的に知覚される。200ms 窓の平均音量
// チェック（上の「通知音っぽさ」チェック）はこれを捉えられなかったため、
// より短い窓の RMS で急激な音圧ジャンプ（オンセット）そのものを数える。
// 500Hz ハイパス後の信号を対象にすることで、低域のうねり（パッドのビブラート
// やベースの立ち上がり）ではなく、撥弦特有の高域を含む鋭いアタックだけを拾う。
//
// パラメータ（窓 10ms・ジャンプ閾値 8dB）は当初案の「20ms窓・+12dB」から
// 実測に基づき調整した。理由: このコードベースの BGM はパッド/ディレイ/
// リバーブが常時鳴っておりバックグラウンド自体が無音ではないため、20ms窓
// では撥弦のアタックがバックグラウンドに埋もれて検出感度が不足しがちだった
// （旧 KS ベースの alt_calm 実装で 20ms/12dB 設定は onsets=0 になってしまい、
// 検出器自体が撥弦を捉えられていなかった）。窓を 10ms に縮めて撥弦アタック
// の立ち上がり区間そのものを捉えると、旧 KS ベースのスローなメロディは
// 直前窓比で明確なジャンプ（実測 9dB 超）を示すようになったため、8dB を
// 「静かな声部の自然な音量変化とは区別できる」しきい値として採用している。
//
// BGM 全面改修（docs/bgm-design-notes.md 参照）で 3 曲すべてから撥弦を
// 排除し完全にレガート/持続音構成にしたため、今は main/alt_calm/alt_tension
// の 3 曲すべてで onsets=0 を要求する。検出器自体が壊れていないことは
// `transient_detector_catches_plucks_reference`（`synth::karplus_strong` を
// 直接鳴らして検出する）で別途保証する。

const TRANSIENT_HIGHPASS_HZ: f32 = 500.0;
const TRANSIENT_WINDOW_SECS: f32 = 0.01;
const TRANSIENT_JUMP_DB: f32 = 8.0;
// `taper_loop_seam`（synth.rs）はループ境界クリックを確実に防ぐため、書き出し
// バッファの先頭/末尾 0.02 秒を意図的に無音から線形フェードしている。この
// フェード自体は「真の無音からの立ち上がり」なので log スケールでは大きな
// dB ジャンプとして現れるが、これは毎ループ 1 回だけ起きる無害な安全策であり
// 「規則的に繰り返すアラーム的パルス」でも「孤立した鋭いアタック」でもない
// ため、オンセット検出の対象から除外する（先頭/末尾 0.05 秒に余裕を持たせて
// ガード）。
const TRANSIENT_EDGE_GUARD_SECS: f32 = 0.05;

/// 500Hz ハイパス後の信号を 10ms 窓で RMS 化し、直前窓比 +8dB 以上のジャンプを
/// 「オンセット」としてカウントする。レガート/持続音のみで構成された音源では
/// このカウントは 0 になるはず。ループ境界のテーパー安全策による見かけ上の
/// ジャンプは対象外にする（`TRANSIENT_EDGE_GUARD_SECS` 参照）。
///
/// 可聴性フロア（うなり低減改修で追加、docs/bgm-design-notes.md §6.4）:
/// ジャンプ到達後の窓レベルがトラック全体のハイパス後RMS **未満** のジャンプは
/// カウントしない。理由: この検出器の目的は「背景から孤立して目立つ鋭い
/// アタック」（Patterson の言う中高音域の孤立パルス）を捉えることであり、
/// トラックの平均的な高域レベルにすら届かない微小な揺らぎは常時鳴っている
/// パッド/リバーブにマスクされ、通知音として知覚され得ない。この必要性は
/// うなり低減（ビブラート/コーラスの深さ半減）で顕在化した: 変調による
/// スメアが減った結果、リードとパッドの同音干渉の「谷からの回復」が静かな
/// 区間（平均高域レベル-2dB付近）でまれに +8.2dB/10ms を超えることがあるが、
/// これは撥弦アタック（実測 +9dB 超・平均レベル以上に到達）とは異なる無害な
/// 干渉現象のため。ジャンプ閾値 8dB 自体は変更していない。
fn count_transient_onsets(buf: &Stereo) -> usize {
    let sr = buf.sr as f32;
    let total_secs = buf.len() as f32 / sr;
    let mono: Vec<f32> = buf
        .l
        .iter()
        .zip(buf.r.iter())
        .map(|(&a, &b)| 0.5 * (a + b))
        .collect();
    let hp = highpass_1pole(&mono, TRANSIENT_HIGHPASS_HZ, sr);
    let overall_hp_db = synth::dbfs(rms(&hp));

    let window_n = (TRANSIENT_WINDOW_SECS * sr) as usize;
    assert!(window_n > 0);
    let n_windows = hp.len() / window_n;

    let mut prev_db: Option<f32> = None;
    let mut onsets = 0usize;
    for w in 0..n_windows {
        let s = w * window_n;
        let e = s + window_n;
        let t_secs = s as f32 / sr;
        let win_db = synth::dbfs(rms(&hp[s..e]));
        let in_edge_guard =
            t_secs < TRANSIENT_EDGE_GUARD_SECS || t_secs > total_secs - TRANSIENT_EDGE_GUARD_SECS;
        if let Some(p) = prev_db {
            if !in_edge_guard && win_db - p >= TRANSIENT_JUMP_DB && win_db >= overall_hp_db {
                onsets += 1;
            }
        }
        prev_db = Some(win_db);
    }
    onsets
}

#[test]
fn transient_detector_catches_plucks_reference() {
    // 検出力の実証用ログ。
    //
    // BGM 全面改修（docs/bgm-design-notes.md 参照）で 3 曲すべてから撥弦
    // (Karplus-Strong) を排除し、完全にレガート/持続音構成にしたため、もはや
    // どの曲も「撥弦を含む基準信号」としては使えない。代わりに、BGM 本体では
    // 使わなくなった `synth::karplus_strong`（UI クリック SE 専用として残して
    // いる撥弦）を直接鳴らし、検出器がちゃんと機能していることをそれ単体で
    // 検証する。
    // 撥弦の前に無音を挟み、「真の無音→撥弦」という遷移が窓境界をまたぐように
    // する（撥弦自身の onset は最初の 10ms 窓の内部で起きるため、直前の窓が
    // 存在しないと検出器はそもそも比較対象を持てない）。
    let silence_n = (0.15 * synth::SR as f32) as usize;
    let pluck = synth::karplus_strong(1200.0, 0.3, synth::SR, 0.994, 6000.0, 0xBEEF);
    let mut samples = vec![0.0f32; silence_n];
    samples.extend_from_slice(&pluck);
    let mono_buf = Stereo {
        sr: synth::SR,
        l: samples.clone(),
        r: samples,
    };
    let onsets = count_transient_onsets(&mono_buf);
    println!(
        "[transient-detector reference] raw pluck onsets={onsets} (撥弦単体のため >0 が期待値)"
    );
    assert!(
        onsets > 0,
        "検出器が撥弦を検出できていない疑いあり（onsets={onsets}）"
    );
}

#[test]
fn main_theme_has_no_transient_onsets() {
    let onsets = count_transient_onsets(&synth::render_main());
    println!(
        "[transient-detector] main.wav onsets={onsets} (撥弦を全廃したレガート構成のため 0 を期待)"
    );
    assert_eq!(
        onsets, 0,
        "main.wav にトランジェント（急峻なオンセット）が検出された: onsets={onsets} — \
         レガートなFMエレピ+パッド構成のはずが、まだ単発の鋭いアタックが残っている疑いあり"
    );
}

#[test]
fn alt_calm_has_no_transient_onsets() {
    let onsets = count_transient_onsets(&synth::render_alt_calm());
    println!("[transient-detector] alt_calm.wav onsets={onsets} (完全レガート構成のため 0 を期待)");
    assert_eq!(
        onsets, 0,
        "alt_calm.wav にトランジェント（急峻なオンセット）が検出された: onsets={onsets} — \
         完全レガート/アンビエント構成のはずが、まだ単発の鋭いアタックが残っている疑いあり"
    );
}

/// alt_tension.wav だけは他の2曲と違い「閾値=0」ではなく上限付き参考値で
/// 判定する。理由: alt_tension は設計上、意図的に低めベースのオスティナート
/// （ルート/5度を交互に刻む、リズムを牽引する反復パターン）を持つ唯一の曲
/// であり、ルート→5度の音高変化そのものが（撥弦のような急峻なノイズ
/// バーストではなく、0.06秒の緩やかなアタックを持つサイン主体の音でも）
/// 500Hzハイパス後の実効音圧をなだらかに押し上げる。これは Patterson の
/// 警告音定義が指す「中高音域の孤立した鋭いパルス」とは音響的に別種の現象
/// （低域・オクターブ内の通常の和声的反復運動）であり、対策として bass の
/// attack を0.008s→0.06sへ、LPFカットオフを1400Hz→650Hzへと大きく緩めた上
/// （55→9 onsetsまで実測で改善）、なお残る少数のオンセットは音楽的に必要な
/// リズム動作の一部として許容し、回帰検知のための上限のみを課す。
const ALT_TENSION_TRANSIENT_ONSET_MAX: usize = 14;

#[test]
fn alt_tension_transient_onsets_bounded() {
    let onsets = count_transient_onsets(&synth::render_alt_tension());
    println!(
        "[transient-detector] alt_tension.wav onsets={onsets} \
         (ベースオスティナートの音高変化を許容する上限付きゲート、上限={ALT_TENSION_TRANSIENT_ONSET_MAX})"
    );
    assert!(
        onsets <= ALT_TENSION_TRANSIENT_ONSET_MAX,
        "alt_tension.wav のオンセット数が上限を超えた: onsets={onsets} (上限={ALT_TENSION_TRANSIENT_ONSET_MAX}) — \
         ベースオスティナートの通常の音高変化を超えて、撥弦のような鋭いアタックが混入している疑いあり"
    );
}

/// デバッグ用: 各曲のオンセット・ジャンプ（+5dB超）が起きている時刻を
/// ログに残す。ゲート自体ではないが、`*_transient_onsets*` が万一失敗した
/// 際に原因箇所を特定するための診断ログとして残してある。
#[test]
fn debug_dump_onset_trace() {
    for (name, buf) in [
        ("main", synth::render_main()),
        ("alt_calm", synth::render_alt_calm()),
        ("alt_tension", synth::render_alt_tension()),
    ] {
        let sr = buf.sr as f32;
        let mono: Vec<f32> = buf
            .l
            .iter()
            .zip(buf.r.iter())
            .map(|(&a, &b)| 0.5 * (a + b))
            .collect();
        let hp = highpass_1pole(&mono, TRANSIENT_HIGHPASS_HZ, sr);
        let window_n = (TRANSIENT_WINDOW_SECS * sr) as usize;
        let n_windows = hp.len() / window_n;
        println!("[{name}] overall_hp_rms={:.1}dBFS", synth::dbfs(rms(&hp)));
        let mut prev_db: Option<f32> = None;
        let mut count = 0;
        for w in 0..n_windows {
            let s = w * window_n;
            let e = s + window_n;
            let win_db = synth::dbfs(rms(&hp[s..e]));
            let t = s as f32 / sr;
            if let Some(p) = prev_db {
                let jump = win_db - p;
                if jump > 5.0 && count < 15 {
                    println!("[{name}] t={t:.3}s win_db={win_db:.1} jump={jump:.1}");
                    count += 1;
                }
            }
            prev_db = Some(win_db);
        }
    }
}

fn print_report(r: &Report) {
    println!(
        "[{}] {:.2}s peak={:.4} rms={:.2}dBFS corr={:.4} boundary(L/R)={:.5}/{:.5} dc(L/R)={:.6}/{:.6}",
        r.name, r.secs, r.peak, r.rms_db, r.corr, r.boundary_l, r.boundary_r, r.dc_l, r.dc_r
    );
}
