//! 自作オーディオミキサ（純 Rust・OS 非依存・完全テスト可能）。
//! WASAPI などの OS 出力層は関知しない。ここで完結する責務:
//! - `PcmSource`: 任意のデコード済み PCM 列を表す最小インターフェイス（インターリーブ）
//! - `Resampler`: 線形補間による任意レート → 任意レートの変換
//! - `ChannelConverter`: mono/stereo/多ch 間の変換
//! - `Mixer`: BGM/BGM2/Voice/SE バッファ群（`NUM_SLOTS` チャンネル）を加算合成し、
//!   音量フェード（任意目標値・完了時遷移付き）・一時停止・ループ・再生終了検知を行う
//!
//! `audio.rs` はこのモジュールに WAV/OGG(Vorbis) 自作デコーダを `PcmSource` として
//! 接続し、`wasapi_out.rs` の出力バッファへ `Mixer::mix_into` で書き込む。

/// デコード済み PCM のソース。サンプルはチャンネルインターリーブ順
/// （L,R,L,R,... のように `channels()` 個ずつ並ぶ）で取り出す。
pub trait PcmSource: Send {
    /// チャンネル数（1 以上）。
    fn channels(&self) -> u16;
    /// サンプルレート（Hz）。
    fn sample_rate(&self) -> u32;
    /// 次のインターリーブ済みサンプルを返す。枯渇したら `None`。
    fn next_sample(&mut self) -> Option<f32>;

    /// `out` へ最大 `out.len()` 個のサンプルを書き、書いた個数を返す。`out.len()`
    /// 未満なら枯渇（書けなかった分は触らない）。リアルタイム経路が 1 サンプルずつ
    /// `dyn` 呼び出しをしないためのブロック読み。既定実装は `next_sample` の反復。
    fn read(&mut self, out: &mut [f32]) -> usize {
        for (i, slot) in out.iter_mut().enumerate() {
            match self.next_sample() {
                Some(s) => *slot = s,
                None => return i,
            }
        }
        out.len()
    }

    /// 先頭側から `samples` 個を読み捨てる。足りずに枯渇したら `false`。
    /// ミキサのロック外でのシーク用。既定実装はブロック読みで捨てる。
    fn skip_samples(&mut self, samples: u64) -> bool {
        skip_by_reading(self, samples)
    }

    /// 先頭からの総フレーム数について、安価に分かる上限を返す。
    fn frame_count_upper_bound(&self) -> Option<u64> {
        None
    }

    /// 先頭に巻き戻してループ再生を続行できるなら `true` を返して巻き戻す。
    /// 巻き戻しをサポートしない（= ストリーミングでシーク不可）ソースは
    /// 既定実装のまま `false` を返せばよい（`Mixer` は非ループ扱いにする）。
    fn rewind(&mut self) -> bool {
        false
    }
}

/// `read` を繰り返して `samples` 個を捨てる（既定の `skip_samples` と、
/// 高速経路を使えない状態からのフォールバック用）。
fn skip_by_reading<S: PcmSource + ?Sized>(source: &mut S, mut samples: u64) -> bool {
    let mut buf = [0.0f32; 512];
    while samples > 0 {
        let want = samples.min(buf.len() as u64) as usize;
        if source.read(&mut buf[..want]) < want {
            return false;
        }
        samples -= want as u64;
    }
    true
}

/// メモリ上に展開済みの PCM バッファ（WAV デコード結果など）を表す `PcmSource`。
/// 巻き戻しをサポートするので `Mixer` のループ機構と組み合わせて使える。
pub struct BufferSource {
    data: Vec<f32>,
    channels: u16,
    rate: u32,
    pos: usize,
}

impl BufferSource {
    pub fn new(channels: u16, rate: u32, data: Vec<f32>) -> Self {
        Self {
            data,
            channels,
            rate,
            pos: 0,
        }
    }
}

impl PcmSource for BufferSource {
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn next_sample(&mut self) -> Option<f32> {
        let s = self.data.get(self.pos).copied()?;
        self.pos += 1;
        Some(s)
    }
    fn read(&mut self, out: &mut [f32]) -> usize {
        let n = out.len().min(self.data.len() - self.pos);
        out[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        n
    }
    fn skip_samples(&mut self, samples: u64) -> bool {
        let remaining = (self.data.len() - self.pos) as u64;
        if samples <= remaining {
            self.pos += samples as usize;
            true
        } else {
            self.pos = self.data.len();
            false
        }
    }
    fn frame_count_upper_bound(&self) -> Option<u64> {
        Some((self.data.len() / self.channels.max(1) as usize) as u64)
    }
    fn rewind(&mut self) -> bool {
        self.pos = 0;
        !self.data.is_empty()
    }
}

/// 線形補間による任意レート → `out_rate` のリサンプラ。`PcmSource` を包んで
/// 同じ口 (`PcmSource`) で出力できる（チェーン可能）。
///
/// 内部バッファは全て `new` で確保した固定長で、読み出し中にヒープ確保しない。
pub struct Resampler<S> {
    inner: S,
    channels: usize,
    out_rate: u32,
    ratio: f64, // in_rate / out_rate
    frame0: Vec<f32>,
    frame1: Vec<f32>,
    have_frames: bool,
    t: f64, // frame0 と frame1 の間の位置 [0, 1)
    ended: bool,
    /// 直近に補間した 1 出力フレーム。`out_pos` 以降が未消費。
    out_frame: Vec<f32>,
    out_pos: usize,
}

impl<S: PcmSource> Resampler<S> {
    pub fn new(inner: S, out_rate: u32) -> Self {
        let channels = inner.channels().max(1) as usize;
        let in_rate = inner.sample_rate().max(1);
        Self {
            inner,
            channels,
            out_rate: out_rate.max(1),
            ratio: in_rate as f64 / out_rate.max(1) as f64,
            frame0: vec![0.0; channels],
            frame1: vec![0.0; channels],
            have_frames: false,
            t: 0.0,
            ended: false,
            out_frame: vec![0.0; channels],
            out_pos: channels,
        }
    }

    fn ensure_frames(&mut self) -> bool {
        if self.have_frames {
            return true;
        }
        if self.inner.read(&mut self.frame0) < self.channels {
            return false;
        }
        if self.inner.read(&mut self.frame1) < self.channels {
            self.frame1.copy_from_slice(&self.frame0);
        }
        self.have_frames = true;
        true
    }

    /// 次の出力 1 フレームを `out_frame` に作る。作れたら `true`。
    fn produce_next_output_frame(&mut self) -> bool {
        if self.ended {
            return false;
        }
        if !self.ensure_frames() {
            self.ended = true;
            return false;
        }
        let t = self.t as f32;
        for ((o, a), b) in self
            .out_frame
            .iter_mut()
            .zip(&self.frame0)
            .zip(&self.frame1)
        {
            *o = a + (b - a) * t;
        }

        self.t += self.ratio;
        while self.t >= 1.0 {
            self.t -= 1.0;
            std::mem::swap(&mut self.frame0, &mut self.frame1);
            if self.inner.read(&mut self.frame1) < self.channels {
                // 入力が尽きた: これ以上の新規フレームはない。今回出力した
                // フレームまでは有効なので返すが、以降は打ち切る。
                self.ended = true;
                break;
            }
        }
        true
    }

    /// 出力を作らずに入力を `adv` フレーム進めた状態（`frame0`/`frame1` が
    /// 入力の `adv` フレーム先）にする。足りなければ `false`。
    fn advance_input(&mut self, adv: u64) -> bool {
        let ch = self.channels;
        match adv {
            0 => true,
            1 => {
                std::mem::swap(&mut self.frame0, &mut self.frame1);
                self.inner.read(&mut self.frame1) == ch
            }
            _ => {
                self.inner.skip_samples((adv - 2).saturating_mul(ch as u64))
                    && self.inner.read(&mut self.frame0) == ch
                    && self.inner.read(&mut self.frame1) == ch
            }
        }
    }
}

impl<S: PcmSource> PcmSource for Resampler<S> {
    fn channels(&self) -> u16 {
        self.channels as u16
    }
    fn sample_rate(&self) -> u32 {
        self.out_rate
    }
    fn next_sample(&mut self) -> Option<f32> {
        if self.out_pos >= self.channels {
            if !self.produce_next_output_frame() {
                return None;
            }
            self.out_pos = 0;
        }
        let s = self.out_frame[self.out_pos];
        self.out_pos += 1;
        Some(s)
    }
    fn read(&mut self, out: &mut [f32]) -> usize {
        let mut n = 0;
        while n < out.len() {
            if self.out_pos >= self.channels {
                if !self.produce_next_output_frame() {
                    break;
                }
                self.out_pos = 0;
            }
            let take = (self.channels - self.out_pos).min(out.len() - n);
            out[n..n + take].copy_from_slice(&self.out_frame[self.out_pos..self.out_pos + take]);
            self.out_pos += take;
            n += take;
        }
        n
    }
    /// フレーム境界での読み捨ては、位相 `t` の更新（f64 の逐次加算なので
    /// 出力 1 フレームずつ同じ演算を繰り返す）だけを行い、入力側は
    /// `inner.skip_samples` で一括して進める。結果の状態は 1 フレームずつ
    /// 読み捨てた場合とビット単位で一致する（テストで固定）。
    fn skip_samples(&mut self, samples: u64) -> bool {
        let ch = self.channels as u64;
        if self.out_pos < self.channels || !samples.is_multiple_of(ch) {
            return skip_by_reading(self, samples);
        }
        let frames = samples / ch;
        if frames == 0 {
            return true;
        }
        if self.ended {
            return false;
        }
        if !self.ensure_frames() {
            self.ended = true;
            return false;
        }
        let mut adv: u64 = 0;
        for _ in 1..frames {
            self.t += self.ratio;
            while self.t >= 1.0 {
                self.t -= 1.0;
                adv += 1;
            }
        }
        if !self.advance_input(adv) {
            self.ended = true;
            return false;
        }
        // 最後の 1 フレームは通常経路（入力枯渇の判定を含む）で作って捨てる。
        let produced = self.produce_next_output_frame();
        self.out_pos = self.channels;
        produced
    }
    fn frame_count_upper_bound(&self) -> Option<u64> {
        let input_frames = self.inner.frame_count_upper_bound()?;
        if input_frames == 0 {
            return Some(0);
        }
        let in_rate = self.inner.sample_rate().max(1) as u128;
        let numerator = input_frames as u128 * self.out_rate as u128;
        let scaled = numerator.div_ceil(in_rate);
        // f64 位相の丸めで入力境界が 1 出力フレーム遅れる場合がある。
        let upper_bound = scaled.saturating_add(1);
        Some(upper_bound.min(u64::MAX as u128) as u64)
    }
    fn rewind(&mut self) -> bool {
        if self.inner.rewind() {
            self.have_frames = false;
            self.t = 0.0;
            self.ended = false;
            self.out_pos = self.channels;
            true
        } else {
            false
        }
    }
}

/// `ChannelConverter` が入力を一度に引く最大フレーム数（スクラッチ長）。
const CONVERT_CHUNK_FRAMES: usize = 256;

/// mono/stereo/多ch 間のチャンネル変換。
/// - mono → stereo: 複製
/// - stereo → mono: 平均
/// - 3ch 以上の入力は先頭 2ch (L/R 相当) にダウンミックスしてから上記と同様に扱う
/// - 出力が 3ch 以上の場合は L/R を先頭 2ch に置き、残りは無音
pub struct ChannelConverter<S> {
    inner: S,
    in_channels: usize,
    out_channels: usize,
    /// 入力をまとめて引くスクラッチ（`new` で確保、以後は再利用）。
    scratch: Vec<f32>,
    /// 直近に変換した 1 出力フレーム。`out_pos` 以降が未消費。
    out_frame: Vec<f32>,
    out_pos: usize,
}

impl<S: PcmSource> ChannelConverter<S> {
    pub fn new(inner: S, out_channels: u16) -> Self {
        let in_channels = inner.channels().max(1) as usize;
        let out_channels = out_channels.max(1) as usize;
        Self {
            inner,
            in_channels,
            out_channels,
            scratch: vec![0.0; CONVERT_CHUNK_FRAMES * in_channels],
            out_frame: vec![0.0; out_channels],
            out_pos: out_channels,
        }
    }

    /// 入力 1 フレーム → 出力 1 フレーム（`out.len()` が出力チャンネル数）。
    fn convert(frame: &[f32], out: &mut [f32]) {
        // まず L/R 相当の 2 値に正規化する。
        let (l, r) = match frame.len() {
            0 => (0.0, 0.0),
            1 => (frame[0], frame[0]),
            _ => (frame[0], frame[1]),
        };
        match out.len() {
            1 => out[0] = (l + r) * 0.5,
            2 => {
                out[0] = l;
                out[1] = r;
            }
            _ => {
                out.fill(0.0);
                out[0] = l;
                out[1] = r;
            }
        }
    }
}

impl<S: PcmSource> PcmSource for ChannelConverter<S> {
    fn channels(&self) -> u16 {
        self.out_channels as u16
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn next_sample(&mut self) -> Option<f32> {
        if self.out_pos >= self.out_channels {
            let ic = self.in_channels;
            if self.inner.read(&mut self.scratch[..ic]) < ic {
                return None;
            }
            Self::convert(&self.scratch[..ic], &mut self.out_frame);
            self.out_pos = 0;
        }
        let s = self.out_frame[self.out_pos];
        self.out_pos += 1;
        Some(s)
    }
    fn read(&mut self, out: &mut [f32]) -> usize {
        let (ic, oc) = (self.in_channels, self.out_channels);
        let mut n = 0;
        while n < out.len() && self.out_pos < oc {
            out[n] = self.out_frame[self.out_pos];
            n += 1;
            self.out_pos += 1;
        }
        while out.len() - n >= oc {
            let want = ((out.len() - n) / oc).min(CONVERT_CHUNK_FRAMES);
            let got = self.inner.read(&mut self.scratch[..want * ic]);
            for f in 0..got / ic {
                Self::convert(&self.scratch[f * ic..(f + 1) * ic], &mut out[n..n + oc]);
                n += oc;
            }
            if got < want * ic {
                return n;
            }
        }
        if n < out.len() {
            if self.inner.read(&mut self.scratch[..ic]) < ic {
                return n;
            }
            Self::convert(&self.scratch[..ic], &mut self.out_frame);
            let take = out.len() - n;
            out[n..].copy_from_slice(&self.out_frame[..take]);
            self.out_pos = take;
            n += take;
        }
        n
    }
    fn skip_samples(&mut self, samples: u64) -> bool {
        let oc = self.out_channels as u64;
        if self.out_pos < self.out_channels || !samples.is_multiple_of(oc) {
            return skip_by_reading(self, samples);
        }
        self.inner
            .skip_samples((samples / oc).saturating_mul(self.in_channels as u64))
    }
    fn frame_count_upper_bound(&self) -> Option<u64> {
        self.inner.frame_count_upper_bound()
    }
    fn rewind(&mut self) -> bool {
        if self.inner.rewind() {
            self.out_pos = self.out_channels;
            true
        } else {
            false
        }
    }
}

fn frames_for_ms(ms: u32, rate: u32) -> u32 {
    ((ms as u64 * rate as u64) / 1000) as u32
}

/// ミキサのスロット（チャンネル）数。チャンネル名 → index の解決は
/// `audio::channel_index` が行う（bgm / bgm2 / voice / se0..se15）。
pub const NUM_SLOTS: usize = 19;

/// フェード完了時にスロットへ適用する遷移。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FadeThen {
    /// 何もしない（音量変更のみ）。
    #[default]
    None,
    /// 再生を停止する（source を破棄。`finished` は立てない）。
    Stop,
    /// 一時停止する（source と再生位置は保持）。
    Pause,
}

struct Slot {
    source: Option<Box<dyn PcmSource>>,
    looping: bool,
    /// 一時停止中はサンプルを進めず出力にも混ぜない（位置は保持）。
    paused: bool,
    /// 非ループ再生が終端まで達して自然終了した。明示 stop では立たない。
    finished: bool,
    volume: f32,
    target: f32,
    step: f32,
    fade_then: FadeThen,
    position_frames: u64,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            source: None,
            looping: false,
            paused: false,
            finished: false,
            volume: 1.0,
            target: 1.0,
            step: 0.0,
            fade_then: FadeThen::None,
            position_frames: 0,
        }
    }
}

/// `Mixer::mix_into` が 1 スロットあたり一度に処理する最大フレーム数。
/// スクラッチ長（`Mixer::new` で確保）を固定し、コールバック中にヒープ確保しない。
const MIX_CHUNK_FRAMES: usize = 256;

/// ミキサに差し込む直前まで（ロックの外で）用意した再生ソース。
/// 開始位置への読み捨て（`Mixer::prepare_start`）は重いので、ミキサの
/// Mutex を握る前にここまで済ませ、ロック内では `Mixer::play_prepared` で
/// 差し込むだけにする。
pub struct PreparedSource {
    source: Option<Box<dyn PcmSource>>,
    position_frames: u64,
}

/// `NUM_SLOTS` チャンネルのミキサ。加算合成し [-1,1] にクランプする。
/// フェードは出力サンプル数（フレーム数）ベースで進める。
pub struct Mixer {
    out_channels: u16,
    out_rate: u32,
    slots: [Slot; NUM_SLOTS],
    scratch: Vec<f32>,
}

impl Mixer {
    pub fn new(out_channels: u16, out_rate: u32) -> Self {
        let out_channels = out_channels.max(1);
        Self {
            out_channels,
            out_rate: out_rate.max(1),
            slots: std::array::from_fn(|_| Slot::default()),
            scratch: vec![0.0; MIX_CHUNK_FRAMES * out_channels as usize],
        }
    }

    pub fn out_channels(&self) -> u16 {
        self.out_channels
    }
    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    /// 新しい音を再生する。`source` は既に出力フォーマット（チャンネル数/レート）に
    /// 変換済みであることを前提とする（`Resampler`/`ChannelConverter` を通す）。
    pub fn play(
        &mut self,
        ch: usize,
        source: Box<dyn PcmSource>,
        looping: bool,
        volume: f32,
        fade_in_ms: u32,
    ) {
        self.play_from_ms(ch, source, looping, volume, fade_in_ms, 0);
    }

    pub fn play_from_ms(
        &mut self,
        ch: usize,
        source: Box<dyn PcmSource>,
        looping: bool,
        volume: f32,
        fade_in_ms: u32,
        start_position_ms: u64,
    ) {
        let prepared =
            Self::prepare_start(source, start_position_ms, self.out_channels, self.out_rate);
        self.play_prepared(ch, prepared, looping, volume, fade_in_ms);
    }

    /// `source` を出力フォーマット `(out_channels, out_rate)` の `start_position_ms`
    /// 位置まで読み進める。ミキサを必要としない（= ミキサのロックを握らずに呼べる）。
    /// 終端を越える指定は先頭へ戻す。巻き戻せないソースは `None`（再生不能）になる。
    pub fn prepare_start(
        mut source: Box<dyn PcmSource>,
        start_position_ms: u64,
        out_channels: u16,
        out_rate: u32,
    ) -> PreparedSource {
        let start_frame =
            ((start_position_ms as u128 * out_rate.max(1) as u128) / 1000).min(u64::MAX as u128) as u64;
        let past_known_end = source
            .frame_count_upper_bound()
            .is_some_and(|upper_bound| start_frame > upper_bound);
        let start_applied = !past_known_end
            && source.skip_samples(start_frame.saturating_mul(out_channels.max(1) as u64));
        let source_available = start_applied || source.rewind();
        PreparedSource {
            source: source_available.then_some(source),
            position_frames: if start_applied { start_frame } else { 0 },
        }
    }

    /// `prepare_start` 済みのソースをスロットへ差し込む（軽量。ロック内で呼ぶ）。
    pub fn play_prepared(
        &mut self,
        ch: usize,
        prepared: PreparedSource,
        looping: bool,
        volume: f32,
        fade_in_ms: u32,
    ) {
        let volume = volume.max(0.0);
        let frames = frames_for_ms(fade_in_ms, self.out_rate);
        let slot = &mut self.slots[ch];
        slot.source = prepared.source;
        slot.looping = looping;
        slot.paused = false;
        slot.finished = false;
        slot.fade_then = FadeThen::None;
        slot.position_frames = prepared.position_frames;
        slot.target = volume;
        if fade_in_ms > 0 && frames > 0 {
            slot.volume = 0.0;
            slot.step = volume / frames as f32;
        } else {
            slot.volume = volume;
            slot.step = 0.0;
        }
    }

    pub fn stop(&mut self, ch: usize, fade_out_ms: u32) {
        if fade_out_ms == 0 || self.slots[ch].paused {
            let slot = &mut self.slots[ch];
            if slot.source.is_none() {
                return;
            }
            slot.source = None;
            slot.paused = false;
            slot.fade_then = FadeThen::None;
            slot.step = 0.0;
            slot.position_frames = 0;
        } else {
            self.fade_to(ch, 0.0, fade_out_ms, FadeThen::Stop);
        }
    }

    /// 現在音量から `target` まで `ms` かけて線形フェードし、完了時に `then` を適用する。
    /// 何も再生していないチャンネルには no-op。`ms == 0` または既に目標音量なら
    /// 即時に音量を確定して `then` を適用する。
    pub fn fade_to(&mut self, ch: usize, target: f32, ms: u32, then: FadeThen) {
        let out_rate = self.out_rate;
        let slot = &mut self.slots[ch];
        if slot.source.is_none() {
            return;
        }
        if slot.paused && then == FadeThen::Stop {
            Self::apply_fade_then(slot, then);
            return;
        }
        let target = target.max(0.0);
        let frames = frames_for_ms(ms, out_rate);
        if frames == 0 || (slot.volume - target).abs() <= f32::EPSILON {
            slot.volume = target;
            slot.target = target;
            slot.step = 0.0;
            Self::apply_fade_then(slot, then);
            return;
        }
        slot.target = target;
        slot.step = (slot.volume - target).abs() / frames as f32;
        slot.fade_then = then;
    }

    pub fn pause(&mut self, ch: usize) {
        let slot = &mut self.slots[ch];
        if slot.source.is_some() {
            if slot.fade_then == FadeThen::Stop {
                Self::apply_fade_then(slot, FadeThen::Stop);
            } else {
                slot.paused = true;
            }
        }
    }

    pub fn resume(&mut self, ch: usize) {
        self.slots[ch].paused = false;
    }

    pub fn set_volume(&mut self, ch: usize, volume: f32) {
        let volume = volume.max(0.0);
        let slot = &mut self.slots[ch];
        slot.volume = volume;
        slot.target = volume;
        slot.step = 0.0;
        slot.fade_then = FadeThen::None;
    }

    pub fn is_playing(&self, ch: usize) -> bool {
        self.slots[ch].source.is_some()
    }

    pub fn is_paused(&self, ch: usize) -> bool {
        self.slots[ch].paused
    }

    pub fn position_ms(&self, ch: usize) -> u64 {
        let slot = &self.slots[ch];
        if slot.source.is_none() {
            return 0;
        }
        slot.position_frames.saturating_mul(1000) / self.out_rate as u64
    }

    /// フェード進行中か（目標音量に達していない間 true）。
    pub fn is_fading(&self, ch: usize) -> bool {
        let slot = &self.slots[ch];
        slot.source.is_some()
            && slot.step != 0.0
            && (slot.volume - slot.target).abs() > f32::EPSILON
    }

    /// 非ループ再生が終端まで達して自然終了したか（明示 stop では立たない）。
    /// 次の `play` でクリアされる。
    pub fn is_finished(&self, ch: usize) -> bool {
        self.slots[ch].finished
    }

    /// `out` (インターリーブ, `out_channels` 単位のフレーム列) に加算合成する。
    /// `out` は事前にゼロクリアされていなくてもよい（内部でクリアする）。
    ///
    /// リアルタイム（オーディオコールバック）から呼ばれるので、ヒープ確保をしない。
    /// 出力は `MIX_CHUNK_FRAMES` フレームずつ、スロット単位でブロック処理する。
    /// 各出力サンプルへの加算順（スロット番号順）と各スロットの状態遷移は
    /// 1 フレームずつ処理した場合と同一で、結果はビット単位で変わらない。
    pub fn mix_into(&mut self, out: &mut [f32]) {
        let ch_count = self.out_channels as usize;
        out.fill(0.0);
        if ch_count == 0 || out.is_empty() {
            return;
        }
        debug_assert_eq!(
            out.len() % ch_count,
            0,
            "out はフレーム境界で揃っている必要がある"
        );
        let whole = out.len() / ch_count * ch_count;
        for chunk in out[..whole].chunks_mut(MIX_CHUNK_FRAMES * ch_count) {
            for slot in &mut self.slots {
                Self::mix_slot(slot, chunk, &mut self.scratch, ch_count);
            }
        }

        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
    }

    /// 1 スロットを `chunk`（`MIX_CHUNK_FRAMES` フレーム以下）へ加算する。
    fn mix_slot(slot: &mut Slot, chunk: &mut [f32], scratch: &mut [f32], ch_count: usize) {
        let frames = chunk.len() / ch_count;
        let mut done = 0;
        let mut just_rewound = false;
        while done < frames {
            if slot.paused {
                return;
            }
            let want = Self::frames_before_fade_then(slot, frames - done);
            let Some(source) = slot.source.as_mut() else {
                return;
            };
            let got = source.read(&mut scratch[..want * ch_count]) / ch_count;
            if got == 0 && just_rewound {
                Self::finish_slot(slot);
                return;
            }
            just_rewound = false;
            for f in 0..got {
                let vol = slot.volume;
                let dst = &mut chunk[(done + f) * ch_count..(done + f + 1) * ch_count];
                for (d, s) in dst.iter_mut().zip(&scratch[f * ch_count..]) {
                    *d += *s * vol;
                }
                slot.position_frames = slot.position_frames.saturating_add(1);
                Self::advance_fade(slot);
            }
            done += got;
            if got == want {
                continue;
            }
            // 読み切った（端数フレームは捨てる）。ループなら巻き戻して続ける。
            if slot.looping && slot.source.as_mut().is_some_and(|s| s.rewind()) {
                slot.position_frames = 0;
                just_rewound = true;
                continue;
            }
            // 非ループ再生の自然終端（明示 stop とは区別して finished を立てる）
            Self::finish_slot(slot);
            return;
        }
    }

    fn finish_slot(slot: &mut Slot) {
        slot.source = None;
        slot.finished = true;
        slot.position_frames = 0;
    }

    /// 次に読むフレーム数の上限。フェード完了時に Pause/Stop へ遷移する場合は、
    /// 遷移後に不要なぶんを source から読み進めないよう、完了フレームまでに絞る。
    fn frames_before_fade_then(slot: &Slot, max: usize) -> usize {
        if slot.fade_then == FadeThen::None
            || slot.step == 0.0
            || (slot.volume - slot.target).abs() <= f32::EPSILON
        {
            return max;
        }
        let (mut v, target, step) = (slot.volume, slot.target, slot.step);
        for i in 0..max {
            if v < target {
                v = (v + step).min(target);
            } else {
                v = (v - step).max(target);
            }
            if (v - target).abs() <= f32::EPSILON {
                return i + 1;
            }
        }
        max
    }

    fn advance_fade(slot: &mut Slot) {
        if slot.step == 0.0 || (slot.volume - slot.target).abs() <= f32::EPSILON {
            return;
        }
        if slot.volume < slot.target {
            slot.volume = (slot.volume + slot.step).min(slot.target);
        } else {
            slot.volume = (slot.volume - slot.step).max(slot.target);
        }
        if (slot.volume - slot.target).abs() <= f32::EPSILON {
            slot.step = 0.0;
            let then = slot.fade_then;
            Self::apply_fade_then(slot, then);
        }
    }

    /// フェード完了時（または即時フェード）の遷移を適用する。
    fn apply_fade_then(slot: &mut Slot, then: FadeThen) {
        slot.fade_then = FadeThen::None;
        match then {
            FadeThen::None => {}
            FadeThen::Stop => {
                slot.source = None;
                slot.paused = false;
                slot.step = 0.0;
                slot.position_frames = 0;
            }
            FadeThen::Pause => {
                slot.paused = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// テスト用の純サイン波ソース（無限長）。
    struct SineSource {
        freq: f32,
        rate: u32,
        channels: u16,
        phase: u64,
    }
    impl SineSource {
        fn new(freq: f32, rate: u32, channels: u16) -> Self {
            Self {
                freq,
                rate,
                channels,
                phase: 0,
            }
        }
    }
    impl PcmSource for SineSource {
        fn channels(&self) -> u16 {
            self.channels
        }
        fn sample_rate(&self) -> u32 {
            self.rate
        }
        fn next_sample(&mut self) -> Option<f32> {
            let frame = self.phase / self.channels as u64;
            let t = frame as f32 / self.rate as f32;
            let v = (2.0 * std::f32::consts::PI * self.freq * t).sin();
            self.phase += 1;
            Some(v)
        }
    }

    struct CountingFiniteSource {
        data: Vec<f32>,
        pos: usize,
        reads: Arc<AtomicUsize>,
    }

    impl PcmSource for CountingFiniteSource {
        fn channels(&self) -> u16 {
            1
        }
        fn sample_rate(&self) -> u32 {
            44100
        }
        fn next_sample(&mut self) -> Option<f32> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let sample = self.data.get(self.pos).copied()?;
            self.pos += 1;
            Some(sample)
        }
        fn frame_count_upper_bound(&self) -> Option<u64> {
            Some(self.data.len() as u64)
        }
        fn rewind(&mut self) -> bool {
            self.pos = 0;
            !self.data.is_empty()
        }
    }

    // ---- Resampler ----

    #[test]
    fn resampler_preserves_frequency_44100_to_48000() {
        // 44100Hz で 441Hz のサイン波を作り、48000Hz にリサンプルする。
        // 出力を「48000Hz 上での 441Hz サイン波」の期待値と比較する。
        let src = SineSource::new(441.0, 44100, 1);
        let mut rs = Resampler::new(src, 48000);
        assert_eq!(rs.sample_rate(), 48000);
        assert_eq!(rs.channels(), 1);

        let n = 2000;
        let mut max_err = 0.0f32;
        for i in 0..n {
            let got = rs.next_sample().unwrap();
            let t = i as f32 / 48000.0;
            let want = (2.0 * std::f32::consts::PI * 441.0 * t).sin();
            max_err = max_err.max((got - want).abs());
        }
        // 線形補間の誤差は低周波数に対しては小さい。
        assert!(max_err < 0.05, "誤差が大きすぎる: {max_err}");
    }

    #[test]
    fn resampler_downsample_preserves_frequency_48000_to_24000() {
        // 48000 -> 24000 (ratio=2)。周期(周波数)が変わらず保たれているかを、
        // 出力レート上での期待サイン波と直接比較して確認する。
        let src = SineSource::new(200.0, 48000, 1);
        let mut rs = Resampler::new(src, 24000);
        assert_eq!(rs.sample_rate(), 24000);

        let n = 1000;
        let mut max_err = 0.0f32;
        for i in 0..n {
            let got = rs.next_sample().unwrap();
            let t = i as f32 / 24000.0;
            let want = (2.0 * std::f32::consts::PI * 200.0 * t).sin();
            max_err = max_err.max((got - want).abs());
        }
        assert!(max_err < 0.05, "誤差が大きすぎる: {max_err}");
    }

    #[test]
    fn resampler_finite_source_ends() {
        let data = vec![0.0f32, 0.5, 1.0, 0.5, 0.0, -0.5];
        let src = BufferSource::new(1, 44100, data);
        let mut rs = Resampler::new(src, 22050); // ratio = 2.0
        let mut got = Vec::new();
        while let Some(s) = rs.next_sample() {
            got.push(s);
            if got.len() > 100 {
                panic!("リサンプラが終端しない");
            }
        }
        assert!(!got.is_empty());
        assert!(got.len() <= 6);
    }

    #[test]
    fn resampler_frame_bound_covers_f64_phase_rounding() {
        let src = BufferSource::new(1, 1, vec![0.5]);
        let mut resampler = Resampler::new(src, 6);
        let upper_bound = resampler.frame_count_upper_bound().unwrap();
        let output_frames = std::iter::from_fn(|| resampler.next_sample()).count() as u64;

        assert_eq!(output_frames, 7);
        assert_eq!(upper_bound, 7);
    }

    // ---- ChannelConverter ----

    #[test]
    fn channel_converter_mono_to_stereo_duplicates() {
        let data = vec![0.1f32, 0.2, 0.3];
        let src = BufferSource::new(1, 44100, data);
        let mut cc = ChannelConverter::new(src, 2);
        assert_eq!(cc.channels(), 2);
        let mut out = Vec::new();
        while let Some(s) = cc.next_sample() {
            out.push(s);
        }
        assert_eq!(out, vec![0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);
    }

    #[test]
    fn channel_converter_stereo_to_mono_averages() {
        let data = vec![1.0f32, -1.0, 0.5, 0.5]; // frame0=(1,-1) frame1=(0.5,0.5)
        let src = BufferSource::new(2, 44100, data);
        let mut cc = ChannelConverter::new(src, 1);
        assert_eq!(cc.channels(), 1);
        let out: Vec<f32> = std::iter::from_fn(|| cc.next_sample()).collect();
        assert_eq!(out.len(), 2);
        assert!((out[0]).abs() < 1e-6);
        assert!((out[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn channel_converter_multichannel_downmixes_to_first_two() {
        // 4ch: L, R, C, LFE -> 先頭 2ch (L,R) だけを使う
        let data = vec![0.2f32, 0.4, 0.6, 0.8];
        let src = BufferSource::new(4, 44100, data);
        let mut cc = ChannelConverter::new(src, 2);
        let out: Vec<f32> = std::iter::from_fn(|| cc.next_sample()).collect();
        assert_eq!(out, vec![0.2, 0.4]);
    }

    #[test]
    fn channel_converter_passthrough_stereo() {
        let data = vec![0.1f32, -0.1, 0.2, -0.2];
        let src = BufferSource::new(2, 44100, data.clone());
        let mut cc = ChannelConverter::new(src, 2);
        let out: Vec<f32> = std::iter::from_fn(|| cc.next_sample()).collect();
        assert_eq!(out, data);
    }

    // ---- Mixer: 加算合成・クランプ ----

    #[test]
    fn mixer_adds_two_sources_and_clamps() {
        let mut mixer = Mixer::new(1, 44100);
        let a = BufferSource::new(1, 44100, vec![0.8, 0.8, 0.8]);
        let b = BufferSource::new(1, 44100, vec![0.8, 0.8, 0.8]);
        mixer.play(0, Box::new(a), false, 1.0, 0);
        mixer.play(1, Box::new(b), false, 1.0, 0);

        let mut out = vec![0.0f32; 3];
        mixer.mix_into(&mut out);
        // 0.8+0.8=1.6 -> clamp to 1.0
        for &s in &out {
            assert!((s - 1.0).abs() < 1e-6, "クランプされていない: {s}");
        }
    }

    #[test]
    fn mixer_silence_when_nothing_playing() {
        let mut mixer = Mixer::new(2, 44100);
        let mut out = vec![1.0f32; 8]; // ゴミを入れておく
        mixer.mix_into(&mut out);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn mixer_reports_playing_state() {
        let mut mixer = Mixer::new(1, 44100);
        assert!(!mixer.is_playing(0));
        let src = BufferSource::new(1, 44100, vec![0.1; 4]);
        mixer.play(0, Box::new(src), false, 1.0, 0);
        assert!(mixer.is_playing(0));

        // データはちょうど 4 サンプル。5 フレーム分要求すると、5 フレーム目の
        // 読み出しで枯渇を検知して停止する（ちょうど 4 フレーム分の要求では
        // 「読み切った」ことがまだ確定しないので is_playing のままでよい）。
        let mut out = vec![0.0f32; 5];
        mixer.mix_into(&mut out);
        assert!(!mixer.is_playing(0));
    }

    #[test]
    fn play_from_ms_skips_output_frames_and_tracks_position() {
        let mut mixer = Mixer::new(2, 1000);
        let src = BufferSource::new(2, 1000, vec![0.1, -0.1, 0.2, -0.2, 0.3, -0.3, 0.4, -0.4]);
        mixer.play_from_ms(0, Box::new(src), false, 1.0, 0, 2);
        assert_eq!(mixer.position_ms(0), 2);

        let mut out = vec![0.0; 2];
        mixer.mix_into(&mut out);
        assert_eq!(out, vec![0.3, -0.3]);
        assert_eq!(mixer.position_ms(0), 3);
    }

    #[test]
    fn position_holds_while_paused_and_resets_on_stop() {
        let mut mixer = Mixer::new(1, 1000);
        let src = BufferSource::new(1, 1000, vec![0.25; 20]);
        mixer.play_from_ms(0, Box::new(src), false, 1.0, 0, 5);

        let mut out = vec![0.0; 2];
        mixer.mix_into(&mut out);
        assert_eq!(mixer.position_ms(0), 7);

        mixer.pause(0);
        mixer.mix_into(&mut out);
        assert_eq!(mixer.position_ms(0), 7);

        mixer.stop(0, 0);
        assert_eq!(mixer.position_ms(0), 0);
    }

    #[test]
    fn looping_rewind_resets_position_to_current_iteration() {
        let mut mixer = Mixer::new(1, 1000);
        let src = BufferSource::new(1, 1000, vec![0.25, 0.5]);
        mixer.play(0, Box::new(src), true, 1.0, 0);

        let mut first_iteration = vec![0.0; 2];
        mixer.mix_into(&mut first_iteration);
        assert_eq!(mixer.position_ms(0), 2);

        let mut next_iteration = vec![0.0; 1];
        mixer.mix_into(&mut next_iteration);
        assert_eq!(next_iteration, vec![0.25]);
        assert_eq!(mixer.position_ms(0), 1);
    }

    #[test]
    fn exact_end_is_resolved_on_next_render_for_looping_and_non_looping_sources() {
        let mut mixer = Mixer::new(1, 1000);
        mixer.play(
            0,
            Box::new(BufferSource::new(1, 1000, vec![0.25, 0.5])),
            false,
            1.0,
            0,
        );
        mixer.play(
            1,
            Box::new(BufferSource::new(1, 1000, vec![0.25, 0.5])),
            true,
            1.0,
            0,
        );

        let mut exact_length = vec![0.0; 2];
        mixer.mix_into(&mut exact_length);
        assert!(mixer.is_playing(0));
        assert!(mixer.is_playing(1));
        assert_eq!(mixer.position_ms(0), 2);
        assert_eq!(mixer.position_ms(1), 2);

        let mut next_render = vec![0.0; 1];
        mixer.mix_into(&mut next_render);
        assert!(!mixer.is_playing(0));
        assert!(mixer.is_finished(0));
        assert_eq!(mixer.position_ms(0), 0);
        assert!(mixer.is_playing(1));
        assert_eq!(mixer.position_ms(1), 1);
    }

    #[test]
    fn play_from_ms_past_end_rewinds_to_start_without_hanging() {
        let mut mixer = Mixer::new(1, 1000);
        let src = BufferSource::new(1, 1000, vec![0.25, 0.5]);
        mixer.play_from_ms(0, Box::new(src), false, 1.0, 0, 10);
        assert_eq!(mixer.position_ms(0), 0);

        let mut out = vec![0.0; 1];
        mixer.mix_into(&mut out);
        assert_eq!(out, vec![0.25]);
        assert_eq!(mixer.position_ms(0), 1);
    }

    #[test]
    fn huge_start_uses_finite_chain_bound_without_scanning_samples() {
        let reads = Arc::new(AtomicUsize::new(0));
        let source = CountingFiniteSource {
            data: vec![0.25; 1000],
            pos: 0,
            reads: reads.clone(),
        };
        let source = Resampler::new(source, 48000);
        let source = ChannelConverter::new(source, 2);
        let mut mixer = Mixer::new(2, 48000);

        mixer.play_from_ms(0, Box::new(source), true, 1.0, 0, u64::MAX);

        assert_eq!(reads.load(Ordering::Relaxed), 0);
        assert_eq!(mixer.position_ms(0), 0);
        let mut out = vec![0.0; 2];
        mixer.mix_into(&mut out);
        assert!(reads.load(Ordering::Relaxed) > 0);
        assert_eq!(mixer.position_ms(0), 0);
    }

    // ---- フェード ----

    #[test]
    fn fade_in_slope_after_n_frames() {
        let mut mixer = Mixer::new(1, 1000); // 1000Hz なので 1frame=1ms で計算しやすい
        let src = BufferSource::new(1, 1000, vec![1.0; 500]);
        // 100ms のフェードイン = 100 フレーム
        mixer.play(0, Box::new(src), false, 1.0, 100);

        let mut out = vec![0.0f32; 50];
        mixer.mix_into(&mut out);
        // 50 フレーム目で約 0.5 の音量のはず（線形）
        let got = out[49];
        assert!(
            (got - 0.5).abs() < 0.02,
            "フェードイン中間値がずれている: {got}"
        );

        let mut out2 = vec![0.0f32; 60];
        mixer.mix_into(&mut out2);
        // 合計 110 フレーム経過 -> フェード完了、以後は 1.0
        let got_end = out2[59];
        assert!(
            (got_end - 1.0).abs() < 1e-4,
            "フェードイン完了後の音量がずれている: {got_end}"
        );
    }

    #[test]
    fn fade_out_completes_and_stops() {
        let mut mixer = Mixer::new(1, 1000);
        let src = BufferSource::new(1, 1000, vec![1.0; 1000]);
        mixer.play(0, Box::new(src), false, 1.0, 0);
        mixer.stop(0, 100); // 100 フレームでフェードアウト
        assert!(mixer.is_playing(0), "フェードアウト中はまだ再生中のはず");

        let mut out = vec![0.0f32; 50];
        mixer.mix_into(&mut out);
        let mid = out[49];
        assert!(
            (mid - 0.5).abs() < 0.02,
            "フェードアウト中間値がずれている: {mid}"
        );
        assert!(mixer.is_playing(0));

        let mut out2 = vec![0.0f32; 60];
        mixer.mix_into(&mut out2);
        // フェードアウト完了で自動停止しているはず
        assert!(
            !mixer.is_playing(0),
            "フェードアウト完了後は停止しているはず"
        );
        assert_eq!(mixer.position_ms(0), 0);
        // 完了直後のサンプルは無音（source が None になった後の出力）
        assert_eq!(out2[59], 0.0);
    }

    #[test]
    fn fade_stop_on_paused_slot_stops_immediately_and_resets_position() {
        let mut mixer = Mixer::new(1, 1000);
        mixer.play_from_ms(
            0,
            Box::new(BufferSource::new(1, 1000, vec![1.0; 1000])),
            false,
            1.0,
            0,
            5,
        );
        let mut out = vec![0.0; 2];
        mixer.mix_into(&mut out);
        mixer.pause(0);
        assert_eq!(mixer.position_ms(0), 7);

        mixer.stop(0, 100);

        assert!(!mixer.is_playing(0));
        assert!(!mixer.is_paused(0));
        assert!(!mixer.is_fading(0));
        assert_eq!(mixer.position_ms(0), 0);
        assert!(!mixer.is_finished(0));
    }

    #[test]
    fn pausing_an_active_stop_fade_completes_the_stop_immediately() {
        let mut mixer = Mixer::new(1, 1000);
        mixer.play_from_ms(
            0,
            Box::new(BufferSource::new(1, 1000, vec![1.0; 1000])),
            false,
            1.0,
            0,
            5,
        );
        mixer.stop(0, 100);
        assert!(mixer.is_fading(0));

        mixer.pause(0);

        assert!(!mixer.is_playing(0));
        assert!(!mixer.is_paused(0));
        assert!(!mixer.is_fading(0));
        assert_eq!(mixer.position_ms(0), 0);
        assert!(!mixer.is_finished(0));
    }

    #[test]
    fn fade_to_stop_on_an_already_paused_slot_stops_immediately() {
        let mut mixer = Mixer::new(1, 1000);
        mixer.play_from_ms(
            0,
            Box::new(BufferSource::new(1, 1000, vec![1.0; 1000])),
            false,
            1.0,
            0,
            5,
        );
        mixer.pause(0);

        mixer.fade_to(0, 0.0, 100, FadeThen::Stop);

        assert!(!mixer.is_playing(0));
        assert!(!mixer.is_paused(0));
        assert!(!mixer.is_fading(0));
        assert_eq!(mixer.position_ms(0), 0);
        assert!(!mixer.is_finished(0));
    }

    #[test]
    fn set_volume_is_immediate_not_faded() {
        let mut mixer = Mixer::new(1, 1000);
        let src = BufferSource::new(1, 1000, vec![1.0; 100]);
        mixer.play(0, Box::new(src), false, 1.0, 0);
        mixer.set_volume(0, 0.25);

        let mut out = vec![0.0f32; 1];
        mixer.mix_into(&mut out);
        assert!((out[0] - 0.25).abs() < 1e-6);
    }

    // ---- ループ再生の継ぎ目連続性 ----

    #[test]
    fn looping_buffer_is_seamless() {
        let data = vec![0.1f32, 0.2, 0.3, 0.4];
        let mut mixer = Mixer::new(1, 44100);
        mixer.play(
            0,
            Box::new(BufferSource::new(1, 44100, data.clone())),
            true,
            1.0,
            0,
        );

        // 4サンプルのバッファを 3 周分 (12サンプル) 読む。継ぎ目でも途切れず
        // (無音を挟まず) データが繰り返されるはず。
        let mut out = vec![0.0f32; 12];
        mixer.mix_into(&mut out);

        let expected: Vec<f32> = data.iter().cycle().take(12).copied().collect();
        for i in 0..12 {
            assert!(
                (out[i] - expected[i]).abs() < 1e-6,
                "ループ継ぎ目 idx={i}: got={} want={}",
                out[i],
                expected[i]
            );
        }
        // ループしているので終わらず再生中のまま
        assert!(mixer.is_playing(0));
    }

    #[test]
    fn non_looping_buffer_stops_at_end_without_repeating() {
        let data = vec![0.1f32, 0.2, 0.3, 0.4];
        let mut mixer = Mixer::new(1, 44100);
        mixer.play(
            0,
            Box::new(BufferSource::new(1, 44100, data.clone())),
            false,
            1.0,
            0,
        );

        let mut out = vec![0.0f32; 8];
        mixer.mix_into(&mut out);
        for i in 0..4 {
            assert!((out[i] - data[i]).abs() < 1e-6);
        }
        for &s in &out[4..8] {
            assert_eq!(s, 0.0, "非ループは終端後は無音のはず");
        }
        assert!(!mixer.is_playing(0));
    }

    // ---- W2: 一時停止・FadeTo・finished・複数 SE バッファ ----

    #[test]
    fn pause_holds_position_and_outputs_silence() {
        let data = vec![0.1f32, 0.2, 0.3, 0.4];
        let mut mixer = Mixer::new(1, 44100);
        mixer.play(
            0,
            Box::new(BufferSource::new(1, 44100, data)),
            false,
            1.0,
            0,
        );
        let mut out = vec![0.0f32; 2];
        mixer.mix_into(&mut out);
        assert_eq!(out, vec![0.1, 0.2]);

        mixer.pause(0);
        assert!(
            mixer.is_playing(0),
            "一時停止中も source は保持され再生中扱い"
        );
        assert!(mixer.is_paused(0));
        let mut out2 = vec![0.0f32; 2];
        mixer.mix_into(&mut out2);
        assert_eq!(
            out2,
            vec![0.0, 0.0],
            "一時停止中は無音（サンプルを進めない）"
        );

        mixer.resume(0);
        let mut out3 = vec![0.0f32; 2];
        mixer.mix_into(&mut out3);
        assert_eq!(out3, vec![0.3, 0.4], "再開後は停止位置の続きから再生される");
    }

    #[test]
    fn fade_to_passes_intermediate_and_reaches_target_volume() {
        let mut mixer = Mixer::new(1, 1000); // 1frame = 1ms
        mixer.play(
            0,
            Box::new(BufferSource::new(1, 1000, vec![1.0; 1000])),
            false,
            1.0,
            0,
        );
        mixer.fade_to(0, 0.4, 100, FadeThen::None);
        assert!(mixer.is_fading(0));

        let mut out = vec![0.0f32; 50];
        mixer.mix_into(&mut out);
        let mid = out[49];
        assert!(
            (mid - 0.7).abs() < 0.02,
            "FadeTo 中間音量がずれている: {mid}"
        );

        let mut out2 = vec![0.0f32; 60];
        mixer.mix_into(&mut out2);
        assert!(
            (out2[59] - 0.4).abs() < 1e-4,
            "FadeTo 完了後の音量: {}",
            out2[59]
        );
        assert!(!mixer.is_fading(0), "フェード完了で fading が解除される");
        assert!(mixer.is_playing(0), "FadeThen::None は再生を継続する");
    }

    #[test]
    fn fade_then_pause_pauses_at_target_without_finished() {
        let mut mixer = Mixer::new(1, 1000);
        mixer.play(
            0,
            Box::new(BufferSource::new(1, 1000, vec![1.0; 1000])),
            false,
            1.0,
            0,
        );
        mixer.fade_to(0, 0.0, 50, FadeThen::Pause);

        let mut out = vec![0.0f32; 60];
        mixer.mix_into(&mut out);
        assert!(
            mixer.is_playing(0),
            "FadeThen::Pause 後も source は保持される"
        );
        assert!(mixer.is_paused(0), "フェード完了で一時停止に遷移する");
        assert!(!mixer.is_finished(0), "一時停止は自然終了ではない");

        // resume + set_volume で続きから再生できる
        mixer.resume(0);
        mixer.set_volume(0, 1.0);
        let mut out2 = vec![0.0f32; 4];
        mixer.mix_into(&mut out2);
        assert!(
            out2.iter().all(|&s| (s - 1.0).abs() < 1e-6),
            "再開後の出力: {out2:?}"
        );
    }

    #[test]
    fn fade_then_stop_stops_without_finished() {
        let mut mixer = Mixer::new(1, 1000);
        mixer.play(
            0,
            Box::new(BufferSource::new(1, 1000, vec![1.0; 1000])),
            false,
            1.0,
            0,
        );
        mixer.fade_to(0, 0.0, 50, FadeThen::Stop);

        let mut out = vec![0.0f32; 60];
        mixer.mix_into(&mut out);
        assert!(
            !mixer.is_playing(0),
            "FadeThen::Stop はフェード完了で停止する"
        );
        assert!(!mixer.is_finished(0), "明示停止では finished は立たない");
        assert_eq!(mixer.position_ms(0), 0);
    }

    #[test]
    fn fade_to_with_zero_ms_applies_then_immediately() {
        let mut mixer = Mixer::new(1, 1000);
        mixer.play(
            0,
            Box::new(BufferSource::new(1, 1000, vec![1.0; 100])),
            false,
            1.0,
            0,
        );
        mixer.fade_to(0, 0.0, 0, FadeThen::Pause);
        assert!(
            mixer.is_paused(0),
            "ms=0 の FadeTo は即時に then を適用する"
        );
        assert!(!mixer.is_fading(0));
    }

    #[test]
    fn multiple_se_buffers_mix_together() {
        // se0 (ch1) と se1 (ch4) の同時再生が加算合成される
        let mut mixer = Mixer::new(1, 44100);
        mixer.play(
            1,
            Box::new(BufferSource::new(1, 44100, vec![0.3; 8])),
            false,
            1.0,
            0,
        );
        mixer.play(
            4,
            Box::new(BufferSource::new(1, 44100, vec![0.4; 8])),
            false,
            1.0,
            0,
        );
        let mut out = vec![0.0f32; 8];
        mixer.mix_into(&mut out);
        for &s in &out {
            assert!((s - 0.7).abs() < 1e-6, "合成値がずれている: {s}");
        }
    }

    #[test]
    fn natural_end_sets_finished_and_play_clears_it() {
        let mut mixer = Mixer::new(1, 44100);
        mixer.play(
            0,
            Box::new(BufferSource::new(1, 44100, vec![0.1; 4])),
            false,
            1.0,
            0,
        );
        assert!(!mixer.is_finished(0));

        let mut out = vec![0.0f32; 8];
        mixer.mix_into(&mut out);
        assert!(!mixer.is_playing(0));
        assert!(mixer.is_finished(0), "非ループ終端で finished が立つ");
        assert_eq!(mixer.position_ms(0), 0);

        mixer.play(
            0,
            Box::new(BufferSource::new(1, 44100, vec![0.1; 4])),
            false,
            1.0,
            0,
        );
        assert!(!mixer.is_finished(0), "再 play で finished がクリアされる");
        mixer.stop(0, 0);
        assert!(!mixer.is_finished(0), "明示 stop では finished は立たない");
    }

    #[test]
    fn xchgbgm_command_sequence_crossfades_two_channels() {
        // xchgbgm 相当: 旧 BGM (ch0=bgm) を 100ms フェードアウトしつつ
        // 新 BGM (ch3=bgm2) を 100ms フェードイン。両フェードが独立に進行する。
        let mut mixer = Mixer::new(1, 1000);
        mixer.play(
            0,
            Box::new(BufferSource::new(1, 1000, vec![1.0; 1000])),
            false,
            1.0,
            0,
        );
        mixer.stop(0, 100);
        mixer.play(
            3,
            Box::new(BufferSource::new(1, 1000, vec![1.0; 1000])),
            false,
            1.0,
            100,
        );

        let mut out = vec![0.0f32; 50];
        mixer.mix_into(&mut out);
        // 線形クロスフェードの合成は常に約 1.0（旧 0.5 + 新 0.5）
        assert!(
            (out[49] - 1.0).abs() < 0.05,
            "クロスフェード中間の合成: {}",
            out[49]
        );
        assert!(
            mixer.is_fading(0) && mixer.is_fading(3),
            "両チャンネルが同時にフェード中"
        );

        let mut out2 = vec![0.0f32; 60];
        mixer.mix_into(&mut out2);
        assert!(!mixer.is_playing(0), "旧 BGM はフェードアウト完了で停止");
        assert!(mixer.is_playing(3), "新 BGM は再生継続");
        assert!(
            (out2[59] - 1.0).abs() < 1e-4,
            "新 BGM がフル音量: {}",
            out2[59]
        );
    }

    // ---- 統合: リサンプル + チャンネル変換をチェーンして Mixer に渡す ----

    #[test]
    fn mixer_accepts_resampled_and_channel_converted_source() {
        let src = BufferSource::new(1, 22050, vec![1.0; 100]); // mono, 22050Hz
        let resampled = Resampler::new(src, 44100); // -> 44100Hz
        let stereo = ChannelConverter::new(resampled, 2); // -> stereo
        assert_eq!(stereo.channels(), 2);
        assert_eq!(stereo.sample_rate(), 44100);

        let mut mixer = Mixer::new(2, 44100);
        mixer.play(0, Box::new(stereo), false, 1.0, 0);
        let mut out = vec![0.0f32; 20];
        mixer.mix_into(&mut out);
        // 定数 1.0 の入力なので、L/R とも 1.0 近辺のはず
        for &s in &out {
            assert!((s - 1.0).abs() < 1e-3, "got={s}");
        }
    }

    // ---- R3: 出力ビット一致の基準シナリオと確保回数・計測 ----

    /// 決定論的な疑似乱数 PCM（LCG、-1..1）。
    fn lcg_pcm(seed: u64, n: usize) -> Vec<f32> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((x >> 40) as f32 / (1u64 << 23) as f32) - 1.0
            })
            .collect()
    }

    fn chain(src: BufferSource, out_rate: u32, out_ch: u16) -> Box<dyn PcmSource> {
        Box::new(ChannelConverter::new(Resampler::new(src, out_rate), out_ch))
    }

    fn fnv_bits(h: &mut u64, v: &[f32]) {
        for s in v {
            for b in s.to_bits().to_le_bytes() {
                *h ^= b as u64;
                *h = h.wrapping_mul(0x100000001b3);
            }
        }
    }

    /// リサンプル・チャンネル変換・ループ・自然終端・フェード(Pause/Stop)・シーク・
    /// 不規則なブロック長を混ぜた基準シナリオの出力ハッシュ。`mix_into` の内部実装
    /// （ブロック化・確保削減）が出力ビットを変えないことの回帰固定。
    fn reference_scenario_hash() -> u64 {
        let mut m = Mixer::new(2, 48000);
        m.play(
            0,
            chain(BufferSource::new(1, 44100, lcg_pcm(1, 20000)), 48000, 2),
            true,
            0.9,
            100,
        );
        m.play(
            1,
            chain(BufferSource::new(2, 22050, lcg_pcm(2, 6000)), 48000, 2),
            false,
            0.7,
            0,
        );
        m.play(
            2,
            chain(BufferSource::new(3, 32000, lcg_pcm(3, 777 * 3)), 48000, 2),
            true,
            1.0,
            0,
        );
        m.set_volume(2, 0.5);
        m.fade_to(2, 0.1, 30, FadeThen::Pause);
        m.play_from_ms(
            3,
            chain(BufferSource::new(1, 48000, lcg_pcm(4, 30000)), 48000, 2),
            false,
            1.0,
            0,
            37,
        );
        m.play(
            4,
            chain(BufferSource::new(2, 96000, lcg_pcm(5, 40000)), 48000, 2),
            false,
            0.4,
            7,
        );
        let blocks = [1usize, 7, 64, 255, 256, 257, 1000, 3, 513, 2000, 31, 4096, 9, 700];
        let mut h = 0xcbf29ce484222325u64;
        for (i, &frames) in blocks.iter().cycle().take(40).enumerate() {
            match i {
                5 => m.stop(3, 20),
                8 => m.resume(2),
                9 => m.fade_to(0, 0.2, 50, FadeThen::None),
                12 => m.pause(1),
                14 => m.resume(1),
                16 => m.fade_to(1, 0.0, 10, FadeThen::Stop),
                20 => m.set_volume(0, 0.8),
                25 => m.fade_to(2, 0.0, 40, FadeThen::Stop),
                _ => {}
            }
            let mut out = vec![9.0f32; frames * 2];
            m.mix_into(&mut out);
            fnv_bits(&mut h, &out);
            for ch in 0..5 {
                h ^= (m.position_ms(ch) << 3) ^ ((m.is_playing(ch) as u64) << 1) ^ m.is_finished(ch) as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
        }
        h
    }

    #[test]
    fn reference_scenario_output_is_bit_identical_to_pre_r3_baseline() {
        assert_eq!(reference_scenario_hash(), 18294807968326885762);
    }

    #[test]
    fn mix_into_makes_no_heap_allocations_with_active_resampled_slots() {
        let mut m = Mixer::new(2, 48000);
        for ch in 0..8 {
            let (ich, rate) = [(1u16, 44100u32), (2, 22050), (3, 32000), (2, 96000)][ch % 4];
            m.play(
                ch,
                chain(
                    BufferSource::new(ich, rate, lcg_pcm(ch as u64, 30000 * ich as usize)),
                    48000,
                    2,
                ),
                ch % 2 == 0,
                0.5,
                10,
            );
        }
        let mut out = vec![0.0f32; 4096 * 2];
        // 確保が 1 回でもあれば、その間の生存バイト数のピークが 0 を超える。
        let ((), peak) = crate::test_support::measure_peak(|| {
            for _ in 0..20 {
                m.mix_into(&mut out);
            }
        });
        assert_eq!(peak, 0, "mix_into 中に確保したバイト数（ピーク）");
    }

    /// 計測用（`cargo test --release -p noiria-core --lib mixer::tests::bench -- --ignored --nocapture`）。
    #[test]
    #[ignore]
    fn bench_mix_into_eight_resampled_slots() {
        let mut m = Mixer::new(2, 48000);
        for ch in 0..8 {
            let (ich, rate) = [(1u16, 44100u32), (2, 22050), (2, 44100), (2, 96000)][ch % 4];
            m.play(
                ch,
                chain(
                    BufferSource::new(ich, rate, lcg_pcm(ch as u64, 400_000 * ich as usize)),
                    48000,
                    2,
                ),
                true,
                0.5,
                0,
            );
        }
        let mut out = vec![0.0f32; 480 * 2];
        let iters = 2000;
        let t = std::time::Instant::now();
        for _ in 0..iters {
            m.mix_into(&mut out);
        }
        let el = t.elapsed();
        eprintln!(
            "BENCH mix_into(480 frames, 8 slots): {:.1} us/call ({:.1} ns/frame)",
            el.as_secs_f64() * 1e6 / iters as f64,
            el.as_secs_f64() * 1e9 / (iters * 480) as f64
        );
    }

    /// 計測用: BGM を 60 秒地点から再開するときに**ミキサ Mutex を握る時間**。
    #[test]
    #[ignore]
    fn bench_bgm_resume_at_60s() {
        let src = chain(
            BufferSource::new(2, 44100, lcg_pcm(9, 44100 * 2 * 300)),
            48000,
            2,
        );
        let mut m = Mixer::new(2, 48000);
        let t = std::time::Instant::now();
        m.play_from_ms(0, src, true, 1.0, 0, 60_000);
        eprintln!("BENCH play_from_ms(60s) total: {:.2} ms", t.elapsed().as_secs_f64() * 1e3);
    }

    #[test]
    fn fade_then_pause_mid_block_does_not_read_ahead_of_the_pause_point() {
        let data: Vec<f32> = (0..100).map(|i| i as f32 / 1000.0).collect();
        let mut m = Mixer::new(1, 1000);
        m.play(0, Box::new(BufferSource::new(1, 1000, data)), false, 1.0, 0);
        m.fade_to(0, 0.0, 5, FadeThen::Pause);
        let mut out = vec![0.0; 20];
        m.mix_into(&mut out);
        assert!(m.is_paused(0));
        assert_eq!(m.position_ms(0), 5, "フェード完了フレームで位置が止まる");
        m.resume(0);
        m.set_volume(0, 1.0);
        let mut next = vec![0.0; 1];
        m.mix_into(&mut next);
        assert_eq!(next, vec![0.005], "一時停止点の続き（先読みで捨てていない）");
    }

    /// 1 フレームずつ（`next_sample`）読み捨てた場合との同値性:
    /// 高速 `skip_samples` の可否と、その後の出力 60 フレームがビット一致する。
    #[test]
    fn chain_skip_samples_matches_stepwise_skip_bit_exactly() {
        let cases: [(u16, u32, u32, usize); 6] = [
            (1, 44100, 48000, 5000),
            (2, 22050, 48000, 3001),
            (3, 96000, 48000, 7000),
            (2, 48000, 48000, 900),
            (1, 8000, 48000, 400),
            (2, 48000, 44100, 1234),
        ];
        for (ich, in_rate, out_rate, in_frames) in cases {
            let data = lcg_pcm(in_rate as u64, in_frames * ich as usize);
            let total_out = (in_frames as u64 * out_rate as u64).div_ceil(in_rate as u64) + 1;
            let mk = || {
                ChannelConverter::new(
                    Resampler::new(BufferSource::new(ich, in_rate, data.clone()), out_rate),
                    2,
                )
            };
            let mut starts: Vec<u64> = vec![0, 1, 2, 3, 17, 100, total_out / 2];
            starts.extend((total_out.saturating_sub(4))..=(total_out + 2));
            for start in starts {
                let mut fast = mk();
                let mut slow = mk();
                let ok_fast = fast.skip_samples(start * 2);
                let ok_slow = (0..start * 2).all(|_| slow.next_sample().is_some());
                assert_eq!(ok_fast, ok_slow, "可否 in={in_rate} out={out_rate} start={start}");
                if !ok_slow {
                    continue;
                }
                let a: Vec<Option<f32>> = (0..60).map(|_| fast.next_sample()).collect();
                let b: Vec<Option<f32>> = (0..60).map(|_| slow.next_sample()).collect();
                let bits = |v: &[Option<f32>]| v.iter().map(|x| x.map(f32::to_bits)).collect::<Vec<_>>();
                assert_eq!(bits(&a), bits(&b), "後続出力 in={in_rate} out={out_rate} start={start}");
            }
        }
    }

    #[test]
    fn block_read_matches_next_sample_for_resampler_and_converter() {
        let data = lcg_pcm(77, 2 * 5000);
        let mk = || {
            ChannelConverter::new(
                Resampler::new(BufferSource::new(2, 44100, data.clone()), 48000),
                1,
            )
        };
        let mut a = mk();
        let slow: Vec<f32> = std::iter::from_fn(|| a.next_sample()).collect();
        let mut b = mk();
        let mut fast = Vec::new();
        let mut buf = [0.0f32; 37];
        let mut step = 1;
        loop {
            let want = (step * 7) % 37 + 1;
            let n = b.read(&mut buf[..want]);
            fast.extend_from_slice(&buf[..n]);
            if n < want {
                break;
            }
            step += 1;
        }
        assert_eq!(
            slow.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
            fast.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn prepare_start_runs_without_a_mixer_and_matches_play_from_ms() {
        let mk = || chain(BufferSource::new(1, 44100, lcg_pcm(5, 44100 * 3)), 48000, 2);
        let mut m1 = Mixer::new(2, 48000);
        m1.play_from_ms(0, mk(), false, 1.0, 0, 1500);
        let mut m2 = Mixer::new(2, 48000);
        let prepared = Mixer::prepare_start(mk(), 1500, 2, 48000);
        m2.play_prepared(0, prepared, false, 1.0, 0);
        assert_eq!(m1.position_ms(0), 1500);
        assert_eq!(m2.position_ms(0), 1500);
        let (mut a, mut b) = (vec![0.0; 600], vec![0.0; 600]);
        m1.mix_into(&mut a);
        m2.mix_into(&mut b);
        assert_eq!(a, b);
    }
}
