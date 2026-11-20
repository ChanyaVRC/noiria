//! W7: 全数監査（最終）— `docs/kag-tag-compat.md` の対応表と `runtime/kag/tags.ts` の実装を
//! 機械照合する回帰テスト。表とコードが乖離したら `cargo test`（CI）で落ちることが目的。
//!
//! 正規表現や外部クレートには依存せず、素朴な文字列走査でテキストを解析する（プロジェクトの
//! 「外部依存は最小限」という自作方針に合わせる。W6 までの `docs/implementation-notes.md` にも
//! 前例が無い新規の判断のため、抽出方法の限界をここに明記する）。
//!
//! ## 抽出方法の限界（`docs/implementation-notes.md`「2026-07-11 W7」参照）
//!
//! - `runtime/kag/tags.ts` のハンドラ抽出は「行頭からちょうど半角スペース4個ぶんインデントされた
//!   識別子 + `:` + (`async` 修飾可) + `(`」という単一の記法パターンのみを認識する
//!   （`makeTags` 内の `const handlers: Record<string, TagHandler> = { ... }` はこの形式で
//!   統一されている）。将来、複数行にまたがるハンドラ定義など別の記法が混じると抽出漏れになり
//!   うるが、その場合は本テストが「対応表にあるのに tags.ts に見つからない」という形で fail する
//!   ため、乖離自体は検出できる（原因の切り分けにはテストコードの読解が必要になる）。
//! - `iscript` は上記パターンでは検出できない既知の例外: `runtime/kag/parser.ts` が
//!   `[iscript]`〜`[endscript]` を専用の `Command::iscript` として切り出し、
//!   `runtime/kag/player.ts` の `execCommand` が `case "iscript":` で直接処理する
//!   （`tags.ts` の `makeTags` ハンドラテーブルを経由しない、`docs/implementation-notes.md`
//!   「2026-07-11 W6: iscript の tsint 対応可否」参照）。そのため `iscript` だけは
//!   「`player.ts` に `case \"iscript\":` があること」を別途確認する専用チェックにしている。
//! - STUB_REASONS（➖ 対象外 10 種）は `const STUB_REASONS: Record<string, string> = { ... };`
//!   ブロック内の同じ 4 スペースインデントのキーを、値が文字列リテラル（`"` 始まり）であることで
//!   ハンドラ本体（`(` 始まり）と区別して抽出する。

use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    // crates/noiria-app/tests/kag_tag_compat.rs から見て、ワークスペースルートは 2 つ上。
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読み込めません: {e}", path.display()))
}

/// `docs/kag-tag-compat.md` の「## 対応表」〜「## ウェーブ割当」間にある
/// `| \`tag\` | status | ... |` 形式の行だけを (tag, status) として返す。
fn parse_compat_table(md: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut in_section = false;
    for line in md.lines() {
        if line.starts_with("## 対応表") {
            in_section = true;
            continue;
        }
        if line.starts_with("## ウェーブ割当") {
            break;
        }
        if !in_section {
            continue;
        }
        let line = line.trim();
        if !line.starts_with("| `") {
            continue; // ヘッダ行・区切り行 (`| --- | ... |`)・空行を除外
        }
        let cols: Vec<&str> = line.split('|').map(|c| c.trim()).collect();
        // cols[0] は先頭の `|` の前（空文字列）、cols[1] がタグ名、cols[2] が状態。
        if cols.len() < 3 {
            continue;
        }
        let tag = cols[1].trim_matches('`').to_string();
        let status = cols[2].to_string();
        rows.push((tag, status));
    }
    rows
}

/// 「## Noiria 独自拡張タグ」セクションの `| \`tag\` | 概要 |` 行からタグ名だけを返す。
fn parse_extension_tags(md: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut in_section = false;
    for line in md.lines() {
        if line.starts_with("## Noiria 独自拡張タグ") {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        let line = line.trim();
        if !line.starts_with("| `") {
            continue;
        }
        let cols: Vec<&str> = line.split('|').map(|c| c.trim()).collect();
        if cols.len() < 2 {
            continue;
        }
        tags.push(cols[1].trim_matches('`').to_string());
    }
    tags
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// tags.ts の `const handlers: Record<string, TagHandler> = { ... }` 直下（ちょうど 4 スペース
/// インデント）にある `name: (`／`name: async (` 形式のキーを抽出する。
fn parse_handler_names(ts: &str) -> Vec<String> {
    let mut names = Vec::new();
    for raw_line in ts.lines() {
        if !raw_line.starts_with("    ") || raw_line.starts_with("     ") {
            continue; // ちょうど 4 スペースインデントの行だけを対象にする
        }
        let trimmed = &raw_line[4..];
        let Some(colon) = trimmed.find(':') else {
            continue;
        };
        let key = trimmed[..colon].trim();
        if !is_ident(key) {
            continue;
        }
        let rest = trimmed[colon + 1..].trim_start();
        let rest = rest
            .strip_prefix("async")
            .map(|s| s.trim_start())
            .unwrap_or(rest);
        if rest.starts_with('(') {
            names.push(key.to_string());
        }
    }
    names
}

/// `const STUB_REASONS: Record<string, string> = { ... };` ブロック内のキー（値が文字列の
/// エントリ）だけを抽出する。
fn parse_stub_names(ts: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_block = false;
    for raw_line in ts.lines() {
        if raw_line.contains("const STUB_REASONS: Record<string, string> = {") {
            in_block = true;
            continue;
        }
        if !in_block {
            continue;
        }
        if raw_line.trim() == "};" {
            break;
        }
        if !raw_line.starts_with("    ") || raw_line.starts_with("     ") {
            continue;
        }
        let body = &raw_line[4..];
        let Some(colon) = body.find(':') else {
            continue;
        };
        let key = body[..colon].trim();
        if is_ident(key) {
            names.push(key.to_string());
        }
    }
    names
}

/// player.ts の execCommand に `case "iscript":` があることを確認する（iscript の既知の例外
/// 経路、上記モジュールコメント参照）。
fn player_ts_handles_iscript_directly(player_ts: &str) -> bool {
    player_ts.contains("case \"iscript\":")
}

#[test]
fn compat_table_has_exactly_161_rows_all_resolved() {
    let md = read("docs/kag-tag-compat.md");
    let rows = parse_compat_table(&md);
    assert_eq!(
        rows.len(),
        161,
        "対応表の行数が161ではありません: {}",
        rows.len()
    );

    for (tag, status) in &rows {
        assert!(
            status.contains('✅') || status.contains('➖'),
            "タグ `{tag}` の状態が ✅ でも ➖ でもありません（未確定行が残っています）: {status:?}"
        );
    }

    let ok_count = rows.iter().filter(|(_, s)| s.contains('✅')).count();
    let taigai_count = rows.iter().filter(|(_, s)| s.contains('➖')).count();
    assert_eq!(ok_count, 151, "✅ 行数が151ではありません: {ok_count}");
    assert_eq!(
        taigai_count, 10,
        "➖ 行数が10ではありません: {taigai_count}"
    );
    assert_eq!(ok_count + taigai_count, 161);
}

#[test]
fn every_compat_table_tag_is_registered_in_tags_ts() {
    let md = read("docs/kag-tag-compat.md");
    let ts = read("runtime/kag/tags.ts");
    let player_ts = read("runtime/kag/player.ts");

    let rows = parse_compat_table(&md);
    assert!(
        !rows.is_empty(),
        "対応表の行が1件も抽出できませんでした（パーサ側の不具合の疑い）"
    );
    let handler_names = parse_handler_names(&ts);
    let stub_names = parse_stub_names(&ts);
    assert!(
        !handler_names.is_empty() && !stub_names.is_empty(),
        "tags.ts の抽出結果が空です"
    );

    // iscript は tags.ts のハンドラテーブルを経由しない既知の例外（モジュール冒頭コメント参照）。
    const ISCRIPT_EXCEPTION: &str = "iscript";

    for (tag, status) in &rows {
        if status.contains('✅') {
            if tag == ISCRIPT_EXCEPTION {
                assert!(
                    player_ts_handles_iscript_directly(&player_ts),
                    "iscript は player.ts の execCommand 内 `case \"iscript\":` で処理されるはずですが見つかりません"
                );
                continue;
            }
            assert!(
                handler_names.iter().any(|n| n == tag),
                "✅ タグ `{tag}` が runtime/kag/tags.ts の makeTags ハンドラに見つかりません"
            );
        } else if status.contains('➖') {
            assert!(
                stub_names.iter().any(|n| n == tag),
                "➖ タグ `{tag}` が runtime/kag/tags.ts の STUB_REASONS に見つかりません"
            );
        }
    }
}

#[test]
fn tags_ts_has_no_undocumented_kag3_tag_handlers() {
    // 逆方向: makeTags に登録されているのに対応表に無い KAG3 タグが無いことを確認する。
    // Noiria 独自拡張タグ（対応表末尾の「Noiria 独自拡張タグ」セクションから動的に読み取る。
    // ハードコードしないことで、拡張タグが増減してもこのテスト自体は追随する）は許容リストとして除外。
    let md = read("docs/kag-tag-compat.md");
    let ts = read("runtime/kag/tags.ts");

    let rows = parse_compat_table(&md);
    let ok_tags: Vec<String> = rows
        .iter()
        .filter(|(_, s)| s.contains('✅'))
        .map(|(t, _)| t.clone())
        .collect();
    let extension_tags = parse_extension_tags(&md);
    assert!(
        !extension_tags.is_empty(),
        "Noiria 独自拡張タグのセクションが抽出できませんでした"
    );

    let handler_names = parse_handler_names(&ts);
    let mut undocumented = Vec::new();
    for name in &handler_names {
        let documented =
            ok_tags.iter().any(|t| t == name) || extension_tags.iter().any(|t| t == name);
        if !documented {
            undocumented.push(name.clone());
        }
    }
    assert!(
        undocumented.is_empty(),
        "tags.ts に登録されているが対応表（✅ タグ or Noiria 独自拡張）に無いハンドラがあります: {undocumented:?}"
    );
}

#[test]
fn stub_reasons_exactly_match_taigai_rows() {
    let md = read("docs/kag-tag-compat.md");
    let ts = read("runtime/kag/tags.ts");

    let rows = parse_compat_table(&md);
    let mut taigai_tags: Vec<String> = rows
        .iter()
        .filter(|(_, s)| s.contains('➖'))
        .map(|(t, _)| t.clone())
        .collect();
    taigai_tags.sort();

    let mut stub_names = parse_stub_names(&ts);
    stub_names.sort();

    assert_eq!(
        taigai_tags, stub_names,
        "runtime/kag/tags.ts の STUB_REASONS のキー集合が対応表の➖行と一致しません"
    );
}
