//! 動画デコードの共通インターフェイスと AVI (MJPEG) / IVF (VP9) 実装。
//!
//! `VideoSource` は表示フレーム単位の読み出しと厳密シークを提供する
//! ストリーミング型インターフェイス。Media Foundation (`mf_video.rs`) のように
//! ランダムアクセスができないデコーダは rewind + decode-forward で同じ契約を満たす。
//! 新しいコーデックを追加するときは `VideoSource` を実装して
//! `open_video_file()` のスニッフィングに 1 分岐足すだけでよい（再生側は不変）。

use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

use super::jpeg;

#[cfg(windows)]
use super::mf_video::MfVideo;

/// フレーム単位でデコードできる動画ソース。呼び出し側は `next_frame()` を
/// 順番に呼び続け、`Ok(None)` で終端に達したら `rewind()` でループするか
/// 再生を終える。
pub trait VideoSource: Send {
    /// `next_frame()` が返した直近の表示フレーム幅。最初のフレームより前は
    /// コンテナまたはデコーダが通知した初期幅。
    fn width(&self) -> u32;
    /// `next_frame()` が返した直近の表示フレーム高。最初のフレームより前は
    /// コンテナまたはデコーダが通知した初期高。
    fn height(&self) -> u32;
    fn fps(&self) -> f32;
    /// 次のフレームを straight RGBA8 で返す。終端で Ok(None)。
    fn next_frame(&mut self) -> Result<Option<Vec<u8>>>;
    /// 次のフレームを `buf` に直接書き込む（バッファ再利用版）。終端で Ok(false)。
    fn next_frame_into(&mut self, buf: &mut Vec<u8>) -> Result<bool> {
        match self.next_frame()? {
            Some(frame) => {
                *buf = frame;
                Ok(true)
            }
            None => Ok(false),
        }
    }
    /// 先頭へ巻き戻す（ループ再生用）。
    fn rewind(&mut self) -> Result<()>;
    /// 0-based の表示フレーム位置へ移動する。成功後の `next_frame()` が `target` を返す。
    /// `target == 総表示フレーム数` は EOF 位置、超過はエラー。
    fn seek_frame(&mut self, target: usize) -> Result<()> {
        self.rewind()?;
        let mut scratch = Vec::new();
        for _ in 0..target {
            ensure!(
                self.next_frame_into(&mut scratch)?,
                "動画の表示フレーム番号が範囲外です: {target}"
            );
        }
        Ok(())
    }
}

fn read_prefix(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// シーク可能なストリームをスニッフィングして開く（AVI/MJPEG・IVF/VP9 専用）。
pub fn open_video_reader<R: Read + Seek + Send + 'static>(
    mut reader: R,
) -> Result<Box<dyn VideoSource>> {
    let mut head = [0u8; 12];
    let n = read_prefix(&mut reader, &mut head)?;
    if n >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"AVI " {
        reader.seek(SeekFrom::Start(0))?;
        return Ok(Box::new(MjpegAvi::from_reader(reader)?));
    }
    if n >= 12 && &head[0..4] == b"DKIF" {
        if &head[8..12] == b"VP90" {
            reader.seek(SeekFrom::Start(0))?;
            return Ok(Box::new(Vp9Ivf::from_reader(reader)?));
        }
        bail!(
            "VP9 (FourCC=VP90) 以外の IVF は自作デコーダ非対応です: {:?}",
            &head[8..12]
        );
    }
    if n >= 4 && head[0] == 0x00 && head[1] == 0x00 && head[2] == 0x01 {
        bail!("MPEG-1 は未対応です（ロードマップ参照）");
    }
    if n >= 12 && &head[4..8] == b"ftyp" {
        bail!("MP4 (H.264/H.265) は未対応です（ロードマップ参照）");
    }
    bail!("不明な動画形式です（対応: AVI/MJPEG, IVF/VP9）");
}

/// メモリ上の動画コンテナをスニッフィングして開く（AVI/MJPEG・IVF/VP9 専用）。
pub fn open_video(data: Vec<u8>) -> Result<Box<dyn VideoSource>> {
    open_video_reader(Cursor::new(data))
}

/// ファイルパスから動画を開く。先頭バイトでコンテナを判別する:
/// AVI は自作 MJPEG デコーダへ、IVF+VP90 は自作 VP9 デコーダ (`vp9dec`) へ、
/// それ以外 (MP4/H.264, WMV, MPEG-1, IVF の他コーデック等) は
/// Windows では OS 内蔵の Media Foundation へフォールバックする。
/// IVF+VP9 は MF より優先する（自作デコーダが公式コンフォーマンス済みのため）。
pub fn open_video_file(path: &Path) -> Result<Box<dyn VideoSource>> {
    let mut f = std::fs::File::open(path)
        .with_context(|| format!("動画が開けません: {}", path.display()))?;
    let mut head = [0u8; 12];
    let n = read_prefix(&mut f, &mut head)
        .with_context(|| format!("動画が読めません: {}", path.display()))?;
    if n < head.len() {
        // 12 バイト未満のファイルは何であれ不正
        bail!("動画ファイルが小さすぎます: {}", path.display());
    }

    if &head[0..4] == b"RIFF" && &head[8..12] == b"AVI " {
        f.seek(SeekFrom::Start(0))?;
        return Ok(Box::new(MjpegAvi::from_reader(BufReader::new(f))?));
    }

    if &head[0..4] == b"DKIF" {
        if &head[8..12] == b"VP90" {
            f.seek(SeekFrom::Start(0))?;
            return Ok(Box::new(Vp9Ivf::from_reader(BufReader::new(f))?));
        }
        drop(f);
        // IVF だが VP9 以外 (VP80 等) は自作デコーダの対象外。
        #[cfg(windows)]
        {
            return Ok(Box::new(MfVideo::open(path)?));
        }
        #[cfg(not(windows))]
        {
            bail!(
                "VP9 (FourCC=VP90) 以外の IVF は Windows 以外では未対応です: FourCC={:?}",
                &head[8..12]
            );
        }
    }

    drop(f);
    #[cfg(windows)]
    {
        // MP4 (ftyp) / MPEG-1 (000001 スタートコード) / WMV (ASF) 等は
        // すべて Media Foundation の URL オープンに委ねる（コンテナ判定は MF 側が行う）。
        Ok(Box::new(MfVideo::open(path)?))
    }

    #[cfg(not(windows))]
    {
        if &head[4..8] == b"ftyp" {
            bail!("MP4 (H.264/H.265) は Windows 以外では未対応です（ロードマップ参照）");
        }
        bail!("不明な動画形式です（対応: AVI/MJPEG, IVF/VP9。MP4 等は Windows のみ）");
    }
}

fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// AVI コンテナに入った Motion JPEG。
pub struct MjpegAvi<R = Cursor<Vec<u8>>> {
    reader: R,
    /// (offset, len) — 各フレームの JPEG バイト列
    frames: Vec<(u64, usize)>,
    frame_buf: Vec<u8>,
    width: u32,
    height: u32,
    fps: f32,
    /// next_frame() が次に返すフレーム番号（ストリーミング読み出し用カーソル）
    cursor: usize,
}

impl MjpegAvi<Cursor<Vec<u8>>> {
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        Self::from_reader(Cursor::new(data))
    }
}

impl<R: Read + Seek> MjpegAvi<R> {
    pub fn from_reader(mut reader: R) -> Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        ensure!(file_len >= 12, "AVI ファイルではありません");
        reader.seek(SeekFrom::Start(0))?;
        let mut head = [0u8; 12];
        reader.read_exact(&mut head)?;
        ensure!(
            &head[0..4] == b"RIFF" && &head[8..12] == b"AVI ",
            "AVI ファイルではありません"
        );
        let mut width = 0u32;
        let mut height = 0u32;
        let mut fps = 30.0f32;
        let mut frames = Vec::new();

        // LIST の入れ子の上限。実在の AVI は RIFF > LIST(hdrl) > LIST(strl) 程度（3 段）。
        const MAX_LIST_DEPTH: u32 = 8;

        // RIFF チャンクの走査（LIST は再帰的に中を見る）
        #[allow(clippy::too_many_arguments)]
        fn walk<R: Read + Seek>(
            reader: &mut R,
            file_len: u64,
            depth: u32,
            mut pos: u64,
            end: u64,
            width: &mut u32,
            height: &mut u32,
            fps: &mut f32,
            frames: &mut Vec<(u64, usize)>,
        ) -> Result<()> {
            while pos.checked_add(8).is_some_and(|p| p <= end) {
                reader.seek(SeekFrom::Start(pos))?;
                let mut hdr = [0u8; 8];
                reader.read_exact(&mut hdr)?;
                let fourcc = &hdr[0..4];
                let len = u32le(&hdr, 4) as u64;
                let body = pos + 8;
                let body_end = body.checked_add(len).context("AVI チャンクが範囲外")?;
                ensure!(body_end <= file_len, "AVI チャンクが範囲外");
                match fourcc {
                    b"LIST" => {
                        ensure!(depth < MAX_LIST_DEPTH, "AVI の LIST の入れ子が深すぎます");
                        // LIST 種別 4 バイトの後に子チャンクが続く
                        if len >= 4 {
                            walk(
                                reader,
                                file_len,
                                depth + 1,
                                body + 4,
                                body_end,
                                width,
                                height,
                                fps,
                                frames,
                            )?;
                        }
                    }
                    b"avih" => {
                        ensure!(len >= 40, "avih が短すぎます");
                        let mut avih = [0u8; 40];
                        reader.read_exact(&mut avih)?;
                        let usec = u32le(&avih, 0);
                        if usec > 0 {
                            *fps = 1_000_000.0 / usec as f32;
                        }
                        *width = u32le(&avih, 32);
                        *height = u32le(&avih, 36);
                    }
                    // ストリーム 00 のビデオフレーム (dc=compressed, db=uncompressed扱いも JPEG とみなす)
                    b"00dc" | b"00db" if len > 0 => {
                        let len = usize::try_from(len).context("AVI チャンクが範囲外")?;
                        frames.push((body, len));
                    }
                    _ => {}
                }
                let Some(next) = body_end.checked_add(len & 1) else {
                    break;
                };
                pos = next; // 2 バイト境界へパディング
            }
            Ok(())
        }
        walk(
            &mut reader,
            file_len,
            0,
            12,
            file_len,
            &mut width,
            &mut height,
            &mut fps,
            &mut frames,
        )?;

        ensure!(!frames.is_empty(), "ビデオフレームがありません");
        ensure!(width > 0 && height > 0, "AVI のサイズ情報がありません");
        ensure!(fps > 0.0 && fps <= 240.0, "フレームレートが不正: {fps}");
        Ok(Self {
            reader,
            frames,
            frame_buf: Vec::new(),
            width,
            height,
            fps,
            cursor: 0,
        })
    }

    /// AVI 内の総フレーム数（テスト・デバッグ用に公開）。
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    fn read_frame_bytes(&mut self, index: usize) -> Result<()> {
        let (off, len) = *self.frames.get(index).context("フレーム番号が範囲外")?;
        self.reader
            .seek(SeekFrom::Start(off))
            .with_context(|| format!("フレーム {index} へのシークに失敗"))?;
        self.frame_buf.resize(len, 0);
        self.reader
            .read_exact(&mut self.frame_buf)
            .with_context(|| format!("フレーム {index} の読み込みに失敗"))?;
        Ok(())
    }

    pub fn decode_frame_into(&mut self, index: usize, rgba: &mut Vec<u8>) -> Result<()> {
        self.read_frame_bytes(index)?;
        let (w, h) = jpeg::decode_into(&self.frame_buf, rgba)
            .with_context(|| format!("フレーム {index} の JPEG デコードに失敗"))?;
        ensure!(
            w == self.width && h == self.height,
            "フレームサイズが AVI ヘッダと不一致"
        );
        Ok(())
    }

    /// 指定インデックスのフレームをランダムアクセスでデコードする
    /// （テスト互換のために残している。再生経路は `next_frame` を使う）。
    pub fn decode_frame(&mut self, index: usize) -> Result<Vec<u8>> {
        let mut rgba = Vec::new();
        self.decode_frame_into(index, &mut rgba)?;
        Ok(rgba)
    }
}

impl<R: Read + Seek + Send> VideoSource for MjpegAvi<R> {
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
        if self.cursor >= self.frames.len() {
            return Ok(false);
        }
        let index = self.cursor;
        self.cursor += 1;
        self.decode_frame_into(index, buf)?;
        Ok(true)
    }

    fn rewind(&mut self) -> Result<()> {
        self.cursor = 0;
        Ok(())
    }

    fn seek_frame(&mut self, target: usize) -> Result<()> {
        ensure!(
            target <= self.frames.len(),
            "動画の表示フレーム番号が範囲外です: {target} (総数={})",
            self.frames.len()
        );
        self.cursor = target;
        Ok(())
    }
}

/// テストとサンプル生成が共用する最小 AVI ライタ（MJPEG）。
pub fn write_mjpeg_avi(width: u32, height: u32, fps: u32, jpeg_frames: &[Vec<u8>]) -> Vec<u8> {
    let mut movi = Vec::new();
    movi.extend_from_slice(b"movi");
    for f in jpeg_frames {
        movi.extend_from_slice(b"00dc");
        movi.extend_from_slice(&(f.len() as u32).to_le_bytes());
        movi.extend_from_slice(f);
        if f.len() & 1 == 1 {
            movi.push(0);
        }
    }

    let mut avih = Vec::new();
    avih.extend_from_slice(&(1_000_000u32 / fps.max(1)).to_le_bytes()); // usec/frame
    avih.extend_from_slice(&0u32.to_le_bytes()); // max bytes/sec
    avih.extend_from_slice(&0u32.to_le_bytes()); // padding
    avih.extend_from_slice(&0u32.to_le_bytes()); // flags
    avih.extend_from_slice(&(jpeg_frames.len() as u32).to_le_bytes());
    avih.extend_from_slice(&0u32.to_le_bytes()); // initial frames
    avih.extend_from_slice(&1u32.to_le_bytes()); // streams
    avih.extend_from_slice(&0u32.to_le_bytes()); // suggested buffer
    avih.extend_from_slice(&width.to_le_bytes());
    avih.extend_from_slice(&height.to_le_bytes());
    avih.extend_from_slice(&[0u8; 16]); // reserved

    let mut hdrl = Vec::new();
    hdrl.extend_from_slice(b"hdrl");
    hdrl.extend_from_slice(b"avih");
    hdrl.extend_from_slice(&(avih.len() as u32).to_le_bytes());
    hdrl.extend_from_slice(&avih);

    let mut riff_body = Vec::new();
    riff_body.extend_from_slice(b"AVI ");
    riff_body.extend_from_slice(b"LIST");
    riff_body.extend_from_slice(&(hdrl.len() as u32).to_le_bytes());
    riff_body.extend_from_slice(&hdrl);
    riff_body.extend_from_slice(b"LIST");
    riff_body.extend_from_slice(&(movi.len() as u32).to_le_bytes());
    riff_body.extend_from_slice(&movi);

    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(riff_body.len() as u32).to_le_bytes());
    out.extend_from_slice(&riff_body);
    out
}

/// IVF ファイルヘッダのサイズ（バイト、通常値）。
const IVF_FILE_HEADER_SIZE: usize = 32;
/// IVF フレームヘッダのサイズ（バイト）。
const IVF_FRAME_HEADER_SIZE: usize = 12;

/// IVF (VP9) コンテナを自作デコーダ (`vp9dec`) で再生する。
///
/// フレーム位置を `(offset, len)` のインデックスだけ先に歩いておき、
/// 再生時は `reader` から 1 フレーム分ずつ `frame_buf` へ読み出す。
pub struct Vp9Ivf<R = Cursor<Vec<u8>>> {
    reader: R,
    /// (offset, len) — 各 IVF フレームの VP9 ビットストリーム（デコード順）
    frames: Vec<(u64, usize)>,
    frame_buf: Vec<u8>,
    width: u32,
    height: u32,
    fps: f32,
    decoder: vp9dec::Decoder,
    /// next_frame() が次に読む IVF フレーム番号（ストリーミング読み出し用カーソル）
    cursor: usize,
}

impl Vp9Ivf<Cursor<Vec<u8>>> {
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        Self::from_reader(Cursor::new(data))
    }
}

impl<R: Read + Seek> Vp9Ivf<R> {
    pub fn from_reader(mut reader: R) -> Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        ensure!(
            file_len >= IVF_FILE_HEADER_SIZE as u64,
            "IVF ファイルではありません"
        );
        reader.seek(SeekFrom::Start(0))?;
        let mut hdr = [0u8; IVF_FILE_HEADER_SIZE];
        reader.read_exact(&mut hdr)?;
        ensure!(&hdr[0..4] == b"DKIF", "IVF ファイルではありません");
        let fourcc = &hdr[8..12];
        ensure!(
            fourcc == b"VP90",
            "VP9 (FourCC=VP90) 以外の IVF は自作デコーダ非対応です: {:?}",
            fourcc
        );

        let header_length = u16::from_le_bytes([hdr[6], hdr[7]]) as u64;
        ensure!(
            (IVF_FILE_HEADER_SIZE as u64..=file_len).contains(&header_length),
            "IVF ヘッダ長が不正です"
        );

        let width = u16::from_le_bytes([hdr[12], hdr[13]]) as u32;
        let height = u16::from_le_bytes([hdr[14], hdr[15]]) as u32;
        let timebase_denominator = u32le(&hdr, 16);
        let timebase_numerator = u32le(&hdr, 20);
        ensure!(width > 0 && height > 0, "IVF のサイズ情報がありません");
        let fps = if timebase_numerator > 0 {
            timebase_denominator as f32 / timebase_numerator as f32
        } else {
            30.0
        };
        ensure!(fps > 0.0 && fps <= 240.0, "フレームレートが不正: {fps}");

        // フレーム位置を一度だけ歩いて記録する（12 バイトのフレームヘッダ + フレームデータ）。
        let mut frames = Vec::new();
        let mut pos = header_length;
        if pos != IVF_FILE_HEADER_SIZE as u64 {
            reader.seek(SeekFrom::Start(pos))?;
        }
        while pos
            .checked_add(IVF_FRAME_HEADER_SIZE as u64)
            .is_some_and(|p| p <= file_len)
        {
            let mut fh = [0u8; IVF_FRAME_HEADER_SIZE];
            reader.read_exact(&mut fh)?;
            let frame_len = u32le(&fh, 0) as u64;
            let data_start = pos + IVF_FRAME_HEADER_SIZE as u64;
            let data_end = data_start
                .checked_add(frame_len)
                .context("IVF フレーム長が不正")?;
            ensure!(data_end <= file_len, "IVF フレームデータが範囲外です");
            if frame_len > 0 {
                let len = usize::try_from(frame_len).context("IVF フレーム長が不正")?;
                frames.push((data_start, len));
                reader.seek(SeekFrom::Start(data_end))?;
            }
            pos = data_end;
        }
        ensure!(!frames.is_empty(), "ビデオフレームがありません");

        Ok(Self {
            reader,
            frames,
            frame_buf: Vec::new(),
            width,
            height,
            fps,
            decoder: vp9dec::Decoder::new(),
            cursor: 0,
        })
    }

    fn read_chunk(&mut self) -> Result<Option<usize>> {
        if self.cursor >= self.frames.len() {
            return Ok(None);
        }
        let (off, len) = self.frames[self.cursor];
        let index = self.cursor;
        self.cursor += 1;
        self.reader
            .seek(SeekFrom::Start(off))
            .with_context(|| format!("VP9 フレーム {index} へのシークに失敗"))?;
        self.frame_buf.resize(len, 0);
        self.reader
            .read_exact(&mut self.frame_buf)
            .with_context(|| format!("VP9 フレーム {index} の読み込みに失敗"))?;
        Ok(Some(index))
    }
}

impl<R: Read + Seek + Send> VideoSource for Vp9Ivf<R> {
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
        while let Some(index) = self.read_chunk()? {
            match self.decoder.decode_frame(&self.frame_buf) {
                // vp9dec はチャンク（IVF フレーム 1 個）毎に構成フレームの Vec を返し、
                // 表示フレーム（frame: Some）は高々 1 つ（superframe 内の非表示
                // altref や SVC 下位レイヤは frame: None / 表示は最終 shown のみ）。
                Ok(decoded) => {
                    if let Some(frame) = decoded.into_iter().find_map(|df| df.frame) {
                        vp9_frame_to_rgba_bt601_into(&frame, buf)
                            .with_context(|| format!("VP9 フレーム {index} の RGBA 変換に失敗"))?;
                        self.width = frame.width;
                        self.height = frame.height;
                        return Ok(true);
                    }
                    // 非表示フレームのみのチャンク（droppable/altref）はスキップして次へ。
                }
                Err(e) => bail!("VP9 フレーム {index} のデコードに失敗: {e:?}"),
            }
        }
        Ok(false)
    }

    fn rewind(&mut self) -> Result<()> {
        // VP9 はフレーム間状態（参照フレーム・確率テーブル等）を持つため、
        // 巻き戻しはデコーダを作り直してフレーム位置を 0 に戻すことで行う。
        self.decoder = vp9dec::Decoder::new();
        self.cursor = 0;
        Ok(())
    }

    fn seek_frame(&mut self, target: usize) -> Result<()> {
        self.rewind()?;
        let mut shown = 0usize;
        while shown < target {
            let Some(index) = self.read_chunk()? else {
                bail!("動画の表示フレーム番号が範囲外です: {target}");
            };
            match self.decoder.decode_frame(&self.frame_buf) {
                Ok(decoded) => {
                    if let Some(frame) = decoded.into_iter().find_map(|df| df.frame) {
                        self.width = frame.width;
                        self.height = frame.height;
                        shown += 1;
                    }
                }
                Err(e) => bail!("VP9 フレーム {index} のデコードに失敗: {e:?}"),
            }
        }
        Ok(())
    }
}

fn vp9_frame_to_rgba_bt601_into(frame: &vp9dec::Frame, rgba: &mut Vec<u8>) -> Result<()> {
    match frame.bit_depth {
        8 => {
            ensure!(
                matches!(
                    (&frame.y, &frame.u, &frame.v),
                    (
                        vp9dec::PlaneData::U8(_),
                        vp9dec::PlaneData::U8(_),
                        vp9dec::PlaneData::U8(_)
                    )
                ),
                "8-bit VP9 フレームの Y/U/V plane はすべて U8 である必要があります"
            );
            validate_vp9_plane_lengths(frame)?;
            yuv_to_rgba_bt601_into(frame, rgba);
            Ok(())
        }
        10 | 12 => yuv_to_rgba_bt601_hbd_into(frame, rgba),
        bit_depth => {
            bail!("VP9 の bit depth {bit_depth} は RGBA 変換非対応です（対応: 8/10/12-bit）")
        }
    }
}

#[cfg(test)]
fn vp9_frame_to_rgba_bt601(frame: &vp9dec::Frame) -> Result<Vec<u8>> {
    let mut rgba = Vec::new();
    vp9_frame_to_rgba_bt601_into(frame, &mut rgba)?;
    Ok(rgba)
}

fn validate_vp9_plane_lengths(frame: &vp9dec::Frame) -> Result<()> {
    ensure!(
        frame.subsampling_x <= 1 && frame.subsampling_y <= 1,
        "VP9 の chroma subsampling が不正です: x={}, y={}",
        frame.subsampling_x,
        frame.subsampling_y
    );

    let width = frame.width as usize;
    let height = frame.height as usize;
    let uv_width = frame
        .width
        .checked_add(frame.subsampling_x)
        .context("VP9 chroma plane の幅が範囲外です")?
        >> frame.subsampling_x;
    let uv_height = frame
        .height
        .checked_add(frame.subsampling_y)
        .context("VP9 chroma plane の高さが範囲外です")?
        >> frame.subsampling_y;
    let y_len = width
        .checked_mul(height)
        .context("VP9 luma plane のサイズが範囲外です")?;
    let uv_len = (uv_width as usize)
        .checked_mul(uv_height as usize)
        .context("VP9 chroma plane のサイズが範囲外です")?;

    ensure!(
        frame.y.len() == y_len,
        "VP9 Y plane のサンプル数が不正です: 期待 {y_len}, 実際 {}",
        frame.y.len()
    );
    ensure!(
        frame.u.len() == uv_len,
        "VP9 U plane のサンプル数が不正です: 期待 {uv_len}, 実際 {}",
        frame.u.len()
    );
    ensure!(
        frame.v.len() == uv_len,
        "VP9 V plane のサンプル数が不正です: 期待 {uv_len}, 実際 {}",
        frame.v.len()
    );
    Ok(())
}

/// 8-bit YUV を straight alpha の RGBA8 へ変換し `rgba` へ書き込む（BT.601, limited range）。
/// クロマは frame の `subsampling_x/y` に従い最近傍 (`x>>sub_x, y>>sub_y`) で
/// アップサンプリングするので 4:2:0/4:2:2/4:4:0/4:4:4 のいずれにも対応する
/// （係数は `vp9dec` の目視検収用 PNG 出力 `examples/decode_to_png.rs` と同じ）。
/// 呼び出し側で `frame.bit_depth == 8` を保証していること（`as_u8` は 10/12-bit で panic）。
fn yuv_to_rgba_bt601_into(frame: &vp9dec::Frame, rgba: &mut Vec<u8>) {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let sub_x = frame.subsampling_x;
    let sub_y = frame.subsampling_y;
    // クロマ平面の幅（サンプル数）: spec §8.9 の ((width + sub_x) >> sub_x)。
    let uv_w = ((frame.width + sub_x) >> sub_x) as usize;
    let y_plane = frame.y.as_u8();
    let u_plane = frame.u.as_u8();
    let v_plane = frame.v.as_u8();

    rgba.resize(w * h * 4, 0);
    for y in 0..h {
        for x in 0..w {
            let y_val = y_plane[y * w + x] as f32;
            let cx = x >> sub_x;
            let cy = y >> sub_y;
            let u_val = u_plane[cy * uv_w + cx] as f32;
            let v_val = v_plane[cy * uv_w + cx] as f32;

            let yy = 1.164 * (y_val - 16.0);
            let cb = u_val - 128.0;
            let cr = v_val - 128.0;

            let r = yy + 1.596 * cr;
            let g = yy - 0.392 * cb - 0.813 * cr;
            let b = yy + 2.017 * cb;

            let idx = (y * w + x) * 4;
            rgba[idx] = clamp_u8(r);
            rgba[idx + 1] = clamp_u8(g);
            rgba[idx + 2] = clamp_u8(b);
            rgba[idx + 3] = 255;
        }
    }
}

fn yuv_to_rgba_bt601_hbd_into(frame: &vp9dec::Frame, rgba: &mut Vec<u8>) -> Result<()> {
    let (y_plane, u_plane, v_plane) = match (&frame.y, &frame.u, &frame.v) {
        (vp9dec::PlaneData::U16(y), vp9dec::PlaneData::U16(u), vp9dec::PlaneData::U16(v)) => {
            (y.as_slice(), u.as_slice(), v.as_slice())
        }
        _ => bail!(
            "{}-bit VP9 フレームの Y/U/V plane はすべて U16 である必要があります",
            frame.bit_depth
        ),
    };
    validate_vp9_plane_lengths(frame)?;

    let w = frame.width as usize;
    let h = frame.height as usize;
    let sub_x = frame.subsampling_x;
    let sub_y = frame.subsampling_y;
    let uv_w = ((frame.width + sub_x) >> sub_x) as usize;
    let scale = (1u16 << (frame.bit_depth - 8)) as f32;

    // Frame/IVF に伝達関数やマスタリング情報が無いため、PQ/HLG を推測せず決定的な SDR スケーリングに留める。
    rgba.resize(w * h * 4, 0);
    for y in 0..h {
        for x in 0..w {
            let y_val = y_plane[y * w + x] as f32 / scale;
            let cx = x >> sub_x;
            let cy = y >> sub_y;
            let u_val = u_plane[cy * uv_w + cx] as f32 / scale;
            let v_val = v_plane[cy * uv_w + cx] as f32 / scale;

            let yy = 1.164 * (y_val - 16.0);
            let cb = u_val - 128.0;
            let cr = v_val - 128.0;

            let r = yy + 1.596 * cr;
            let g = yy - 0.392 * cb - 0.813 * cr;
            let b = yy + 2.017 * cb;

            let idx = (y * w + x) * 4;
            rgba[idx] = clamp_u8(r);
            rgba[idx + 1] = clamp_u8(g);
            rgba[idx + 2] = clamp_u8(b);
            rgba[idx + 3] = 255;
        }
    }
    Ok(())
}

fn clamp_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;

    fn make_frame(w: u32, h: u32, shade: u8) -> Vec<u8> {
        let rgb: Vec<u8> = (0..w * h).flat_map(|_| [shade, 128, 255 - shade]).collect();
        let mut jpg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpg, 90)
            .encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        jpg
    }

    fn hbd_frame(
        size: (u32, u32),
        bit_depth: u8,
        subsampling: (u32, u32),
        y: Vec<u16>,
        u: Vec<u16>,
        v: Vec<u16>,
    ) -> vp9dec::Frame {
        vp9dec::Frame {
            width: size.0,
            height: size.1,
            bit_depth,
            subsampling_x: subsampling.0,
            subsampling_y: subsampling.1,
            y: vp9dec::PlaneData::U16(y),
            u: vp9dec::PlaneData::U16(u),
            v: vp9dec::PlaneData::U16(v),
        }
    }

    fn rgba_pixel(rgba: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
        let offset = (y * width + x) * 4;
        rgba[offset..offset + 4].try_into().unwrap()
    }

    #[test]
    fn vp9_8bit_bt601_known_values_are_unchanged() {
        let frame = vp9dec::Frame {
            width: 3,
            height: 1,
            bit_depth: 8,
            subsampling_x: 0,
            subsampling_y: 0,
            y: vp9dec::PlaneData::U8(vec![16, 235, 81]),
            u: vp9dec::PlaneData::U8(vec![128, 128, 90]),
            v: vp9dec::PlaneData::U8(vec![128, 128, 240]),
        };

        assert_eq!(
            vp9_frame_to_rgba_bt601(&frame).unwrap(),
            vec![0, 0, 0, 255, 255, 255, 255, 255, 254, 0, 0, 255]
        );
    }

    #[test]
    fn vp9_hbd_black_white_and_neutral_scale_at_10_and_12_bit() {
        for (bit_depth, scale) in [(10, 4u16), (12, 16u16)] {
            let frame = hbd_frame(
                (3, 1),
                bit_depth,
                (0, 0),
                vec![16 * scale, 235 * scale, 128 * scale],
                vec![128 * scale; 3],
                vec![128 * scale; 3],
            );

            assert_eq!(
                vp9_frame_to_rgba_bt601(&frame).unwrap(),
                vec![0, 0, 0, 255, 255, 255, 255, 255, 130, 130, 130, 255],
                "{bit_depth}-bit"
            );
        }
    }

    #[test]
    fn vp9_hbd_keeps_fractional_luma_and_clamps_superwhite() {
        let frame = hbd_frame(
            (2, 1),
            10,
            (0, 0),
            vec![66, 1023],
            vec![512, 512],
            vec![512, 512],
        );

        assert_eq!(
            vp9_frame_to_rgba_bt601(&frame).unwrap(),
            vec![1, 1, 1, 255, 255, 255, 255, 255]
        );
    }

    #[test]
    fn vp9_hbd_odd_420_uses_last_chroma_row_and_column() {
        let frame = hbd_frame(
            (3, 3),
            10,
            (1, 1),
            vec![512; 9],
            vec![512; 4],
            vec![512, 768, 256, 512],
        );
        let rgba = vp9_frame_to_rgba_bt601(&frame).unwrap();

        assert_eq!(rgba.len(), 3 * 3 * 4);
        assert_eq!(rgba_pixel(&rgba, 3, 0, 0), [130, 130, 130, 255]);
        assert_eq!(rgba_pixel(&rgba, 3, 2, 0), [233, 78, 130, 255]);
        assert_eq!(rgba_pixel(&rgba, 3, 0, 2), [28, 182, 130, 255]);
        assert_eq!(rgba_pixel(&rgba, 3, 2, 2), [130, 130, 130, 255]);
    }

    #[test]
    fn vp9_hbd_422_indexes_chroma_on_every_row() {
        let frame = hbd_frame(
            (3, 2),
            10,
            (1, 0),
            vec![512; 6],
            vec![512; 4],
            vec![512, 768, 256, 512],
        );
        let rgba = vp9_frame_to_rgba_bt601(&frame).unwrap();

        assert_eq!(rgba_pixel(&rgba, 3, 0, 0), [130, 130, 130, 255]);
        assert_eq!(rgba_pixel(&rgba, 3, 2, 0), [233, 78, 130, 255]);
        assert_eq!(rgba_pixel(&rgba, 3, 0, 1), [28, 182, 130, 255]);
        assert_eq!(rgba_pixel(&rgba, 3, 2, 1), [130, 130, 130, 255]);
    }

    #[test]
    fn vp9_rgba_conversion_rejects_unsupported_depth_and_plane_types() {
        let unsupported = hbd_frame((1, 1), 9, (0, 0), vec![0], vec![0], vec![0]);
        let err = vp9_frame_to_rgba_bt601(&unsupported)
            .unwrap_err()
            .to_string();
        assert!(err.contains("bit depth 9"), "{err}");

        let wrong_8bit_planes = hbd_frame((1, 1), 8, (0, 0), vec![0], vec![0], vec![0]);
        let err = vp9_frame_to_rgba_bt601(&wrong_8bit_planes)
            .unwrap_err()
            .to_string();
        assert!(err.contains("U8"), "{err}");

        let mut wrong_10bit_planes = hbd_frame((1, 1), 10, (0, 0), vec![64], vec![512], vec![512]);
        wrong_10bit_planes.u = vp9dec::PlaneData::U8(vec![128]);
        let err = vp9_frame_to_rgba_bt601(&wrong_10bit_planes)
            .unwrap_err()
            .to_string();
        assert!(err.contains("U16"), "{err}");
    }

    #[test]
    fn roundtrip_write_parse_decode() {
        let (w, h) = (64u32, 48u32);
        let frames: Vec<Vec<u8>> = [40u8, 120, 200]
            .iter()
            .map(|&s| make_frame(w, h, s))
            .collect();
        let avi = write_mjpeg_avi(w, h, 12, &frames);

        // ランダムアクセス API (frame_count/decode_frame) は MjpegAvi の
        // 固有メソッドとして残っている（テスト互換）。
        let mut video = MjpegAvi::parse(avi.clone()).unwrap();
        assert_eq!(video.width(), w);
        assert_eq!(video.height(), h);
        assert!((video.fps() - 12.0).abs() < 0.1);
        assert_eq!(video.frame_count(), 3);

        // 各フレームがおおよそ期待の色でデコードされる
        for (i, &shade) in [40u8, 120, 200].iter().enumerate() {
            let rgba = video.decode_frame(i).unwrap();
            assert_eq!(rgba.len(), (w * h * 4) as usize);
            let center = ((h / 2 * w + w / 2) * 4) as usize;
            assert!(
                (rgba[center] as i32 - shade as i32).abs() <= 12,
                "フレーム {i}: R={} 期待≒{shade}",
                rgba[center]
            );
        }
    }

    #[test]
    fn streaming_next_frame_and_rewind() {
        let (w, h) = (32u32, 24u32);
        let frames: Vec<Vec<u8>> = [10u8, 220].iter().map(|&s| make_frame(w, h, s)).collect();
        let avi = write_mjpeg_avi(w, h, 10, &frames);

        // VideoSource トレイト経由（再生スレッドが実際に使う経路）
        let mut video: Box<dyn VideoSource> = open_video(avi).unwrap();
        let f0 = video.next_frame().unwrap().expect("1枚目がある");
        assert_eq!(f0.len(), (w * h * 4) as usize);
        let f1 = video.next_frame().unwrap().expect("2枚目がある");
        assert_ne!(f0, f1);
        assert!(video.next_frame().unwrap().is_none(), "3枚目は無いはず");

        video.rewind().unwrap();
        let f0_again = video.next_frame().unwrap().expect("巻き戻し後の1枚目");
        assert_eq!(f0, f0_again);
    }

    #[test]
    fn mjpeg_seek_frame_supports_middle_eof_and_rejects_past_eof() {
        let (w, h) = (32u32, 24u32);
        let frames: Vec<Vec<u8>> = [10u8, 120, 220]
            .iter()
            .map(|&s| make_frame(w, h, s))
            .collect();
        let avi = write_mjpeg_avi(w, h, 10, &frames);
        let mut sequential = open_video(avi.clone()).unwrap();
        sequential.next_frame().unwrap().unwrap();
        let expected_middle = sequential.next_frame().unwrap().unwrap();

        let mut video: Box<dyn VideoSource> = open_video(avi).unwrap();
        video.seek_frame(1).unwrap();
        assert_eq!(video.next_frame().unwrap().unwrap(), expected_middle);

        video.seek_frame(3).unwrap();
        assert!(video.next_frame().unwrap().is_none());

        let err = video.seek_frame(4).unwrap_err().to_string();
        assert!(err.contains("範囲外"), "{err}");
    }

    #[test]
    fn unknown_formats_give_clear_errors() {
        let mpeg = vec![0x00, 0x00, 0x01, 0xBA, 0, 0, 0, 0, 0, 0, 0, 0];
        let err = match open_video(mpeg) {
            Ok(_) => panic!("MPEG-1 は未対応のはず"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("MPEG-1"), "{err}");

        let mut mp4 = vec![0, 0, 0, 24];
        mp4.extend_from_slice(b"ftypisom");
        mp4.extend_from_slice(&[0; 16]);
        let err = match open_video(mp4) {
            Ok(_) => panic!("MP4 は未対応のはず"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("MP4"), "{err}");
    }

    /// vp9dec 側のコンフォーマンステストベクタ（IVF 実ファイル）を使った統合テスト。
    /// vp9dec リポジトリが隣に無い環境（CI 等）ではベクタが無いのでスキップする。
    fn droppable_1_vector_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../vp9dec/tests/vectors/vp90-2-12-droppable_1.ivf")
    }

    #[test]
    fn vp9_ivf_open_and_dimensions() {
        let path = droppable_1_vector_path();
        if !path.exists() {
            eprintln!(
                "[skip] vp9dec テストベクタが見つかりません: {}",
                path.display()
            );
            return;
        }
        let video = open_video_file(&path).unwrap();
        assert_eq!(video.width(), 352);
        assert_eq!(video.height(), 288);
        assert!(
            (25.0..=30.0).contains(&video.fps()),
            "fps が想定範囲外: {}",
            video.fps()
        );
    }

    #[test]
    fn vp9_ivf_decodes_all_display_frames_and_rewinds() {
        let path = droppable_1_vector_path();
        if !path.exists() {
            eprintln!(
                "[skip] vp9dec テストベクタが見つかりません: {}",
                path.display()
            );
            return;
        }
        let mut video = open_video_file(&path).unwrap();
        let (w, h) = (video.width(), video.height());

        let mut shown = 0usize;
        let mut first_frame: Option<Vec<u8>> = None;
        while let Some(rgba) = video.next_frame().unwrap() {
            assert_eq!(rgba.len(), (w * h * 4) as usize);
            if first_frame.is_none() {
                first_frame = Some(rgba);
            }
            shown += 1;
        }
        assert_eq!(shown, 99, "表示フレーム数が想定と異なる");

        // 先頭フレームの中心付近ピクセルが非ゼロ（実写映像なので真っ黒/透明ではないはず）。
        let first_frame = first_frame.expect("少なくとも1枚は表示フレームがあるはず");
        let center = ((h / 2 * w + w / 2) * 4) as usize;
        let pixel = &first_frame[center..center + 4];
        assert!(
            pixel.iter().any(|&c| c != 0),
            "先頭フレーム中心ピクセルが全ゼロ: {pixel:?}"
        );

        // rewind 後にまた最初から読めること（同じ先頭フレームが得られる）。
        video.rewind().unwrap();
        let f0_again = video.next_frame().unwrap().expect("巻き戻し後の1枚目");
        assert_eq!(first_frame, f0_again, "rewind 後の1枚目が一致しない");
    }

    /// 8-bit 非 4:2:0（ここでは 4:4:4）の VP9 を最後まで RGBA へ変換できること。
    /// `yuv_to_rgba_bt601` が `subsampling_x/y` からクロマをアップサンプリングする経路の
    /// 実再生検証（4:2:0 前提のハードコードだと range 外アクセスで panic していた）。
    #[test]
    fn vp9_ivf_8bit_yuv444_decodes_to_rgba() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../vp9dec/tests/vectors/vp91-2-04-yuv444.ivf");
        if !path.exists() {
            eprintln!(
                "[skip] vp91 4:4:4 ベクタが見つかりません: {}",
                path.display()
            );
            return;
        }
        let mut video = open_video_file(&path).unwrap();
        let (w, h) = (video.width(), video.height());
        let mut shown = 0usize;
        while let Some(rgba) = video.next_frame().unwrap() {
            assert_eq!(rgba.len(), (w * h * 4) as usize);
            shown += 1;
        }
        assert!(shown > 0, "表示フレームが1枚も無い");
    }

    fn fnv1a64(bytes: &[u8]) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        for &byte in bytes {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    fn assert_hbd_fixture_decodes(filename: &str, bytes: &[u8], expected_hashes: [u64; 2]) {
        let mut video = Vp9Ivf::parse(bytes.to_vec()).unwrap();
        assert_eq!(video.frames.len(), 2, "{filename}");
        assert_eq!((video.width(), video.height()), (16, 16), "{filename}");

        let mut frames = Vec::new();
        let mut hashes = Vec::new();
        while let Some(rgba) = video.next_frame().unwrap() {
            assert_eq!(rgba.len(), 16 * 16 * 4, "{filename}");
            assert!(
                rgba.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255),
                "{filename}: alpha が 255 でない pixel があります"
            );
            assert!(
                rgba.as_chunks::<4>()
                    .0
                    .iter()
                    .skip(1)
                    .any(|pixel| pixel[..3] != rgba[..3]),
                "{filename}: 単色フレームです"
            );
            hashes.push(fnv1a64(&rgba));
            frames.push(rgba);
        }
        assert_eq!(frames.len(), 2, "{filename}: 表示フレーム数");
        assert_ne!(frames[0], frames[1], "{filename}: 2フレームが同一です");
        assert_eq!(hashes, expected_hashes, "{filename}: RGBA hash");

        video.rewind().unwrap();
        assert_eq!(
            video.next_frame().unwrap().unwrap(),
            frames[0],
            "{filename}: rewind 後の先頭フレームが一致しない"
        );
    }

    #[test]
    fn vp9_ivf_fixture_10bit_yuv420_decodes_all_frames_and_rewinds() {
        assert_hbd_fixture_decodes(
            "vp9_p2_10bit_yuv420_16x16_2f.ivf",
            include_bytes!("../../tests/fixtures/vp9_p2_10bit_yuv420_16x16_2f.ivf"),
            [0xf81cafcd4ca58f30, 0xcba89f5f439cb1ad],
        );
    }

    #[test]
    fn vp9_ivf_seek_frame_one_matches_sequential_decode() {
        let bytes =
            include_bytes!("../../tests/fixtures/vp9_p2_10bit_yuv420_16x16_2f.ivf").to_vec();
        let mut sequential = Vp9Ivf::parse(bytes.clone()).unwrap();
        sequential.next_frame().unwrap().unwrap();
        let expected = sequential.next_frame().unwrap().unwrap();

        let mut video: Box<dyn VideoSource> = Box::new(Vp9Ivf::parse(bytes).unwrap());
        video.seek_frame(1).unwrap();
        assert_eq!(video.next_frame().unwrap().unwrap(), expected);

        video.seek_frame(2).unwrap();
        assert!(video.next_frame().unwrap().is_none());
        let err = video.seek_frame(3).unwrap_err().to_string();
        assert!(err.contains("範囲外"), "{err}");
    }

    #[test]
    fn vp9_ivf_fixture_12bit_yuv444_decodes_all_frames_and_rewinds() {
        assert_hbd_fixture_decodes(
            "vp9_p3_12bit_yuv444_16x16_2f.ivf",
            include_bytes!("../../tests/fixtures/vp9_p3_12bit_yuv444_16x16_2f.ivf"),
            [0x3caf05ee5b97bebe, 0xc8fa2bb62cb35af9],
        );
    }

    #[test]
    fn vp9_ivf_dimensions_follow_the_last_decoded_frame() {
        let mut bytes =
            include_bytes!("../../tests/fixtures/vp9_p2_10bit_yuv420_16x16_2f.ivf").to_vec();
        bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&1u16.to_le_bytes());
        let mut video = Vp9Ivf::parse(bytes).unwrap();
        assert_eq!((video.width(), video.height()), (1, 1));

        let rgba = video.next_frame().unwrap().unwrap();

        assert_eq!((video.width(), video.height()), (16, 16));
        assert_eq!(rgba.len(), 16 * 16 * 4);
    }
    // --- R1: AVI の LIST 入れ子 ---

    fn riff_chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(id);
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        if body.len() & 1 == 1 {
            v.push(0);
        }
        v
    }

    /// avih とビデオフレーム 1 枚を `depth` 重の LIST の奥に置いた AVI。
    fn nested_avi(depth: usize) -> Vec<u8> {
        let mut avih = vec![0u8; 56];
        avih[0..4].copy_from_slice(&33_333u32.to_le_bytes());
        avih[32..36].copy_from_slice(&16u32.to_le_bytes());
        avih[36..40].copy_from_slice(&16u32.to_le_bytes());
        let mut inner = riff_chunk(b"avih", &avih);
        inner.extend(riff_chunk(b"00dc", &make_frame(16, 16, 100)));
        for _ in 0..depth {
            let mut body = b"strl".to_vec();
            body.extend(inner);
            inner = riff_chunk(b"LIST", &body);
        }
        let mut body = b"AVI ".to_vec();
        body.extend(inner);
        riff_chunk(b"RIFF", &body)
    }

    #[test]
    fn avi_list_nesting_is_depth_limited() {
        assert!(MjpegAvi::parse(nested_avi(3)).is_ok(), "実在する程度の入れ子は読める");
        assert!(MjpegAvi::parse(nested_avi(200)).is_err());
        // 数万重でもスタックを使い切らず Err を返す。
        assert!(MjpegAvi::parse(nested_avi(20_000)).is_err());
    }
    #[test]
    fn fuzz_corrupted_avi_never_panics_or_overallocates() {
        let frames: Vec<Vec<u8>> = (0..3).map(|i| make_frame(16, 16, 60 + i * 50)).collect();
        let base = write_mjpeg_avi(16, 16, 10, &frames);
        assert!(MjpegAvi::parse(base.clone()).is_ok());
        crate::test_support::fuzz("avi", 0x6666, 2000, &base, |d| {
            if let Ok(mut v) = MjpegAvi::parse(d.to_vec()) {
                for _ in 0..4 {
                    match v.next_frame() {
                        Ok(Some(f)) => {
                            assert_eq!(f.len(), v.width() as usize * v.height() as usize * 4)
                        }
                        _ => break,
                    }
                }
            }
        });
    }

    #[test]
    fn fuzz_corrupted_ivf_never_panics_or_overallocates() {
        let base = include_bytes!("../../tests/fixtures/vp9_p2_10bit_yuv420_16x16_2f.ivf");
        assert!(Vp9Ivf::parse(base.to_vec()).is_ok());
        crate::test_support::fuzz("ivf", 0x7777, 1000, base, |d| {
            if let Ok(mut v) = Vp9Ivf::parse(d.to_vec()) {
                for _ in 0..3 {
                    if !matches!(v.next_frame(), Ok(Some(_))) {
                        break;
                    }
                }
            }
        });
    }

    fn write_temp_video(tag: &str, ext: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "noiria_video_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("clip.{ext}"));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn open_video_file_mjpeg_avi_playback_rewind_and_seek() {
        let (w, h) = (32u32, 24u32);
        let frames: Vec<Vec<u8>> = [20u8, 120, 220]
            .iter()
            .map(|&s| make_frame(w, h, s))
            .collect();
        let avi = write_mjpeg_avi(w, h, 15, &frames);
        let mut expected_src = open_video(avi.clone()).unwrap();
        let mut expected = Vec::new();
        while let Some(f) = expected_src.next_frame().unwrap() {
            expected.push(f);
        }
        assert_eq!(expected.len(), 3);

        let path = write_temp_video("avi_stream", "avi", &avi);
        {
            let mut video = open_video_file(&path).unwrap();
            assert_eq!((video.width(), video.height()), (w, h));
            assert!((video.fps() - 15.0).abs() < 0.1);

            for (i, want) in expected.iter().enumerate() {
                assert_eq!(video.next_frame().unwrap().as_ref(), Some(want), "frame {i}");
            }
            assert!(video.next_frame().unwrap().is_none());

            video.rewind().unwrap();
            assert_eq!(video.next_frame().unwrap().as_ref(), Some(&expected[0]));

            video.seek_frame(2).unwrap();
            assert_eq!(video.next_frame().unwrap().as_ref(), Some(&expected[2]));
            assert!(video.next_frame().unwrap().is_none());

            video.seek_frame(1).unwrap();
            assert_eq!(video.next_frame().unwrap().as_ref(), Some(&expected[1]));

            video.seek_frame(3).unwrap();
            assert!(video.next_frame().unwrap().is_none());
            assert!(video.seek_frame(4).is_err());
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn open_video_file_vp9_ivf_playback_rewind_and_seek() {
        let fixtures: [(&str, &[u8]); 2] = [
            (
                "vp9_10bit",
                include_bytes!("../../tests/fixtures/vp9_p2_10bit_yuv420_16x16_2f.ivf"),
            ),
            (
                "vp9_12bit",
                include_bytes!("../../tests/fixtures/vp9_p3_12bit_yuv444_16x16_2f.ivf"),
            ),
        ];
        for (tag, bytes) in fixtures {
            let mut mem = open_video(bytes.to_vec()).unwrap();
            let mut expected = Vec::new();
            while let Some(f) = mem.next_frame().unwrap() {
                expected.push(f);
            }
            assert_eq!(expected.len(), 2, "{tag}");

            let path = write_temp_video(tag, "ivf", bytes);
            {
                let mut video = open_video_file(&path).unwrap();
                assert_eq!((video.width(), video.height()), (16, 16), "{tag}");

                for (i, want) in expected.iter().enumerate() {
                    assert_eq!(
                        video.next_frame().unwrap().as_ref(),
                        Some(want),
                        "{tag} frame {i}"
                    );
                }
                assert!(video.next_frame().unwrap().is_none(), "{tag} EOF");

                video.rewind().unwrap();
                assert_eq!(
                    video.next_frame().unwrap().as_ref(),
                    Some(&expected[0]),
                    "{tag} rewind"
                );

                video.seek_frame(1).unwrap();
                assert_eq!(
                    video.next_frame().unwrap().as_ref(),
                    Some(&expected[1]),
                    "{tag} seek 1"
                );
                assert!(video.next_frame().unwrap().is_none(), "{tag} after seek 1");

                video.seek_frame(2).unwrap();
                assert!(video.next_frame().unwrap().is_none(), "{tag} seek EOF");
                assert!(video.seek_frame(3).is_err(), "{tag} seek past EOF");
            }
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
    }

    #[test]
    fn next_frame_into_matches_next_frame_and_reuses_buffer() {
        let (w, h) = (32u32, 24u32);
        let avi = write_mjpeg_avi(
            w,
            h,
            12,
            &[15u8, 128, 240]
                .iter()
                .map(|&s| make_frame(w, h, s))
                .collect::<Vec<_>>(),
        );
        let ivf = include_bytes!("../../tests/fixtures/vp9_p2_10bit_yuv420_16x16_2f.ivf").to_vec();

        let cases: [(&str, &str, Vec<u8>); 2] = [("avi_reuse", "avi", avi), ("ivf_reuse", "ivf", ivf)];
        for (tag, ext, bytes) in cases {
            let mut ref_src = open_video(bytes.clone()).unwrap();
            let mut expected = Vec::new();
            while let Some(f) = ref_src.next_frame().unwrap() {
                expected.push(f);
            }
            assert!(expected.len() >= 2, "{tag}");

            let path = write_temp_video(tag, ext, &bytes);
            {
                let mut src = open_video_file(&path).unwrap();
                let mut buf = Vec::new();
                assert!(src.next_frame_into(&mut buf).unwrap(), "{tag} frame 0");
                assert_eq!(buf, expected[0], "{tag} frame 0 bytes");
                let warm_ptr = buf.as_ptr();
                let warm_cap = buf.capacity();
                assert!(warm_cap >= expected[0].len());

                for (i, want) in expected.iter().enumerate().skip(1) {
                    assert!(src.next_frame_into(&mut buf).unwrap(), "{tag} frame {i}");
                    assert_eq!(buf, *want, "{tag} frame {i} bytes");
                    assert_eq!(buf.as_ptr(), warm_ptr, "{tag} frame {i} reallocated");
                    assert_eq!(buf.capacity(), warm_cap, "{tag} frame {i} capacity changed");
                }
                assert!(!src.next_frame_into(&mut buf).unwrap(), "{tag} EOF");
                assert_eq!(buf.as_ptr(), warm_ptr, "{tag} EOF reallocated");
                assert_eq!(buf.capacity(), warm_cap, "{tag} EOF capacity changed");

                src.rewind().unwrap();
                assert!(src.next_frame_into(&mut buf).unwrap(), "{tag} after rewind");
                assert_eq!(buf, expected[0], "{tag} after rewind bytes");
                assert_eq!(buf.as_ptr(), warm_ptr, "{tag} rewind reallocated");
                assert_eq!(buf.capacity(), warm_cap, "{tag} rewind capacity changed");
            }
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
    }
}
