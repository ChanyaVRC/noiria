//! Windows 専用: WASAPI (共有モード・イベント駆動) 直叩きの音声出力。
//!
//! `codec/mf_video.rs` と同格の OS 境界 API 呼び出し（ffmpeg 同様、cpal/rodio
//! のような外部オーディオライブラリは使わない）。ミキシング・リサンプル・
//! フェード等の「音声処理そのもの」は一切ここに置かず `mixer.rs`（純 Rust・
//! OS 非依存）に任せる。ここは「OS にバッファを渡す」ことだけに責務を絞る。
//!
//! 呼び出し規約:
//! - [`WasapiOutput::open_default`] は「呼び出したスレッド」で完結させ、
//!   COM オブジェクト一式はそのスレッド内で生成・破棄すること
//!   （返された `WasapiOutput` を他スレッドへ move しない。COM の
//!   アパートメント規約は `codec/mf_video.rs` と同じ扱い）。
//! - 実際の再生ループは [`WasapiOutput::run`] が担い、`stop` フラグが立つか
//!   デバイスエラーが起きるまでブロックする。呼び出し元は専用スレッドで
//!   `open_default().and_then(|o| o.run(...))` のように使う。
//!   デバイスの取り外し・フォーマット変更（`AUDCLNT_E_DEVICE_INVALIDATED` 等）は
//!   `run` が `Err` で返す。開き直しは呼び出し元（`audio.rs`）の責務。

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, ensure, Context, Result};

use windows::core::GUID;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE,
};
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// `KSDATAFORMAT_SUBTYPE_PCM`（`Win32_Media_KernelStreaming` feature を増やさずここで定義する）。
const KSDATAFORMAT_SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);

/// `AUDCLNT_BUFFERFLAGS_SILENT`: 書き込んだ内容を無視して無音として扱わせる。
const BUFFERFLAGS_SILENT: u32 = 0x2;

/// `WaitForSingleObject` の戻り値（`WAIT_EVENT.0`）。
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;

thread_local! {
    // COM のアパートメント状態はスレッドごと。mf_video.rs と同じ流儀:
    // MTA で初期化し、スレッド終了時の後始末は OS に任せる（CoUninitialize は呼ばない）。
    static COM_INITED: Cell<bool> = const { Cell::new(false) };
}

fn ensure_com_ready() {
    COM_INITED.with(|inited| {
        if !inited.get() {
            // S_OK/S_FALSE のどちらでも問題ない。別スレッドで STA 等に初期化済みの
            // 場合の RPC_E_CHANGED_MODE も致命的ではないので無視する
            // (mf_video.rs の ensure_mf_ready と同じ考え方)。
            // SAFETY: 予約引数は仕様どおり None（NULL）。戻り値は上記の理由で無視してよい。
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            inited.set(true);
        }
    });
}

/// レンダーバッファへ書き込む際のサンプルフォーマット。
/// 共有モードの mix format は理論上 IEEE float とは限らないため、
/// f32 でなければここで変換する（要求仕様の「変換層」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeviceFormat {
    F32,
    I16,
}

impl DeviceFormat {
    fn bytes_per_sample(self) -> usize {
        match self {
            DeviceFormat::F32 => 4,
            DeviceFormat::I16 => 2,
        }
    }
}

/// `CreateEventW` で得たイベントハンドルの所有者。Drop で必ず `CloseHandle` する
/// （`open_default` の途中のエラー経路でも、`WasapiOutput` の破棄でもリークしない）。
struct EventHandle(HANDLE);

impl EventHandle {
    fn new() -> Result<Self> {
        // SAFETY: 全引数が NULL / false の自動リセット・無名イベント。返ったハンドルは
        // この値だけが所有し、Drop で 1 回だけ閉じる。
        let handle = unsafe { CreateEventW(None, false, false, None) }
            .context("イベントハンドルの作成に失敗")?;
        Ok(Self(handle))
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` は `CreateEventW` が返した有効なハンドルで、ここで初めて閉じる。
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// WASAPI 共有モード・イベント駆動の出力ストリーム。
/// 公開 API は小さく保つ: フォーマットを問い合わせて `run` でブロッキング
/// 再生ループに入るだけ。
pub struct WasapiOutput {
    // フィールドは宣言順に Drop される。`audio_client` がイベントを参照するので
    // `event` は最後（= クライアントを解放してからハンドルを閉じる）。
    audio_client: IAudioClient,
    render_client: IAudioRenderClient,
    buffer_frame_count: u32,
    channels: u16,
    sample_rate: u32,
    device_format: DeviceFormat,
    event: EventHandle,
}

// SAFETY: この型は「生成したスレッドで生成し、同じスレッド内で使い切って
// drop する」運用に限定する（呼び出し規約に明記）。複数スレッドから同時に
// 触ることはなく、Send を要求する呼び出し元（thread::spawn のクロージャ内）
// に閉じ込めるためだけに実装する。COM の MTA 上ではポインタ自体の
// スレッド間移動は安全だが、本実装はそれに頼らず単一スレッド完結にしている。
unsafe impl Send for WasapiOutput {}

impl WasapiOutput {
    pub fn channels(&self) -> u16 {
        self.channels
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// 既定の再生デバイスを共有モード・イベント駆動で開く。
    /// デバイスが無い/使用できない環境では `Err` を返す
    /// （呼び出し元は「無音で続行」にフォールバックすればよい）。
    pub fn open_default() -> Result<Self> {
        ensure_com_ready();

        // SAFETY: この呼び出しで COM は初期化済み。CLSID と要求インターフェース型
        // (IMMDeviceEnumerator) は対応しており、集約なし（None）で生成する。
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .context("IMMDeviceEnumerator の作成に失敗（オーディオデバイスなし？）")?;
        // SAFETY: `enumerator` は上で得た有効な COM オブジェクト。引数は値の列挙子のみ。
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .context("既定の再生デバイスが見つかりません")?;
        // SAFETY: `device` は有効な IMMDevice。活性化パラメータは無し（None）。
        let audio_client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .context("IAudioClient の Activate に失敗")?;

        // SAFETY: `audio_client` は有効。戻りのポインタは CoTaskMemAlloc されたもので、
        // 直下の `CoTaskMemGuard` が必ず 1 回解放する。
        let mix_format_ptr =
            unsafe { audio_client.GetMixFormat() }.context("GetMixFormat に失敗")?;
        ensure!(
            !mix_format_ptr.is_null(),
            "GetMixFormat が null を返しました"
        );
        // GetMixFormat は CoTaskMemAlloc されたバッファを返す。呼び出し元が解放する。
        let _free_guard = CoTaskMemGuard(mix_format_ptr as *mut core::ffi::c_void);
        // SAFETY: null でないことを確認済みで、`_free_guard` がこの関数の終わりまで
        // 解放しないため、`mix_format` の使用期間（関数内）ずっと有効。
        let mix_format = unsafe { &*mix_format_ptr };

        let channels = mix_format.nChannels;
        let sample_rate = mix_format.nSamplesPerSec;
        ensure!(channels >= 1, "チャンネル数が不正です: {channels}");
        ensure!(sample_rate > 0, "サンプルレートが不正です");

        // SAFETY: `mix_format` は `mix_format_ptr` の借用で、GetMixFormat が返した
        // 有効な WAVEFORMATEX(EXTENSIBLE) バッファを指す（`_free_guard` で生存中）。
        let device_format = unsafe { classify_format(mix_format, mix_format_ptr) }?;

        // 共有モード・イベント駆動: hnsBufferDuration/hnsPeriodicity は仕様上
        // 0 を渡す（システムが最小レイテンシのバッファを選ぶ）。
        // pFormat は GetMixFormat が返した値をそのまま渡す（共有モードでは
        // mix format は必ず受理される、というのが WASAPI の契約）。
        // SAFETY: `mix_format_ptr` は上記ガードで有効な WAVEFORMATEX(EXTENSIBLE) を指す。
        // セッション GUID は None（既定）。
        unsafe {
            audio_client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                0,
                0,
                mix_format_ptr,
                None,
            )
        }
        .context("IAudioClient::Initialize に失敗")?;

        let event = EventHandle::new()?;
        // SAFETY: `event.0` は有効なイベントハンドル。`audio_client` より長く生きる
        // （`WasapiOutput` ではフィールド順で最後に閉じる。エラー経路では
        // クライアントは Start されないのでイベントが signal されることはない）。
        unsafe { audio_client.SetEventHandle(event.0) }.context("SetEventHandle に失敗")?;

        // SAFETY: Initialize 済みのクライアントから、IAudioRenderClient として要求する。
        let render_client: IAudioRenderClient =
            unsafe { audio_client.GetService() }.context("IAudioRenderClient の取得に失敗")?;
        // SAFETY: Initialize 済みのクライアントに対する引数なしの問い合わせ。
        let buffer_frame_count =
            unsafe { audio_client.GetBufferSize() }.context("GetBufferSize に失敗")?;
        ensure!(buffer_frame_count > 0, "バッファサイズが 0 です");

        Ok(Self {
            audio_client,
            render_client,
            buffer_frame_count,
            channels,
            sample_rate,
            device_format,
            event,
        })
    }

    /// イベント待ち → バッファ埋め、を `stop` が立つまで繰り返す。
    /// `fill` は「out_channels 単位でインターリーブされた f32 バッファ」を
    /// 要求ごとに渡されるので、そこへ次のサンプルを書き込む
    /// （無音にしたい場合は何もしなくてよい。バッファは呼び出し前にゼロ初期化する）。
    ///
    /// デバイスの喪失・フォーマット変更・イベント待ちの失敗は `Err` で返す
    /// （呼び出し元が開き直す）。`stop` が立ったら `Ok(())`。
    pub fn run(self, stop: &AtomicBool, mut fill: impl FnMut(&mut [f32])) -> Result<()> {
        let channels = self.channels as usize;
        let mut scratch: Vec<f32> = vec![0.0; self.buffer_frame_count as usize * channels];

        // 開始前にバッファ全体を無音で埋めておく（WASAPI 推奨の初期化手順）。
        fill(&mut scratch);
        self.write_frames(self.buffer_frame_count, &scratch)
            .context("初期バッファの書き込みに失敗")?;

        // SAFETY: Initialize 済みで、上でバッファを埋め終えている（Start の前提）。
        unsafe { self.audio_client.Start() }.context("IAudioClient::Start に失敗")?;

        let result = (|| -> Result<()> {
            while !stop.load(Ordering::Relaxed) {
                // stop フラグを取りこぼさないよう短いタイムアウトで待つ。
                // SAFETY: `self.event.0` は `self` が所有する有効なイベントハンドル。
                let wait = unsafe { WaitForSingleObject(self.event.0, 200) }.0;
                match wait {
                    WAIT_OBJECT_0 => {}
                    WAIT_TIMEOUT => {
                        // 正常経路（stop 確認のため）。ただしデバイスが無効化されると
                        // イベントが来なくなるので、ここで状態を問い合わせて検知する。
                        // SAFETY: Start 済みクライアントへの引数なしの問い合わせ。
                        unsafe { self.audio_client.GetCurrentPadding() }
                            .context("GetCurrentPadding に失敗（デバイス喪失？）")?;
                        continue;
                    }
                    other => bail!("WaitForSingleObject が失敗しました（戻り値 {other:#x}）"),
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                // SAFETY: Start 済みクライアントへの引数なしの問い合わせ。
                let padding = unsafe { self.audio_client.GetCurrentPadding() }
                    .context("GetCurrentPadding に失敗")?;
                let available = self.buffer_frame_count.saturating_sub(padding);
                if available == 0 {
                    continue;
                }
                // ユーザーの fill（ロック待ちや panic の可能性あり）は GetBuffer の前に済ませ、
                // GetBuffer〜ReleaseBuffer の間にはコールバックを挟まない。
                let needed = available as usize * channels;
                if scratch.len() < needed {
                    scratch.resize(needed, 0.0);
                }
                let buf = &mut scratch[..needed];
                buf.fill(0.0);
                fill(buf);
                self.write_frames(available, buf)?;
            }
            Ok(())
        })();

        // SAFETY: 引数なし。Start 済み・未 Start のどちらでも呼べる（失敗は無視する）。
        let _ = unsafe { self.audio_client.Stop() };
        result
    }

    /// レンダーバッファの `frames` フレームぶんを借り、`src` を書いて返却する。
    /// `src.len()` は `frames * channels` と一致している必要がある。
    /// `GetBuffer` に成功したら、書き込みの成否にかかわらず必ず `ReleaseBuffer` する。
    fn write_frames(&self, frames: u32, src: &[f32]) -> Result<()> {
        ensure!(
            frames <= self.buffer_frame_count,
            "要求フレーム数 {frames} がバッファサイズ {} を超えています",
            self.buffer_frame_count
        );
        // SAFETY: `frames` は GetBufferSize 以下（上で確認）。GetBuffer/ReleaseBuffer は
        // 1 対で、間に他の GetBuffer を挟まない（この関数内で完結する）。
        let ptr = unsafe { self.render_client.GetBuffer(frames) }.context("GetBuffer に失敗")?;
        // SAFETY: `ptr` は `GetBuffer(frames)` が返した `frames * channels * bytes_per_sample`
        // バイト以上の書き込み可能なデバイス領域（`nBlockAlign` は `classify_format` で検証済み）で
        // `src` とは重ならない。
        let written = unsafe {
            write_device_buffer(
                ptr,
                frames as usize,
                self.channels as usize,
                src,
                self.device_format,
            )
        };
        let flags = if written.is_ok() { 0 } else { BUFFERFLAGS_SILENT };
        // SAFETY: 直前の GetBuffer(frames) と同じフレーム数での対の返却。
        let released = unsafe { self.render_client.ReleaseBuffer(frames, flags) }
            .context("ReleaseBuffer に失敗");
        written?;
        released
    }
}

/// `GetMixFormat` が指すメモリを Drop 時に `CoTaskMemFree` する RAII ガード。
struct CoTaskMemGuard(*mut core::ffi::c_void);
impl Drop for CoTaskMemGuard {
    fn drop(&mut self) {
        // SAFETY: `self.0` は GetMixFormat が CoTaskMemAlloc で返したポインタで、
        // このガードだけが所有し、ここで 1 回だけ解放する。
        unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(self.0)) };
    }
}

/// `fmt` が指す mix format を分類する。`ptr` は `fmt` の借用元と同じ生ポインタで、
/// `WAVE_FORMAT_EXTENSIBLE` のときだけ拡張部まで読むため `cbSize` を検査する。
///
/// # Safety
/// `ptr` は `fmt` と同じ割り当ての先頭を指し、`fmt.wFormatTag == WAVE_FORMAT_EXTENSIBLE`
/// の場合は `size_of::<WAVEFORMATEX>() + fmt.cbSize` バイト以上が読み取り可能であること。
unsafe fn classify_format(fmt: &WAVEFORMATEX, ptr: *mut WAVEFORMATEX) -> Result<DeviceFormat> {
    // WAVEFORMATEX は packed(1) なので、フィールドは一度ローカル変数へコピーしてから
    // 使う（&packed_struct.field は未整列参照になり UB になるため）。
    let tag = fmt.wFormatTag;
    let bits = fmt.wBitsPerSample;
    let cb_size = fmt.cbSize;
    let channels = fmt.nChannels;
    let block_align = fmt.nBlockAlign;
    let format = match tag {
        WAVE_FORMAT_IEEE_FLOAT_TAG if bits == 32 => DeviceFormat::F32,
        t if t == WAVE_FORMAT_PCM && bits == 16 => DeviceFormat::I16,
        t if t == WAVE_FORMAT_EXTENSIBLE => {
            ensure!(
                (cb_size as usize)
                    >= std::mem::size_of::<WAVEFORMATEXTENSIBLE>()
                        - std::mem::size_of::<WAVEFORMATEX>(),
                "WAVEFORMATEXTENSIBLE の cbSize が不足しています"
            );
            // SAFETY: `ptr` は `fmt` の借用元で、cbSize が拡張部ぶんあることを上で確認した
            // ので WAVEFORMATEXTENSIBLE 全体が確保範囲内。構造体は packed(1)（整列 1）。
            let ext = unsafe { &*(ptr as *const WAVEFORMATEXTENSIBLE) };
            let sub_format = ext.SubFormat; // packed struct から値コピー
            if sub_format == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT && bits == 32 {
                DeviceFormat::F32
            } else if sub_format == KSDATAFORMAT_SUBTYPE_PCM && bits == 16 {
                DeviceFormat::I16
            } else {
                bail!("対応していない WAVEFORMATEXTENSIBLE です (bits={bits}, SubFormat={sub_format:?})")
            }
        }
        t => bail!("対応していない mix format です (tag={t}, bits={bits})"),
    };
    // バッファへ書くレイアウト（インターリーブ・フレーム = チャンネル数 × サンプル幅）と
    // OS の報告するフレーム幅が一致しないフォーマットは扱えない。
    ensure!(
        block_align as usize == channels as usize * format.bytes_per_sample(),
        "nBlockAlign ({block_align}) が想定フレーム幅 ({channels}ch x {}B) と一致しません",
        format.bytes_per_sample()
    );
    Ok(format)
}

// WAVE_FORMAT_IEEE_FLOAT は u32 定数として提供されるが wFormatTag は u16 なので
// 比較用に u16 へ縮める（3 は u16 範囲に収まるので安全）。
const WAVE_FORMAT_IEEE_FLOAT_TAG: u16 = WAVE_FORMAT_IEEE_FLOAT as u16;

/// スクラッチ (f32, `channels` 単位インターリーブ, `dst_frames` フレーム) をデバイスの
/// ネイティブフォーマットへ変換して `dst` へ書き込む。
///
/// 貸与領域の整列は WASAPI が文書化していないので、スライスを作らず（f32/i16 の
/// 整列を仮定せず）バイト単位のコピー / `write_unaligned` で書く。
///
/// # Safety
/// `dst` が非 null のとき、`dst` は `dst_frames * channels * fmt.bytes_per_sample()` バイト
/// 以上の書き込み可能な領域を指し、`src` と重ならないこと。
unsafe fn write_device_buffer(
    dst: *mut u8,
    dst_frames: usize,
    channels: usize,
    src: &[f32],
    fmt: DeviceFormat,
) -> Result<()> {
    ensure!(!dst.is_null(), "GetBuffer が null を返しました");
    ensure!(
        dst_frames.checked_mul(channels) == Some(src.len()),
        "書き込みサンプル数 {} が {dst_frames} フレーム x {channels}ch と一致しません",
        src.len()
    );
    match fmt {
        DeviceFormat::F32 => {
            // SAFETY: `dst` は src.len() * 4 バイト書き込み可能（上記の長さ検査 +
            // nBlockAlign 検査）。`src` は呼び出し元の Rust 配列で、デバイス領域とは別物
            // なので重ならない。u8 単位のコピーは整列を要求しない。
            unsafe {
                std::ptr::copy_nonoverlapping(src.as_ptr() as *const u8, dst, src.len() * 4);
            }
        }
        DeviceFormat::I16 => {
            let dst = dst as *mut i16;
            for (i, &s) in src.iter().enumerate() {
                let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                // SAFETY: i < src.len() で、`dst` は src.len() * 2 バイト書き込み可能。
                // 整列は保証されないので write_unaligned を使う。
                unsafe { dst.add(i).write_unaligned(v) };
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// **無音のみを書き込むスモークテスト**。open_default → GetMixFormat 相当の
    /// フォーマット取得 → 完全な無音バッファを約 200ms 分書いて Stop、という
    /// 最小往復を確認する。音量やデバイス設定は一切変更しない。
    /// デバイスが無い CI 環境では skip する（eprintln して return）。
    #[test]
    fn open_default_and_play_silence_briefly() {
        let output = match WasapiOutput::open_default() {
            Ok(o) => o,
            Err(e) => {
                eprintln!("オーディオデバイスが利用できないためスキップ: {e:#}");
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
            // fill は何もしない = バッファは常にゼロ初期化されたまま =完全な無音。
            let result = output.run(&stop, |_buf| {
                // 無音のみ。可聴音は一切出さない。
            });
            assert!(result.is_ok(), "無音再生ループが失敗: {result:?}");
        });
    }

    fn is_valid_handle(h: HANDLE) -> bool {
        let mut flags = 0u32;
        // SAFETY: 出力先は有効な &mut u32。無効なハンドル値でも失敗を返すだけ。
        unsafe { windows::Win32::Foundation::GetHandleInformation(h, &mut flags) }.is_ok()
    }

    /// イベントハンドルを毎回閉じること（旧実装は open のたびにリークしていた）。
    /// 並走する他テストがハンドル値を再利用しうるので、1 回ごとの判定ではなく割合で見る
    /// （対照: 閉じないハンドルは全てが有効なまま）。
    #[test]
    fn event_handle_is_closed_on_drop() {
        const N: usize = 200;
        let closed = (0..N)
            .filter(|_| {
                let event = EventHandle::new().unwrap();
                let raw = event.0;
                assert!(is_valid_handle(raw));
                drop(event);
                !is_valid_handle(raw)
            })
            .count();

        let leaked = (0..N)
            .filter(|_| {
                // SAFETY: 全引数 NULL/false。意図的に閉じない（対照実験）。
                let raw = unsafe { CreateEventW(None, false, false, None) }.unwrap();
                !is_valid_handle(raw)
            })
            .count();

        assert_eq!(leaked, 0, "対照: 閉じていないハンドルは有効なまま");
        assert!(closed >= N * 9 / 10, "EventHandle が閉じていない: {closed}/{N}");
    }

    fn wave_format(tag: u16, bits: u16, channels: u16, block_align: u16) -> WAVEFORMATEX {
        WAVEFORMATEX {
            wFormatTag: tag,
            nChannels: channels,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 48_000 * block_align as u32,
            nBlockAlign: block_align,
            wBitsPerSample: bits,
            cbSize: 0,
        }
    }

    fn extensible(sub: GUID, bits: u16, channels: u16, block_align: u16) -> WAVEFORMATEXTENSIBLE {
        let mut format = wave_format(WAVE_FORMAT_EXTENSIBLE, bits, channels, block_align);
        format.cbSize = 22;
        WAVEFORMATEXTENSIBLE {
            Format: format,
            Samples: windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE_0 {
                wValidBitsPerSample: bits,
            },
            dwChannelMask: 3,
            SubFormat: sub,
        }
    }

    fn classify_ext(e: &WAVEFORMATEXTENSIBLE) -> Result<DeviceFormat> {
        let ptr = e as *const WAVEFORMATEXTENSIBLE as *mut WAVEFORMATEX;
        // SAFETY: `ptr` は有効な WAVEFORMATEXTENSIBLE（先頭が WAVEFORMATEX）を指す。
        unsafe { classify_format(&*ptr, ptr) }
    }

    #[test]
    fn classify_format_accepts_float32_and_pcm16_and_rejects_the_rest() {
        let f32_fmt = wave_format(WAVE_FORMAT_IEEE_FLOAT_TAG, 32, 2, 8);
        // SAFETY: 各ローカル変数は有効な WAVEFORMATEX で EXTENSIBLE ではない。
        unsafe {
            assert_eq!(
                classify_format(&f32_fmt, &f32_fmt as *const _ as *mut _).unwrap(),
                DeviceFormat::F32
            );
            let pcm16 = wave_format(WAVE_FORMAT_PCM, 16, 2, 4);
            assert_eq!(
                classify_format(&pcm16, &pcm16 as *const _ as *mut _).unwrap(),
                DeviceFormat::I16
            );
            let pcm24 = wave_format(WAVE_FORMAT_PCM, 24, 2, 6);
            assert!(classify_format(&pcm24, &pcm24 as *const _ as *mut _).is_err());
            let bad_align = wave_format(WAVE_FORMAT_IEEE_FLOAT_TAG, 32, 2, 12);
            assert!(classify_format(&bad_align, &bad_align as *const _ as *mut _).is_err());
        }
    }

    #[test]
    fn classify_format_checks_the_extensible_subformat() {
        let float = extensible(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, 32, 6, 24);
        assert_eq!(classify_ext(&float).unwrap(), DeviceFormat::F32);
        let pcm = extensible(KSDATAFORMAT_SUBTYPE_PCM, 16, 2, 4);
        assert_eq!(classify_ext(&pcm).unwrap(), DeviceFormat::I16);
        // 16bit でも PCM でない SubFormat（旧実装は bits==16 だけで I16 とみなしていた）。
        let not_pcm = extensible(GUID::from_u128(0x1234), 16, 2, 4);
        assert!(classify_ext(&not_pcm).is_err());
        // 24bit PCM（未対応）は Err で、無音続行/再試行の経路に乗る。
        let pcm24 = extensible(KSDATAFORMAT_SUBTYPE_PCM, 24, 2, 6);
        assert!(classify_ext(&pcm24).is_err());
        // cbSize が不足した EXTENSIBLE は拡張部を読まずに拒否する。
        let mut short = extensible(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, 32, 2, 8);
        short.Format.cbSize = 0;
        assert!(classify_ext(&short).is_err());
    }

    #[test]
    fn write_device_buffer_handles_unaligned_destinations_and_checks_lengths() {
        let src = [0.5f32, -0.5, 1.5, -2.0, 0.0, 0.25];
        for offset in 0..4 {
            let mut f32_dst = vec![0u8; 4 * src.len() + 8];
            // SAFETY: `f32_dst[offset..]` は `4 * src.len()` バイト以上の書き込み可能な独自バッファ。
            unsafe {
                write_device_buffer(f32_dst[offset..].as_mut_ptr(), 3, 2, &src, DeviceFormat::F32)
            }
            .unwrap();
            let got: Vec<f32> = f32_dst[offset..offset + 4 * src.len()]
                .chunks(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            assert_eq!(got, src, "F32 offset={offset}");

            let mut i16_dst = vec![0u8; 2 * src.len() + 8];
            // SAFETY: `i16_dst[offset..]` は `2 * src.len()` バイト以上の書き込み可能な独自バッファ。
            unsafe {
                write_device_buffer(i16_dst[offset..].as_mut_ptr(), 3, 2, &src, DeviceFormat::I16)
            }
            .unwrap();
            let got: Vec<i16> = i16_dst[offset..offset + 2 * src.len()]
                .chunks(2)
                .map(|b| i16::from_le_bytes(b.try_into().unwrap()))
                .collect();
            assert_eq!(
                got,
                vec![16383, -16383, i16::MAX, -i16::MAX, 0, 8191],
                "I16 offset={offset}"
            );
            assert!(
                i16_dst[offset + 2 * src.len()..].iter().all(|&b| b == 0),
                "確保範囲の外へ書いていない"
            );
        }
        let mut dst = [0u8; 64];
        // SAFETY: 長さ不一致 / null は書き込み前に `ensure!` で Err になる。
        unsafe {
            assert!(write_device_buffer(dst.as_mut_ptr(), 4, 2, &src, DeviceFormat::F32).is_err());
            assert!(
                write_device_buffer(std::ptr::null_mut(), 3, 2, &src, DeviceFormat::F32).is_err()
            );
        }
    }
}
