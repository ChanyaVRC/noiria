//! デモゲーム「星待ちの放課後」用の BGM（5 曲）と SE（3 本）。
//!
//! BGM は採用済み BGM（`games/sample` 用の 2op FM + デチューン saw パッド）の
//! 音色系統を使わず、`inst` の加算合成ピアノ（旋律・分散和音・ベース）と
//! ストリングス・アンサンブル（和音）で作る。曲はスコア（`SongSpec`）として
//! 持ち、テスト（`tests/demo_audio_quality.rs`）が旋律の跳躍・強拍の和声音率・
//! 衝突音程・低域の間隔・反復を譜面データから機械検査する。

mod inst;

use super::*;
pub use inst::STRINGS_DETUNE_CENTS;
use inst::{add_strings_run, piano_note};

// ============================================================
// 曲の定義（スコア）
// ============================================================

/// 1 つのコード区間。`root` は C4 基準の半音、`iv` はルートからの音程列。
pub struct ChordSeg {
    pub root: i32,
    pub iv: &'static [i32],
    pub bars: usize,
}

const MAJ: &[i32] = &[0, 4, 7];
const MAJ7: &[i32] = &[0, 4, 7, 11];
const MIN7: &[i32] = &[0, 3, 7, 10];

/// 1 つのメロディ音: (開始拍, C4 基準半音, 長さ拍, 音量係数)。
pub type Note = (f32, i32, f32, f32);

/// 伴奏型（ストリングスとベースは常に鳴る。これはピアノの分散和音の有無/速さ）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Comp {
    Sustain,
    Arp4,
    Arp8,
}

/// 1 曲ぶんの楽譜と音作りパラメータ。
pub struct SongSpec {
    pub bpm: f32,
    pub segs: Vec<ChordSeg>,
    pub melody: Vec<Note>,
    /// 調の音階の構成音（ピッチクラス）。経過音がこれ以外に出ないことを検査する。
    pub key_pcs: &'static [i32],
    /// フレーズ長（小節）。フレーズ末で少し減速し、フレーズ内で強弱の山を作る。
    pub phrase_bars: usize,
    /// (開始小節, 伴奏型)。開始小節の昇順。
    pub comp: Vec<(usize, Comp)>,
    pub strings_level: f32,
    pub bass_level: f32,
    pub arp_level: f32,
    pub lead_level: f32,
    pub anchor: i32,
    /// (feedback, damp, wet)
    pub reverb: (f32, f32, f32),
    pub target_rms_dbfs: f32,
    pub seed: u64,
}

impl SongSpec {
    pub fn total_bars(&self) -> usize {
        self.segs.iter().map(|s| s.bars).sum()
    }
    /// コード区間 `ci` の拍範囲 [開始, 終了)。
    pub fn seg_beats(&self, ci: usize) -> (f32, f32) {
        let s: usize = self.segs[..ci].iter().map(|x| x.bars).sum();
        (s as f32 * 4.0, (s + self.segs[ci].bars) as f32 * 4.0)
    }
    pub fn comp_at_bar(&self, bar: usize) -> Comp {
        let mut c = Comp::Sustain;
        for &(b, k) in &self.comp {
            if b <= bar {
                c = k;
            }
        }
        c
    }
}

fn seg(root: i32, iv: &'static [i32], bars: usize) -> ChordSeg {
    ChordSeg { root, iv, bars }
}

/// サブベースの C4 基準半音（ルートのピッチクラスを C2〜B2 へ畳む）。
pub fn bass_semis(root: i32) -> i32 {
    root.rem_euclid(12) - 24
}

/// 同時に鳴らしてはいけない 2 音（短 2 度・短 9 度・三全音。オクターブ違いも含む）。
pub fn clash(a: i32, b: i32) -> bool {
    let i = (a - b).abs() % 12;
    i == 1 || i == 6
}

// ============================================================
// 時間軸（フレーズ末の減速）
// ============================================================

/// 拍→秒の写像。各フレーズの最後の 2 拍で拍の長さが最大 +7% まで伸びる
/// （人が弾くときのフレーズ末のわずかなリタルダンド）。
pub struct TimeMap {
    cum: Vec<f64>,
}

const TM_RES: usize = 32;

impl TimeMap {
    pub fn new(spec: &SongSpec) -> Self {
        let total_beats = spec.total_bars() * 4;
        let phrase_beats = (spec.phrase_bars * 4) as f32;
        let base = 60.0 / spec.bpm as f64;
        let mut cum = Vec::with_capacity(total_beats * TM_RES + 1);
        cum.push(0.0);
        for i in 0..total_beats * TM_RES {
            let b = (i as f32 + 0.5) / TM_RES as f32;
            let pos = b % phrase_beats;
            let x = ((pos - (phrase_beats - 2.0)) / 2.0).clamp(0.0, 1.0);
            let w = x * x * (3.0 - 2.0 * x);
            let dt = base * (1.0 + 0.07 * w as f64) / TM_RES as f64;
            cum.push(cum[i] + dt);
        }
        Self { cum }
    }
    pub fn at(&self, beat: f32) -> f32 {
        let x = (beat.max(0.0) * TM_RES as f32) as f64;
        let i = (x.floor() as usize).min(self.cum.len() - 2);
        let f = x - i as f64;
        (self.cum[i] * (1.0 - f) + self.cum[i + 1] * f) as f32
    }
    pub fn total(&self) -> f32 {
        *self.cum.last().unwrap() as f32
    }
}

// ============================================================
// 声部連結（パッド）と分散和音の音選び
// ============================================================

fn seg_melody(spec: &SongSpec, ci: usize) -> Vec<i32> {
    let (s, e) = spec.seg_beats(ci);
    spec.melody
        .iter()
        .filter(|&&(b, _, len, _)| b + len > s + 0.25 && b < e - 0.25)
        .map(|&(_, m, _, _)| m)
        .collect()
}

/// 声部の濁りへの罰則（Low Interval Limit、docs/bgm-design-notes.md §1.5）。
/// ベース音を含め、C3 以下は隣接音を 5 半音以上離し、それより上でも E4 未満は
/// 3 半音以上、同音・短 2 度は避ける。衝突音程（`clash`）は旋律・ベースとの間も
/// 含めて強く罰する。パッド帯域（C3〜C5）から外れる音も避ける。
fn voicing_penalty(pad: &[i32], bass: i32, melody: &[i32]) -> i32 {
    let mut s = pad.to_vec();
    s.push(bass);
    s.sort_unstable();
    let mut pen = 0;
    for w in s.windows(2) {
        let gap = w[1] - w[0];
        if gap == 0 {
            pen += 20;
        }
        if gap < 5 && w[1] <= -12 {
            pen += 30;
        } else if gap < 3 && w[0] < 4 {
            pen += 10;
        }
        if gap == 1 {
            pen += 5;
        }
    }
    for (i, &a) in s.iter().enumerate() {
        for &b in &s[i + 1..] {
            if clash(a, b) {
                pen += 50;
            }
        }
        for &m in melody {
            if clash(a, m) {
                pen += 40;
            }
        }
    }
    for &n in pad {
        if n > 12 && n <= 19 {
            pen += 8;
        } else if !(-12..=19).contains(&n) {
            pen += 60;
        }
    }
    pen
}

/// 移動量が最小になる声部対応を総当たりで選ぶ声部連結（`synth::voice_lead` と
/// 同じ考え方。オクターブも ±1 探索し、`voicing_penalty` を同時に最小化する）。
fn voice_lead_iv(prev: &[i32], root: i32, iv: &[i32], melody: &[i32]) -> Vec<i32> {
    let n = prev.len();
    let mut targets: Vec<i32> = iv.iter().map(|x| root + x).collect();
    while targets.len() < n {
        targets.push(root + 12);
    }
    targets.truncate(n);
    let bass = bass_semis(root);
    let mut best_cost = i32::MAX;
    let mut best = vec![0i32; n];
    for perm in permutations(n) {
        let bases: Vec<i32> = (0..n)
            .map(|vi| nearest_octave(targets[perm[vi]], prev[vi]))
            .collect();
        for c in 0..3usize.pow(n as u32) {
            let mut k = c;
            let mut assign = vec![0i32; n];
            let mut cost = 0;
            for vi in 0..n {
                let off = (k % 3) as i32 * 12 - 12;
                k /= 3;
                assign[vi] = bases[vi] + off;
                cost += (assign[vi] - prev[vi]).abs();
            }
            cost += voicing_penalty(&assign, bass, melody);
            if cost < best_cost {
                best_cost = cost;
                best = assign;
            }
        }
    }
    best.sort_unstable();
    best
}

/// 各コード区間の 4 声ボイシング（昇順）。ループの継ぎ目（最終→先頭）も声部連結に
/// なるよう、一周ぶん先に回して最終コードの声部を作ってから先頭を決め直す。
pub fn voicings(spec: &SongSpec) -> Vec<Vec<i32>> {
    let a = spec.anchor;
    let mut prev: Vec<i32> = vec![a - 7, a - 2, a + 3, a + 7];
    let mut out = Vec::new();
    for _pass in 0..2 {
        out.clear();
        for (ci, s) in spec.segs.iter().enumerate() {
            let v = voice_lead_iv(&prev, s.root, s.iv, &seg_melody(spec, ci));
            prev = v.clone();
            out.push(v);
        }
    }
    out
}

/// コード区間 `ci` のピアノ分散和音に使う音（ボイシングの 1 オクターブ下から、
/// パッド・ベース・旋律と衝突する音と、C3 以下で 5 半音未満に詰まる音を除いたもの）。
pub fn arp_tones(spec: &SongSpec, vs: &[Vec<i32>], ci: usize) -> Vec<i32> {
    let bass = bass_semis(spec.segs[ci].root);
    let mel = seg_melody(spec, ci);
    let mut out: Vec<i32> = Vec::new();
    let mut cands: Vec<i32> = vs[ci].iter().map(|&v| v - 12).collect();
    cands.sort_unstable();
    for c in cands {
        if clash(c, bass)
            || vs[ci].iter().any(|&p| clash(c, p))
            || mel.iter().any(|&m| clash(c, m))
            || out.iter().any(|&o| clash(c, o))
        {
            continue;
        }
        let below = out.last().copied().unwrap_or(bass);
        if c <= -12 && c - below < 5 {
            continue;
        }
        if c - bass < 5 && c <= -12 {
            continue;
        }
        out.push(c);
    }
    if out.len() < 2 {
        out = vec![bass + 12 * ((-12 - bass).max(0) / 12 + 1), bass + 19];
        out.retain(|&c| !vs[ci].iter().any(|&p| clash(c, p)));
    }
    out
}

// ============================================================
// レンダリング
// ============================================================

fn pan_for(semi: i32, spread: f32) -> f32 {
    ((semi as f32 - 6.0) / 30.0).clamp(-1.0, 1.0) * spread
}

#[allow(clippy::too_many_arguments)]
fn add_piano(
    buf: &mut Stereo,
    sr: u32,
    t: f32,
    semi: i32,
    vel: f32,
    hold: f32,
    release: f32,
    level: f32,
    seed: u64,
) {
    let note = piano_note(sr, degree_freq(C4, semi), vel, hold, release, seed);
    add_mono_track(
        buf,
        (t.max(0.0) * sr as f32) as usize,
        &note,
        level,
        pan_for(semi, 0.5),
    );
}

const ARP4: [usize; 4] = [0, 1, 2, 1];
const ARP8: [usize; 8] = [0, 1, 2, 3, 2, 1, 2, 1];

fn build_song(buf: &mut Stereo, sr: u32, spec: &SongSpec, tm: &TimeMap) {
    let srf = sr as f32;
    let vs = voicings(spec);
    let mut human = Rng::new(spec.seed);
    let phrase_beats = (spec.phrase_bars * 4) as f32;

    // ストリングス: 共通音は区間をまたいで伸ばし、再発音しない。
    let n_seg = spec.segs.len();
    for ci in 0..n_seg {
        let (sb, _) = spec.seg_beats(ci);
        for (vi, &x) in vs[ci].iter().enumerate() {
            if ci > 0 && vs[ci - 1].contains(&x) {
                continue;
            }
            let mut cj = ci;
            while cj + 1 < n_seg && vs[cj + 1].contains(&x) {
                cj += 1;
            }
            let (_, eb) = spec.seg_beats(cj);
            let t0 = tm.at(sb);
            let hold = tm.at(eb) - t0;
            add_strings_run(
                buf,
                sr,
                (t0 * srf) as usize,
                degree_freq(C4, x),
                hold,
                0.7,
                0.8,
                spec.strings_level,
                spec.seed + 0x100 + (ci * 8 + vi) as u64,
            );
        }
    }

    // ベース（小節頭にピアノ低音）と分散和音。
    let mut bar_cursor = 0usize;
    for (ci, s) in spec.segs.iter().enumerate() {
        let arp = arp_tones(spec, &vs, ci);
        for b in 0..s.bars {
            let bar = bar_cursor + b;
            let t0 = tm.at(bar as f32 * 4.0) + human.next_f32() * 0.008;
            let t1 = tm.at(bar as f32 * 4.0 + 4.0);
            add_piano(
                buf,
                sr,
                t0,
                bass_semis(s.root),
                0.55 * (1.0 + human.next_f32() * 0.08),
                (t1 - t0) * 0.95,
                0.35,
                spec.bass_level,
                spec.seed + 0x200 + bar as u64,
            );
            let (slots, pat): (usize, &[usize]) = match spec.comp_at_bar(bar) {
                Comp::Sustain => (0, &ARP4),
                Comp::Arp4 => (4, &ARP4),
                Comp::Arp8 => (8, &ARP8),
            };
            for k in 0..slots {
                let bt = bar as f32 * 4.0 + k as f32 * (4.0 / slots as f32);
                let t = tm.at(bt) + human.next_f32() * 0.008;
                let accent = if k == 0 { 0.10 } else { 0.0 };
                let vel = (0.36 + accent) * (1.0 + human.next_f32() * 0.12);
                let tone = arp[pat[k % pat.len()] % arp.len()];
                add_piano(
                    buf,
                    sr,
                    t,
                    tone,
                    vel,
                    ((t1 - t) * 0.6).max(0.5),
                    0.30,
                    spec.arp_level,
                    spec.seed + 0x400 + (bar * 8 + k) as u64,
                );
            }
        }
        bar_cursor += s.bars;
    }

    // 旋律: 発音タイミング ±12ms・音量 ±10%・音長 ±6% の揺らぎ、フレーズ内の強弱の山。
    for (i, &(b, semi, len, lv)) in spec.melody.iter().enumerate() {
        let t = tm.at(b) + human.next_f32() * 0.012;
        let end = tm.at(b + len);
        let p = (b % phrase_beats) / phrase_beats;
        let arch = 0.66 + 0.48 * (PI * p).sin();
        let vel = (lv * arch * (1.0 + human.next_f32() * 0.10)).clamp(0.2, 1.0);
        let hold = (end - t) * 0.97 * (1.0 + human.next_f32() * 0.06);
        add_piano(
            buf,
            sr,
            t,
            semi,
            vel,
            hold,
            0.25,
            spec.lead_level,
            spec.seed + 0x300 + i as u64,
        );
    }
}

pub fn render_song(spec: &SongSpec) -> Stereo {
    let tm = TimeMap::new(spec);
    let (rf, rd, rw) = spec.reverb;
    let reverb = ReverbParams {
        feedback: rf,
        damp: rd,
        wet: rw,
    };
    render_looped(
        tm.total(),
        3.0,
        0.5,
        0.0,
        0.0,
        reverb,
        spec.target_rms_dbfs,
        |buf, sr| build_song(buf, sr, spec, &tm),
    )
}

// ============================================================
// 5 曲のスコア
//
// 共通の作法: 旋律は E4〜E5 前後の中音域、和声音中心の順次進行で、4 小節の
// 「問い」（開いた音で終わる）と「答え」（閉じる）の対を A / A' / B の 3 部以上に
// 並べ、同じ小節の繰り返しを作らない。強拍（各小節の 1・3 拍目）は和声音、
// 経過音は 1 拍以内の調内音のみ。フレーズの切れ目に息継ぎ（休符）を置く。
// 全曲ダイアトニックで、属和音は三全音を含む七の和音にせず三和音にする。
// ============================================================

/// title: 静かで期待感。G 長調 BPM 78、12 小節。
pub fn title_spec() -> SongSpec {
    SongSpec {
        bpm: 78.0,
        segs: vec![
            seg(7, MAJ7, 2), // Gmaj7
            seg(4, MIN7, 2), // Em7
            seg(0, MAJ7, 2), // Cmaj7
            seg(7, MAJ7, 2), // Gmaj7
            seg(9, MIN7, 2), // Am7
            seg(2, MAJ, 2),  // D
        ],
        melody: vec![
            (0.5, 11, 1.5, 0.80),
            (2.0, 14, 1.0, 0.85),
            (3.0, 11, 1.0, 0.75),
            (4.0, 7, 1.0, 0.75),
            (5.0, 9, 1.0, 0.70),
            (6.0, 11, 2.0, 0.80),
            (8.0, 14, 2.0, 0.85),
            (10.0, 11, 1.5, 0.75),
            (11.5, 9, 0.5, 0.70),
            (12.5, 7, 1.5, 0.75),
            (14.0, 11, 1.5, 0.75),
            (16.5, 7, 1.5, 0.85),
            (18.0, 4, 1.0, 0.75),
            (19.0, 7, 1.0, 0.70),
            (20.0, 11, 1.0, 0.80),
            (21.0, 14, 1.0, 0.80),
            (22.0, 16, 2.0, 0.90),
            (24.0, 14, 1.0, 0.85),
            (25.0, 11, 1.0, 0.75),
            (26.0, 7, 1.5, 0.80),
            (27.5, 9, 0.5, 0.65),
            (28.0, 11, 1.5, 0.80),
            (29.5, 9, 0.5, 0.65),
            (30.0, 7, 1.5, 0.75),
            (32.5, 12, 1.5, 0.85),
            (34.0, 9, 1.0, 0.75),
            (35.0, 7, 1.0, 0.70),
            (36.0, 12, 1.5, 0.80),
            (37.5, 14, 0.5, 0.80),
            (38.0, 16, 2.0, 0.90),
            (40.0, 14, 1.0, 0.85),
            (41.0, 11, 1.0, 0.70),
            (42.0, 9, 1.5, 0.80),
            (43.5, 11, 0.5, 0.65),
            (44.0, 9, 2.0, 0.80),
            (46.0, 6, 1.0, 0.70),
            (47.0, 7, 0.8, 0.70),
        ],
        key_pcs: &[7, 9, 11, 0, 2, 4, 6],
        phrase_bars: 4,
        comp: vec![(0, Comp::Sustain), (4, Comp::Arp4), (8, Comp::Arp8)],
        strings_level: 0.081,
        bass_level: 0.55,
        arp_level: 0.78,
        lead_level: 0.90,
        anchor: 4,
        reverb: (0.78, 0.35, 0.20),
        target_rms_dbfs: -18.5,
        seed: 0x7171_0001,
    }
}

/// daily: 朝の通学路、明るく軽い。D 長調 BPM 104、16 小節（1 小節 1 コード）。
pub fn daily_spec() -> SongSpec {
    SongSpec {
        bpm: 104.0,
        segs: vec![
            seg(2, MAJ, 1),   // D
            seg(-1, MIN7, 1), // Bm7
            seg(7, MAJ, 1),   // G
            seg(9, MAJ, 1),   // A
            seg(2, MAJ, 1),   // D
            seg(6, MIN7, 1),  // F#m7
            seg(7, MAJ, 1),   // G
            seg(9, MAJ, 1),   // A
            seg(7, MAJ, 1),   // G
            seg(9, MAJ, 1),   // A
            seg(-1, MIN7, 1), // Bm7
            seg(6, MIN7, 1),  // F#m7
            seg(7, MAJ, 1),   // G
            seg(9, MAJ, 1),   // A
            seg(4, MIN7, 1),  // Em7
            seg(9, MAJ, 1),   // A
        ],
        melody: vec![
            (0.0, 14, 1.0, 0.85),
            (1.0, 13, 0.5, 0.70),
            (1.5, 11, 0.5, 0.65),
            (2.0, 9, 1.5, 0.80),
            (3.5, 11, 0.5, 0.65),
            (4.0, 9, 1.5, 0.85),
            (5.5, 11, 0.5, 0.65),
            (6.0, 9, 1.0, 0.75),
            (7.0, 6, 1.0, 0.70),
            (8.0, 11, 1.0, 0.80),
            (9.0, 14, 1.0, 0.70),
            (10.0, 11, 1.0, 0.75),
            (11.0, 9, 1.0, 0.70),
            (12.0, 9, 1.5, 0.80),
            (13.5, 6, 0.5, 0.65),
            (14.0, 4, 1.5, 0.80),
            (16.0, 9, 1.0, 0.85),
            (17.0, 11, 0.5, 0.65),
            (17.5, 13, 0.5, 0.70),
            (18.0, 14, 1.5, 0.85),
            (19.5, 16, 0.5, 0.70),
            (20.0, 13, 1.0, 0.80),
            (21.0, 11, 0.5, 0.65),
            (21.5, 9, 0.5, 0.65),
            (22.0, 6, 1.0, 0.75),
            (23.0, 9, 1.0, 0.70),
            (24.0, 7, 1.0, 0.75),
            (25.0, 9, 0.5, 0.65),
            (25.5, 11, 0.5, 0.70),
            (26.0, 14, 1.5, 0.85),
            (27.5, 11, 0.5, 0.65),
            (28.0, 13, 1.5, 0.85),
            (29.5, 11, 0.5, 0.65),
            (30.0, 9, 1.5, 0.80),
            (32.0, 14, 1.5, 0.85),
            (33.5, 11, 0.5, 0.65),
            (34.0, 7, 1.0, 0.75),
            (35.0, 9, 1.0, 0.80),
            (36.0, 16, 2.0, 0.90),
            (38.0, 13, 1.0, 0.80),
            (39.0, 9, 1.0, 0.70),
            (40.0, 11, 1.0, 0.80),
            (41.0, 9, 1.0, 0.65),
            (42.0, 6, 1.5, 0.80),
            (43.5, 9, 0.5, 0.65),
            (44.0, 13, 1.0, 0.85),
            (45.0, 9, 1.0, 0.70),
            (46.0, 6, 1.0, 0.75),
            (47.0, 4, 1.0, 0.70),
            (48.0, 7, 1.0, 0.75),
            (49.0, 9, 1.0, 0.70),
            (50.0, 11, 1.0, 0.85),
            (51.0, 14, 1.0, 0.75),
            (52.0, 13, 2.0, 0.80),
            (54.0, 16, 1.0, 0.85),
            (55.0, 13, 1.0, 0.75),
            (56.0, 11, 1.0, 0.80),
            (57.0, 9, 0.5, 0.65),
            (57.5, 7, 0.5, 0.65),
            (58.0, 4, 1.0, 0.75),
            (59.5, 7, 0.5, 0.65),
            (60.0, 9, 1.5, 0.80),
            (61.5, 11, 0.5, 0.65),
            (62.0, 13, 1.5, 0.80),
        ],
        key_pcs: &[2, 4, 6, 7, 9, 11, 1],
        phrase_bars: 4,
        comp: vec![(0, Comp::Arp8), (8, Comp::Arp4), (12, Comp::Arp8)],
        strings_level: 0.072,
        bass_level: 0.50,
        arp_level: 0.72,
        lead_level: 0.96,
        anchor: 4,
        reverb: (0.74, 0.38, 0.18),
        target_rms_dbfs: -18.5,
        seed: 0x7171_0002,
    }
}

/// calm: 放課後の部室、穏やか。C 長調 BPM 76、12 小節。
pub fn calm_spec() -> SongSpec {
    SongSpec {
        bpm: 76.0,
        segs: vec![
            seg(0, MAJ7, 2), // Cmaj7
            seg(9, MIN7, 2), // Am7
            seg(2, MIN7, 2), // Dm7
            seg(7, MAJ, 2),  // G
            seg(4, MIN7, 2), // Em7
            seg(5, MAJ7, 2), // Fmaj7
        ],
        melody: vec![
            (0.5, 7, 1.5, 0.80),
            (2.0, 11, 1.0, 0.85),
            (3.0, 9, 1.0, 0.70),
            (4.0, 7, 1.5, 0.80),
            (5.5, 9, 0.5, 0.65),
            (6.0, 11, 2.0, 0.80),
            (8.0, 9, 1.0, 0.75),
            (9.0, 11, 1.0, 0.70),
            (10.0, 12, 1.5, 0.85),
            (11.5, 14, 0.5, 0.65),
            (12.0, 16, 2.0, 0.90),
            (14.0, 12, 1.5, 0.75),
            (16.5, 9, 1.5, 0.80),
            (18.0, 12, 1.0, 0.80),
            (19.0, 14, 1.0, 0.70),
            (20.0, 12, 1.5, 0.80),
            (21.5, 7, 0.5, 0.65),
            (22.0, 5, 2.0, 0.80),
            (24.0, 7, 1.0, 0.75),
            (25.0, 9, 1.0, 0.70),
            (26.0, 11, 1.0, 0.80),
            (27.0, 14, 1.0, 0.70),
            (28.0, 11, 1.5, 0.80),
            (29.5, 9, 0.5, 0.65),
            (30.0, 7, 1.5, 0.75),
            (32.5, 11, 1.5, 0.80),
            (34.0, 14, 1.0, 0.85),
            (35.0, 16, 1.0, 0.75),
            (36.0, 14, 1.0, 0.80),
            (37.0, 11, 1.0, 0.70),
            (38.0, 7, 2.0, 0.75),
            (40.0, 9, 1.0, 0.80),
            (41.0, 7, 1.0, 0.65),
            (42.0, 4, 1.5, 0.80),
            (43.5, 7, 0.5, 0.65),
            (44.0, 9, 1.5, 0.75),
            (46.0, 12, 1.5, 0.85),
        ],
        key_pcs: &[0, 2, 4, 5, 7, 9, 11],
        phrase_bars: 4,
        comp: vec![(0, Comp::Sustain), (4, Comp::Arp8), (8, Comp::Arp4)],
        strings_level: 0.086,
        bass_level: 0.50,
        arp_level: 0.78,
        lead_level: 0.96,
        anchor: 2,
        reverb: (0.80, 0.34, 0.22),
        target_rms_dbfs: -18.5,
        seed: 0x7171_0003,
    }
}

/// tension: 雨の木曜、すれ違いの不安。D 短調 BPM 84、12 小節。
pub fn tension_spec() -> SongSpec {
    SongSpec {
        bpm: 84.0,
        segs: vec![
            seg(2, MIN7, 2), // Dm7
            seg(10, MAJ, 2), // Bb
            seg(7, MIN7, 2), // Gm7
            seg(2, MIN7, 2), // Dm7
            seg(10, MAJ, 2), // Bb
            seg(9, MAJ, 2),  // A
        ],
        melody: vec![
            (0.5, 9, 2.5, 0.80),
            (3.0, 7, 1.0, 0.70),
            (4.0, 5, 2.0, 0.80),
            (6.0, 9, 2.0, 0.80),
            (8.0, 10, 2.0, 0.85),
            (10.0, 5, 1.5, 0.75),
            (11.5, 7, 0.5, 0.65),
            (12.0, 10, 1.0, 0.80),
            (13.0, 12, 1.0, 0.70),
            (14.0, 10, 1.5, 0.75),
            (16.5, 7, 1.5, 0.80),
            (18.0, 10, 1.5, 0.85),
            (19.5, 9, 0.5, 0.65),
            (20.0, 7, 2.0, 0.80),
            (22.0, 5, 1.5, 0.75),
            (24.0, 9, 1.5, 0.80),
            (25.5, 7, 0.5, 0.65),
            (26.0, 5, 2.0, 0.80),
            (28.0, 9, 1.0, 0.80),
            (29.0, 5, 1.0, 0.70),
            (30.0, 2, 1.5, 0.80),
            (32.5, 5, 1.0, 0.75),
            (33.5, 7, 0.5, 0.70),
            (34.0, 10, 1.5, 0.85),
            (35.5, 12, 0.5, 0.65),
            (36.0, 14, 2.0, 0.90),
            (38.0, 10, 1.5, 0.80),
            (40.0, 9, 2.0, 0.80),
            (42.0, 4, 1.5, 0.80),
            (44.0, 1, 2.0, 0.80),
            (46.0, 4, 1.5, 0.75),
        ],
        key_pcs: &[2, 4, 5, 7, 9, 10, 0, 1],
        phrase_bars: 4,
        comp: vec![(0, Comp::Arp4), (4, Comp::Arp4), (8, Comp::Arp8)],
        strings_level: 0.081,
        bass_level: 0.58,
        arp_level: 0.72,
        lead_level: 0.90,
        anchor: 2,
        reverb: (0.78, 0.36, 0.20),
        target_rms_dbfs: -18.5,
        seed: 0x7171_0004,
    }
}

/// night: 星空の告白、温かい。F 長調 BPM 66、10 小節。
pub fn night_spec() -> SongSpec {
    SongSpec {
        bpm: 66.0,
        segs: vec![
            seg(5, MAJ7, 2), // Fmaj7
            seg(2, MIN7, 2), // Dm7
            seg(10, MAJ, 2), // Bb
            seg(7, MIN7, 2), // Gm7
            seg(0, MAJ, 2),  // C
        ],
        melody: vec![
            (0.5, 9, 2.5, 0.85),
            (3.0, 12, 1.5, 0.85),
            (4.5, 9, 1.5, 0.75),
            (6.0, 5, 1.5, 0.75),
            (8.0, 9, 2.0, 0.80),
            (10.0, 12, 2.0, 0.80),
            (12.0, 14, 2.0, 0.85),
            (14.0, 12, 1.5, 0.75),
            (16.5, 10, 1.5, 0.80),
            (18.0, 14, 1.0, 0.75),
            (19.0, 12, 1.0, 0.65),
            (20.0, 10, 1.5, 0.80),
            (21.5, 12, 0.5, 0.65),
            (22.0, 14, 2.0, 0.85),
            (24.0, 10, 1.5, 0.80),
            (25.5, 9, 0.5, 0.65),
            (26.0, 7, 2.0, 0.80),
            (28.0, 5, 2.0, 0.75),
            (30.0, 2, 1.5, 0.75),
            (32.0, 4, 2.0, 0.75),
            (34.0, 7, 2.0, 0.80),
            (36.0, 12, 2.0, 0.80),
            (38.0, 7, 1.8, 0.75),
        ],
        key_pcs: &[5, 7, 9, 10, 0, 2, 4],
        phrase_bars: 4,
        comp: vec![(0, Comp::Arp4), (4, Comp::Arp8), (8, Comp::Arp4)],
        strings_level: 0.090,
        bass_level: 0.50,
        arp_level: 0.72,
        lead_level: 0.96,
        anchor: 2,
        reverb: (0.82, 0.32, 0.24),
        target_rms_dbfs: -18.5,
        seed: 0x7171_0005,
    }
}

pub fn render_title() -> Stereo {
    render_song(&title_spec())
}
pub fn render_daily() -> Stereo {
    render_song(&daily_spec())
}
pub fn render_calm() -> Stereo {
    render_song(&calm_spec())
}
pub fn render_tension() -> Stereo {
    render_song(&tension_spec())
}
pub fn render_night() -> Stereo {
    render_song(&night_spec())
}

// ============================================================
// SE 共通
// ============================================================

/// モノラル信号を `tail_secs` 秒の無音で延長し、リバーブを通して再びモノラルへ。
fn mono_reverb(x: &[f32], tail_secs: f32, feedback: f32, damp: f32, wet: f32) -> Vec<f32> {
    let n = x.len() + (tail_secs * SR as f32) as usize;
    let mut b = Stereo::zeros(SR, n);
    b.l[..x.len()].copy_from_slice(x);
    b.r[..x.len()].copy_from_slice(x);
    apply_reverb(&mut b, feedback, damp, wet);
    b.l.iter()
        .zip(b.r.iter())
        .map(|(l, r)| 0.5 * (l + r))
        .collect()
}

/// DC 除去 → 先頭/末尾の余弦フェード → ピーク正規化。フェードを最後に掛けるので
/// 先頭/末尾サンプルは確実に 0 近傍になる（クリック防止）。
fn finish_se(x: &mut [f32], fade_in_secs: f32, fade_out_secs: f32, peak: f32) {
    let n = x.len();
    let mean = x.iter().sum::<f32>() / n.max(1) as f32;
    for v in x.iter_mut() {
        *v -= mean;
    }
    let fi = ((fade_in_secs * SR as f32) as usize).clamp(1, n / 2);
    let fo = ((fade_out_secs * SR as f32) as usize).clamp(1, n / 2);
    for (i, v) in x.iter_mut().take(fi).enumerate() {
        *v *= 0.5 - 0.5 * (PI * i as f32 / fi as f32).cos();
    }
    for i in 0..fo {
        x[n - 1 - i] *= 0.5 - 0.5 * (PI * i as f32 / fo as f32).cos();
    }
    let pk = x.iter().fold(0f32, |m, &v| m.max(v.abs()));
    if pk > 1e-9 {
        let g = peak / pk;
        for v in x.iter_mut() {
            *v *= g;
        }
    }
}

// ============================================================
// chime.wav: 日本の学校チャイム風 4 音
// ============================================================

/// チャイムの 4 音（ファ・ラ・ソ・ド = F5 A5 G5 C5。ウェストミンスター型の
/// 「1-3-2-5(下)」で、最後の C5 だけ長く余韻を残す）。
pub const CHIME_FREQS: [f32; 4] = [698.46, 880.00, 783.99, 523.25];

/// 柔らかいビブラフォン/鐘系の部分音: (基音に対する整数倍, 振幅, 減衰時間の倍率)。
/// 金属的な非整数倍音（ティアス 1.19 倍など）は使わない。倍音は 4 次まで。
const BELL_PARTIALS: [(f32, f32, f32); 4] = [
    (1.0, 1.00, 1.0),
    (2.0, 0.28, 0.5),
    (3.0, 0.09, 0.3),
    (4.0, 0.04, 0.2),
];

fn add_bell(out: &mut [f32], start_secs: f32, freq: f32, decay_secs: f32, level: f32) {
    let srf = SR as f32;
    let start = (start_secs * srf) as usize;
    let len = (decay_secs * 6.0 * srf) as usize;
    for (k, &(mult, amp, dmul)) in BELL_PARTIALS.iter().enumerate() {
        let f = freq * mult;
        // 近接した 2 本目（+0.12%、約 2 セント）で、鐘特有のゆるいうなりを作る。
        let f2 = f * 1.0012;
        let tau = decay_secs * dmul;
        let ph = k as f32 * 0.7;
        for i in 0..len {
            let idx = start + i;
            if idx >= out.len() {
                break;
            }
            let t = i as f32 / srf;
            let env = (-t / tau).exp() * (t / 0.008).min(1.0);
            let s = (TAU * f * t + ph).sin() + 0.5 * (TAU * f2 * t + ph + 1.3).sin();
            out[idx] += s * amp * env * level;
        }
    }
}

/// 全長 3.6 秒（末尾 0.7 秒でフェードアウト）。
pub fn render_chime() -> Vec<f32> {
    let total = 3.6f32;
    let mut x = vec![0.0f32; (total * SR as f32) as usize];
    let notes = [
        (0.00, CHIME_FREQS[0], 0.55, 0.85),
        (0.62, CHIME_FREQS[1], 0.55, 0.80),
        (1.24, CHIME_FREQS[2], 0.55, 0.80),
        (1.86, CHIME_FREQS[3], 0.95, 1.00),
    ];
    for &(t, f, d, lv) in notes.iter() {
        add_bell(&mut x, t, f, d, lv);
    }
    let mut y = mono_reverb(&x, 0.0, 0.80, 0.35, 0.22);
    y.truncate(x.len());
    finish_se(&mut y, 0.003, 0.7, 0.60);
    y
}

/// チャイム 1 音だけを（`render_chime` と同じ音色・リバーブで）鳴らしたもの。
/// 部分音の整数倍関係を、他の音の余韻に邪魔されずにテストするために使う。
pub fn render_chime_note(freq: f32) -> Vec<f32> {
    let mut x = vec![0.0f32; (2.0 * SR as f32) as usize];
    add_bell(&mut x, 0.0, freq, 0.55, 0.85);
    let mut y = mono_reverb(&x, 0.0, 0.80, 0.35, 0.22);
    y.truncate(x.len());
    y
}

// ============================================================
// sparkle.wav: 流れ星のきらめき
// ============================================================

/// G 長調の分散和音（G5 B5 D6 G6）を 0.085 秒間隔で駆け上がる、柔らかいサイン主体の
/// 4 音（各音は 2 倍音を弱く足すだけ）。高域は G6 の 2 倍音（約 3.1kHz）までで、
/// 耳に刺さる帯域を作らない。リバーブの尾が余韻。全長 1.0 秒。
pub fn render_sparkle() -> Vec<f32> {
    let srf = SR as f32;
    let n = (0.7 * srf) as usize;
    let mut x = vec![0.0f32; n];
    let notes = [
        (0.0f32, 783.99f32, 0.8f32),
        (0.085, 987.77, 0.8),
        (0.17, 1174.66, 0.75),
        (0.255, 1567.98, 0.65),
    ];
    for &(t0, f, lv) in notes.iter() {
        let start = (t0 * srf) as usize;
        for i in 0..(0.5 * srf) as usize {
            let idx = start + i;
            if idx >= n {
                break;
            }
            let t = i as f32 / srf;
            let env = (t / 0.008).min(1.0) * (-t / 0.11).exp();
            let s = (TAU * f * t).sin() + 0.16 * (TAU * 2.0 * f * t + 0.5).sin();
            x[idx] += s * env * lv;
        }
    }
    let mut y = mono_reverb(&x, 0.35, 0.80, 0.35, 0.30);
    finish_se(&mut y, 0.004, 0.35, 0.45);
    y
}

// ============================================================
// heartbeat.wav: 低い鼓動 2 拍
// ============================================================

fn add_thump(out: &mut [f32], start_secs: f32, f_start: f32, f_end: f32, level: f32) {
    let srf = SR as f32;
    let start = (start_secs * srf) as usize;
    let len = (0.5 * srf) as usize;
    let mut phase = 0f32;
    for i in 0..len {
        let idx = start + i;
        if idx >= out.len() {
            break;
        }
        let t = i as f32 / srf;
        let f = f_end + (f_start - f_end) * (-t / 0.05).exp();
        phase += f / srf;
        phase -= phase.floor();
        let env = (1.0 - (-t / 0.014).exp()) * (-t / 0.09).exp();
        let s = (phase * TAU).sin()
            + 0.20 * (2.0 * phase * TAU).sin()
            + 0.04 * (3.0 * phase * TAU).sin();
        out[idx] += s * env * level;
    }
}

/// 「ドクン…ドクン」: 1 拍は強い一打（約 58Hz へ落ちる 82Hz のサイン）と 0.21 秒後の
/// 弱い二打（約 64Hz）の組、それを 0.78 秒間隔で 2 拍。立ち上がりを 14ms に丸め、
/// 2・3 次倍音はごく弱く、全体を低く柔らかくする。
pub fn render_heartbeat() -> Vec<f32> {
    let total = 1.7f32;
    let mut x = vec![0.0f32; (total * SR as f32) as usize];
    for &t0 in [0.06f32, 0.84].iter() {
        add_thump(&mut x, t0, 82.0, 58.0, 1.0);
        add_thump(&mut x, t0 + 0.21, 90.0, 64.0, 0.62);
    }
    finish_se(&mut x, 0.004, 0.3, 0.7);
    x
}
