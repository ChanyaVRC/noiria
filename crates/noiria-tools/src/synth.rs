//! ミニ手続き生成シンセサイザー（BGM全面改修版）。
//!
//! 外部音源ファイルは一切使わず（ライセンスフリー自作維持）、以下の自作 DSP
//! だけで BGM/SE の波形を組み立てる:
//! - オシレータ: サイン / 三角 / デチューン saw ペア / 2op FM
//! - Karplus-Strong 撥弦（クリック SE 専用。BGM 本体では使わない）
//! - 1-pole ローパスフィルタ（時間変化するカットオフに対応）
//! - ADSR エンベロープ
//! - ビブラート（発音から遅れて立ち上がる緩やかなピッチLFO）
//! - コーラス/アンサンブル（LFO変調ディレイ数本によるステレオ拡張）
//! - 自作 Schroeder/Freeverb 型リバーブ（並列コム＋直列オールパス）
//! - フィードバックディレイ（補助的なアンビエンス。主残響はリバーブが担う）
//! - マスターソフトクリップ（tanh）
//!
//! # 「エラー音／アラームに聞こえる」問題への対応（経緯）
//!
//! ①初版のアルペジオが「エラー音っぽい」→ ②プラックを排除し持続音構成に
//! 変更 → ③それでも「伸ばしている音までエラー音に聞こえる」という評価が
//! 続いたため、このバージョンでは Web リサーチ（詳細は
//! `docs/bgm-design-notes.md` を参照）に基づき音響設計を全面的にやり直した。
//! 要点:
//!
//! - 警告音は「短い音」だから警告音なのではなく、**(a) 反射のない孤立音、
//!   (b) 完全に静的な音色/音高、(c) 一定パルスレートでの規則的な繰り返し**
//!   という構造的特徴を持つ（Patterson の pulse/burst/warning 階層論、
//!   ラフネス/フラクチュエーション知覚など）。改修前の実装は撥弦アルペジオ
//!   だけでなく、**単発フィードバックディレイが一定周期で同じ音高を打ち返す
//!   こと自体**が「規則的パルス」に該当し、パッドのような持続音にまで
//!   アラーム的な印象を与えていたと診断した。
//! - 対策として、(1) 自作リバーブを全曲に適用して「反射のない孤立音」を
//!   根絶し、(2) ディレイの比重を下げてリバーブに主残響を任せ、(3) ビブラート
//!   ／ゆっくりしたフィルタ・音量の揺らぎ／コーラスで「完全に静的な音」を
//!   根絶し、(4) 撥弦（KS）を BGM 本体から完全に排除してレガートな
//!   2op FM エレピに置き換え、規則的な鋭いオンセットの反復を無くした。
//! - 加えて、コード進行はブロック平行移動ではなく声部連結（共通音保持・
//!   最小移動・転回形）でボイシングし、微小なヒューマナイズ（タイミング/
//!   音量/ピッチドリフト）を全声部に加えることで「機械が鳴らしている」
//!   無機質さも合わせて低減している。
//!
//! 曲は「テンポ・小節・コード進行（度数指定）・声部（パッド/ベース/
//! メロディ）をデータで書く」小さな枠組みの上に、各曲ごとのアレンジを
//! 手続き的に積み上げる形で実装している。
//!
//! ループはシームレスにするため「ループ長 + テール」を通しでレンダリングし、
//! テールをループ先頭へ折り返し加算 (`fold_tail_into_loop`) してからループ長
//! ちょうどに切り詰める。

use std::f32::consts::PI;

const TAU: f32 = 2.0 * PI;

/// 全曲共通のサンプルレート。
pub const SR: u32 = 44100;

/// 基準音（C4）。曲のコードは全てこの周波数からの半音オフセットで指定する。
pub const C4: f32 = 261.6256;

// ============================================================
// 乱数（xorshift64、依存クレート追加を避けるための自作 PRNG）
// ============================================================

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 16) as u32
    }
    /// -1.0..=1.0 の一様乱数。
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

// ============================================================
// ステレオバス
// ============================================================

pub struct Stereo {
    pub sr: u32,
    pub l: Vec<f32>,
    pub r: Vec<f32>,
}

impl Stereo {
    pub fn zeros(sr: u32, n: usize) -> Self {
        Self {
            sr,
            l: vec![0.0; n],
            r: vec![0.0; n],
        }
    }
    pub fn len(&self) -> usize {
        self.l.len()
    }
    pub fn is_empty(&self) -> bool {
        self.l.is_empty()
    }
    pub fn seconds(&self) -> f32 {
        self.len() as f32 / self.sr as f32
    }
    #[inline]
    pub fn add_stereo_at(&mut self, i: usize, l: f32, r: f32) {
        if i < self.l.len() {
            self.l[i] += l;
            self.r[i] += r;
        }
    }
}

/// -1(左)..1(右) の等パワーパンからL/Rゲインを求める。
fn pan_gains(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    let theta = (p + 1.0) * 0.25 * PI; // 0..PI/2
    (theta.cos(), theta.sin())
}

/// モノラルの完成波形をパン付きでバスへ加算する。
fn add_mono_track(buf: &mut Stereo, start_s: usize, samples: &[f32], level: f32, pan: f32) {
    let (lg, rg) = pan_gains(pan);
    for (i, &s) in samples.iter().enumerate() {
        buf.add_stereo_at(start_s + i, s * level * lg, s * level * rg);
    }
}

// ============================================================
// オシレータ
// ============================================================

pub fn osc_sine(phase: f32) -> f32 {
    (phase * TAU).sin()
}
pub fn osc_tri(phase: f32) -> f32 {
    let p = phase - phase.floor();
    4.0 * (p - 0.5).abs() - 1.0
}
pub fn osc_saw(phase: f32) -> f32 {
    let p = phase - phase.floor();
    2.0 * p - 1.0
}

// ============================================================
// エンベロープ / フィルタ
// ============================================================

#[derive(Clone, Copy)]
pub struct Adsr {
    pub a: f32,
    pub d: f32,
    pub s: f32,
    pub r: f32,
}

impl Adsr {
    /// t: ノートオンからの経過秒。hold: ノートオン→ノートオフ（リリース開始）までの長さ秒。
    pub fn value(&self, t: f32, hold: f32) -> f32 {
        if t < 0.0 {
            return 0.0;
        }
        if t < self.a {
            if self.a <= 1e-6 {
                1.0
            } else {
                t / self.a
            }
        } else if t < self.a + self.d {
            let dt = (t - self.a) / self.d.max(1e-6);
            1.0 + (self.s - 1.0) * dt
        } else if t < hold {
            self.s
        } else {
            let rt = (t - hold) / self.r.max(1e-6);
            if rt >= 1.0 {
                0.0
            } else {
                self.s * (1.0 - rt)
            }
        }
    }
}

/// 1-pole ローパスフィルタ。`set_cutoff` でカットオフを時間変化させられる
/// （パッドに「完全に静的ではない」フィルタの揺らぎを持たせるため）。
pub struct Lpf {
    a: f32,
    z: f32,
}
impl Lpf {
    pub fn new(cutoff_hz: f32, sr: f32) -> Self {
        let a = (-TAU * cutoff_hz / sr).exp();
        Self { a, z: 0.0 }
    }
    #[inline]
    pub fn set_cutoff(&mut self, cutoff_hz: f32, sr: f32) {
        self.a = (-TAU * cutoff_hz / sr).exp();
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.z = x * (1.0 - self.a) + self.z * self.a;
        self.z
    }
}

// ============================================================
// ビブラート / 低速LFO
// ============================================================

/// ビブラート用ピッチ乗数。`onset_delay` 秒までは無変調で、そこから 0.3 秒
/// かけて深さ `depth_cents` までフェードインしながら `rate_hz` で揺れる。
/// 歌唱/弦楽器のビブラートが「発音直後は素の音程、少し遅れて揺れ始める」
/// という研究知見を模す（発音から遅れて始まることで「完全に安定した音」を
/// 崩す一方、アタック直後の音程感はぼやけさせない）。
fn vibrato_mult(t: f32, rate_hz: f32, depth_cents: f32, onset_delay: f32) -> f32 {
    if t < onset_delay {
        return 1.0;
    }
    let te = t - onset_delay;
    let fade = (te / 0.3).min(1.0);
    let cents = depth_cents * fade * (te * TAU * rate_hz).sin();
    2f32.powf(cents / 1200.0)
}

/// -1..1 の低速LFO（フィルタカットオフや音量をゆっくり揺らすために使う）。
fn slow_lfo(t: f32, rate_hz: f32, phase0: f32) -> f32 {
    ((t * rate_hz + phase0) * TAU).sin()
}

// ============================================================
// コーラス/アンサンブル（LFO変調ディレイ）
// ============================================================

/// LFO変調ディレイ1本（線形補間読み出し）。コーラス効果の構成要素。
struct ModDelay {
    buf: Vec<f32>,
    write: usize,
    base_n: f32,
    depth_n: f32,
    rate_hz: f32,
    phase: f32,
    sr: f32,
}
impl ModDelay {
    fn new(sr: f32, base_ms: f32, depth_ms: f32, rate_hz: f32, phase0: f32) -> Self {
        let max_n = (((base_ms + depth_ms) * 0.001 * sr) as usize) + 4;
        Self {
            buf: vec![0.0; max_n.max(4)],
            write: 0,
            base_n: base_ms * 0.001 * sr,
            depth_n: depth_ms * 0.001 * sr,
            rate_hz,
            phase: phase0,
            sr,
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let n = self.buf.len();
        self.buf[self.write] = x;
        let mod_n = self.base_n + self.depth_n * (self.phase * TAU).sin();
        self.phase += self.rate_hz / self.sr;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let delay = mod_n.max(1.0);
        let read_pos = ((self.write as f32 - delay) % n as f32 + n as f32) % n as f32;
        let i0 = read_pos.floor() as usize;
        let frac = read_pos - i0 as f32;
        let i1 = (i0 + 1) % n;
        let y = self.buf[i0] * (1.0 - frac) + self.buf[i1] * frac;
        self.write = (self.write + 1) % n;
        y
    }
}

/// コーラス/アンサンブル効果: LFO変調ディレイを2〜3本重ね、L/Rで異なる
/// レート/位相にすることでステレオの広がりを作る。「複数の楽器が同時に
/// わずかにずれて鳴っている」ような自然な揺らぎを、単一オシレータの
/// デチューンだけよりも豊かに加える。パッドとリード（FMエレピ）に適用する。
pub struct Chorus {
    voices_l: Vec<ModDelay>,
    voices_r: Vec<ModDelay>,
}
impl Chorus {
    pub fn new(sr: u32, voices: usize, seed: u64) -> Self {
        let srf = sr as f32;
        let mut rng = Rng::new(seed);
        let mut voices_l = Vec::new();
        let mut voices_r = Vec::new();
        for _ in 0..voices {
            let base = 9.0 + rng.next_f32().abs() * 10.0; // 9〜19ms
                                                          // 深さ 2〜5ms → 1〜2.5ms（約半分）: ユーザー評価「うなりが深すぎる」への
                                                          // 聴感優先の調整（docs/bgm-design-notes.md §6.4）。変調ディレイの深さは
                                                          // そのままピッチ揺れ→合成後の振幅うなりの深さになるため半減した。
                                                          // 0にはしない（完全に静的な音への逆戻り禁止）。
            let depth = 1.0 + rng.next_f32().abs() * 1.5; // 1〜2.5ms
            let rate = 0.15 + rng.next_f32().abs() * 0.45; // 0.15〜0.6Hz
            let ph0 = rng.next_f32().abs();
            voices_l.push(ModDelay::new(srf, base, depth, rate, ph0));
            voices_r.push(ModDelay::new(
                srf,
                base * 1.09,
                depth,
                rate * 0.91,
                ph0 + 0.5,
            ));
        }
        Self { voices_l, voices_r }
    }
    pub fn process(&mut self, buf: &mut Stereo, mix: f32) {
        let nv = self.voices_l.len().max(1) as f32;
        for i in 0..buf.len() {
            let dl = buf.l[i];
            let dr = buf.r[i];
            let mut wl = 0.0;
            let mut wr = 0.0;
            for v in self.voices_l.iter_mut() {
                wl += v.process(dl);
            }
            for v in self.voices_r.iter_mut() {
                wr += v.process(dr);
            }
            buf.l[i] = dl * (1.0 - mix) + (wl / nv) * mix;
            buf.r[i] = dr * (1.0 - mix) + (wr / nv) * mix;
        }
    }
}

// ============================================================
// リバーブ（自作 Schroeder / Freeverb 型）
// ============================================================

/// フィードバック経路に1-poleローパスを内蔵したコムフィルタ（Freeverb型）。
/// 高域ほど早く減衰する自然な残響を作る。
struct CombLpf {
    buf: Vec<f32>,
    idx: usize,
    feedback: f32,
    damp: f32,
    lp_state: f32,
}
impl CombLpf {
    fn new(delay_n: usize, feedback: f32, damp: f32) -> Self {
        Self {
            buf: vec![0.0; delay_n.max(1)],
            idx: 0,
            feedback,
            damp,
            lp_state: 0.0,
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let n = self.buf.len();
        let out = self.buf[self.idx];
        self.lp_state = out * (1.0 - self.damp) + self.lp_state * self.damp;
        self.buf[self.idx] = x + self.lp_state * self.feedback;
        self.idx = (self.idx + 1) % n;
        out
    }
}

/// Schroeder オールパスフィルタ。
struct Allpass {
    buf: Vec<f32>,
    idx: usize,
    g: f32,
}
impl Allpass {
    fn new(delay_n: usize, g: f32) -> Self {
        Self {
            buf: vec![0.0; delay_n.max(1)],
            idx: 0,
            g,
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let n = self.buf.len();
        let buf_out = self.buf[self.idx];
        let y = buf_out - self.g * x;
        self.buf[self.idx] = x + self.g * y;
        self.idx = (self.idx + 1) % n;
        y
    }
}

/// 自作 Schroeder/Freeverb 型リバーブ。並列コム（6本、フィードバック内に
/// ローパスを持つ）→ 直列オールパス（3本）という定番構成。コムの遅延長は
/// 互いに素（素数）を選び、周期的な色付けの相関を避けて密な残響にする。
/// R側は各遅延長に一定サンプルのオフセットを足してL/Rを非相関化し、
/// ステレオ感のある拡散した残響を作る（Freeverb の stereospread に相当する
/// 一般的な技法。定数自体はこのエンジン用に独自に選定）。
///
/// 「反射のない孤立音」というアラーム的特徴を全曲から根絶するのがこの
/// リバーブの主目的（詳細は `docs/bgm-design-notes.md` 参照）。
pub struct Reverb {
    combs_l: Vec<CombLpf>,
    combs_r: Vec<CombLpf>,
    aps_l: Vec<Allpass>,
    aps_r: Vec<Allpass>,
}
impl Reverb {
    pub fn new(sr: u32, feedback: f32, damp: f32) -> Self {
        let scale = sr as f32 / 44100.0;
        const COMB_BASE: [usize; 6] = [1013, 1097, 1181, 1279, 1373, 1471];
        const AP_BASE: [usize; 3] = [227, 347, 449];
        const SPREAD: usize = 19;
        let fb = feedback.clamp(0.6, 0.98);
        let combs_l = COMB_BASE
            .iter()
            .map(|&n| CombLpf::new(((n as f32) * scale) as usize, fb, damp))
            .collect();
        let combs_r = COMB_BASE
            .iter()
            .map(|&n| CombLpf::new(((n + SPREAD) as f32 * scale) as usize, fb, damp))
            .collect();
        let aps_l = AP_BASE
            .iter()
            .map(|&n| Allpass::new(((n as f32) * scale) as usize, 0.5))
            .collect();
        let aps_r = AP_BASE
            .iter()
            .map(|&n| Allpass::new(((n + SPREAD) as f32 * scale) as usize, 0.5))
            .collect();
        Self {
            combs_l,
            combs_r,
            aps_l,
            aps_r,
        }
    }
    #[inline]
    fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let mono_in = (in_l + in_r) * 0.5;
        let mut out_l = 0.0;
        for c in self.combs_l.iter_mut() {
            out_l += c.process(mono_in);
        }
        let mut out_r = 0.0;
        for c in self.combs_r.iter_mut() {
            out_r += c.process(mono_in);
        }
        out_l /= self.combs_l.len() as f32;
        out_r /= self.combs_r.len() as f32;
        for a in self.aps_l.iter_mut() {
            out_l = a.process(out_l);
        }
        for a in self.aps_r.iter_mut() {
            out_r = a.process(out_r);
        }
        (out_l, out_r)
    }
}

/// マスターバス全体にリバーブを掛ける（ウェットを加算、ドライは維持）。
pub fn apply_reverb(buf: &mut Stereo, feedback: f32, damp: f32, wet: f32) {
    let mut rv = Reverb::new(buf.sr, feedback, damp);
    for i in 0..buf.len() {
        let (rl, rr) = rv.process(buf.l[i], buf.r[i]);
        buf.l[i] += rl * wet;
        buf.r[i] += rr * wet;
    }
}

// ============================================================
// Karplus-Strong 撥弦（クリック SE 専用。BGM 本体では使わない）
// ============================================================

/// ノイズバースト + 短い遅延線 + ローパスフィードバックによるプラック音色。
/// **BGM の声部には使わない**（撥弦の「バースト励起→単調減衰」という構造は
/// 音色を丸めても Patterson の警告音 pulse 定義と一致してしまうため）。
/// UI クリック SE のような単発の効果音にのみ用いる。
pub fn karplus_strong(
    freq: f32,
    secs: f32,
    sr: u32,
    damping: f32,
    brightness_hz: f32,
    seed: u64,
) -> Vec<f32> {
    let n = ((sr as f32 / freq).round() as usize).max(2);
    let mut rng = Rng::new(seed);
    let raw: Vec<f32> = (0..n).map(|_| rng.next_f32()).collect();
    let mut ring: Vec<f32> = (0..n)
        .map(|i| {
            let prev = raw[(i + n - 1) % n];
            let cur = raw[i];
            let next = raw[(i + 1) % n];
            (prev + 2.0 * cur + next) * 0.25
        })
        .collect();
    let total = (secs * sr as f32) as usize;
    let mut out = Vec::with_capacity(total);
    let mut idx = 0usize;
    for _ in 0..total {
        let cur = ring[idx];
        let nxt = ring[(idx + 1) % n];
        let filtered = 0.5 * (cur + nxt) * damping;
        ring[idx] = filtered;
        out.push(cur);
        idx = (idx + 1) % n;
    }
    let mut lpf = Lpf::new(brightness_hz, sr as f32);
    for v in out.iter_mut() {
        *v = lpf.process(*v);
    }
    let fade_n = ((0.006 * sr as f32) as usize).min(out.len());
    for (i, v) in out.iter_mut().take(fade_n).enumerate() {
        *v *= i as f32 / fade_n.max(1) as f32;
    }
    out
}

/// アルペジオ/メロディ系の声部が超えてはいけない実効音域の上限（およそ C5）。
/// 短い音が中高音域〜高音域で孤立して鳴ると通知音/エラー音と音響的特徴が
/// 似てしまうため、旋律系の声部は原則としてこれを超えない。
pub const MELODY_CAP_HZ: f32 = 523.3;

/// `freq` を `[lo, hi]` の帯域に収まるまでオクターブ移動する。メロディ声部の
/// 音を「声部連結済みのパッドボイシングの中から選びつつ、常に同じ狭い帯域に
/// 折り畳む」ために使う（下限も設けることで、たまたま低い声部を選んでも
/// 極端に低いオクターブへ飛ばない）。
fn fold_into_range(freq: f32, lo: f32, hi: f32) -> f32 {
    let mut f = freq;
    while f < lo {
        f *= 2.0;
    }
    while f > hi {
        f *= 0.5;
    }
    f
}

// ============================================================
// コード（度数指定）/ 声部連結（voice leading）
// ============================================================

#[derive(Clone, Copy)]
pub enum ChordQ {
    Maj,
    Min,
    Maj7,
    Min7,
    Dom7,
}

fn chord_intervals(q: ChordQ) -> &'static [i32] {
    match q {
        ChordQ::Maj => &[0, 4, 7],
        ChordQ::Min => &[0, 3, 7],
        ChordQ::Maj7 => &[0, 4, 7, 11],
        ChordQ::Min7 => &[0, 3, 7, 10],
        ChordQ::Dom7 => &[0, 4, 7, 10],
    }
}

/// `root_hz` を基準に、半音オフセット `semitones` の音高を返す。
pub fn degree_freq(root_hz: f32, semitones: i32) -> f32 {
    root_hz * 2f32.powf(semitones as f32 / 12.0)
}

/// `target` と同じピッチクラス（12で割った余り）を持ち、`near` に最も近い
/// 絶対半音値を返す。
fn nearest_octave(target: i32, near: i32) -> i32 {
    let base = target.rem_euclid(12);
    let near_oct = ((near - base) as f32 / 12.0).round() as i32;
    base + near_oct * 12
}

/// 0..n の順列を総当たりで全列挙する（n<=4 程度の小規模専用、24通り以内）。
fn permutations(n: usize) -> Vec<Vec<usize>> {
    fn helper(items: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
        if k == items.len() {
            out.push(items.clone());
            return;
        }
        for i in k..items.len() {
            items.swap(k, i);
            helper(items, k + 1, out);
            items.swap(k, i);
        }
    }
    let mut items: Vec<usize> = (0..n).collect();
    let mut out = Vec::new();
    helper(&mut items, 0, &mut out);
    out
}

/// 声部連結（voice leading）。前の和音のボイシング（C4基準の絶対半音、
/// 声部ごとに1本の連続した「声部の線」を成す）から、次の和音
/// （`root_semi` + `quality`）への移動量が最小になる声部の対応付けを
/// 総当たり（声部数は最大4なので24通り）で探す。共通音は移動距離0で
/// そのまま保持され、転回形も自然に選ばれる。コードトーン数が
/// `voice_count` に満たない場合はルートをオクターブ上に複製して埋める。
/// `prev` が `None`（曲頭）の場合は `anchor` を中心にした close position を
/// 初期ボイシングとする。
///
/// ブロック平行移動（毎回ルート+固定音程で組み直す）は声部が全て同じ方向へ
/// 大きく飛ぶため機械的に聞こえやすい——これを避けるのがこの関数の目的。
fn voice_lead(
    prev: Option<&[i32]>,
    root_semi: i32,
    quality: ChordQ,
    voice_count: usize,
    anchor: i32,
) -> Vec<i32> {
    let mut targets: Vec<i32> = chord_intervals(quality)
        .iter()
        .map(|iv| root_semi + iv)
        .collect();
    while targets.len() < voice_count {
        targets.push(root_semi + 12);
    }
    targets.truncate(voice_count);

    match prev {
        None => {
            let mut v: Vec<i32> = targets.iter().map(|&t| nearest_octave(t, anchor)).collect();
            v.sort_unstable();
            v
        }
        Some(prev_v) => {
            let n = prev_v.len().min(voice_count);
            let mut best_cost = i32::MAX;
            let mut best_assign = vec![0i32; n];
            for perm in permutations(n) {
                let mut cost = 0i32;
                let mut assign = vec![0i32; n];
                for (voice_i, &target_i) in perm.iter().enumerate() {
                    let note = nearest_octave(targets[target_i], prev_v[voice_i]);
                    assign[voice_i] = note;
                    cost += (note - prev_v[voice_i]).abs();
                }
                if cost < best_cost {
                    best_cost = cost;
                    best_assign = assign;
                }
            }
            best_assign
        }
    }
}

// ============================================================
// 声部レンダラ
// ============================================================

/// パッド: デチューンした saw ペアを L/R それぞれ独立位相で鳴らし、
/// 時間変化するローパスを通してから ADSR で包絡する。加えて:
/// - ビブラート（発音から0.4秒遅れて立ち上がる5.6Hz・微小セント）
/// - ゆっくりしたフィルタカットオフの揺らぎ（0.11Hz、±11%）
/// - ゆっくりした音量の揺らぎ（0.17Hz、±2.5%）
/// - コーラス/アンサンブル（LFO変調ディレイ3本、35%ミックス）
///
/// を重ねることで「完全に静的な音」を無くす。コードトーン全部を重ねて
/// 厚みを出す。
///
/// 各揺らぎの深さはユーザー評価「うなりが深すぎる」を受けて一律ほぼ半分に
/// 減らした（docs/bgm-design-notes.md §6.4）。ゼロにはしない（完全に静的な
/// 「テストトーン」への逆戻り禁止）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_pad(
    buf: &mut Stereo,
    sr: u32,
    start_s: usize,
    len_s: usize,
    freqs: &[f32],
    level: f32,
    lpf_hz: f32,
    attack: f32,
    release: f32,
    seed: u64,
) {
    let srf = sr as f32;
    let adsr = Adsr {
        a: attack,
        d: 0.15,
        s: 0.85,
        r: release,
    };
    let hold_secs = len_s as f32 / srf;
    let extra = (release * srf) as usize + 32;
    let total = len_s + extra;
    // Hz 単位の絶対デチューン（saw ペアの基礎うねり）。0.7→0.35Hz（約半分）:
    // saw ペアのビート周波数はこの差分そのもの（L≈0.35Hz/R≈0.44Hz）なので、
    // 半減はうなり深度でなく「うなりの単純さ」はそのままに、位相干渉の谷を
    // 浅くする方向に効く（聴感優先の調整、docs/bgm-design-notes.md §6.4）。
    let detune = 0.35_f32;
    let mut rng = Rng::new(seed);

    // ローカルスクラッチへ描画してからコーラスを掛け、最後にメインバスへ
    // ミックスする（パッド全体に自然にコーラスがかかるようにするため）。
    let mut scratch = Stereo::zeros(sr, total);

    for &f in freqs {
        let mut lpf_l = Lpf::new(lpf_hz, srf);
        let mut lpf_r = Lpf::new(lpf_hz, srf);
        let ph0 = rng.next_f32().abs();
        let mut phase_l1 = ph0;
        let mut phase_l2 = ph0 + 0.37;
        let mut phase_r1 = ph0 + 0.13;
        let mut phase_r2 = ph0 + 0.61;
        // 各LFOの位相をボイスごとにばらけさせ、揺らぎが全ボイス一斉に動いて
        // 再び「一定パターン」に戻らないようにする。
        let filt_lfo_phase = rng.next_f32().abs();
        let amp_lfo_phase = rng.next_f32().abs();
        let vib_phase_off = rng.next_f32().abs() * 0.2;

        for i in 0..total {
            let t = i as f32 / srf;
            let env = adsr.value(t, hold_secs);

            // 揺らぎ深さは聴感優先で約半分に減量（§6.4）: ビブラート 4.0→1.5セント
            // （5.6Hz と速い=知覚されやすい変調なので優先的に浅く）、フィルタLFO
            // ±22→±11%、音量LFO ±5→±2.5%。
            let vib = vibrato_mult(t + vib_phase_off, 5.6, 1.5, 0.4);
            let filt_mod = 1.0 + 0.11 * slow_lfo(t, 0.11, filt_lfo_phase);
            let amp_mod = 1.0 + 0.025 * slow_lfo(t, 0.17, amp_lfo_phase);

            lpf_l.set_cutoff((lpf_hz * filt_mod).max(80.0), srf);
            lpf_r.set_cutoff((lpf_hz * filt_mod).max(80.0), srf);

            let fl1 = (f - detune * 0.5) * vib;
            let fl2 = (f + detune * 0.5) * vib;
            let fr1 = (f - detune * 0.63) * vib;
            let fr2 = (f + detune * 0.63) * vib;

            let sl = (osc_saw(phase_l1) + osc_saw(phase_l2)) * 0.5;
            let sr_ = (osc_saw(phase_r1) + osc_saw(phase_r2)) * 0.5;
            phase_l1 += fl1 / srf;
            phase_l1 -= phase_l1.floor();
            phase_l2 += fl2 / srf;
            phase_l2 -= phase_l2.floor();
            phase_r1 += fr1 / srf;
            phase_r1 -= phase_r1.floor();
            phase_r2 += fr2 / srf;
            phase_r2 -= phase_r2.floor();

            let fl = lpf_l.process(sl);
            let fr = lpf_r.process(sr_);
            let amp = env * level / freqs.len() as f32 * amp_mod;
            scratch.add_stereo_at(i, fl * amp, fr * amp);
        }
    }

    let mut chorus = Chorus::new(sr, 3, seed ^ 0x1357_9BDF);
    chorus.process(&mut scratch, 0.35);

    for i in 0..total {
        buf.add_stereo_at(start_s + i, scratch.l[i], scratch.r[i]);
    }
}

/// パーカッシブすぎない、丸めのベース（サイン主体 + 少量の三角波 + LPF）。
/// サイン波の単独露出はここ（サブベース）のみ許可する。
fn synth_bass_note(freq: f32, dur_s: f32, sr: u32, adsr: Adsr, cutoff_hz: f32) -> Vec<f32> {
    let srf = sr as f32;
    let n = (dur_s * srf) as usize + (adsr.r * srf) as usize + 16;
    let mut out = Vec::with_capacity(n);
    let mut phase = 0f32;
    let mut lpf = Lpf::new(cutoff_hz, srf);
    for i in 0..n {
        let t = i as f32 / srf;
        let env = adsr.value(t, dur_s);
        let raw = osc_sine(phase) * 0.85 + osc_tri(phase) * 0.15;
        phase += freq / srf;
        out.push(lpf.process(raw) * env);
    }
    out
}

/// 2オペレータ FM エレクトリックピアノ（キャリア比:モジュレータ比 = 1:1）。
/// モジュレーション・インデックスをアタック直後に高く、`mod_decay` 秒で
/// 指数減衰させることで、アタックは倍音豊富/サステインは丸い、という
/// エレピ特有の音色変化を作る（2op FM スタックによるエレピ設計の定石）。
/// ビブラートは発音から遅れて緩やかに立ち上がる。純サイン波の単独露出には
/// あたらない（FM で常に倍音を持つ）。
#[allow(clippy::too_many_arguments)]
fn fm_epiano_note(
    freq: f32,
    hold_secs: f32,
    sr: u32,
    adsr: Adsr,
    index0: f32,
    mod_decay: f32,
    seed: u64,
) -> Vec<f32> {
    let srf = sr as f32;
    let n = (hold_secs * srf) as usize + (adsr.r * srf) as usize + 32;
    let mut rng = Rng::new(seed);
    // ヒューマナイズ: ごくわずかなピッチドリフト（±3セント程度）。
    let pitch_drift = 2f32.powf(rng.next_f32() * 3.0 / 1200.0);
    let f0 = freq * pitch_drift;
    let vib_phase_off = rng.next_f32().abs() * 0.3;
    let mut phase_c = 0f32;
    let mut phase_m = 0f32;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / srf;
        let env = adsr.value(t, hold_secs);
        let mod_env = (-t / mod_decay).exp();
        let index = index0 * mod_env;
        // ビブラート深さ 5.0→2.0セント（聴感優先の減量、§6.4）。5.4Hz と速い
        // （>2Hz）変調は特に知覚されやすいため優先的に浅くした。0にはしない。
        let vib = vibrato_mult(t + vib_phase_off, 5.4, 2.0, 0.35);
        let fc = f0 * vib;
        let fm = f0 * vib; // キャリア比:モジュレータ比 = 1:1
        let modv = (phase_m * TAU).sin() * index;
        let s = (phase_c * TAU + modv).sin();
        phase_c += fc / srf;
        phase_c -= phase_c.floor();
        phase_m += fm / srf;
        phase_m -= phase_m.floor();
        out.push(s * env);
    }
    out
}

/// レガートな FM エレピのメロディ音符を1つ、コーラスで軽くステレオに広げて
/// からメインバスへ加算する。アタックはなだらか（`a=0.07s`）で、撥弦のような
/// 急峻なオンセットにならない。
#[allow(clippy::too_many_arguments)]
fn add_lead_note(
    buf: &mut Stereo,
    sr: u32,
    start_s: usize,
    freq: f32,
    hold_secs: f32,
    level: f32,
    pan: f32,
    seed: u64,
) {
    let adsr = Adsr {
        a: 0.07,
        d: 0.12,
        s: 0.72,
        r: 0.35,
    };
    let mono = fm_epiano_note(freq, hold_secs, sr, adsr, 2.3, 0.35, seed);
    let n = mono.len();
    let mut scratch = Stereo {
        sr,
        l: mono.clone(),
        r: mono,
    };
    let mut chorus = Chorus::new(sr, 2, seed ^ 0x2468_ACE0);
    chorus.process(&mut scratch, 0.4);
    let (lg, rg) = pan_gains(pan);
    for i in 0..n {
        buf.add_stereo_at(
            start_s + i,
            scratch.l[i] * level * lg,
            scratch.r[i] * level * rg,
        );
    }
}

// ============================================================
// エフェクト（ディレイ / マスター）
// ============================================================

/// 補助的なアンビエンス用フィードバックディレイ。L/R でわずかに遅延時間を
/// ずらして広がりを作る。**主残響は `apply_reverb` が担う**ため、ここは
/// ウェット/フィードバックを控えめにしてある。単発ディレイのタップが
/// 一定周期で同じ音高を打ち返すこと自体が Patterson の言う「一定パルス
/// レートの burst」に構造的に一致し、警告音的な印象の一因になり得るため
/// （詳細は `docs/bgm-design-notes.md` の診断を参照）、旧版よりウェットを
/// 下げてリバーブとの併用でこの規則性を目立たなくしている。
pub fn apply_delay(buf: &mut Stereo, base_delay_secs: f32, feedback: f32, wet: f32) {
    let sr = buf.sr as f32;
    let dl_n = ((base_delay_secs * sr).round() as usize).max(1);
    let dr_n = ((base_delay_secs * 1.03 * sr).round() as usize).max(1);
    let mut dl = vec![0.0f32; dl_n];
    let mut dr = vec![0.0f32; dr_n];
    let mut wl = 0usize;
    let mut wr = 0usize;
    for i in 0..buf.len() {
        let in_l = buf.l[i];
        let in_r = buf.r[i];
        let out_l = dl[wl];
        let out_r = dr[wr];
        dl[wl] = in_l + out_l * feedback;
        dr[wr] = in_r + out_r * feedback;
        wl = (wl + 1) % dl_n;
        wr = (wr + 1) % dr_n;
        buf.l[i] = in_l + out_l * wet;
        buf.r[i] = in_r + out_r * wet;
    }
}

fn remove_dc(buf: &mut Stereo) {
    if buf.l.is_empty() {
        return;
    }
    let n = buf.l.len() as f32;
    let ml: f32 = buf.l.iter().sum::<f32>() / n;
    let mr: f32 = buf.r.iter().sum::<f32>() / n;
    for v in buf.l.iter_mut() {
        *v -= ml;
    }
    for v in buf.r.iter_mut() {
        *v -= mr;
    }
}

fn scale(buf: &mut Stereo, g: f32) {
    for v in buf.l.iter_mut() {
        *v *= g;
    }
    for v in buf.r.iter_mut() {
        *v *= g;
    }
}

pub fn combined_rms(buf: &Stereo) -> f32 {
    if buf.l.is_empty() {
        return 0.0;
    }
    let n = buf.l.len() as f32;
    let sum: f32 = buf
        .l
        .iter()
        .zip(buf.r.iter())
        .map(|(l, r)| l * l + r * r)
        .sum();
    (sum / (2.0 * n)).sqrt()
}

pub fn combined_peak(buf: &Stereo) -> f32 {
    buf.l
        .iter()
        .chain(buf.r.iter())
        .fold(0f32, |m, &v| m.max(v.abs()))
}

pub fn dbfs(linear: f32) -> f32 {
    20.0 * linear.max(1e-9).log10()
}

/// dB（振幅比）→ 線形ゲイン。
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// L/R のピアソン相関係数（ステレオ感の検証用）。
pub fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len()) as f32;
    if n <= 1.0 {
        return 1.0;
    }
    let ma = a.iter().sum::<f32>() / n;
    let mb = b.iter().sum::<f32>() / n;
    let mut cov = 0.0f32;
    let mut va = 0.0f32;
    let mut vb = 0.0f32;
    for i in 0..n as usize {
        let da = a[i] - ma;
        let db = b[i] - mb;
        cov += da * db;
        va += da * da;
        vb += db * db;
    }
    if va <= 1e-12 || vb <= 1e-12 {
        return 1.0;
    }
    (cov / (va.sqrt() * vb.sqrt())).clamp(-1.0, 1.0)
}

/// RMS 正規化 → ソフトクリップ (tanh) → ピークセーフティトリムの順にかける。
pub fn finalize_master(buf: &mut Stereo, target_rms_dbfs: f32) {
    remove_dc(buf);
    let cur = combined_rms(buf);
    if cur > 1e-9 {
        let target_lin = db_to_lin(target_rms_dbfs);
        scale(buf, target_lin / cur);
    }
    for v in buf.l.iter_mut() {
        *v = v.tanh();
    }
    for v in buf.r.iter_mut() {
        *v = v.tanh();
    }
    let pk = combined_peak(buf);
    if pk > 0.95 {
        scale(buf, 0.95 / pk);
    }
}

/// ループ境界の確実な無音化。末尾 `taper_secs` 秒をゼロへ、先頭 `taper_secs`
/// 秒をゼロからそれぞれ線形フェードすることで、境界サンプルの飛びを確実に
/// 小さくする。
fn taper_loop_seam(buf: &mut Stereo, taper_secs: f32) {
    let n = buf.len();
    let taper_n = ((taper_secs * buf.sr as f32) as usize).min(n / 2).max(1);
    for i in 0..taper_n {
        let g_in = i as f32 / taper_n as f32;
        buf.l[i] *= g_in;
        buf.r[i] *= g_in;
        let g_out = 1.0 - i as f32 / taper_n as f32;
        let idx = n - taper_n + i;
        buf.l[idx] *= g_out;
        buf.r[idx] *= g_out;
    }
}

/// 「ループ長 + テール」でレンダリングした波形のテールをループ先頭へ折り返し
/// 加算し、ループ長ちょうどに切り詰める。リバーブ/ディレイの余韻や音の
/// 減衰しっぽが途切れず、境界がシームレスになる。
pub fn fold_tail_into_loop(buf: &Stereo, loop_n: usize) -> Stereo {
    let mut out = Stereo::zeros(buf.sr, loop_n);
    out.l[..loop_n].copy_from_slice(&buf.l[..loop_n]);
    out.r[..loop_n].copy_from_slice(&buf.r[..loop_n]);
    for i in loop_n..buf.len() {
        let j = (i - loop_n) % loop_n;
        out.l[j] += buf.l[i];
        out.r[j] += buf.r[i];
    }
    out
}

// ============================================================
// 曲構造
// ============================================================

struct Chord {
    semitone_offset: i32,
    quality: ChordQuality,
}

// re-export local alias to avoid clippy confusion with ChordQ enum name in call sites
type ChordQuality = ChordQ;

/// リバーブのパラメータ束（`render_looped` の引数を減らすための小さな構造体）。
#[derive(Clone, Copy)]
struct ReverbParams {
    feedback: f32,
    damp: f32,
    wet: f32,
}

fn bar_secs(bpm: f32) -> f32 {
    60.0 / bpm * 4.0
}

/// ループ長 + テールをレンダリングし、ディレイ→リバーブをかけ、テールを
/// 折り返し、マスタリングして返す共通ラッパ。`build` が dry な声部を書き込む。
#[allow(clippy::too_many_arguments)]
fn render_looped<F: FnOnce(&mut Stereo, u32)>(
    loop_secs: f32,
    tail_secs: f32,
    delay_base_secs: f32,
    delay_feedback: f32,
    delay_wet: f32,
    reverb: ReverbParams,
    target_rms_dbfs: f32,
    build: F,
) -> Stereo {
    let sr = SR;
    let loop_n = (loop_secs * sr as f32).round() as usize;
    let total_n = loop_n + (tail_secs * sr as f32).round() as usize;
    let mut buf = Stereo::zeros(sr, total_n);
    build(&mut buf, sr);
    apply_delay(&mut buf, delay_base_secs, delay_feedback, delay_wet);
    apply_reverb(&mut buf, reverb.feedback, reverb.damp, reverb.wet);
    let mut folded = fold_tail_into_loop(&buf, loop_n);
    finalize_master(&mut folded, target_rms_dbfs);
    taper_loop_seam(&mut folded, 0.02);
    folded
}

// ------------------------------------------------------------
// main.wav: 温かい日常曲
// ------------------------------------------------------------

/// 温かい日常曲。Am7-Fmaj7-Cmaj7-G7 のパッド + 声部連結メロディ(FMエレピ) +
/// サブベース。8 小節×2 周。
///
/// 「エラー音っぽさ」対策として:
/// - コード進行は `voice_lead` で声部連結（共通音保持・最小移動・転回形）し
///   てあり、ブロック平行移動していない。
/// - メロディは撥弦を排し、レガートな2opFMエレピ（`add_lead_note`）に統一。
///   各音符はパッドと同じ声部連結済みボイシングから選び、G4〜C5の狭い帯域
///   （`fold_into_range`）に折り畳むことで、実効音域上限 `MELODY_CAP_HZ` を
///   守りながら滑らかな輪郭になる。
/// - パッドはコード区間（2小節）の頭から attack/release を長く取って発音し、
///   ビブラート・ゆっくりしたフィルタ/音量の揺らぎ・コーラスで常に音色が
///   動き続ける（完全に静的な音を作らない）。
/// - 全曲にリバーブを適用し「反射のない孤立音」を無くしている。
pub fn render_main() -> Stereo {
    const BPM: f32 = 90.0;
    let bar = bar_secs(BPM);
    let beat = bar / 4.0;
    let bars_per_pass = 8; // 4 コード × 2 小節
    let total_bars = bars_per_pass * 2;
    let loop_secs = total_bars as f32 * bar;
    let delay_base = beat * 0.75; // 付点8分

    let progression = [
        Chord {
            semitone_offset: -3,
            quality: ChordQ::Min7,
        }, // Am7
        Chord {
            semitone_offset: -7,
            quality: ChordQ::Maj7,
        }, // Fmaj7
        Chord {
            semitone_offset: 0,
            quality: ChordQ::Maj7,
        }, // Cmaj7
        Chord {
            semitone_offset: -5,
            quality: ChordQ::Dom7,
        }, // G7
    ];

    // 声部連結: 4声のパッドボイシングを共通音保持・最小移動・転回形で接続。
    let mut voicings: Vec<Vec<i32>> = Vec::new();
    let mut prev: Option<Vec<i32>> = None;
    for chord in progression.iter() {
        let v = voice_lead(prev.as_deref(), chord.semitone_offset, chord.quality, 4, 4);
        prev = Some(v.clone());
        voicings.push(v);
    }

    let mel_lo = MELODY_CAP_HZ * 0.75; // ~G4
    let mel_pattern_a = [0usize, 1, 2, 3, 0, 1, 2, 3];
    let mel_pattern_b = [3usize, 2, 1, 0, 1, 2, 3, 0];

    let reverb = ReverbParams {
        feedback: 0.84,
        damp: 0.32,
        wet: 0.26,
    };

    render_looped(
        loop_secs,
        3.6,
        delay_base,
        0.30,
        0.22,
        reverb,
        -15.0,
        move |buf, sr| {
            let srf = sr as f32;
            let mut human = Rng::new(0x4841_4E44);
            for bar_idx in 0..total_bars {
                let chord_span_bars = 2usize;
                let ci = (bar_idx / chord_span_bars) % progression.len();
                let chord = &progression[ci];
                let voicing = &voicings[ci];
                let pad_tones: Vec<f32> = voicing.iter().map(|&s| degree_freq(C4, s)).collect();
                let root = degree_freq(C4, chord.semitone_offset);

                // パッドはコード区間（2小節）の頭でだけ発音するが、attack/release を
                // 長く取ることで次のコードの発音とクロスフェードし、常時鳴り続ける。
                if bar_idx % chord_span_bars == 0 {
                    let start_s = (bar_idx as f32 * bar * srf) as usize;
                    let len_s = (chord_span_bars as f32 * bar * srf) as usize;
                    add_pad(
                        buf,
                        sr,
                        start_s,
                        len_s,
                        &pad_tones,
                        0.60,
                        1200.0,
                        0.9,
                        1.2,
                        0x9001 + bar_idx as u64,
                    );
                }

                // ベース: 小節頭にルート音（1オクターブ下）。ヒューマナイズ付き。
                let t_off = human.next_f32() * 0.010;
                let lvl = 1.0 + human.next_f32() * 0.05;
                let pitch = 2f32.powf(human.next_f32() * 3.0 / 1200.0);
                let bass_freq = root * 0.5 * pitch;
                let bass_adsr = Adsr {
                    a: 0.015,
                    d: 0.15,
                    s: 0.3,
                    r: 0.35,
                };
                let bass = synth_bass_note(bass_freq, bar * 0.9, sr, bass_adsr, 1400.0);
                let bass_start = ((bar_idx as f32 * bar + t_off).max(0.0) * srf) as usize;
                add_mono_track(buf, bass_start, &bass, 0.5 * lvl, 0.0);

                // メロディ: 4分音符刻みのレガートなFMエレピ。1周目/2周目で異なる
                // 輪郭を使うが、どちらも声部連結由来の音から選ぶので滑らかに動く。
                let second_pass = bar_idx >= bars_per_pass;
                let pattern = if second_pass {
                    &mel_pattern_b
                } else {
                    &mel_pattern_a
                };
                for beat_i in 0..4usize {
                    let idx = pattern[beat_i % pattern.len()];
                    let raw = pad_tones[idx % pad_tones.len()];
                    let freq = fold_into_range(raw, mel_lo, MELODY_CAP_HZ);
                    let jt = human.next_f32() * 0.010;
                    let jl = 1.0 + human.next_f32() * 0.05;
                    let start_s = ((bar_idx as f32 * bar + beat_i as f32 * beat + jt).max(0.0)
                        * srf) as usize;
                    let hold = beat * 1.05; // わずかにオーバーラップさせレガートに
                    let pan = if beat_i % 2 == 0 { -0.28 } else { 0.28 };
                    add_lead_note(
                        buf,
                        sr,
                        start_s,
                        freq,
                        hold,
                        0.30 * jl,
                        pan,
                        0xA000 + (bar_idx * 4 + beat_i) as u64,
                    );
                }
            }
        },
    )
}

// ------------------------------------------------------------
// alt_calm.wav: 静かめの曲
// ------------------------------------------------------------

/// 静かめの曲。パッド + スローなレガート FM エレピメロディが中心。
///
/// メロディが疎（1コードにつき2音だけ）なぶん、伴奏無しで孤立した音が
/// 目立つと警告音的に聞こえやすい。そのためパッドを厚く・長い release で
/// 常時敷き詰めてメロディを支え、メロディ自体も声部連結済みボイシングから
/// 選び、狭い帯域に折り畳んで滑らかにしている。旧版の撥弦(KS)は全廃した。
pub fn render_alt_calm() -> Stereo {
    const BPM: f32 = 60.0;
    let bar = bar_secs(BPM);
    let beat = bar / 4.0;
    let chord_span_bars = 2usize;

    let progression = [
        Chord {
            semitone_offset: 0,
            quality: ChordQ::Maj7,
        }, // Cmaj7
        Chord {
            semitone_offset: -3,
            quality: ChordQ::Min7,
        }, // Am7
        Chord {
            semitone_offset: -7,
            quality: ChordQ::Maj7,
        }, // Fmaj7
        Chord {
            semitone_offset: -5,
            quality: ChordQ::Dom7,
        }, // G7
        Chord {
            semitone_offset: 0,
            quality: ChordQ::Maj7,
        }, // Cmaj7 (ターンアラウンド)
    ];
    let total_bars = progression.len() * chord_span_bars; // 10小節 = 40秒
    let loop_secs = total_bars as f32 * bar;
    let delay_base = beat * 0.75;

    let mut voicings: Vec<Vec<i32>> = Vec::new();
    let mut prev: Option<Vec<i32>> = None;
    for chord in progression.iter() {
        let v = voice_lead(prev.as_deref(), chord.semitone_offset, chord.quality, 4, 2);
        prev = Some(v.clone());
        voicings.push(v);
    }

    let mel_lo = MELODY_CAP_HZ * 0.7;
    // リバーブ/ディレイの wet を 0.34/0.20→0.28/0.16 に微減（3周目の調整）:
    // メロディ hold 延長（ダイナミクス平滑化）でクレストファクタが 12.0dB 前後
    // まで下がり実曲下限(12.3dB)を割ったため、常時鳴るウェット成分の比率を
    // 下げて波形のピーク/平均コントラストを実曲レンジへ戻す。残響そのものは
    // 維持する（「反射のない孤立音」対策のリバーブ適用は変えない）。
    let reverb = ReverbParams {
        feedback: 0.90,
        damp: 0.24,
        wet: 0.28,
    };

    // マスター RMS 目標 -16.0→-17.5dBFS（3周目の調整、ゲート -20〜-12 内）:
    // マスターの tanh ソフトクリップはレベルが高いほどピークを強く潰すため、
    // 静かめの曲であるこの曲は目標レベル自体を下げてピークの潰れを減らし、
    // クレストファクタを実曲レンジ（12.3dB以上）へ引き上げる。
    render_looped(
        loop_secs,
        3.8,
        delay_base,
        0.28,
        0.16,
        reverb,
        -17.5,
        move |buf, sr| {
            let srf = sr as f32;
            let mut human = Rng::new(0x4341_4C4D);
            for (ci, _chord) in progression.iter().enumerate() {
                let voicing = &voicings[ci];
                let tones: Vec<f32> = voicing.iter().map(|&s| degree_freq(C4, s)).collect();
                let start_s = (ci * chord_span_bars) as f32 * bar * srf;

                // パッドを厚めに（音量アップ + release を伸ばして次のコードとクロス
                // フェード気味にする）。疎なメロディを常時支える土台にする。
                add_pad(
                    buf,
                    sr,
                    start_s as usize,
                    (chord_span_bars as f32 * bar * srf) as usize,
                    &tones,
                    0.62,
                    1000.0,
                    1.1,
                    1.5,
                    0x1001 + ci as u64,
                );

                // スローなレガートFMエレピメロディ: 拍1と拍3あたりに長めの音符。
                // 声部連結済みボイシングの上声/内声から選び、狭い帯域へ折り畳む。
                let hi = tones[tones.len() - 1];
                let mid = tones[(tones.len() / 2).min(tones.len() - 1)];

                // リファレンス駆動のダイナミクス平滑化（docs/bgm-design-notes.md §5）:
                // 実曲群の1秒窓RMS標準偏差は 0.5〜3.1dB だが、旧設定（hold=2.3秒・
                // mel2 開始=2.2拍）はコード区間8秒の後半3秒がパッドのみになり
                // 3.5dB と起伏過多だった。音符を長く（hold 2.3→3.4秒）し、mel2 の
                // 開始を区間後半（2.2→3.6拍）へ移して、区間全体をメロディが途切れず
                // 覆うようにする（音量の1秒スケールの谷を無くす）。
                let mel1 = fold_into_range(hi, mel_lo, MELODY_CAP_HZ);
                let t1 = human.next_f32() * 0.010;
                let l1 = 1.0 + human.next_f32() * 0.05;
                let s1 = (ci * chord_span_bars) as f32 * bar + t1;
                // メロディ2音の音量差を付ける（0.42/0.38→0.50/0.32、3周目の調整）:
                // hold 延長でクレストファクタが実曲下限(12.3dB)を下回ったため、
                // 「強拍側を強く・弱拍側を弱く」して平均音量を保ったままピークを
                // 引き上げる（音楽的にもフレーズの起伏として自然な方向）。
                // 注（うなり低減改修 §6.4）: 音量LFO/コーラス半減の副作用でクレストが
                // 11.7dB と実曲下限(12.3dB)をわずかに割るが、強弱差の深掘り
                // (0.58/0.27) やマスターレベル低下(-18.5dB)での回復を試した結果、
                // どちらも 0.3〜8Hz 帯域の音量変動＝うなり深度そのものを押し戻して
                // 本改修の目的と相反した（クレストとAM深度は物理的にトレードオフ）。
                // ユーザーの聴感（うなり過多）を優先し、0.6dB のクレスト不足は許容する。
                add_lead_note(
                    buf,
                    sr,
                    (s1.max(0.0) * srf) as usize,
                    mel1,
                    3.4,
                    0.50 * l1,
                    -0.18,
                    0x1100 + ci as u64 * 2,
                );

                let mel2 = fold_into_range(mid, mel_lo, MELODY_CAP_HZ);
                let t2 = human.next_f32() * 0.010;
                let l2 = 1.0 + human.next_f32() * 0.05;
                let s2 = (ci * chord_span_bars) as f32 * bar + beat * 3.6 + t2;
                add_lead_note(
                    buf,
                    sr,
                    (s2.max(0.0) * srf) as usize,
                    mel2,
                    3.4,
                    0.32 * l2,
                    0.22,
                    0x1100 + ci as u64 * 2 + 1,
                );
            }
        },
    )
}

// ------------------------------------------------------------
// alt_tension.wav: 少し緊張感のある曲
// ------------------------------------------------------------

/// 少し緊張感のある短調の曲。低めベースのオスティナート + パッド。
/// 旋律/アルペジオ声部を持たず、和音は三和音どまりなので実効音域は元々
/// `MELODY_CAP_HZ` 以下に収まっているが、他の2曲と同じ「静的にしない・
/// 孤立させない」原則に合わせ、パッドに声部連結・ビブラート・コーラス・
/// リバーブを適用し、ベースにもヒューマナイズを加えた。
pub fn render_alt_tension() -> Stereo {
    const BPM: f32 = 100.0;
    let bar = bar_secs(BPM);
    let beat = bar / 4.0;
    let eighth = beat / 2.0;
    let chord_span_bars = 4usize;

    // Dm(短調) - Bb - F - C の疾走感あるオスティナート進行
    let progression = [
        Chord {
            semitone_offset: -10,
            quality: ChordQ::Min,
        }, // Dm
        Chord {
            semitone_offset: -2,
            quality: ChordQ::Maj,
        }, // Bb
        Chord {
            semitone_offset: -7,
            quality: ChordQ::Maj,
        }, // F
        Chord {
            semitone_offset: 0,
            quality: ChordQ::Maj,
        }, // C
    ];
    let total_bars = progression.len() * chord_span_bars; // 16小節 = 38.4秒
    let loop_secs = total_bars as f32 * bar;
    let delay_base = beat * 0.75;

    let mut voicings: Vec<Vec<i32>> = Vec::new();
    let mut prev: Option<Vec<i32>> = None;
    for chord in progression.iter() {
        let v = voice_lead(prev.as_deref(), chord.semitone_offset, chord.quality, 3, 2);
        prev = Some(v.clone());
        voicings.push(v);
    }

    // ベースオスティナート（8分×8）: ルート中心に時々5度を混ぜて動きを出す
    let ostinato = [0usize, 0, 1, 0, 0, 0, 1, 0]; // 0=ルート, 1=5度
    let reverb = ReverbParams {
        feedback: 0.78,
        damp: 0.38,
        wet: 0.22,
    };

    render_looped(
        loop_secs,
        3.0,
        delay_base,
        0.26,
        0.18,
        reverb,
        -16.0,
        move |buf, sr| {
            let srf = sr as f32;
            let mut human = Rng::new(0x5445_4E53);
            for (ci, chord) in progression.iter().enumerate() {
                let voicing = &voicings[ci];
                let tones: Vec<f32> = voicing.iter().map(|&s| degree_freq(C4, s)).collect();
                let root = degree_freq(C4, chord.semitone_offset);
                let bar0 = ci * chord_span_bars;

                // パッド: コード区間の大部分（3.2小節）を鳴らし、長い release で
                // 区間の残りにも減衰しながら鳴り続けさせる（無音の隙間を作らない）。
                //
                // リファレンス駆動の帯域再設計（docs/bgm-design-notes.md §5）:
                // 実曲群は低域(<250Hz) 2〜70% / 中域(250-2k) 29〜98% に収まるのに
                // 対し、旧設定（パッド0.46/LPF850Hz/1.8小節 + ベース0.45）は低域95%
                // と極端な低域偏重で、スペクトル重心も152Hzと実曲下限(235Hz)未満
                // だった。パッドを明るく(LPF 850→1500Hz)・強く(0.46→0.60)・長く
                // (1.8→3.2小節)し、さらに1オクターブ上の薄いダブリングで中域の
                // 倍音を足して、帯域バランスと重心を実曲レンジへ引き上げる。
                add_pad(
                    buf,
                    sr,
                    (bar0 as f32 * bar * srf) as usize,
                    (3.2 * bar * srf) as usize,
                    &tones,
                    0.60,
                    1500.0,
                    1.3,
                    1.5,
                    0x2001 + ci as u64,
                );
                // 1オクターブ上の薄いダブリング（「こもった電子音」対策の中高域
                // 補強。主役はあくまで元のパッドなので音量は控えめ）。
                let tones_8va: Vec<f32> = tones.iter().map(|&f| f * 2.0).collect();
                add_pad(
                    buf,
                    sr,
                    (bar0 as f32 * bar * srf) as usize,
                    (3.2 * bar * srf) as usize,
                    &tones_8va,
                    0.22,
                    2200.0,
                    1.6,
                    1.5,
                    0x2101 + ci as u64,
                );

                // 低めベースオスティナート: コード区間全体（4小節）に渡って刻む
                let bass_root = root * 0.5;
                let bass_fifth = tones.get(2).copied().unwrap_or(tones[0]) * 0.5;
                // attack はオスティナートのリズム感を保ちつつ、500Hz以上への
                // スペクトル漏れ（速すぎるアタックのAMサイドバンド）を抑えるため
                // 0.008s→0.06s に伸ばした（トランジェント検出ゲート対応）。
                let bass_adsr = Adsr {
                    a: 0.06,
                    d: 0.08,
                    s: 0.28,
                    r: 0.18,
                };
                for b in 0..chord_span_bars {
                    for (step, &which) in ostinato.iter().enumerate() {
                        let base_freq = if which == 0 { bass_root } else { bass_fifth };
                        let t_off = human.next_f32() * 0.010;
                        let lvl = 1.0 + human.next_f32() * 0.05;
                        let pitch = 2f32.powf(human.next_f32() * 2.0 / 1200.0);
                        // カットオフを650Hzまで下げ、速いオスティナート刻みでも
                        // 500Hz超のトランジェント検出帯へ漏れる高域成分を抑える。
                        let note =
                            synth_bass_note(base_freq * pitch, eighth * 0.85, sr, bass_adsr, 650.0);
                        let t = (bar0 + b) as f32 * bar + step as f32 * eighth + t_off;
                        // 0.45→0.28: 低域偏重（実測 low 95% vs 実曲 ≤70%）の主因だった
                        // ベースの比重を下げる（リズムの牽引役は維持する）。0.30 では
                        // 中域比 29.3% と実曲下限(29.4%)に紙一重で届かなかったため
                        // 2周目の調整でもう一段下げた。
                        add_mono_track(buf, (t.max(0.0) * srf) as usize, &note, 0.28 * lvl, 0.0);
                    }
                }
            }
        },
    )
}

// ------------------------------------------------------------
// SE: KS プラッククリック
// ------------------------------------------------------------

/// 短い KS プラック SE（モノラル）。UI クリックのような単発効果音は
/// 「短い減衰音」であること自体が目的（ボタンを押した確認音）なので、
/// BGM 本体とは異なりここでは撥弦を使ってよい。
pub fn render_click() -> Vec<f32> {
    let mut s = karplus_strong(1400.0, 0.14, SR, 0.994, 6500.0, 0xC1C4);
    let n = s.len();
    for (i, v) in s.iter_mut().enumerate() {
        let t = i as f32 / n as f32;
        *v *= (1.0 - t).powf(1.5);
    }
    // DC オフセット除去（減衰ノイズの平均は理論上ゼロに近いが、有限長では
    // わずかに偏るので明示的に引く）
    let mean: f32 = s.iter().sum::<f32>() / n as f32;
    for v in s.iter_mut() {
        *v -= mean;
    }
    let pk = s.iter().fold(0f32, |m, &x| m.max(x.abs()));
    if pk > 1e-6 {
        let g = 0.55 / pk;
        for v in s.iter_mut() {
            *v *= g;
        }
    }
    s
}

pub mod demo;
