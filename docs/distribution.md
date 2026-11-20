# 配布ビルド手順（W7）

`games/<name>` フォルダから、数 MB 級の tsint 専用 exe + ゲーム固有鍵で暗号化された
`game.pak` + ライセンス表記 (`THIRD-PARTY.txt`) + 起動説明 (`README.txt`) の配布物一式を
Windows/Linux 両ターゲット向けに作る手順。`docs/implementation-notes.md`
「2026-07-12 配布対応 W7」に設計判断の詳細がある。

## 全体の流れ

```
1. Windows 側で dist ビルド（Windows exe）
2. WSL 側で dist ビルド（Linux バイナリ）
3. noiria-export でゲームを export（両方のビルド済みバイナリへ KEY_SLOT パッチ + pack）
```

export はクロスコンパイルしない。KEY_SLOT バイナリパッチ方式（`docs/implementation-notes.md`
2026-07-12 W6/W7 参照）なので、**ビルド済みバイナリはゲームごとの再ビルドが不要**
— 手順 1・2 は最初に 1 回行えば、以後は何本ゲームを export してもそのまま使い回せる
（tsint のランタイム TS モジュールは全ゲーム共通の埋め込みなので、Rust 側の再ビルドは
ゲーム固有ではない）。

## 1. Windows 側で dist ビルド

```powershell
cargo build --profile dist --no-default-features --features engine-tsint,dist -p noiria-app
```

`target\dist\noiria.exe` が生成される（V8 抜き、`[profile.dist]`
= release + `lto=thin` + `strip=symbols` + `panic=abort` の軽量ビルド）。

## 2. WSL 側で dist ビルド（Linux バイナリ）

```
wsl -e bash -lc "bash /mnt/c/Users/<you>/Documents/BuildSoft/noiria/scripts/build-dist-wsl.sh"
```

（`docs/linux-dev.md` と同じ理由で `/mnt/c` 上のスクリプトは必ず `bash` 経由で呼ぶこと。
ビルド自体は WSL 内 ext4 ($HOME/.noiria-target) で行い、9P 越しの `/mnt/c` 上のビルドで
著しく遅くなるのを避ける。）

完了すると Windows 側から見て `target\dist\noiria`（拡張子なし、Linux ELF）が
生成される。`target\dist\` には Windows 版 (`noiria.exe`) と Linux 版 (`noiria`)
が同居する。

## 3. export

```powershell
cargo run -p noiria-tools --bin noiria-export -- --game games/sample --out target/export
```

既定で `--targets windows,linux` 両方、`--bin-windows`/`--bin-linux` は
`target/dist/noiria.exe` / `target/dist/noiria`（手順 1・2 の出力）を自動的に探す。
`--exe-name` 省略時は `game.json` の `title` を安全な文字（英数字/`-`/`_`）だけへ
正規化した名前になる（例: `games/sample` の `"Noiria サンプル"` → `Noiria`）。
既定動作は strict で、指定した全ターゲットのバイナリを読み込み、`KEY_SLOT` を検証してから
成果物を書き始める。片方が欠けていても存在する側だけを出力したい場合は、明示的に
`--skip-missing-targets` を付ける。

```powershell
cargo run -p noiria-tools --bin noiria-export -- --game games/sample --out target/export --skip-missing-targets
```

このオプションでスキップするのは「バイナリが見つからない」ターゲットだけである。
権限エラー、壊れた `KEY_SLOT`、pack・書き込み失敗は通常どおり全体をエラーにする。
全ターゲットが見つからない場合も失敗する。スキップしたターゲットの既存出力は削除しない。

処理内容:

1. `game.json` の必須項目（`title`/`id`）を確認。`.mp4` アセットが含まれる場合は
   「pak 内から再生できない」警告を表示する（Media Foundation は実ファイルパス前提の
   API のため。`docs/implementation-notes.md` 2026-07-12 W6 参照。`.avi`/`.ivf`
   (VP9/MJPEG) を推奨）。
2. OS 乱数でゲーム固有の pak 鍵を 32 バイト生成する
   （Windows: `bcryptprimitives.dll` の `ProcessPrng`（`BOOL ProcessPrng(PBYTE, SIZE_T)`、Windows 8+）を
   raw-dylib で直宣言、それ以外: `/dev/urandom`。外部クレートには依存しない）。
3. 指定されたターゲットぶんのビルド済みバイナリを事前に読み込み、埋め込み `KEY_SLOT`
   マジック（`NOIRIA-KEYSLOT1\0`）をバイト検索して、直後の 32 バイトを手順 2 の鍵で
   上書きする（マジックが 0 個・2 個以上見つかった場合は、出力開始前に明確なエラーで
   中断する）。
4. `noiria-tools::pack::pack_dir_with`（`noiria-pack` CLI と同じ実装）でゲームディレクトリをその鍵で
   暗号化 pak (`game.pak`) にまとめる。**除外**: `--out` ディレクトリ（`--game` 内にあっても）、
   ドットファイル/ディレクトリ、`Thumbs.db` `desktop.ini` `*~` `*.bak` `*.tmp`、`game.json` の `pack.exclude`、`--exclude <glob>`（複数可。
   `*` は `/` を跨がない・`**` は跨ぐ・`?`。`/` を含まないパターンはファイル/ディレクトリ名、含むパターンは
   ゲームからの相対パスに照合し、ディレクトリに当たれば配下ごと除外）。既定除外は `--no-default-excludes` で切る。
   シンボリックリンク/ジャンクション・非 UTF-8 名・大文字小文字だけが違うキーの重複はエラー。
   `--list` で pak に入ったエントリ（パス・元サイズ・変換）を表示する。
   WebP / wav-flac 変換は、採用前にデコードし直して元と一致を確認する（不一致なら警告して無変換）。
5. `out/<target>.tmp/` へ実行ファイル・`game.pak`・`THIRD-PARTY.txt`・`README.txt` を全部書いてから
   `out/<target>/` と入れ替える（既存の出力は一旦退避し、失敗したら戻す）。書き込みに失敗しても旧出力は壊れず、
   exe と `game.pak` の鍵が食い違う配布物は残らない。**`out/<target>/` は毎回丸ごと置き換わる**ので、
   中に置いた手動の追加ファイルは消える（旧名の exe も残らない）。

生成物:

```
target/export/windows/Noiria.exe
target/export/windows/game.pak
target/export/windows/THIRD-PARTY.txt
target/export/windows/README.txt
target/export/linux/Noiria
target/export/linux/game.pak
target/export/linux/THIRD-PARTY.txt
target/export/linux/README.txt
```

配布時はこのフォルダの中身をそのまま zip 等にまとめる（`game.pak` は実行ファイルと
同じフォルダに置く必要がある）。

### pak に入れないファイル（`game.json` の `pack.exclude`）

開発用のファイル（autotest スクリプト等）を配布物から外すには、`game.json` に任意フィールド `pack` を書く:

```json
{ "title": "...", "id": "...", "pack": { "exclude": ["autotest/**", "*.psd"] } }
```

除外は **既定除外（ドットファイル・`Thumbs.db` `desktop.ini` `*~` `*.bak` `*.tmp`） + `game.json` の `pack.exclude` +
CLI の `--exclude <glob>`** の和集合。glob は `*`（`/` を跨がない）・`**`（跨ぐ）・`?`、ゲームディレクトリからの相対パス
（`/` 区切り）に照合し、`/` を含まないパターンはファイル/ディレクトリ名に照合する（ディレクトリに当たれば配下ごと除外）。
`--no-default-excludes` は既定除外だけ、`--no-project-excludes` は `game.json` の指定だけを無効にする。
`game.json` 自体と `main` が指すファイルは除外できない（除外しようとするとエラー）。`pack` の型が不正（配列でない・
文字列でない要素）でもエラーで、該当箇所（例: `pack.exclude[1]`）を示す。エンジン本体は `pack` を読まず無視する。
`--list` は pak に入ったエントリ、`--list-excluded` は除外されたパスと規則（`project: autotest/**` など）を表示し、
`noiria-export` は最後に除外件数と内訳を 1 行で表示する。`games/demo` は `autotest/**` を除外している。

## プレイヤー側の起動

- 引数を付けずに実行ファイルをそのまま起動すると、実行ファイルと同じフォルダの
  `game.pak`（無ければ `game/` フォルダ）を自動的に探す（`main.rs::
  dist_default_game_path`）。開発ビルドの `games/sample` フォールバックは
  dist ビルドには適用されない。
- ログはセーブデータと同じ場所（ゲーム固有のユーザーデータフォルダ、
  `noiria_core::savedata::save_dir(game_id)`）の `noiria.log` に出力される
  （`env_logger` の代わりに自作した `dist_logger::FileLogger`。ゲームが正常に
  開けるまでの起動直後のログだけは実行ファイル隣接の `noiria-startup.log` に出る。
  詳細度は環境変数 `NOIRIA_LOG`、既定 `warn`）。
- Windows 版は `windows_subsystem="windows"`（`dist` feature 時のみ）でコンソールを
  出さない。そのため **致命エラーは `MessageBoxW` で表示**する（`game.pak` / `game/` が見つからない・
  ゲームを開けない・スクリプトスレッドの致命終了・panic。本文末尾にログの場所を案内）。ログには常に
  残る。`--autotest*`（ヘッドレス）・`cargo test`・環境変数 `NOIRIA_NO_DIALOG` 設定時はダイアログを
  出さず stderr/ログのみ。Linux は常に stderr + ログ。
- ウィンドウはリサイズ・最大化でき（レターボックス）、**F11 / Alt+Enter** でフルスクリーン切替。
  起動時はモニタの DPI 倍率で作業領域に収まる最大まで拡大して開く。閉じる操作はスクリプトへ
  `WindowClose` を送り、2 秒以内に終了しなければ強制終了する（`README.md` 参照）。
- ヘッドレス検証用に `--save-dir <dir>`（環境変数 `NOIRIA_SAVE_DIR`）でセーブ先を差し替えられる
  （配布物の自動検証で実プレイヤーのセーブを汚さない）。

## 検証（このリポジトリでの自動検証の範囲）

- `scripts/feature-build-matrix.ps1`: default / engine-v8 のみ / engine-tsint+dist の
  3 通りが `cargo build` で通ることを確認し、各構成の `cargo clippy -- -D warnings`（依存 crate 分も含む）が警告 0 であることも要求する（警告があればその構成は失敗）。
  `env_logger` は `dev-log` feature（`engine-v8` が有効にする）の optional 依存で、dist ビルドの依存グラフには載らない。
- `crates/noiria-tools/tests/third_party_check.rs`: `cargo metadata` の実依存グラフと
  `THIRD-PARTY.txt` の突き合わせ（陳腐化検出）。
- `crates/noiria-tools/src/export.rs` の単体テスト: 鍵生成の乱数性、`KEY_SLOT`
  パッチの成功/失敗（マジック 0 個・2 個）、exe 名の正規化。
- `crates/noiria-tools/tests/export_cli.rs`: strict の事前失敗、片側だけの部分成功と
  CLI 報告、全ターゲット欠如、壊れた `KEY_SLOT` をスキップしないことに加え、再 export 時の整合（読み取り専用 pak・
  書き込み失敗の注入でも旧出力が無傷、旧名 exe が残らない、`--out` が `--game` 内でも前回の出力を pak に詰めない）を
  確認する。`tests/pack_walk.rs` は走査（リンク・重複キー・既定除外）の検査。
- export 産物の実機 E2E（`--autotest`、KEY_SLOT パッチ済み/未パッチの鍵不一致確認）は
  `docs/implementation-notes.md` 2026-07-12 W7 の検証結果を参照（このドキュメントは
  手順書なので実行結果そのものは載せない）。

## 実機の遅さを切り分ける（`NOIRIA_PROF`）

画面の出が遅いと感じたら、環境変数 `NOIRIA_PROF=1` を付けて起動し、遅い操作をしてから終了して `noiria.log`（セーブ先の
`save` フォルダ内）を送ってください。フレームごとに 1 行出ます（Windows / X11 とも）:
`[PROF] t=<起動からms> compose=<合成ms> convert=<BGRX変換ms> scale=<拡大ms> blit=<転送ms> memblit=<0> lock_wait=<シーンのロック待ちms>
held=<その周期の合計ms> layers=<可視レイヤ数> anim=<動き中か> wakes=<前回から受けた再描画要求数> client=<クライアントサイズ> since_input=<最後のクリック/キーからの ms>`。`since_input` が小さいフレームが並ぶ間が、その操作の画面が出るまでの区間です。
`compose+scale+blit` が 16ms を超えるフレームが続く、`lock_wait` が大きい、のどちらかが原因の目安です。`NOIRIA_PROF=2` は
転送をメモリ DC へも行い、画面に出ない環境でも拡大転送の実コストを測ります。
開発用に `noiria <ゲーム> --bench-window 1920x1080 --bench-script <autotest 用 DSL>`（Windows のみ）で、不可視ウィンドウ・無音のまま
実ウィンドウの経路へ入力を注入して同じログを取れます（`shot <名前>` はログの目印になります）。

## 音声アセットの軽量化（W9）

pak の大部分（`games/sample` では 9 割強）は無圧縮 WAV の BGM/SE/ボイスが占める。作者向けの
指針:

- **`.ogg`（Vorbis）を直接持ち込むのが推奨**: `codec/ogg.rs` + `codec/vorbis/` の自作デコーダが
  既に対応しており、`.ogg` は元から圧縮済み（`pak::is_precompressed_ext` が stored 判定）なので
  pak はそのまま格納する。有損だが小さい。
- **`.wav` はそのまま置いてよい**: pack/export 時に**自動で可逆圧縮**される（W9、下記）。
  シナリオ側は `.wav` のまま参照し続けてよく、挙動やタグの書き方は一切変わらない。
- **`.flac` ファイルの直接持ち込みは引き続き非対応**（`audio.rs` は WAV/OGG のみ読める）。
  `.flac` 素材は事前に `.wav` か `.ogg` へ変換してから配置すること。

### 仕組み（pak 内透過コーデック wav-flac）

pack 時、`.wav` のうち 16bit PCM / 1-2ch のものは data チャンクだけを自作 FLAC 風サブセット
コーデック（fixed predictor 0-4 + LPC 1-8（Levinson-Durbin）+ Rice 符号、本家 FLAC との
ビットストリーム互換は無い、`crates/noiria-core/src/codec/flac_subset.rs` 参照）で可逆圧縮し、
pak には `codec=2 (wav-flac)` として格納する。**エンジンが `read_bytes` で読む際は元の WAV
バイト列を完全に復元する**ため、`audio.rs` 側の WAV デコード経路・シナリオの `.wav` 参照・
公開仕様は一切変わらない（縮まない場合や 8/24bit 等の非対応フォーマットは従来通り
DEFLATE/stored にフォールバックする）。

`games/sample` 実測（WAV 5 ファイル）: 21.47MB → 12.29MB（**-42.7%**、LPC 拡張前は -42.2%）。
エンコーダは `noiria-tools/src/flac_enc.rs`（`noiria-pack`/`noiria-export` の pack 処理から
呼ばれる）、デコーダは `noiria-core` 側の `codec/flac_subset.rs`（エンジン本体・`Pak::read`
から呼ばれる）に意図的に別クレートで独立実装してある。`noiria-pack`/`noiria-export` とも
既定で有効、`--no-flac` で無効化できる（`--no-webp` と同様、圧縮を試みず元の WAV バイト列の
まま DEFLATE/stored ルートで格納する）。設計判断の詳細は `docs/implementation-notes.md`
「2026-07-12 配布対応 W9」「2026-07-13 wav-flac LPC 拡張」参照。

## 既知の制約

- export はクロスコンパイルしない。Linux 版を出力するには、Linux バイナリを事前に
  WSL（または実 Linux 環境）でビルドしておく必要がある。
- 動画は `.mp4`（Media Foundation 依存）が pak 内から再生できない。`.avi`/`.ivf`
  (VP9/MJPEG、自作デコーダ) を推奨（`docs/implementation-notes.md` 2026-07-12 W6 参照）。
- アイコン: ウィンドウ/タスクバー（Win32 `WM_SETICON` / X11 `_NET_WM_ICON`、ゲームの `icon.png`
  またはエンジン既定）は対応済み。Explorer 上の exe ファイルアイコンはエンジン既定（`build.rs` が
  埋め込む）のままで、ゲーム毎の差し替え（PE `.rsrc` パッチ）は未実装（`docs/backlog.md` B1）。
