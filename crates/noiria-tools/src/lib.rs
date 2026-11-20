//! `gen-assets`（`src/main.rs`）から使う手続き生成ロジックの共有ライブラリ部分。
//! ライブラリとして切り出しているのは、`tests/` 配下の統合テストから
//! 音声レンダリング関数（`synth` モジュール）を直接呼び出し、実際に
//! WAV ファイルを書き出さずに数値検証（ピーク/RMS/相関/ループ境界連続性）
//! できるようにするため。

pub mod analysis;
pub mod demo;
pub mod export;
pub mod flac_enc;
pub mod pack;
pub mod paint;
pub mod synth;
pub mod testutil;
pub mod third_party;
pub mod wavout;
