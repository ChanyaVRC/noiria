# Linux (WSL2) 開発・検証手順

Noiria はヘッドレス（`--autotest`）に加え、GUI ウィンドウ（`run_windowed`）・
音声出力も Linux で動く（配布対応 W1: フォント / W2: X11 ウィンドウ+入力 /
W3: ALSA 音声）。Windows は Win32 直叩き + WASAPI 直叩き、Linux/Unix は
X11 コアプロトコル自作（libX11/xcb 非リンク） + ALSA (`libasound`) の
dlopen 実行時ロード（リンク時依存なし）でそれぞれ実装している。このドキュメントは
WSL2 Ubuntu での `cargo test` / `--autotest` / X11 (Xvfb) / ALSA (null デバイス)
検証手順をまとめる。

**重要**: `/mnt/c` 上のスクリプトをシェバン経由で直接実行する
（`./scripts/wsl-test.sh` や `wsl -e bash -lc "/mnt/c/.../wsl-test.sh"`）と、
環境によっては `bash: line 1: C:/Program: No such file or directory` という
不可解なエラーで失敗することがある（DrvFs 上のシェバン実行と WSL の Windows
PATH interop が絡む相性問題、W2 で発見）。**必ず `bash scripts/wsl-test.sh`
のように bash を明示して呼び出すこと。**

## 前提

- WSL2 + Ubuntu（`wsl --install` 等で導入済みのこと）
- WSL 内に rustup 済みの Rust ツールチェイン（`rustc`/`cargo` が通ること）
- リポジトリ 3 つが Windows 側 `C:\Users\<you>\Documents\BuildSoft\` 配下に並んでいること
  （`noiria` / `tsint` / `vp9dec`）。WSL からは `/mnt/c/Users/<you>/Documents/BuildSoft/...`
  で同じファイルを直接参照できる（`Cargo.toml` の `path = "../../../vp9dec"` 等の相対パス
  依存は WSL 側でもそのまま解決される。**tsint/vp9dec リポジトリ自体は変更しない**）

## 最小手順

```bash
wsl
cd /mnt/c/Users/<you>/Documents/BuildSoft/noiria

# target/ を /mnt/c 上に置くと 9P 経由の I/O で著しく遅くなるため、
# WSL 内の ext4 (=$HOME 配下) に逃がす。ソース自体は /mnt/c を直接参照する。
export CARGO_TARGET_DIR=$HOME/.noiria-target

cargo test --workspace
cargo run -p noiria-app -- games/sample --autotest 24 --autotest-out $HOME/noiria-autotest
```

まとめて実行したい場合は `scripts/wsl-test.sh` を使う（`cargo test` → `--autotest`
→ X11 (Xvfb) テストの 3 ステップをまとめて実行する）。

```bash
wsl -e bash -lc "bash /mnt/c/Users/<you>/Documents/BuildSoft/noiria/scripts/wsl-test.sh"
```

X11 ステップを飛ばしたい場合は `--skip-x11` を付ける。Xvfb が用意できない環境
（後述）では自動的に skip し、その旨をメッセージで報告する。

生成された PNG は WSL 内パス（既定 `$HOME/noiria-autotest`）にあるので、Windows 側で
目視確認したい場合は `/mnt/c/...` 配下へコピーする:

```bash
cp -r "$HOME/noiria-autotest" "/mnt/c/Users/<you>/Documents/BuildSoft/noiria/target/wsl-autotest-copy"
```

## 日本語フォント（重要）

`--autotest` のスクリーンショットで日本語が描画されるには、Linux 側に CJK フォントが
必要（W1 で `font/registry.rs` が per-OS のシステムフォントディレクトリ
[`/usr/share/fonts`, `/usr/local/share/fonts`, `~/.local/share/fonts`, `~/.fonts`] を
再帰走査し、`Noto Sans CJK JP` → `Noto Sans JP` → `TakaoGothic` → `IPAGothic` →
`VL Gothic` の順にフォールバックする）。

推奨（sudo が使える場合）:

```bash
sudo apt update && sudo apt install -y fonts-noto-cjk
```

sudo が使えない/インストールできない環境では、**ゲームローカルフォント**機能
（`games/<game>/fonts/*.ttf|ttc` を同梱すると自動的にシステムフォントより優先される、
W1 の本命機能）を使って検証できる。例えば Windows 側の游ゴシックを一時的にコピーする:

```bash
mkdir -p games/sample/fonts
cp /mnt/c/Windows/Fonts/YuGothM.ttc games/sample/fonts/
```

**注意**: フォントは再配布ライセンス上コミットしない。検証が終わったら削除すること
（`rm -rf games/sample/fonts`）。`.gitignore` は現状無いので、コミット前に手動で
確認すること。

## システムフォントが 1 件も見つからない場合

`FontRegistry::scan_system_fonts()` は走査した全ディレクトリを添えて `warn` ログを出す
（「黙って文字が消える」状態を避けるための W1 の変更）。ログに `システムフォントが1件も
見つかりませんでした` が出た場合は、上記のいずれかの方法でフォントを用意すること。

## X11 ウィンドウ（GUI）の検証: Xvfb

`run_windowed`（Unix 版は `crates/noiria-app/src/x11/`）は X11 コアプロトコルを
Unix ドメインソケット上に自作で実装しており、**libX11/xcb には一切リンクしない**
（Win32 直叩きと同じ思想）。ユーザーの実デスクトップ（WSLg の `:0` 含む）には
絶対に窓を出さないという制約があるため、検証は常に **Xvfb**（隔離された仮想
ディスプレイ、既定 `:99`）上でのみ行う。

`scripts/wsl-test.sh` が Xvfb の確保から起動、テスト実行までを自動化している
（root 権限は一切使わない）。具体的には:

1. システムに `Xvfb` があればそれを使う。無ければ `sudo -n apt-get install`
   （NOPASSWD sudoers があれば効く）を試し、それも駄目なら
   `apt-get download`（**インストールではなく .deb のダウンロードのみ、root
   不要**）で `xvfb`/`xserver-common`/`libxfont2`/`x11-xkb-utils`/
   `libfontenc1`/`libxkbfile1` を取得して `$HOME/.noiria-xvfb-root` へ展開する。
2. WSLg の `/tmp/.X11-unix` は実デスクトップ用に**読み取り専用の tmpfs**として
   マウントされているため、そのままでは Xvfb 用のソケットを作れない。
   非特権ユーザー名前空間（`unshare --user --mount --map-root-user`、root
   不要）の中だけで `/tmp/.X11-unix` を書き込み可能な tmpfs へ再マウントする
   （この名前空間は完全に隔離されており、実デスクトップには一切影響しない）。
3. 同じ名前空間内で `/usr/bin` に overlayfs を重ね、Xvfb が実行時に
   ハードコードして呼び出す `xkbcomp`（未インストールだと keyboard 初期化に
   失敗する）を追加する。
4. `NOIRIA_X11_TEST_DISPLAY=:99` を設定して `cargo test -p noiria-app --bin
   noiria x11:: -- --test-threads=1`（同一 Xvfb 上でウィンドウのスタッキング
   判定を行うテストがあるため直列実行が必須）を、同じ名前空間内で実行する。

ウィンドウ機能のテスト（リサイズ → レターボックス描画とマウス座標の逆変換、F11 / Alt+Enter、キーの
オートリピート抑止、`WindowClose`）は、別クライアントからの `ConfigureWindow` / `SendEvent`（合成キー・ボタン
イベント）と、root の SubstructureRedirect を握る**偽 WM**（`_NET_WM_STATE` の ClientMessage を読む）で行う。
Xvfb には WM が無いため、実 WM でのフルスクリーン遷移（見た目・多画面）は未検証。

X11 系のテストは環境変数 `NOIRIA_X11_TEST_DISPLAY` が設定されているときだけ
実走する（未設定なら即座に skip）。通常の `cargo test --workspace`
（ステップ 1）ではこの変数を設定しないため、Xvfb の有無にかかわらず安全に
実行できる。

## ALSA 音声（W3）

`audio.rs` の出力層は Unix では `alsa_out.rs` を使う。`libasound.so.2`
（無ければ `libasound.so`）を **dlopen で実行時ロード**するのみで、
`Cargo.toml` にはリンク時依存を一切追加していない（libasound の無い
環境でもビルド・起動でき、`open_default()` が `Err` を返して既存の
「無音で続行」にフォールバックするだけ）。

検証は常に **ALSA `"null"` デバイス**（何も鳴らさない仮想デバイス）にのみ
接続する。実オーディオデバイスや PulseAudio/PipeWire ブリッジには一切
触れない:

```bash
NOIRIA_ALSA_DEVICE=null cargo test -p noiria-core --lib alsa_out::
```

`scripts/wsl-test.sh` の `[3/4]` がこのテストを（libasound が無ければ
root 不要のベンダリング経路で確保した上で）自動実行し、`[4/4]` で
Xvfb 上の windowed 実行と組み合わせた統合スモーク（`NOIRIA_ALSA_DEVICE=null`
のまま 10 秒 `run_windowed` を実行しログにデバイス open 成功の証跡が
出ることを確認）まで行う。libasound が全く用意できない環境では
両方とも自動 skip される（`docs/linux-dev.md` 本節と同じ設計）。

実機の音声デバイスで実際に音が鳴るかどうかの確認（`NOIRIA_ALSA_DEVICE`
省略時の `"default"`、通常は PulseAudio/PipeWire の ALSA ブリッジ経由）は、
本ドキュメントの自動化検証ではカバーしない。ユーザー自身が通常の WSLg
環境で `cargo run -p noiria-app -- games/sample` を実行して確認するタスク
として切り出す。

## 既知の制約（WSL2 環境依存）

- `sudo` にパスワードが必要な環境では apt install が非対話実行できない（このドキュメントの
  検証時点の環境がそうだった）。CI 等で自動化する場合は事前にフォントをイメージへ
  焼き込むか、`NOPASSWD` sudoers を用意すること。X11 検証（Xvfb）・ALSA 検証
  （libasound）は上記の通り sudo 無しでも動く経路を用意済み。
- WSLg 実窓（`:0`）での実際の見た目・操作感・音の目視/聴取確認は、本ドキュメントの
  自動化検証ではカバーしない（Xvfb は隔離ディスプレイであり実デスクトップに
  描画されないため。ALSA も null デバイスのみで実音は出さない）。ユーザー自身が
  `cargo run -p noiria-app -- games/sample` を通常の WSLg 環境で実行して
  確認するタスクとして切り出す。
