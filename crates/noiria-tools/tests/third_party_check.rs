//! `crates/noiria-tools/THIRD-PARTY.txt` の陳腐化検出テスト（W7）。
//! `cargo metadata` で配布ビルド（`noiria-app --no-default-features --features
//! engine-tsint,dist`）の実際の依存グラフ（Windows/Linux 両ターゲットの和集合、
//! normal + build 依存のみ、dev-dependencies は対象外）を取得し、crates.io 由来の
//! パッケージ（このワークスペース内の path 依存 noiria-core/tsint/vp9dec は対象外）が
//! 全て THIRD-PARTY.txt 中にクレート名として出現することを確認する。新しい依存が
//! 追加されたのに THIRD-PARTY.txt の更新を忘れた場合にここで検出できる。
//!
//! `cargo metadata` のサブプロセス実行が必要なため、CI 等ネットワーク遮断環境では
//! 失敗しうる（初回のみ、依存はすでに `Cargo.lock` に解決済みなのでネットワークは
//! 通常不要）。実行に数秒かかる。

use std::collections::BTreeSet;
use std::process::Command;

use serde_json::Value;

const THIRD_PARTY_TXT: &str = include_str!("../THIRD-PARTY.txt");

fn dependency_closure(filter_platform: &str) -> BTreeSet<(String, String)> {
    let manifest_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../noiria-app/Cargo.toml")
        .replace('/', std::path::MAIN_SEPARATOR_STR);

    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--no-default-features",
            "--features",
            "engine-tsint,dist",
            "--manifest-path",
            &manifest_path,
            "--format-version=1",
            "--filter-platform",
            filter_platform,
        ])
        .output()
        .expect("cargo metadata の実行に失敗しました");
    assert!(
        output.status.success(),
        "cargo metadata が失敗しました: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let meta: Value = serde_json::from_slice(&output.stdout)
        .expect("cargo metadata の出力が JSON として不正です");
    let packages = meta["packages"]
        .as_array()
        .expect("packages 配列がありません");
    let pkg_by_id: std::collections::HashMap<&str, &Value> = packages
        .iter()
        .map(|p| (p["id"].as_str().unwrap(), p))
        .collect();

    let resolve = &meta["resolve"];
    let nodes = resolve["nodes"]
        .as_array()
        .expect("resolve.nodes がありません");
    let node_by_id: std::collections::HashMap<&str, &Value> = nodes
        .iter()
        .map(|n| (n["id"].as_str().unwrap(), n))
        .collect();
    let root = resolve["root"]
        .as_str()
        .expect("resolve.root がありません（ワークスペースのルート指定が必要）");

    let mut visited: BTreeSet<&str> = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(cur) = stack.pop() {
        if !visited.insert(cur) {
            continue;
        }
        let Some(node) = node_by_id.get(cur) else {
            continue;
        };
        let Some(deps) = node["deps"].as_array() else {
            continue;
        };
        for dep in deps {
            let kinds = dep["dep_kinds"].as_array().cloned().unwrap_or_default();
            // normal (kind: null) または build 依存のみ辿る（dev-dependencies は除外）。
            let keep = kinds.is_empty()
                || kinds
                    .iter()
                    .any(|k| matches!(k["kind"].as_str(), None | Some("build")));
            if keep {
                if let Some(pkg_id) = dep["pkg"].as_str() {
                    stack.push(pkg_id);
                }
            }
        }
    }

    let mut out = BTreeSet::new();
    for id in visited {
        let Some(pkg) = pkg_by_id.get(id) else {
            continue;
        };
        // path 依存（source が null）はワークスペース内の自作クレートなので対象外。
        if pkg["source"].is_null() {
            continue;
        }
        let name = pkg["name"].as_str().unwrap_or_default().to_string();
        let version = pkg["version"].as_str().unwrap_or_default().to_string();
        out.insert((name, version));
    }
    out
}

#[test]
fn third_party_txt_lists_every_dist_build_dependency() {
    let mut all: BTreeSet<(String, String)> = BTreeSet::new();
    all.extend(dependency_closure("x86_64-pc-windows-msvc"));
    all.extend(dependency_closure("x86_64-unknown-linux-gnu"));

    assert!(
        all.len() > 10,
        "依存クレート数が異常に少ないです（cargo metadata の解析ロジックを疑う）: {all:?}"
    );

    let mut missing = Vec::new();
    for (name, version) in &all {
        // クレート名が単語境界で THIRD-PARTY.txt に出現するか（前方一致の誤検出を避けるため
        // 名前の直後に英数字/`-`/`_` が続かないことも確認する）。
        let mut found = false;
        let bytes = THIRD_PARTY_TXT.as_bytes();
        let needle = name.as_bytes();
        let mut start = 0;
        while let Some(rel) = THIRD_PARTY_TXT[start..].find(name.as_str()) {
            let idx = start + rel;
            let before_ok = idx == 0
                || !THIRD_PARTY_TXT.as_bytes()[idx - 1].is_ascii_alphanumeric()
                    && THIRD_PARTY_TXT.as_bytes()[idx - 1] != b'-'
                    && THIRD_PARTY_TXT.as_bytes()[idx - 1] != b'_';
            let after = idx + needle.len();
            let after_ok = after >= bytes.len()
                || !(bytes[after].is_ascii_alphanumeric()
                    || bytes[after] == b'-'
                    || bytes[after] == b'_');
            if before_ok && after_ok {
                found = true;
                break;
            }
            start = idx + 1;
        }
        if !found {
            missing.push(format!("{name} {version}"));
        }
    }

    assert!(
        missing.is_empty(),
        "THIRD-PARTY.txt に記載が無い依存クレートがあります（追加/更新してください）: {missing:#?}"
    );
}
