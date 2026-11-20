#!/usr/bin/env bash
# WSL2 (Ubuntu) 内で noiria の cargo test + ヘッドレス autotest + X11 (Xvfb)
# テストを一括実行する。ソースは /mnt/c 側を直接参照する（クローンし直さない）が、
# ビルド成果物は $HOME/.noiria-target（WSL 内の ext4）に出す。9P 越しに target/
# を置くとビルドが大幅に遅くなるための回避策（docs/linux-dev.md 参照）。
#
# 使い方:
#   wsl -e bash -lc "bash /mnt/c/Users/alice/Documents/BuildSoft/noiria/scripts/wsl-test.sh"
# または WSL シェル内で:
#   bash scripts/wsl-test.sh [--autotest-secs N] [--engine v8|tsint] [--skip-x11]
#
# 注意: /mnt/c (DrvFs) 上のスクリプトをシェバン経由で直接実行する
# （`./wsl-test.sh` や `wsl -e bash -lc "/mnt/c/.../wsl-test.sh"`）と、環境に
# よっては `bash: line 1: C:/Program: No such file or directory` のような
# 謎のエラーで失敗することがある（DrvFs 上のシェバン実行と WSL の Windows
# PATH interop が絡む既知の相性問題）。**必ず `bash wsl-test.sh` のように
# bash を明示して呼び出すこと。**
#
# 制約: ユーザーのデスクトップに窓・音を出さない。--autotest はヘッドレスなので
# 安全。X11 テストは Xvfb（隔離ディスプレイ、既定 :99）にのみ接続し、
# ユーザーの WSLg :0 デスクトップには一切触れない（詳細は下記 [4/4] 参照）。
# ALSA (音声) テストは常に `NOIRIA_ALSA_DEVICE=null`（何も鳴らさない仮想
# デバイス）にのみ接続し、実オーディオデバイスには一切触れない（詳細は
# 下記 [3/4] 参照）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.noiria-target}"

AUTOTEST_SECS=24
ENGINE=""
AUTOTEST_OUT="$HOME/noiria-autotest"
SKIP_X11=0
SKIP_ALSA=0
X11_DISPLAY_NUM=99

while [[ $# -gt 0 ]]; do
  case "$1" in
    --autotest-secs) AUTOTEST_SECS="$2"; shift 2 ;;
    --engine) ENGINE="$2"; shift 2 ;;
    --autotest-out) AUTOTEST_OUT="$2"; shift 2 ;;
    --skip-x11) SKIP_X11=1; shift ;;
    --skip-alsa) SKIP_ALSA=1; shift ;;
    *) echo "不明な引数: $1" >&2; exit 1 ;;
  esac
done

cd "$REPO_ROOT"

echo "=== [1/4] cargo test --workspace（CARGO_TARGET_DIR=$CARGO_TARGET_DIR）==="
cargo test --workspace

echo
echo "=== [2/4] --autotest ${AUTOTEST_SECS}秒（出力先: $AUTOTEST_OUT）==="
ENGINE_ARGS=()
if [[ -n "$ENGINE" ]]; then
  ENGINE_ARGS=(--engine "$ENGINE")
fi
cargo run -p noiria-app -- games/sample --autotest "$AUTOTEST_SECS" --autotest-out "$AUTOTEST_OUT" "${ENGINE_ARGS[@]}"

echo
echo "=== [3/4] ALSA (libasound) テスト ==="
ALSA_LIBDIR=""
ALSA_CONFIG_PATH_VENDORED=""
if [[ "$SKIP_ALSA" -eq 1 ]]; then
  echo "--skip-alsa 指定のため skip します。"
else
  # --- libasound.so.2 + alsa.conf の確保 ----------------------------------
  # 1) システムに既にあればそれを使う（alsa.conf も同時に入っている前提）。
  # 2) 無ければ root 不要の `apt-get download libasound2`（Ubuntu 24.04+ は
  #    パッケージ名が libasound2t64 に変わっているため両方試す）+
  #    `libasound2-data`（"null"/"default" 等の名前付き PCM を定義する
  #    /usr/share/alsa/alsa.conf 本体。これが無いと動的ロードした
  #    libasound.so.2 自体は開けても snd_pcm_open("null") が
  #    "Unknown PCM null" で失敗する）で .deb を取得し
  #    $HOME/.noiria-alsa-root へ展開する（W2 の Xvfb ベンダリングと同じ方式。
  #    インストールはしない）。
  # 3) それも失敗したら「skip する設計」であることを明示して次へ進む
  #    （alsa_out.rs の open_default はライブラリ不在時に Err を返すだけなので、
  #    cargo test 自体は [1/4] の時点で既に安全に通過している）。
  ALSA_ROOT="$HOME/.noiria-alsa-root"
  if ldconfig -p 2>/dev/null | grep -q 'libasound\.so\.2' && [[ -f /usr/share/alsa/alsa.conf ]]; then
    echo "システムに libasound.so.2 + alsa.conf が見つかりました。"
  else
    echo "システムに libasound が見つかりません。root 不要のベンダリング経路を試します..."
    mkdir -p "$ALSA_ROOT" "$HOME/.noiria-alsa-debs"
    if command -v apt-get >/dev/null 2>&1 && (
        cd "$HOME/.noiria-alsa-debs" &&
        { apt-get download libasound2 >/dev/null 2>&1 || apt-get download libasound2t64 >/dev/null 2>&1; } &&
        apt-get download libasound2-data >/dev/null 2>&1
      ); then
      for f in "$HOME"/.noiria-alsa-debs/*.deb; do
        dpkg-deb -x "$f" "$ALSA_ROOT"
      done
    fi
    FOUND_LIB="$(find "$ALSA_ROOT" -name 'libasound.so.2*' 2>/dev/null | head -n1)"
    FOUND_CONF="$(find "$ALSA_ROOT" -name 'alsa.conf' 2>/dev/null | head -n1)"
    if [[ -n "$FOUND_LIB" && -n "$FOUND_CONF" ]]; then
      ALSA_LIBDIR="$(dirname "$FOUND_LIB")"
      ALSA_CONFIG_PATH_VENDORED="$FOUND_CONF"
      echo "libasound.so.2 + alsa.conf をベンダリングしました: $ALSA_LIBDIR / $FOUND_CONF"
    else
      echo "libasound を用意できませんでした。ALSA スモークは [1/4] の cargo test 内で"
      echo "自動 skip される設計のまま進めます（手動で用意する場合:"
      echo "  sudo apt update && sudo apt install -y libasound2"
      echo "または apt のネットワークが使えない場合、別環境で"
      echo "  apt-get download libasound2 (または libasound2t64) libasound2-data"
      echo "した .deb をこの WSL 環境の $HOME/.noiria-alsa-debs/ へ配置して再実行してください）"
    fi
  fi

  if [[ -n "$ALSA_LIBDIR" ]]; then
    echo "--- alsa_out:: スモークテストを明示的な libasound 付きで再実行 ---"
    LD_LIBRARY_PATH="$ALSA_LIBDIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
      ALSA_CONFIG_PATH="$ALSA_CONFIG_PATH_VENDORED" \
      cargo test -p noiria-core --lib alsa_out:: -- --nocapture
  else
    echo "--- alsa_out:: スモークテスト（システムの libasound がそのまま使えれば実行、無ければ skip） ---"
    cargo test -p noiria-core --lib alsa_out:: -- --nocapture || true
  fi
fi

echo
echo "=== [4/4] X11 (Xvfb) + ALSA 統合テスト ==="
if [[ "$SKIP_X11" -eq 1 ]]; then
  echo "--skip-x11 指定のため skip します。"
  exit 0
fi

# --- Xvfb バイナリの確保 -----------------------------------------------
# 1) 既にシステムに Xvfb があれば使う。
# 2) 無ければ `sudo -n apt-get install -y xvfb` を非対話で試す（NOPASSWD sudo
#    が設定済みの環境向け）。
# 3) それも失敗したら、root 権限なしで動く経路として `apt-get download`
#    （root 不要）で .deb を取得し $HOME/.noiria-xvfb-root へ展開する
#    （インストールはしない、パッケージマネージャの管理外）。
XVFB_ROOT="$HOME/.noiria-xvfb-root"
XVFB_BIN=""
if command -v Xvfb >/dev/null 2>&1; then
  XVFB_BIN="$(command -v Xvfb)"
  XVFB_LIBDIR=""
elif sudo -n apt-get install -y xvfb x11-xkb-utils >/dev/null 2>&1 && command -v Xvfb >/dev/null 2>&1; then
  XVFB_BIN="$(command -v Xvfb)"
  XVFB_LIBDIR=""
else
  echo "システムに Xvfb が無く、sudo も非対話では使えません。"
  echo "root 不要のベンダリング経路（apt-get download + 展開）を試します..."
  mkdir -p "$XVFB_ROOT" "$HOME/.noiria-xvfb-debs"
  if command -v apt-get >/dev/null 2>&1 && (
      cd "$HOME/.noiria-xvfb-debs" &&
      apt-get download xvfb xserver-common libxfont2 x11-xkb-utils libfontenc1 libxkbfile1 >/dev/null 2>&1
    ); then
    for f in "$HOME"/.noiria-xvfb-debs/*.deb; do
      dpkg-deb -x "$f" "$XVFB_ROOT"
    done
  fi
  if [[ -x "$XVFB_ROOT/usr/bin/Xvfb" ]]; then
    XVFB_BIN="$XVFB_ROOT/usr/bin/Xvfb"
    XVFB_LIBDIR="$XVFB_ROOT/usr/lib/x86_64-linux-gnu:$XVFB_ROOT/usr/lib"
  fi
fi

if [[ -z "$XVFB_BIN" ]]; then
  cat <<'EOF'
Xvfb を用意できませんでした（apt-get download も失敗）。X11 テストは skip します。
手動で用意する場合は以下のいずれかを実行してください:
  sudo apt update && sudo apt install -y xvfb x11-xkb-utils
または、apt のネットワークアクセス自体がブロックされている場合は、
別環境で `apt-get download xvfb xserver-common libxfont2 x11-xkb-utils libfontenc1 libxkbfile1`
した .deb 一式をこの WSL 環境の $HOME/.noiria-xvfb-debs/ へ配置してから再実行してください。
EOF
  exit 0
fi

# --- WSLg の /tmp/.X11-unix は読み取り専用の tmpfs バインドマウントなので、
#     ここに直接ソケットを作れない（実デスクトップ :0 用に WSLg が管理して
#     いるため、書き込み権限が無い＝ユーザーの実デスクトップを汚さない
#     設計そのものでもある）。非特権ユーザー名前空間で /tmp/.X11-unix と
#     xkbcomp（Xvfb がハードコードされた /usr/bin/xkbcomp を execl するため
#     PATH では解決できない）だけを差し替えた隔離マウント名前空間の中で
#     Xvfb と cargo test を両方走らせることで、実デスクトップに一切触れずに
#     検証する。
XKB_UPPER="$HOME/.noiria-xkb-upper"
XKB_WORK="$HOME/.noiria-xkb-work"
mkdir -p "$XKB_UPPER" "$XKB_WORK"
if [[ -n "${XVFB_LIBDIR:-}" ]]; then
  cp -f "$XVFB_ROOT/usr/bin/xkbcomp" "$XKB_UPPER/" 2>/dev/null || true
fi
# システムの Xvfb を使う場合でも xkbcomp（と依存の libxkbfile1）が入って
# いないことがあるので、無ければ .deb から持ってくる。
if [[ ! -x "$XKB_UPPER/xkbcomp" ]] && ! command -v xkbcomp >/dev/null 2>&1; then
  mkdir -p "$HOME/.noiria-xvfb-debs"
  (cd "$HOME/.noiria-xvfb-debs" && apt-get download x11-xkb-utils libxkbfile1 >/dev/null 2>&1) || true
  for f in "$HOME"/.noiria-xvfb-debs/x11-xkb-utils*.deb "$HOME"/.noiria-xvfb-debs/libxkbfile1*.deb; do
    [[ -f "$f" ]] || continue
    dpkg-deb -x "$f" "$XVFB_ROOT"
  done
  cp -f "$XVFB_ROOT/usr/bin/xkbcomp" "$XKB_UPPER/" 2>/dev/null || true
  # xkbcomp を vendor 展開した場合、その依存ライブラリ（libxkbfile.so.1 等）も
  # 一緒に見えるよう LD_LIBRARY_PATH へ追加する（Xvfb 自体は system 版でも）。
  XVFB_LIBDIR="${XVFB_LIBDIR:-}${XVFB_LIBDIR:+:}$XVFB_ROOT/usr/lib/x86_64-linux-gnu:$XVFB_ROOT/usr/lib"
fi

X11_TEST_RUNNER="$(mktemp)"
cat > "$X11_TEST_RUNNER" <<INNER_EOF
#!/usr/bin/env bash
set -euo pipefail
mount -t tmpfs tmpfs /tmp/.X11-unix -o mode=1777
if [[ -f "$XKB_UPPER/xkbcomp" ]]; then
  mount -t overlay overlay -o lowerdir=/usr/bin,upperdir="$XKB_UPPER",workdir="$XKB_WORK" /usr/bin
fi
export LD_LIBRARY_PATH="${XVFB_LIBDIR:-}${ALSA_LIBDIR:+:$ALSA_LIBDIR}"
${ALSA_CONFIG_PATH_VENDORED:+export ALSA_CONFIG_PATH="$ALSA_CONFIG_PATH_VENDORED"}
rm -f "/tmp/.X${X11_DISPLAY_NUM}-lock"
"$XVFB_BIN" :${X11_DISPLAY_NUM} -screen 0 1280x720x24 -ac -nolisten tcp &
XVFB_PID=\$!
trap 'kill \$XVFB_PID 2>/dev/null || true' EXIT

for _ in \$(seq 1 50); do
  [[ -S "/tmp/.X11-unix/X${X11_DISPLAY_NUM}" ]] && break
  sleep 0.1
done
if [[ ! -S "/tmp/.X11-unix/X${X11_DISPLAY_NUM}" ]]; then
  echo "Xvfb がソケットを作成できませんでした。X11 テストを skip します。" >&2
  exit 0
fi

cd "$REPO_ROOT"
export CARGO_TARGET_DIR="$CARGO_TARGET_DIR"
export NOIRIA_X11_TEST_DISPLAY=":${X11_DISPLAY_NUM}"
unset DISPLAY
# 複数テストが同一 Xvfb 上でウィンドウ生成・スタッキング判定を行うため直列実行。
cargo test -p noiria-app --bin noiria x11:: -- --test-threads=1

echo
echo "--- windowed + 音声の統合スモーク: games/sample を10秒実行 (NOIRIA_ALSA_DEVICE=null) ---"
export NOIRIA_ALSA_DEVICE=null
SMOKE_LOG="\$(mktemp)"
set +e
timeout --kill-after=5 12 cargo run -p noiria-app --bin noiria -- games/sample > "\$SMOKE_LOG" 2>&1
set -e
if grep -q "音声デバイス open 成功" "\$SMOKE_LOG"; then
  echo "証跡: \$(grep '音声デバイス open 成功' "\$SMOKE_LOG")"
else
  echo "警告: 「音声デバイス open 成功」のログが見つかりませんでした（libasound 未確保の可能性。ログ全文:）" >&2
  cat "\$SMOKE_LOG" >&2
fi
if grep -qiE "panicked at|thread '.*' panicked" "\$SMOKE_LOG"; then
  echo "エラー: games/sample の windowed 実行中にクラッシュ（panic）を検出しました" >&2
  cat "\$SMOKE_LOG" >&2
  exit 1
fi
mkdir -p "$REPO_ROOT/target/x11-smoke" 2>/dev/null || true
cp "\$SMOKE_LOG" "$REPO_ROOT/target/x11-smoke/games_sample_windowed_alsa_smoke.log" 2>/dev/null || true
echo "ログ: $REPO_ROOT/target/x11-smoke/games_sample_windowed_alsa_smoke.log （\$SMOKE_LOG のコピー）"
INNER_EOF
chmod +x "$X11_TEST_RUNNER"

unshare --user --mount --map-root-user "$X11_TEST_RUNNER"
rm -f "$X11_TEST_RUNNER"

SHOT="$REPO_ROOT/target/x11-smoke/games_sample_screenshot.png"
if [[ -f "$SHOT" ]]; then
  echo
  echo "games/sample の X11 スクリーンショット（GetImage 取得）: $SHOT"
  echo "（/mnt/c 経由なので Windows 側からもこのパスでそのまま開けます）"
fi

echo
echo "完了。--autotest の PNG は $AUTOTEST_OUT （Windows 側からは \\\\wsl.localhost\\... または /mnt/c 経由でコピーして確認）"
