//! noiria-core — ノベルゲームエンジン Noiria のネイティブコア。
//!
//! krkrZ の「小さなネイティブコア + スクリプト製フレームワーク」構成に倣い、
//! ここにはウィンドウ/スクリプトエンジンに依存しない純粋なロジックだけを置く:
//! レイヤ合成・トランジション・テキストラスタ・アセット・オーディオ・セーブ。
//!
//! ## 外部依存の隔離方針（将来の完全自作化のための継ぎ目）
//! - 画像デコード (image)      → assets.rs のみ
//! - フォント/整形 → **自作済み・cosmic-text 依存を削除済み**。`font/` 配下の TTF/TTC
//!   パーサ + AA ラスタライザ + フォントレジストリを `text.rs` がレイアウト・禁則処理・
//!   フォールバックとともに統合している（詳細は font/mod.rs, text.rs 参照）
//! - 音声出力 (WASAPI/Windows, ALSA dlopen/Unix) → wasapi_out.rs / alsa_out.rs
//!   のみ（どちらも OS 境界 API。ALSA はリンク時依存なしの dlopen）
//! - 音声ミキシング/リサンプル      → mixer.rs（純 Rust・OS 非依存・自作済み）
//! - 音声デコード (WAV/OGG(Vorbis)) → **自作済み・外部依存なし**。`codec/wav.rs` +
//!   `codec/ogg.rs` + `codec/vorbis/` を audio.rs が `AudioCmd` 境界越しに使う
//!   （MP3/FLAC は対応打ち切り）
//! - 合成・トランジション・シーンは既に完全自作（依存なし）

#![warn(clippy::undocumented_unsafe_blocks)]

#[cfg(unix)]
pub mod alsa_out;
pub mod assets;
pub mod audio;
pub mod codec;
pub mod compose;
pub mod crypto;
pub mod font;
pub mod localtime;
pub mod mixer;
pub mod pak;
pub mod pixmap;
pub mod savedata;
pub mod scale;
pub mod scene;
pub mod text;
#[cfg(test)]
mod test_support;
pub mod transition;
pub mod uiraster;
#[cfg(windows)]
pub mod wasapi_out;
