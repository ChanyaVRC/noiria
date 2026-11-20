//! シナリオのラベル整合性テスト（games/ 配下の全ゲームを対象）。
//! 各ゲームの `scenario/*.ks` を走査し、以下を機械照合する（kag_tag_compat.rs と同じ
//! 「リポジトリのテキストを素朴に突き合わせる」方式。外部クレート不使用）:
//!
//! 1. `target=*xxx` を持つ全タグ参照（jump/call/link/timeout/click/wheel/rclick/setbgmstop 等、
//!    タグ名を問わず抽出）が、参照先ファイル（`storage=` 省略時は同一ファイル）に実在する
//!    ラベルを指すこと。
//! 2. `storage=xxx.ks` が実在するシナリオファイルを指すこと。
//! 3. 同一ファイル内でラベル名が重複していないこと。
//! 4. ファイル先頭ラベルの到達可能性（エントリポイントは各ゲームの main.ts の
//!    `run("xxx.ks")` から動的に特定する）。
//!
//! 抽出の限界: ブラケットタグ `[...]` のみを対象とし、`[iscript]`〜`[endscript]` 間の生コードと
//! `;` 始まりのコメント行は走査から除外する。属性値は空白/`]` 区切りの素朴な切り出しのため、
//! 引用符内に空白を含む `target=` は扱えない（ラベル名に空白は使わない前提）。
//! テストコード内の埋め込み断片（tag_behavior_tests.rs / mod.rs の js_tests）は対象外
//! （それら自身がテストとして実行され、ジャンプ後の挙動をアサートしているため機能的に検証済み）。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

fn games_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games")
}

/// games/ 配下の各ゲームについて (ゲーム名, エントリ .ks 名, {ファイル名 → 中身}) を返す。
fn load_games() -> Vec<(String, String, HashMap<String, String>)> {
    let mut games = Vec::new();
    for entry in fs::read_dir(games_dir()).expect("games/ を列挙できません") {
        let game_path = entry.unwrap().path();
        let scenario_dir = game_path.join("scenario");
        if !scenario_dir.is_dir() {
            continue;
        }
        let game_name = game_path.file_name().unwrap().to_string_lossy().to_string();

        // main.ts の run("xxx.ks") からエントリシナリオを特定する
        let main_ts = fs::read_to_string(game_path.join("main.ts"))
            .unwrap_or_else(|e| panic!("{game_name}/main.ts を読めません: {e}"));
        let entry_ks = main_ts
            .split("run(\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_else(|| panic!("{game_name}/main.ts に run(\"xxx.ks\") が見つかりません"))
            .to_string();

        let mut files = HashMap::new();
        for f in fs::read_dir(&scenario_dir).unwrap() {
            let path = f.unwrap().path();
            if path.extension().is_some_and(|e| e == "ks") {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                files.insert(name, fs::read_to_string(&path).unwrap());
            }
        }
        assert!(
            !files.is_empty(),
            "{game_name}: シナリオ .ks ファイルが 1 つもありません"
        );
        assert!(
            files.contains_key(&entry_ks),
            "{game_name}: エントリ {entry_ks} が scenario/ に存在しません"
        );
        games.push((game_name, entry_ks, files));
    }
    assert!(
        !games.is_empty(),
        "games/ 配下にゲームが 1 つも見つかりません"
    );
    games
}

/// ファイル内のラベル定義（`*名前` または `*名前|表示名`）を行順に返す。
fn parse_labels(src: &str) -> Vec<String> {
    let mut labels = Vec::new();
    for line in src.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix('*') {
            let name = rest.split('|').next().unwrap_or("").trim();
            if !name.is_empty() {
                labels.push(name.to_string());
            }
        }
    }
    labels
}

/// ブラケットタグ内の `storage=`/`target=` の組を (storage, target) として返す。
/// storage が無い場合は None（= 同一ファイル参照）。iscript ブロックとコメント行は除外。
fn parse_refs(src: &str) -> Vec<(Option<String>, Option<String>)> {
    let mut refs = Vec::new();
    let mut in_iscript = false;
    for line in src.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(';') {
            continue;
        }
        if in_iscript {
            if trimmed.starts_with("[endscript]") || trimmed.starts_with("@endscript") {
                in_iscript = false;
            }
            continue;
        }
        if trimmed.starts_with("[iscript]") || trimmed.starts_with("@iscript") {
            in_iscript = true;
            continue;
        }
        // 1 行に複数タグがありうるので `[...]` ごとに切り出す
        let mut rest = line;
        while let Some(open) = rest.find('[') {
            let after = &rest[open + 1..];
            // `[[` はエスケープされた表示用ブラケット
            if let Some(stripped) = after.strip_prefix('[') {
                rest = stripped;
                continue;
            }
            let Some(close) = after.find(']') else { break };
            let body = &after[..close];
            let storage = attr_value(body, "storage=");
            let target = attr_value(body, "target=");
            if target.is_some() || storage.as_deref().is_some_and(|s| s.ends_with(".ks")) {
                refs.push((storage, target));
            }
            rest = &after[close + 1..];
        }
    }
    refs
}

fn attr_value(body: &str, key: &str) -> Option<String> {
    let pos = body.find(key)?;
    let raw = body[pos + key.len()..]
        .split([' ', '\t'])
        .next()
        .unwrap_or("")
        .trim_matches('"');
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_string())
    }
}

#[test]
fn all_label_references_resolve() {
    for (game, _entry, files) in load_games() {
        let labels: HashMap<&String, HashSet<String>> = files
            .iter()
            .map(|(name, src)| (name, parse_labels(src).into_iter().collect()))
            .collect();

        let mut checked = 0usize;
        for (name, src) in &files {
            for (storage, target) in parse_refs(src) {
                // storage= が .ks を指す場合はファイルの実在を確認
                let target_file = match &storage {
                    Some(s) if s.ends_with(".ks") => {
                        assert!(
                            files.contains_key(s),
                            "{game}/{name}: storage={s} が指すシナリオファイルが存在しません"
                        );
                        s.clone()
                    }
                    _ => name.to_string(),
                };
                if let Some(t) = target {
                    let label = t.strip_prefix('*').unwrap_or(&t);
                    assert!(
                        labels[&target_file].contains(label),
                        "{game}/{name}: target={t} (storage={target_file}) が指すラベルが存在しません"
                    );
                    checked += 1;
                }
            }
        }
        // 参照が 1 件も抽出できていなければ走査自体が壊れている
        assert!(
            checked >= 3,
            "{game}: 抽出できたラベル参照が少なすぎます ({checked} 件) — 走査ロジックを確認"
        );
    }
}

#[test]
fn no_duplicate_labels_within_a_file() {
    for (game, _entry, files) in load_games() {
        for (name, src) in files {
            let labels = parse_labels(&src);
            let mut seen = HashSet::new();
            for l in labels {
                assert!(
                    seen.insert(l.clone()),
                    "{game}/{name}: ラベル *{l} が重複定義されています"
                );
            }
        }
    }
}

#[test]
fn every_label_is_reachable() {
    // 到達性 = ①どこかのファイルから target= で参照される、②直前ブロックからのフォールスルー
    // （ファイル先頭以外のラベルは常にフォールスルー候補）、③エントリポイント（main.ts の
    // run("xxx.ks") が指すファイルの先頭ラベル）。②があるため「参照されないラベル」は
    // 即エラーではなく、ここでは「ファイル先頭のラベルなのにどこからも参照されない」ものだけを
    // 検出する（先頭ラベルへはフォールスルーで到達できないため）。
    for (game, entry, files) in load_games() {
        let mut referenced: HashSet<(String, String)> = HashSet::new();
        for (name, src) in &files {
            for (storage, target) in parse_refs(src) {
                if let Some(t) = target {
                    let file = match &storage {
                        Some(s) if s.ends_with(".ks") => s.clone(),
                        _ => name.to_string(),
                    };
                    referenced.insert((file, t.trim_start_matches('*').to_string()));
                }
            }
        }
        for (name, src) in &files {
            if let Some(first) = parse_labels(src).first() {
                let is_entry = *name == entry;
                assert!(
                    is_entry || referenced.contains(&(name.clone(), first.clone())),
                    "{game}/{name}: 先頭ラベル *{first} がどこからも参照されていません（到達不能）"
                );
            }
        }
    }
}
