#!/usr/bin/env bash
# WSL2 (Linux) 側で配布用 (dist プロファイル、tsint 専用) バイナリをビルドし、
# `noiria-export` の既定探索パス（Windows 側から見て `target/dist/noiria`）へ
# コピーする。docs/distribution.md の手順の一部。
#
# 使い方（WSL シェル内、または `wsl -e bash -lc "..."`）:
#   bash scripts/build-dist-wsl.sh
#
# 注意: `/mnt/c` 上のスクリプトをシェバン経由で直接実行すると環境によっては
# 失敗することがある（docs/linux-dev.md 参照）。**必ず `bash
# scripts/build-dist-wsl.sh` のように bash を明示して呼び出すこと。**
#
# ビルド成果物自体は $CARGO_TARGET_DIR（既定 $HOME/.noiria-target、WSL 内の
# ext4）に置く（9P 経由の /mnt/c 上でビルドすると著しく遅いため、
# scripts/wsl-test.sh と同じ回避策）。最終バイナリだけを /mnt/c 側の
# `target/dist/noiria` へコピーするので、Windows 側で動く `noiria-export` の
# 既定パス探索（`target/dist/noiria`）からそのまま拾える。
#
# export はこのバイナリをクロスコンパイルしない（KEY_SLOT バイナリパッチ方式
# なのでゲームごとの再ビルドは不要 — このスクリプトで 1 回ビルドしておけば、
# 以後は何本ゲームを export しても WSL 側の再ビルドは不要）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.noiria-target}"

cd "$REPO_ROOT"

echo "=== dist ビルド (engine-tsint のみ, CARGO_TARGET_DIR=$CARGO_TARGET_DIR) ==="
cargo build --profile dist --no-default-features --features engine-tsint,dist -p noiria-app

BUILT_BIN="$CARGO_TARGET_DIR/dist/noiria"
if [[ ! -f "$BUILT_BIN" ]]; then
  echo "エラー: ビルド後にバイナリが見つかりません: $BUILT_BIN" >&2
  exit 1
fi

OUT_DIR="$REPO_ROOT/target/dist"
mkdir -p "$OUT_DIR"
cp -f "$BUILT_BIN" "$OUT_DIR/noiria"
chmod +x "$OUT_DIR/noiria"

SIZE=$(stat -c%s "$OUT_DIR/noiria" 2>/dev/null || wc -c < "$OUT_DIR/noiria")
echo
echo "完了: $OUT_DIR/noiria （${SIZE} バイト）"
echo "Windows 側からは $REPO_ROOT/target/dist/noiria としてそのまま見える"
echo "（noiria-export --bin-linux を省略すればこのパスを既定探索する）。"
