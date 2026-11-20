---
name: verify-noiria
description: Noiria の実装ウェーブを検収（受け入れ確認）するときのチェックリスト。委任先エージェントの完了報告を検収する、または自分の変更を確定する前に使う。ヘッドレス検証（cargo test 両OS・autotest フレーム比較・PNG 目視）の手順と、報告数字を鵜呑みにしない照合方法を定める。
---

# Noiria 検収チェックリスト

委任先の完了報告や自分の変更を**成果物ベースで**確認する。報告された数字・主張は鵜呑みにせず、自分で再実行する。窓・音は出さない（[[no-gui-launch]]）。

## 1. テスト（両 OS）

- **Windows**: `cargo test`（ワークスペース）。合計を集計:
  `cargo test 2>&1 | grep -E "^test result" | awk -F'[ ;]+' '{s+=$4; f+=$6} END {print "passed:", s, "failed:", f}'`
- **WSL**（Linux 対応が絡むなら必須）:
  `wsl -e bash -lc "cd /mnt/c/Users/alice/Documents/BuildSoft/noiria && CARGO_TARGET_DIR=\$HOME/.noiria-target cargo test --workspace 2>&1 | grep -E '^test result' | ..."`
- **tsint コーパス**（`runtime/` を触ったら）: `cd ../tsint && cargo test`（読み取り専用リポジトリ）。
- `cargo clippy` の変更分に新規 warning が無いこと。

## 2. テスト数の食い違いを潰す

報告値と実測がずれたら（このリポジトリで何度も起きた）、**推測で片付けない**:
- `cargo test -p <crate> --bin <bin> -- --list` の差分を両 OS で取り、消えた/増えたテスト関数を特定する。
- プラットフォーム差は cfg で説明できる: Windows 専用 = win32/wasapi/mf 系、Unix 専用 = x11/alsa 系。差分がこれらだけなら退行ではない。
- `#[ignore]` はフィクスチャ再生成ヘルパ等の意図的なものか確認。

## 3. E2E フレーム比較

- 決定論・回帰は `scripts/autotest-compare.sh` を使う（両エンジン K 回 → 差分フレーム + 集約ハッシュ）。
  - 挙動不変が受け入れ基準のとき: 変更前後で PNG ハッシュ一致を確認（変更前ベースラインを先に取得）。
  - dist ビルドや pak 起動を含むとき: `dir 起動 vs pak 起動`、`dev vs dist` の一致も見る。
- **V8 と tsint のエンジン間でも全フレームのバイト完全一致を確認する**。B2 で
  `frame_03_hover.png` の既知差異は解消済みであり、比較から除外するフレームはない。

## 4. 目視

- 新機能が写るフレーム PNG を Read で開いて目視する（装飾・ルビ・レイヤ・アイコン・トランジション等）。
- 見た目・音の官能品質はユーザーが最終ゲート（[[sensory-quality-workflow]]）。数値が通ってもユーザー指摘を優先。実機の窓・音はユーザーが確認する。

## 5. ドキュメント

- `docs/implementation-notes.md` に当該ウェーブの節（判断→理由→影響）が追記されているか。
- 近似・制限は対応表（`docs/kag-tag-compat.md`）と notes の両方に書かれているか。
- 対応表を触ったら `crates/noiria-app/tests/kag_tag_compat.rs` の機械照合が通ること。

## 差し戻し基準

上記いずれかが未達、または報告と実測が食い違う場合は、原因を特定して委任先へ差し戻すか自分で修正する。「0 failed だが数が減った」も放置しない（必ず --list で説明をつける）。
