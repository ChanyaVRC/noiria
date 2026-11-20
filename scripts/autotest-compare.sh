#!/usr/bin/env bash
# autotest-compare.sh — ヘッドレス autotest の決定論・回帰をフレーム単位で検証する。
#
# 「N 回実行して全フレームがバイト一致するか（= 決定論）」と「V8 と tsint で一致するか」を
# 1 コマンドで確認する。このセッションで何度も手書きした md5sum/cmp ループの固定化で、
# `cargo run --engine ...`（cargo に渡ると誤る）のような引数ミスも防ぐ。
#
# 使い方:
#   scripts/autotest-compare.sh [-g GAME] [-d SECONDS] [-n RUNS] [-e ENGINES] [-S SCRIPT] [-f]
#     -g GAME      ゲームパス or .pak（既定: games/sample）
#     -d SECONDS   --autotest の秒数（既定: 24）
#     -n RUNS      各エンジンの実行回数（既定: 3）
#     -e ENGINES   カンマ区切り。v8,tsint（既定）/ v8 / tsint
#     -S SCRIPT    --autotest-script のファイル（指定時は -d は仮想時間の上限にしか使わない）。
#                  実行ごとに空の --save-dir を使う（実ユーザーのセーブを汚さない）
#     -f           --autotest-fast（実時間スリープを省く。-S と併用）
#   例: scripts/autotest-compare.sh -n 6 -e v8
#       scripts/autotest-compare.sh -g target/sample.pak -e tsint
#
# 終了コード: 全エンジンが各自 K 回で完全一致なら 0、揺れがあれば 1。
# エンジン間差異は「参考情報」として表示するが終了コードには含めない
# （V8/tsint はエンジン意味論差でフレームが確定的に異なりうるため。単独決定論こそが要件）。
set -u

GAME="games/sample"
SECS=24
RUNS=3
ENGINES="v8,tsint"
SCRIPT=""
SECS_SET=0
FAST=0
while getopts "g:d:n:e:S:f" opt; do
  case "$opt" in
    g) GAME="$OPTARG" ;;
    d) SECS="$OPTARG"; SECS_SET=1 ;;
    n) RUNS="$OPTARG" ;;
    e) ENGINES="$OPTARG" ;;
    S) SCRIPT="$OPTARG" ;;
    f) FAST=1 ;;
    *) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
  esac
done

OUT_ROOT="target/autotest-compare"
rm -rf "$OUT_ROOT"; mkdir -p "$OUT_ROOT"
fail=0

# 1 フレームずつの md5 を連結した「集約ハッシュ」。run 間で同一なら全フレーム一致。
agg_hash() { md5sum "$1"/*.png 2>/dev/null | awk '{print $1}' | sort | md5sum | awk '{print $1}'; }

for eng in ${ENGINES//,/ }; do
  echo "=== engine=$eng: $RUNS 回実行 ($SECS 秒, $GAME) ==="
  first_dir=""
  base_hash=""
  eng_ok=1
  for i in $(seq 1 "$RUNS"); do
    dir="$OUT_ROOT/$eng-$i"
    # 注意: --engine は cargo ではなくアプリへ渡す（`--` の後ろ）
    extra=()
    if [ -n "$SCRIPT" ]; then
      rm -rf "$OUT_ROOT/save-$eng-$i"
      extra=(--autotest-script "$SCRIPT" --save-dir "$OUT_ROOT/save-$eng-$i")
      [ "$FAST" = 1 ] && extra+=(--autotest-fast)
    fi
    if [ -z "$SCRIPT" ] || [ "$SECS_SET" = 1 ]; then extra+=(--autotest "$SECS"); fi
    cargo run -q -p noiria-app -- "$GAME" --engine "$eng" --autotest-out "$dir" "${extra[@]}" >/dev/null 2>&1
    h=$(agg_hash "$dir")
    if [ -z "$base_hash" ]; then base_hash="$h"; first_dir="$dir"; echo "  run$i: $h (基準)"; else
      if [ "$h" = "$base_hash" ]; then echo "  run$i: $h  一致"; else
        echo "  run$i: $h  不一致:"; eng_ok=0
        for f in "$first_dir"/*.png; do n=$(basename "$f"); cmp -s "$f" "$dir/$n" || echo "    DIFF: $n"; done
      fi
    fi
  done
  if [ "$eng_ok" = 1 ]; then echo "  => $eng は完全決定論（$RUNS/$RUNS 一致）"; else echo "  => $eng に揺れあり"; fail=1; fi
done

# エンジン間比較（両方指定時のみ、参考情報）
if echo "$ENGINES" | grep -q v8 && echo "$ENGINES" | grep -q tsint; then
  echo "=== 参考: v8 vs tsint (run1 同士) ==="
  diffs=0
  for f in "$OUT_ROOT/v8-1"/*.png; do
    n=$(basename "$f")
    if ! cmp -s "$f" "$OUT_ROOT/tsint-1/$n"; then
      echo "  DIFF: $n ($(stat -c%s "$f") vs $(stat -c%s "$OUT_ROOT/tsint-1/$n") bytes)"; diffs=$((diffs+1))
    fi
  done
  [ "$diffs" = 0 ] && echo "  全フレーム一致" || echo "  $diffs フレーム相違（エンジン意味論差の可能性、単独決定論とは別）"
fi

exit $fail
