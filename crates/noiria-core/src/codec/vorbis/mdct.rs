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

/// 特定ブロックサイズ `n` 用に事前計算した cos テーブルを保持する逆 MDCT 実行器。
pub struct Imdct {
    n: usize,
    /// cos(pi*m/(4n)) for m in 0..8n
    cos_table: Vec<f64>,
}

impl Imdct {
    pub fn new(n: usize) -> Self {
        let period = 8 * n;
        let cos_table: Vec<f64> = (0..period)
            .map(|m| (PI * m as f64 / (4.0 * n as f64)).cos())
            .collect();
        Self { n, cos_table }
    }

    #[inline]
    fn cos_h(&self, n_idx: usize, k_idx: usize) -> f64 {
        let n = self.n as i64;
        let m = (4 * n_idx as i64 + n + 2) * (2 * k_idx as i64 + 1);
        let period = 8 * self.n as i64;
        let idx = m.rem_euclid(period) as usize;
        self.cos_table[idx]
    }

    /// `x` は長さ n/2 の周波数領域係数。出力は長さ n の時間領域信号
    /// (まだ窓掛け・オーバーラップ前)。
    #[allow(clippy::needless_range_loop)]
    pub fn transform(&self, x: &[f32]) -> Vec<f32> {
        debug_assert_eq!(x.len(), self.n / 2);
        // 注意: 教科書的な TDAC (Time Domain Alias Cancellation) の理論上は
        // 「無スケールの forward カーネル和」と対になる逆変換のスケールは 4/N になる
        // (このモジュールのテスト `mdct_perfect_reconstruction_random_signal` は実際に
        // その組み合わせで完全再構成を検証している)。しかし実際の Vorbis ビットストリーム
        // は、エンコーダ側の forward 変換に十分な正規化が織り込み済みという別の規約を
        // 採用しており、デコーダ側はスケール 1.0 (無スケール) の逆カーネル和だけでよい。
        // これは rodio (symphonia-codec-vorbis をバックエンドとする) との突き合わせテスト
        // (`tests/vorbis_crosscheck.rs`) で実測して確認した (スケール 1.0 のときのみ
        // 誤差ゼロ・相関 1.0 で一致し、4/N 等の他の定数では一致しない)。
        let scale = 1.0f64;
        let half = self.n / 2;
        let mut out = vec![0f32; self.n];
        for n_idx in 0..self.n {
            let mut acc = 0f64;
            for k_idx in 0..half {
                acc += x[k_idx] as f64 * self.cos_h(n_idx, k_idx);
            }
            out[n_idx] = (acc * scale) as f32;
        }
        out
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
