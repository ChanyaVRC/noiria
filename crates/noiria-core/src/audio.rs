//! オーディオスレッド。チャンネルは文字列名（"bgm" / "bgm2" / "voice" /
//! "se0".."se15"。"se" は "se0" の別名）を `channel_index` で index に解決して扱う。
//! "bgm2" は [xchgbgm]（クロスフェード切替）が使う内部チャンネル。
//!
//! 出力層 (`wasapi_out.rs`（Windows）/ `alsa_out.rs`（Unix、dlopen）、
//! いずれも OS 境界 API) とミキシング層
//! (`mixer.rs`, 純 Rust・OS 非依存) を橋渡しする。フェード・一時停止・ループ・
//! 音量の実処理はすべて `mixer::Mixer` に委譲し、ここでは:
//! - AudioCmd を受けてデコード（WAV / OGG(Vorbis) は自作デコーダ。コマンドスレッドを
//!   止めないよう別スレッドで行い、完了後にミキサへ差し込む）
//! - デコード結果を `mixer::PcmSource` としてミキサへ渡す
//! - 出力スレッドとコマンド処理スレッドの橋渡し（出力が死んでも null シンクで
//!   ミキサを消費し続け、バックオフ付きで開き直す）
//! - `AudioShared`（再生中/フェード中/演奏終了フラグ・現在位置）の更新
//!
//! だけを行う。外部オーディオライブラリ (rodio 等) には一切依存しない。
//! WAV (`codec::wav`)・OGG/Vorbis (`codec::ogg` + `codec::vorbis`) とも
//! クリーンルーム自作デコーダで逐次ストリーミングデコードし、定数メモリで再生する。
//! MP3/FLAC は対応を打ち切った（`build_source` 参照）。

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use crossbeam_channel::{select, Receiver, Sender};

use crate::assets::GameDir;
use crate::codec::ogg::{OggBookmark, OggPacketReader};
use crate::codec::vorbis::VorbisStreamDecoder;
use crate::codec::wav::{parse_header as parse_wav_header, WavHeader};
pub use crate::mixer::FadeThen;
use crate::mixer::{ChannelConverter, Mixer, PcmSource, PreparedSource, Resampler, NUM_SLOTS};

pub const CH_BGM: usize = 0;
pub const CH_SE: usize = 1;
pub const CH_VOICE: usize = 2;
/// [xchgbgm] クロスフェード用の第 2 BGM チャンネル。
pub const CH_BGM2: usize = 3;
/// SE バッファ数の上限（"se0".."se15"）。"se0" は CH_SE の別名。
pub const MAX_SE_BUFS: usize = 16;

/// チャンネル名 → ミキサスロット index。
/// "se1".."se15" は CH_BGM2 の後ろ（4..=18）に並ぶ。
pub fn channel_index(name: &str) -> Option<usize> {
    match name {
        "bgm" => Some(CH_BGM),
        "bgm2" => Some(CH_BGM2),
        "voice" => Some(CH_VOICE),
        "se" | "se0" => Some(CH_SE),
        _ => {
            let n: usize = name.strip_prefix("se")?.parse().ok()?;
            (1..MAX_SE_BUFS).contains(&n).then_some(CH_BGM2 + n)
        }
    }
}

#[derive(Debug)]
pub enum AudioCmd {
    Play {
        ch: usize,
        /// GameDir ルート相対パス（`build_source` が `GameDir::open_reader` 経由で読む）。
        rel: String,
        looping: bool,
        volume: f32,
        fade_in_ms: u32,
        start_position_ms: u64,
        start_paused: bool,
        request_id: u64,
    },
    Stop {
        ch: usize,
        fade_out_ms: u32,
        request_id: u64,
    },
    SetVolume {
        ch: usize,
        volume: f32,
    },
    /// 一時停止（サンプル位置を保持したまま無音化）。
    Pause {
        ch: usize,
    },
    /// 一時停止からの再開。
    Resume {
        ch: usize,
    },
    /// 現在音量から target_vol へ ms かけて線形フェードし、完了時に then を適用。
    FadeTo {
        ch: usize,
        target_vol: f32,
        ms: u32,
        then: FadeThen,
    },
    Quit,
}

/// スクリプトスレッドから参照する再生状態。
#[derive(Default)]
pub struct AudioShared {
    playing: [AtomicBool; NUM_SLOTS],
    fading: [AtomicBool; NUM_SLOTS],
    finished: [AtomicBool; NUM_SLOTS],
    position_ms: [AtomicU64; NUM_SLOTS],
    position_requests: Mutex<PositionRequests>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PositionRequest {
    Play(u64),
    Stop(u64),
}

struct PositionRequests {
    next_id: u64,
    pending: [Option<PositionRequest>; NUM_SLOTS],
}

impl Default for PositionRequests {
    fn default() -> Self {
        Self {
            next_id: 0,
            pending: [None; NUM_SLOTS],
        }
    }
}

impl AudioShared {
    pub fn is_playing(&self, ch: usize) -> bool {
        self.playing[ch].load(Ordering::Relaxed)
    }
    /// `Play` を要求済みで、まだミキサに差し込まれていない（デコード中）か。デコード失敗・`Stop`・
    /// 別の `Play` による上書きで偽に戻る。`is_playing` はこの間も偽なので、再生終了待ちは
    /// 「`is_playing` でも `is_play_pending` でもない」ことで終了を判定する（固定の猶予時間は不要）。
    pub fn is_play_pending(&self, ch: usize) -> bool {
        let requests = self
            .position_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        matches!(requests.pending[ch], Some(PositionRequest::Play(_)))
    }
    /// 音声スレッドへ届けられなかった（受信側が無い）`Play` 要求を取り下げる。音声スレッドが
    /// 無い（ヘッドレス等）と誰も確認応答しないため、送れなかった側が自分で確認応答する。
    pub fn cancel_play_request(&self, ch: usize, request_id: u64, position_ms: u64) {
        self.acknowledge_play_requested(ch, request_id, position_ms);
    }
    /// フェード進行中か（[wb]/[wf] 用）。
    pub fn is_fading(&self, ch: usize) -> bool {
        self.fading[ch].load(Ordering::Relaxed)
    }
    /// 非ループ再生が自然終端したか（[wl]/[setbgmstop] 用。明示 stop では立たない）。
    pub fn is_finished(&self, ch: usize) -> bool {
        self.finished[ch].load(Ordering::Relaxed)
    }
    pub fn position_ms(&self, ch: usize) -> u64 {
        self.position_ms[ch].load(Ordering::Relaxed)
    }
    pub fn mark_play_requested(&self, ch: usize, position_ms: u64) -> u64 {
        self.mark_position_requested(ch, position_ms, PositionRequest::Play)
    }
    pub fn mark_stop_requested(&self, ch: usize) -> u64 {
        self.mark_position_requested(ch, 0, PositionRequest::Stop)
    }
    fn set_playing(&self, ch: usize, v: bool) {
        self.playing[ch].store(v, Ordering::Relaxed);
    }
    fn set_fading(&self, ch: usize, v: bool) {
        self.fading[ch].store(v, Ordering::Relaxed);
    }
    fn set_finished(&self, ch: usize, v: bool) {
        self.finished[ch].store(v, Ordering::Relaxed);
    }
    fn set_position_ms(&self, ch: usize, v: u64) {
        self.position_ms[ch].store(v, Ordering::Relaxed);
    }
    fn mark_position_requested(
        &self,
        ch: usize,
        position_ms: u64,
        request: fn(u64) -> PositionRequest,
    ) -> u64 {
        let mut requests = self
            .position_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        requests.next_id = requests.next_id.wrapping_add(1).max(1);
        let request_id = requests.next_id;
        requests.pending[ch] = Some(request(request_id));
        self.set_position_ms(ch, position_ms);
        request_id
    }
    fn acknowledge_play_requested(&self, ch: usize, request_id: u64, position_ms: u64) {
        let mut requests = self
            .position_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if requests.pending[ch] == Some(PositionRequest::Play(request_id)) {
            self.set_position_ms(ch, position_ms);
            requests.pending[ch] = None;
        }
    }
    fn acknowledge_stop_requested(&self, ch: usize, request_id: u64, is_playing: bool) {
        let mut requests = self
            .position_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !is_playing && requests.pending[ch] == Some(PositionRequest::Stop(request_id)) {
            self.set_position_ms(ch, 0);
            requests.pending[ch] = None;
        }
    }
    fn sync_positions(&self, mixer: &Mixer) {
        let mut requests = self
            .position_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (ch, pending) in requests.pending.iter_mut().enumerate() {
            match *pending {
                Some(PositionRequest::Play(_)) => {}
                Some(PositionRequest::Stop(_)) if mixer.is_playing(ch) => {}
                Some(PositionRequest::Stop(_)) => {
                    self.set_position_ms(ch, 0);
                    *pending = None;
                }
                None => self.set_position_ms(ch, mixer.position_ms(ch)),
            }
        }
    }
}

/// コマンド処理（フェード進行含む）とデバイス I/O の同期に使う周期。
/// フェード自体は `Mixer` 内部で出力サンプル数ベースに進むので、この
/// 周期は「再生状態と現在位置をどれくらいの頻度で同期するか」だけを決める。
const TICK: Duration = Duration::from_millis(10);

/// 1/2/3/4 バイトサンプルすべての公倍数（12 の倍数）に揃えた WAV ストリーミング読み込み長。
const WAV_STREAM_BUF_BYTES: usize = 16_380;

/// WAV (RIFF/WAVE) を固定サイズバッファで逐次デコードする `PcmSource`。
/// ヘッダを一度だけ解析し、再生・巻き戻し・シーク中にヒープ確保しない。
pub struct StreamingWavSource<R> {
    reader: R,
    header: WavHeader,
    total_samples: u64,
    usable_bytes: u64,
    file_data_pos: u64,
    sample_pos: u64,
    raw_buf: Vec<u8>,
    raw_pos: usize,
    raw_len: usize,
}

impl<R: Read + Seek + Send> StreamingWavSource<R> {
    pub fn new(mut reader: R) -> Result<Self> {
        let header = parse_wav_header(&mut reader)?;
        let bps = header.format.bytes_per_sample() as u64;
        let total_samples = header.data_len / bps;
        let usable_bytes = total_samples * bps;
        let mut src = Self {
            reader,
            header,
            total_samples,
            usable_bytes,
            file_data_pos: 0,
            sample_pos: 0,
            raw_buf: vec![0u8; WAV_STREAM_BUF_BYTES],
            raw_pos: 0,
            raw_len: 0,
        };
        // デコードスレッド上で先頭チャンクを先読みしておき、短い SE は
        // 再生スレッドから OS read ゼロで鳴らし切れるようにする。
        if total_samples > 0 {
            src.refill();
        }
        Ok(src)
    }

    fn refill(&mut self) -> bool {
        let bps = self.header.format.bytes_per_sample();
        let rem = self.raw_len - self.raw_pos;
        if rem > 0 {
            self.raw_buf.copy_within(self.raw_pos..self.raw_len, 0);
        }
        self.raw_pos = 0;
        self.raw_len = rem;

        while self.raw_len < bps {
            let remaining_in_chunk = self.usable_bytes.saturating_sub(self.file_data_pos);
            if remaining_in_chunk == 0 {
                return false;
            }
            let max_read = (self.raw_buf.len() - self.raw_len).min(remaining_in_chunk as usize);
            match self
                .reader
                .read(&mut self.raw_buf[self.raw_len..self.raw_len + max_read])
            {
                Ok(0) => return false,
                Ok(n) => {
                    self.raw_len += n;
                    self.file_data_pos += n as u64;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    log::warn!("WAV ストリーム読み込みエラー（再生を終了します）: {e}");
                    self.file_data_pos = self.usable_bytes;
                    return false;
                }
            }
        }
        true
    }
}

impl<R: Read + Seek + Send> PcmSource for StreamingWavSource<R> {
    fn channels(&self) -> u16 {
        self.header.channels
    }

    fn sample_rate(&self) -> u32 {
        self.header.sample_rate
    }

    fn next_sample(&mut self) -> Option<f32> {
        let mut one = [0.0f32];
        (self.read(&mut one) == 1).then_some(one[0])
    }

    fn read(&mut self, out: &mut [f32]) -> usize {
        let bps = self.header.format.bytes_per_sample();
        let mut written = 0;
        while written < out.len() {
            if self.raw_len - self.raw_pos < bps && !self.refill() {
                break;
            }
            let decoded = self.header.format.decode_into(
                &self.raw_buf[self.raw_pos..self.raw_len],
                &mut out[written..],
            );
            if decoded == 0 {
                break;
            }
            self.raw_pos += decoded * bps;
            self.sample_pos += decoded as u64;
            written += decoded;
        }
        written
    }

    fn skip_samples(&mut self, samples: u64) -> bool {
        let remaining = self.total_samples.saturating_sub(self.sample_pos);
        if samples > remaining {
            self.sample_pos = self.total_samples;
            self.file_data_pos = self.usable_bytes;
            self.raw_pos = 0;
            self.raw_len = 0;
            return false;
        }
        if samples == 0 {
            return true;
        }
        let bps = self.header.format.bytes_per_sample() as u64;
        let skip_bytes = samples * bps;
        let buffered = (self.raw_len - self.raw_pos) as u64;
        if skip_bytes <= buffered {
            self.raw_pos += skip_bytes as usize;
            self.sample_pos += samples;
            return true;
        }
        let new_sample_pos = self.sample_pos + samples;
        let target_data_offset = new_sample_pos * bps;
        let Some(abs_offset) = self.header.data_offset.checked_add(target_data_offset) else {
            return false;
        };
        if self.reader.seek(SeekFrom::Start(abs_offset)).is_err() {
            return false;
        }
        self.sample_pos = new_sample_pos;
        self.file_data_pos = target_data_offset;
        self.raw_pos = 0;
        self.raw_len = 0;
        true
    }

    fn frame_count_upper_bound(&self) -> Option<u64> {
        Some(self.total_samples / self.header.channels.max(1) as u64)
    }

    fn rewind(&mut self) -> bool {
        if self.total_samples == 0 {
            return false;
        }
        if self
            .reader
            .seek(SeekFrom::Start(self.header.data_offset))
            .is_err()
        {
            return false;
        }
        self.sample_pos = 0;
        self.file_data_pos = 0;
        self.raw_pos = 0;
        self.raw_len = 0;
        true
    }
}

/// Ogg/Vorbis をページ・パケット単位で逐次デコードする `PcmSource`。
/// 現在の 1 パケット分のデコード済み PCM のみを保持し、長尺 BGM でもメモリ使用量を一定に保つ。
pub struct StreamingVorbisSource<R> {
    ogg: OggPacketReader<R>,
    decoder: VorbisStreamDecoder,
    audio_start: OggBookmark,
    frame_upper_bound: Option<u64>,
    pcm_buf: Vec<f32>,
    pcm_pos: usize,
    ended: bool,
    has_any_samples: bool,
}

impl<R: Read + Seek + Send> StreamingVorbisSource<R> {
    pub fn new(reader: R) -> Result<Self> {
        let mut ogg = OggPacketReader::new(reader);
        let id_packet = ogg
            .next_packet()?
            .context("Vorbis の 3 ヘッダパケットが揃っていません")?;
        let _comment_packet = ogg
            .next_packet()?
            .context("Vorbis の 3 ヘッダパケットが揃っていません")?;
        let setup_packet = ogg
            .next_packet()?
            .context("Vorbis の 3 ヘッダパケットが揃っていません")?;
        let decoder = VorbisStreamDecoder::new(&id_packet, &setup_packet)?;
        let audio_start = ogg.bookmark()?;

        // 各有効音声パケットは最小でも 1 バイト以上を占め、overlap-add は最大 blocksize_1/2 フレームを
        // 返す。granule_position を使わないのは、合成ストリームで granule=0 のまま音声パケットを
        // 持つ場合があるため（過小見積もりはシーク失敗になる）。
        let remaining_bytes = ogg.stream_len()?.saturating_sub(audio_start.byte_offset());
        let frame_upper_bound =
            Some(remaining_bytes.saturating_mul((decoder.max_blocksize() / 2) as u64));

        let mut src = Self {
            ogg,
            decoder,
            audio_start,
            frame_upper_bound,
            pcm_buf: Vec::new(),
            pcm_pos: 0,
            ended: false,
            has_any_samples: false,
        };
        // デコードスレッド上で先頭 PCM ブロックを用意し、再生開始時の初回遅延と
        // 先頭音声ページの破損を前倒しで処理する。
        src.prime_first_pcm()?;
        src.has_any_samples = !src.pcm_buf.is_empty();
        Ok(src)
    }

    fn prime_first_pcm(&mut self) -> Result<()> {
        loop {
            match self.ogg.next_packet()? {
                Some(pkt) => {
                    let pcm = self.decoder.decode_packet(&pkt)?;
                    if !pcm.is_empty() {
                        self.pcm_buf = pcm;
                        self.pcm_pos = 0;
                        return Ok(());
                    }
                }
                None => {
                    self.ended = true;
                    return Ok(());
                }
            }
        }
    }

    fn fill_next_pcm(&mut self) -> bool {
        if self.ended {
            return false;
        }
        loop {
            match self.ogg.next_packet() {
                Ok(Some(pkt)) => match self.decoder.decode_packet(&pkt) {
                    Ok(pcm) => {
                        if !pcm.is_empty() {
                            self.pcm_buf = pcm;
                            self.pcm_pos = 0;
                            return true;
                        }
                    }
                    Err(e) => {
                        log::warn!("OGG/Vorbis デコードエラー（再生を終了します）: {e:#}");
                        self.ended = true;
                        return false;
                    }
                },
                Ok(None) => {
                    self.ended = true;
                    return false;
                }
                Err(e) => {
                    log::warn!("OGG ストリーム読み込みエラー（再生を終了します）: {e:#}");
                    self.ended = true;
                    return false;
                }
            }
        }
    }
}

impl<R: Read + Seek + Send> PcmSource for StreamingVorbisSource<R> {
    fn channels(&self) -> u16 {
        self.decoder.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.decoder.sample_rate()
    }

    fn next_sample(&mut self) -> Option<f32> {
        if self.pcm_pos >= self.pcm_buf.len() && !self.fill_next_pcm() {
            return None;
        }
        let s = self.pcm_buf[self.pcm_pos];
        self.pcm_pos += 1;
        Some(s)
    }

    fn read(&mut self, out: &mut [f32]) -> usize {
        let mut written = 0;
        while written < out.len() {
            if self.pcm_pos >= self.pcm_buf.len() && !self.fill_next_pcm() {
                break;
            }
            let take = (self.pcm_buf.len() - self.pcm_pos).min(out.len() - written);
            out[written..written + take]
                .copy_from_slice(&self.pcm_buf[self.pcm_pos..self.pcm_pos + take]);
            self.pcm_pos += take;
            written += take;
        }
        written
    }

    fn skip_samples(&mut self, mut samples: u64) -> bool {
        while samples > 0 {
            if self.pcm_pos >= self.pcm_buf.len() && !self.fill_next_pcm() {
                return false;
            }
            let avail = (self.pcm_buf.len() - self.pcm_pos) as u64;
            let take = samples.min(avail);
            self.pcm_pos += take as usize;
            samples -= take;
        }
        true
    }

    fn frame_count_upper_bound(&self) -> Option<u64> {
        self.frame_upper_bound
    }

    fn rewind(&mut self) -> bool {
        if !self.has_any_samples {
            return false;
        }
        if self.ogg.seek_to_bookmark(&self.audio_start).is_err() {
            self.ended = true;
            return false;
        }
        self.decoder.reset();
        self.pcm_buf.clear();
        self.pcm_pos = 0;
        self.ended = false;
        true
    }
}

/// 音声ファイルを開いて出力フォーマット (device channels / rate) 変換を繋いだ
/// `PcmSource` を返す。戻り値の bool は「`Mixer` 側でループを面倒見る必要があるか」。
fn build_source(
    game_dir: &GameDir,
    rel: &str,
    looping: bool,
    out_channels: u16,
    out_rate: u32,
) -> DecodedSource {
    let ext = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "wav" => {
            let reader = game_dir.open_reader(rel)?;
            let src = StreamingWavSource::new(reader)?;
            ensure!(src.channels() > 0, "WAV のチャンネル数が不正: {rel}");
            ensure!(src.sample_rate() > 0, "WAV のサンプルレートが不正: {rel}");
            let resampled = Resampler::new(src, out_rate);
            let converted = ChannelConverter::new(resampled, out_channels);
            Ok((Box::new(converted), looping))
        }
        "ogg" | "oga" => {
            let reader = game_dir.open_reader(rel)?;
            let src = StreamingVorbisSource::new(reader)?;
            ensure!(src.channels() > 0, "OGG のチャンネル数が不正: {rel}");
            ensure!(src.sample_rate() > 0, "OGG のサンプルレートが不正: {rel}");
            let resampled = Resampler::new(src, out_rate);
            let converted = ChannelConverter::new(resampled, out_channels);
            Ok((Box::new(converted), looping))
        }
        "mp3" | "flac" => {
            bail!("MP3/FLAC は非対応になりました。WAV か OGG (Vorbis) を使ってください")
        }
        other => {
            bail!("非対応の音声形式です（WAV / OGG(Vorbis) のみ対応）: 拡張子={other:?}")
        }
    }
}

/// `build_source` と同じ形の関数。テストで差し替えられるよう `Arc<dyn Fn>` で持つ。
type DecodeFn = Arc<dyn Fn(&GameDir, &str, bool, u16, u32) -> DecodedSource + Send + Sync>;
type DecodedSource = Result<(Box<dyn PcmSource>, bool)>;

/// 開いた出力デバイス 1 本。render スレッド上で生成し、同じスレッドで使い切って破棄する。
trait OutputDevice {
    fn channels(&self) -> u16;
    fn sample_rate(&self) -> u32;
    /// `stop` が立つかデバイスエラーが起きるまでブロックして `fill` で書き続ける。
    fn run(self: Box<Self>, stop: &AtomicBool, fill: &mut dyn FnMut(&mut [f32])) -> Result<()>;
}

/// 既定の出力デバイスを（開き直しのたびに再取得して）開く。
type OpenDevice = Box<dyn FnMut() -> Result<Box<dyn OutputDevice>> + Send>;

#[cfg(windows)]
impl OutputDevice for crate::wasapi_out::WasapiOutput {
    fn channels(&self) -> u16 {
        crate::wasapi_out::WasapiOutput::channels(self)
    }
    fn sample_rate(&self) -> u32 {
        crate::wasapi_out::WasapiOutput::sample_rate(self)
    }
    fn run(self: Box<Self>, stop: &AtomicBool, fill: &mut dyn FnMut(&mut [f32])) -> Result<()> {
        crate::wasapi_out::WasapiOutput::run(*self, stop, |buf| fill(buf))
    }
}

#[cfg(unix)]
impl OutputDevice for crate::alsa_out::AlsaOutput {
    fn channels(&self) -> u16 {
        crate::alsa_out::AlsaOutput::channels(self)
    }
    fn sample_rate(&self) -> u32 {
        crate::alsa_out::AlsaOutput::sample_rate(self)
    }
    fn run(self: Box<Self>, stop: &AtomicBool, fill: &mut dyn FnMut(&mut [f32])) -> Result<()> {
        crate::alsa_out::AlsaOutput::run(*self, stop, |buf| fill(buf))
    }
}

/// **OS ごとの分岐はここ 1 箇所だけ**（Windows=WASAPI / Unix=ALSA(dlopen) /
/// それ以外は未対応として常に Err = 無音続行）。
fn system_open() -> OpenDevice {
    #[cfg(windows)]
    {
        Box::new(|| Ok(Box::new(crate::wasapi_out::WasapiOutput::open_default()?)))
    }
    #[cfg(unix)]
    {
        Box::new(|| Ok(Box::new(crate::alsa_out::AlsaOutput::open_default()?)))
    }
    #[cfg(not(any(windows, unix)))]
    {
        Box::new(|| bail!("このプラットフォームでは音声出力は未実装です"))
    }
}

/// 出力が死んだとき・最初から無いときの再接続の間隔。
#[derive(Clone, Copy)]
struct RenderConfig {
    initial_backoff: Duration,
    max_backoff: Duration,
    /// これ以上続いた再生は「健全だった」とみなし、次の失敗のバックオフを初期値へ戻す。
    healthy_run: Duration,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            initial_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(5),
            healthy_run: Duration::from_secs(2),
        }
    }
}

/// デバイスが無いときにミキサが前提にするフォーマット。
const FALLBACK_FORMAT: (u16, u32) = (2, 48_000);

/// オーディオスレッドを起動する。デバイスが無い環境やこのプラットフォームで
/// 音声出力が未実装の場合も、時間駆動の null シンクがミキサを実時間で消費して
/// 無音で続行する（`finished` 等の再生状態も通常どおり進む）。
/// `game_dir` は `AudioCmd::Play` の相対パスをデコード時に読むために使う
/// （W5: GameDir が内部 Arc 化されたので Clone してスレッドへ渡すのは安価）。
pub fn spawn(
    rx: Receiver<AudioCmd>,
    shared: Arc<AudioShared>,
    game_dir: GameDir,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("noiria-audio".into())
        .spawn(move || {
            run(
                rx,
                shared,
                game_dir,
                system_open(),
                RenderConfig::default(),
                Arc::new(build_source),
            )
        })
        .expect("オーディオスレッドの起動に失敗")
}

/// デバイスの open から再生ループまでを専用の描画スレッドに閉じ込める
/// （WASAPI の COM オブジェクトは「開いたスレッドで使い切る」制約があるため。
/// ALSA にその制約は無いが、同じスレッド構造をそのまま流用する）。
/// このコマンド処理スレッドは AudioCmd を受けて Arc<Mutex<Mixer>> を更新するだけ。
fn run(
    rx: Receiver<AudioCmd>,
    shared: Arc<AudioShared>,
    game_dir: GameDir,
    open: OpenDevice,
    cfg: RenderConfig,
    decode: DecodeFn,
) {
    let (init_tx, init_rx) = crossbeam_channel::bounded::<(u16, u32, bool)>(1);
    let (mixer_tx, mixer_rx) = crossbeam_channel::bounded::<Arc<Mutex<Mixer>>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_render = stop.clone();

    let render_thread = std::thread::Builder::new()
        .name("noiria-audio-render".into())
        .spawn(move || run_render_thread(open, init_tx, mixer_rx, stop_render, cfg))
        .expect("オーディオ描画スレッドの起動に失敗");

    let Ok((channels, sample_rate, has_device)) = init_rx.recv() else {
        // 描画スレッドが開始前に落ちた（panic 等）。コマンドは読み捨てる。
        log::error!("オーディオ描画スレッドが起動できません（音なしで続行）");
        let _ = render_thread.join();
        drain_commands(rx, &shared);
        return;
    };
    if has_device {
        log::info!("音声デバイス open 成功: {channels}ch {sample_rate}Hz");
    }
    let mixer = Arc::new(Mutex::new(Mixer::new(channels, sample_rate)));
    if mixer_tx.send(mixer.clone()).is_err() {
        // 描画スレッドが既に終了している。
        let _ = render_thread.join();
        drain_commands(rx, &shared);
        return;
    }
    let mut processor =
        CommandProcessor::new(mixer, shared, channels, sample_rate, game_dir, decode);
    command_loop(&rx, &mut processor);
    stop.store(true, Ordering::Relaxed);
    let _ = render_thread.join();
}

type FillFn = Box<dyn FnMut(&mut [f32]) + Send>;

/// 出力が `fill` を呼べる形にミキサを繋ぐ。デバイスのフォーマットがミキサと同じなら
/// そのまま `mix_into`、違う（再接続で既定デバイスが変わった）ときは
/// チャンネル変換 + 線形リサンプルを挟む（ミキサ側のフォーマットは起動時に固定のため）。
fn make_fill(
    mixer: &Arc<Mutex<Mixer>>,
    mixer_format: (u16, u32),
    device_format: (u16, u32),
) -> FillFn {
    if mixer_format == device_format {
        let mixer = mixer.clone();
        return Box::new(move |buf| mix_locked(&mixer, buf));
    }
    let pull = MixerPull::new(mixer.clone(), mixer_format);
    let mut adapted = ChannelConverter::new(Resampler::new(pull, device_format.1), device_format.0);
    Box::new(move |buf| {
        let n = adapted.read(buf);
        buf[n..].fill(0.0);
    })
}

fn mix_locked(mixer: &Mutex<Mixer>, buf: &mut [f32]) {
    if let Ok(mut m) = mixer.lock() {
        m.mix_into(buf);
    } else {
        buf.fill(0.0);
    }
}

/// `MixerPull` が一度にミキサから引くフレーム数。
const PULL_CHUNK_FRAMES: usize = 256;

/// ミキサを「尽きない `PcmSource`」として見せる（フォーマット変換の入口）。
struct MixerPull {
    mixer: Arc<Mutex<Mixer>>,
    channels: u16,
    rate: u32,
    buf: Vec<f32>,
    pos: usize,
}

impl MixerPull {
    fn new(mixer: Arc<Mutex<Mixer>>, (channels, rate): (u16, u32)) -> Self {
        let len = PULL_CHUNK_FRAMES * channels as usize;
        Self {
            mixer,
            channels,
            rate,
            buf: vec![0.0; len],
            pos: len,
        }
    }
}

impl PcmSource for MixerPull {
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn next_sample(&mut self) -> Option<f32> {
        let mut one = [0.0f32];
        (self.read(&mut one) == 1).then_some(one[0])
    }
    fn read(&mut self, out: &mut [f32]) -> usize {
        let mut n = 0;
        while n < out.len() {
            if self.pos >= self.buf.len() {
                mix_locked(&self.mixer, &mut self.buf);
                self.pos = 0;
            }
            let take = (self.buf.len() - self.pos).min(out.len() - n);
            out[n..n + take].copy_from_slice(&self.buf[self.pos..self.pos + take]);
            self.pos += take;
            n += take;
        }
        n
    }
}

/// null シンクが一度に消費する最大フレーム数 / 時刻確認の間隔。
const NULL_SINK_MAX_FRAMES: usize = 1024;
const NULL_SINK_POLL: Duration = Duration::from_millis(5);

/// 出力デバイスが無い間、ミキサを**実時間相当の速さ**で空読みし続ける。
/// 再生中の音が自然終端し `finished` が立つ、フェードが進む、といったミキサの
/// 時間進行を保つため（誰も消費しないとスクリプトの音声待ちが永久に止まる）。
fn run_null_sink(
    mixer: &Mutex<Mixer>,
    (channels, rate): (u16, u32),
    duration: Duration,
    stop: &AtomicBool,
) {
    let mut buf = vec![0.0f32; NULL_SINK_MAX_FRAMES * channels as usize];
    let start = Instant::now();
    let mut produced: u64 = 0;
    while !stop.load(Ordering::Relaxed) {
        let elapsed = start.elapsed();
        if elapsed >= duration {
            break;
        }
        let due = (elapsed.as_secs_f64() * rate as f64) as u64;
        let mut need = due.saturating_sub(produced);
        produced += need;
        while need > 0 {
            let n = need.min(NULL_SINK_MAX_FRAMES as u64) as usize;
            mix_locked(mixer, &mut buf[..n * channels as usize]);
            need -= n as u64;
        }
        std::thread::sleep(NULL_SINK_POLL);
    }
}

/// 出力デバイスの open + 再生ループ。出力が失敗・消失したら null シンクでミキサを
/// 消費しつつ、バックオフ付きで既定デバイスを開き直す（`stop` まで終わらない）。
/// 最初に開けるかどうかにかかわらず、先頭で `(channels, rate, デバイスあり)` を
/// `init_tx` へ送る（開けなければ `FALLBACK_FORMAT`）。
fn run_render_thread(
    mut open: OpenDevice,
    init_tx: crossbeam_channel::Sender<(u16, u32, bool)>,
    mixer_rx: Receiver<Arc<Mutex<Mixer>>>,
    stop: Arc<AtomicBool>,
    cfg: RenderConfig,
) {
    let mut device = match open() {
        Ok(d) => Some(d),
        Err(e) => {
            log::warn!("オーディオデバイスを開けません（音なしで続行・再試行します）: {e:#}");
            None
        }
    };
    let format = device
        .as_ref()
        .map_or(FALLBACK_FORMAT, |d| (d.channels(), d.sample_rate()));
    if init_tx.send((format.0, format.1, device.is_some())).is_err() {
        return;
    }
    let Ok(mixer) = mixer_rx.recv() else { return };

    let mut backoff = cfg.initial_backoff;
    while !stop.load(Ordering::Relaxed) {
        if let Some(dev) = device.take() {
            let device_format = (dev.channels(), dev.sample_rate());
            let mut fill = make_fill(&mixer, format, device_format);
            let started = Instant::now();
            let result = dev.run(&stop, &mut *fill);
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match result {
                Ok(()) => log::warn!("オーディオ再生ループが予期せず終了しました（開き直します）"),
                Err(e) => log::error!("オーディオ再生ループが停止しました（開き直します）: {e:#}"),
            }
            backoff = if started.elapsed() >= cfg.healthy_run {
                cfg.initial_backoff
            } else {
                (backoff * 2).min(cfg.max_backoff)
            };
        }
        run_null_sink(&mixer, format, backoff, &stop);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match open() {
            Ok(d) => {
                log::info!(
                    "音声デバイスを開き直しました: {}ch {}Hz",
                    d.channels(),
                    d.sample_rate()
                );
                device = Some(d);
            }
            Err(e) => {
                log::debug!("音声デバイスの再オープンに失敗: {e:#}");
                backoff = (backoff * 2).min(cfg.max_backoff);
            }
        }
    }
}

fn drain_commands(rx: Receiver<AudioCmd>, shared: &AudioShared) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            AudioCmd::Quit => break,
            // 読み捨てても確認応答は返す（再生終了待ちが `is_play_pending` で永久に待たないように）。
            AudioCmd::Play {
                ch, request_id, ..
            } => shared.acknowledge_play_requested(ch, request_id, 0),
            AudioCmd::Stop { ch, request_id, .. } => {
                shared.acknowledge_stop_requested(ch, request_id, false);
            }
            _ => {}
        }
    }
}

/// デコード（+ 開始位置までの読み捨て）の完了通知。
struct DecodeDone {
    ch: usize,
    request_id: u64,
    result: Result<(PreparedSource, bool)>,
}

/// デコード待ちの再生要求。同じチャンネルへの後続コマンドは、`Play` と同じ順序で
/// 効くようここに溜め、差し込み直後に同じミキサロック内で適用する。
struct PendingPlay {
    request_id: u64,
    rel: String,
    volume: f32,
    fade_in_ms: u32,
    start_paused: bool,
    deferred: Vec<AudioCmd>,
}

/// `AudioCmd` を `Mixer` へ反映する。`Play` のデコードは別スレッドで行い、完了後に
/// 差し込む（コマンドスレッドをデコード時間ぶん止めない）。要求 ID で
/// 「デコード中に Stop / 別の Play が来たら結果を捨てる」。
struct CommandProcessor {
    mixer: Arc<Mutex<Mixer>>,
    shared: Arc<AudioShared>,
    out_channels: u16,
    out_rate: u32,
    game_dir: GameDir,
    decode: DecodeFn,
    pending: [Option<PendingPlay>; NUM_SLOTS],
    done_tx: Sender<DecodeDone>,
    done_rx: Receiver<DecodeDone>,
}

impl CommandProcessor {
    fn new(
        mixer: Arc<Mutex<Mixer>>,
        shared: Arc<AudioShared>,
        out_channels: u16,
        out_rate: u32,
        game_dir: GameDir,
        decode: DecodeFn,
    ) -> Self {
        let (done_tx, done_rx) = crossbeam_channel::unbounded();
        Self {
            mixer,
            shared,
            out_channels,
            out_rate,
            game_dir,
            decode,
            pending: std::array::from_fn(|_| None),
            done_tx,
            done_rx,
        }
    }

    fn handle(&mut self, cmd: AudioCmd) {
        match cmd {
            AudioCmd::Play {
                ch,
                rel,
                looping,
                volume,
                fade_in_ms,
                start_position_ms,
                start_paused,
                request_id,
            } => {
                // 同じチャンネルの未完了の Play は、結果が届いても捨てる（上書き）。
                self.pending[ch] = Some(PendingPlay {
                    request_id,
                    rel: rel.clone(),
                    volume,
                    fade_in_ms,
                    start_paused,
                    deferred: Vec::new(),
                });
                self.spawn_decode(ch, request_id, rel, looping, start_position_ms);
            }
            AudioCmd::Stop {
                ch,
                fade_out_ms,
                request_id,
            } => {
                self.pending[ch] = None;
                if let Ok(mut m) = self.mixer.lock() {
                    m.stop(ch, fade_out_ms);
                    self.shared
                        .acknowledge_stop_requested(ch, request_id, m.is_playing(ch));
                }
            }
            AudioCmd::SetVolume { ch, .. }
            | AudioCmd::Pause { ch }
            | AudioCmd::Resume { ch }
            | AudioCmd::FadeTo { ch, .. } => {
                if let Some(pending) = &mut self.pending[ch] {
                    pending.deferred.push(cmd);
                } else if let Ok(mut m) = self.mixer.lock() {
                    apply_channel_cmd(&mut m, cmd);
                }
            }
            AudioCmd::Quit => unreachable!(),
        }
    }

    fn spawn_decode(
        &mut self,
        ch: usize,
        request_id: u64,
        rel: String,
        looping: bool,
        start_position_ms: u64,
    ) {
        let decode = self.decode.clone();
        let game_dir = self.game_dir.clone();
        let (out_channels, out_rate) = (self.out_channels, self.out_rate);
        let done_tx = self.done_tx.clone();
        let spawned = std::thread::Builder::new()
            .name("noiria-audio-decode".into())
            .spawn(move || {
                let result = decode(&game_dir, &rel, looping, out_channels, out_rate).map(
                    |(source, effective_looping)| {
                        // 開始位置への読み捨てもここ（ミキサのロックの外）で済ませる。
                        let prepared =
                            Mixer::prepare_start(source, start_position_ms, out_channels, out_rate);
                        (prepared, effective_looping)
                    },
                );
                let _ = done_tx.send(DecodeDone {
                    ch,
                    request_id,
                    result,
                });
            });
        if let Err(e) = spawned {
            self.finish_decode(DecodeDone {
                ch,
                request_id,
                result: Err(anyhow::anyhow!("デコードスレッドを起動できません: {e}")),
            });
        }
    }

    /// デコード結果をミキサへ差し込む。要求がもう最新でなければ（Stop / 別の Play で
    /// 上書き済み）何もせず捨てる。
    fn finish_decode(&mut self, done: DecodeDone) {
        let ch = done.ch;
        let is_current = self.pending[ch]
            .as_ref()
            .is_some_and(|p| p.request_id == done.request_id);
        if !is_current {
            return;
        }
        let Some(pending) = self.pending[ch].take() else {
            return;
        };
        let Ok(mut m) = self.mixer.lock() else {
            self.shared
                .acknowledge_play_requested(ch, pending.request_id, 0);
            return;
        };
        match done.result {
            Ok((prepared, effective_looping)) => {
                m.play_prepared(
                    ch,
                    prepared,
                    effective_looping,
                    pending.volume,
                    pending.fade_in_ms,
                );
                if pending.start_paused {
                    m.pause(ch);
                }
                // 再生中フラグを要求確認より先に立てる（どちらも偽の瞬間を作らない。次の定期同期まで
                // 待つと、再生終了待ちがその隙間で「終わった」と誤判定する）。
                sync_shared(&self.shared, &m);
                self.shared
                    .acknowledge_play_requested(ch, pending.request_id, m.position_ms(ch));
            }
            Err(e) => {
                m.stop(ch, 0);
                self.shared
                    .acknowledge_play_requested(ch, pending.request_id, 0);
                log::error!("音声を再生できません {}: {e:#}", pending.rel);
            }
        }
        for cmd in pending.deferred {
            apply_channel_cmd(&mut m, cmd);
        }
    }

    fn sync(&self) {
        if let Ok(m) = self.mixer.lock() {
            sync_shared(&self.shared, &m);
        }
    }

    /// デコード待ちの要求が無くなるまで待つ（テスト用）。
    #[cfg(test)]
    fn settle(&mut self) {
        while self.pending.iter().any(Option::is_some) {
            let done = self
                .done_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("デコード完了が来ない");
            self.finish_decode(done);
        }
    }
}

/// チャンネル単位の軽いコマンド（`Play`/`Stop` 以外）をミキサへ適用する。
fn apply_channel_cmd(m: &mut Mixer, cmd: AudioCmd) {
    match cmd {
        AudioCmd::SetVolume { ch, volume } => m.set_volume(ch, volume),
        AudioCmd::Pause { ch } => m.pause(ch),
        AudioCmd::Resume { ch } => m.resume(ch),
        AudioCmd::FadeTo {
            ch,
            target_vol,
            ms,
            then,
        } => m.fade_to(ch, target_vol, ms, then),
        AudioCmd::Play { .. } | AudioCmd::Stop { .. } | AudioCmd::Quit => {
            unreachable!("チャンネル単位の軽いコマンドだけが来る")
        }
    }
}

fn command_loop(rx: &Receiver<AudioCmd>, processor: &mut CommandProcessor) {
    let done_rx = processor.done_rx.clone();
    loop {
        select! {
            recv(rx) -> msg => match msg {
                Ok(AudioCmd::Quit) | Err(_) => break,
                Ok(cmd) => processor.handle(cmd),
            },
            recv(done_rx) -> done => {
                if let Ok(done) = done {
                    processor.finish_decode(done);
                }
            },
            default(TICK) => {}
        }
        processor.sync();
    }
}

fn sync_shared(shared: &AudioShared, mixer: &Mixer) {
    for ch in 0..NUM_SLOTS {
        shared.set_playing(ch, mixer.is_playing(ch));
        shared.set_fading(ch, mixer.is_fading(ch));
        shared.set_finished(ch, mixer.is_finished(ch));
    }
    shared.sync_positions(mixer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::BufferSource;
    use std::path::PathBuf;

    /// PCM16 モノラルの最小 WAV バイト列を組み立てる（テスト専用）。
    fn build_wav_bytes(rate: u32, samples: &[i16]) -> Vec<u8> {
        let mut body = Vec::new();
        for &s in samples {
            body.extend_from_slice(&s.to_le_bytes());
        }
        let block_align: u16 = 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + body.len() as u32).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&1u16.to_le_bytes()); // mono
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
        v.extend_from_slice(&block_align.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(&body);
        v
    }

    /// W5: `build_source` は `GameDir` 経由でしか読まなくなったので、テストも
    /// 生ファイルパスの代わりに一時ディレクトリを `GameDir::for_test` で包んで渡す。
    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("noiria_audio_test_{}_{name}", std::process::id()))
    }

    fn vectors_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors_audio")
    }

    #[test]
    fn build_source_decodes_wav_via_own_decoder() {
        let dir = temp_path("wav-dir");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("clip.wav"),
            build_wav_bytes(44100, &[0, 32767, -32768, 16384]),
        )
        .unwrap();
        let game_dir = GameDir::for_test(dir.clone());

        let (mut source, mixer_should_loop) =
            build_source(&game_dir, "clip.wav", true, 2, 44100).unwrap();
        // WAV は Mixer 側ループ（BufferSource::rewind による）。
        assert!(mixer_should_loop);
        assert_eq!(
            source.channels(),
            2,
            "ChannelConverter で出力チャンネル数に変換される"
        );
        assert_eq!(source.sample_rate(), 44100);
        // 何かしらサンプルが取り出せる（無音ではなくデコードが実際に走っている）こと。
        let mut got_nonzero = false;
        for _ in 0..64 {
            match source.next_sample() {
                Some(s) if s.abs() > 1e-6 => got_nonzero = true,
                Some(_) => {}
                None => break,
            }
        }
        assert!(got_nonzero, "WAV デコード結果が全て無音になっている");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_source_decodes_ogg_via_own_vorbis_decoder() {
        let dir = vectors_dir();
        if !dir.join("mono1_a_boy2.ogg").exists() {
            eprintln!(
                "skip: テストベクタが見つかりません: {}",
                dir.join("mono1_a_boy2.ogg").display()
            );
            return;
        }
        let game_dir = GameDir::for_test(dir);
        let (mut source, mixer_should_loop) =
            build_source(&game_dir, "mono1_a_boy2.ogg", true, 2, 48000).unwrap();
        // OGG も WAV 同様、全デコード後の BufferSource なので Mixer 側ループ。
        assert!(mixer_should_loop);
        assert_eq!(source.channels(), 2);
        assert_eq!(source.sample_rate(), 48000);
        assert!(
            source.next_sample().is_some(),
            "デコード結果からサンプルが取り出せない"
        );
    }

    #[test]
    fn build_source_rejects_mp3_with_clear_message_and_no_panic() {
        let game_dir = GameDir::for_test(std::env::temp_dir());
        let Err(err) = build_source(&game_dir, "dummy.mp3", false, 2, 44100) else {
            panic!("MP3 は非対応のはずが成功してしまった");
        };
        let msg = format!("{err:#}");
        assert!(
            msg.contains("MP3/FLAC は非対応になりました")
                && msg.contains("WAV")
                && msg.contains("OGG"),
            "エラーメッセージが期待と異なる: {msg}"
        );
    }

    #[test]
    fn build_source_rejects_flac_with_clear_message_and_no_panic() {
        let game_dir = GameDir::for_test(std::env::temp_dir());
        let Err(err) = build_source(&game_dir, "dummy.flac", false, 2, 44100) else {
            panic!("FLAC は非対応のはずが成功してしまった");
        };
        let msg = format!("{err:#}");
        assert!(
            msg.contains("MP3/FLAC は非対応になりました"),
            "エラーメッセージが期待と異なる: {msg}"
        );
    }

    #[test]
    fn build_source_rejects_unknown_extension_without_panic() {
        let game_dir = GameDir::for_test(std::env::temp_dir());
        assert!(build_source(&game_dir, "dummy.xyz", false, 2, 44100).is_err());
        assert!(build_source(&game_dir, "dummy", false, 2, 44100).is_err());
    }

    #[test]
    fn channel_index_resolves_dynamic_se_buffers_and_aliases() {
        assert_eq!(channel_index("bgm"), Some(CH_BGM));
        assert_eq!(channel_index("bgm2"), Some(CH_BGM2));
        assert_eq!(channel_index("voice"), Some(CH_VOICE));
        assert_eq!(channel_index("se"), Some(CH_SE), "\"se\" は後方互換の別名");
        assert_eq!(
            channel_index("se0"),
            Some(CH_SE),
            "\"se0\" は \"se\" と同一チャンネル"
        );
        assert_eq!(channel_index("se1"), Some(CH_BGM2 + 1));
        assert_eq!(channel_index("se15"), Some(CH_BGM2 + 15));
        assert_eq!(channel_index("se16"), None, "SE バッファ上限超えは拒否");
        assert_eq!(channel_index("se-1"), None);
        assert_eq!(channel_index("sex"), None);
        assert_eq!(channel_index("unknown"), None);
        // 解決される index は全てミキサのスロット数に収まる
        for name in ["bgm", "bgm2", "voice", "se", "se0", "se1", "se15"] {
            assert!(channel_index(name).unwrap() < NUM_SLOTS, "{name} が範囲外");
        }
    }

    #[test]
    fn sync_shared_copies_position_and_clears_it_after_stop() {
        let shared = AudioShared::default();
        let mut mixer = Mixer::new(1, 1000);
        mixer.play_from_ms(
            CH_BGM,
            Box::new(BufferSource::new(1, 1000, vec![0.25; 20])),
            false,
            1.0,
            0,
            4,
        );
        let mut out = vec![0.0; 3];
        mixer.mix_into(&mut out);
        sync_shared(&shared, &mixer);
        assert!(shared.is_playing(CH_BGM));
        assert_eq!(shared.position_ms(CH_BGM), 7);

        mixer.pause(CH_BGM);
        mixer.mix_into(&mut out);
        sync_shared(&shared, &mixer);
        assert_eq!(shared.position_ms(CH_BGM), 7);

        mixer.stop(CH_BGM, 0);
        sync_shared(&shared, &mixer);
        assert!(!shared.is_playing(CH_BGM));
        assert_eq!(shared.position_ms(CH_BGM), 0);
    }

    #[test]
    fn pending_play_and_stop_requests_are_not_overwritten_by_old_mixer_state() {
        let shared = AudioShared::default();
        let mut mixer = Mixer::new(1, 1000);
        mixer.play_from_ms(
            CH_BGM,
            Box::new(BufferSource::new(1, 1000, vec![0.25; 8000])),
            false,
            1.0,
            0,
            4,
        );
        let mut out = vec![0.0; 3];
        mixer.mix_into(&mut out);
        sync_shared(&shared, &mixer);
        assert_eq!(shared.position_ms(CH_BGM), 7);

        let old_request = shared.mark_play_requested(CH_BGM, 2345);
        assert_eq!(shared.position_ms(CH_BGM), 2345);
        sync_shared(&shared, &mixer);
        assert_eq!(shared.position_ms(CH_BGM), 2345);

        let new_request = shared.mark_play_requested(CH_BGM, 6789);
        shared.acknowledge_play_requested(CH_BGM, old_request, 111);
        sync_shared(&shared, &mixer);
        assert_eq!(shared.position_ms(CH_BGM), 6789);

        mixer.play_from_ms(
            CH_BGM,
            Box::new(BufferSource::new(1, 1000, vec![0.25; 8000])),
            false,
            1.0,
            0,
            6789,
        );
        shared.acknowledge_play_requested(CH_BGM, new_request, mixer.position_ms(CH_BGM));
        sync_shared(&shared, &mixer);
        assert_eq!(shared.position_ms(CH_BGM), 6789);

        let stop_request = shared.mark_stop_requested(CH_BGM);
        assert_eq!(shared.position_ms(CH_BGM), 0);
        sync_shared(&shared, &mixer);
        assert_eq!(shared.position_ms(CH_BGM), 0);

        mixer.stop(CH_BGM, 100);
        shared.acknowledge_stop_requested(CH_BGM, stop_request, mixer.is_playing(CH_BGM));
        mixer.pause(CH_BGM);
        sync_shared(&shared, &mixer);
        assert!(!shared.is_playing(CH_BGM));
        assert_eq!(shared.position_ms(CH_BGM), 0);
        assert!(
            shared.position_requests.lock().unwrap().pending[CH_BGM].is_none(),
            "terminal stop の同期後は pending request を残さない"
        );

        let play_after_stop = shared.mark_play_requested(CH_BGM, 3456);
        shared.acknowledge_stop_requested(CH_BGM, stop_request, false);
        sync_shared(&shared, &mixer);
        assert_eq!(shared.position_ms(CH_BGM), 3456);
        shared.acknowledge_play_requested(CH_BGM, play_after_stop, 3456);
    }

    #[test]
    fn play_command_applies_start_position_and_pause_under_one_mixer_lock() {
        let dir = temp_path("atomic-paused-play-dir");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("clip.wav"),
            build_wav_bytes(1000, &[1000, 2000, 3000, 4000, 5000]),
        )
        .unwrap();
        let game_dir = GameDir::for_test(dir.clone());
        let mixer = Arc::new(Mutex::new(Mixer::new(1, 1000)));
        let shared = Arc::new(AudioShared::default());
        let request_id = shared.mark_play_requested(CH_BGM, 2);
        let mut processor = CommandProcessor::new(
            mixer.clone(),
            shared.clone(),
            1,
            1000,
            game_dir.clone(),
            Arc::new(build_source),
        );

        processor.handle(
            AudioCmd::Play {
                ch: CH_BGM,
                rel: "clip.wav".to_string(),
                looping: true,
                volume: 1.0,
                fade_in_ms: 0,
                start_position_ms: 2,
                start_paused: true,
                request_id,
            },
        );
        processor.settle();

        let mut mixer = mixer.lock().unwrap();
        assert!(mixer.is_playing(CH_BGM));
        assert!(mixer.is_paused(CH_BGM));
        assert_eq!(mixer.position_ms(CH_BGM), 2);
        assert_eq!(shared.position_ms(CH_BGM), 2);
        let mut out = vec![1.0; 2];
        mixer.mix_into(&mut out);
        assert_eq!(out, vec![0.0; 2]);
        assert_eq!(mixer.position_ms(CH_BGM), 2);
        drop(mixer);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- R3: 出力の消失からの復帰 / デコードの非同期化 ----

    use std::sync::atomic::AtomicUsize;

    /// 偽の出力デバイス。`fills_until_err` 回 `fill` を呼んだらデバイス消失を模して Err を返す
    /// （`usize::MAX` なら `stop` まで動き続ける）。実デバイスは開かない。
    struct FakeDevice {
        fmt: (u16, u32),
        fills_until_err: usize,
        frames: Arc<AtomicUsize>,
        samples: Arc<Mutex<Vec<f32>>>,
    }

    impl OutputDevice for FakeDevice {
        fn channels(&self) -> u16 {
            self.fmt.0
        }
        fn sample_rate(&self) -> u32 {
            self.fmt.1
        }
        fn run(
            self: Box<Self>,
            stop: &AtomicBool,
            fill: &mut dyn FnMut(&mut [f32]),
        ) -> Result<()> {
            let mut buf = vec![0.0f32; 64 * self.fmt.0 as usize];
            let mut fills = 0usize;
            while !stop.load(Ordering::Relaxed) {
                fill(&mut buf);
                self.frames.fetch_add(64, Ordering::Relaxed);
                self.samples.lock().unwrap().extend_from_slice(&buf);
                fills += 1;
                if fills >= self.fills_until_err {
                    bail!("デバイスが取り外されました（テスト）");
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        }
    }

    const TEST_CFG: RenderConfig = RenderConfig {
        initial_backoff: Duration::from_millis(5),
        max_backoff: Duration::from_millis(20),
        healthy_run: Duration::from_secs(60),
    };

    struct RenderHarness {
        mixer: Arc<Mutex<Mixer>>,
        stop: Arc<AtomicBool>,
        thread: JoinHandle<()>,
        has_device: bool,
        format: (u16, u32),
    }

    impl RenderHarness {
        fn start(open: OpenDevice) -> Self {
            let (init_tx, init_rx) = crossbeam_channel::bounded(1);
            let (mixer_tx, mixer_rx) = crossbeam_channel::bounded(1);
            let stop = Arc::new(AtomicBool::new(false));
            let stop2 = stop.clone();
            let thread = std::thread::spawn(move || {
                run_render_thread(open, init_tx, mixer_rx, stop2, TEST_CFG)
            });
            let (ch, rate, has_device) = init_rx.recv().unwrap();
            let mixer = Arc::new(Mutex::new(Mixer::new(ch, rate)));
            mixer_tx.send(mixer.clone()).unwrap();
            Self {
                mixer,
                stop,
                thread,
                has_device,
                format: (ch, rate),
            }
        }
        fn wait_until(&self, what: &str, f: impl Fn(&Mixer) -> bool) {
            let t = Instant::now();
            while !f(&self.mixer.lock().unwrap()) {
                assert!(t.elapsed() < Duration::from_secs(5), "タイムアウト: {what}");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        /// 停止してスレッドが生きて終了できたこと（panic していない）を確認する。
        fn finish(self) {
            self.stop.store(true, Ordering::Relaxed);
            self.thread.join().expect("描画スレッドが panic した");
        }
    }

    fn finite_sound(frames: usize, rate: u32, value: f32) -> Box<dyn PcmSource> {
        Box::new(BufferSource::new(1, rate, vec![value; frames]))
    }

    #[test]
    fn render_thread_survives_a_device_that_keeps_failing_and_finite_sounds_finish() {
        let opens = Arc::new(AtomicUsize::new(0));
        let frames = Arc::new(AtomicUsize::new(0));
        let samples = Arc::new(Mutex::new(Vec::new()));
        let (o, f, sm) = (opens.clone(), frames.clone(), samples.clone());
        let h = RenderHarness::start(Box::new(move || {
            o.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(FakeDevice {
                fmt: (1, 8000),
                fills_until_err: 2,
                frames: f.clone(),
                samples: sm.clone(),
            }) as Box<dyn OutputDevice>)
        }));
        assert!(h.has_device);
        // 0.5 秒ぶんの有限音源。デバイスは 2 回 fill しただけで死ぬ。
        h.mixer
            .lock()
            .unwrap()
            .play(CH_SE, finite_sound(4000, 8000, 0.25), false, 1.0, 0);
        h.wait_until("有限音源の再生完了", |m| m.is_finished(CH_SE));
        assert!(opens.load(Ordering::Relaxed) >= 3, "開き直しが行われること");
        assert!(
            frames.load(Ordering::Relaxed) >= 128 * 2,
            "再接続後のデバイスにも音が届く"
        );
        h.finish();
    }

    #[test]
    fn render_thread_without_any_device_consumes_the_mixer_in_real_time() {
        let h = RenderHarness::start(Box::new(|| bail!("デバイスなし（テスト）")));
        assert!(!h.has_device);
        assert_eq!(h.format, FALLBACK_FORMAT);
        let start = Instant::now();
        // 0.3 秒ぶん（フォールバックの 48kHz）。
        h.mixer
            .lock()
            .unwrap()
            .play(
                CH_VOICE,
                Box::new(BufferSource::new(2, 48_000, vec![0.5; 2 * 14_400])),
                false,
                1.0,
                0,
            );
        h.wait_until("デバイスなしでも finished が立つ", |m| {
            m.is_finished(CH_VOICE)
        });
        let took = start.elapsed();
        assert!(
            took >= Duration::from_millis(250),
            "null シンクは実時間で進む（即座に消費しない）: {took:?}"
        );
        h.finish();
    }

    #[test]
    fn render_thread_reopens_after_open_failures_and_adapts_a_different_format() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let frames = Arc::new(AtomicUsize::new(0));
        let samples = Arc::new(Mutex::new(Vec::new()));
        let (a, f, sm) = (attempts.clone(), frames.clone(), samples.clone());
        // 1 回目: 開けない（フォールバック 2ch/48kHz のミキサになる）。2 回目以降:
        // 既定デバイスが 1ch/24kHz で現れ、stop まで動く。
        let h = RenderHarness::start(Box::new(move || {
            if a.fetch_add(1, Ordering::Relaxed) < 1 {
                bail!("まだデバイスなし");
            }
            Ok(Box::new(FakeDevice {
                fmt: (1, 24_000),
                fills_until_err: usize::MAX,
                frames: f.clone(),
                samples: sm.clone(),
            }) as Box<dyn OutputDevice>)
        }));
        assert_eq!(h.format, FALLBACK_FORMAT);
        // 10 秒ぶんの定数 0.5（ステレオ L=R）。
        h.mixer.lock().unwrap().play(
            CH_BGM,
            Box::new(BufferSource::new(2, 48_000, vec![0.5; 2 * 480_000])),
            false,
            1.0,
            0,
        );
        h.wait_until("再接続したデバイスが 0.5 秒ぶん消費", |_| {
            frames.load(Ordering::Relaxed) >= 12_000
        });
        let (device_frames, mixer_ms) = {
            let m = h.mixer.lock().unwrap();
            (frames.load(Ordering::Relaxed), m.position_ms(CH_BGM))
        };
        h.finish();
        let device_ms = device_frames as f64 / 24.0;
        assert!(
            (mixer_ms as f64 - device_ms).abs() < 150.0,
            "ミキサの進み {mixer_ms}ms とデバイスの消費 {device_ms:.0}ms が実時間で一致する"
        );
        let samples = samples.lock().unwrap();
        let tail = &samples[samples.len() / 2..];
        assert!(
            tail.iter().all(|s| (s - 0.5).abs() < 1e-3),
            "ステレオ → モノ変換後も定数 0.5 が保たれる"
        );
    }

    /// デコードが完了するまで待たせられる偽デコーダ（"slow" で始まる名前だけ待つ）。
    fn gated_decoder(release: crossbeam_channel::Receiver<()>) -> DecodeFn {
        Arc::new(move |_gd, rel, looping, _ch, _rate| {
            if rel.starts_with("slow") {
                release.recv_timeout(Duration::from_secs(10)).unwrap();
            }
            let value = if rel.contains("b.") { 0.7 } else { 0.5 };
            Ok((finite_sound(1000, 1000, value), looping))
        })
    }

    fn play_cmd(ch: usize, rel: &str, shared: &AudioShared) -> AudioCmd {
        AudioCmd::Play {
            ch,
            rel: rel.to_string(),
            looping: false,
            volume: 1.0,
            fade_in_ms: 0,
            start_position_ms: 0,
            start_paused: false,
            request_id: shared.mark_play_requested(ch, 0),
        }
    }

    fn processor_with(
        decode: DecodeFn,
    ) -> (CommandProcessor, Arc<Mutex<Mixer>>, Arc<AudioShared>) {
        let mixer = Arc::new(Mutex::new(Mixer::new(1, 1000)));
        let shared = Arc::new(AudioShared::default());
        let p = CommandProcessor::new(
            mixer.clone(),
            shared.clone(),
            1,
            1000,
            GameDir::for_test(std::env::temp_dir()),
            decode,
        );
        (p, mixer, shared)
    }

    fn recv_done(p: &mut CommandProcessor) {
        let done = p.done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        p.finish_decode(done);
    }

    #[test]
    fn slow_bgm_decode_does_not_delay_se_and_stop_discards_its_result() {
        let (release_tx, release_rx) = crossbeam_channel::unbounded();
        let (mut p, mixer, shared) = processor_with(gated_decoder(release_rx));

        p.handle(play_cmd(CH_BGM, "slow_bgm.wav", &shared));
        p.handle(play_cmd(CH_SE, "se.wav", &shared));
        recv_done(&mut p); // 先に届くのは SE（BGM はまだデコード中）
        {
            let m = mixer.lock().unwrap();
            assert!(m.is_playing(CH_SE), "BGM のデコード待ちの間に SE が鳴る");
            assert!(!m.is_playing(CH_BGM));
        }
        assert!(p.pending[CH_BGM].is_some());

        let stop_id = shared.mark_stop_requested(CH_BGM);
        p.handle(AudioCmd::Stop {
            ch: CH_BGM,
            fade_out_ms: 0,
            request_id: stop_id,
        });
        assert!(
            p.pending[CH_BGM].is_none(),
            "Stop でデコード待ちの要求は取り消される"
        );
        release_tx.send(()).unwrap();
        recv_done(&mut p);
        assert!(
            !mixer.lock().unwrap().is_playing(CH_BGM),
            "取り消された結果は捨てる"
        );
        assert_eq!(shared.position_ms(CH_BGM), 0);
    }

    /// 再生終了待ちの判定（`is_playing || is_play_pending`）が、デコード中も差し込み後も途切れない。
    #[test]
    fn wait_end_condition_holds_from_the_play_request_through_decode_to_playback() {
        let busy = |shared: &AudioShared| shared.is_playing(CH_BGM) || shared.is_play_pending(CH_BGM);
        let (release_tx, release_rx) = crossbeam_channel::unbounded();
        let (mut p, _mixer, shared) = processor_with(gated_decoder(release_rx));
        assert!(!busy(&shared), "何も要求していなければ終わっている");

        p.handle(play_cmd(CH_BGM, "slow_bgm.wav", &shared));
        assert!(!shared.is_playing(CH_BGM), "デコード中は is_playing が偽");
        assert!(shared.is_play_pending(CH_BGM) && busy(&shared));

        release_tx.send(()).unwrap();
        recv_done(&mut p);
        assert!(shared.is_playing(CH_BGM), "差し込み直後から is_playing（次の定期同期を待たない）");
        assert!(!shared.is_play_pending(CH_BGM));
        assert!(busy(&shared));
    }

    #[test]
    fn decode_failure_clears_the_pending_play_request() {
        let decode: DecodeFn = Arc::new(|_gd, _rel, _looping, _ch, _rate| anyhow::bail!("壊れた音声"));
        let (mut p, _mixer, shared) = processor_with(decode);
        p.handle(play_cmd(CH_BGM, "broken.wav", &shared));
        assert!(shared.is_play_pending(CH_BGM));
        recv_done(&mut p);
        assert!(!shared.is_play_pending(CH_BGM) && !shared.is_playing(CH_BGM));
    }

    #[test]
    fn stop_and_a_newer_play_replace_the_pending_request() {
        let (release_tx, release_rx) = crossbeam_channel::unbounded();
        let (mut p, _mixer, shared) = processor_with(gated_decoder(release_rx));
        p.handle(play_cmd(CH_BGM, "slow_a.wav", &shared));
        assert!(shared.is_play_pending(CH_BGM));
        let stop_id = shared.mark_stop_requested(CH_BGM);
        assert!(!shared.is_play_pending(CH_BGM), "Stop を要求した時点で Play の保留は外れる");
        p.handle(AudioCmd::Stop {
            ch: CH_BGM,
            fade_out_ms: 0,
            request_id: stop_id,
        });
        release_tx.send(()).unwrap();
        recv_done(&mut p);
        assert!(!shared.is_play_pending(CH_BGM) && !shared.is_playing(CH_BGM));
    }

    #[test]
    fn a_play_request_that_cannot_be_delivered_can_be_cancelled() {
        let shared = AudioShared::default();
        let id = shared.mark_play_requested(CH_BGM, 1234);
        assert!(shared.is_play_pending(CH_BGM));
        shared.cancel_play_request(CH_BGM, id, 1234);
        assert!(!shared.is_play_pending(CH_BGM));
        assert_eq!(shared.position_ms(CH_BGM), 1234);
        // 別の要求が上書きしたあとの古い取り下げは、新しい要求に影響しない。
        let old = shared.mark_play_requested(CH_BGM, 0);
        let _new = shared.mark_play_requested(CH_BGM, 0);
        shared.cancel_play_request(CH_BGM, old, 0);
        assert!(shared.is_play_pending(CH_BGM));
    }

    #[test]
    fn draining_commands_without_a_mixer_acknowledges_requests() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let shared = AudioShared::default();
        tx.send(play_cmd(CH_BGM, "x.wav", &shared)).unwrap();
        assert!(shared.is_play_pending(CH_BGM));
        tx.send(AudioCmd::Quit).unwrap();
        drain_commands(rx, &shared);
        assert!(!shared.is_play_pending(CH_BGM));
    }

    #[test]
    fn newer_play_on_the_same_channel_wins_over_a_slow_earlier_decode() {
        let (release_tx, release_rx) = crossbeam_channel::unbounded();
        let (mut p, mixer, shared) = processor_with(gated_decoder(release_rx));

        p.handle(play_cmd(CH_BGM, "slow_a.wav", &shared));
        p.handle(play_cmd(CH_BGM, "b.wav", &shared));
        recv_done(&mut p);
        release_tx.send(()).unwrap();
        recv_done(&mut p); // 遅れて届いた古い結果
        let mut out = [0.0f32; 1];
        mixer.lock().unwrap().mix_into(&mut out);
        assert_eq!(
            out[0], 0.7,
            "後から要求した曲が鳴り、古いデコード結果は無視される"
        );
    }

    #[test]
    fn commands_for_a_channel_with_a_pending_play_apply_right_after_it_installs() {
        let (release_tx, release_rx) = crossbeam_channel::unbounded();
        let (mut p, mixer, shared) = processor_with(gated_decoder(release_rx));

        p.handle(play_cmd(CH_BGM, "slow_a.wav", &shared));
        p.handle(AudioCmd::SetVolume {
            ch: CH_BGM,
            volume: 0.25,
        });
        p.handle(AudioCmd::Pause { ch: CH_BGM });
        p.handle(AudioCmd::Resume { ch: CH_BGM });
        release_tx.send(()).unwrap();
        recv_done(&mut p);
        let mut out = [0.0f32; 1];
        mixer.lock().unwrap().mix_into(&mut out);
        assert_eq!(
            out[0], 0.125,
            "Play の後ろの SetVolume が Play の後に効く（従来の順序）"
        );
        assert!(!mixer.lock().unwrap().is_paused(CH_BGM));
    }

    /// 遅い読み捨て（シーク）を持つ偽ソース。
    struct SlowSeekSource(BufferSource);
    impl PcmSource for SlowSeekSource {
        fn channels(&self) -> u16 {
            self.0.channels()
        }
        fn sample_rate(&self) -> u32 {
            self.0.sample_rate()
        }
        fn next_sample(&mut self) -> Option<f32> {
            self.0.next_sample()
        }
        fn skip_samples(&mut self, samples: u64) -> bool {
            std::thread::sleep(Duration::from_millis(250));
            self.0.skip_samples(samples)
        }
        fn rewind(&mut self) -> bool {
            self.0.rewind()
        }
    }

    /// `during` の実行中に、別スレッドが `mix_into` のためミキサのロックを取るまでの最大待ち時間。
    fn max_render_lock_wait(mixer: &Arc<Mutex<Mixer>>, during: impl FnOnce() + Send) -> Duration {
        let stop = AtomicBool::new(false);
        let mut worst = Duration::ZERO;
        std::thread::scope(|s| {
            let work = s.spawn(|| {
                during();
                stop.store(true, Ordering::Relaxed);
            });
            let mut out = [0.0f32; 16];
            while !stop.load(Ordering::Relaxed) {
                let t = Instant::now();
                let mut m = mixer.lock().unwrap();
                worst = worst.max(t.elapsed());
                m.mix_into(&mut out);
                drop(m);
                std::thread::sleep(Duration::from_millis(1));
            }
            work.join().unwrap();
        });
        worst
    }

    #[test]
    fn seek_for_a_resumed_bgm_happens_outside_the_mixer_lock() {
        let make = || -> Box<dyn PcmSource> {
            Box::new(SlowSeekSource(BufferSource::new(1, 1000, vec![0.5; 5000])))
        };
        // 対照: 旧方式（ロックを握ったまま play_from_ms）では待ち時間が読み捨て時間に張り付く。
        let control_mixer = Arc::new(Mutex::new(Mixer::new(1, 1000)));
        let held = max_render_lock_wait(&control_mixer, || {
            control_mixer
                .lock()
                .unwrap()
                .play_from_ms(CH_BGM, make(), false, 1.0, 0, 100);
        });
        assert!(
            held >= Duration::from_millis(150),
            "対照が検出できていない: {held:?}"
        );

        let decode: DecodeFn = Arc::new(move |_, _, looping, _, _| Ok((make(), looping)));
        let (mut p, mixer, shared) = processor_with(decode);
        let cmd = AudioCmd::Play {
            ch: CH_BGM,
            rel: "bgm.wav".into(),
            looping: false,
            volume: 1.0,
            fade_in_ms: 0,
            start_position_ms: 100,
            start_paused: false,
            request_id: shared.mark_play_requested(CH_BGM, 100),
        };
        let worst = max_render_lock_wait(&mixer, || {
            p.handle(cmd);
            p.settle();
        });
        assert!(
            worst < Duration::from_millis(120),
            "シーク中にレンダー側のロック待ちが発生した: {worst:?}"
        );
        assert!(mixer.lock().unwrap().position_ms(CH_BGM) >= 100);
    }

    // ---- ストリーミングデコード（WAV / OGG Vorbis） ----

    /// 1 回の `read` を `max_chunk` バイト以下に制限するラッパ（途中切れ読み出しの回帰用）。
    struct ShortReadCursor {
        inner: std::io::Cursor<Vec<u8>>,
        max_chunk: usize,
    }

    impl Read for ShortReadCursor {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let cap = buf.len().min(self.max_chunk);
            self.inner.read(&mut buf[..cap])
        }
    }

    impl Seek for ShortReadCursor {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    fn build_wav_custom(
        format_tag: u16,
        channels: u16,
        rate: u32,
        bits: u16,
        body: &[u8],
        fmt_after_data: bool,
    ) -> Vec<u8> {
        let block_align = channels * bits / 8;
        let mut fmt_chunk = Vec::new();
        fmt_chunk.extend_from_slice(b"fmt ");
        fmt_chunk.extend_from_slice(&16u32.to_le_bytes());
        fmt_chunk.extend_from_slice(&format_tag.to_le_bytes());
        fmt_chunk.extend_from_slice(&channels.to_le_bytes());
        fmt_chunk.extend_from_slice(&rate.to_le_bytes());
        fmt_chunk.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
        fmt_chunk.extend_from_slice(&block_align.to_le_bytes());
        fmt_chunk.extend_from_slice(&bits.to_le_bytes());

        // 奇数長チャンク（2 バイト境界パディングの検証用）
        let mut junk_chunk = Vec::new();
        junk_chunk.extend_from_slice(b"JUNK");
        junk_chunk.extend_from_slice(&3u32.to_le_bytes());
        junk_chunk.extend_from_slice(&[1, 2, 3, 0]);

        let mut data_chunk = Vec::new();
        data_chunk.extend_from_slice(b"data");
        data_chunk.extend_from_slice(&(body.len() as u32).to_le_bytes());
        data_chunk.extend_from_slice(body);
        if body.len() % 2 == 1 {
            data_chunk.push(0);
        }

        let payload_len = fmt_chunk.len() + junk_chunk.len() + data_chunk.len();
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(4 + payload_len as u32).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(&junk_chunk);
        if fmt_after_data {
            v.extend_from_slice(&data_chunk);
            v.extend_from_slice(&fmt_chunk);
        } else {
            v.extend_from_slice(&fmt_chunk);
            v.extend_from_slice(&data_chunk);
        }
        v
    }

    fn drain_source(src: &mut dyn PcmSource, chunk_lens: &[usize]) -> Vec<f32> {
        let mut out = Vec::new();
        let mut buf = vec![0.0f32; *chunk_lens.iter().max().unwrap_or(&1)];
        let mut idx = 0;
        loop {
            let want = chunk_lens[idx % chunk_lens.len()];
            idx += 1;
            let got = src.read(&mut buf[..want]);
            out.extend_from_slice(&buf[..got]);
            if got < want {
                break;
            }
        }
        out
    }

    #[test]
    fn streaming_wav_matches_wav_decode_across_all_formats_and_short_reads() {
        // 16 KB の内部バッファを複数回またぐデータ量で全 4 形式を検証する。
        let n_samples = 12_000usize;
        let body8: Vec<u8> = (0..n_samples).map(|i| ((i * 37) % 256) as u8).collect();
        let body16: Vec<u8> = (0..n_samples)
            .flat_map(|i| (((i as i32 * 521) - 32000) as i16).to_le_bytes())
            .collect();
        let body24: Vec<u8> = (0..n_samples)
            .flat_map(|i| {
                let v = ((i as i32 * 131_071) % 8_388_607) - 4_194_304;
                let b = v.to_le_bytes();
                [b[0], b[1], b[2]]
            })
            .collect();
        let bodyf32: Vec<u8> = (0..n_samples)
            .flat_map(|i| ((i as f32 / n_samples as f32) * 2.0 - 1.0).to_le_bytes())
            .collect();

        let cases: [(&str, Vec<u8>, usize); 5] = [
            ("pcm8", build_wav_custom(1, 1, 22050, 8, &body8, false), 13),
            ("pcm16", build_wav_custom(1, 2, 44100, 16, &body16, false), 7),
            ("pcm24_short_read", build_wav_custom(1, 2, 48000, 24, &body24, false), 5),
            ("float32", build_wav_custom(3, 2, 48000, 32, &bodyf32, false), 11),
            ("fmt_after_data", build_wav_custom(1, 1, 44100, 16, &body16[..500], true), 19),
        ];

        for (label, wav_bytes, max_chunk) in cases {
            let expected = crate::codec::wav::decode(&wav_bytes).unwrap();
            let reader = ShortReadCursor {
                inner: std::io::Cursor::new(wav_bytes),
                max_chunk,
            };
            let mut src = StreamingWavSource::new(reader).unwrap();
            assert_eq!(src.channels(), expected.channels, "{label}");
            assert_eq!(src.sample_rate(), expected.sample_rate, "{label}");
            let got = drain_source(&mut src, &[1, 7, 63, 257, 1024]);
            assert_eq!(
                got.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                expected.samples.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                "{label}: ストリーミング WAV 出力が wav::decode とビット一致しない"
            );
        }
    }

    #[test]
    fn streaming_wav_looping_seek_position_ms_and_zero_alloc_playback() {
        // 2 秒 (44100Hz ステレオ 16bit = ~352 KB) で複数バッファ跨ぎのシーク・ループ・無確保を検証。
        let frames = 44_100 * 2;
        let body: Vec<u8> = (0..frames * 2)
            .flat_map(|i| (((i * 257) % 60000 - 30000) as i16).to_le_bytes())
            .collect();
        let wav_bytes = build_wav_custom(1, 2, 44100, 16, &body, false);
        let decoded = crate::codec::wav::decode(&wav_bytes).unwrap();

        // 1. 任意位置 (750ms) からの再生が BufferSource とビット一致し position_ms が正確
        let mk_stream = || {
            let s = StreamingWavSource::new(std::io::Cursor::new(wav_bytes.clone())).unwrap();
            Box::new(ChannelConverter::new(Resampler::new(s, 48000), 2)) as Box<dyn PcmSource>
        };
        let mk_buf = || {
            let s = BufferSource::new(decoded.channels, decoded.sample_rate, decoded.samples.clone());
            Box::new(ChannelConverter::new(Resampler::new(s, 48000), 2)) as Box<dyn PcmSource>
        };

        let mut m_stream = Mixer::new(2, 48000);
        let mut m_buf = Mixer::new(2, 48000);
        m_stream.play_from_ms(CH_BGM, mk_stream(), true, 1.0, 0, 750);
        m_buf.play_from_ms(CH_BGM, mk_buf(), true, 1.0, 0, 750);
        assert_eq!(m_stream.position_ms(CH_BGM), 750);
        assert_eq!(m_stream.position_ms(CH_BGM), m_buf.position_ms(CH_BGM));

        // 末尾をまたいで 2 周目に入るまで (1.5 秒 = 72000 フレーム) 比較
        let mut out_s = vec![0.0f32; 4800 * 2];
        let mut out_b = vec![0.0f32; 4800 * 2];
        for _ in 0..15 {
            m_stream.mix_into(&mut out_s);
            m_buf.mix_into(&mut out_b);
            assert_eq!(
                out_s.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                out_b.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
            );
            assert_eq!(m_stream.position_ms(CH_BGM), m_buf.position_ms(CH_BGM));
        }

        // 2. 終端超過 (u64::MAX) でも先頭へ巻き戻される
        m_stream.play_from_ms(CH_BGM, mk_stream(), false, 1.0, 0, u64::MAX);
        assert_eq!(m_stream.position_ms(CH_BGM), 0);
        m_stream.mix_into(&mut out_s[..200]);
        assert!(out_s[..200].iter().any(|&s| s.abs() > 1e-6));

        // 3. 再生・シーク・巻き戻し中にヒープ確保が 0 バイトであること
        let mut src = StreamingWavSource::new(std::io::Cursor::new(wav_bytes)).unwrap();
        let mut scratch = vec![0.0f32; 512];
        let ((), peak) = crate::test_support::measure_peak(|| {
            assert!(src.skip_samples(50_000));
            for _ in 0..20 {
                let _ = src.read(&mut scratch);
            }
            assert!(src.rewind());
            let _ = src.read(&mut scratch);
        });
        assert_eq!(peak, 0, "StreamingWavSource は再生・シーク・巻き戻し中に確保しない");
    }

    #[test]
    fn streaming_vorbis_matches_vorbis_decode_bit_exactly() {
        let dir = vectors_dir();
        for name in [
            "mono1_a_boy2.ogg",
            "mono2_abashed_en.ogg",
            "stereo1_kalmykia_anthem_trimmed.ogg",
        ] {
            let path = dir.join(name);
            if !path.exists() {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let expected = crate::codec::vorbis::decode(&bytes).unwrap();
            let mut src = StreamingVorbisSource::new(std::io::Cursor::new(bytes)).unwrap();
            assert_eq!(src.channels(), expected.channels, "{name}");
            assert_eq!(src.sample_rate(), expected.sample_rate, "{name}");
            let got = drain_source(&mut src, &[1, 19, 256, 1024, 333]);
            assert_eq!(
                got.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                expected.samples.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                "{name}: StreamingVorbisSource が vorbis::decode とビット一致しない"
            );
        }
    }

    #[test]
    fn streaming_vorbis_uses_constant_memory_much_smaller_than_full_pcm() {
        let path = vectors_dir().join("stereo1_kalmykia_anthem_trimmed.ogg");
        if !path.exists() {
            return;
        }
        // 27 秒ステレオ 48kHz の全デコード済み PCM は約 10.4 MB。
        // ストリーミング再生中のピーク確保はそれより桁違いに小さい（定数メモリ）はず。
        let (total_samples, peak) = crate::test_support::measure_peak(|| {
            let file = std::fs::File::open(&path).unwrap();
            let mut src = StreamingVorbisSource::new(file).unwrap();
            let mut buf = [0.0f32; 1024];
            let mut total = 0usize;
            loop {
                let n = src.read(&mut buf);
                total += n;
                if n < buf.len() {
                    break;
                }
            }
            total
        });
        let full_pcm_bytes = total_samples * std::mem::size_of::<f32>();
        assert!(full_pcm_bytes > 8 * 1024 * 1024, "想定より短いベクタ: {full_pcm_bytes}");
        assert!(
            peak < 2 * 1024 * 1024,
            "ストリーミング再生のピーク確保が大きすぎる: peak={peak} bytes (full_pcm={full_pcm_bytes} bytes)"
        );
    }

    #[test]
    fn streaming_vorbis_looping_and_position_seek_match_buffer_source() {
        let path = vectors_dir().join("mono2_abashed_en.ogg");
        if !path.exists() {
            return;
        }
        let bytes = std::fs::read(&path).unwrap();
        let decoded = crate::codec::vorbis::decode(&bytes).unwrap();

        let mk_stream = || {
            let s = StreamingVorbisSource::new(std::io::Cursor::new(bytes.clone())).unwrap();
            Box::new(ChannelConverter::new(Resampler::new(s, 48000), 2)) as Box<dyn PcmSource>
        };
        let mk_buf = || {
            let s = BufferSource::new(decoded.channels, decoded.sample_rate, decoded.samples.clone());
            Box::new(ChannelConverter::new(Resampler::new(s, 48000), 2)) as Box<dyn PcmSource>
        };

        // 1. 途中位置 (500ms) から開始し、終端 (約 1.67s) をまたいで 2 周目まで BufferSource と一致
        let mut m_stream = Mixer::new(2, 48000);
        let mut m_buf = Mixer::new(2, 48000);
        m_stream.play_from_ms(CH_BGM, mk_stream(), true, 1.0, 0, 500);
        m_buf.play_from_ms(CH_BGM, mk_buf(), true, 1.0, 0, 500);
        assert_eq!(m_stream.position_ms(CH_BGM), 500);

        let mut out_s = vec![0.0f32; 4800 * 2];
        let mut out_b = vec![0.0f32; 4800 * 2];
        for _ in 0..20 {
            m_stream.mix_into(&mut out_s);
            m_buf.mix_into(&mut out_b);
            assert_eq!(
                out_s.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                out_b.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
            );
            assert_eq!(m_stream.position_ms(CH_BGM), m_buf.position_ms(CH_BGM));
        }

        // 2. 終端超過 (60s / u64::MAX) は先頭に巻き戻されて先頭から一致再生される
        for past_end in [60_000u64, u64::MAX] {
            m_stream.play_from_ms(CH_BGM, mk_stream(), false, 1.0, 0, past_end);
            m_buf.play_from_ms(CH_BGM, mk_buf(), false, 1.0, 0, past_end);
            assert_eq!(m_stream.position_ms(CH_BGM), 0);
            m_stream.mix_into(&mut out_s);
            m_buf.mix_into(&mut out_b);
            assert_eq!(
                out_s.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                out_b.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn build_source_streams_ogg_and_wav_from_file_backed_pak() {
        let ogg_path = vectors_dir().join("mono1_a_boy2.ogg");
        if !ogg_path.exists() {
            return;
        }
        let ogg_bytes = std::fs::read(&ogg_path).unwrap();
        let wav_bytes = build_wav_bytes(44100, &[0, 16384, 32767, -32768, -16384, 0]);
        let game_json = br#"{"title":"AudioPak","id":"audio-pak"}"#.to_vec();

        let dir = temp_path("pak-audio");
        std::fs::create_dir_all(&dir).unwrap();
        let pak_path = dir.join("game.pak");
        let key = crate::pak::dev_key();
        let (pak_bytes, _) = crate::pak::build(
            vec![
                ("game.json".to_string(), game_json),
                ("bgm/theme.ogg".to_string(), ogg_bytes.clone()),
                ("se/click.wav".to_string(), wav_bytes.clone()),
            ],
            &key,
        );
        std::fs::write(&pak_path, &pak_bytes).unwrap();

        {
            let (game_dir, _) = GameDir::open(&pak_path).unwrap();
            let (mut ogg_src, ogg_loop) =
                build_source(&game_dir, "bgm/theme.ogg", true, 1, 44100).unwrap();
            assert!(ogg_loop);
            let expected_ogg = crate::codec::vorbis::decode(&ogg_bytes).unwrap();
            let mut expected_ogg_chain = ChannelConverter::new(
                Resampler::new(
                    BufferSource::new(
                        expected_ogg.channels,
                        expected_ogg.sample_rate,
                        expected_ogg.samples.clone(),
                    ),
                    44100,
                ),
                1,
            );
            let got_ogg = drain_source(&mut *ogg_src, &[512]);
            let want_ogg = drain_source(&mut expected_ogg_chain, &[512]);
            assert_eq!(
                got_ogg.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                want_ogg.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
            );
            assert!(ogg_src.rewind());
            let mut head = [0.0f32; 64];
            assert_eq!(ogg_src.read(&mut head), 64);
            assert_eq!(
                head.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                want_ogg[..64]
                    .iter()
                    .map(|x| x.to_bits())
                    .collect::<Vec<_>>()
            );

            let (mut wav_src, wav_loop) =
                build_source(&game_dir, "se/click.wav", false, 1, 44100).unwrap();
            assert!(!wav_loop);
            let expected_wav = crate::codec::wav::decode(&wav_bytes).unwrap();
            let mut expected_wav_chain = ChannelConverter::new(
                Resampler::new(
                    BufferSource::new(
                        expected_wav.channels,
                        expected_wav.sample_rate,
                        expected_wav.samples,
                    ),
                    44100,
                ),
                1,
            );
            let got_wav = drain_source(&mut *wav_src, &[16]);
            let want_wav = drain_source(&mut expected_wav_chain, &[16]);
            assert_eq!(
                got_wav.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                want_wav.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
