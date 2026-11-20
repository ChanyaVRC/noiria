//! Residue デコード (仕様書 8章)。type 0/1/2。

use super::bitreader::BitReader;
use super::codebook::Codebook;
use anyhow::{ensure, Result};

pub struct ResidueSetup {
    pub residue_type: u16,
    pub begin: u32,
    pub end: u32,
    pub partition_size: u32,
    pub classifications: u32,
    pub classbook: u32,
    /// [classification][pass] -> book index (None なら未使用)。
    pub books: Vec<[Option<u32>; 8]>,
}

impl ResidueSetup {
    /// classbook と各パスのコードブック番号が `num_codebooks` 未満であることを確かめる
    /// （setup で弾かないとデコード時に範囲外添字で panic する）。
    pub fn validate_books(&self, num_codebooks: usize) -> Result<()> {
        ensure!(
            (self.classbook as usize) < num_codebooks,
            "residue の classbook {} がコードブック数 {num_codebooks} の範囲外です",
            self.classbook
        );
        for book in self.books.iter().flatten().flatten() {
            ensure!(
                (*book as usize) < num_codebooks,
                "residue の book {book} がコードブック数 {num_codebooks} の範囲外です"
            );
        }
        Ok(())
    }

    pub fn parse(br: &mut BitReader, residue_type: u16) -> Result<Self> {
        let begin = br.read(24);
        let end = br.read(24);
        let partition_size = br.read(24) + 1;
        let classifications = br.read(6) + 1;
        let classbook = br.read(8);

        let mut cascade = vec![0u32; classifications as usize];
        for c in cascade.iter_mut() {
            let low_bits = br.read(3);
            let mut high_bits = 0u32;
            if br.read_bool() {
                high_bits = br.read(5);
            }
            *c = high_bits * 8 + low_bits;
        }

        let mut books = vec![[None; 8]; classifications as usize];
        for (i, c) in cascade.iter().enumerate() {
            for j in 0..8u32 {
                if c & (1 << j) != 0 {
                    books[i][j as usize] = Some(br.read(8));
                }
            }
        }
        ensure!(
            !br.eof,
            "residue setup 読み取り中にパケット終端に達しました"
        );

        Ok(Self {
            residue_type,
            begin,
            end,
            partition_size,
            classifications,
            classbook,
            books,
        })
    }

    /// パケットデコード (8.6.2〜8.6.5)。`ch` 個のベクトル (各長 `n`) を返す。
    /// `do_not_decode[j]` が true のチャンネルはゼロベクトルのまま返す。
    pub fn decode(
        &self,
        br: &mut BitReader,
        codebooks: &[Codebook],
        ch: usize,
        n: usize,
        do_not_decode: &[bool],
    ) -> Result<Vec<Vec<f32>>> {
        match self.residue_type {
            0 | 1 => self.decode_core(br, codebooks, self.residue_type, ch, n, do_not_decode),
            2 => self.decode_type2(br, codebooks, ch, n, do_not_decode),
            t => unreachable!("非対応の residue type: {t}"),
        }
    }

    /// residue type 0/1 共通のコアデコード (8.6.2)。`format` は 0 か 1。
    fn decode_core(
        &self,
        br: &mut BitReader,
        codebooks: &[Codebook],
        format: u16,
        ch: usize,
        n: usize,
        do_not_decode: &[bool],
    ) -> Result<Vec<Vec<f32>>> {
        let mut out = vec![vec![0f32; n]; ch];
        let actual_size = n as u32;
        let limit_begin = self.begin.min(actual_size);
        let limit_end = self.end.min(actual_size);
        if limit_end <= limit_begin {
            return Ok(out);
        }
        let n_to_read = limit_end - limit_begin;
        let partitions_to_read = n_to_read / self.partition_size;
        if partitions_to_read == 0 {
            return Ok(out);
        }

        let classbook = &codebooks[self.classbook as usize];
        let classwords_per_codeword = classbook.dimensions;
        ensure!(
            classwords_per_codeword > 0,
            "residue classbook の次元が 0 です"
        );

        // classifications[j][partition]
        let mut classifications = vec![vec![0u32; partitions_to_read as usize]; ch];

        for pass in 0..8u32 {
            let mut partition_count = 0u32;
            while partition_count < partitions_to_read {
                if pass == 0 {
                    for j in 0..ch {
                        if !do_not_decode[j] {
                            let mut temp = classbook.decode_scalar(br)?;
                            for i in (0..classwords_per_codeword).rev() {
                                let pc = partition_count + i;
                                if pc < partitions_to_read {
                                    classifications[j][pc as usize] = temp % self.classifications;
                                }
                                temp /= self.classifications;
                            }
                        }
                    }
                    if br.eof {
                        return Ok(out);
                    }
                }
                let mut i = 0u32;
                while i < classwords_per_codeword && partition_count < partitions_to_read {
                    for j in 0..ch {
                        if !do_not_decode[j] {
                            let vqclass = classifications[j][partition_count as usize] as usize;
                            if let Some(vqbook) = self.books[vqclass][pass as usize] {
                                let offset =
                                    (limit_begin + partition_count * self.partition_size) as usize;
                                let book = &codebooks[vqbook as usize];
                                decode_partition(
                                    format,
                                    br,
                                    book,
                                    &mut out[j],
                                    offset,
                                    self.partition_size as usize,
                                )?;
                                if br.eof {
                                    return Ok(out);
                                }
                            }
                        }
                    }
                    partition_count += 1;
                    i += 1;
                }
            }
        }
        Ok(out)
    }

    /// residue type 2 (8.6.5): ch 本のベクトルをインターリーブして 1 本の長さ ch*n の
    /// ベクトルとして format 1 decode し、その後デインターリーブする。
    fn decode_type2(
        &self,
        br: &mut BitReader,
        codebooks: &[Codebook],
        ch: usize,
        n: usize,
        do_not_decode: &[bool],
    ) -> Result<Vec<Vec<f32>>> {
        if do_not_decode.iter().all(|&d| d) {
            return Ok(vec![vec![0f32; n]; ch]);
        }
        let interleaved_len = n * ch;
        let single_flag = [false];
        let single_vecs = self.decode_core(br, codebooks, 1, 1, interleaved_len, &single_flag)?;
        let single = &single_vecs[0];

        let mut out = vec![vec![0f32; n]; ch];
        for i in 0..n {
            for j in 0..ch {
                out[j][i] = single[i * ch + j];
            }
        }
        Ok(out)
    }
}

/// 1 パーティション分のデコード (8.6.3 format0 / 8.6.4 format1)。
/// `v` は対象チャンネルの出力ベクトル全体、`offset` はパーティション先頭位置。
fn decode_partition(
    format: u16,
    br: &mut BitReader,
    book: &Codebook,
    v: &mut [f32],
    offset: usize,
    partition_size: usize,
) -> Result<()> {
    let dims = book.dimensions as usize;
    ensure!(dims > 0, "residue codebook の次元が 0 です");
    if format == 0 {
        // format 0: インターリーブ配置
        let step = partition_size / dims;
        for i in 0..step {
            let entry = book.decode_vector(br)?;
            for (j, &val) in entry.iter().enumerate() {
                let idx = offset + i + j * step;
                if idx < v.len() {
                    v[idx] += val;
                }
            }
            if br.eof {
                return Ok(());
            }
        }
    } else {
        // format 1: 順序どおり配置
        let mut i = 0usize;
        while i < partition_size {
            let entry = book.decode_vector(br)?;
            for &val in entry {
                let idx = offset + i;
                if idx < v.len() {
                    v[idx] += val;
                }
                i += 1;
            }
            if br.eof {
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // residue のユニットテストは、実データ (setup ヘッダ由来のコードブック) が
    // 必要になるため、decoder.rs 側の統合テスト (クロスチェック) でまとめて検証する。
}
