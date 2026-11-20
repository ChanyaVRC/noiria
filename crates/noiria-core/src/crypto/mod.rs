//! 自作暗号プリミティブ。外部非依存化ロードマップの一環（`codec/deflate.rs` 等と同格）。
//!
//! 現状は ChaCha20 (RFC 8439) のみ。W6 の暗号化 pak (`pak.rs`) 専用に導入した。
//! **Poly1305 (MAC) は実装しない**（`pak.rs` のモジュール doc 参照: 脅威モデルは
//! カジュアルな抜き出しの抑止であり、改ざん耐性は各エントリの CRC32 が代替する）。

mod chacha20;

pub use chacha20::{apply_keystream, block};
