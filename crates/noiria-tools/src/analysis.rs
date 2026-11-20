//! BGM 数値解析の共通処理（`examples/analyze_bgm.rs` から使う）。
//!
//! リファレンス曲（実制作の CC0 BGM）と自作 3 曲を同じ指標で数値化し、差分を
//! 測るための土台。音の再生は一切行わない。新規クレート追加を避けるため、
//! FFT も radix-2 Cooley-Tukey を自作している（依存: `std` のみ）。

use std::f32::consts::PI;

use crate::synth::{self, Stereo};

// ============================================================
// FFT（radix-2 Cooley-Tukey、in-place、自作）
// ============================================================

#[derive(Clone, Copy, Default)]
pub struct Complex {
    pub re: f32,
    pub im: f32,
}

#[inline]
fn cmul(a: Complex, b: Complex) -> Complex {
    Complex {
        re: a.re * b.re - a.im * b.im,
        im: a.re * b.im + a.im * b.re,
    }
}
#[inline]
fn cadd(a: Complex, b: Complex) -> Complex {
    Complex {
        re: a.re + b.re,
        im: a.im + b.im,
    }
}
#[inline]
fn csub(a: Complex, b: Complex) -> Complex {
    Complex {
        re: a.re - b.re,
        im: a.im - b.im,
    }
}

/// `n`（`buf.len()`）が2の冪であることを前提にした in-place FFT。
/// 順変換のみ（逆変換は提供しない）。ビット反転並べ替え→バタフライ演算という定番構成。
pub fn fft(buf: &mut [Complex]) {
    let n = buf.len();
    if n <= 1 {
        return;
    }
    debug_assert!(n.is_power_of_two(), "FFT長は2の冪である必要がある (n={n})");

    // ビット反転並べ替え
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            buf.swap(i, j);
        }
    }

    // バタフライ演算（段階ごとに長さを倍にしていく）
    let mut len = 2usize;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let wlen = Complex {
            re: ang.cos(),
            im: ang.sin(),
        };
        let mut i = 0usize;
        while i < n {
            let mut w = Complex { re: 1.0, im: 0.0 };
            for k in 0..len / 2 {
                let u = buf[i + k];
                let v = cmul(buf[i + k + len / 2], w);
                buf[i + k] = cadd(u, v);
                buf[i + k + len / 2] = csub(u, v);
                w = cmul(w, wlen);
            }
            i += len;
        }
        len <<= 1;
    }
}

/// Hann窓（サイズ`n`）。
pub fn hann_window(n: usize) -> Vec<f32> {
    if n <= 1 {
        return vec![1.0; n];
    }
    (0..n)
        .map(|i| 0.5 - 0.5 * (TAU * i as f32 / (n - 1) as f32).cos())
        .collect()
}
const TAU: f32 = 2.0 * PI;

// ============================================================
// 曲全体の指標
// ============================================================

pub struct Metrics {
    pub name: String,
    pub secs: f32,
    pub sr: u32,
    pub peak: f32,
    pub rms_dbfs: f32,
    pub crest_db: f32,
    /// 低(<250Hz)/中(250-2kHz)/高(>2kHz) のエネルギー比率（0..1、合計1）。
    pub band_low: f32,
    pub band_mid: f32,
    pub band_high: f32,
    /// スペクトル重心（フレームごと）の平均と標準偏差 [Hz]。
    pub centroid_mean_hz: f32,
    pub centroid_std_hz: f32,
    /// 1秒あたりのオンセット数（全帯域スペクトルフラックスによる検出）。
    pub onset_density_per_sec: f32,
    /// 1秒窓RMS（dBFS）の標準偏差。値が大きいほど音量の起伏が大きい。
    pub dyn_rms_std_db: f32,
    /// L/R ピアソン相関（-1..1）。1に近いほどモノラルに近い。
    pub stereo_corr: f32,
    /// オンセット後 300ms のエネルギー減衰スロープ平均 [dB/秒]（負値。0に近いほど
    /// 減衰が緩やか=ウェット、大きく負ほど急峻=ドライ）。オンセットが検出できない
    /// 場合は `None`。
    pub decay_slope_db_per_s: Option<f32>,
    /// うなり（振幅変調=AM）深度。0.3〜8Hz帯域の変調指数 m=(max-min)/(max+min)
    /// の中央値（曲全体の平均的なうなり深さ）。算出方法は `measure_am_depth` 参照。
    pub am_depth_median: f32,
    /// 同・90パーセンタイル（持続和音などで局所的に最も深くうなる瞬間の深さ。
    /// レンジ超過の判定はこちらを基準にする — 詳細は `measure_am_depth` 参照）。
    pub am_depth_p90: f32,
}

const FRAME: usize = 2048;
const HOP: usize = 512;
const BAND_LOW_HZ: f32 = 250.0;
const BAND_HIGH_HZ: f32 = 2000.0;

fn to_mono(buf: &Stereo) -> Vec<f32> {
    buf.l
        .iter()
        .zip(buf.r.iter())
        .map(|(&a, &b)| 0.5 * (a + b))
        .collect()
}

fn db(x: f32) -> f32 {
    synth::dbfs(x)
}

/// STFTを1回走らせ、帯域エネルギー・スペクトル重心系列・スペクトルフラックス系列を
/// まとめて計算する（3つの指標で同じフレーム分割を再利用するため）。
struct Stft {
    centroids: Vec<f32>,
    flux: Vec<f32>,
    frame_times: Vec<f32>,
    band_low: f64,
    band_mid: f64,
    band_high: f64,
}

fn run_stft(mono: &[f32], sr: f32) -> Stft {
    let window = hann_window(FRAME);
    let bin_hz = sr / FRAME as f32;
    let half = FRAME / 2;

    let mut centroids = Vec::new();
    let mut flux = Vec::new();
    let mut frame_times = Vec::new();
    let mut band_low = 0f64;
    let mut band_mid = 0f64;
    let mut band_high = 0f64;
    let mut prev_mag: Option<Vec<f32>> = None;

    let mut i = 0usize;
    while i + FRAME <= mono.len() {
        let mut c: Vec<Complex> = (0..FRAME)
            .map(|k| Complex {
                re: mono[i + k] * window[k],
                im: 0.0,
            })
            .collect();
        fft(&mut c);
        let mag: Vec<f32> = c[..half]
            .iter()
            .map(|z| (z.re * z.re + z.im * z.im).sqrt())
            .collect();

        let mut l = 0f64;
        let mut m = 0f64;
        let mut h = 0f64;
        let mut wsum = 0f64;
        let mut fsum = 0f64;
        for (k, &mg) in mag.iter().enumerate() {
            let f = k as f32 * bin_hz;
            let e = (mg as f64) * (mg as f64);
            if f < BAND_LOW_HZ {
                l += e;
            } else if f < BAND_HIGH_HZ {
                m += e;
            } else {
                h += e;
            }
            wsum += e;
            fsum += f as f64 * e;
        }
        band_low += l;
        band_mid += m;
        band_high += h;
        if wsum > 1e-9 {
            centroids.push((fsum / wsum) as f32);
        }

        if let Some(pm) = &prev_mag {
            let fl: f32 = mag
                .iter()
                .zip(pm.iter())
                .map(|(&a, &b)| (a - b).max(0.0))
                .sum();
            flux.push(fl);
            frame_times.push(i as f32 / sr);
        }
        prev_mag = Some(mag);
        i += HOP;
    }

    Stft {
        centroids,
        flux,
        frame_times,
        band_low,
        band_mid,
        band_high,
    }
}

fn mean(xs: &[f32]) -> f32 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f32>() / xs.len() as f32
    }
}
fn std_dev(xs: &[f32]) -> f32 {
    if xs.len() < 2 {
        return 0.0;
    }
    let m = mean(xs);
    let var = xs.iter().map(|&x| (x - m) * (x - m)).sum::<f32>() / xs.len() as f32;
    var.sqrt()
}

/// スペクトルフラックス系列からオンセットを検出する（適応閾値: 平均+1.5σ、
/// 最小間隔100ms）。汎用の記述的指標であり、`tests/audio_quality.rs` の
/// 「通知音っぽさ」ゲート用トランジェント検出器（500Hzハイパス+短窓dBジャンプ）とは
/// 別物（全帯域スペクトルフラックス方式）。
fn onset_indices(flux: &[f32], frame_times: &[f32], min_gap_secs: f32) -> Vec<usize> {
    if flux.len() < 3 {
        return Vec::new();
    }
    let threshold = mean(flux) + 1.5 * std_dev(flux);
    let mut onsets = Vec::new();
    let mut last_t = f32::NEG_INFINITY;
    for i in 1..flux.len() - 1 {
        let v = flux[i];
        if v > threshold && v >= flux[i - 1] && v >= flux[i + 1] {
            let t = frame_times[i];
            if t - last_t >= min_gap_secs {
                onsets.push(i);
                last_t = t;
            }
        }
    }
    onsets
}

fn count_onsets(flux: &[f32], frame_times: &[f32], min_gap_secs: f32) -> usize {
    onset_indices(flux, frame_times, min_gap_secs).len()
}

/// オンセット後 300ms のエネルギー減衰スロープ（dB/秒）を、20ms窓のRMSに対する
/// 最小二乗直線の傾きとして推定し、全オンセットで平均する。
fn decay_slope(mono: &[f32], sr: f32, flux: &[f32], frame_times: &[f32]) -> Option<f32> {
    let sub_win = (0.02 * sr) as usize; // 20ms
    let span = (0.3 * sr) as usize; // 300ms
    if sub_win == 0 {
        return None;
    }

    let mut slopes = Vec::new();
    for i in onset_indices(flux, frame_times, 0.1) {
        let t = frame_times[i];
        let start = (t * sr) as usize;
        if start + span > mono.len() {
            continue;
        }
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        let n_sub = span / sub_win;
        for w in 0..n_sub {
            let s0 = start + w * sub_win;
            let s1 = s0 + sub_win;
            let seg = &mono[s0..s1];
            let rms = (seg.iter().map(|&v| v * v).sum::<f32>() / seg.len() as f32).sqrt();
            if rms > 1e-7 {
                xs.push(w as f32 * 0.02);
                ys.push(db(rms));
            }
        }
        if xs.len() < 4 {
            continue;
        }
        // 最小二乗直線の傾き
        let n = xs.len() as f32;
        let mx = xs.iter().sum::<f32>() / n;
        let my = ys.iter().sum::<f32>() / n;
        let mut num = 0f32;
        let mut den = 0f32;
        for (x, y) in xs.iter().zip(ys.iter()) {
            num += (x - mx) * (y - my);
            den += (x - mx) * (x - mx);
        }
        if den > 1e-9 {
            slopes.push(num / den);
        }
    }
    if slopes.is_empty() {
        None
    } else {
        Some(mean(&slopes))
    }
}

/// 1秒窓RMS(dBFS)の標準偏差（ダイナミクスの指標）。
fn dynamics_rms_std_db(mono: &[f32], sr: f32) -> f32 {
    let win = sr as usize;
    if win == 0 || mono.len() < win {
        return 0.0;
    }
    let mut dbs = Vec::new();
    let mut i = 0usize;
    while i + win <= mono.len() {
        let seg = &mono[i..i + win];
        let rms = (seg.iter().map(|&v| v * v).sum::<f32>() / seg.len() as f32).sqrt();
        if rms > 1e-8 {
            dbs.push(db(rms));
        }
        i += win;
    }
    std_dev(&dbs)
}

// ============================================================
// うなり（AM: 振幅変調）深度
// ============================================================
//
// ユーザー評価「うなりが深すぎる」を受けて追加した指標。デチューン/コーラス/
// 音量・フィルタLFOはいずれも音量（振幅）を周期的に揺らす=振幅変調(AM)として
// 現れるため、持続部の振幅包絡を取り出し、うなり/ラフネス知覚帯域とされる
// 0.3〜8Hz（フラクチュエーション強度〜ラフネス下端、`docs/bgm-design-notes.md`
// §1.1 参照）だけを抜き出して変調の深さを数値化する。
//
// 手順:
// 1. 50ms窓・25msホップのRMSで振幅包絡 `env` を作る（包絡のサンプルレートは
//    1/0.025s=40Hz。8Hz帯域を見るのに十分なナイキスト余裕がある）。
// 2. `env` に 1-poleローパス(0.3Hz)を掛けたものを「トレンド」（曲そのものの
//    強弱変化、0.3Hz未満の成分）として分離する。
// 3. `env - トレンド` = 0.3Hz以上の変動。これにさらに 1-poleローパス(8Hz)を
//    掛けて 0.3〜8Hz 帯域だけを残す（ハイパス→ローパスの縦続=バンドパス）。
// 4. トレンド + 帯域成分 を「関心帯域だけを残した包絡」として再構成し、
//    2.5秒窓（0.3Hzの1周期をまたぐ長さ）・1秒ホップでスライドさせながら、
//    各窓内の変調指数 m=(max-min)/(max+min) を求める（トレンドが全体ピーク
//    から-30dB以上低い、無音に近い窓=非持続部は除外する）。
// 5. 全窓の m から中央値（typical: 曲全体の平均的なうなり深さ）と90パーセン
//    タイル（peak: 最も深くうなる瞬間、持続和音などの局所区間）の両方を返す。
//    実測で分かったこと: 自作曲は「曲全体を通した中央値」ではリファレンス
//    レンジ内に収まるが、旋律が途切れて和音が伸びる区間だけ局所的に深く
//    うなり、その**局所ピーク**がレンジを超える（例: alt_calm 中央値0.46
//    <レンジ内 vs 局所p90/max 0.62〜0.68 >レンジ上限0.60）。「うなりが
//    深すぎる」という評価は当然こうした持続区間で気付くものなので、レンジ
//    超過の判定は中央値ではなく p90（peak）を基準にする。
//
// 1-poleフィルタは片方向（非ゼロ位相）だが、これは診断用の記述統計であり
// 位相特性を厳密に問わないため許容している。
const AM_ENV_WINDOW_SECS: f32 = 0.05;
const AM_ENV_HOP_SECS: f32 = 0.025;
const AM_LOW_HZ: f32 = 0.3;
const AM_HIGH_HZ: f32 = 8.0;
const AM_MEASURE_WIN_SECS: f32 = 2.5;
const AM_MEASURE_HOP_SECS: f32 = 1.0;
const AM_SILENCE_FLOOR_DB: f32 = -30.0;

/// 50ms窓・25msホップのRMS振幅包絡と、その包絡自体のサンプルレート[Hz]を返す。
fn amplitude_envelope(mono: &[f32], sr: f32) -> (Vec<f32>, f32) {
    let win_n = ((AM_ENV_WINDOW_SECS * sr) as usize).max(1);
    let hop_n = ((AM_ENV_HOP_SECS * sr) as usize).max(1);
    let mut env = Vec::new();
    let mut i = 0usize;
    while i + win_n <= mono.len() {
        let seg = &mono[i..i + win_n];
        let rms = (seg.iter().map(|&v| v * v).sum::<f32>() / seg.len() as f32).sqrt();
        env.push(rms);
        i += hop_n;
    }
    let env_sr = sr / hop_n as f32;
    (env, env_sr)
}

/// 1-poleローパスフィルタを系列 `x`（サンプルレート `seq_sr`）に順方向で適用する。
fn lowpass_1pole_seq(x: &[f32], cutoff_hz: f32, seq_sr: f32) -> Vec<f32> {
    if x.is_empty() {
        return Vec::new();
    }
    let a = (-TAU * cutoff_hz / seq_sr).exp();
    let mut z = x[0];
    x.iter()
        .map(|&v| {
            z = v * (1.0 - a) + z * a;
            z
        })
        .collect()
}

/// 帯域制限した振幅包絡から、測定窓（`AM_MEASURE_WIN_SECS`長・`AM_MEASURE_HOP_SECS`
/// ホップ）ごとの変調指数 m=(max-min)/(max+min) を求めて返す（無音に近い
/// 非持続部の窓は除外する）。
fn am_depth_windows(mono: &[f32], sr: f32) -> Vec<f32> {
    let (env, env_sr) = amplitude_envelope(mono, sr);
    if env.len() < 4 {
        return Vec::new();
    }
    let trend = lowpass_1pole_seq(&env, AM_LOW_HZ, env_sr);
    let highpassed: Vec<f32> = env.iter().zip(trend.iter()).map(|(&e, &t)| e - t).collect();
    let band = lowpass_1pole_seq(&highpassed, AM_HIGH_HZ, env_sr);
    let reconstructed: Vec<f32> = trend
        .iter()
        .zip(band.iter())
        .map(|(&t, &b)| (t + b).max(0.0))
        .collect();

    let peak_trend = trend.iter().cloned().fold(0f32, f32::max);
    if peak_trend <= 1e-9 {
        return Vec::new();
    }
    let floor_lin = peak_trend * synth::db_to_lin(AM_SILENCE_FLOOR_DB);

    let win_n = ((AM_MEASURE_WIN_SECS * env_sr) as usize).max(2);
    let hop_n = ((AM_MEASURE_HOP_SECS * env_sr) as usize).max(1);

    let mut ms: Vec<f32> = Vec::new();
    if reconstructed.len() >= win_n {
        let mut i = 0usize;
        while i + win_n <= reconstructed.len() {
            let seg = &reconstructed[i..i + win_n];
            let seg_trend = &trend[i..i + win_n];
            if mean(seg_trend) >= floor_lin {
                let mx = seg.iter().cloned().fold(f32::MIN, f32::max);
                let mn = seg.iter().cloned().fold(f32::MAX, f32::min);
                let denom = mx + mn;
                if denom > 1e-9 {
                    ms.push(((mx - mn) / denom).clamp(0.0, 1.0));
                }
            }
            i += hop_n;
        }
    }
    ms
}

fn median(xs: &mut [f32]) -> f32 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(|a, b| a.total_cmp(b));
    let mid = xs.len() / 2;
    if xs.len().is_multiple_of(2) {
        0.5 * (xs[mid - 1] + xs[mid])
    } else {
        xs[mid]
    }
}

/// `xs`（昇順ソート済みでなくてよい）の `p`（0..1）パーセンタイルを最近傍法で返す。
fn percentile(xs: &mut [f32], p: f32) -> f32 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(|a, b| a.total_cmp(b));
    let idx = ((xs.len() as f32 - 1.0) * p).round() as usize;
    xs[idx.min(xs.len() - 1)]
}

/// AM（振幅変調＝うなり）深度を測定し、(中央値, 90パーセンタイル) を返す。
/// 帯域内に有効な（=持続部の）測定窓が無い場合は (0.0, 0.0) を返す。
fn measure_am_depth(mono: &[f32], sr: f32) -> (f32, f32) {
    let mut ms = am_depth_windows(mono, sr);
    if ms.is_empty() {
        return (0.0, 0.0);
    }
    let p90 = percentile(&mut ms.clone(), 0.9);
    let med = median(&mut ms);
    (med, p90)
}

pub fn analyze_track(name: &str, buf: &Stereo) -> Metrics {
    let sr = buf.sr as f32;
    let mono = to_mono(buf);
    let peak = synth::combined_peak(buf);
    let rms = synth::combined_rms(buf);
    let rms_dbfs = db(rms);
    let crest_db = db(peak) - rms_dbfs;
    let stereo_corr = synth::correlation(&buf.l, &buf.r);

    let stft = run_stft(&mono, sr);
    let total_band = (stft.band_low + stft.band_mid + stft.band_high).max(1e-12);
    let band_low = (stft.band_low / total_band) as f32;
    let band_mid = (stft.band_mid / total_band) as f32;
    let band_high = (stft.band_high / total_band) as f32;

    let centroid_mean_hz = mean(&stft.centroids);
    let centroid_std_hz = std_dev(&stft.centroids);

    let onset_count = count_onsets(&stft.flux, &stft.frame_times, 0.1);
    let onset_density_per_sec = if buf.seconds() > 0.0 {
        onset_count as f32 / buf.seconds()
    } else {
        0.0
    };

    let dyn_rms_std_db = dynamics_rms_std_db(&mono, sr);
    let decay = decay_slope(&mono, sr, &stft.flux, &stft.frame_times);
    let (am_depth_median, am_depth_p90) = measure_am_depth(&mono, sr);

    Metrics {
        name: name.to_string(),
        secs: buf.seconds(),
        sr: buf.sr,
        peak,
        rms_dbfs,
        crest_db,
        band_low,
        band_mid,
        band_high,
        centroid_mean_hz,
        centroid_std_hz,
        onset_density_per_sec,
        dyn_rms_std_db,
        stereo_corr,
        decay_slope_db_per_s: decay,
        am_depth_median,
        am_depth_p90,
    }
}

/// `noiria_core` のデコーダ出力（インターリーブ f32、任意チャンネル数）を
/// `synth::Stereo` に変換する（モノラルはL=Rに複製、3ch以上は先頭2chのみ使用）。
pub fn interleaved_to_stereo(sr: u32, channels: u16, interleaved: &[f32]) -> Stereo {
    let ch = channels.max(1) as usize;
    let n = interleaved.len() / ch;
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    for i in 0..n {
        let base = i * ch;
        let lv = interleaved[base];
        let rv = if ch >= 2 { interleaved[base + 1] } else { lv };
        l.push(lv);
        r.push(rv);
    }
    Stereo { sr, l, r }
}

pub fn print_table(rows: &[Metrics]) {
    println!(
        "{:<34} {:>7} {:>6} {:>7} {:>6} {:>6} {:>6} {:>6} {:>8} {:>7} {:>8} {:>7} {:>7} {:>9} {:>7} {:>7}",
        "name", "secs", "peak", "rmsdB", "crest", "low%", "mid%", "high%", "cent_Hz", "cent±", "onset/s", "dyn±dB", "L/Rcor", "decaydB/s", "AMmed", "AMp90"
    );
    for r in rows {
        let decay_s = match r.decay_slope_db_per_s {
            Some(v) => format!("{v:>9.1}"),
            None => format!("{:>9}", "n/a"),
        };
        println!(
            "{:<34} {:>7.1} {:>6.3} {:>7.2} {:>6.2} {:>6.1} {:>6.1} {:>6.1} {:>8.0} {:>7.0} {:>8.3} {:>7.2} {:>7.3} {} {:>7.3} {:>7.3}",
            r.name,
            r.secs,
            r.peak,
            r.rms_dbfs,
            r.crest_db,
            r.band_low * 100.0,
            r.band_mid * 100.0,
            r.band_high * 100.0,
            r.centroid_mean_hz,
            r.centroid_std_hz,
            r.onset_density_per_sec,
            r.dyn_rms_std_db,
            r.stereo_corr,
            decay_s,
            r.am_depth_median,
            r.am_depth_p90
        );
    }
}

#[cfg(test)]
mod am_depth_tests {
    use super::measure_am_depth;
    use std::f32::consts::PI;

    /// 定常音（無変調）は AM 深度がほぼ0であるべき（測定手法の健全性確認）。
    #[test]
    fn steady_tone_has_near_zero_am_depth() {
        let sr = 44100.0f32;
        let n = (5.0 * sr) as usize;
        let mono: Vec<f32> = (0..n)
            .map(|i| (i as f32 / sr * 2.0 * PI * 440.0).sin())
            .collect();
        let (med, p90) = measure_am_depth(&mono, sr);
        assert!(
            med < 0.05 && p90 < 0.05,
            "定常音のAM深度が0に近くない: med={med} p90={p90}"
        );
    }

    /// per-voice AM 診断: パッド1声部だけを単体レンダリングしてAM深度を測る。
    /// ミックス全体の測定では他声部（ベース/メロディ/リバーブ）に希釈されて
    /// 「単一パッドの一定周期のうなり」が薄まって見えるため、うなりの発生源
    /// であるパッドを裸で観測する（docs/bgm-design-notes.md §6.4）。
    /// main.wav の Am7 ボイシング・実使用パラメータ（level 0.60 / LPF 1200Hz /
    /// attack 0.9s / release 1.2s）で 16 秒持続させた定常状態を測定する。
    /// アサーションは「うなりが消えていない（med>0.02、テストトーン化の禁止）
    /// かつ過剰でない（p90<0.55、リファレンス AM_p90 平均 0.61 のやや下）」の
    /// 粗い両側ゲート。
    #[test]
    fn pad_voice_am_depth_in_expected_band() {
        use crate::synth::{self, Stereo};
        let sr = synth::SR;
        let secs = 16.0f32;
        let n = (secs * sr as f32) as usize;
        let mut buf = Stereo::zeros(sr, n + (2.0 * sr as f32) as usize);
        // main.wav の Am7 声部連結ボイシング相当（A3, C4, E4, G4）
        let freqs: Vec<f32> = [-3i32, 0, 4, 7]
            .iter()
            .map(|&s| synth::degree_freq(synth::C4, s))
            .collect();
        synth::add_pad(&mut buf, sr, 0, n, &freqs, 0.60, 1200.0, 0.9, 1.2, 0x9001);
        let mono: Vec<f32> = buf
            .l
            .iter()
            .zip(buf.r.iter())
            .map(|(&a, &b)| 0.5 * (a + b))
            .collect();
        // ノート自身のアタック/リリースの立ち上がり・減衰は本物の音量変化で
        // あり「うなり」ではないので、先頭/末尾4秒を捨てて持続部だけを測る
        // （曲中のパッドはクロスフェードで常時鳴るため持続部が実態に近い）。
        let skip = (4.0 * sr as f32) as usize;
        let sustain = &mono[skip..mono.len() - skip];
        let (med, p90) = measure_am_depth(sustain, sr as f32);
        println!("[pad-voice AM] med={med:.3} p90={p90:.3}");
        assert!(
            med > 0.02,
            "パッドのうなりが消えている（テストトーン化）: med={med}"
        );
        assert!(p90 < 0.55, "パッド単体のうなりが深すぎる: p90={p90}");
    }

    /// 既知の変調指数(m0=0.9)・帯域内レート(2Hz)でトレモロ変調した信号は、
    /// 測定されるAM深度がほぼ m0 に一致するべき（測定手法の健全性確認）。
    #[test]
    fn known_tremolo_depth_is_recovered() {
        let sr = 44100.0f32;
        let n = (6.0 * sr) as usize;
        let m0 = 0.9f32;
        let mono: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr;
                let env = 1.0 + m0 * (t * 2.0 * PI * 2.0).sin();
                env * (t * 2.0 * PI * 440.0).sin()
            })
            .collect();
        let (med, _p90) = measure_am_depth(&mono, sr);
        assert!(
            (med - m0).abs() < 0.15,
            "既知トレモロ深度({m0})を再現できていない: 測定={med}"
        );
    }
}
