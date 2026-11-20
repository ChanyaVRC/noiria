//! `THIRD-PARTY.txt`（配布ビルドの依存ライセンス表記）を静的に埋め込む。
//! `noiria-export` が配布物ディレクトリへそのままコピーする。実体の更新は
//! `crates/noiria-tools/THIRD-PARTY.txt` を直接編集する（`tests/third_party_check.rs`
//! が `cargo metadata` との突き合わせで陳腐化を検出する）。

pub const THIRD_PARTY_TXT: &str = include_str!("../THIRD-PARTY.txt");
