//! 境界チェック付きバイト列リーダー。
//!
//! TrueType/OpenType のテーブルは壊れている・切り詰められている可能性が
//! あるため、ここでの読み取りはすべて範囲外アクセス時に `Err` を返す
//! （決して panic しない・添字アクセスも `usize` オーバーフローも起こさない）。

use anyhow::{ensure, Result};

pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Result<Self> {
        ensure!(pos <= data.len(), "読み取り開始位置がデータ範囲外です");
        Ok(Self { data, pos })
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, pos: usize) -> Result<()> {
        ensure!(pos <= self.data.len(), "seek 先がデータ範囲を超えています");
        self.pos = pos;
        Ok(())
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        let p = self
            .pos
            .checked_add(n)
            .ok_or_else(|| anyhow::anyhow!("オフセット計算がオーバーフローしました"))?;
        self.seek(p)
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    /// `n` バイトのスライスを読み進める。
    pub fn slice(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| anyhow::anyhow!("長さ計算がオーバーフローしました"))?;
        ensure!(
            end <= self.data.len(),
            "データ末尾を超えて読み取ろうとしました"
        );
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.slice(1)?[0])
    }

    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    pub fn u16(&mut self) -> Result<u16> {
        let s = self.slice(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32> {
        let s = self.slice(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }

    /// F2Dot14 固定小数点 (符号付き 2.14) を f32 に変換する。
    pub fn f2dot14(&mut self) -> Result<f32> {
        Ok(self.i16()? as f32 / 16384.0)
    }

    pub fn tag(&mut self) -> Result<[u8; 4]> {
        let s = self.slice(4)?;
        Ok([s[0], s[1], s[2], s[3]])
    }
}

/// テーブル先頭からの固定オフセットで読む単発ヘルパー群
/// （シーケンシャルな `Reader` を作るまでもない固定レイアウトのテーブル用）。
pub(crate) fn u16_at(data: &[u8], off: usize) -> Result<u16> {
    Reader::at(data, off)?.u16()
}
