//! Windows 専用: Media Foundation (OS 内蔵デコーダ/エンコーダ) 経由の動画再生。
//!
//! H.264/MP4 等、完全自作デコーダでは非現実的な形式を OS 境界 API
//! （ウィンドウ/音声出力と同格の扱い。ffmpeg 等の外部ライブラリは使わない）で
//! フォールバック再生するための実装。`VideoSource` トレイトを実装するので
//! 再生スレッド (`ops_video.rs`) からは AVI (MJPEG) と同じ口で扱える。

use std::cell::Cell;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{ensure, Context, Result};

use windows::core::{GUID, HSTRING};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaBuffer, IMFMediaType, IMFSample, IMFSinkWriter, IMFSourceReader,
    MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFCreateSourceReaderFromURL, MFMediaType_Video, MFStartup,
    MFVideoFormat_H264, MFVideoFormat_RGB32, MFSTARTUP_FULL, MF_MT_ALL_SAMPLES_INDEPENDENT,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READERF_ENDOFSTREAM,
    MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use super::video::VideoSource;

// プログレッシブ (ノンインターレース)。MFVideoInterlace_Progressive の値。
const MF_VIDEO_INTERLACE_PROGRESSIVE: u32 = 2;

/// dwStreamIndex に渡す定数は `MF_SOURCE_READER_CONSTANTS(i32)` 型なので
/// u32 の引数に渡せるようここで変換しておく。
fn first_video_stream() -> u32 {
    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32
}

/// プロセス内で一度だけ呼べばよい MFStartup 用のガード。
static MF_STARTED: OnceLock<()> = OnceLock::new();

thread_local! {
    // COM のアパートメント状態はスレッドごとに持つため、CoInitializeEx は
    // MFStartup と違い「呼び出しスレッドごとに一度」必要。動画再生は専用スレッドで
    // 完結する（スレッド終了時に OS が後始末する）ため CoUninitialize は呼ばない。
    static COM_INITED: Cell<bool> = const { Cell::new(false) };
}

/// このスレッドで Media Foundation を使う準備をする（COM 初期化 + MFStartup）。
/// 複数回呼んでも安全（べき等）。
fn ensure_mf_ready() -> Result<()> {
    COM_INITED.with(|inited| {
        if !inited.get() {
            // 戻り値は S_OK/S_FALSE (既に初期化済み) のどちらでも問題ない。
            // 別スレッドで STA 等に初期化済みだった場合の RPC_E_CHANGED_MODE も
            // 致命的ではない（MTA 前提の呼び出しはできなくなるが動作は継続できる）ので無視する。
            // SAFETY: 予約引数は仕様どおり None（NULL）。戻り値は上記の理由で無視してよい。
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            inited.set(true);
        }
    });
    MF_STARTED.get_or_init(|| {
        // 失敗時もここでは無視し、以降の呼び出し (MFCreateSourceReaderFromURL 等) の
        // エラーとして自然に呼び出し元へ伝播させる。
        // SAFETY: 引数は SDK 定義の定数のみ。`OnceLock` でプロセス内 1 回しか呼ばれない。
        let _ = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) };
    });
    Ok(())
}

/// 現在のメディアタイプから幅・高さ・fps・stride を読み取る。
fn read_video_params(reader: &IMFSourceReader) -> Result<(u32, u32, f32, i32)> {
    // SAFETY: `reader` は有効な COM オブジェクト。引数はストリーム番号の値のみ。
    let media_type: IMFMediaType = unsafe { reader.GetCurrentMediaType(first_video_stream()) }
        .context("動画のメディアタイプが取得できません")?;

    // SAFETY: `media_type` は直前に得た有効な COM オブジェクト。キーは SDK 定義の GUID への
    // 有効な参照（以降の GetUINT32/GetUINT64/SetXxx も同じ前提）。
    let packed_size = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }
        .context("MF_MT_FRAME_SIZE が取得できません")?;
    let width = (packed_size >> 32) as u32;
    let height = (packed_size & 0xFFFF_FFFF) as u32;

    // SAFETY: 上記（`media_type` は有効、キーは有効な GUID 参照）。
    let fps = match unsafe { media_type.GetUINT64(&MF_MT_FRAME_RATE) } {
        Ok(packed) => {
            let num = (packed >> 32) as u32;
            let den = (packed & 0xFFFF_FFFF) as u32;
            if den > 0 {
                num as f32 / den as f32
            } else {
                30.0
            }
        }
        Err(_) => 30.0,
    };

    // MF_MT_DEFAULT_STRIDE は「正=トップダウン / 負=ボトムアップ」。属性値は
    // UINT32 として格納されるが実体は符号付きなので i32 に再解釈する。
    // 属性が無い場合、非圧縮 RGB の既定は DIB 規約の「ボトムアップ」なので
    // 負のストライドとみなす（MF の Image Stride 仕様）。
    // SAFETY: 上記（`media_type` は有効、キーは有効な GUID 参照）。
    let stride = match unsafe { media_type.GetUINT32(&MF_MT_DEFAULT_STRIDE) } {
        Ok(s) => s as i32,
        Err(_) => -((width as i64 * 4) as i32),
    };

    Ok((width, height, fps, stride))
}

/// Media Foundation の `IMFSourceReader` をラップした `VideoSource` 実装。
pub struct MfVideo {
    reader: IMFSourceReader,
    width: u32,
    height: u32,
    fps: f32,
    stride: i32,
}

// SAFETY: IMFSourceReader は COM の apartment-threaded ルールから見ると
// 本来スレッド固有だが、本実装は「生成したスレッドで使い切る前に、
// Box<dyn VideoSource> ごと 1 本の再生スレッドへ move してから使う」運用に限定しており、
// 複数スレッドから同時アクセスすることはない（送るのはオブジェクトの所有権の移動のみ）。
unsafe impl Send for MfVideo {}

impl MfVideo {
    /// 動画ファイルを開く。パスは絶対パスを推奨（MF の URL 解決に渡すため）。
    pub fn open(path: &Path) -> Result<Self> {
        ensure_mf_ready()?;

        let path_str = path
            .to_str()
            .with_context(|| format!("パスに非UTF-8文字が含まれています: {}", path.display()))?;
        // `Path::canonicalize()` が返す `\\?\` (拡張長パス) プレフィックスは
        // Media Foundation の URL 解決器がファイルパスとして認識できないため取り除く。
        let path_str = path_str.strip_prefix(r"\\?\").unwrap_or(path_str);
        let url = HSTRING::from(path_str);

        // SAFETY: 出力先 `out` は呼び出しの間生きている有効な `Option`。成功時は
        // `Some` を返す契約で、`None` なら直後に `context` で Err にする。
        let attrs: IMFAttributes = unsafe {
            let mut out: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut out, 1).context("MFCreateAttributes に失敗")?;
            out.context("MFCreateAttributes が空を返しました")?
        };
        // SAFETY: `attrs` は有効な COM オブジェクト、キーは有効な GUID 参照。
        unsafe {
            attrs
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .context("MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING の設定に失敗")?;
        }

        // SAFETY: `url` は NUL 終端の HSTRING で呼び出しの間生きている。`attrs` は有効。
        let reader: IMFSourceReader = unsafe { MFCreateSourceReaderFromURL(&url, &attrs) }
            .with_context(|| format!("Media Foundation で開けません: {}", path.display()))?;

        // RGB32 (実体は BGRX8) へ変換するようソースリーダーに指示する。
        // SAFETY: 引数なし。MFStartup は `ensure_mf_ready` で済んでいる。
        let out_type = unsafe { MFCreateMediaType() }.context("MFCreateMediaType に失敗")?;
        // SAFETY: `out_type`/`reader` は有効な COM オブジェクト、GUID は SDK 定義の参照。
        unsafe {
            out_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .context("MF_MT_MAJOR_TYPE の設定に失敗")?;
            out_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .context("MF_MT_SUBTYPE の設定に失敗")?;
            reader
                .SetCurrentMediaType(first_video_stream(), None, &out_type)
                .with_context(|| format!("RGB32 への変換設定に失敗: {}", path.display()))?;
        }

        let (width, height, fps, stride) = read_video_params(&reader)?;
        ensure!(
            width > 0 && height > 0,
            "動画のサイズ情報が取得できません: {}",
            path.display()
        );
        ensure!(
            fps > 0.0 && fps <= 240.0,
            "動画のフレームレートが不正です: {fps}"
        );

        Ok(Self {
            reader,
            width,
            height,
            fps,
            stride,
        })
    }

    /// IMFSample の映像バッファを straight RGBA8 として `rgba` に書き込む。
    fn sample_to_rgba_into(&self, sample: &IMFSample, rgba: &mut Vec<u8>) -> Result<()> {
        // SAFETY: `sample` は有効な COM オブジェクト。
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }
            .context("サンプルの連続バッファ化に失敗")?;
        let locked = LockedBuffer::lock(&buffer).context("動画バッファのロックに失敗")?;
        let src = locked.as_slice();

        let (w, h) = (self.width as usize, self.height as usize);
        let row_bytes = w * 4;
        let abs_stride = self.stride.unsigned_abs() as usize;
        ensure!(abs_stride >= row_bytes, "動画のストライドが幅より小さい");

        rgba.resize(row_bytes * h, 0);
        for y in 0..h {
            // 負のストライドはボトムアップ格納を意味する。
            let src_row_index = if self.stride < 0 { h - 1 - y } else { y };
            let src_off = src_row_index * abs_stride;
            ensure!(src_off + row_bytes <= src.len(), "動画バッファが短すぎます");
            let src_row = &src[src_off..src_off + row_bytes];
            let dst_row = &mut rgba[y * row_bytes..(y + 1) * row_bytes];
            // RGB32 (D3DFMT_X8R8G8B8 相当) はメモリ上 B,G,R,X の順。
            for x in 0..w {
                let so = x * 4;
                dst_row[so] = src_row[so + 2]; // R
                dst_row[so + 1] = src_row[so + 1]; // G
                dst_row[so + 2] = src_row[so]; // B
                dst_row[so + 3] = 255; // A
            }
        }
        // `locked` は `src` を使い終えたここで Drop され、Unlock する。
        Ok(())
    }
}

/// `IMFMediaBuffer::Lock` 中の領域。Drop で必ず `Unlock` する（途中の `?` や panic でも）。
/// スライスは `&self` / `&mut self` の借用から返すので、`Unlock` より後まで生き残れない。
struct LockedBuffer<'a> {
    buffer: &'a IMFMediaBuffer,
    ptr: *mut u8,
    cur_len: usize,
    max_len: usize,
}

impl<'a> LockedBuffer<'a> {
    fn lock(buffer: &'a IMFMediaBuffer) -> Result<Self> {
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let (mut max_len, mut cur_len) = (0u32, 0u32);
        // SAFETY: 3 つの出力先は呼び出しの間生きているローカル変数。成功した Lock は
        // 下の `LockedBuffer` の Drop が 1 回だけ Unlock する。
        unsafe { buffer.Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len)) }?;
        let locked = Self {
            buffer,
            ptr,
            cur_len: cur_len as usize,
            max_len: max_len as usize,
        };
        // 検査に失敗しても `locked` の Drop で Unlock される。
        ensure!(!locked.ptr.is_null(), "Lock が null を返しました");
        ensure!(
            locked.cur_len <= locked.max_len,
            "有効長 {} が最大長 {} を超えています",
            locked.cur_len,
            locked.max_len
        );
        Ok(locked)
    }

    /// 有効データ（`GetCurrentLength` 相当）。
    fn as_slice(&self) -> &[u8] {
        // SAFETY: `ptr` は非 null で、Lock〜Unlock の間 `max_len >= cur_len` バイトが
        // 読める（`lock` で検査済み）。戻り値の借用は `self`（= ロック期間）に縛られる。
        unsafe { std::slice::from_raw_parts(self.ptr, self.cur_len) }
    }

    /// 書き込み可能な全域（`GetMaxLength` 相当）。
    fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: `ptr` は非 null で、Lock〜Unlock の間 `max_len` バイトが読み書きできる
        // （`lock` で検査済み）。`&mut self` の借用で他の参照と重ならない。
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.max_len) }
    }
}

impl Drop for LockedBuffer<'_> {
    fn drop(&mut self) {
        // SAFETY: `lock` が成功した対の Unlock。`buffer` は借用で生きている。
        let _ = unsafe { self.buffer.Unlock() };
    }
}

impl VideoSource for MfVideo {
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn fps(&self) -> f32 {
        self.fps
    }

    fn next_frame(&mut self) -> Result<Option<Vec<u8>>> {
        let mut rgba = Vec::new();
        if self.next_frame_into(&mut rgba)? {
            Ok(Some(rgba))
        } else {
            Ok(None)
        }
    }

    fn next_frame_into(&mut self, buf: &mut Vec<u8>) -> Result<bool> {
        loop {
            let mut stream_flags: u32 = 0;
            let mut sample: Option<IMFSample> = None;
            // SAFETY: 出力先 `stream_flags`/`sample` は呼び出しの間生きているローカル変数。
            // `reader` は有効で、他のスレッドから同時に呼ばれない（`MfVideo` の運用規約）。
            unsafe {
                self.reader.ReadSample(
                    first_video_stream(),
                    0,
                    None,
                    Some(&mut stream_flags),
                    None,
                    Some(&mut sample),
                )
            }
            .context("ReadSample に失敗")?;

            if stream_flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return Ok(false);
            }

            let Some(sample) = sample else {
                // ストリームギャップ等でサンプルが無いことがある。継続して次を読む。
                continue;
            };
            self.sample_to_rgba_into(&sample, buf)?;
            return Ok(true);
        }
    }

    fn rewind(&mut self) -> Result<()> {
        let zero = PROPVARIANT::from(0i64);
        // SAFETY: `GUID::zeroed()`（時間単位 = 100ns）と有効な PROPVARIANT への参照。
        unsafe { self.reader.SetCurrentPosition(&GUID::zeroed(), &zero) }
            .context("動画の巻き戻しに失敗")?;
        Ok(())
    }
}

/// テスト・サンプル生成用: RGBA フレーム列を H.264/MP4 にエンコードする。
/// エンコーダ MFT が存在しない環境 (最小構成の CI 等) では呼び出し元が
/// エラーを見て skip できるよう `Err` を返す（panic はしない）。
pub fn encode_test_mp4(
    path: &Path,
    width: u32,
    height: u32,
    fps: u32,
    frames: &[Vec<u8>],
) -> Result<()> {
    ensure_mf_ready()?;
    ensure!(width > 0 && height > 0 && fps > 0, "サイズ/fps が不正です");

    let path_str = path
        .to_str()
        .with_context(|| format!("パスに非UTF-8文字が含まれています: {}", path.display()))?;
    let path_str = path_str.strip_prefix(r"\\?\").unwrap_or(path_str);
    let url = HSTRING::from(path_str);

    // SAFETY: `url` は NUL 終端の HSTRING で呼び出しの間生きている。バイトストリーム/属性は None。
    let writer: IMFSinkWriter = unsafe { MFCreateSinkWriterFromURL(&url, None, None) }
        .with_context(|| format!("SinkWriter を作成できません: {}", path.display()))?;

    let frame_size = ((width as u64) << 32) | (height as u64);
    let frame_rate = ((fps as u64) << 32) | 1u64;

    // 出力 (H.264)
    // SAFETY: 引数なし（MFStartup 済み）。
    let out_type = unsafe { MFCreateMediaType() }?;
    // SAFETY: `out_type` は有効な COM オブジェクト、キーは SDK 定義の定数の参照。
    unsafe {
        out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        out_type.SetUINT32(&MF_MT_AVG_BITRATE, 2_000_000)?;
        out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MF_VIDEO_INTERLACE_PROGRESSIVE)?;
        out_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size)?;
        out_type.SetUINT64(&MF_MT_FRAME_RATE, frame_rate)?;
    }
    // SAFETY: `writer`/`out_type` は有効な COM オブジェクト。
    let stream_index = unsafe { writer.AddStream(&out_type) }.context("AddStream に失敗")?;

    // 入力 (RGB32 = BGRX8, 非圧縮)
    // SAFETY: 引数なし（MFStartup 済み）。
    let in_type = unsafe { MFCreateMediaType() }?;
    // SAFETY: `in_type`/`writer` は有効な COM オブジェクト、`stream_index` は AddStream の戻り値。
    unsafe {
        in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MF_VIDEO_INTERLACE_PROGRESSIVE)?;
        in_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        in_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size)?;
        in_type.SetUINT64(&MF_MT_FRAME_RATE, frame_rate)?;
        // RGB32 は MF_MT_DEFAULT_STRIDE 未設定だと DIB 規約の「ボトムアップ」と
        // 解釈され、トップダウンで渡す本実装のバッファが上下反転してエンコード
        // されてしまう。正のストライドを明示してトップダウンだと宣言する。
        in_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4)?;
        writer
            .SetInputMediaType(stream_index, &in_type, None)
            .context("SetInputMediaType に失敗")?;
    }

    // SAFETY: 入出力メディアタイプの設定が済んだ有効な `writer`。
    unsafe { writer.BeginWriting() }.context("BeginWriting に失敗")?;

    let row_bytes = (width as usize) * 4;
    let buf_len = row_bytes * height as usize;
    ensure!(
        u32::try_from(buf_len).is_ok(),
        "フレームが大きすぎます: {buf_len} バイト"
    );
    let duration_100ns = 10_000_000i64 / fps as i64;
    let mut time_100ns = 0i64;

    for rgba in frames {
        ensure!(
            rgba.len() == buf_len,
            "フレームサイズが width*height*4 と不一致"
        );

        // SAFETY: `buf_len` は u32 に収まることを上で確認済み。
        let mem_buffer = unsafe { MFCreateMemoryBuffer(buf_len as u32) }
            .context("MFCreateMemoryBuffer に失敗")?;
        {
            let mut locked = LockedBuffer::lock(&mem_buffer).context("バッファロックに失敗")?;
            ensure!(
                locked.max_len >= buf_len,
                "メモリバッファが要求 {buf_len} バイトより小さい: {}",
                locked.max_len
            );
            let dst = &mut locked.as_mut_slice()[..buf_len];
            for i in 0..(width as usize * height as usize) {
                let so = i * 4;
                // RGBA -> BGRX (RGB32) へ並べ替え
                dst[so] = rgba[so + 2]; // B
                dst[so + 1] = rgba[so + 1]; // G
                dst[so + 2] = rgba[so]; // R
                dst[so + 3] = 0; // X
            }
            // `locked` はこのブロックの終わりで Unlock される（SetCurrentLength の前）。
        }
        // SAFETY: Unlock 済みの有効なバッファ。長さは確保サイズ以下。
        unsafe { mem_buffer.SetCurrentLength(buf_len as u32) }
            .context("SetCurrentLength に失敗")?;

        // SAFETY: 引数なし（MFStartup 済み）。
        let sample: IMFSample = unsafe { MFCreateSample() }.context("MFCreateSample に失敗")?;
        // SAFETY: `sample`/`mem_buffer`/`writer` は有効な COM オブジェクトで、`stream_index` は
        // この writer の有効なストリーム。時刻/長さは値渡し。
        unsafe {
            sample.AddBuffer(&mem_buffer).context("AddBuffer に失敗")?;
            sample
                .SetSampleTime(time_100ns)
                .context("SetSampleTime に失敗")?;
            sample
                .SetSampleDuration(duration_100ns)
                .context("SetSampleDuration に失敗")?;
            writer
                .WriteSample(stream_index, &sample)
                .context("WriteSample に失敗")?;
        }
        time_100ns += duration_100ns;
    }

    // SAFETY: 書き込みを開始済みの有効な `writer`。
    unsafe { writer.Finalize() }.context("Finalize に失敗")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// メモリバッファのロック: 書き込み域は最大長、読み取り域は現在長で、いずれも
    /// 範囲内に収まる。Drop で Unlock され、続けて何度でもロックできる。
    #[test]
    fn locked_buffer_exposes_current_and_max_ranges_and_unlocks_on_drop() {
        ensure_mf_ready().unwrap();
        // SAFETY: 引数は値のみ（MFStartup 済み）。
        let buffer = unsafe { MFCreateMemoryBuffer(16) }.unwrap();
        {
            let mut locked = LockedBuffer::lock(&buffer).unwrap();
            assert!(locked.max_len >= 16);
            assert_eq!(
                locked.as_slice().len(),
                0,
                "現在長は SetCurrentLength まで 0"
            );
            locked.as_mut_slice()[..16].copy_from_slice(&[7u8; 16]);
        }
        // SAFETY: Unlock 済みの有効なバッファ。
        unsafe { buffer.SetCurrentLength(16) }.unwrap();
        for _ in 0..3 {
            let locked = LockedBuffer::lock(&buffer).unwrap();
            assert_eq!(locked.as_slice(), &[7u8; 16]);
        }
    }

    /// 上下非対称のフィクスチャ: 上 1/3 は青一色、下 1/3 は赤一色、
    /// 中央帯はフレーム番号依存の色 (r, 90, 200)。上下反転 (flip) すると
    /// 上端と下端の色が入れ替わるので検出できる。
    fn make_frame(w: u32, h: u32, r: u8) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let px: [u8; 4] = if y < h / 3 {
                [0, 0, 255, 255] // 上帯: 青
            } else if y >= h * 2 / 3 {
                [255, 0, 0, 255] // 下帯: 赤
            } else {
                [r, 90, 200, 255] // 中央帯: フレームごとに変化
            };
            for _ in 0..w {
                rgba.extend_from_slice(&px);
            }
        }
        rgba
    }

    /// (R,G,B) が期待値と誤差 tol 以内か。
    fn assert_rgb_near(rgba: &[u8], at: usize, expected: [u8; 3], tol: i32, what: &str) {
        for (c, name) in ["R", "G", "B"].iter().enumerate() {
            let actual = rgba[at + c] as i32;
            let want = expected[c] as i32;
            assert!(
                (actual - want).abs() <= tol,
                "{what}: {name}={actual} 期待≒{want} (H.264 圧縮誤差 ±{tol} を超過)"
            );
        }
    }

    /// H.264 エンコーダ MFT が無い環境 (最小構成の CI 等) では
    /// テストを事実上 skip する。
    fn try_encode(path: &Path, w: u32, h: u32, fps: u32, frames: &[Vec<u8>]) -> bool {
        match encode_test_mp4(path, w, h, fps, frames) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("H.264 エンコーダが利用できないためスキップ: {e:#}");
                false
            }
        }
    }

    #[test]
    fn encode_and_decode_roundtrip() {
        let (w, h, fps, n) = (128u32, 96u32, 10u32, 16u32);
        let frames: Vec<Vec<u8>> = (0..n)
            .map(|i| make_frame(w, h, (i * 255 / (n - 1)) as u8))
            .collect();

        let dir = std::env::temp_dir().join(format!("noiria_mf_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.mp4");

        if !try_encode(&path, w, h, fps, &frames) {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        let mut video = match MfVideo::open(&path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("デコーダが利用できないためスキップ: {e:#}");
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }
        };
        assert_eq!(video.width(), w);
        assert_eq!(video.height(), h);
        assert!(
            (video.fps() - fps as f32).abs() < 1.0,
            "fps={}",
            video.fps()
        );

        let mut decoded = Vec::new();
        while let Some(rgba) = video.next_frame().unwrap() {
            assert_eq!(rgba.len(), (w * h * 4) as usize);
            decoded.push(rgba);
        }
        assert!(
            decoded.len() as i64 >= n as i64 - 2 && decoded.len() as i64 <= n as i64 + 2,
            "デコードされたフレーム数がずれすぎ: {} (期待 ≒{n})",
            decoded.len()
        );

        // 上端中央 (y=2)・下端中央 (y=h-3)・中心の 3 点を検証する。
        // 上下反転 (flip) があれば top/bottom の色が入れ替わって FAIL する。
        let top = ((2 * w + w / 2) * 4) as usize;
        let bottom = (((h - 3) * w + w / 2) * 4) as usize;
        let center = ((h / 2 * w + w / 2) * 4) as usize;
        for (i, rgba) in decoded.iter().enumerate() {
            let idx = i.min(n as usize - 1);
            let expected_r = (idx as u32 * 255 / (n - 1)) as u8;
            assert_rgb_near(
                rgba,
                top,
                [0, 0, 255],
                40,
                &format!("フレーム {i} 上端(青のはず)"),
            );
            assert_rgb_near(
                rgba,
                bottom,
                [255, 0, 0],
                40,
                &format!("フレーム {i} 下端(赤のはず)"),
            );
            assert_rgb_near(
                rgba,
                center,
                [expected_r, 90, 200],
                40,
                &format!("フレーム {i} 中心"),
            );
        }

        video.rewind().unwrap();
        let again = video.next_frame().unwrap();
        assert!(again.is_some(), "巻き戻し後に再度読めるはず");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seek_frame_roundtrip_matches_sequential_frame() {
        let (w, h, fps, n) = (128u32, 96u32, 10u32, 12u32);
        let frames: Vec<Vec<u8>> = (0..n)
            .map(|i| make_frame(w, h, (i * 255 / (n - 1)) as u8))
            .collect();
        let dir = std::env::temp_dir().join(format!("noiria_mf_seek_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("seek-roundtrip.mp4");

        if !try_encode(&path, w, h, fps, &frames) {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        let mut video = match MfVideo::open(&path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("デコーダが利用できないためスキップ: {e:#}");
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }
        };
        let mut sequential = Vec::new();
        while let Some(frame) = video.next_frame().unwrap() {
            sequential.push(frame);
        }
        assert!(
            sequential.len() > 6,
            "中間 seek 用のフレームが不足しています"
        );

        video.seek_frame(6).unwrap();
        assert_eq!(
            video.next_frame().unwrap().unwrap(),
            sequential[6],
            "seek 後の次フレームが逐次デコード結果と一致しません"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
