//! 逆 MDCT (Modified Discrete Cosine Transform) と Vorbis 窓関数、オーバーラップ加算。
//!
//! 仕様書 4.3.7 は MDCT 自体の詳細な説明を Sporer/Brandenburg/Edler の論文に譲っており、
//! 具体的な変換式は載せていない。ここでは一般に知られる（特定のデコーダ実装に依存しない）
//! "odd time / odd frequency" MDCT の標準核
//! `h(n,k) = cos( (2π/N) * (n + N/4 + 1/2) * (k + 1/2) )`
//! を用い、逆変換は `y[n] = (4/N) * Σ_k X[k] * h(n,k)` というテキストブック通りの
//! TDAC (Time Domain Alias Cancellation) の定義に従う。この核はコサイン計算を
//! `cos(π*m/(4N))` の 8N 要素テーブル 1 本の参照に落とし込める (角度の周期性を利用)。

use std::f64::consts::PI;

/// 仕様書 4.3.1 の Vorbis 窓関数。`n` は現在ブロックのサイズ、`blockflag` は long/short、
/// `prev_window_flag`/`next_window_flag` は long ブロック時のみ意味を持つ。
// ループ変数 i は書き込み先の添字であると同時に区間内オフセット計算にも使うため、
// enumerate+skip/take への書き換えはかえって読みにくくなる。
#[allow(clippy::needless_range_loop)]
pub fn build_window(
    n: usize,
    blockflag: bool,
    prev_window_flag: bool,
    next_window_flag: bool,
    blocksize_0: usize,
) -> Vec<f32> {
    let window_center = n / 2;
    let (left_window_start, left_window_end, left_n) = if blockflag && !prev_window_flag {
        (
            n / 4 - blocksize_0 / 4,
            n / 4 + blocksize_0 / 4,
            blocksize_0 / 2,
        )
    } else {
        (0, window_center, n / 2)
    };
    let (right_window_start, right_window_end, right_n) = if blockflag && !next_window_flag {
        (
            n * 3 / 4 - blocksize_0 / 4,
            n * 3 / 4 + blocksize_0 / 4,
            blocksize_0 / 2,
        )
    } else {
        (window_center, n, n / 2)
    };

    let mut w = vec![0f32; n];
    for i in left_window_start..left_window_end {
        let x = (i - left_window_start) as f64 + 0.5;
        let inner = (x / left_n as f64) * (PI / 2.0);
        w[i] = (PI / 2.0 * inner.sin().powi(2)).sin() as f32;
    }
    for i in left_window_end..right_window_start {
        w[i] = 1.0;
    }
    for i in right_window_start..right_window_end {
        let x = (i - right_window_start) as f64 + 0.5;
        let inner = (x / right_n as f64) * (PI / 2.0) + (PI / 2.0);
        w[i] = (PI / 2.0 * inner.sin().powi(2)).sin() as f32;
    }
    w
}

/// `Imdct::transform_into` 用の再利用スクラッチ。パケット毎のヒープ確保を避ける。
#[derive(Default)]
pub struct ImdctScratch {
    fft_re: Vec<f64>,
    fft_im: Vec<f64>,
    u: Vec<f32>,
}

/// 特定ブロックサイズ `n` 用の O(N log N) 逆 MDCT 実行器（N/4 点複素 FFT ベース）。
pub struct Imdct {
    n: usize,
    /// exp(+i * 2π/N * (k + 1/8)) for k in 0..N/4
    tw_re: Vec<f64>,
    tw_im: Vec<f64>,
    /// radix-2 FFT 用のビット反転表 (長さ N/4)
    bit_rev: Vec<usize>,
    /// radix-2 FFT ステージごとの回転因子 exp(+i * 2π * j / step)
    fft_tw_re: Vec<f64>,
    fft_tw_im: Vec<f64>,
}

impl Imdct {
    pub fn new(n: usize) -> Self {
        assert!(
            n >= 8 && n.is_power_of_two(),
            "IMDCT size must be power of 2 >= 8, got {n}"
        );
        let n4 = n / 4;
        let mut tw_re = Vec::with_capacity(n4);
        let mut tw_im = Vec::with_capacity(n4);
        let base_ang = 2.0 * PI / n as f64;
        for k in 0..n4 {
            let ang = base_ang * (k as f64 + 0.125);
            tw_re.push(ang.cos());
            tw_im.push(ang.sin());
        }

        let log_n4 = n4.trailing_zeros();
        let mut bit_rev = Vec::with_capacity(n4);
        for i in 0..n4 {
            bit_rev.push(if log_n4 == 0 {
                0
            } else {
                i.reverse_bits() >> (usize::BITS - log_n4)
            });
        }

        let mut fft_tw_re = Vec::with_capacity(n4);
        let mut fft_tw_im = Vec::with_capacity(n4);
        let mut half_len = 1usize;
        while half_len < n4 {
            let step = half_len * 2;
            let ang_step = 2.0 * PI / step as f64;
            for j in 0..half_len {
                let ang = ang_step * j as f64;
                fft_tw_re.push(ang.cos());
                fft_tw_im.push(ang.sin());
            }
            half_len = step;
        }

        Self {
            n,
            tw_re,
            tw_im,
            bit_rev,
            fft_tw_re,
            fft_tw_im,
        }
    }

    /// `x` は長さ n/2 の周波数領域係数。出力は長さ n の時間領域信号
    /// (まだ窓掛け・オーバーラップ前)。
    #[cfg(test)]
    pub fn transform(&self, x: &[f32]) -> Vec<f32> {
        let mut scratch = ImdctScratch::default();
        let mut out = vec![0f32; self.n];
        self.transform_into(x, &mut scratch, &mut out);
        out
    }

    /// `scratch` を再利用して `out` (長さ n) へ直接書き込む。
    #[allow(clippy::needless_range_loop)]
    pub fn transform_into(&self, x: &[f32], scratch: &mut ImdctScratch, out: &mut [f32]) {
        debug_assert_eq!(x.len(), self.n / 2);
        debug_assert_eq!(out.len(), self.n);
        // 実際の Vorbis ビットストリームはスケール 1.0 (無スケール) の逆カーネル和を使う
        // (`tests/vorbis_crosscheck.rs` で symphonia と検証済み)。
        let n = self.n;
        let n2 = n / 2;
        let n4 = n / 4;

        scratch.fft_re.resize(n4, 0.0);
        scratch.fft_im.resize(n4, 0.0);
        scratch.u.resize(n2, 0.0);
        let fft_re = &mut scratch.fft_re[..n4];
        let fft_im = &mut scratch.fft_im[..n4];
        let u = &mut scratch.u[..n2];

        for k in 0..n4 {
            let a = x[2 * k] as f64;
            let b = -(x[n2 - 1 - 2 * k] as f64);
            let tr = self.tw_re[k];
            let ti = self.tw_im[k];
            let dst = self.bit_rev[k];
            fft_re[dst] = a * tr - b * ti;
            fft_im[dst] = a * ti + b * tr;
        }

        let mut stage_off = 0usize;
        let mut half_len = 1usize;
        while half_len < n4 {
            let step = half_len * 2;
            for block in (0..n4).step_by(step) {
                for j in 0..half_len {
                    let wr = self.fft_tw_re[stage_off + j];
                    let wi = self.fft_tw_im[stage_off + j];
                    let u_idx = block + j;
                    let v_idx = u_idx + half_len;
                    let vr = fft_re[v_idx];
                    let vi = fft_im[v_idx];
                    let tr = vr * wr - vi * wi;
                    let ti = vr * wi + vi * wr;
                    let ur = fft_re[u_idx];
                    let ui = fft_im[u_idx];
                    fft_re[u_idx] = ur + tr;
                    fft_im[u_idx] = ui + ti;
                    fft_re[v_idx] = ur - tr;
                    fft_im[v_idx] = ui - ti;
                }
            }
            stage_off += half_len;
            half_len = step;
        }

        for j in 0..n4 {
            let cr = fft_re[j];
            let ci = fft_im[j];
            let tr = self.tw_re[j];
            let ti = self.tw_im[j];
            u[2 * j] = (cr * tr - ci * ti) as f32;
            u[n2 - 1 - 2 * j] = (cr * ti + ci * tr) as f32;
        }

        for t in 0..n4 {
            out[t] = u[n4 + t];
            out[n4 + t] = -u[n2 - 1 - t];
            out[n2 + t] = -u[n4 - 1 - t];
            out[3 * n4 + t] = -u[t];
        }
    }
}

/// テスト/検証専用の素朴な forward MDCT (kernel-sum, スケールなし)。
/// 本番デコーダでは使わない (Vorbis はエンコーダ側の変換なので decode 専用実装には不要)。
#[cfg(test)]
#[allow(clippy::needless_range_loop)]
fn forward_mdct(x: &[f32], n: usize) -> Vec<f32> {
    let n0 = n as f64 / 4.0 + 0.5;
    let mut out = vec![0f32; n / 2];
    for k in 0..n / 2 {
        let mut acc = 0f64;
        for (i, &xi) in x.iter().enumerate() {
            acc += xi as f64 * (2.0 * PI / n as f64 * (i as f64 + n0) * (k as f64 + 0.5)).cos();
        }
        out[k] = acc as f32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_satisfies_princen_bradley() {
        // 対称な (long/long) 窓では w[i]^2 + w[i+n/2]^2 == 1 が全 i で成り立つ必要がある。
        let n = 1024;
        let w = build_window(n, true, true, true, 256);
        for i in 0..n / 2 {
            let s = w[i] * w[i] + w[i + n / 2] * w[i + n / 2];
            assert!((s - 1.0).abs() < 1e-5, "i={i} sum={s}");
        }
    }

    #[test]
    fn window_endpoints_near_zero_and_center_is_one_ish() {
        let n = 256;
        let w = build_window(n, false, false, false, 256);
        assert!(w[0] < 0.01);
        assert!(w[n - 1] < 0.01);
        assert!(w[n / 2 - 1] > 0.99);
    }

    #[test]
    fn mdct_perfect_reconstruction_random_signal() {
        // ランダム信号を 2 ブロック分用意し、forward MDCT (テスト専用の素朴な参照実装) →
        // 本番の Imdct::transform → 窓掛け → オーバーラップ加算で、中央 n/2 サンプルが
        // 元信号と一致することを確認する (MDCT の完全再構成性)。
        let n = 64usize;
        let mut rng = SmallRng(0x1234_5678);
        let sig: Vec<f32> = (0..2 * n).map(|_| rng.next_f32() * 2.0 - 1.0).collect();

        let w = build_window(n, false, false, false, n);
        let block0: Vec<f32> = (0..n).map(|i| sig[i] * w[i]).collect();
        let block1: Vec<f32> = (0..n).map(|i| sig[n / 2 + i] * w[i]).collect();

        // 教科書的な TDAC の完全再構成には、無スケールの forward カーネル和と対にして
        // 逆変換側に 4/N のスケールが必要になる (`Imdct::transform` 自体のスケールに
        // ついては本体のコメント参照: 実際の Vorbis ビットストリームのデコードでは
        // このテストとは異なる規約でスケール 1.0 を使う)。ここでは MDCT の変換対そのものの
        // 数学的な完全再構成性を検証したいので、そのテスト専用の 4/N を明示的に掛ける。
        let recon_scale = 4.0 / n as f32;
        let imdct = Imdct::new(n);
        let x0 = forward_mdct(&block0, n);
        let x1 = forward_mdct(&block1, n);
        let y0: Vec<f32> = imdct
            .transform(&x0)
            .iter()
            .zip(&w)
            .map(|(&y, &wi)| y * recon_scale * wi)
            .collect();
        let y1: Vec<f32> = imdct
            .transform(&x1)
            .iter()
            .zip(&w)
            .map(|(&y, &wi)| y * recon_scale * wi)
            .collect();

        let mut max_err = 0f32;
        for i in 0..n / 2 {
            let recon = y0[n / 2 + i] + y1[i];
            let orig = sig[n / 2 + i];
            max_err = max_err.max((recon - orig).abs());
        }
        assert!(max_err < 1e-3, "max_err={max_err}");
    }

    #[test]
    fn imdct_matches_naive_reference() {
        let mut rng = SmallRng(0xCAFE_BABE);
        for &n in &[64usize, 256, 2048] {
            let x: Vec<f32> = (0..n / 2).map(|_| rng.next_f32() * 2.0 - 1.0).collect();
            let fast = Imdct::new(n).transform(&x);
            let n0 = n as f64 / 4.0 + 0.5;
            let mut max_err = 0f32;
            for (i, &got) in fast.iter().enumerate() {
                let mut acc = 0f64;
                for (k, &xk) in x.iter().enumerate() {
                    acc += xk as f64
                        * (2.0 * PI / n as f64 * (i as f64 + n0) * (k as f64 + 0.5)).cos();
                }
                max_err = max_err.max((got - acc as f32).abs());
            }
            assert!(max_err < 1e-3, "n={n} max_err={max_err}");
        }
    }

    // 依存クレートを増やさないための超簡易 LCG 乱数 (テスト専用)。
    struct SmallRng(u64);
    impl SmallRng {
        fn next_f32(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 40) as u32 as f64 / (1u64 << 24) as f64) as f32
        }
    }
}
