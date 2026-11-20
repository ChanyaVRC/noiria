# Noiria

**TypeScript で動くノベルゲームエンジン**。コアは Rust 製。

```text
┌────────────────────────────────────────────────┐
│ ゲーム (games/xxx)                              │
│   main.ts + scenario/*.ks + assets/            │
├────────────────────────────────────────────────┤
│ タグシナリオフレームワーク (TS, runtime/kag)    │
│   .ks パーサ/プレイヤー/セーブ管理              │
├────────────────────────────────────────────────┤
│ エンジン API (TypeScript, runtime/api.ts)       │
├────────────────────────────────────────────────┤
│ ネイティブコア (Rust)                           │
│   V8(deno_core) / tsint(自作TSインタプリタ) /   │
│   レイヤCPU合成 / テキスト / トランジション /   │
│   オーディオ / セーブIO                         │
└────────────────────────────────────────────────┘
```

## 実行

```powershell
# サンプルアセットの生成（初回のみ）
cargo run -p noiria-tools -- games/sample/assets

# サンプルゲームの起動
cargo run -p noiria-app -- games/sample

# デモ（恋愛アドベンチャー「星待ちの放課後」、3 エンド）の起動
cargo run -p noiria-app -- games/demo
```

`games/sample` はタグ網羅の機能デモ、`games/demo` は 1 本のゲームとして遊べる作例
（市販 ADV 風のモダン UI: 下部の全幅の帯・選択肢ボタン・専用タイトル・カード式のセーブ/ロード・ギャラリーつき）。デモのアセットは生成済みで同梱しており、
再生成は `cargo run --release -p noiria-tools --bin gen-demo-assets -- games/demo/assets`。
配布物の作り方は `docs/distribution.md`（`--game games/demo --exe-name Hoshimachi`）。

操作: クリック/Enter=送り、右クリック=メッセージ消し、S=スキップ、A=オート、H/PageUp/ホイール上=バックログ、
F5=クイックセーブ、F9=クイックロード（画面上部に約 1.2 秒のトースト）、
Escape=システムメニュー（セーブ/ロード/設定のタブ。右クリック・Esc で閉じる）:
セーブ/ロードは 1 ページ 6 枠×5 ページ（30 枠）+ オート/クイック枠（A / Q ページ）で、各枠にサムネイル・保存日時・
章名（ラベルの `|` 以降）・直近の本文冒頭を出す。上書き/ロード/削除/タイトルへ戻る/ゲーム終了は確認つき。
設定は BGM/SE/ボイス音量・文字速度・オート速度・未読もスキップ・メッセージ窓の濃さと「初期設定に戻す」
（スライダーはクリック位置で値が決まり、変更は即時反映+自動保存）。↑↓←→ + Enter + Esc + Tab でキーボードでも操作できる。
バックログは話者名つき（ボイスのある行はクリックで再生）、ホイール・↑↓・PageUp/Down でスクロール。
選択肢（`[link]`）は ↑↓ で選び Enter で決定できる。選択肢が出た時点で自動的にオートセーブ枠へ保存する。

モダン UI (B3): `KagOptions.ui: "modern"`（既定は `"classic"`）で、会話画面・選択肢・タイトル・メニュー・ギャラリーが市販 ADV 風になる
（デモ `games/demo` が使用）。実装は `runtime/kag/{mui,mhud,mtitle,mmenu}.ts`（classic の `ui.ts`/`menu.ts` は無変更）。
- **会話画面**: 箱型の窓をやめ、下部の全幅の帯（上端が透明へ溶ける暗幕。「メッセージ窓の濃さ」設定が効く）の中央寄せカラム（1 行 30 字前後）に本文を出す。
  話者名は本文の左上にアクセント色 + 細いバー（独立した箱にしない）、クリック待ちは本文末尾の小さな ▼（ゆっくり上下・明滅）。
  システムボタン（`systemButtons: true`）は帯の下端右寄りの小さなテキストボタン列（ホバーで明るくなり下線が左から伸びる、Auto/Skip の ON はアクセント色）。
- **選択肢**: `KagOptions.choices: { style: "inline" | "buttons" }`（既定は modern なら `"buttons"`、classic は `"inline"`）。`"buttons"` は `[s]` で止まった時点の
  `[link]` 群を画面中央の縦並びの横長ボタンにする（シナリオは書き換え不要。`[link]` の前の地の文は窓に残り、リンクの文字は窓に出さずボタンのラベルにする。
  先頭の空白・▶ は除く。`exp=` リンクも動く）。出現は背景を少し暗くして上から順にフェード + スライド、ホバー/↑↓ で枠と左バーがアクセント色、選択時は選ばれた 1 つが光って他が消える。
- **タイトル**: `KagOptions.titleScreen: { name, subtitle?, tagline?, version?, menu: [{ label, sub?, action? | target?/storage? | exp? }] }` を宣言し、シナリオは `[titlescreen]` と書くだけ。
  `action` は `"continue"`（ロード画面。セーブが無いと無効表示）/ `"gallery"` / `"config"` / `"exit"`（確認つき）。ロゴ → 項目の順にフェードインし、項目はホバーで右へ寄りアクセントの線が伸びる。
  メッセージ窓はタイトル用に隠す（`[layopt layer=message0 visible=false]`）。`sys.gallery()` でシナリオからもギャラリーを開ける。
- **メニュー**: セーブ/ロードはサムネイルが主役の 3 列 x 2 行のカード（ホバーで浮く・ページ切替はスライド・削除は × ボタン）、設定は「SOUND / TEXT / SYSTEM」のグループ見出しと細いスライダー・セグメントコントロール、
  バックログは話者名（左）と本文（右）の 2 カラム、確認ダイアログは中央のカード（はい=アクセント塗り）、トーストは上部中央からスライドイン、オート/スキップ中は右上にバッジ。
  すべての出現/消滅/ホバーは 120〜220ms のイージング付きアニメ（仮想クロック駆動で、ヘッドレスでも V8/tsint のフレームが一致）。
- **ギャラリー**: `KagOptions.gallery: [{ flag, image, title }]`（`flag` は sf の変数名。真なら解放済み = CG サムネイル + 名前、偽なら鍵つきの「？？？」）。クリックで全画面表示、もう一度クリックで戻る。
- 新しい描画 op（`layer.panel` / `loadImageScaled` / `cropTo`・`cropW`、`loadSaveThumb(…, w, h)` / `thumbCapture(w, h)`）の仕様と設計判断は `docs/implementation-notes.md`「2026-11-19 B3」。

ウィンドウ: 起動時はモニタの DPI 倍率（150%/200% でも極小にならない）で作業領域に収まる最大まで拡大して開く。
ドラッグでリサイズ・最大化でき、アスペクト比を保ったレターボックス（黒帯）で表示する（マウス座標は
表示倍率・黒帯を逆変換して論理座標でスクリプトへ渡る）。**F11 / Alt+Enter** でボーダーレスフルスクリーン切替
（X11 は WM の `_NET_WM_STATE_FULLSCREEN`、WM が無い環境では無効）。閉じるボタンはスクリプトへ
`WindowClose` を送り（保存してから終了できる）、2 秒以内に終了しなければ強制終了する。

### サンプル BGM（手続き生成シンセ）

`gen-assets`（`crates/noiria-tools/src/synth.rs`）は外部音源ファイルを一切使わず、オシレータ
（サイン/三角/デチューン saw/2op FM）・声部連結（voice leading）ボイシング・ビブラート・
コーラス/アンサンブル（LFO変調ディレイ）・自作 Schroeder/Freeverb 型リバーブ・1-pole LPF
（時間変化するカットオフ対応）・ADSR・補助的なフィードバックディレイ・ソフトクリップだけで
BGM/SE をライセンスフリー自作生成する。BGM 本体は全て撥弦を使わないレガート/持続音構成
（Karplus-Strong 撥弦は UI クリック SE 専用に残している）。設計判断の経緯とリサーチは
`docs/bgm-design-notes.md` にまとめてある。`bgm/` には 3 曲入っている（いずれもステレオ
44.1kHz/16bit、32〜48秒級のシームレスループ。`fold_tail_into_loop` でループテールを先頭へ
折り返し加算し、境界を `taper_loop_seam` で軽くテーパーすることでクリックを防いでいる）。

| ファイル | 曲調 | 構成 |
| --- | --- | --- |
| `bgm/main.wav` | 温かい日常曲 | Am7-Fmaj7-Cmaj7-G7 (BPM90) / 声部連結パッド + レガート2opFMエレピメロディ + サブベース。8小節×2周（2周目でメロディ輪郭が変化） |
| `bgm/alt_calm.wav` | 静かめ | Cmaj7 系進行 (BPM60) / 厚めのパッド + スローなレガートFMエレピメロディ中心 |
| `bgm/alt_tension.wav` | 少し緊張感 | Dm-Bb-F-C の短調進行 (BPM100) / 低めベースのオスティナート + 声部連結パッド |

差し替えは `.ks` 側で `storage=` を変えるだけでよい。例えば `games/sample/scenario/first.ks` の
`[playbgm storage=main.wav]` を `[playbgm storage=alt_calm.wav]` に変更すれば静かめの BGM に切り替わる
（`alt_calm.wav` / `alt_tension.wav` も `gen-assets` 実行時に `bgm/` へ生成されるので、`storage=` を
差し替えるだけで別ファイルを用意する必要はない）。数値的な品質検証（ピーク/RMS/ステレオ相関/ループ境界
連続性/長さ/DCオフセット）は `crates/noiria-tools/tests/audio_quality.rs`（`cargo test -p noiria-tools`）。

> **注意**: ウィンドウ/描画/入力は Win32 API 直叩き実装（`crates/noiria-app/src/win32.rs`）。
> 検証は `cargo test`・`--autotest`（ヘッドレス）・不可視ウィンドウ統合テストのみで行っており、
> **可視ウィンドウでの最終確認（実際に画面に表示してのクリック位置・リサイズ/レターボックス・DPI 拡大・F11 フルスクリーン・タイトル表示等の目視確認）は未実施**。
> 実機での動作確認をお願いします。

### ヘッドレス自動テスト（ウィンドウ・音なし）

```powershell
cargo run -p noiria-app -- games/sample --autotest 30 --autotest-out target/autotest
```

窓も音も出さずにシナリオを自動進行（合成クリック/キーで送り・選択肢・ホバー・セーブロード・メニューまで実施）し、
合成フレームを PNG で書き出す。描画・進行のスモークテストや CI に使う。
`games/testbed/` は選択肢を出しっぱなしにする検証専用ゲーム（ホバーハイライト等の確認用）。

#### スクリプト駆動 autotest（`--autotest-script`）

固定振付の代わりに、行指向の DSL で任意の操作列を実行し、好きな場面を `shot` で撮れる。

```powershell
cargo run -p noiria-app -- games/demo --engine tsint `
  --autotest-script games/demo/autotest/hinata.txt --autotest-fast `
  --autotest-out target/autotest-demo/hinata --save-dir target/autotest-demo/save-hinata
```

```text
# コメント            wait 500          仮想時間で待つ(ms)
click 1150 600        rclick 640 360    move 160 560   （座標は論理解像度。click は直前に同座標の move を送る）
key Enter / keyup Control / key Space   shot title_01  （<out>/title_01.png を保存）
repeat 20 { click 1150 600 ; wait 400 } （1 行完結の繰り返し。入れ子可、コマンドは ; で区切る）
```

- 時間は固定振付と同じ仮想クロック + ロックステップ（イベントごとにスクリプト側の応答を待つ）。スクリプトの
  末尾、またはゲームの `[close]` で終了し、終了コードは 0（ゲームスクリプトが致命エラーで終了した場合は非 0）。
  `--autotest <秒>` を併用すると仮想時間の上限になる（既定 3600 秒）。
- `--autotest-fast` は 16ms の実時間スリープを省き仮想時間だけを進める（スクリプト実行時のみ有効。
  毎 tick の合成もしない。V8/tsint 両エンジンで同一スクリプト 3 回の全 PNG が一致することを確認済み）。
- `--save-dir <dir>`（または環境変数 `NOIRIA_SAVE_DIR`）でセーブ先を差し替えられる。autotest が実際の
  `%APPDATA%\Noiria\<id>` を汚さないよう**必ず指定し、実行ごとに空のディレクトリにする**
  （前回の `sf.*`・設定が残ると到達済みエンド表示や文字速度が変わる）。
- `games/demo/autotest/{hinata,shiori,normal}.txt` はデモの 3 エンドルート（タイトル・各選択肢・エンド CG・
  エンド名の行・おまけ画面を撮る）。クリック数は `.ks` の `[np]` 数に依存するので、シナリオを変えたら
  スクリプト冒頭のコメントの内訳を見直すこと。

### スクリプトエンジンの選択（V8 / tsint）

Noiria は 2 つのスクリプトエンジンを選んで実行できる。

- **`v8`（既定）**: `deno_core`/V8。既存の実績あるパス。
- **`tsint`**: 完全自作の TypeScript インタプリタ（別リポジトリ [`tsint`](../tsint)、BuildSoft 配下に
  sibling で clone されている前提。`Cargo.toml` の `tsint = { path = "../../../tsint" }`）。
  tokio 非依存の単一 OS スレッド上で、自前のタイマーヒープ + 入力ポーリングによる
  駆動ループで非同期 ops（`sys.wait`/`trans.wait`/`audio.waitEnd`/入力ポンプ）を回す。

`game.json` の `"engine"` フィールド（`"v8"` | `"tsint"`、省略時は `"v8"`）か、CLI の `--engine` 引数
（`game.json` より優先）で選ぶ。`game.json` の任意フィールド `"pack": {"exclude": [glob...]}` は配布 pak に入れないファイルの指定（`docs/distribution.md` 参照、エンジンは読まない）。ops の実装（`script/engine_state.rs`）は両エンジンで共用しており、
挙動は一致する（V8 側は `#[op2]` の薄いラッパ、tsint 側は `globalThis.Deno.core.ops` へ生やした
ネイティブ関数から、同じ `EngineState` メソッドを呼ぶ）。

```powershell
cargo run -p noiria-app -- games/sample --engine tsint
cargo run -p noiria-app -- games/sample --engine tsint --autotest 16 --autotest-out target/autotest-tsint
```

両エンジンで同一シナリオを `--autotest` 実行し、出力 PNG を比較することで並走検証できる。
加えて、**ops 呼び出し列を突き合わせる自動比較テスト**が `cargo test` に組み込まれている
（`crates/noiria-app/src/script/tag_harness.rs` のタグ振る舞いテストハーネスが同じ .ks 断片を
両エンジンで実行し、`EngineState` の記録した ops 列の完全一致を検証する。
`v8_and_tsint_emit_identical_ops_for_*` テスト参照）。

## 構成

| パス | 内容 |
| --- | --- |
| `crates/noiria-core` | ウィンドウ/スクリプトエンジン非依存の純ロジック（合成・テキスト・音声・セーブ）。ヘッドレステスト可能 |
| `crates/noiria-app` | 実行バイナリ。Win32 API 直叩き (`win32.rs`) + スクリプトエンジン 2 種（`script/`: V8/deno_core と tsint、`--engine` で選択） |
| `crates/noiria-tools` | プレースホルダアセット生成 (`gen-assets`) |
| `runtime/` | エンジン同梱の TypeScript ライブラリ（バイナリに埋め込み。両エンジンで共用） |
| `games/sample/` | 日本語サンプルゲーム |
| `../tsint`（別リポジトリ） | 完全自作 TypeScript インタプリタ。`--engine tsint` で選択（`script/tsint_engine.rs` が統合点） |

## スレッドモデル

- **メインスレッド**: Win32 メッセージループ (`win32.rs`)。`RegisterClassW` + `CreateWindowExW` でウィンドウを作り、`MsgWaitForMultipleObjects` + `PeekMessage` でメッセージをドレインする（アニメ中は約16msタイムアウト、アイドル時は無限待機）。`Arc<Mutex<Scene>>` を毎周期合成して `StretchDIBits` で present。トランジション完了時のページスワップもここ。
- **スクリプトスレッド**: `--engine` で選んだ実装が 1 本の OS スレッド上で動く。
  - `v8`（既定）: tokio(current-thread) + V8。TypeScript を deno_ast でその場変換して実行。
  - `tsint`: 完全自作インタプリタ（別リポジトリ [`tsint`](../tsint)）。tokio を使わず、自前の駆動ループ
    （タイマーヒープ + 入力の非ブロッキングポーリング + トランジション/オーディオのポーリング）で
    非同期 ops を回す（`script/tsint_engine.rs`）。
  
  どちらも ops（`script/engine_state.rs` の `EngineState` メソッド）で Scene を書き換え、
  `WakeSink::Win32`（チャンネル送信 + `PostMessageW(WM_APP)`）でメインループを起こす。
- **オーディオコマンドスレッド** (`noiria-audio`): `AudioCmd` を受けて WAV / OGG(Vorbis) を自作デコーダ（`codec/wav.rs` / `codec/ogg.rs`+`codec/vorbis/`）で全デコードし、`mixer::Mixer`（純 Rust・自作リサンプラ/フェード）へ渡す。BGM/SE/Voice の 3 チャンネル。MP3/FLAC は対応を打ち切っており、読み込もうとするとエラーログを出して無音継続する（panic しない）。
- **オーディオ描画スレッド** (`noiria-audio-render`): `wasapi_out.rs`（WASAPI 共有モード・イベント駆動、OS 境界 API 直叩き）が `Mixer::mix_into` を呼んでデバイスバッファを埋める。COM オブジェクトはこのスレッド内で開いて使い切る（`codec/mf_video.rs` と同じアパートメント規約）。デバイスが無い環境ではコマンドを読み捨てて無音続行する。

## シナリオ (.ks) 対応タグ

`l p r cm ct er delay nowait endnowait image freeimage backlay move wm trans(crossfade/universal/scroll) wt position wait s quake wq playbgm stopbgm fadeoutbgm fadebgm fadeinbgm fadepausebgm pausebgm resumebgm xchgbgm bgmopt setbgmstop clearbgmstop wb wl playse stopse ws fadeinse fadeoutse fadese wf seopt entersebuf playvoice stopvoice jump call return if elsif else endif macro endmacro eval emb link endlink title history cancelskip autowait screenshot close name font resetfont style resetstyle deffont defstyle ruby ch locate indent endindent laycount current layopt hidemessage graph ptext hch autowc wc copylay resetwait button checkbox edit commit input glyph click cclick wheel cwheel timeout ctimeout waitclick waittrig locklink unlocklink clickskip nextskip cancelautomode rclick ask animstart animstop wa stopmove stopquake stoptrans pimage tempsave tempload store disablestore record goback gotostart startanchor locksnapshot unlocksnapshot save load copybookmark erasebookmark clearvar clearsysvar iscript endscript ignore endignore hr hact endhact showhistory trace erasemacro video wv stopvideo openvideo preparevideo playvideo pausevideo resumevideo rewindvideo videoevent cancelvideoevent videosegloop cancelvideosegloop wp`

音声系 (W2): SE 系タグは `buf=`（0..15、既定 0）で多重バッファ再生に対応。`fadebgm` は `time=` でフェード。`xchgbgm` は現 BGM をフェードアウトしつつ新 BGM をフェードイン。`wb`/`wf` はフェード完了待ち、`wl` は BGM 演奏終了（非ループ）待ち（いずれも `canskip=`、既定 true）。

複数メッセージレイヤ (W3): `[laycount messages=N]`（上限 8）でメッセージレイヤを増やし、`[current layer=message1]` でカレントを切り替えると以後の本文・`er`・`position` 等がそのレイヤに作用する（`cm` は全レイヤ、`ct` は全レイヤ + カレントを message0 へ戻す）。`[layopt]` で位置/不透明度/重ね順/表示を個別設定、`[hidemessage]` は全レイヤ一時非表示（次のクリックで復帰）。`[graph storage=xx.png]` はメッセージ本文へのインライン画像（1 グリフ扱い）、`[ptext]` は任意レイヤへの直接テキスト描画。`[autowc]`/`[wc]` は文字単位の自動/手動ウェイト、`[copylay]` はレイヤ間コピー。セーブデータは v2 形式（レイヤ構成込み）になり、v1 セーブも読める（`migrateSaveV1toV2`）。

UI・ボタン・クリック制御 (W4): `[button graphic= target= storage= exp= x= y=]` は正常/ホバー/押下の 3 状態が横に並んだ 1 枚画像を切り替える自作ボタン（新規 op `layer_blit_image` で部分矩形をレイヤへブリット）。`[checkbox name=]` は変数トグル付きの自作矩形+チェックマーク、`[edit name= length=]`/`[commit]` はメッセージレイヤ内の自作テキスト入力欄、`[input title= prompt= initial= name=]` は自作モーダル入力ダイアログ、`[ask text= var=]`（sourceforge 拡張）は自作の「はい/いいえ」ダイアログ。button/checkbox/edit の活性化は `[s]` 停止中のみ（KAG 準拠）。`[click]`/`[wheel]`（各 `[cclick]`/`[cwheel]` で解除）はマウス常設ジャンプフック、`[timeout]`（`[ctimeout]` で解除）はクリック待ち（`[l]`/`[p]`/`[s]`）中のみ発火する 1 回発火型の実時間タイマー（KAG3 準拠、クリック待ち外で満了した場合は次のクリック待ちに入った時点で発火）、`[waitclick]` はクリック 1 回待ち、`[waittrig name=]` は `kag.trigger(name)`（KAG3 準拠、`sys.player.trigger(name)` でも可）で発火するトリガ待ち。`[locklink]`/`[unlocklink]` はリンク・ボタン・クリック送りの一括ロック、`[clickskip]`/`[nextskip]`/`[cancelautomode]` はスキップ/オートモードの制御、`[rclick]` は右クリック動作のカスタム（既定はメッセージ非表示トグル）、`[glyph line= fix=]` はクリック待ち記号のカスタム画像（`fix=false` 既定はクリック待ち開始時のペン位置へ追従、`fix=true` は固定位置）。実装の詳細・設計判断は `docs/implementation-notes.md`「2026-07-11 W4」「2026-07-12」を参照。

システム UI (B2): `runtime/kag/menu.ts`（セーブ/ロード/設定メニュー）と `runtime/kag/ui.ts`（部品・トースト・確認ダイアログ・常設ボタン）。`KagOptions` で次を切り替える（すべて既定は従来どおり）:
`systemButtons: true`（または `{ items?, color?, textColor? }`）でメッセージ窓の直上・右寄せに Q.SAVE / Q.LOAD / SAVE / LOAD / AUTO / SKIP / LOG / MENU を常設（クリック待ち・文字送り・`[s]` のどれでも押せ、AUTO/SKIP は ON を強調。窓を隠す操作・`[layopt visible=false]`・`[sysbar visible=false]` では一緒に隠れ、窓内には入らない）、
`title: { storage, target? }` でメニューに「タイトルへ戻る」を出す、`message: { frame, color, marginX/marginR/marginY/marginB, ... }` と `[position frame= color= marginl= margint= marginr= marginb=]` でメッセージ窓のスキン（`frame` は窓サイズ一致の背景画像で、窓と一体に隠れ・濃さ設定に従う）、
`namePlate: { color?, textColor? }` で名前欄の色、`sound: { hover?, click? }` でリンク/メニューの選択音（未指定は従来の click.wav、hover は無音）。
`[autosave enabled=false]` / `[sysbar visible=false]`（Noiria 拡張）でタイトル画面などのオートセーブ・常設ボタンを止められる（ロードすると有効に戻る）。
セーブデータには保存時刻・本文の抜粋・サムネイル（セーブ領域の `<slot>.png`、新 op `thumbCapture/thumbSave/layer.loadSaveThumb`）・保存時点の履歴を追加で持つ（旧セーブは空欄/空表示で読める）。設計判断は `docs/implementation-notes.md`「2026-11-19 B2」。

トランジション/アニメ/画像操作 (W5、move の時間配分と pimage の属性名は 2026-07-12 に修正): `trans method=scroll`（`from=left/right/top/bottom`、`stay=` は受理のみで描画は区別しない）でスクロールワイプ、`move path=(x1,y1,o1)(x2,y2,o2)...`（`Scene::Waypoint`/`tween_queue` による折れ線区間列。`time=` は各区間の幾何距離に比例して配分、KAG3 の等速移動に準拠）で複数点移動。`[stopmove]`/`[stopquake]`/`[stoptrans]` は各アニメーションの途中停止。`[animstart layer= storage=]`/`[wa]`/`[animstop]` は `.asd`（最小サブセット: `セル番号,graphic,wait` + `loop=`）によるレイヤのフレームアニメーション。`[pimage]` は既存の `layer_blit_image` を再利用した部分画像の重ね描画（`clip=` に加え KAG3 標準の `sx=/sy=/sw=/sh=` も受理）。`[image]` に `clip=`/`flipud=`/`fliplr=`/`grayscale=` を追加。あわせて W3〜W4 由来の `[copylay]` 既知バグ（コピー先メッセージレイヤの位置が恒久的にずれる）を修正。実装の詳細・設計判断は `docs/implementation-notes.md`「2026-07-11 W5」「2026-07-12」を参照。

ストア系・スクリプト系・履歴系・動画拡張 (W6、一部 2026-07-12 に KAG3 準拠へ修正): `[tempsave
place=]`/`[tempload place=]` は place>=0 ならディスク（専用の `"temp{N}"` スロット）、負数なら
メモリ上のみのスナップショット（カレントレイヤの表示内容も含めて復元）。`[record]`/`[goback]`
は巻き戻し履歴リング（`trail`、上限 30 件、`[save]`/`[load]` をまたいで永続化される）。
`[startanchor enabled=]`（既定 true）/`[gotostart]` は専用アンカースロットへの記録/復帰
（`trail` とは独立）。`[store place=]` は変数 (f) のみのシリアライズ保存、`[disablestore
enabled=]` はセーブ/ロード系タグの一時禁止、`[locksnapshot]`/`[unlocksnapshot]` は自動
スナップショット更新の一時禁止。`[save place= ask=]`/`[load place= ask=]` は既存の
save()/load() をタグから呼ぶ（ask=true ではい/いいえ確認）。`[copybookmark src= dest=]`/
`[erasebookmark place=]` はセーブスロット間コピー/真の削除（`sys.saveDelete`）。
`[clearvar]`/`[clearsysvar]` は変数 (f)/システム変数 (sf) の
全消去。`[iscript]`〜`[endscript]` は生テキストを TS として評価（TJS 互換ではない。V8/tsint
両エンジンで動作確認済み）。`[ignore exp=]`〜`[endignore]` は条件付きブロックスキップ、
`[erasemacro name=]` はマクロ削除、`[trace exp=]` はデバッグログ出力。`[hr]` はバックログへの
区切り線挿入、`[hact]`〜`[endhact]` はホットスポット風テキスト（アクションは非対応）、
`[showhistory]` はバックログ画面を開く。`[openvideo]`/`[preparevideo]`（現在の開始フレーム表示）/
`[playvideo]`/`[pausevideo]`/`[resumevideo]`/`[rewindvideo]` は動画再生の詳細制御。
フレーム seek と動画イベントの詳細は後述の「動画」および
`docs/implementation-notes.md`「2026-07-27 B4」を参照。

KAG3 タグ全体（161 種）に対する対応状況の一覧は `docs/kag-tag-compat.md` を参照
（✅ 対応済み 151 + ➖ 対象外 10 = 161、全行対応完了）。

`[move layer=0 x=. y=. opacity=. time=500 accel=0]` はレイヤの位置/不透明度をネイティブ側 (`Tween`) でアニメーションする（KAG 準拠: `accel` は 0=線形, >0=加速, <0=減速）。`[wm]` は進行中の move が全て完了するまで待つ（スキップモード時は即座に完了させる）。

`[name text=アリス]` はメッセージウィンドウの左上に重なる名前欄プレート（専用テキストレイヤ）を表示する。`[name]`（text 省略）で非表示にする。`[cm]`/`[er]` では消えず、明示的に名前を変える（または `[name]` で消す）まで維持される（KAG の慣習）。セーブ/ロードでも現在の名前を復元する。寸法はメッセージウィンドウのフォントサイズから計算する（`runtime/kag/player.ts` の `computeNamePlateCfg`）。

メッセージ本文は `text.rs` の自作レイアウトエンジンで折り返す（1 文字 1 グリフの素朴な折返し）。句読点・閉じ括弧・小書き仮名などが行頭に来る場合はぶら下げ、開き括弧などが行末に来る場合は次行への道連れ、という**日本語禁則処理**を実装済み。

### 動画

`[video storage=xx.avi layer=1 loop=true frame=0 position=0]` / `[wv]` / `[stopvideo]`
に対応する。動画タグの `slot=` は 0 のみ。`frame=` は 0-based 表示フレーム、
`position=` は ms で、両方を指定した場合は `frame=` を優先する。
W6 で `[openvideo]`（読み込みのみ）/`[preparevideo]`（現在の開始フレーム表示）/
`[playvideo]`（準備済み動画の再生開始、または storage= 指定時は即時再生）/`[pausevideo]`/
`[resumevideo]`/`[rewindvideo]`（フレーム 0 への厳密シーク）を追加した
（`EngineState::video_open`/`video_prepare`/`video_play_prepared`/`video_pause`/`video_resume`/
`video_rewind`、既存の単一同時再生モデルを維持したまま最小差分で拡張）。
B4 で `[videoevent frame=N]`、`[videosegloop start=N end=M]` と各 cancel タグ、
`[wp for=loop|period|prepare|segLoop]` を追加した。period event は 1 回発火型、
区間ループは `[start,end)`。`for=` を省略した `[wp]` は任意の動画イベントを待つ。
**AVI (Motion JPEG) を完全自作デコーダで再生**する（JPEG ベースラインデコーダ + AVI/RIFF パーサ、`codec/jpeg.rs` / `codec/video.rs`）。
**VP9 (IVF コンテナ) も完全自作デコーダで再生**する（別リポジトリ [`vp9dec`](../vp9dec) を `path` 依存として取り込み、`codec/video.rs` の `Vp9Ivf` が `VideoSource` を実装。WebM 公式コンフォーマンスベクタで全フレーム MD5 一致済み）。
MP4 (H.264 等、および VP9 以外の IVF) は Windows 上では OS 内蔵の Media Foundation にフォールバックする（`codec/mf_video.rs`。ウィンドウ/音声出力と同格の OS 境界 API 呼び出しで、ffmpeg 等の外部ライブラリは使わない）。
`VideoSource` トレイトはシーケンシャル読み出し (`next_frame`/`rewind`) と
0-based 表示フレームへの `seek_frame` を提供する。MJPEG は索引へ直接移動し、
VP9/MF は rewind + 表示フレーム単位の decode-forward で同じ厳密シーク契約を満たす。
コーデックの追加は `VideoSource` トレイト実装 + `open_video_file()` への 1 分岐で済む設計。

| コーデック | 状態 |
| --- | --- |
| MJPEG (AVI) | **自作済み・稼働中** |
| VP9 (IVF) | **自作済み・稼働中**（別リポジトリ [`vp9dec`](../vp9dec)、WebM 公式コンフォーマンスベクタで全フレーム MD5 一致済み）。IVF コンテナのみ対応（MP4 に包まれた VP9 や、IVF でも VP9 以外の FourCC は非対応）。MF フォールバックは MP4 等の他コンテナ形式用 |
| MPEG-1 | 完全自作デコーダは未対応（次の自作候補。JPEG で作った IDCT/ハフマンを再利用可能、数千行規模）。Windows では Media Foundation が対応していれば再生可（環境依存） |
| H.264 (MP4) | **対応（OS 内蔵デコーダ, Media Foundation 経由）**。フル自作は数万行規模で非現実的なため、Windows のみ OS 境界 API にフォールバックする |

### 音声

`playbgm` / `playse` / `playvoice` (`storage=` に音声ファイルパス) に対応。

| フォーマット | 状態 |
| --- | --- |
| WAV | **自作済み・稼働中**（`codec/wav.rs`。PCM 8/16/24bit + float32） |
| OGG (Vorbis) | **自作済み・稼働中**（`codec/ogg.rs` + `codec/vorbis/`。クリーンルーム実装、Xiph の Vorbis I 仕様書と Ogg 仕様 (RFC 3533) のみ参照） |
| MP3 / FLAC | **非対応（打ち切り）**。読み込もうとすると `音声を再生できません ...: MP3/FLAC は非対応になりました。WAV か OGG (Vorbis) を使ってください` とエラーログを出し、panic せず無音のまま続行する |

WAV・OGG(Vorbis) とも `audio.rs` の `build_source()` がファイル全体を一度にデコードして `mixer::BufferSource`（巻き戻し可能なメモリ上バッファ）に載せ、ループは `mixer::Mixer` 側で行う。ノベルゲームの BGM は長くても 1 分未満・数MB程度のため全デコードで実用上問題ないという判断で、ストリーミングデコードは将来の最適化候補としてコードコメントに残してある（`codec::vorbis::VorbisStreamDecoder` はパケット単位デコードに対応済みなので土台はある）。

## セーブデータ

`%APPDATA%\Noiria\<game-id>\save\` に JSON で保存。ラベル通過時点の状態（位置・変数 f・レイヤ・BGM）を記録し、ロード時にパスから再構築する。システム変数 sf と既読情報は `sys.json`。

## 外部依存の段階的自作化ロードマップ

最終目標は**完全自作・外部非依存**。そのため外部クレートは 1 モジュール 1 依存に隔離してあり、内側の型だけを公開している（差し替えるファイルが 1 つで済む）。

| 依存 | 隔離場所 | 自作化の道筋 | 難度 |
| --- | --- | --- | --- |
| （なし）合成/トランジション/シーン | `noiria-core` | **既に完全自作** | — |
| ~~PNG デコード/エンコード~~ | `codec/png.rs` + `codec/inflate.rs` | **自作済み・稼働中**（デコード: zlib/DEFLATE 展開も自作、image クレートと出力一致をテストで担保。8bit 非インターレースのみ対応で、16bit/インターレース等の非対応 PNG や PNG/JPEG 以外の画像形式は明確なエラーを返す（image クレートへのフォールバックは廃止）。エンコード: `codec::png::encode` が IHDR/IDAT(zlib stored ブロック)/IEND を自作生成、CRC32 も自作（PNG 用多項式 0xEDB88320）。スクリーンショット/セーブサムネイル (`assets::save_png`) で使用、自作デコーダとの roundtrip をテストで担保） | 済 |
| ~~WAV デコード~~ | `codec/wav.rs` | **自作済み・稼働中**（PCM 8/16/24bit + float32。ゲームの .wav 再生は自作デコーダ経由）。**pak 軽量化（W9 + LPC 拡張）**: 16bit PCM/1-2ch の `.wav` は pack 時に自作 FLAC 風サブセットコーデック（fixed predictor 0-4 + LPC 1-8（Levinson-Durbin、整数量子化係数）+ Rice 符号、`codec/flac_subset.rs`＝デコード/`noiria-tools/flac_enc.rs`＝エンコードへ意図的に分離実装）で可逆圧縮し pak へ格納、読込時に元 WAV バイト列を完全復元する（`games/sample` 実測 -42.7%、LPC 拡張前は -42.2%）。外部依存はゼロ（元々自作のため本表の「自作化」対象ではないが、pak の軽量化施策として付記） | 済 |
| ~~JPEG デコード~~ | `codec/jpeg.rs` | **自作済み・稼働中**（ベースライン。動画 MJPEG に加え、静止画 assets 経路 (`assets::load_image_rgba`) にも接続済み。PNG/JPEG 以外の形式は明確なエラーを返す） | 済 |
| WebP (VP8L ロスレス) デコード/エンコード | `codec/webp.rs` | **外部依存（隔離・将来自作置換候補）**: `image-webp`（image-rs 公式の純 Rust クレート、`#![forbid(unsafe_code)]`、C 依存なし）を 1 モジュールに隔離して使用（公開 API は `decode_rgba`/`encode_lossless` の 2 関数のみ、`image_webp` の型は外へ漏らさない）。`assets::decode_image_bytes` は拡張子ではなく RIFF/WEBP マジックバイトで判定。`noiria-pack`/`noiria-export` が `.png` エントリを WebP ロスレスへ変換し、元 PNG より小さい場合だけ pak に採用する（パス・拡張子は `.png` のまま。`--no-webp` で無効化可）。推移依存は `byteorder-lite`/`quick-error` の純 Rust 2 パッケージのみ | 隔離済（自作 VP8L は将来候補） |
| ~~動画 (MJPEG/AVI)~~ | `codec/video.rs` | **自作済み・稼働中**（`VideoSource` トレイトが追加コーデックの差し込み口） | 済 |
| ~~動画 (VP9/IVF)~~ | `codec/video.rs` (`Vp9Ivf`) + 別リポジトリ [`vp9dec`](../vp9dec) | **自作済み・稼働中**（WebM 公式コンフォーマンスベクタで全フレーム MD5 一致済み。`path` 依存として取り込み、純 Rust・依存クレートゼロなので自作化ロードマップの精神と矛盾しない） | 済 |
| `windows` (Media Foundation) | `codec/mf_video.rs` | H.264/MP4 等（および VP9 以外の IVF）の動画再生・(テスト用) エンコード専用。**OS 境界 API**（ウィンドウ/音声出力と同格の扱い、ユーザー承認済み）であり自作化ロードマップの対象外。ffmpeg 等の外部ライブラリではない | — |
| ~~`rodio` + cpal~~ | `audio.rs` (`AudioCmd` 境界) | **音声デコード 自作済み・稼働中（WAV+OGG/Vorbis。MP3/FLAC は打ち切り）**。音声出力は WASAPI 直叩き + 自作ミキサ/リサンプラ（`wasapi_out.rs` が OS 境界 API、`mixer.rs` が純 Rust・OS 非依存の 3ch ミキサ＋線形補間リサンプラ＋チャンネル変換＋フェード）。**Vorbis デコーダ自作済み**（`codec/ogg.rs` + `codec/vorbis/`。クリーンルーム実装、Xiph の Vorbis I 仕様書 (<https://xiph.org/vorbis/doc/Vorbis_I_spec.html>) と Ogg 仕様 (RFC 3533) のみ参照、libvorbis/lewton 等の既存実装は不参照。Ogg ページング・CRC32・コードブック(カノニカルハフマン+VQ lookup 0/1/2)・Floor type 1・Residue type 0/1/2・channel coupling・逆 MDCT+Vorbis 窓+オーバーラップ加算を実装。V1 で rodio 0.22 (バックエンドは symphonia-codec-vorbis) をテスト内の参照デコーダとして使い、実 OGG ファイル (mono 2 本・stereo 1 本、`tests/vectors_audio/`) でクロスチェックし相関 1.0・最大誤差 1e-7 オーダーで一致することを確認済み）。**V2: `audio.rs` の `build_source()` を自作デコーダ経由に置き換え、`rodio`/`symphonia` 系クレート (15 パッケージ) を依存から完全削除済み**（`cargo tree` で rodio/symphonia が消えたことを確認済み。MP3/FLAC は対応打ち切り、読み込もうとするとエラーログを出して無音継続。`tests/vorbis_crosscheck.rs` は rodio 参照デコーダが使えなくなったため、削除前に rodio で取得した参照スナップショット (チャンネル数・サンプルレート・全体 RMS・1 秒毎チャンク RMS) を定数として焼き込んだ回帰テストに切り替え） | 済 |
| ~~`cosmic-text`~~ | `text.rs` (`RasterText` 境界) | **自作済み・稼働中**（TTF/TTC パーサ + AA ラスタライザ + レイアウト + 禁則処理。シェーピング/リガチャ/ルビは未対応）。**F1: TTF パーサ自作済み**（`font/` モジュール。sfnt 単体・TTC コレクション対応、CFF/'OTTO' は非対応を明示 Err、head/maxp/cmap(format4+12)/loca/glyf/hhea/hmtx/OS-2/name を境界チェック付きで解析し、単純/複合グリフを線分+2次ベジェのアウトラインへ正規化。手組み TTF バイト列での既知値一致テストと実システムフォント (msgothic.ttc / YuGothM.ttc・meiryo.ttc / arial.ttf) でのスモークテスト、LCG 破壊 fuzz テスト 1000 件で検証済み）。**F2: スキャンライン AA ラスタライザ自作済み**（`font/raster.rs`。Y方向16分割サブスキャンライン + X方向解析的カバレッジのハイブリッド方式、2次ベジェは曲率に応じた適応分割、nonzero winding rule でホール（'A' の内側等）も正しく処理。ヒンティングは意図的に非実装（AAのみで十分な可読性、理由はモジュール冒頭コメント参照）。合成アウトラインのゴールデンテスト（面積一致・ドーナツ穴・AA階調）で検証済み。`GlyphCache`（LRU）も用意済み）。**F3: `text.rs` への統合完了・`cosmic-text` を依存から実削除済み**（`cargo tree` で cosmic-text/fontdb/swash が依存ツリーから消えたことを確認済み）。`font/registry.rs` が `C:\Windows\Fonts` を起動時 1 回走査して family 名（ローカライズ名含む）→ (パス, TTC index) を索引し、要求 family → Yu Gothic UI → Yu Gothic → Meiryo → MS Gothic → レジストリ内の任意フォントの順でフォールバック（文字単位のグリフフォールバックにも対応）。レイアウトは 1 文字 1 グリフの素朴な折返し（シェーピング/リガチャ非対応）で、**日本語禁則処理**を実装（行頭禁則はぶら下げ方式、行末禁則は道連れ方式。詳細は `text.rs` 冒頭コメント参照）。`font::rasterize` を直接呼んだ結果と `TextEngine` 経由の結果が完全一致することを検証する自己無矛盾性テストあり | 済 |
| ~~ウィンドウ~~ (`winit` + `softbuffer`) | `win32.rs` | **自作済み・稼働中**（Win32 API 直叩き: `RegisterClassW`+`CreateWindowExW`+`StretchDIBits`、メッセージループは `MsgWaitForMultipleObjects`+`PeekMessage`。**可視ウィンドウでの最終確認は未実施（ユーザーの実機確認待ち）**。不可視ウィンドウ統合テストと `cargo test`/`--autotest` のみで検証済み） | 済 |
| ~~`tokio`~~（tsint エンジン選択時のみ） | `script/tsint_engine.rs` | **自作済み・稼働中**（tsint エンジンは tokio 非依存。自前のタイマーヒープ + 入力ポーリングの駆動ループで非同期 ops を回す） | 済（tsint 選択時） |
| `deno_core` (V8) + `deno_ast` | `script/`（V8 経路, ops 境界） | 自作 TS インタプリタ（別リポジトリ [`tsint`](../tsint)）を `--engine tsint` で選択可能になった（T5b）。ops（`script/engine_state.rs`）は両エンジンで共用し、挙動が一致することを確認済み。**既定は当面 V8 のまま**（V8 と tsint の並走検証期間。deno_core への完全置換・削除は将来のステップ） | 最高（並走検証中） |

残りの推奨順序: ~~音声出力(WASAPI+自作ミキサ)~~ (済) → ~~フォント(TTFパーサ+ラスタライザ+レイアウト+禁則)~~ (済) → ~~スクリプトエンジン(自作TSインタプリタ、`--engine tsint` で選択可)~~ (済、並走検証中) → V8/deno_core 経路の完全置換・削除（将来）。
`ops` の関数表（`runtime/api.ts` が呼ぶ面）がエンジンの安定 ABI なので、どの層を差し替えてもゲームとタグシナリオフレームワークはそのまま動く。

## テスト・ベンチマーク

```powershell
cargo test                                          # Windows 763 passed（ignored 5、2026-11-20）
cargo run --release -p noiria-core --example bench  # 合成ホットパスの計測
cargo clippy --workspace --all-targets -- -D warnings  # 警告 0 が基準（feature 構成ごとは scripts/feature-build-matrix.ps1）
```

PowerShell で cargo を実行するときは、作業ディレクトリのドライブ文字を大文字（`Set-Location 'C:\...\noiria'`）にすること。
小文字の `c:\` や Git Bash の cwd だと v8 のビルドスクリプトが別ドライブと誤認して symlink 作成（権限エラー 1314）で panic する。

テストには V8/tsint の ops・KAG タグ挙動比較、✅151 タグのカバレッジ照合、
動画の厳密 seek/event/区間ループと競合回帰、各コーデック、および OS 別の
Win32/WASAPI/Media Foundation・X11/ALSA 経路を含む。

1280x720 での合成コスト実測（release, 60fps 予算 16.6ms に対して）:

| 経路 | ms/frame |
| --- | --- |
| 通常合成（背景+立ち絵2+メッセージ） | 約 2.4 |
| クロスフェード中 | 約 0.9（ページ合成を revision キャッシュ、毎フレームはブレンドのみ） |
| ルール画像トランジション中 | 約 1.5（内側ループ整数化済み） |

不透明ピクセルのコピー高速パスにより背景 blit は約 0.2ms。
