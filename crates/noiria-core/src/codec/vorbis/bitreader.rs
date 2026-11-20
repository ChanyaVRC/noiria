//! Vorbis のビットパッキング規約 (仕様書 2章) 用ビットリーダ。
//! LSB ファースト: バイト内は bit0 から順に読み、複数ビット整数は最初に読んだビットが
//! 最下位ビットになる。inflate.rs の DEFLATE 用ビットリーダとは別実装（共用しない）。
//!
//! パケット終端を越えた読み出しは仕様上「エラーではなく 0 埋めで返す」ことが多くの
//! コンテキストで許容されているため、`read()` はエラーを返さず 0 埋めし、
//! `eof` フラグを立てる。呼び出し側が文脈に応じてエラー扱いにするか判断する。

pub struct BitReader<'a> {
    data: &'a [u8],
    /// 次に読む絶対ビット位置 (LSb 起点)。
    bitpos: usize,
    pub eof: bool,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bitpos: 0,
            eof: false,
        }
    }

    fn total_bits(&self) -> usize {
        self.data.len() * 8
    }

    /// 残りの読めるビット数。ファイル由来の個数・寸法が入力で表現できる量かの検査に使う。
    pub fn bits_left(&self) -> usize {
        self.total_bits().saturating_sub(self.bitpos)
    }

    /// 0..=32 ビットを符号なし整数として読む。範囲外は 0 埋め (eof セット)。
    pub fn read(&mut self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        let mut v: u32 = 0;
        for i in 0..n {
            let bit = if self.bitpos < self.total_bits() {
                let byte = self.data[self.bitpos / 8];
                (byte >> (self.bitpos % 8)) & 1
            } else {
                self.eof = true;
                0
            };
            v |= (bit as u32) << i;
            self.bitpos += 1;
        }
        v
    }

    /// 1 ビットをブール値として読む。
    pub fn read_bool(&mut self) -> bool {
        self.read(1) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_coding_example() {
        // 仕様書 2.1.6/2.1.7 の例: 4bit=12(b1100), 3bit=-1(b111), 7bit=17(b0010001)
        // をこの順で詰めると byte0=0b1111_1100, byte1=0b0000_1000 になる。
        // 続けて 2bit,2bit を読むと 'b00' と 'b11' が返る、という 2.1.7 の記述を検証。
        let data = [0b1111_1100u8, 0b0000_1000];
        let mut br = BitReader::new(&data);
        assert_eq!(br.read(2), 0b00);
        assert_eq!(br.read(2), 0b11);
    }

    #[test]
    fn read_various_widths() {
        // 12 = 1100, 追加で 3bit '111' (-1 相当 = 7 unsigned), 7bit '0010001' = 17
        let mut data = vec![0u8; 4];
        // 手動でビットを詰める (LSB ファースト)
        let mut bitpos = 0usize;
        let push = |val: u32, n: u32, data: &mut Vec<u8>, bitpos: &mut usize| {
            for i in 0..n {
                let bit = (val >> i) & 1;
                if bit != 0 {
                    data[*bitpos / 8] |= 1 << (*bitpos % 8);
                }
                *bitpos += 1;
            }
        };
        push(12, 4, &mut data, &mut bitpos);
        push(0b111, 3, &mut data, &mut bitpos);
        push(17, 7, &mut data, &mut bitpos);
        push(6969, 13, &mut data, &mut bitpos);

        let mut br = BitReader::new(&data);
        assert_eq!(br.read(4), 12);
        assert_eq!(br.read(3), 0b111);
        assert_eq!(br.read(7), 17);
        assert_eq!(br.read(13), 6969);
    }

    #[test]
    fn eof_returns_zero_padded() {
        let data = [0xFFu8];
        let mut br = BitReader::new(&data);
        assert_eq!(br.read(8), 0xFF);
        assert!(!br.eof);
        let v = br.read(8);
        assert_eq!(v, 0);
        assert!(br.eof);
    }

    #[test]
    fn zero_bit_read_returns_zero() {
        let data = [0xFFu8];
        let mut br = BitReader::new(&data);
        assert_eq!(br.read(0), 0);
        assert!(!br.eof);
    }
}
