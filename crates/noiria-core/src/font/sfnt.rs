//! sfnt コンテナ（単体 TTF/OTF）と TTC（TrueType Collection）のヘッダ解析。
//!
//! 役割は「フォントデータ全体からテーブルディレクトリ（各テーブルの
//! オフセットと長さ）を取り出す」ことだけ。個々のテーブルの中身は
//! `tables.rs` / `cmap.rs` / `glyf.rs` が解釈する。

use std::collections::HashMap;

use anyhow::{bail, ensure, Result};

use super::reader::Reader;

const TAG_TRUE_TYPE: u32 = 0x0001_0000; // sfnt version 1.0
const TAG_TRUETYPE_MAC: u32 = 0x7472_7565; // 'true'
const TAG_TTC: u32 = 0x7474_6366; // 'ttcf'
const TAG_OTTO: u32 = 0x4F54_544F; // 'OTTO' (CFF アウトライン)

pub(crate) struct TableDirectory {
    pub tables: HashMap<[u8; 4], (u32, u32)>, // tag -> (offset, length)
}

impl TableDirectory {
    pub fn find(&self, tag: &[u8; 4]) -> Option<(u32, u32)> {
        self.tables.get(tag).copied()
    }

    pub fn require<'a>(&self, data: &'a [u8], tag: &[u8; 4]) -> Result<&'a [u8]> {
        let (off, len) = self.find(tag).ok_or_else(|| {
            anyhow::anyhow!(
                "必須テーブル '{}' がありません",
                String::from_utf8_lossy(tag)
            )
        })?;
        table_slice(data, off, len)
    }

    pub fn find_slice<'a>(&self, data: &'a [u8], tag: &[u8; 4]) -> Result<Option<&'a [u8]>> {
        match self.find(tag) {
            Some((off, len)) => Ok(Some(table_slice(data, off, len)?)),
            None => Ok(None),
        }
    }
}

pub(crate) fn table_slice(data: &[u8], offset: u32, length: u32) -> Result<&[u8]> {
    let start = offset as usize;
    let end = start
        .checked_add(length as usize)
        .ok_or_else(|| anyhow::anyhow!("テーブル長の計算がオーバーフローしました"))?;
    ensure!(end <= data.len(), "テーブルがファイル範囲外を指しています");
    Ok(&data[start..end])
}

/// `data` から `index` 番目のフォント（TTC でなければ 0 のみ有効）の
/// テーブルディレクトリを取り出す。
pub(crate) fn parse_table_directory(data: &[u8], index: u32) -> Result<TableDirectory> {
    ensure!(data.len() >= 4, "ファイルが短すぎます");
    let mut r = Reader::new(data);
    let tag = r.u32()?;

    let sfnt_offset: usize = if tag == TAG_TTC {
        // TTC ヘッダ: Tag(4) majorVersion(2) minorVersion(2) numFonts(4) offsetTable[numFonts](4 each) ...
        let _major = r.u16()?;
        let _minor = r.u16()?;
        let num_fonts = r.u32()?;
        ensure!(
            index < num_fonts,
            "フォントインデックス {index} が範囲外です（コレクション内 {num_fonts} 個）"
        );
        r.skip((index as usize) * 4)?;
        r.u32()? as usize
    } else {
        ensure!(
            index == 0,
            "単体フォントファイルでインデックス {index} は指定できません"
        );
        0
    };

    ensure!(
        sfnt_offset <= data.len(),
        "sfnt テーブルのオフセットが範囲外です"
    );
    let mut sr = Reader::at(data, sfnt_offset)?;
    let sfnt_version = sr.u32()?;

    if sfnt_version == TAG_OTTO {
        bail!("OpenType/CFF ('OTTO') は非対応です（glyf 系アウトラインのみ対応）");
    }
    ensure!(
        sfnt_version == TAG_TRUE_TYPE || sfnt_version == TAG_TRUETYPE_MAC,
        "未知の sfnt バージョンです: 0x{sfnt_version:08X}"
    );

    let num_tables = sr.u16()?;
    let _search_range = sr.u16()?;
    let _entry_selector = sr.u16()?;
    let _range_shift = sr.u16()?;

    let mut tables = HashMap::with_capacity(num_tables as usize);
    for _ in 0..num_tables {
        let tag = sr.tag()?;
        let _checksum = sr.u32()?;
        let offset = sr.u32()?;
        let length = sr.u32()?;
        // オフセット/長さの妥当性はテーブル参照時に table_slice でチェックする
        // （壊れたレコードがあっても、そのテーブルを実際に使うまではエラーにしない）。
        tables.insert(tag, (offset, length));
    }

    Ok(TableDirectory { tables })
}
