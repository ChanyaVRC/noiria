//! Unix (Linux/ALSA) 専用: `libasound` を **dlopen で実行時ロード**して鳴らす
//! 音声出力。`wasapi_out.rs` と同格の OS 境界 API 呼び出し（外部オーディオ
//! クレート = cpal/rodio 等は使わない）だが、こちらは **リンク時には
//! libasound に一切依存しない**（`Cargo.toml` に alsa-sys 等を追加しない）。
//! ライブラリが存在しない環境（サーバ版 Linux 等）でもビルド・起動でき、
//! `open_default()` が `Err` を返すだけで済むようにするため。
//!
//! 呼び出し規約は `wasapi_out.rs` に合わせてある: `open_default() / channels()
//! / sample_rate() / run(stop, fill)` の 4 メソッド。`run` は 512 フレーム
//! 単位で `fill` を呼び、ブロッキング `snd_pcm_writei` へ書き込む（writei の
//! ブロッキングそのものがペーシングを兼ねる）。デバイス喪失（`-ENODEV`）や
//! 連続失敗は `run` が `Err` で返し、開き直しは呼び出し元（`audio.rs`）が行う。
//!
//! シンボルは全て `dlsym` 経由の関数ポインタとして解決し、`snd_pcm_t` /
//! `snd_pcm_hw_params_t` は中身を一切覗かない opaque ポインタとして扱う
//! （構造体レイアウトに依存しない = ALSA のバージョン差異に強い）。

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};

extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *mut c_char;
}

const RTLD_NOW: c_int = 2;

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
const SND_PCM_FORMAT_S16_LE: c_int = 2;
const SND_PCM_FORMAT_FLOAT_LE: c_int = 14;

// Linux errno（ALSA の負の戻り値は -errno）。alsa_out.rs は cfg(unix) だが
// 実際に到達するのは libasound が見つかった場合のみ = 事実上 Linux のみ。
const EPIPE: c_long = -32;
const ESTRPIPE: c_long = -86;
const ENODEV: c_long = -19;

/// 中身を見ない opaque 型。
enum SndPcmT {}
enum SndPcmHwParamsT {}

type FnPcmOpen = unsafe extern "C" fn(*mut *mut SndPcmT, *const c_char, c_int, c_int) -> c_int;
type FnPcmClose = unsafe extern "C" fn(*mut SndPcmT) -> c_int;
type FnHwParamsMalloc = unsafe extern "C" fn(*mut *mut SndPcmHwParamsT) -> c_int;
type FnHwParamsFree = unsafe extern "C" fn(*mut SndPcmHwParamsT);
type FnHwParamsAny = unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT) -> c_int;
type FnSetAccess = unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT, c_int) -> c_int;
type FnSetFormat = unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT, c_int) -> c_int;
type FnSetChannelsNear =
    unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT, *mut c_uint) -> c_int;
type FnSetRateNear =
    unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT, *mut c_uint, *mut c_int) -> c_int;
type FnHwParamsCommit = unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT) -> c_int;
type FnGetChannels = unsafe extern "C" fn(*const SndPcmHwParamsT, *mut c_uint) -> c_int;
type FnGetRate = unsafe extern "C" fn(*const SndPcmHwParamsT, *mut c_uint, *mut c_int) -> c_int;
type FnPrepare = unsafe extern "C" fn(*mut SndPcmT) -> c_int;
type FnWritei = unsafe extern "C" fn(*mut SndPcmT, *const c_void, c_ulong) -> c_long;
type FnDrain = unsafe extern "C" fn(*mut SndPcmT) -> c_int;
type FnStrerror = unsafe extern "C" fn(c_int) -> *const c_char;

/// dlsym で解決した関数ポインタ一式。全てただの関数ポインタ (Copy) なので
/// `AlsaOutput` へそのまま埋め込める。
#[derive(Clone, Copy)]
struct Symbols {
    open: FnPcmOpen,
    close: FnPcmClose,
    hw_params_malloc: FnHwParamsMalloc,
    hw_params_free: FnHwParamsFree,
    hw_params_any: FnHwParamsAny,
    set_access: FnSetAccess,
    set_format: FnSetFormat,
    set_channels_near: FnSetChannelsNear,
    set_rate_near: FnSetRateNear,
    hw_params_commit: FnHwParamsCommit,
    get_channels: FnGetChannels,
    get_rate: FnGetRate,
    prepare: FnPrepare,
    writei: FnWritei,
    drain: FnDrain,
    strerror: FnStrerror,
}

/// `dlsym` の結果を型 `$ty`（`unsafe extern "C" fn` 型）の関数ポインタとして受け取る。
/// null は変換の前に `Err` にする。`transmute::<*mut c_void, $ty>` はサイズ不一致なら
/// コンパイルエラーになるので、シンボルごとに型付きで「サイズが合う関数ポインタ以外」を
/// 受けることはない。
macro_rules! load_symbol {
    ($handle:expr, $name:literal, $ty:ty) => {{
        // SAFETY: 呼び出し元 (`Symbols::load`) が `handle` の有効性を保証する。
        let ptr = unsafe { dlsym_nonnull($handle, $name)? };
        // SAFETY: `ptr` は libasound の `$name` の関数アドレス（null でないことを確認済み）。
        // `$ty` はその C 関数の実際のシグネチャ（ALSA の公開ヘッダどおり）で、
        // ポインタ幅同士の transmute（サイズはコンパイル時に検査される）。
        unsafe { std::mem::transmute::<*mut c_void, $ty>(ptr) }
    }};
}

impl Symbols {
    /// # Safety
    /// `handle` は `dlopen("libasound.so*")` が返した有効な共有ライブラリハンドルであること。
    unsafe fn load(handle: *mut c_void) -> Result<Self> {
        Ok(Self {
            open: load_symbol!(handle, c"snd_pcm_open", FnPcmOpen),
            close: load_symbol!(handle, c"snd_pcm_close", FnPcmClose),
            hw_params_malloc: load_symbol!(handle, c"snd_pcm_hw_params_malloc", FnHwParamsMalloc),
            hw_params_free: load_symbol!(handle, c"snd_pcm_hw_params_free", FnHwParamsFree),
            hw_params_any: load_symbol!(handle, c"snd_pcm_hw_params_any", FnHwParamsAny),
            set_access: load_symbol!(handle, c"snd_pcm_hw_params_set_access", FnSetAccess),
            set_format: load_symbol!(handle, c"snd_pcm_hw_params_set_format", FnSetFormat),
            set_channels_near: load_symbol!(
                handle,
                c"snd_pcm_hw_params_set_channels_near",
                FnSetChannelsNear
            ),
            set_rate_near: load_symbol!(handle, c"snd_pcm_hw_params_set_rate_near", FnSetRateNear),
            hw_params_commit: load_symbol!(handle, c"snd_pcm_hw_params", FnHwParamsCommit),
            get_channels: load_symbol!(handle, c"snd_pcm_hw_params_get_channels", FnGetChannels),
            get_rate: load_symbol!(handle, c"snd_pcm_hw_params_get_rate", FnGetRate),
            prepare: load_symbol!(handle, c"snd_pcm_prepare", FnPrepare),
            writei: load_symbol!(handle, c"snd_pcm_writei", FnWritei),
            drain: load_symbol!(handle, c"snd_pcm_drain", FnDrain),
            strerror: load_symbol!(handle, c"snd_strerror", FnStrerror),
        })
    }
}

/// `dlsym` で `name` を引き、null なら `Err`（null を関数ポインタにしてはならない）。
///
/// # Safety
/// `handle` は `dlopen` が返した有効なハンドルであること。
unsafe fn dlsym_nonnull(handle: *mut c_void, name: &CStr) -> Result<*mut c_void> {
    // SAFETY: `handle` の有効性は呼び出し元が保証し、`name` は NUL 終端の `CStr`。
    let ptr = unsafe { dlsym(handle, name.as_ptr()) };
    ensure!(
        !ptr.is_null(),
        "ALSA: シンボル '{}' が見つかりません（dlerror: {}）",
        name.to_string_lossy(),
        last_dlerror()
    );
    Ok(ptr)
}

fn last_dlerror() -> String {
    // SAFETY: 引数なし。戻り値は次の dl* 呼び出しまで有効な NUL 終端文字列か NULL で、
    // 直下で null 検査し、同じ式の中で所有文字列へコピーする（間に dl* を挟まない）。
    let ptr = unsafe { dlerror() };
    if ptr.is_null() {
        "詳細不明".to_string()
    } else {
        // SAFETY: 上記のとおり非 null・NUL 終端で、コピーまで有効。
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

fn strerror(syms: &Symbols, err: c_int) -> String {
    // SAFETY: snd_strerror は任意の整数を受け、静的に確保された文字列（または NULL）を返す。
    let ptr = unsafe { (syms.strerror)(err) };
    if ptr.is_null() {
        format!("errno={err}")
    } else {
        // SAFETY: 非 null で NUL 終端の静的文字列。
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

/// `libasound.so.2`（無ければ `libasound.so`）を dlopen する。
/// 見つからない環境では `Err` を返す（呼び出し元は無音続行にフォールバック）。
fn load_libasound() -> Result<(*mut c_void, &'static str)> {
    for (cname, name) in [
        (c"libasound.so.2", "libasound.so.2"),
        (c"libasound.so", "libasound.so"),
    ] {
        // SAFETY: `cname` は NUL 終端の静的 C 文字列。flag は有効な RTLD_NOW。
        let handle = unsafe { dlopen(cname.as_ptr(), RTLD_NOW) };
        if !handle.is_null() {
            return Ok((handle, name));
        }
    }
    bail!(
        "libasound (ALSA) をロードできません（dlerror: {}）。\
         音声デバイスの無い/ALSA が入っていない環境では無音で続行します",
        last_dlerror()
    );
}

/// ロード済みのシンボル表。成功した結果だけを覚える（開き直しのたびに dlopen の
/// 参照カウントを増やさない。失敗は覚えないので、後からライブラリが入れば拾える）。
static SYMBOLS: OnceLock<(Symbols, &'static str)> = OnceLock::new();

fn symbols() -> Result<(Symbols, &'static str)> {
    if let Some(s) = SYMBOLS.get() {
        return Ok(*s);
    }
    let (handle, lib_name) = load_libasound()?;
    // SAFETY: `handle` は直前の `load_libasound` が返した非 null の libasound ハンドル。
    let loaded = (unsafe { Symbols::load(handle) }?, lib_name);
    Ok(*SYMBOLS.get_or_init(|| loaded))
}

/// 使用するデバイス名。`NOIRIA_ALSA_DEVICE` があればそれを優先し、
/// 無ければ `"default"`（PulseAudio/PipeWire の ALSA ブリッジ経由で通常の
/// デスクトップでも鳴る）。テスト・検証では `"null"`（ALSA null デバイス、
/// 何も鳴らさない）を明示的に指定する。
fn device_name() -> String {
    std::env::var("NOIRIA_ALSA_DEVICE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

/// `write_device_buffer` 用のサンプルフォーマット（`wasapi_out.rs` と同型）。
#[derive(Clone, Copy)]
enum DeviceFormat {
    F32,
    I16,
}

/// hw_params の `Drop` ガード。`ensure!`/`?` による早期リターンでも必ず
/// `snd_pcm_hw_params_free` される。
struct HwParamsGuard {
    free_fn: FnHwParamsFree,
    params: *mut SndPcmHwParamsT,
}
impl Drop for HwParamsGuard {
    fn drop(&mut self) {
        if !self.params.is_null() {
            // SAFETY: `params` は snd_pcm_hw_params_malloc が返した非 null のポインタで、
            // このガードだけが所有し、ここで 1 回だけ解放する。
            unsafe { (self.free_fn)(self.params) };
        }
    }
}

/// ALSA (`libasound`, dlopen) の出力ストリーム。
pub struct AlsaOutput {
    pcm: *mut SndPcmT,
    syms: Symbols,
    channels: u16,
    sample_rate: u32,
    device_format: DeviceFormat,
}

// SAFETY: `wasapi_out::WasapiOutput` と同じ運用規約 —「生成したスレッドで
// 生成し、同じスレッド内で使い切って drop する」に限定する。COM のような
// アパートメント制約は無いが、それでも呼び出し元（render 専用スレッド）に
// move して閉じ込めるためだけに Send を実装する。
unsafe impl Send for AlsaOutput {}

impl AlsaOutput {
    pub fn channels(&self) -> u16 {
        self.channels
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// デバイスを開く。`NOIRIA_ALSA_DEVICE`（既定 `"default"`）を
    /// RW_INTERLEAVED・FLOAT_LE（ダメなら S16_LE）・ch=2 近傍・rate=48000 近傍
    /// で構成する。libasound が無い/デバイスが開けない環境では `Err` を返す
    /// （呼び出し元は「無音で続行」にフォールバックすればよい）。
    pub fn open_default() -> Result<Self> {
        Self::open_device(&device_name())
    }

    fn open_device(device: &str) -> Result<Self> {
        let (syms, lib_name) = symbols()?;

        let device_c =
            CString::new(device).with_context(|| format!("デバイス名が不正です: {device}"))?;

        let mut pcm: *mut SndPcmT = std::ptr::null_mut();
        // SAFETY: `&mut pcm` は書き込み可能な出力先、`device_c` は NUL 終端で呼び出しの間
        // 生きている。成功時に返る `pcm` はこのスコープが所有し、必ず close する。
        let ret = unsafe { (syms.open)(&mut pcm, device_c.as_ptr(), SND_PCM_STREAM_PLAYBACK, 0) };
        ensure!(
            ret == 0,
            "snd_pcm_open(\"{device}\") に失敗: {}（{lib_name} 自体はロードできています）",
            strerror(&syms, ret)
        );
        ensure!(!pcm.is_null(), "snd_pcm_open が null を返しました");

        // SAFETY: `pcm` は上で open に成功した非 null ハンドル。失敗時は直下で必ず close する。
        let result = unsafe { Self::configure(pcm, syms) };
        if let Err(e) = &result {
            // configure 失敗時は pcm を必ず閉じる（configure 側は params の
            // free のみ責務を持ち、pcm のクローズはこちらの責務にする）。
            // SAFETY: `pcm` は上で open に成功した有効なハンドルで、`Self` に
            // 渡されていない（= Drop で二重に閉じない）ここで 1 回だけ閉じる。
            unsafe { (syms.close)(pcm) };
            log::debug!("ALSA hw_params 構成に失敗: {e:#}");
        }
        result
    }

    /// # Safety
    /// `pcm` は `snd_pcm_open` が返した有効な非 null ハンドルで、呼び出し中生存すること。
    /// 成功時は `Self` が所有権を引き取り、失敗時は呼び出し元が `close` すること。
    unsafe fn configure(pcm: *mut SndPcmT, syms: Symbols) -> Result<Self> {
        let mut params: *mut SndPcmHwParamsT = std::ptr::null_mut();
        // SAFETY: `&mut params` は書き込み可能な出力先。
        let ret = unsafe { (syms.hw_params_malloc)(&mut params) };
        ensure!(
            ret == 0,
            "snd_pcm_hw_params_malloc に失敗: {}",
            strerror(&syms, ret)
        );
        ensure!(!params.is_null(), "snd_pcm_hw_params_malloc が null を返しました");
        let _params_guard = HwParamsGuard {
            free_fn: syms.hw_params_free,
            params,
        };

        // 以降の呼び出しの共通の前提（SAFETY）: `pcm` は open 済みの有効なハンドル
        // （呼び出し元が configure の間だけ生存を保証）、`params` は上で確保した非 null で
        // `_params_guard` がこの関数の終わりまで解放しない。`&mut x` の出力先は
        // いずれも呼び出しの間生きているローカル変数。
        // SAFETY: 上記の共通の前提。
        let ret = unsafe { (syms.hw_params_any)(pcm, params) };
        ensure!(
            ret == 0,
            "snd_pcm_hw_params_any に失敗: {}",
            strerror(&syms, ret)
        );

        // SAFETY: 上記の共通の前提。
        let ret = unsafe { (syms.set_access)(pcm, params, SND_PCM_ACCESS_RW_INTERLEAVED) };
        ensure!(
            ret == 0,
            "snd_pcm_hw_params_set_access(RW_INTERLEAVED) に失敗: {}",
            strerror(&syms, ret)
        );

        // FLOAT_LE 優先、ダメなら S16_LE にフォールバック（wasapi_out.rs の
        // DeviceFormat 分岐と同じ考え方）。
        let mut device_format = DeviceFormat::F32;
        // SAFETY: 上記の共通の前提。
        let ret_f32 = unsafe { (syms.set_format)(pcm, params, SND_PCM_FORMAT_FLOAT_LE) };
        if ret_f32 != 0 {
            // SAFETY: 上記の共通の前提。
            let ret_i16 = unsafe { (syms.set_format)(pcm, params, SND_PCM_FORMAT_S16_LE) };
            ensure!(
                ret_i16 == 0,
                "FLOAT_LE ({}) / S16_LE ({}) いずれのフォーマット設定も失敗しました",
                strerror(&syms, ret_f32),
                strerror(&syms, ret_i16)
            );
            device_format = DeviceFormat::I16;
        }

        let mut channels: c_uint = 2;
        // SAFETY: 上記の共通の前提。`&mut channels` は有効な c_uint。
        let ret = unsafe { (syms.set_channels_near)(pcm, params, &mut channels) };
        ensure!(
            ret == 0,
            "snd_pcm_hw_params_set_channels_near に失敗: {}",
            strerror(&syms, ret)
        );

        let mut rate: c_uint = 48_000;
        let mut dir: c_int = 0;
        // SAFETY: 上記の共通の前提。`&mut rate`/`&mut dir` は有効な出力先。
        let ret = unsafe { (syms.set_rate_near)(pcm, params, &mut rate, &mut dir) };
        ensure!(
            ret == 0,
            "snd_pcm_hw_params_set_rate_near に失敗: {}",
            strerror(&syms, ret)
        );

        // SAFETY: 上記の共通の前提。
        let ret = unsafe { (syms.hw_params_commit)(pcm, params) };
        ensure!(
            ret == 0,
            "snd_pcm_hw_params (commit) に失敗: {}",
            strerror(&syms, ret)
        );

        let mut out_channels: c_uint = 0;
        // SAFETY: 上記の共通の前提。commit 済みの `params` から値を読むだけ。
        let ret = unsafe { (syms.get_channels)(params, &mut out_channels) };
        ensure!(
            ret == 0,
            "snd_pcm_hw_params_get_channels に失敗: {}",
            strerror(&syms, ret)
        );
        let mut out_rate: c_uint = 0;
        let mut out_dir: c_int = 0;
        // SAFETY: 上記の共通の前提。
        let ret = unsafe { (syms.get_rate)(params, &mut out_rate, &mut out_dir) };
        ensure!(
            ret == 0,
            "snd_pcm_hw_params_get_rate に失敗: {}",
            strerror(&syms, ret)
        );

        // params はここで用済み（_params_guard の Drop で free される）。

        // SAFETY: `pcm` は open 済みで hw_params を commit した有効なハンドル。
        let ret = unsafe { (syms.prepare)(pcm) };
        ensure!(ret == 0, "snd_pcm_prepare に失敗: {}", strerror(&syms, ret));

        ensure!(out_channels >= 1, "channels が不正です: {out_channels}");
        ensure!(out_rate > 0, "sample rate が不正です: {out_rate}");

        Ok(Self {
            pcm,
            syms,
            channels: out_channels as u16,
            sample_rate: out_rate,
            device_format,
        })
    }

    /// `fill` で 512 フレーム単位のバッファを埋めては `snd_pcm_writei` へ
    /// ブロッキング書き込みする、を `stop` が立つまで繰り返す
    /// （writei 自体のブロッキングがペーシングを兼ねる）。
    ///
    /// アンダーラン (`-EPIPE`) は `snd_pcm_prepare` して同バッファを再送。
    /// `-ESTRPIPE`（サスペンド）はリトライ上限付きで少し待って再送。
    /// `-ENODEV`（デバイス喪失）は即 `Err`。それ以外の失敗も含め連続失敗が上限を
    /// 超えたらループを終了する（`wasapi_out.rs` の「デバイスエラーで無限リトライしない」
    /// 方針と同じ。`Err` を受けた呼び出し元が開き直す）。
    pub fn run(self, stop: &AtomicBool, fill: impl FnMut(&mut [f32])) -> Result<()> {
        let mut io = RealPcm {
            syms: &self.syms,
            pcm: self.pcm,
            channels: self.channels as usize,
        };
        let result = pump(
            &mut io,
            self.channels as usize,
            self.device_format,
            stop,
            fill,
        );
        if result.is_ok() {
            // 停止時、バッファ済みぶん（無音のことが多い）を吐き切ってから閉じる。
            // 失敗で抜けたとき（デバイス喪失など）は drain しない（待たされうる）。
            // SAFETY: `self.pcm` は open 済みの有効なハンドル（Drop まで生きている）。
            unsafe { (self.syms.drain)(self.pcm) };
        }
        result
    }
}

impl Drop for AlsaOutput {
    fn drop(&mut self) {
        if !self.pcm.is_null() {
            // SAFETY: `pcm` は open に成功した有効なハンドルで、ここで 1 回だけ閉じて null にする。
            unsafe { (self.syms.close)(self.pcm) };
            self.pcm = std::ptr::null_mut();
        }
    }
}

enum PcmChunk<'a> {
    F32(&'a [f32]),
    I16(&'a [i16]),
}

/// `pump` が PCM に対して行う操作（本物は libasound、テストは偽物）。
trait PcmIo {
    /// 先頭 `frames` フレーム分を書く。
    fn writei(&mut self, chunk: PcmChunk<'_>, frames: usize) -> c_long;
    fn prepare(&mut self) -> c_int;
    fn strerror(&self, err: c_int) -> String;
}

struct RealPcm<'a> {
    syms: &'a Symbols,
    pcm: *mut SndPcmT,
    channels: usize,
}

impl PcmIo for RealPcm<'_> {
    fn writei(&mut self, chunk: PcmChunk<'_>, frames: usize) -> c_long {
        let needed = frames * self.channels;
        let ptr = match chunk {
            PcmChunk::F32(s) => {
                assert!(s.len() >= needed);
                s.as_ptr() as *const c_void
            }
            PcmChunk::I16(s) => {
                assert!(s.len() >= needed);
                s.as_ptr() as *const c_void
            }
        };
        // SAFETY: `pcm` は open 済みの有効なハンドル。上でスライス長が `frames * channels`
        // 以上であることを検証済みのため、`snd_pcm_writei` が読む範囲はすべて有効。
        unsafe { (self.syms.writei)(self.pcm, ptr, frames as c_ulong) }
    }
    fn prepare(&mut self) -> c_int {
        // SAFETY: `pcm` は open 済みの有効なハンドル。
        unsafe { (self.syms.prepare)(self.pcm) }
    }
    fn strerror(&self, err: c_int) -> String {
        strerror(self.syms, err)
    }
}

/// 書き込み失敗（`snd_pcm_writei` の負の戻り値）の扱い。
#[derive(Debug, PartialEq, Eq)]
enum WriteFailure {
    /// アンダーラン: prepare して再送。
    Underrun,
    /// サスペンド: 少し待ってから prepare して再送。
    Suspended,
    /// デバイスが無くなった: 再送せず `Err`。
    DeviceGone,
    /// 未知: prepare で復帰を試み、連続するなら上限で終了。
    Other,
}

fn classify_write_failure(ret: c_long) -> WriteFailure {
    match ret {
        EPIPE => WriteFailure::Underrun,
        ESTRPIPE => WriteFailure::Suspended,
        ENODEV => WriteFailure::DeviceGone,
        _ => WriteFailure::Other,
    }
}

const CHUNK_FRAMES: usize = 512;
const MAX_CONSECUTIVE_FAILURES: u32 = 8;
/// `writei` が 0 フレームを返し続けても進まないときに諦める回数。
const MAX_ZERO_WRITES: u32 = 1000;

fn pump(
    io: &mut dyn PcmIo,
    channels: usize,
    format: DeviceFormat,
    stop: &AtomicBool,
    mut fill: impl FnMut(&mut [f32]),
) -> Result<()> {
    let mut f32_buf = vec![0f32; CHUNK_FRAMES * channels];
    let mut i16_buf: Vec<i16> = match format {
        DeviceFormat::I16 => vec![0i16; CHUNK_FRAMES * channels],
        DeviceFormat::F32 => Vec::new(),
    };

    while !stop.load(Ordering::Relaxed) {
        f32_buf.fill(0.0);
        fill(&mut f32_buf);

        if let DeviceFormat::I16 = format {
            for (d, &s) in i16_buf.iter_mut().zip(&f32_buf) {
                *d = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            }
        }

        let mut frames_written = 0usize;
        let mut consecutive_failures = 0u32;
        let mut zero_writes = 0u32;
        while frames_written < CHUNK_FRAMES {
            let sample_offset = frames_written * channels;
            let remaining = CHUNK_FRAMES - frames_written;
            let chunk = match format {
                DeviceFormat::F32 => PcmChunk::F32(&f32_buf[sample_offset..]),
                DeviceFormat::I16 => PcmChunk::I16(&i16_buf[sample_offset..]),
            };

            let ret = io.writei(chunk, remaining);
            if ret > 0 {
                // 要求したフレーム数を超えて書いたと報告されても、範囲外へ進めない。
                frames_written += (ret as usize).min(remaining);
                consecutive_failures = 0;
                zero_writes = 0;
                continue;
            }
            if ret == 0 {
                zero_writes += 1;
                if zero_writes > MAX_ZERO_WRITES {
                    bail!("ALSA writei が 0 フレームを返し続けるため再生ループを終了します");
                }
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }

            let failure = classify_write_failure(ret);
            if failure == WriteFailure::DeviceGone {
                bail!(
                    "ALSA デバイスが失われました（{}）",
                    io.strerror(ret as c_int)
                );
            }
            consecutive_failures += 1;
            if consecutive_failures > MAX_CONSECUTIVE_FAILURES {
                bail!(
                    "ALSA writei が連続失敗したため再生ループを終了します（最終エラー: {}）",
                    io.strerror(ret as c_int)
                );
            }
            match failure {
                // アンダーラン: prepare して同じチャンクを再送。
                WriteFailure::Underrun => {}
                // サスペンド: 少し待ってから prepare して再送。
                WriteFailure::Suspended => std::thread::sleep(Duration::from_millis(20)),
                WriteFailure::Other => std::thread::sleep(Duration::from_millis(5)),
                WriteFailure::DeviceGone => unreachable!(),
            }
            let prepared = io.prepare();
            if prepared as c_long == ENODEV {
                bail!("ALSA デバイスが失われました（prepare: {}）", io.strerror(prepared));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **無音のみを書き込むスモークテスト**。ALSA `"null"` デバイス
    /// （何も鳴らさない仮想デバイス）に対して open → 約 200ms 分の無音書き込み
    /// → stop、という最小往復を確認する。実オーディオデバイスには一切触れない。
    /// libasound が無い環境では skip する（eprintln して return）。
    #[test]
    fn open_null_device_and_play_silence_briefly() {
        let output = match AlsaOutput::open_device("null") {
            Ok(o) => o,
            Err(e) => {
                eprintln!("ALSA (libasound) が利用できないためスキップ: {e:#}");
                return;
            }
        };

        let rate = output.sample_rate();
        let channels = output.channels();
        assert!(
            (8_000..=384_000).contains(&rate),
            "サンプルレートが妥当域外: {rate}"
        );
        assert!(
            (1..=8).contains(&channels),
            "チャンネル数が妥当域外: {channels}"
        );

        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(200));
                stop.store(true, Ordering::Relaxed);
            });
            // fill は何もしない = バッファは常にゼロ初期化されたまま = 完全な無音。
            let result = output.run(&stop, |_buf| {
                // 無音のみ。可聴音は一切出さない（そもそも null デバイス）。
            });
            assert!(result.is_ok(), "無音再生ループが失敗: {result:?}");
        });
    }

    /// libasound 自体はロードできても、存在しないデバイス名を指定すると
    /// `open_device` が明確な `Err` を返すことを確認する（呼び出し元の
    /// 「無音続行」フォールバック経路の前提条件）。libasound が無い環境では
    /// この経路も自然に `Err`（dlopen 失敗）になるため区別せず skip 扱いにする。
    #[test]
    fn open_device_with_nonexistent_device_name_fails_gracefully() {
        let result = AlsaOutput::open_device("noiria_test_definitely_nonexistent_device_xyz");
        assert!(result.is_err(), "存在しないデバイス名は Err を返すべき");
    }

    /// `writei` の戻り値を順に返す偽 PCM（libasound を使わない）。
    struct ScriptedPcm {
        script: Vec<c_long>,
        calls: usize,
        prepares: usize,
        prepare_ret: c_int,
    }

    impl ScriptedPcm {
        fn new(script: Vec<c_long>) -> Self {
            Self {
                script,
                calls: 0,
                prepares: 0,
                prepare_ret: 0,
            }
        }
    }

    impl PcmIo for ScriptedPcm {
        fn writei(&mut self, _chunk: PcmChunk<'_>, frames: usize) -> c_long {
            let ret = self
                .script
                .get(self.calls)
                .copied()
                .unwrap_or(frames as c_long);
            self.calls += 1;
            ret
        }
        fn prepare(&mut self) -> c_int {
            self.prepares += 1;
            self.prepare_ret
        }
        fn strerror(&self, err: c_int) -> String {
            format!("errno={err}")
        }
    }

    fn pump_once(io: &mut ScriptedPcm) -> Result<()> {
        let stop = AtomicBool::new(false);
        let mut fills = 0;
        pump(io, 2, DeviceFormat::F32, &stop, |_| {
            fills += 1;
            if fills >= 2 {
                stop.store(true, Ordering::Relaxed);
            }
        })
    }

    #[test]
    fn pump_fails_immediately_when_the_device_is_gone() {
        let mut io = ScriptedPcm::new(vec![ENODEV]);
        let err = pump_once(&mut io).unwrap_err();
        assert!(format!("{err:#}").contains("失われました"), "{err:#}");
        assert_eq!(io.calls, 1, "ENODEV は再試行しない");
        assert_eq!(io.prepares, 0);
    }

    #[test]
    fn pump_fails_when_prepare_reports_a_gone_device() {
        let mut io = ScriptedPcm::new(vec![-5]);
        io.prepare_ret = ENODEV as c_int;
        assert!(pump_once(&mut io).is_err());
    }

    #[test]
    fn pump_recovers_from_underruns_and_partial_writes() {
        // 部分書き込み → EPIPE → ESTRPIPE → 残りを書いて完了、を 2 チャンク。
        let mut io = ScriptedPcm::new(vec![100, EPIPE, ESTRPIPE, 412]);
        assert!(pump_once(&mut io).is_ok());
        assert_eq!(io.prepares, 2);
        assert_eq!(io.calls, 4 + 1, "2 チャンク目は 1 回で書き切る");
    }

    #[test]
    fn pump_gives_up_after_consecutive_unknown_failures() {
        let mut io = ScriptedPcm::new(vec![-5; 100]);
        assert!(pump_once(&mut io).is_err());
        assert_eq!(io.calls as u32, MAX_CONSECUTIVE_FAILURES + 1);
    }

    #[test]
    fn write_failure_classification() {
        assert_eq!(classify_write_failure(EPIPE), WriteFailure::Underrun);
        assert_eq!(classify_write_failure(ESTRPIPE), WriteFailure::Suspended);
        assert_eq!(classify_write_failure(ENODEV), WriteFailure::DeviceGone);
        assert_eq!(classify_write_failure(-5), WriteFailure::Other);
    }
}
