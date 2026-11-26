//! 表示用の拡大縮小（BGRX/RGBA の u32 列）。OS の拡大転送（Windows の HALFTONE の `StretchDIBits`）は、等倍でない
//! 表示で 1 フレーム 15〜20ms かかり、メニューのようにフレームごとに描き直す画面を 30fps 以下にしていた。
//! ここは整数演算の双一次補間（2 チャンネルずつ SWAR）を行ごとに分けて常駐ワーカースレッドで並列に回す。
//! 1920x1080 へ 1 フレーム約 1〜2ms。画素の位置は中心合わせ（`(dx + 0.5) * sw / dw - 0.5`）。

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

const MAX_THREADS: usize = 16;

type AxisTap = (u32, u32, u32);

/// 2 画素を重み `w`（0..=256、`b` の割合）で混ぜる。4 チャンネルを 2 つずつ 16 ビット幅に載せて乗算する。
#[inline(always)]
fn lerp_px(a: u32, b: u32, w: u32) -> u32 {
    let iw = 256 - w;
    let rb = (((a & 0x00FF_00FF) * iw + (b & 0x00FF_00FF) * w + 0x0080_0080) >> 8) & 0x00FF_00FF;
    let ga =
        (((a >> 8) & 0x00FF_00FF) * iw + ((b >> 8) & 0x00FF_00FF) * w + 0x0080_0080) & 0xFF00_FF00;
    rb | ga
}

/// 出力 1 方向ぶんの標本位置（左の画素, 右の画素, 右の重み 0..=256）を `out` に書き込む。
fn fill_axis(dst: usize, src: usize, out: &mut Vec<AxisTap>) {
    out.clear();
    out.reserve(dst);
    for d in 0..dst {
        let pos = ((d as f64 + 0.5) * src as f64 / dst as f64 - 0.5).clamp(0.0, (src - 1) as f64);
        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(src - 1);
        out.push((
            i0 as u32,
            i1 as u32,
            ((pos - i0 as f64) * 256.0).round() as u32,
        ));
    }
}

#[derive(Default)]
struct AxisCache {
    x_key: (usize, usize),
    xs: Vec<AxisTap>,
    y_key: (usize, usize),
    ys: Vec<AxisTap>,
    #[cfg(test)]
    rebuilds: u32,
}

impl AxisCache {
    fn get(&mut self, sw: usize, sh: usize, dw: usize, dh: usize) -> (&[AxisTap], &[AxisTap]) {
        if self.x_key != (sw, dw) || self.xs.len() != dw {
            fill_axis(dw, sw, &mut self.xs);
            self.x_key = (sw, dw);
            #[cfg(test)]
            {
                self.rebuilds += 1;
            }
        }
        if self.y_key != (sh, dh) || self.ys.len() != dh {
            fill_axis(dh, sh, &mut self.ys);
            self.y_key = (sh, dh);
            #[cfg(test)]
            {
                self.rebuilds += 1;
            }
        }
        (&self.xs, &self.ys)
    }
}

/// 1 本の入力行 `src_row` を `xs` に従って水平補間し `dst` へ書き込む。
#[inline]
fn horiz_row(src_row: &[u32], xs: &[AxisTap], dst: &mut [u32]) {
    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        // SAFETY: AVX2 availability was checked above, and `xs` was built by `fill_axis`
        // for `src_row.len()`, so every `(x0, x1)` index is strictly `< src_row.len()`.
        unsafe { horiz_row_avx2(src_row, xs, dst) };
        return;
    }
    for (o, &(x0, x1, wx)) in dst.iter_mut().zip(xs.iter()) {
        *o = lerp_px(src_row[x0 as usize], src_row[x1 as usize], wx);
    }
}

/// 2 本の水平補間済み行 `r0`, `r1` を重み `wy` (0..=256) で垂直補間して `dst` へ書き込む。
#[inline]
fn vert_row(r0: &[u32], r1: &[u32], wy: u32, dst: &mut [u32]) {
    if wy == 0 {
        dst.copy_from_slice(r0);
        return;
    }
    if wy == 256 {
        dst.copy_from_slice(r1);
        return;
    }
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: AVX2 availability was checked above.
            unsafe { vert_row_avx2(r0, r1, wy, dst) };
        } else {
            // SAFETY: SSE2 is guaranteed on x86_64.
            unsafe { vert_row_sse2(r0, r1, wy, dst) };
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    vert_row_scalar(r0, r1, wy, dst);
}

#[inline]
fn vert_row_scalar(r0: &[u32], r1: &[u32], wy: u32, dst: &mut [u32]) {
    for ((o, &a), &b) in dst.iter_mut().zip(r0.iter()).zip(r1.iter()) {
        *o = lerp_px(a, b, wy);
    }
}

/// # Safety
/// - Caller must verify `is_x86_feature_detected!("avx2")`.
/// - Every `(x0, x1, _)` tap in `xs[..xs.len().min(dst.len())]` must satisfy
///   `(x0 as usize) < src_row.len()` and `(x1 as usize) < src_row.len()`.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn horiz_row_avx2(src_row: &[u32], xs: &[AxisTap], dst: &mut [u32]) {
    use std::arch::x86_64::*;

    let dw = xs.len().min(dst.len());
    debug_assert!(xs[..dw]
        .iter()
        .all(|&(x0, x1, _)| (x0 as usize) < src_row.len() && (x1 as usize) < src_row.len()));
    let src_ptr = src_row.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let v128 = _mm256_set1_epi16(128);
    let v256 = _mm256_set1_epi16(256);
    let mut i = 0usize;
    while i + 8 <= dw {
        let t0 = *xs.get_unchecked(i);
        let t1 = *xs.get_unchecked(i + 1);
        let t2 = *xs.get_unchecked(i + 2);
        let t3 = *xs.get_unchecked(i + 3);
        let t4 = *xs.get_unchecked(i + 4);
        let t5 = *xs.get_unchecked(i + 5);
        let t6 = *xs.get_unchecked(i + 6);
        let t7 = *xs.get_unchecked(i + 7);

        let a_lo = _mm256_cvtepu8_epi16(_mm_setr_epi32(
            *src_ptr.add(t0.0 as usize) as i32,
            *src_ptr.add(t1.0 as usize) as i32,
            *src_ptr.add(t2.0 as usize) as i32,
            *src_ptr.add(t3.0 as usize) as i32,
        ));
        let b_lo = _mm256_cvtepu8_epi16(_mm_setr_epi32(
            *src_ptr.add(t0.1 as usize) as i32,
            *src_ptr.add(t1.1 as usize) as i32,
            *src_ptr.add(t2.1 as usize) as i32,
            *src_ptr.add(t3.1 as usize) as i32,
        ));
        let w_lo = _mm256_setr_epi64x(
            (t0.2 as i64).wrapping_mul(0x0001_0001_0001_0001),
            (t1.2 as i64).wrapping_mul(0x0001_0001_0001_0001),
            (t2.2 as i64).wrapping_mul(0x0001_0001_0001_0001),
            (t3.2 as i64).wrapping_mul(0x0001_0001_0001_0001),
        );
        let iw_lo = _mm256_sub_epi16(v256, w_lo);
        let res_lo = _mm256_srli_epi16::<8>(_mm256_add_epi16(
            _mm256_add_epi16(
                _mm256_mullo_epi16(a_lo, iw_lo),
                _mm256_mullo_epi16(b_lo, w_lo),
            ),
            v128,
        ));

        let a_hi = _mm256_cvtepu8_epi16(_mm_setr_epi32(
            *src_ptr.add(t4.0 as usize) as i32,
            *src_ptr.add(t5.0 as usize) as i32,
            *src_ptr.add(t6.0 as usize) as i32,
            *src_ptr.add(t7.0 as usize) as i32,
        ));
        let b_hi = _mm256_cvtepu8_epi16(_mm_setr_epi32(
            *src_ptr.add(t4.1 as usize) as i32,
            *src_ptr.add(t5.1 as usize) as i32,
            *src_ptr.add(t6.1 as usize) as i32,
            *src_ptr.add(t7.1 as usize) as i32,
        ));
        let w_hi = _mm256_setr_epi64x(
            (t4.2 as i64).wrapping_mul(0x0001_0001_0001_0001),
            (t5.2 as i64).wrapping_mul(0x0001_0001_0001_0001),
            (t6.2 as i64).wrapping_mul(0x0001_0001_0001_0001),
            (t7.2 as i64).wrapping_mul(0x0001_0001_0001_0001),
        );
        let iw_hi = _mm256_sub_epi16(v256, w_hi);
        let res_hi = _mm256_srli_epi16::<8>(_mm256_add_epi16(
            _mm256_add_epi16(
                _mm256_mullo_epi16(a_hi, iw_hi),
                _mm256_mullo_epi16(b_hi, w_hi),
            ),
            v128,
        ));

        let packed = _mm256_permute4x64_epi64::<0xD8>(_mm256_packus_epi16(res_lo, res_hi));
        _mm256_storeu_si256(dst_ptr.add(i) as *mut __m256i, packed);
        i += 8;
    }
    while i < dw {
        let (x0, x1, wx) = xs[i];
        dst[i] = lerp_px(src_row[x0 as usize], src_row[x1 as usize], wx);
        i += 1;
    }
}

/// # Safety
/// Caller must verify `is_x86_feature_detected!("avx2")`.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn vert_row_avx2(r0: &[u32], r1: &[u32], wy: u32, dst: &mut [u32]) {
    use std::arch::x86_64::*;

    let len = dst.len().min(r0.len()).min(r1.len());
    let zero = _mm256_setzero_si256();
    let v_w = _mm256_set1_epi16(wy as i16);
    let v_iw = _mm256_set1_epi16((256 - wy) as i16);
    let v128 = _mm256_set1_epi16(128);
    let p0 = r0.as_ptr();
    let p1 = r1.as_ptr();
    let pd = dst.as_mut_ptr();
    let mut i = 0usize;
    while i + 8 <= len {
        let va = _mm256_loadu_si256(p0.add(i) as *const __m256i);
        let vb = _mm256_loadu_si256(p1.add(i) as *const __m256i);
        let a_lo = _mm256_unpacklo_epi8(va, zero);
        let a_hi = _mm256_unpackhi_epi8(va, zero);
        let b_lo = _mm256_unpacklo_epi8(vb, zero);
        let b_hi = _mm256_unpackhi_epi8(vb, zero);
        let lo = _mm256_srli_epi16::<8>(_mm256_add_epi16(
            _mm256_add_epi16(
                _mm256_mullo_epi16(a_lo, v_iw),
                _mm256_mullo_epi16(b_lo, v_w),
            ),
            v128,
        ));
        let hi = _mm256_srli_epi16::<8>(_mm256_add_epi16(
            _mm256_add_epi16(
                _mm256_mullo_epi16(a_hi, v_iw),
                _mm256_mullo_epi16(b_hi, v_w),
            ),
            v128,
        ));
        _mm256_storeu_si256(pd.add(i) as *mut __m256i, _mm256_packus_epi16(lo, hi));
        i += 8;
    }
    vert_row_scalar(&r0[i..len], &r1[i..len], wy, &mut dst[i..len]);
}

/// # Safety
/// Caller must ensure SSE2 is available (guaranteed on x86_64).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn vert_row_sse2(r0: &[u32], r1: &[u32], wy: u32, dst: &mut [u32]) {
    use std::arch::x86_64::*;

    let len = dst.len().min(r0.len()).min(r1.len());
    let zero = _mm_setzero_si128();
    let v_w = _mm_set1_epi16(wy as i16);
    let v_iw = _mm_set1_epi16((256 - wy) as i16);
    let v128 = _mm_set1_epi16(128);
    let p0 = r0.as_ptr();
    let p1 = r1.as_ptr();
    let pd = dst.as_mut_ptr();
    let mut i = 0usize;
    while i + 4 <= len {
        let va = _mm_loadu_si128(p0.add(i) as *const __m128i);
        let vb = _mm_loadu_si128(p1.add(i) as *const __m128i);
        let a_lo = _mm_unpacklo_epi8(va, zero);
        let a_hi = _mm_unpackhi_epi8(va, zero);
        let b_lo = _mm_unpacklo_epi8(vb, zero);
        let b_hi = _mm_unpackhi_epi8(vb, zero);
        let lo = _mm_srli_epi16::<8>(_mm_add_epi16(
            _mm_add_epi16(_mm_mullo_epi16(a_lo, v_iw), _mm_mullo_epi16(b_lo, v_w)),
            v128,
        ));
        let hi = _mm_srli_epi16::<8>(_mm_add_epi16(
            _mm_add_epi16(_mm_mullo_epi16(a_hi, v_iw), _mm_mullo_epi16(b_hi, v_w)),
            v128,
        ));
        _mm_storeu_si128(pd.add(i) as *mut __m128i, _mm_packus_epi16(lo, hi));
        i += 4;
    }
    vert_row_scalar(&r0[i..len], &r1[i..len], wy, &mut dst[i..len]);
}

thread_local! {
    static ROW_CACHE: RefCell<(Vec<u32>, Vec<u32>)> = const { RefCell::new((Vec::new(), Vec::new())) };
}

fn scale_rows(
    src: &[u32],
    sw: usize,
    xs: &[AxisTap],
    ys: &[AxisTap],
    y_start: usize,
    dst: &mut [u32],
) {
    let dw = xs.len();
    ROW_CACHE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let (ref mut h0, ref mut h1) = *guard;
        if h0.len() < dw {
            h0.resize(dw, 0);
            h1.resize(dw, 0);
        }
        let mut cached_y0 = usize::MAX;
        let mut cached_y1 = usize::MAX;

        for (r, row) in dst.chunks_exact_mut(dw).enumerate() {
            let (y0, y1, wy) = ys[y_start + r];
            let y0 = y0 as usize;
            let y1 = y1 as usize;

            if y0 != cached_y0 {
                if y0 == cached_y1 {
                    std::mem::swap(h0, h1);
                    std::mem::swap(&mut cached_y0, &mut cached_y1);
                } else {
                    horiz_row(&src[y0 * sw..][..sw], xs, &mut h0[..dw]);
                    cached_y0 = y0;
                }
            }

            if wy == 0 || y0 == y1 {
                row.copy_from_slice(&h0[..dw]);
            } else {
                if y1 != cached_y1 {
                    horiz_row(&src[y1 * sw..][..sw], xs, &mut h1[..dw]);
                    cached_y1 = y1;
                }
                vert_row(&h0[..dw], &h1[..dw], wy, row);
            }
        }
    });
}

#[derive(Clone, Copy)]
struct Job {
    src: *const u32,
    src_len: usize,
    sw: usize,
    xs: *const (u32, u32, u32),
    dw: usize,
    ys: *const (u32, u32, u32),
    dh: usize,
    /// `dst` starts at row `band` (band 0 is owned exclusively by the caller thread).
    dst_rest: *mut u32,
    band: usize,
    worker_bands: usize,
}

// SAFETY: pointers in `Job` are valid for the duration of a single dispatch while
// the caller blocks on `WaitGuard` until `remaining == 0`. Workers only read
// shared immutable slices (`src`, `xs`, `ys`) and write strictly disjoint row
// ranges within `dst_rest`.
unsafe impl Send for Job {}
// SAFETY: same disjointness and lifetime invariants as `Send` above.
unsafe impl Sync for Job {}

struct DispatchState {
    epoch: u64,
    job: Option<Job>,
    remaining: usize,
}

struct Shared {
    state: Mutex<DispatchState>,
    work_ready: Condvar,
    work_done: Condvar,
    dead: AtomicBool,
}

struct FinishGuard<'a> {
    shared: &'a Shared,
    completed: bool,
}

impl Drop for FinishGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.shared.dead.store(true, Ordering::Release);
        }
        let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        st.remaining -= 1;
        if st.remaining == 0 {
            self.shared.work_done.notify_one();
        }
    }
}

struct WaitGuard<'a>(&'a Shared);

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        let mut st = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        while st.remaining > 0 {
            st = self.0.work_done.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.job = None;
    }
}

fn worker_loop(id: usize, mut last_epoch: u64, shared: Arc<Shared>) {
    loop {
        let job = {
            let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while st.epoch == last_epoch {
                st = shared
                    .work_ready
                    .wait(st)
                    .unwrap_or_else(|e| e.into_inner());
            }
            last_epoch = st.epoch;
            match st.job {
                Some(j) if id < j.worker_bands => Some(j),
                _ => None,
            }
        };
        if let Some(job) = job {
            // Drop guard instead of catch_unwind: zero fast-path overhead, decrements
            // `remaining` for the in-flight dispatch, and marks the pool dead so future
            // calls fall back to single-threaded scaling instead of waiting on a dead worker.
            let mut finish = FinishGuard {
                shared: &shared,
                completed: false,
            };
            let y_start = (id + 1) * job.band;
            let y_end = ((id + 2) * job.band).min(job.dh);
            let rows = y_end - y_start;
            let rest_off = id * job.band * job.dw;
            // SAFETY: caller holds `PoolCaller` lock and `WaitGuard` until all workers finish.
            // `src_len >= sw * sh`, `xs.len() == dw`, `ys.len() == dh`, and each worker writes
            // a disjoint slice `[id * band * dw .. (y_end - band) * dw)` within `dst_rest`.
            let (src, xs, ys, dst) = unsafe {
                (
                    std::slice::from_raw_parts(job.src, job.src_len),
                    std::slice::from_raw_parts(job.xs, job.dw),
                    std::slice::from_raw_parts(job.ys, job.dh),
                    std::slice::from_raw_parts_mut(job.dst_rest.add(rest_off), rows * job.dw),
                )
            };
            scale_rows(src, job.sw, xs, ys, y_start, dst);
            finish.completed = true;
        }
    }
}

struct PoolCaller {
    axis: AxisCache,
    spawned: usize,
    shared: Arc<Shared>,
}

impl PoolCaller {
    fn new() -> Self {
        Self {
            axis: AxisCache::default(),
            spawned: 0,
            shared: Arc::new(Shared {
                state: Mutex::new(DispatchState {
                    epoch: 0,
                    job: None,
                    remaining: 0,
                }),
                work_ready: Condvar::new(),
                work_done: Condvar::new(),
                dead: AtomicBool::new(false),
            }),
        }
    }

    fn ensure_workers(&mut self, needed: usize) {
        if self.spawned >= needed {
            return;
        }
        let current_epoch = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .epoch;
        while self.spawned < needed {
            let id = self.spawned;
            let shared = Arc::clone(&self.shared);
            let res = std::thread::Builder::new()
                .name(format!("noiria-scale-{id}"))
                .spawn(move || worker_loop(id, current_epoch, shared));
            if res.is_ok() {
                self.spawned += 1;
            } else {
                break;
            }
        }
    }

    fn scale(
        &mut self,
        src: &[u32],
        (sw, sh): (usize, usize),
        dst: &mut [u32],
        (dw, dh): (usize, usize),
        threads: usize,
    ) {
        let wanted_threads = if self.shared.dead.load(Ordering::Acquire) {
            1
        } else {
            threads.clamp(1, MAX_THREADS).min(dh)
        };
        if wanted_threads > 1 {
            self.ensure_workers(wanted_threads - 1);
        }
        let usable_threads = wanted_threads.min(self.spawned + 1);
        let (xs, ys) = self.axis.get(sw, sh, dw, dh);
        let band = dh.div_ceil(usable_threads);
        let num_bands = dh.div_ceil(band);
        let active_workers = num_bands - 1;
        if active_workers == 0 {
            scale_rows(src, sw, xs, ys, 0, &mut dst[..dw * dh]);
            return;
        }
        // Split band 0 from the remaining bands so caller and workers have disjoint provenance.
        let (first_band, rest) = dst[..dw * dh].split_at_mut(band * dw);
        let job = Job {
            src: src.as_ptr(),
            src_len: sw * sh,
            sw,
            xs: xs.as_ptr(),
            dw,
            ys: ys.as_ptr(),
            dh,
            dst_rest: rest.as_mut_ptr(),
            band,
            worker_bands: active_workers,
        };
        {
            let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            st.job = Some(job);
            st.remaining = active_workers;
            st.epoch = st.epoch.wrapping_add(1);
        }
        self.shared.work_ready.notify_all();
        let _wait = WaitGuard(&self.shared);
        scale_rows(src, sw, xs, ys, 0, first_band);
    }
}

fn pool() -> &'static Mutex<PoolCaller> {
    static POOL: OnceLock<Mutex<PoolCaller>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(PoolCaller::new()))
}

/// `src`（`sw`x`sh`）を `dst`（`dw`x`dh`）へ双一次補間で拡縮する。`threads` > 1 なら行を分けて並列に回す。
pub fn scale_bilinear(
    src: &[u32],
    sw: usize,
    sh: usize,
    dst: &mut [u32],
    dw: usize,
    dh: usize,
    threads: usize,
) {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() < sw * sh || dst.len() < dw * dh {
        return;
    }
    if sw == dw && sh == dh {
        dst[..dw * dh].copy_from_slice(&src[..sw * sh]);
        return;
    }
    let mut guard = pool().lock().unwrap_or_else(|e| e.into_inner());
    guard.scale(src, (sw, sh), dst, (dw, dh), threads);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(r: u32, g: u32, b: u32, a: u32) -> u32 {
        (a << 24) | (r << 16) | (g << 8) | b
    }

    fn naive(
        src: &[u32],
        sw: usize,
        sh: usize,
        dw: usize,
        dh: usize,
        x: usize,
        y: usize,
    ) -> [f64; 4] {
        let fx = ((x as f64 + 0.5) * sw as f64 / dw as f64 - 0.5).clamp(0.0, (sw - 1) as f64);
        let fy = ((y as f64 + 0.5) * sh as f64 / dh as f64 - 0.5).clamp(0.0, (sh - 1) as f64);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
        let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
        let mut out = [0.0; 4];
        for (c, o) in out.iter_mut().enumerate() {
            let ch = |p: u32| ((p >> (8 * c)) & 255) as f64;
            let top = ch(src[y0 * sw + x0]) * (1.0 - tx) + ch(src[y0 * sw + x1]) * tx;
            let bot = ch(src[y1 * sw + x0]) * (1.0 - tx) + ch(src[y1 * sw + x1]) * tx;
            *o = top * (1.0 - ty) + bot * ty;
        }
        out
    }

    #[test]
    fn same_size_is_a_copy_and_a_flat_colour_stays_flat() {
        let src: Vec<u32> = (0..12).map(|i| pack(i, 255 - i, i * 7, 255)).collect();
        let mut dst = vec![0; 12];
        scale_bilinear(&src, 4, 3, &mut dst, 4, 3, 2);
        assert_eq!(src, dst);
        let flat = vec![pack(10, 200, 99, 255); 20];
        let mut big = vec![0; 7 * 9];
        scale_bilinear(&flat, 5, 4, &mut big, 7, 9, 3);
        assert!(big.iter().all(|&p| p == pack(10, 200, 99, 255)));
    }

    #[test]
    fn matches_a_float_bilinear_within_a_level_and_a_half_and_is_thread_count_independent() {
        let (sw, sh, dw, dh) = (16usize, 9usize, 24usize, 14usize);
        let src: Vec<u32> = (0..sw * sh)
            .map(|i| {
                pack(
                    (i * 37 % 256) as u32,
                    (i * 91 % 256) as u32,
                    (i * 13 % 256) as u32,
                    255,
                )
            })
            .collect();
        let mut one = vec![0; dw * dh];
        let mut four = vec![0; dw * dh];
        scale_bilinear(&src, sw, sh, &mut one, dw, dh, 1);
        scale_bilinear(&src, sw, sh, &mut four, dw, dh, 4);
        assert_eq!(one, four, "行の分け方で結果が変わらない");
        for y in 0..dh {
            for x in 0..dw {
                let want = naive(&src, sw, sh, dw, dh, x, y);
                let got = one[y * dw + x];
                for (c, w) in want.iter().enumerate() {
                    let g = ((got >> (8 * c)) & 255) as f64;
                    assert!((g - w).abs() <= 1.5, "({x},{y}) ch{c}: {g} vs {w}");
                }
            }
        }
    }

    #[test]
    fn axis_cache_reuses_tables_without_rebuilding_on_same_dimensions() {
        let mut cache = AxisCache::default();
        let (x_ptr, y_ptr) = {
            let (xs, ys) = cache.get(1280, 720, 1920, 1080);
            assert_eq!((xs.len(), ys.len()), (1920, 1080));
            (xs.as_ptr(), ys.as_ptr())
        };
        assert_eq!(cache.rebuilds, 2);

        for _ in 0..5 {
            let (xs, ys) = cache.get(1280, 720, 1920, 1080);
            assert_eq!(xs.as_ptr(), x_ptr);
            assert_eq!(ys.as_ptr(), y_ptr);
        }
        assert_eq!(cache.rebuilds, 2, "同サイズではテーブルを再構築しない");

        // 幅だけ変えると X のみ再構築し、Y は再利用する
        let (_, y_ptr2) = {
            let (xs, ys) = cache.get(1280, 720, 1600, 1080);
            assert_eq!(xs.len(), 1600);
            (xs.as_ptr(), ys.as_ptr())
        };
        assert_eq!(y_ptr2, y_ptr);
        assert_eq!(cache.rebuilds, 3);
    }

    #[test]
    fn all_thread_counts_and_edge_dimensions_match_single_thread() {
        let cases = [
            (1usize, 1usize, 3usize, 2usize),
            (3, 2, 1, 1),
            (16, 9, 24, 5),
            (16, 9, 24, 14),
            (33, 17, 19, 29),
            (17, 1, 5, 1),
            (1, 17, 1, 5),
        ];
        for &(sw, sh, dw, dh) in &cases {
            let src: Vec<u32> = (0..sw * sh)
                .map(|i| {
                    pack(
                        (i * 53 % 256) as u32,
                        (i * 97 % 256) as u32,
                        (i * 29 % 256) as u32,
                        255,
                    )
                })
                .collect();
            let mut single = vec![0u32; dw * dh];
            scale_bilinear(&src, sw, sh, &mut single, dw, dh, 1);
            for threads in [2usize, 3, 4, 5, 8, 16] {
                let mut multi = vec![0u32; dw * dh];
                scale_bilinear(&src, sw, sh, &mut multi, dw, dh, threads);
                assert_eq!(multi, single, "{sw}x{sh}->{dw}x{dh} threads={threads}");
            }
        }
    }

    #[test]
    fn concurrent_callers_produce_exact_results_and_reuse_pool() {
        let cases = [
            (16usize, 9usize, 24usize, 14usize, 4usize),
            (20, 15, 35, 22, 3),
            (12, 8, 9, 17, 2),
            (25, 11, 40, 19, 4),
        ];
        let handles: Vec<_> = cases
            .into_iter()
            .map(|(sw, sh, dw, dh, threads)| {
                std::thread::spawn(move || {
                    let src: Vec<u32> = (0..sw * sh)
                        .map(|i| {
                            pack(
                                (i * 41 % 256) as u32,
                                (i * 73 % 256) as u32,
                                (i * 19 % 256) as u32,
                                255,
                            )
                        })
                        .collect();
                    let mut want = vec![0u32; dw * dh];
                    scale_bilinear(&src, sw, sh, &mut want, dw, dh, 1);
                    let mut got = vec![0u32; dw * dh];
                    for _ in 0..20 {
                        scale_bilinear(&src, sw, sh, &mut got, dw, dh, threads);
                        assert_eq!(got, want);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert!(pool().lock().unwrap().spawned < MAX_THREADS);
    }

    #[test]
    fn dead_worker_marks_pool_and_falls_back_to_single_threaded_without_deadlock() {
        let mut caller = PoolCaller::new();
        caller.spawned = 3; // pretend 3 workers were spawned (none actually running)
        caller.shared.state.lock().unwrap().remaining = 1;
        let shared = Arc::clone(&caller.shared);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _finish = FinishGuard {
                shared: &shared,
                completed: false,
            };
            panic!("simulated worker panic");
        }));
        assert!(caller.shared.dead.load(Ordering::Acquire));
        assert_eq!(caller.shared.state.lock().unwrap().remaining, 0);

        let (sw, sh, dw, dh) = (8usize, 6usize, 12usize, 10usize);
        let src: Vec<u32> = (0..sw * sh)
            .map(|i| pack(i as u32 * 5, 100, 50, 255))
            .collect();
        let mut want = vec![0u32; dw * dh];
        scale_bilinear(&src, sw, sh, &mut want, dw, dh, 1);

        let mut got = vec![0u32; dw * dh];
        caller.scale(&src, (sw, sh), &mut got, (dw, dh), 4);
        assert_eq!(got, want);
    }

    /// `cargo test --release -p noiria-core scale_cost -- --ignored --nocapture`: 拡大先ごとの所要時間（ms）。
    #[test]
    #[ignore]
    fn scale_cost() {
        let (sw, sh) = (1280usize, 720usize);
        let src: Vec<u32> = (0..sw * sh)
            .map(|i| (i as u32).wrapping_mul(2654435761))
            .collect();
        for (dw, dh) in [(1920usize, 1080usize), (2560, 1440), (3840, 2160)] {
            let mut dst = vec![0u32; dw * dh];
            for threads in [1usize, 4] {
                let t = std::time::Instant::now();
                for _ in 0..10 {
                    scale_bilinear(&src, sw, sh, &mut dst, dw, dh, threads);
                }
                println!(
                    "{dw}x{dh} threads={threads}: {:.2} ms",
                    t.elapsed().as_secs_f64() * 100.0
                );
            }
        }
    }

    /// 目視確認用: `SCALE_IN=<png> SCALE_OUT=<png> SCALE_W=1920 SCALE_H=1080 cargo test -p noiria-core scale_png -- --ignored`
    #[test]
    #[ignore]
    fn scale_png() {
        let (Ok(i), Ok(o)) = (std::env::var("SCALE_IN"), std::env::var("SCALE_OUT")) else {
            return;
        };
        let (dw, dh): (usize, usize) = (
            std::env::var("SCALE_W")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1920),
            std::env::var("SCALE_H")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1080),
        );
        let png = crate::codec::png::decode(&std::fs::read(i).unwrap()).unwrap();
        let src: Vec<u32> = png
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| pack(p[0] as u32, p[1] as u32, p[2] as u32, 255))
            .collect();
        let mut dst = vec![0u32; dw * dh];
        scale_bilinear(
            &src,
            png.width as usize,
            png.height as usize,
            &mut dst,
            dw,
            dh,
            4,
        );
        let mut out = crate::pixmap::Pixmap::new(dw as u32, dh as u32);
        for (px, d) in dst.iter().zip(out.data.as_chunks_mut::<4>().0) {
            d.copy_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, *px as u8, 255]);
        }
        crate::assets::save_png(&out, std::path::Path::new(&o)).unwrap();
    }
}
