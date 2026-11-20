//! Floor type 1 (仕様書 7章)。区分線形のスペクトル包絡線を表現する。
//! Floor type 0 (6章、旧式の LSP ベース) は現代のエンコーダではほぼ使われないため、
//! 遭遇した場合は明示エラーとする（仕様書 6章にも "generally unused" である旨の記載あり）。

use super::bitreader::BitReader;
use super::codebook::{ilog, Codebook};
use anyhow::{bail, ensure, Result};

/// 仕様書 10.1 floor1_inverse_dB_table (256 要素の静的テーブル)。
/// 値は Xiph の Vorbis I 仕様書 (https://xiph.org/vorbis/doc/Vorbis_I_spec.html) 10.1 節から
/// そのまま転記。(clippy::excessive_precision は f32 として丸められる桁の警告だが、
/// 仕様書の値をそのまま転記する意図的な精度なので許容する)
#[allow(clippy::excessive_precision)]
pub const FLOOR1_INVERSE_DB_TABLE: [f32; 256] = [
    1.0649863e-07,
    1.1341951e-07,
    1.2079015e-07,
    1.2863978e-07,
    1.3699951e-07,
    1.4590251e-07,
    1.5538408e-07,
    1.6548181e-07,
    1.7623575e-07,
    1.8768855e-07,
    1.9988561e-07,
    2.1287530e-07,
    2.2670913e-07,
    2.4144197e-07,
    2.5713223e-07,
    2.7384213e-07,
    2.9163793e-07,
    3.1059021e-07,
    3.3077411e-07,
    3.5226968e-07,
    3.7516214e-07,
    3.9954229e-07,
    4.2550680e-07,
    4.5315863e-07,
    4.8260743e-07,
    5.1396998e-07,
    5.4737065e-07,
    5.8294187e-07,
    6.2082472e-07,
    6.6116941e-07,
    7.0413592e-07,
    7.4989464e-07,
    7.9862701e-07,
    8.5052630e-07,
    9.0579828e-07,
    9.6466216e-07,
    1.0273513e-06,
    1.0941144e-06,
    1.1652161e-06,
    1.2409384e-06,
    1.3215816e-06,
    1.4074654e-06,
    1.4989305e-06,
    1.5963394e-06,
    1.7000785e-06,
    1.8105592e-06,
    1.9282195e-06,
    2.0535261e-06,
    2.1869758e-06,
    2.3290978e-06,
    2.4804557e-06,
    2.6416497e-06,
    2.8133190e-06,
    2.9961443e-06,
    3.1908506e-06,
    3.3982101e-06,
    3.6190449e-06,
    3.8542308e-06,
    4.1047004e-06,
    4.3714470e-06,
    4.6555282e-06,
    4.9580707e-06,
    5.2802740e-06,
    5.6234160e-06,
    5.9888572e-06,
    6.3780469e-06,
    6.7925283e-06,
    7.2339451e-06,
    7.7040476e-06,
    8.2047000e-06,
    8.7378876e-06,
    9.3057248e-06,
    9.9104632e-06,
    1.0554501e-05,
    1.1240392e-05,
    1.1970856e-05,
    1.2748789e-05,
    1.3577278e-05,
    1.4459606e-05,
    1.5399272e-05,
    1.6400004e-05,
    1.7465768e-05,
    1.8600792e-05,
    1.9809576e-05,
    2.1096914e-05,
    2.2467911e-05,
    2.3928002e-05,
    2.5482978e-05,
    2.7139006e-05,
    2.8902651e-05,
    3.0780908e-05,
    3.2781225e-05,
    3.4911534e-05,
    3.7180282e-05,
    3.9596466e-05,
    4.2169667e-05,
    4.4910090e-05,
    4.7828601e-05,
    5.0936773e-05,
    5.4246931e-05,
    5.7772202e-05,
    6.1526565e-05,
    6.5524908e-05,
    6.9783085e-05,
    7.4317983e-05,
    7.9147585e-05,
    8.4291040e-05,
    8.9768747e-05,
    9.5602426e-05,
    0.00010181521,
    0.00010843174,
    0.00011547824,
    0.00012298267,
    0.00013097477,
    0.00013948625,
    0.00014855085,
    0.00015820453,
    0.00016848555,
    0.00017943469,
    0.00019109536,
    0.00020351382,
    0.00021673929,
    0.00023082423,
    0.00024582449,
    0.00026179955,
    0.00027881276,
    0.00029693158,
    0.00031622787,
    0.00033677814,
    0.00035866388,
    0.00038197188,
    0.00040679456,
    0.00043323036,
    0.00046138411,
    0.00049136745,
    0.00052329927,
    0.00055730621,
    0.00059352311,
    0.00063209358,
    0.00067317058,
    0.00071691700,
    0.00076350630,
    0.00081312324,
    0.00086596457,
    0.00092223983,
    0.00098217216,
    0.0010459992,
    0.0011139742,
    0.0011863665,
    0.0012634633,
    0.0013455702,
    0.0014330129,
    0.0015261382,
    0.0016253153,
    0.0017309374,
    0.0018434235,
    0.0019632195,
    0.0020908006,
    0.0022266726,
    0.0023713743,
    0.0025254795,
    0.0026895994,
    0.0028643847,
    0.0030505286,
    0.0032487691,
    0.0034598925,
    0.0036847358,
    0.0039241906,
    0.0041792066,
    0.0044507950,
    0.0047400328,
    0.0050480668,
    0.0053761186,
    0.0057254891,
    0.0060975636,
    0.0064938176,
    0.0069158225,
    0.0073652516,
    0.0078438871,
    0.0083536271,
    0.0088964928,
    0.009474637,
    0.010090352,
    0.010746080,
    0.011444421,
    0.012188144,
    0.012980198,
    0.013823725,
    0.014722068,
    0.015678791,
    0.016697687,
    0.017782797,
    0.018938423,
    0.020169149,
    0.021479854,
    0.022875735,
    0.024362330,
    0.025945531,
    0.027631618,
    0.029427276,
    0.031339626,
    0.033376252,
    0.035545228,
    0.037855157,
    0.040315199,
    0.042935108,
    0.045725273,
    0.048696758,
    0.051861348,
    0.055231591,
    0.058820850,
    0.062643361,
    0.066714279,
    0.071049749,
    0.075666962,
    0.080584227,
    0.085821044,
    0.091398179,
    0.097337747,
    0.10366330,
    0.11039993,
    0.11757434,
    0.12521498,
    0.13335215,
    0.14201813,
    0.15124727,
    0.16107617,
    0.17154380,
    0.18269168,
    0.19456402,
    0.20720788,
    0.22067342,
    0.23501402,
    0.25028656,
    0.26655159,
    0.28387361,
    0.30232132,
    0.32196786,
    0.34289114,
    0.36517414,
    0.38890521,
    0.41417847,
    0.44109412,
    0.46975890,
    0.50028648,
    0.53279791,
    0.56742212,
    0.60429640,
    0.64356699,
    0.68538959,
    0.72993007,
    0.77736504,
    0.82788260,
    0.88168307,
    0.9389798,
    1.0,
];

const FLOOR1_RANGE: [u32; 4] = [256, 128, 86, 64];

pub struct Floor1 {
    partition_class_list: Vec<u32>,
    class_dimensions: Vec<u32>,
    class_subclasses: Vec<u32>,
    class_masterbooks: Vec<i32>,
    /// [class][subclass] -> book index (-1 なら未使用)。
    subclass_books: Vec<Vec<i32>>,
    multiplier: u32,
    x_list: Vec<u32>,
    /// x_list をソートした際の元インデックス列 (昇順)。
    sorted_index: Vec<usize>,
}

/// 仕様書 9.2.4/9.2.5 low_neighbor/high_neighbor。
/// v の添字 0..x の範囲内で v[x] 未満の最大値 / v[x] を超える最小値を持つ添字を返す。
fn low_neighbor(v: &[u32], x: usize) -> usize {
    let mut best: Option<usize> = None;
    for n in 0..x {
        if v[n] < v[x] && (best.is_none() || v[n] > v[best.unwrap()]) {
            best = Some(n);
        }
    }
    best.expect("floor1 x_list は 0 番目が必ず最小値のため low_neighbor は常に見つかる")
}
fn high_neighbor(v: &[u32], x: usize) -> usize {
    let mut best: Option<usize> = None;
    for n in 0..x {
        if v[n] > v[x] && (best.is_none() || v[n] < v[best.unwrap()]) {
            best = Some(n);
        }
    }
    best.expect("floor1 x_list は 1 番目が必ず最大値のため high_neighbor は常に見つかる")
}

/// 仕様書 9.2.6 render_point。
fn render_point(x0: i32, y0: i32, x1: i32, y1: i32, x: i32) -> i32 {
    let dy = y1 - y0;
    let adx = x1 - x0;
    let ady = dy.abs();
    let err = ady * (x - x0);
    let off = err / adx;
    if dy < 0 {
        y0 - off
    } else {
        y0 + off
    }
}

/// 仕様書 9.2.7 render_line。v[x0..=x1] を書き込む。
fn render_line(
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    v: &mut [f32],
    to_amplitude: &dyn Fn(i32) -> f32,
) {
    let dy = y1 - y0;
    let adx = x1 - x0;
    if adx <= 0 {
        return;
    }
    let mut ady = dy.abs();
    let base = dy / adx; // Rust の整数除算は 0 方向丸め = 仕様指定の丸めと一致
    let mut x = x0;
    let mut y = y0;
    let mut err = 0i32;
    let sy = if dy < 0 { base - 1 } else { base + 1 };
    ady -= base.abs() * adx;
    if (x as usize) < v.len() {
        v[x as usize] = to_amplitude(y);
    }
    for xi in (x0 + 1)..x1 {
        err += ady;
        if err >= adx {
            err -= adx;
            y += sy;
        } else {
            y += base;
        }
        x = xi;
        if (x as usize) < v.len() {
            v[x as usize] = to_amplitude(y);
        }
    }
}

impl Floor1 {
    /// この floor が参照するコードブック番号（master / subclass book）がすべて `num_codebooks`
    /// 未満であることを確かめる。setup で弾かないとデコード時に範囲外添字で panic する。
    pub fn validate_books(&self, num_codebooks: usize) -> Result<()> {
        for (class, &master) in self.class_masterbooks.iter().enumerate() {
            if self.class_subclasses[class] != 0 {
                ensure!(
                    master >= 0 && (master as usize) < num_codebooks,
                    "floor1 の masterbook {master} がコードブック数 {num_codebooks} の範囲外です"
                );
            }
        }
        for &book in self.subclass_books.iter().flatten() {
            ensure!(
                book < 0 || (book as usize) < num_codebooks,
                "floor1 の subclass book {book} がコードブック数 {num_codebooks} の範囲外です"
            );
        }
        Ok(())
    }

    pub fn parse(br: &mut BitReader) -> Result<Self> {
        let partitions = br.read(5);
        let mut partition_class_list = vec![0u32; partitions as usize];
        let mut maximum_class: i32 = -1;
        for pc in partition_class_list.iter_mut() {
            let c = br.read(4);
            *pc = c;
            maximum_class = maximum_class.max(c as i32);
        }
        let n_classes = (maximum_class + 1).max(0) as usize;
        let mut class_dimensions = vec![0u32; n_classes];
        let mut class_subclasses = vec![0u32; n_classes];
        let mut class_masterbooks = vec![-1i32; n_classes];
        let mut subclass_books = vec![Vec::new(); n_classes];
        for c in 0..n_classes {
            class_dimensions[c] = br.read(3) + 1;
            class_subclasses[c] = br.read(2);
            if class_subclasses[c] != 0 {
                class_masterbooks[c] = br.read(8) as i32;
            }
            let count = 1u32 << class_subclasses[c];
            let mut books = vec![-1i32; count as usize];
            for b in books.iter_mut() {
                *b = br.read(8) as i32 - 1;
            }
            subclass_books[c] = books;
        }
        let multiplier = br.read(2) + 1;
        let rangebits = br.read(4);
        let mut x_list = vec![0u32, 1u32 << rangebits];
        for &pc in &partition_class_list {
            let cdim = class_dimensions[pc as usize];
            for _ in 0..cdim {
                x_list.push(br.read(rangebits));
            }
        }
        ensure!(x_list.len() <= 65, "floor1_X_list が長すぎます");
        ensure!(!br.eof, "floor1 setup 読み取り中にパケット終端に達しました");
        // 一意性チェック (仕様上の必須検査)。
        {
            let mut sorted = x_list.clone();
            sorted.sort_unstable();
            for w in sorted.windows(2) {
                ensure!(w[0] != w[1], "floor1_X_list に重複した値があります");
            }
        }

        let mut sorted_index: Vec<usize> = (0..x_list.len()).collect();
        sorted_index.sort_by_key(|&i| x_list[i]);

        Ok(Self {
            partition_class_list,
            class_dimensions,
            class_subclasses,
            class_masterbooks,
            subclass_books,
            multiplier,
            x_list,
            sorted_index,
        })
    }

    /// パケットデコード (7.2.3) + 曲線合成 (7.2.4)。`n` はブロックサイズ (曲線長は n/2)。
    /// `None` は「このチャンネルは今フレーム未使用 (nonzero フラグ unset)」を意味する。
    pub fn decode(
        &self,
        br: &mut BitReader,
        codebooks: &[Codebook],
        n: usize,
    ) -> Result<Option<Vec<f32>>> {
        if !br.read_bool() {
            return Ok(None);
        }
        let curve_len = n / 2;
        let range = FLOOR1_RANGE[(self.multiplier - 1) as usize];
        let range_bits = ilog(range as i64 - 1);
        let values = self.x_list.len();
        let mut y = vec![0i32; values];
        y[0] = br.read(range_bits) as i32;
        y[1] = br.read(range_bits) as i32;
        let mut offset = 2usize;
        for &pc in &self.partition_class_list {
            let class = pc as usize;
            let cdim = self.class_dimensions[class];
            let cbits = self.class_subclasses[class];
            let csub = (1u32 << cbits) - 1;
            let mut cval: u32 = 0;
            if cbits > 0 {
                let master = self.class_masterbooks[class];
                ensure!(master >= 0, "floor1 masterbook が未設定です");
                cval = codebooks[master as usize].decode_scalar(br)?;
            }
            for j in 0..cdim {
                let book = self.subclass_books[class][(cval & csub) as usize];
                cval >>= cbits;
                let val = if book >= 0 {
                    codebooks[book as usize].decode_scalar(br)? as i32
                } else {
                    0
                };
                y[offset + j as usize] = val;
            }
            offset += cdim as usize;
            if br.eof {
                return Ok(None);
            }
        }
        if br.eof {
            return Ok(None);
        }

        // step 1: 振幅値の合成 (7.2.4)
        let mut step2_flag = vec![false; values];
        let mut final_y = vec![0i32; values];
        step2_flag[0] = true;
        step2_flag[1] = true;
        final_y[0] = y[0].clamp(0, range as i32 - 1);
        final_y[1] = y[1].clamp(0, range as i32 - 1);
        for i in 2..values {
            let lo = low_neighbor(&self.x_list, i);
            let hi = high_neighbor(&self.x_list, i);
            let predicted = render_point(
                self.x_list[lo] as i32,
                final_y[lo],
                self.x_list[hi] as i32,
                final_y[hi],
                self.x_list[i] as i32,
            );
            let val = y[i];
            let highroom = range as i32 - predicted;
            let lowroom = predicted;
            let room = if highroom < lowroom {
                highroom * 2
            } else {
                lowroom * 2
            };
            if val != 0 {
                step2_flag[lo] = true;
                step2_flag[hi] = true;
                step2_flag[i] = true;
                if val >= room {
                    if highroom > lowroom {
                        final_y[i] = val - lowroom + predicted;
                    } else {
                        final_y[i] = predicted - val + highroom - 1;
                    }
                } else if val % 2 == 1 {
                    final_y[i] = predicted - ((val + 1) / 2);
                } else {
                    final_y[i] = predicted + (val / 2);
                }
            } else {
                step2_flag[i] = false;
                final_y[i] = predicted;
            }
            // 仕様書の注記どおり [0, range) にクランプする。
            final_y[i] = final_y[i].clamp(0, range as i32 - 1);
        }

        // step 2: 曲線合成
        let mut floor = vec![0f32; curve_len];
        let to_amp = |v: i32| -> f32 {
            let idx = v.clamp(0, 255) as usize;
            FLOOR1_INVERSE_DB_TABLE[idx]
        };
        let x_sorted: Vec<i32> = self
            .sorted_index
            .iter()
            .map(|&i| self.x_list[i] as i32)
            .collect();
        let y_sorted: Vec<i32> = self.sorted_index.iter().map(|&i| final_y[i]).collect();
        let step2_sorted: Vec<bool> = self.sorted_index.iter().map(|&i| step2_flag[i]).collect();

        let mut hx = 0i32;
        let mut hy = y_sorted[0] * self.multiplier as i32;
        let mut lx = 0i32;
        let mut ly = hy;
        for i in 1..values {
            if step2_sorted[i] {
                hy = y_sorted[i] * self.multiplier as i32;
                hx = x_sorted[i];
                render_line(lx, ly, hx, hy, &mut floor, &to_amp);
                lx = hx;
                ly = hy;
            }
        }
        if hx < curve_len as i32 {
            render_line(hx, hy, curve_len as i32, hy, &mut floor, &to_amp);
        }
        // hx > n の場合はそもそも render_line が curve_len 境界内にしか書かないため
        // 追加の truncate 処理は不要 (Vec は最初から curve_len 長で確保済み)。

        Ok(Some(floor))
    }
}

/// Floor type 0 は現代のエンコーダでは実質使用されないため未実装。遭遇時は明示エラー。
pub fn reject_floor0(br: &mut BitReader) -> Result<()> {
    let _ = br;
    bail!("Floor type 0 は非対応です (現代の Vorbis エンコーダでは通常使用されません)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn table_has_256_entries_and_is_monotonic() {
        assert_eq!(FLOOR1_INVERSE_DB_TABLE.len(), 256);
        for w in FLOOR1_INVERSE_DB_TABLE.windows(2) {
            assert!(w[0] < w[1], "{} !< {}", w[0], w[1]);
        }
        assert!((FLOOR1_INVERSE_DB_TABLE[255] - 1.0).abs() < 1e-6);
        let first = FLOOR1_INVERSE_DB_TABLE[0];
        assert!(first > 0.0 && first < 1e-6);
    }

    #[test]
    fn render_point_matches_linear_interpolation() {
        // x0=0,y0=0 -> x1=10,y1=100 の直線上、x=5 では y=50 付近。
        let y = render_point(0, 0, 10, 100, 5);
        assert_eq!(y, 50);
    }

    #[test]
    fn render_line_fills_left_endpoint_and_is_monotonic() {
        // render_line は仕様書どおり [x0, x1) を書く (x1 自身は次のセグメント呼び出しが
        // 書く、もしくは曲線合成側の最終延長呼び出しが書く)。
        let mut v = vec![-1f32; 20];
        render_line(0, 0, 10, 100, &mut v, &|y| y as f32);
        assert_eq!(v[0], 0.0);
        assert_eq!(v[9], 90.0);
        assert_eq!(v[10], -1.0, "x1 自身は render_line が書かない");
        for w in v[0..10].windows(2) {
            assert!(w[1] >= w[0]);
        }
    }

    #[test]
    fn low_high_neighbor_spec_example() {
        // 仕様書 7.2.1 の例: X list (list order) 0,128,64,32,96,16,48,80,112
        let x = [0u32, 128, 64, 32, 96, 16, 48, 80, 112];
        // i=2 (64): 候補は index0(0),1(128) のみ -> low=0(0<64), high=1(128>64)
        assert_eq!(low_neighbor(&x, 2), 0);
        assert_eq!(high_neighbor(&x, 2), 1);
        // i=3 (32): 候補 index0(0),1(128),2(64) -> low=0(0<32), high=2(64>32, 64<128なので64が最小)
        assert_eq!(low_neighbor(&x, 3), 0);
        assert_eq!(high_neighbor(&x, 3), 2);
    }
}
