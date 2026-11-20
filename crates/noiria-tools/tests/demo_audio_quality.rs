//! デモゲーム用 BGM 5 曲・SE 3 本（`synth::demo`）の数値検証。音は再生しない。
//!
//! ユーザー指摘（機械的・単調 / 不協和 / 音色が安っぽい）への対応ごとに、
//! 守るテストを分けてある。
//!
//! - 単調・機械的: 旋律の反復（小節/2 小節/4 小節の自己類似）、息継ぎ、フレーズ内の頂点、
//!   3 部以上の構成、伴奏型の変化、フレーズ内の強弱（1 秒 RMS 変動）、音量/音色の曲間差。
//! - 不協和: 旋律と伴奏の同時発音（短 2 度・短 9 度・三全音）ゼロ、強拍の和声音率、
//!   経過音の長さ/調内、低域（C3 以下）の音程間隔、ストリングスのデチューン幅、
//!   和音交代直後に前の和音が残らない（クロマ検査）。
//! - 音色: リファレンス（実 BGM）の帯域バランス・スペクトル重心・高域比（こもり/刺さり）。
//!   リファレンスの数値は docs/bgm-design-notes.md §5/§6 の実測レンジ。

use std::sync::OnceLock;

use noiria_tools::analysis::{self, Complex, Metrics};
use noiria_tools::synth::demo::{self, Comp, SongSpec, TimeMap};
use noiria_tools::synth::{self, Stereo};

const MIN_SECS: f32 = 32.0;
const MAX_SECS: f32 = 64.0;

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

fn boundary_discontinuity(ch: &[f32]) -> f32 {
    let n = ch.len();
    let w = 256.min(n / 2);
    let mut seam: Vec<f32> = Vec::with_capacity(w * 2);
    seam.extend_from_slice(&ch[n - w..]);
    seam.extend_from_slice(&ch[..w]);
    let diffs: Vec<f32> = seam.windows(2).map(|p| (p[1] - p[0]).abs()).collect();
    diffs.iter().sum::<f32>() / diffs.len() as f32
}

// ============================================================
// 5 曲のレンダリング結果（一度だけ作って全テストで共有）
// ============================================================

struct Song {
    name: &'static str,
    spec: SongSpec,
    audio: Stereo,
    metrics: Metrics,
}

fn songs() -> &'static Vec<Song> {
    static S: OnceLock<Vec<Song>> = OnceLock::new();
    S.get_or_init(|| {
        let defs: Vec<(&'static str, SongSpec)> = vec![
            ("title", demo::title_spec()),
            ("daily", demo::daily_spec()),
            ("calm", demo::calm_spec()),
            ("tension", demo::tension_spec()),
            ("night", demo::night_spec()),
        ];
        std::thread::scope(|sc| {
            let hs: Vec<_> = defs
                .into_iter()
                .map(|(name, spec)| {
                    sc.spawn(move || {
                        let audio = demo::render_song(&spec);
                        let metrics = analysis::analyze_track(name, &audio);
                        Song {
                            name,
                            spec,
                            audio,
                            metrics,
                        }
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        })
    })
}

// ============================================================
// 基本品質（audio_quality.rs の assert_song_quality と同じ閾値。長さのみ 32〜64 秒）
// ============================================================

#[test]
fn songs_basic_quality() {
    for s in songs() {
        let b = &s.audio;
        let secs = b.seconds();
        let pk = synth::combined_peak(b);
        let rms_db = synth::dbfs(synth::combined_rms(b));
        let corr = synth::correlation(&b.l, &b.r);
        let (bl, br) = (boundary_discontinuity(&b.l), boundary_discontinuity(&b.r));
        println!(
            "[{}] {secs:.2}s peak={pk:.3} rms={rms_db:.2}dBFS corr={corr:.3} boundary={bl:.5}/{br:.5}",
            s.name
        );
        assert!(
            (MIN_SECS..=MAX_SECS).contains(&secs),
            "{}: 長さが仕様外 ({secs})",
            s.name
        );
        let expect = TimeMap::new(&s.spec).total();
        assert!(
            (secs - expect).abs() < 0.01,
            "{}: 長さが拍の時間写像と不一致",
            s.name
        );
        assert!(pk <= 0.95, "{}: ピークがクリップ域 ({pk})", s.name);
        assert!(
            (-20.0..=-12.0).contains(&rms_db),
            "{}: RMS が音量帯域外 ({rms_db})",
            s.name
        );
        assert!(corr < 0.99, "{}: L/R 相関が高すぎる ({corr})", s.name);
        assert!(
            bl < 0.02 && br < 0.02,
            "{}: ループ境界に不連続 ({bl}/{br})",
            s.name
        );
        assert!(
            dc_offset(&b.l).abs() < 1e-3 && dc_offset(&b.r).abs() < 1e-3,
            "{}: DC オフセット",
            s.name
        );
    }
}

// ============================================================
// 「通知音っぽさ」（audio_quality.rs と同一基準）
// ============================================================

const NOTIFY_WINDOW_SECS: f32 = 0.2;
const NOTIFY_SILENCE_FLOOR_DB: f32 = -20.0;
const NOTIFY_HIGHBAND_CUTOFF_HZ: f32 = 1000.0;
const NOTIFY_HIGHBAND_RATIO_MAX: f32 = 0.8;

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

fn mono_of(buf: &Stereo) -> Vec<f32> {
    buf.l
        .iter()
        .zip(buf.r.iter())
        .map(|(&a, &b)| 0.5 * (a + b))
        .collect()
}

#[test]
fn songs_not_notification_like() {
    for s in songs() {
        let sr = s.audio.sr as f32;
        let mono = mono_of(&s.audio);
        let overall_db = synth::dbfs(rms(&mono));
        let hp = highpass_1pole(&mono, NOTIFY_HIGHBAND_CUTOFF_HZ, sr);
        let wn = (NOTIFY_WINDOW_SECS * sr) as usize;
        for w in 0..mono.len() / wn {
            let (a, b) = (w * wn, w * wn + wn);
            let win_db = synth::dbfs(rms(&mono[a..b]));
            assert!(
                win_db >= overall_db + NOTIFY_SILENCE_FLOOR_DB,
                "{}: 200ms窓 #{w} が無音に近い ({win_db:.1} vs 全体 {overall_db:.1})",
                s.name
            );
            let hp_rms = rms(&hp[a..b]);
            let ratio = (hp_rms * hp_rms) / (rms(&mono[a..b]).powi(2)).max(1e-12);
            assert!(
                ratio <= NOTIFY_HIGHBAND_RATIO_MAX,
                "{}: 200ms窓 #{w} が高域だけ突出 ({ratio:.2})",
                s.name
            );
        }
    }
}

// ============================================================
// トランジェント（audio_quality.rs と同一実装・同一閾値）。
// ピアノは打鍵があるので 0 件は要求しないが、「撥弦/通知音のような急峻な立ち上がり」
// （+8dB/10ms を超えて全体の高域 RMS 以上へ達する）の頻度に上限を置く。
// ============================================================

const TRANSIENT_HIGHPASS_HZ: f32 = 500.0;
const TRANSIENT_WINDOW_SECS: f32 = 0.01;
const TRANSIENT_JUMP_DB: f32 = 8.0;
const TRANSIENT_EDGE_GUARD_SECS: f32 = 0.05;

fn count_transient_onsets(buf: &Stereo, jump_db: f32) -> usize {
    let sr = buf.sr as f32;
    let total_secs = buf.len() as f32 / sr;
    let hp = highpass_1pole(&mono_of(buf), TRANSIENT_HIGHPASS_HZ, sr);
    let overall_hp_db = synth::dbfs(rms(&hp));
    let wn = (TRANSIENT_WINDOW_SECS * sr) as usize;
    let mut prev: Option<f32> = None;
    let mut onsets = 0usize;
    for w in 0..hp.len() / wn {
        let t = (w * wn) as f32 / sr;
        let db = synth::dbfs(rms(&hp[w * wn..w * wn + wn]));
        let guard = t < TRANSIENT_EDGE_GUARD_SECS || t > total_secs - TRANSIENT_EDGE_GUARD_SECS;
        if let Some(p) = prev {
            if !guard && db - p >= jump_db && db >= overall_hp_db {
                onsets += 1;
            }
        }
        prev = Some(db);
    }
    onsets
}

#[test]
fn songs_transients_are_soft() {
    for s in songs() {
        let n8 = count_transient_onsets(&s.audio, TRANSIENT_JUMP_DB);
        let n14 = count_transient_onsets(&s.audio, 14.0);
        let notes = s.spec.melody.len();
        println!(
            "[{}] transient onsets +8dB={n8} +14dB={n14} (melody notes {notes})",
            s.name
        );
        assert_eq!(n14, 0, "{}: 14dB を超える急峻な立ち上がりがある", s.name);
        assert!(
            n8 as f32 <= 0.35 * s.audio.seconds(),
            "{}: 8dB 超のトランジェントが多すぎる ({n8})",
            s.name
        );
    }
}

// ============================================================
// リファレンス（実 BGM 5 曲）の実測レンジ内（docs/bgm-design-notes.md §5.2 / §6.2）
// 音色の「こもり」（重心・高域比の下限）と「刺さり」（高域比の上限）もここで守る。
// ============================================================

const REF_LOW_PCT: (f32, f32) = (2.0, 70.55);
const REF_MID_PCT: (f32, f32) = (29.38, 97.99);
const REF_HIGH_PCT: (f32, f32) = (0.01, 1.30);
const REF_CENTROID_HZ: (f32, f32) = (234.8, 805.8);
const REF_CENTROID_STD_HZ: (f32, f32) = (58.1, 173.3);
const REF_ONSET_PER_S: (f32, f32) = (0.949, 3.278);
const REF_DYN_STD_DB: (f32, f32) = (0.50, 3.07);
const REF_CREST_DB: (f32, f32) = (12.32, 16.07);
const REF_CORR: (f32, f32) = (0.147, 1.0);
const REF_DECAY_DB_S: (f32, f32) = (-17.94, 3.56);
const REF_AM_MED: (f32, f32) = (0.412, 0.595);
const REF_AM_P90: (f32, f32) = (0.412, 0.697);

fn in_range(name: &str, label: &str, v: f32, r: (f32, f32)) {
    println!("[{name}] {label} = {v:.3} (ref {:.3}..{:.3})", r.0, r.1);
    assert!(
        v >= r.0 && v <= r.1,
        "{name}: {label}={v:.3} がリファレンス実測レンジ {:.3}..{:.3} の外",
        r.0,
        r.1
    );
}

#[test]
fn songs_within_reference_ranges() {
    for s in songs() {
        let m = &s.metrics;
        let n = s.name;
        in_range(n, "low%", m.band_low * 100.0, REF_LOW_PCT);
        in_range(n, "mid%", m.band_mid * 100.0, REF_MID_PCT);
        in_range(n, "high%(刺さり/こもり)", m.band_high * 100.0, REF_HIGH_PCT);
        in_range(n, "centroid_Hz", m.centroid_mean_hz, REF_CENTROID_HZ);
        in_range(n, "centroid_std_Hz", m.centroid_std_hz, REF_CENTROID_STD_HZ);
        in_range(n, "onset/s", m.onset_density_per_sec, REF_ONSET_PER_S);
        in_range(n, "dyn_std_dB", m.dyn_rms_std_db, REF_DYN_STD_DB);
        in_range(n, "crest_dB", m.crest_db, REF_CREST_DB);
        in_range(n, "L/R corr", m.stereo_corr, REF_CORR);
        in_range(n, "AM_med", m.am_depth_median, REF_AM_MED);
        in_range(n, "AM_p90", m.am_depth_p90, REF_AM_P90);
        if let Some(d) = m.decay_slope_db_per_s {
            in_range(n, "decay_dB/s", d, REF_DECAY_DB_S);
        }
    }
}

/// 2〜5kHz（耳に刺さる帯域）の割合が、リファレンスの高域比上限（全体の 1.3%）以下。
/// 1 秒窓ごとに見て、一瞬だけ突出する窓（打鍵の硬い音）も禁止する。
#[test]
fn songs_have_no_harsh_presence_band() {
    for s in songs() {
        let mono = mono_of(&s.audio);
        let sr = s.audio.sr as f32;
        let n = 16384usize;
        let win = analysis::hann_window(n);
        let mut worst = 0f32;
        let mut pos = 0;
        while pos + n <= mono.len() {
            let mut buf: Vec<Complex> = (0..n)
                .map(|i| Complex {
                    re: mono[pos + i] * win[i],
                    im: 0.0,
                })
                .collect();
            analysis::fft(&mut buf);
            let (mut tot, mut harsh) = (0f64, 0f64);
            for (k, c) in buf.iter().take(n / 2).enumerate() {
                let f = k as f32 * sr / n as f32;
                let e = (c.re * c.re + c.im * c.im) as f64;
                if f >= 60.0 {
                    tot += e;
                }
                if (2000.0..5000.0).contains(&f) {
                    harsh += e;
                }
            }
            worst = worst.max((harsh / tot.max(1e-12)) as f32);
            pos += n;
        }
        println!("[{}] 2-5kHz 比の最悪窓 = {:.4}", s.name, worst);
        assert!(
            worst <= 0.035,
            "{}: 2〜5kHz が突出する窓あり ({worst:.4})",
            s.name
        );
    }
}

/// 曲間の音量・明るさ・厚みの不揃いを禁止する。
#[test]
fn songs_are_consistent_with_each_other() {
    let ss = songs();
    let rms_db: Vec<f32> = ss.iter().map(|s| s.metrics.rms_dbfs).collect();
    let cent: Vec<f32> = ss.iter().map(|s| s.metrics.centroid_mean_hz).collect();
    let spread = |v: &[f32]| {
        v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min)
    };
    println!("RMS dBFS = {rms_db:?} / centroid = {cent:?}");
    assert!(spread(&rms_db) <= 1.0, "曲間の RMS 差が 1dB 超: {rms_db:?}");
    let ratio = cent.iter().cloned().fold(f32::MIN, f32::max)
        / cent.iter().cloned().fold(f32::MAX, f32::min);
    assert!(ratio <= 1.5, "曲間のスペクトル重心の比が 1.5 超: {cent:?}");
}

// ============================================================
// 譜面（SongSpec）の機械検査
// ============================================================

fn seg_pcs(spec: &SongSpec, ci: usize) -> Vec<i32> {
    let s = &spec.segs[ci];
    s.iv.iter().map(|x| (s.root + x).rem_euclid(12)).collect()
}

fn seg_index_at(spec: &SongSpec, beat: f32) -> usize {
    for ci in 0..spec.segs.len() {
        let (s, e) = spec.seg_beats(ci);
        if beat >= s && beat < e {
            return ci;
        }
    }
    spec.segs.len() - 1
}

fn intervals(spec: &SongSpec) -> Vec<i32> {
    spec.melody.windows(2).map(|w| w[1].1 - w[0].1).collect()
}

/// 旋律は中音域（C4〜E5）で、順次/3 度進行が中心、跳躍は 5 度まで。
#[test]
fn melody_is_singable() {
    for s in songs() {
        let sp = &s.spec;
        let (lo, hi) = (
            sp.melody.iter().map(|n| n.1).min().unwrap(),
            sp.melody.iter().map(|n| n.1).max().unwrap(),
        );
        assert!(
            lo >= 0 && hi <= 16,
            "{}: 旋律の音域が C4〜E5 の外 ({lo}..{hi})",
            s.name
        );
        let iv = intervals(sp);
        let n = iv.len() as f32;
        let steps = iv.iter().filter(|d| d.abs() <= 2).count() as f32 / n;
        let big = iv.iter().filter(|d| d.abs() >= 5).count() as f32 / n;
        let max_leap = iv.iter().map(|d| d.abs()).max().unwrap();
        // 同音の連打は機械的に聞こえるので 2 回まで。
        let repeats = iv.iter().filter(|d| **d == 0).count();
        println!(
            "[{}] notes={} 順次={:.0}% 5度以上={:.0}% 最大跳躍={max_leap} 同音反復={repeats} 音域={lo}..{hi}",
            s.name,
            sp.melody.len(),
            steps * 100.0,
            big * 100.0
        );
        assert!(max_leap <= 7, "{}: 5 度を超える跳躍 ({max_leap})", s.name);
        let within_third = iv.iter().filter(|d| d.abs() <= 4).count() as f32 / n;
        assert!(
            steps >= 0.40,
            "{}: 順次進行（2 半音以内）が 40% 未満 ({steps:.2})",
            s.name
        );
        assert!(
            within_third >= 0.85,
            "{}: 3 度以内の進行が 85% 未満 ({within_third:.2})",
            s.name
        );
        assert!(
            big <= 0.15,
            "{}: 4 度以上の跳躍が多すぎる ({big:.2})",
            s.name
        );
        assert!(repeats <= 2, "{}: 同音反復が多い ({repeats})", s.name);
        // ループの継ぎ目: 最後の音 → 最初の音も跳躍しない。
        let (first, last) = (sp.melody[0].1, sp.melody.last().unwrap().1);
        assert!(
            (first - last).abs() <= 5,
            "{}: ループ継ぎ目の旋律が跳ぶ",
            s.name
        );
    }
}

/// 強拍（各小節の 1・3 拍目）は和声音。経過音は 1 拍以内・調内音のみで、全体の 30% 以下。
#[test]
fn melody_follows_harmony() {
    for s in songs() {
        let sp = &s.spec;
        let (mut strong, mut strong_ct, mut nct) = (0, 0, 0);
        for &(b, m, len, _) in &sp.melody {
            let pc = m.rem_euclid(12);
            let ci = seg_index_at(sp, b);
            let is_ct = seg_pcs(sp, ci).contains(&pc);
            if b.fract() == 0.0 && (b as i32 % 4 == 0 || b as i32 % 4 == 2) {
                strong += 1;
                if is_ct {
                    strong_ct += 1;
                }
            }
            if !is_ct {
                nct += 1;
                assert!(len <= 1.0, "{}: 非和声音が 1 拍超 (beat {b})", s.name);
                assert!(sp.key_pcs.contains(&pc), "{}: 調外の音 (beat {b})", s.name);
            }
        }
        let rate = strong_ct as f32 / strong as f32;
        let nct_rate = nct as f32 / sp.melody.len() as f32;
        println!(
            "[{}] 強拍の和声音率={rate:.2} 非和声音率={nct_rate:.2}",
            s.name
        );
        assert!(
            rate >= 0.999,
            "{}: 強拍に非和声音 ({strong_ct}/{strong})",
            s.name
        );
        assert!(
            nct_rate <= 0.30,
            "{}: 非和声音（経過音）が多い ({nct_rate:.2})",
            s.name
        );
    }
}

/// 同時に鳴る音（旋律・ストリングスのボイシング・ベース・分散和音）の間に、
/// 短 2 度・短 9 度・三全音を作らない。
#[test]
fn no_clashing_intervals() {
    for s in songs() {
        let sp = &s.spec;
        let vs = demo::voicings(sp);
        for ci in 0..sp.segs.len() {
            let mut tones: Vec<(i32, &str)> = vs[ci].iter().map(|&v| (v, "pad")).collect();
            tones.push((demo::bass_semis(sp.segs[ci].root), "bass"));
            tones.extend(demo::arp_tones(sp, &vs, ci).into_iter().map(|t| (t, "arp")));
            for (i, &(a, na)) in tones.iter().enumerate() {
                for &(b, nb) in &tones[i + 1..] {
                    assert!(
                        !demo::clash(a, b),
                        "{}: seg{ci} {na}({a}) と {nb}({b}) が衝突",
                        s.name
                    );
                }
            }
            let (st, en) = sp.seg_beats(ci);
            for &(b, m, len, _) in &sp.melody {
                if b + len > st + 0.25 && b < en - 0.25 {
                    for &(t, nt) in &tones {
                        assert!(
                            !demo::clash(m, t),
                            "{}: seg{ci} beat{b} 旋律({m}) と {nt}({t}) が衝突",
                            s.name
                        );
                    }
                }
            }
        }
    }
}

/// 低域（C3 以下）は隣接音を 5 半音以上離す（3 度以下の密集を禁止）。
/// パッドの声部連結は滑らか（隣接コードで総移動量 18 半音以内、ループの継ぎ目も含む）。
#[test]
fn low_register_and_voice_leading() {
    for s in songs() {
        let sp = &s.spec;
        let vs = demo::voicings(sp);
        for ci in 0..sp.segs.len() {
            let mut all: Vec<i32> = vs[ci].clone();
            all.push(demo::bass_semis(sp.segs[ci].root));
            all.extend(demo::arp_tones(sp, &vs, ci));
            all.sort_unstable();
            all.dedup();
            for w in all.windows(2) {
                if w[1] <= -12 {
                    assert!(w[1] - w[0] >= 5, "{}: seg{ci} 低域が密集 {:?}", s.name, all);
                }
            }
            assert!(
                vs[ci].iter().all(|&v| (-12..=19).contains(&v)),
                "{}: seg{ci} パッドが C3〜G5 外 {:?}",
                s.name,
                vs[ci]
            );
            let nxt = &vs[(ci + 1) % vs.len()];
            let mv: i32 = vs[ci]
                .iter()
                .zip(nxt.iter())
                .map(|(a, b)| (a - b).abs())
                .sum();
            // 声部が昇順ソートされているので、同じ順位の声部同士の移動量で近似する。
            assert!(
                mv <= 18,
                "{}: seg{ci}→次のコードで声部が大きく動く ({mv})",
                s.name
            );
        }
    }
}

/// ストリングスのデチューン幅は濁らない範囲（片側 5〜8 セント）。
#[test]
fn strings_detune_is_gentle() {
    let c = demo::STRINGS_DETUNE_CENTS;
    assert!((5.0..=8.0).contains(&c), "デチューン幅 {c} セント");
}

fn bar_onsets(sp: &SongSpec, bar0: usize, nbars: usize) -> Vec<(i32, i32)> {
    let (a, b) = (bar0 as f32 * 4.0, (bar0 + nbars) as f32 * 4.0);
    sp.melody
        .iter()
        .filter(|n| n.0 >= a && n.0 < b)
        .map(|n| (((n.0 - a) * 4.0).round() as i32, n.1))
        .collect()
}

fn similarity(a: &[(i32, i32)], b: &[(i32, i32)]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let common = a.iter().filter(|x| b.contains(x)).count();
    common as f32 / a.len().max(b.len()) as f32
}

/// 同じ小節・2 小節・4 小節の旋律を繰り返さない（リズム+音高の自己類似が 0.5 以下）。
/// 構成は 3 部以上で、伴奏型が部ごとに変わる。
#[test]
fn melody_does_not_repeat() {
    for s in songs() {
        let sp = &s.spec;
        let bars = sp.total_bars();
        let mut worst = 0f32;
        for w in [1usize, 2, 4] {
            for i in 0..=bars.saturating_sub(w) {
                for j in (i + w)..=bars.saturating_sub(w) {
                    let (a, b) = (bar_onsets(sp, i, w), bar_onsets(sp, j, w));
                    if a.len() >= 2 && b.len() >= 2 {
                        let sim = similarity(&a, &b);
                        worst = worst.max(sim);
                        assert!(
                            sim <= 0.5,
                            "{}: {w}小節の旋律が反復 (小節{i} と {j}、類似 {sim:.2})",
                            s.name
                        );
                    }
                }
            }
        }
        println!("[{}] 旋律の最大自己類似 = {worst:.2}", s.name);
        assert!(
            bars >= 12 && bars % sp.phrase_bars == 0 || bars == 10,
            "{}: 構成が短い",
            s.name
        );
        let kinds: std::collections::BTreeSet<_> =
            sp.comp.iter().map(|c| format!("{:?}", c.1)).collect();
        assert!(sp.comp.len() >= 3, "{}: 3 部以上の構成でない", s.name);
        assert!(kinds.len() >= 2, "{}: 伴奏型が部ごとに変わらない", s.name);
        assert!(
            sp.comp.iter().any(|c| c.1 != Comp::Sustain),
            "{}: ピアノの分散和音が一度も無い",
            s.name
        );
    }
}

/// フレーズごとに息継ぎ（0.4 拍以上の休符）があり、最高音は 1 フレーズに多くて 2 回。
#[test]
fn melody_phrases_breathe_and_peak_once() {
    for s in songs() {
        let sp = &s.spec;
        let pb = (sp.phrase_bars * 4) as f32;
        let n_phr = (sp.total_bars() as f32 * 4.0 / pb).ceil() as usize;
        for k in 0..n_phr {
            let (a, b) = (k as f32 * pb, (k + 1) as f32 * pb);
            let notes: Vec<_> = sp.melody.iter().filter(|n| n.0 >= a && n.0 < b).collect();
            let mut gap = notes.first().map(|n| n.0 - a).unwrap_or(0.0);
            for w in sp.melody.windows(2) {
                if w[1].0 >= a && w[1].0 < b + 1.0 {
                    gap = gap.max(w[1].0 - (w[0].0 + w[0].2));
                }
            }
            assert!(
                gap >= 0.4,
                "{}: フレーズ{k} に息継ぎが無い (最大の休符 {gap})",
                s.name
            );
            let top = notes.iter().map(|n| n.1).max().unwrap();
            let cnt = notes.iter().filter(|n| n.1 == top).count();
            assert!(cnt <= 2, "{}: フレーズ{k} の最高音が {cnt} 回", s.name);
        }
    }
}

// ============================================================
// 和音交代直後のクロマの濁り（前の和音が残って新しい和音に被らない）
// ============================================================

fn chroma_of(mono: &[f32], sr: f32, t0: f32, t1: f32) -> [f64; 12] {
    let n = 32768usize;
    let a = (t0 * sr) as usize;
    let b = ((t1 * sr) as usize).min(mono.len());
    let len = (b - a).min(n);
    let win = analysis::hann_window(len);
    let mut buf: Vec<Complex> = (0..n)
        .map(|i| Complex {
            re: if i < len { mono[a + i] * win[i] } else { 0.0 },
            im: 0.0,
        })
        .collect();
    analysis::fft(&mut buf);
    let mut ch = [0f64; 12];
    for (k, c) in buf.iter().take(n / 2).enumerate() {
        let f = k as f32 * sr / n as f32;
        if !(70.0..1400.0).contains(&f) {
            continue;
        }
        let semis = 12.0 * (f / synth::C4).log2();
        let pc = (semis.round() as i32).rem_euclid(12) as usize;
        ch[pc] += (c.re * c.re + c.im * c.im) as f64;
    }
    ch
}

/// 和音交代の直後（0.3〜1.0 秒）に、前のコードにしか無い音（現コード・旋律に無い音）の
/// エネルギー割合が、交代の後半（1.2 秒以降）より `margin` 以上大きくならないこと。
/// 平均を返す（交代ごとの値を出力）。
fn carryover(song: &Song, tm: &TimeMap, mono: &[f32]) -> f32 {
    let sp = &song.spec;
    let sr = song.audio.sr as f32;
    let mut sum = 0f32;
    let mut cnt = 0;
    for ci in 1..sp.segs.len() {
        let (sb, eb) = sp.seg_beats(ci);
        let tc = tm.at(sb);
        let t_end = tm.at(eb);
        if t_end - tc < 2.0 {
            continue;
        }
        let cur = seg_pcs(sp, ci);
        let prev: Vec<i32> = seg_pcs(sp, ci - 1)
            .into_iter()
            .filter(|p| !cur.contains(p))
            .collect();
        let mel: Vec<i32> = sp
            .melody
            .iter()
            .filter(|n| n.0 + n.2 > sb && n.0 < eb)
            .map(|n| n.1.rem_euclid(12))
            .collect();
        let prev: Vec<i32> = prev.into_iter().filter(|p| !mel.contains(p)).collect();
        if prev.is_empty() {
            continue;
        }
        let frac = |t0: f32, t1: f32| {
            let ch = chroma_of(mono, sr, t0, t1);
            let tot: f64 = ch.iter().sum::<f64>().max(1e-12);
            prev.iter().map(|&p| ch[p as usize]).sum::<f64>() / tot
        };
        let early = frac(tc + 0.3, tc + 1.0);
        let late = frac(
            (tc + 1.2).min(t_end - 0.8),
            (tc + 1.2).min(t_end - 0.8) + 0.7,
        );
        sum += (early - late) as f32;
        cnt += 1;
    }
    sum / cnt.max(1) as f32
}

const CARRYOVER_MAX: f32 = 0.02;

#[test]
fn chord_changes_do_not_smear() {
    let mut vals = Vec::new();
    for s in songs() {
        let tm = TimeMap::new(&s.spec);
        let v = carryover(s, &tm, &mono_of(&s.audio));
        println!(
            "[{}] 和音交代直後の前コード残り (early-late) = {v:.4}",
            s.name
        );
        vals.push((s.name, v));
        assert!(
            v <= CARRYOVER_MAX,
            "{}: 和音交代直後に前のコードが残って濁る ({v:.4})",
            s.name
        );
    }

    // 検査の感度確認: 残響を過剰にした同じ曲では、この値が閾値を超えること。
    let mut muddy = demo::title_spec();
    muddy.reverb = (0.95, 0.10, 0.9);
    let audio = demo::render_song(&muddy);
    let song = Song {
        name: "title(muddy)",
        metrics: analysis::analyze_track("m", &audio),
        spec: muddy,
        audio,
    };
    let tm = TimeMap::new(&song.spec);
    let v = carryover(&song, &tm, &mono_of(&song.audio));
    println!("[title(残響過多)] early-late = {v:.4}");
    let normal = vals.iter().find(|x| x.0 == "title").unwrap().1;
    assert!(
        v > CARRYOVER_MAX && v > normal,
        "残響過多の曲で閾値を超えない（検査が効いていない） {v} vs 閾値 {CARRYOVER_MAX}"
    );
}

// ============================================================
// SE（click と同等の簡易検証 + 先頭/末尾の滑らかさ + 長さ + 決定論性）
// ============================================================

fn assert_se_edges_smooth(name: &str, s: &[f32]) {
    let first = s[0].abs();
    let last = s[s.len() - 1].abs();
    assert!(first < 1e-3, "{name}: 先頭サンプルが非ゼロ ({first})");
    assert!(last < 1e-3, "{name}: 末尾サンプルが非ゼロ ({last})");
    let head = s[..32]
        .windows(2)
        .map(|p| (p[1] - p[0]).abs())
        .fold(0f32, f32::max);
    let tail = s[s.len() - 32..]
        .windows(2)
        .map(|p| (p[1] - p[0]).abs())
        .fold(0f32, f32::max);
    assert!(head < 0.02, "{name}: 先頭にクリック ({head})");
    assert!(tail < 0.02, "{name}: 末尾にクリック ({tail})");
}

fn check_se(name: &str, s: &[f32], min_secs: f32, max_secs: f32) {
    assert!(!s.is_empty(), "{name}: SE が空");
    let pk = peak(s);
    let secs = s.len() as f32 / synth::SR as f32;
    let dc = dc_offset(s);
    println!(
        "[{name}] {secs:.3}s peak={pk:.4} rms={:.4} dc={dc:.6}",
        rms(s)
    );
    assert!(pk <= 0.95, "{name}: ピークがクリップ域 ({pk})");
    assert!(pk > 0.05, "{name}: 無音に近い ({pk})");
    assert!(
        (min_secs..=max_secs).contains(&secs),
        "{name}: 長さが仕様外 ({secs}s)"
    );
    assert!(dc.abs() < 1e-3, "{name}: DC オフセットあり ({dc})");
    assert_se_edges_smooth(name, s);
}

/// 発音のある区間（50ms 窓 RMS が最大窓 RMS の -20dB 以内）の RMS [dBFS]。
fn active_rms_db(s: &[f32]) -> f32 {
    let w = (0.05 * synth::SR as f32) as usize;
    let wins: Vec<f32> = s.chunks(w).map(rms).collect();
    let mx = wins.iter().cloned().fold(0f32, f32::max);
    let act: Vec<f32> = wins.into_iter().filter(|&r| r >= mx * 0.1).collect();
    synth::dbfs((act.iter().map(|r| r * r).sum::<f32>() / act.len() as f32).sqrt())
}

#[test]
fn chime_se_quality() {
    let s = synth::demo::render_chime();
    check_se("chime", &s, 2.5, 4.0);
    assert_eq!(s, synth::demo::render_chime(), "chime が非決定論的");
}

#[test]
fn sparkle_se_quality() {
    let s = synth::demo::render_sparkle();
    check_se("sparkle", &s, 0.6, 1.2);
    assert_eq!(s, synth::demo::render_sparkle(), "sparkle が非決定論的");
}

#[test]
fn heartbeat_se_quality() {
    let s = synth::demo::render_heartbeat();
    check_se("heartbeat", &s, 1.2, 2.0);
    assert_eq!(s, synth::demo::render_heartbeat(), "heartbeat が非決定論的");
    let rc = 1.0 / (2.0 * std::f32::consts::PI * 200.0);
    let a = (1.0 / synth::SR as f32) / (rc + 1.0 / synth::SR as f32);
    let mut z = 0.0f32;
    let low: Vec<f32> = s
        .iter()
        .map(|&x| {
            z += a * (x - z);
            z
        })
        .collect();
    let ratio = (rms(&low) / rms(&s)).powi(2);
    println!("[heartbeat] low-band(<200Hz) energy ratio={ratio:.3}");
    assert!(ratio > 0.8, "heartbeat が低域主体でない ({ratio})");
    // 柔らかい立ち上がり: 最初の打音のピークまでが 10ms 以上かかる。
    let first_peak = s
        .iter()
        .take((0.2 * synth::SR as f32) as usize)
        .enumerate()
        .fold(
            (0usize, 0f32),
            |m, (i, &v)| if v.abs() > m.1 { (i, v.abs()) } else { m },
        );
    let t_peak = first_peak.0 as f32 / synth::SR as f32 - 0.06;
    assert!(
        t_peak >= 0.010,
        "heartbeat の立ち上がりが速すぎる ({t_peak}s)"
    );
}

/// 3 本の SE の音量は click.wav と釣り合う（発音区間の RMS 差が 6dB 以内）。
#[test]
fn se_loudness_matches_click() {
    let click = active_rms_db(&synth::render_click());
    for (name, s) in [
        ("chime", synth::demo::render_chime()),
        ("sparkle", synth::demo::render_sparkle()),
    ] {
        let d = active_rms_db(&s);
        println!("[{name}] active RMS {d:.1} dBFS (click {click:.1})");
        assert!(
            (d - click).abs() <= 6.0,
            "{name}: click と音量が違いすぎる ({d:.1} vs {click:.1})"
        );
    }
    // 鼓動は 60〜90Hz が主体で RMS が実効的な大きさを表さない（同じピークでも RMS が
    // 大きく出る）ため、ピークで揃える（click のピーク ±6dB）。
    let (pk_hb, pk_click) = (
        peak(&synth::demo::render_heartbeat()),
        peak(&synth::render_click()),
    );
    let d = 20.0 * (pk_hb / pk_click).log10();
    println!("[heartbeat] peak {pk_hb:.2} vs click {pk_click:.2} ({d:+.1} dB)");
    assert!(
        d.abs() <= 6.0,
        "heartbeat のピークが click と違いすぎる ({d:+.1} dB)"
    );
}

/// スペクトルのピーク（最大の -20dB 以内の局所最大）の周波数を返す（窓付き FFT、放物線補間）。
fn spectral_peaks(x: &[f32], sr: f32, min_hz: f32, max_hz: f32) -> Vec<(f32, f32)> {
    let n = 65536usize;
    let len = x.len().min(n);
    let win = analysis::hann_window(len);
    let mut buf: Vec<Complex> = (0..n)
        .map(|i| Complex {
            re: if i < len { x[i] * win[i] } else { 0.0 },
            im: 0.0,
        })
        .collect();
    analysis::fft(&mut buf);
    let mag: Vec<f32> = buf
        .iter()
        .take(n / 2)
        .map(|c| (c.re * c.re + c.im * c.im).sqrt())
        .collect();
    let (k0, k1) = (
        (min_hz * n as f32 / sr) as usize,
        (max_hz * n as f32 / sr) as usize,
    );
    let mx = mag[k0..k1].iter().cloned().fold(0f32, f32::max);
    let mut out = Vec::new();
    for k in k0.max(1)..k1 {
        if mag[k] > mag[k - 1] && mag[k] >= mag[k + 1] && mag[k] >= mx * 0.1 {
            let (a, b, c) = (mag[k - 1].ln(), mag[k].ln(), mag[k + 1].ln());
            let d = 0.5 * (a - c) / (a - 2.0 * b + c);
            out.push(((k as f32 + d) * sr / n as f32, mag[k] / mx));
        }
    }
    out
}

/// チャイムの各音: 目立つ部分音がすべて基音の整数倍（±1.5%）で、鐘の非整数倍音
/// （1.19 倍・2.74 倍など）が無い。4 音は長調（ファ・ラ・ソ・ド）の音程関係。
#[test]
fn chime_partials_are_harmonic_and_major() {
    let sr = synth::SR as f32;
    for &f0 in synth::demo::CHIME_FREQS.iter() {
        let x = synth::demo::render_chime_note(f0);
        let seg = &x[(0.02 * sr) as usize..(0.9 * sr) as usize];
        let peaks = spectral_peaks(seg, sr, 100.0, 9000.0);
        println!(
            "[chime {f0:.1}Hz] peaks = {:?}",
            peaks
                .iter()
                .map(|p| (p.0.round(), (p.1 * 100.0).round()))
                .collect::<Vec<_>>()
        );
        assert!(peaks.len() >= 2, "{f0}: 部分音が少ない");
        // 基音が最も強い。
        let strongest = peaks
            .iter()
            .cloned()
            .fold((0.0, 0.0), |m, p| if p.1 > m.1 { p } else { m });
        assert!(
            (strongest.0 / f0 - 1.0).abs() < 0.015,
            "{f0}: 最強の部分音が基音でない ({})",
            strongest.0
        );
        for &(f, _) in &peaks {
            let k = (f / f0).round();
            assert!(k >= 1.0, "{f0}: 基音より低い部分音 {f}");
            assert!(
                (f / (k * f0) - 1.0).abs() < 0.015,
                "{f0}: 部分音 {f}Hz が基音の整数倍でない (比 {:.3})",
                f / f0
            );
        }
    }
    let f = synth::demo::CHIME_FREQS;
    let semis: Vec<f32> = f.iter().map(|x| 12.0 * (x / f[0]).log2()).collect();
    for (got, want) in semis.iter().zip([0.0f32, 4.0, 2.0, -5.0]) {
        assert!((got - want).abs() < 0.05, "チャイムの音程関係 {semis:?}");
    }
    // 実際のチャイムでも、各音の発音直後の最強成分は意図した音。
    let c = synth::demo::render_chime();
    for (t, f0) in [0.0f32, 0.62, 1.24, 1.86].iter().zip(f.iter()) {
        let a = ((t + 0.03) * sr) as usize;
        let peaks = spectral_peaks(&c[a..a + (0.3 * sr) as usize], sr, 100.0, 3000.0);
        let st = peaks
            .iter()
            .cloned()
            .fold((0.0, 0.0), |m, p| if p.1 > m.1 { p } else { m });
        assert!(
            (st.0 / f0 - 1.0).abs() < 0.02,
            "t={t}: 最強成分 {} が {f0} でない",
            st.0
        );
    }
}

/// きらめきは控えめ: ピーク 0.6 以下、3kHz 超のエネルギーが全体の 3% 以下。
#[test]
fn sparkle_is_gentle() {
    let s = synth::demo::render_sparkle();
    let pk = peak(&s);
    assert!(pk <= 0.6, "sparkle のピークが大きい ({pk})");
    let sr = synth::SR as f32;
    let n = 65536usize;
    let win = analysis::hann_window(s.len());
    let mut buf: Vec<Complex> = (0..n)
        .map(|i| Complex {
            re: if i < s.len() { s[i] * win[i] } else { 0.0 },
            im: 0.0,
        })
        .collect();
    analysis::fft(&mut buf);
    let (mut tot, mut hi) = (0f64, 0f64);
    for (k, c) in buf.iter().take(n / 2).enumerate() {
        let f = k as f32 * sr / n as f32;
        let e = (c.re * c.re + c.im * c.im) as f64;
        tot += e;
        if f > 3000.0 {
            hi += e;
        }
    }
    let r = hi / tot;
    println!("[sparkle] >3kHz energy ratio = {r:.4}");
    assert!(r <= 0.03, "sparkle の高域が強い ({r})");
}
