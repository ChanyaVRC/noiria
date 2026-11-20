//! テスト専用の補助（`#[cfg(test)]` でのみコンパイルされる）。壊れた入力に対する
//! 「panic しない」「1 回の確保が妥当な上限を超えない」を各デコーダのテストで共通に検査する。
//!
//! 確保サイズの検査は `#[global_allocator]` を差し替えて行う。上限を超える確保を検出したら
//! その場で panic する: 本物の巨大確保を実行して OOM/スワップ嵐になるより、テストが
//! 即座に失敗する方が安全なため。GlobalAlloc からの unwind は仕様上 UB だが、unwind は
//! 呼び出し元の Vec 等（Rust ABI）へ抜けるだけで、テスト専用の使い方として実害は無い。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};

struct GuardAlloc;

thread_local! {
    // const 初期化 + Drop なしの Cell なので、アロケータ内から触っても再入しない。
    static LIMIT: Cell<usize> = const { Cell::new(usize::MAX) };
    /// このスレッドの生存確保バイト数（確保したスレッドと解放するスレッドが違うと不正確）と、その最大値。
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn track(delta: isize) {
    let _ = LIVE.try_with(|l| {
        let v = l.get() + delta;
        l.set(v);
        let _ = PEAK.try_with(|p| {
            if v > p.get() {
                p.set(v)
            }
        });
    });
}

fn check(size: usize) {
    let over = LIMIT.try_with(|l| size > l.get()).unwrap_or(false);
    if over {
        // panic メッセージの組み立ては確保を伴うため、先に無効化する。
        let _ = LIMIT.try_with(|l| l.set(usize::MAX));
        panic!("1 回の確保が上限を超えました: {size} バイト");
    }
}

// SAFETY: 確保・解放は System にそのまま委譲し、追加するのはスレッドローカルの計数と上限検査だけ。
unsafe impl GlobalAlloc for GuardAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        check(layout.size());
        track(layout.size() as isize);
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        check(layout.size());
        track(layout.size() as isize);
        System.alloc_zeroed(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        track(-(layout.size() as isize));
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        check(new_size);
        track(new_size as isize - layout.size() as isize);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOC: GuardAlloc = GuardAlloc;

/// `f` の実行中、このスレッドでの 1 回の確保が `limit` バイトを超えたら panic する。
pub fn with_alloc_limit<R>(limit: usize, f: impl FnOnce() -> R) -> R {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            let _ = LIMIT.try_with(|l| l.set(usize::MAX));
        }
    }
    let _reset = Reset;
    LIMIT.with(|l| l.set(limit));
    f()
}

/// `f` の実行中にこのスレッドで増えた生存確保バイト数のピーク（開始時点を 0 とする）を返す。
pub fn measure_peak<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let base = LIVE.with(|l| l.get());
    PEAK.with(|p| p.set(base));
    let r = f();
    let peak = PEAK.with(|p| p.get());
    (r, (peak - base).max(0) as usize)
}

/// 破壊入力 1 件あたりの確保上限。画像は 2^26 px の RGBA（256MB）が上限で、`Vec` の倍々成長の
/// 余裕を見込んで 512MB。
pub const FUZZ_ALLOC_LIMIT: usize = 512 << 20;

/// 破壊入力 1 件あたりの処理時間の上限。
pub const FUZZ_CASE_TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(3);

struct SlowProbe<'a> {
    trace: bool,
    label: &'a str,
    case: usize,
    started: std::time::Instant,
}

impl Drop for SlowProbe<'_> {
    fn drop(&mut self) {
        if self.trace && self.started.elapsed().as_millis() > 100 {
            eprintln!(
                "{}: case {} が {:?} かかった",
                self.label,
                self.case,
                self.started.elapsed()
            );
        }
    }
}

pub struct Lcg(pub u64);

impl Lcg {
    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 11
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// `base` を LCG で破壊した入力を `cases` 件作る。破壊は、バイト置換・ビット反転・
/// 長さフィールド風の 1/2/4 バイト窓の書き換え（0 / 0xFF / 境界値）・切り詰め・末尾追加。
pub fn mutate(base: &[u8], rng: &mut Lcg) -> Vec<u8> {
    let mut data = base.to_vec();
    let n_mut = 1 + rng.below(4);
    for _ in 0..n_mut {
        if data.is_empty() {
            break;
        }
        let idx = rng.below(data.len());
        match rng.below(6) {
            0 => data[idx] = rng.next() as u8,
            1 => data[idx] ^= 1 << rng.below(8),
            2 | 3 => {
                // 長さ・個数フィールドを狙う: 1/2/4 バイトを極端な値で埋める。
                let width = [1usize, 2, 4][rng.below(3)];
                let fill: [u8; 4] = match rng.below(5) {
                    0 => [0; 4],
                    1 => [0xFF; 4],
                    2 => [0x7F, 0xFF, 0xFF, 0xFF],
                    3 => [0x80, 0, 0, 0],
                    _ => [rng.next() as u8; 4],
                };
                for k in 0..width {
                    if idx + k < data.len() {
                        data[idx + k] = fill[k];
                    }
                }
            }
            4 => {
                // 先頭 64 バイト（ヘッダ）に集中させる。
                let i = rng.below(data.len().min(64));
                data[i] = rng.next() as u8;
            }
            _ => data[idx] = data[idx].wrapping_add(1),
        }
    }
    match rng.below(8) {
        0 => {
            let cut = rng.below(data.len() + 1);
            data.truncate(cut);
        }
        1 => {
            for _ in 0..rng.below(16) {
                data.push(rng.next() as u8);
            }
        }
        _ => {}
    }
    data
}

/// 破壊 fuzz の共通ドライバ。各入力で `run` を呼び、panic と巨大確保を失敗として数える。
/// 失敗があれば最初の数件（ケース番号・panic メッセージ）をまとめて panic する。
pub fn fuzz(label: &str, seed: u64, cases: usize, base: &[u8], run: impl Fn(&[u8])) {
    fuzz_with(label, seed, cases, |rng| mutate(base, rng), run)
}

/// `fuzz` の入力生成を差し替えられる版（CRC 再計算が要る形式など）。
pub fn fuzz_with(
    label: &str,
    seed: u64,
    cases: usize,
    mut gen: impl FnMut(&mut Lcg) -> Vec<u8>,
    run: impl Fn(&[u8]),
) {
    // 深追い用: NOIRIA_FUZZ_SCALE=20 で件数を 20 倍にする（既定の件数は CI 向けに数秒以内）。
    let scale: usize = std::env::var("NOIRIA_FUZZ_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let cases = cases * scale.max(1);
    let mut rng = Lcg(seed);
    let mut failures: Vec<String> = Vec::new();
    let mut n_fail = 0usize;
    let trace = std::env::var_os("NOIRIA_FUZZ_TRACE").is_some();
    for case in 0..cases {
        let input = gen(&mut rng);
        if trace {
            // 固まった入力を特定するため、実行前に現在の入力を書き出す（NOIRIA_FUZZ_TRACE=1 のときだけ）。
            let _ = std::fs::write(std::env::temp_dir().join("noiria_fuzz_current.bin"), &input);
            eprintln!("{label}: case {case} (len={})", input.len());
        }
        let _slow_probe = SlowProbe {
            trace,
            label,
            case,
            started: std::time::Instant::now(),
        };
        let started = std::time::Instant::now();
        let r = catch_unwind(AssertUnwindSafe(|| {
            with_alloc_limit(FUZZ_ALLOC_LIMIT, || run(&input))
        }));
        // 1 件が数秒かかる入力は、宣言された寸法・個数に比例した処理を許している（DoS）。
        let r = match r {
            Ok(()) if started.elapsed() > FUZZ_CASE_TIME_LIMIT => {
                Err(Box::new(format!("処理に {:?} かかった", started.elapsed()))
                    as Box<dyn std::any::Any + Send>)
            }
            other => other,
        };
        if let Err(e) = r {
            n_fail += 1;
            if failures.len() < 5 {
                let msg = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "(payload 不明)".to_string());
                failures.push(format!("case {case} (len={}): {msg}", input.len()));
            }
        }
    }
    assert!(
        n_fail == 0,
        "{label}: {n_fail}/{cases} 件で panic / 巨大確保\n{}",
        failures.join("\n")
    );
}

/// LSB ファースト（Vorbis 規約）のビット詰め。テストで最小限の setup ヘッダ等を手組みするための
/// 独立実装（デコーダ側の `BitReader` とは共有しない）。
#[derive(Default)]
pub struct LsbBits {
    bytes: Vec<u8>,
    nbits: usize,
}

impl LsbBits {
    pub fn push(&mut self, value: u64, n: u32) -> &mut Self {
        for i in 0..n {
            if self.nbits.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if (value >> i) & 1 != 0 {
                *self.bytes.last_mut().unwrap() |= 1 << (self.nbits % 8);
            }
            self.nbits += 1;
        }
        self
    }
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}
