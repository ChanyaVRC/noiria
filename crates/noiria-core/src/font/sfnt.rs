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

/// ランダムアクセス読込（`read_at`）から SFNT ヘッダとテーブルディレクトリを読む。
/// `Font::from_file` と `FontRegistry::index_file` の共通実装。
pub(crate) fn read_sfnt_directory_at(
    read_at: &mut impl FnMut(u64, &mut [u8]) -> std::io::Result<()>,
    file_len: u64,
    sfnt_offset: u64,
    dir_buf: &mut Vec<u8>,
) -> Result<TableDirectory> {
    ensure!(
        sfnt_offset.checked_add(12).is_some_and(|e| e <= file_len),
        "sfnt テーブルのオフセットが範囲外です"
    );
    let mut sh = [0u8; 12];
    read_at(sfnt_offset, &mut sh)?;
    let sfnt_version = u32::from_be_bytes([sh[0], sh[1], sh[2], sh[3]]);
    if sfnt_version == TAG_OTTO {
        bail!("OpenType/CFF ('OTTO') は非対応です（glyf 系アウトラインのみ対応）");
    }
    ensure!(
        sfnt_version == TAG_TRUE_TYPE || sfnt_version == TAG_TRUETYPE_MAC,
        "未知の sfnt バージョンです: 0x{sfnt_version:08X}"
    );
    let num_tables = u16::from_be_bytes([sh[4], sh[5]]) as usize;
    let rec_len = num_tables * 16;
    ensure!(
        sfnt_offset
            .checked_add(12 + rec_len as u64)
            .is_some_and(|e| e <= file_len),
        "テーブルがファイル範囲外を指しています"
    );
    dir_buf.resize(rec_len, 0);
    read_at(sfnt_offset + 12, dir_buf)?;
    let mut tables = HashMap::with_capacity(num_tables);
    for rec in dir_buf.as_chunks::<16>().0 {
        let tag = [rec[0], rec[1], rec[2], rec[3]];
        let off = u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]);
        let len = u32::from_be_bytes([rec[12], rec[13], rec[14], rec[15]]);
        tables.insert(tag, (off, len));
    }
    Ok(TableDirectory { tables })
}
