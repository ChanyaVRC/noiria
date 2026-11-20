//! デモ BGM 用の楽器（採用済み BGM の 2op FM + デチューン saw パッドとは別系統）。
//!
//! - `piano_note`: 加算合成のアコースティックピアノ。弦の非調和性（高次ほど伸びる
//!   部分音）、音域で変わる減衰時間、高次ほど速く消える二段減衰、打鍵位置による
//!   部分音の間引き、ベロシティで変わる明るさ、低次部分音だけの 2 本弦のうなり、
//!   短い打鍵ノイズを持つ。
//! - `add_strings_run`: ストリングス・アンサンブル。3 本のデチューン saw（±6 セント、
//!   polyBLEP でエイリアス無し）、声部ごとの遅れビブラート、遅いアタック、
//!   緩いローパスとフォルマント風のピーキング EQ。
//!
//! ピアノの部分音は f64 の複素位相子（毎サンプル回転）で発振し、sin を呼ばない。

use super::super::*;

/// ストリングスの 3 本 saw のデチューン幅（セント、中心から片側）。濁らない範囲（±5〜8）に収める。
pub const STRINGS_DETUNE_CENTS: f32 = 6.0;

/// 単音のピアノ（モノラル）。`hold` 秒でキーを離し、`release` 秒（e 倍時定数）で
/// ダンパーが効く。長さは `hold + 6*release` 秒。
pub fn piano_note(sr: u32, freq: f32, vel: f32, hold: f32, release: f32, seed: u64) -> Vec<f32> {
    let srf = sr as f64;
    let vel = vel.clamp(0.05, 1.0);
    let mut rng = Rng::new(seed);
    let midi = 69.0 + 12.0 * (freq / 440.0).log2();
    let b_inh = 2.0e-4 * 2f32.powf((midi - 48.0) / 18.0);
    // 低音ほど長く、高音ほど短く鳴る（基音の e 倍時定数 [秒]）。
    let tau_f = (2.4 * 2f32.powf(-(midi - 60.0) / 20.0)).clamp(0.6, 7.0);
    let total = ((hold + 6.0 * release) * sr as f32) as usize + 8;
    let hold_n = (hold * sr as f32) as usize;
    let attack_n = ((0.003 + (1.0 - vel) * 0.006) * sr as f32) as usize;
    let rel_d = (-1.0 / (release as f64 * srf)).exp();
    let p = 1.05 + (1.0 - vel) * 1.0;
    let fc = 900.0 + 2200.0 * vel;
    let mut out = vec![0f32; total];

    for k in 1..=16usize {
        let kf = k as f32;
        let fk = kf * freq * (1.0 + b_inh * kf * kf).sqrt();
        if fk > 4000.0 || fk > srf as f32 * 0.45 {
            break;
        }
        // 打鍵位置（弦長の約 1/8.7）の節にあたる部分音を弱める。
        let comb = 0.55 + 0.45 * (PI * kf * 0.115).sin().abs();
        let lp = 1.0 / (1.0 + (fk / fc).powi(2));
        let amp = comb * lp / kf.powf(p);
        if amp < 0.004 {
            continue;
        }
        let tau_k = tau_f / (1.0 + 0.45 * (kf - 1.0));
        let d_fast = (-1.0 / (0.3 * tau_k as f64 * srf)).exp();
        let d_slow = (-1.0 / (tau_k as f64 * srf)).exp();
        let strings = if k <= 4 { 2 } else { 1 };
        for s in 0..strings {
            let cents = if strings == 2 {
                (if s == 0 { -1.0 } else { 1.0 }) * (0.4 + 0.5 * rng.next_f32().abs())
            } else {
                0.0
            };
            let fs = fk as f64 * 2f64.powf(cents as f64 / 1200.0);
            let w = std::f64::consts::TAU * fs / srf;
            let (rs, rc) = w.sin_cos();
            let ph0 = (rng.next_f32() * 0.15) as f64;
            let (mut zs, mut zc) = ph0.sin_cos();
            let mut e_fast = 0.55f64;
            let mut e_slow = 0.45f64;
            let mut rel = 1.0f64;
            let g = (amp / strings as f32) as f64;
            for (i, o) in out.iter_mut().enumerate() {
                let env = e_fast + e_slow;
                if i >= hold_n {
                    rel *= rel_d;
                }
                let att = if i < attack_n {
                    i as f64 / attack_n as f64
                } else {
                    1.0
                };
                let e = env * rel;
                if e < 1.0e-4 && i > attack_n {
                    break;
                }
                *o += (zs * g * e * att) as f32;
                let ns = zs * rc + zc * rs;
                let nc = zc * rc - zs * rs;
                zs = ns;
                zc = nc;
                e_fast *= d_fast;
                e_slow *= d_slow;
            }
        }
    }

    // 打鍵ノイズ: 数 ms の低域寄りノイズ。ベロシティが強いほど少し硬く大きい。
    let noise_n = (0.007 * sr as f32) as usize;
    let a = (-TAU * (900.0 + 2200.0 * vel) / sr as f32).exp();
    let mut z = 0.0f32;
    for i in 0..noise_n.min(out.len()) {
        let x = rng.next_f32();
        z = x * (1.0 - a) + z * a;
        let env = (1.0 - i as f32 / noise_n as f32).powi(2);
        out[i] += z * env * 0.05 * vel * vel;
    }

    let gain = 0.55 * vel.powf(1.3);
    for v in out.iter_mut() {
        *v *= gain;
    }
    out
}

/// RBJ ピーキング EQ（フォルマント風のゆるい山を作る）。
struct Peaking {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}
impl Peaking {
    fn new(sr: f32, f0: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = TAU * f0 / sr;
        let alpha = w0.sin() / (2.0 * q);
        let cw = w0.cos();
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cw) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cw) / a0,
            a2: (1.0 - alpha / a) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

#[inline]
fn polyblep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

#[inline]
fn blep_saw(phase: f32, dt: f32) -> f32 {
    2.0 * phase - 1.0 - polyblep(phase, dt)
}

#[inline]
fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// ストリングスの 1 声部を `start_s` から `hold` 秒保持して加算する。
/// `attack` / `release` は秒。声部ごとにシードでビブラート速度・遅れ・位相が違う。
#[allow(clippy::too_many_arguments)]
pub fn add_strings_run(
    buf: &mut Stereo,
    sr: u32,
    start_s: usize,
    freq: f32,
    hold: f32,
    attack: f32,
    release: f32,
    level: f32,
    seed: u64,
) {
    let srf = sr as f32;
    let mut rng = Rng::new(seed);
    let total = ((hold + release) * srf) as usize + 8;
    let vib_rate = 5.0 + rng.next_f32().abs() * 0.8;
    let vib_delay = 0.5 + rng.next_f32().abs() * 0.4;
    let vib_ph = rng.next_f32().abs();
    let swell_ph = rng.next_f32().abs();
    let detune = [
        -STRINGS_DETUNE_CENTS + rng.next_f32() * 0.5,
        rng.next_f32() * 0.5,
        STRINGS_DETUNE_CENTS + rng.next_f32() * 0.5,
    ];
    let mut ph = [
        rng.next_f32().abs(),
        rng.next_f32().abs(),
        rng.next_f32().abs(),
    ];
    let wl = [0.85f32, 0.6, 0.25];
    let wr = [0.25f32, 0.6, 0.85];
    let mut lp_l = [Lpf::new(2000.0, srf), Lpf::new(2000.0, srf)];
    let mut lp_r = [Lpf::new(2000.0, srf), Lpf::new(2000.0, srf)];
    let mut eq_l = Peaking::new(srf, 650.0, 0.9, 2.5);
    let mut eq_r = Peaking::new(srf, 650.0, 0.9, 2.5);
    let g = level * db_to_lin(-3.0);
    for i in 0..total {
        let t = i as f32 / srf;
        let env_a = smoothstep(t / attack.max(1e-3));
        let env_r = if t > hold {
            1.0 - smoothstep((t - hold) / release.max(1e-3))
        } else {
            1.0
        };
        let env = env_a * env_r;
        let vfade = (t - vib_delay).clamp(0.0, 1.0);
        let vib = 2f32.powf(3.5 * vfade * (TAU * (vib_rate * t + vib_ph)).sin() / 1200.0);
        let swell = 1.0 + 0.04 * (TAU * (0.09 * t + swell_ph)).sin();
        let mut sl = 0.0;
        let mut sr_ = 0.0;
        for v in 0..3 {
            let f = freq * vib * 2f32.powf(detune[v] / 1200.0);
            let dt = f / srf;
            ph[v] += dt;
            ph[v] -= ph[v].floor();
            let s = blep_saw(ph[v], dt);
            sl += s * wl[v];
            sr_ += s * wr[v];
        }
        let mut l = sl;
        let mut r = sr_;
        for f in lp_l.iter_mut() {
            l = f.process(l);
        }
        for f in lp_r.iter_mut() {
            r = f.process(r);
        }
        l = eq_l.process(l);
        r = eq_r.process(r);
        buf.add_stereo_at(start_s + i, l * env * swell * g, r * env * swell * g);
    }
}
