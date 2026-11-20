# KAG3 タグ対応表

Noiria の `.ks` シナリオパーサ・`KagPlayer`（`runtime/kag/`）が対応する KAG3 タグの一覧。
「KAG3 タグ全対応」計画のパート A として、実装状況を可視化するために作成した。

## 集計方法・出典

- タグ全数は次の 2 つのソースの **和集合** を正とする。
  1. 詳細設計書（`breezy-sauteeing-fox-agent-a1d3d2a0d7eee225c.md`）付録に転記済みの統合リスト
     （krkr2doc Tags.html カテゴリ別索引 + kirikirikag.sourceforge.net キーワード索引のマージ、158 種）。
  2. ユーザー指定の確定版ウェーブ割当（本ドキュメントの元になった作業指示に列挙された、
     ✅/🔨/⬜/➖ の 8 グループ、161 種）。
  - 突き合わせた結果、(1) は (2) の**厳密な部分集合**であり、(1) には `locate` / `macro` / `position`
    の 3 タグが漏れていた（設計書ドラフト時点の転記漏れ。特に `locate` は今回実装した W1 タグ自身、
    `macro`/`position` は既存対応済みタグであり、いずれも実際には Tags.html 索引に存在する）。
    そのため和集合は (2) と同じ **161 種**になる。
  - 出典列が `sourceforge` のタグ（`ask`／`entersebuf`）は krkr2doc の索引には無く、
    kirikirikag.sourceforge.net 版のみに存在する拡張タグ。それ以外は全て `krkr2doc`（krkr2doc 索引に
    存在し、sourceforge 版キーワード索引とも重複する主要タグ）。
- ステータス:
  - ✅ 対応済み: 既存実装（ウェーブ実装以前から対応。「(W2 拡張)」「(W3 拡張)」はそのウェーブで
    属性を拡張したもの）。
  - 🔨→✅ W1: W1（文字装飾系）で新規実装。
  - 🔨→✅ W2: W2（音声系）で新規実装。
  - 🔨→✅ W3: W3（複数メッセージレイヤ）で新規実装。
  - 🔨→✅ W4: W4（UI・ボタン・クリック制御）で新規実装。
  - 🔨→✅ W5: W5（トランジション/アニメ/画像操作）で新規実装。
  - 🔨→✅ W6: W6（ストア系+動画拡張+残余）で新規実装。**これで全ウェーブが完了し、161 行すべてが
    ✅ または ➖ になった**。
  - ➖ 対象外: 実装しない方針（理由は備考欄）。
- Noiria 独自拡張タグ（`playvoice` `stopvoice` `autowait` `screenshot` `name`）は KAG3 標準タグ一覧に
  無いため、この表には含めない。ドキュメント末尾の別セクションに記載する。

## 対応表（161 タグ、五十音/アルファベット順）

| タグ | 状態 | ウェーブ/分類 | 出典 | 備考 |
| --- | --- | --- | --- | --- |
| `animstart` | 🔨→✅ | W5 (今回実装) | krkr2doc | `layer= page= storage= left=/x= top=/y=`。`storage=` は .asd アニメ定義ファイル（`assets/fg/` 相対）。**最小サブセット**（`セル番号,graphic,wait` の行 + `loop=true` または `loop=false` + `;` コメント/空行のみ）をパースし、`layer.loadImage` の差し替え + `sys.wait` ループで再生する（Rust 無変更、`runtime/kag/anim.ts` の `AnimManager`）。`seg=`/`module=` 等の条件分岐を伴うフル .asd 構文は対象外。 |
| `animstop` | 🔨→✅ | W5 (今回実装) | krkr2doc | 指定レイヤの `[animstart]` ループを停止する。 |
| `ask` | 🔨→✅ | W4 (今回実装) | sourceforge | `text= var=`。自作の「はい/いいえ」オーバーレイ（player.ts `askYesNo`）。**任意の 1 クリックで必ず閉じる**（「はい」の行に厳密ヒットしなければ既定で「いいえ」扱い）設計にした。行位置を厳密ヒットテストするだけだと呼び出し側の入力座標が分からない自動化（`--autotest` の固定クリック座標等）でハングするため（出典: sourceforge。krkr2doc 索引には無い拡張タグ）。 |
| `autowc` | 🔨→✅ | W3 (今回実装) | krkr2doc | `enabled= ch= time=` で指定文字の後に自動ウェイトを挿入。TS 側 (`printTextWithAutoWait`) で分割 print + `sys.wait` するだけで Rust の reveal は無変更。 |
| `backlay` | ✅ | 対応済み | krkr2doc | fore→back へレイヤ内容をコピー。tags.ts backlay、api.layer.backlay()。 |
| `bgmopt` | 🔨→✅ | W2 (今回実装) | krkr2doc | volume= をグローバル BGM 音量（settings.bgmVol、sf.config 永続化）へ反映。volFor() が一元適用するため二重掛けなし。volume 以外の属性は無視。 |
| `button` | 🔨→✅ | W4 (今回実装) | krkr2doc | `graphic= target= storage= exp= x= y=`。正常/押下/ホバーが横3分割された1枚画像を切替表示する自作ボタン（`buttons.ts` の `ButtonManager`、新規 op `layer_blit_image` で状態スライスをブリット）。`[s]` 停止中のみ活性。countpage 等の高度な属性は対象外。 |
| `call` | ✅ | 対応済み (B1 で欠落時の挙動を追加) | krkr2doc | サブルーチン呼び出し。tags.ts call、KagPlayer.pushCall。欠落時は `jump` と同じ（戻り先は積まれない）。 |
| `cancelautomode` | 🔨→✅ | W4 (今回実装) | krkr2doc | オートモードを解除する（既存の A キートグルと同じ `mode.auto` フラグを直接 false にする）。 |
| `cancelskip` | ✅ | 対応済み | krkr2doc | スキップモード強制解除。tags.ts cancelskip。 |
| `cancelvideoevent` | 🔨→✅ | B4 (今回実装) | krkr2doc | slot=0 の未発火 period event を解除し、既存 latch も破棄する。 |
| `cancelvideosegloop` | 🔨→✅ | B4 (今回実装) | krkr2doc | slot=0 の区間ループを解除し、現在位置から通常終端へ再生を続ける。 |
| `cclick` | 🔨→✅ | W4 (今回実装) | krkr2doc | `[click]` の常設クリックジャンプフックを解除する。 |
| `ch` | 🔨→✅ | W1 (今回実装) | krkr2doc | text= の先頭 1 コードポイントのみを印字するエイリアス。サロゲートペア対応。 |
| `checkbox` | 🔨→✅ | W4 (今回実装) | krkr2doc | `name= x= y=`。`f./sf.` 変数式をクリックでトグルする自作チェックボックス（角丸矩形 + "✓" の text レイヤ）。`[s]` 停止中のみ活性。 |
| `clearbgmlabel` | ➖ | 対象外 | krkr2doc | setbgmlabel の解除。同上、対象外。 |
| `clearbgmstop` | 🔨→✅ | W2 (今回実装) | krkr2doc | setbgmstop の予約解除。KagPlayer.clearBgmStop（世代カウンタで監視ループも消滅）。 |
| `clearsysvar` | 🔨→✅ | W6 (今回実装) | krkr2doc | システム変数 (sf) を全消去（KAG 準拠）。直後に `readLabels`/`settings`（sf.config）既定値・`trail` を再適用する（設計判断、`resetSysDefaults()`。理由: 再適用しないと未読判定やボリューム設定表示が壊れた状態のまま残るため）。 |
| `clearvar` | 🔨→✅ | W6 (今回実装) | krkr2doc | 通常変数 (f) を全消去。`vars.replaceF({})`。 |
| `clearvideolayer` | ➖ | 対象外 | krkr2doc | videolayer のクリア。同上、代替済みのため対象外。 |
| `click` | 🔨→✅ | W4 (今回実装) | krkr2doc | `storage= target=`。マウスクリックで target へジャンプする常設フック（player.ts `eventJumps.click`）。`[cclick]` で解除。フックは常にジャンプで、`[jump]` タグと同じくメッセージを消す（B1 で方針を明文化）。 |
| `clickskip` | 🔨→✅ | W4 (今回実装) | krkr2doc | `enabled=`（既定 true）。false の間はクリックによるスキップモード解除を禁止する（S キーでの明示解除は引き続き可能）。実装解釈は `docs/implementation-notes.md` 参照。 |
| `close` | ✅ | 対応済み (B1 で保存を追加) | krkr2doc | アプリケーション終了。tags.ts close → `KagPlayer.quitGame()`（読み終えたラベル区間を既読にして sf を保存してから `api.sys.quit()`）。 |
| `cm` | ✅ (W3 拡張) | 対応済み | krkr2doc | 全メッセージレイヤをクリア。tags.ts cm → `clearAllMessages()`。W3 で ct/er と実装を分離（既定 1 レイヤ構成では見た目は無変更）。 |
| `commit` | 🔨→✅ | W4 (今回実装) | krkr2doc | フォーカス中の `[edit]` の内容を変数へ確定する（`ButtonManager.commitEdit`）。 |
| `copybookmark` | 🔨→✅ | W6 (今回実装) | krkr2doc | `src= dest=`（save/load タグと同じ `save{N}` 命名のスロット番号）。`sys.saveRead`/`sys.saveWrite` でスロット間コピー。`[disablestore]` 中は禁止。 |
| `copylay` | 🔨→✅ | W3 (今回実装、W5 でバグ修正) | krkr2doc | `srclayer= destlayer= srcpage= destpage=` でレイヤ間コピー。新規 op `layer_copy`（`Scene::copy_layer`）。z・進行中アニメーションはコピー対象外。コピー先がメッセージレイヤの場合、ホバー/リンク判定用ミラーは同期されない（次の cm/er まで無効）。**既知バグ（W4 で発見、W3 由来）を W5 で修正済み**: `Scene::copy_layer` がコピー先の x/y（ネイティブ位置）も無条件でコピーしていたため、`destlayer=` にメッセージレイヤを指定すると表示位置がコピー元の位置へ恒久的に変わる不具合があった。`copy_position` 引数を追加し、コピー先がメッセージレイヤの場合は位置を非コピーにする（内容は従来通りコピーする）ことで解消した。一般レイヤ間のコピーは従来通り位置もコピーする。`docs/implementation-notes.md`「2026-07-11 W5」参照。 |
| `ct` | ✅ (W3 拡張) | 対応済み | krkr2doc | 全メッセージレイヤをクリアし、カレントを message0/fore へ戻す。tags.ts ct → `clearAllMessages()` + `current.index=0`。 |
| `ctimeout` | 🔨→✅ | W4 (今回実装) | krkr2doc | `[timeout]` の予約を解除する。 |
| `current` | 🔨→✅ | W3 (今回実装) | krkr2doc | `layer= page=` でカレントメッセージレイヤを切替。以後の文章表示・cm 以外のメッセージ操作はカレントに作用。page= は受理するが見た目には影響しない（テキストレイヤは常に fore 描画のため、備考「page 属性の制限」参照）。 |
| `cursor` | ➖ | 対象外 | krkr2doc | マウスカーソル画像の切替。ヘッドレス検証（cargo test / --autotest）では見えず実利が薄いため対象外。 |
| `cwheel` | 🔨→✅ | W4 (今回実装) | krkr2doc | `[wheel]` の常設ホイールジャンプフックを解除する。 |
| `deffont` | 🔨→✅ | W1 (今回実装、face= は B1) | krkr2doc | resetfont の戻り先デフォルト値を更新（`face=` を含む）。tags.ts deffont。 |
| `defstyle` | 🔨→✅ | W1（align/linespacing は B13） | krkr2doc | `resetstyle` の戻り先を更新。文字装飾の既定値に加え、カレントメッセージレイヤの `align=left/center/right` と `linespacing=`（signed px）の既定値を保持する。現在値は即時変更せず、`resetstyle` で適用する。 |
| `delay` | ✅ | 対応済み | krkr2doc | 文字送り速度設定（nowait/user/固定値）。tags.ts delay。 |
| `disablestore` | 🔨→✅ | W6 (今回実装) | krkr2doc | `enabled=`（既定 true）。true の間 save/load/tempsave/tempload/store/copybookmark/erasebookmark/goback/gotostart を一時禁止する（`KagPlayer.guardStore()`、warn して no-op）。`[locksnapshot]`（onLabel の自動スナップショット更新の抑制）とは独立レイヤで併用可能（`docs/implementation-notes.md`「2026-07-11 W6」参照）。 |
| `edit` | 🔨→✅ | W4 (今回実装) | krkr2doc | `name= length= x= y=`。メッセージレイヤ内の自作テキスト入力欄（独立 text レイヤとして重ね描画）。`[commit]`/Enter で確定。**IME 非対応**（ネイティブ側 (win32.rs) の `WM_CHAR` は変換確定後の 1 文字ずつしか届かないため、変換候補選択中の表示は扱わない）。`[s]` 停止中のみ活性。 |
| `else` | ✅ | 対応済み | krkr2doc | 条件分岐の else 節。tags.ts else → skipToEndif。 |
| `elsif` | ✅ | 対応済み | krkr2doc | 条件分岐の elsif 節。tags.ts elsif → skipToEndif。 |
| `emb` | ✅ | 対応済み | krkr2doc | 式展開結果を本文へ埋め込む。tags.ts emb → vars.evalExpr + printText。 |
| `endhact` | 🔨→✅ | W6 (今回実装) | krkr2doc | `[hact]` ブロック終端。no-op（本文はそのまま通常の printText 経路で表示・バックログ記録される）。 |
| `endif` | ✅ | 対応済み | krkr2doc | if ブロック終端。tags.ts endif（no-op）。 |
| `endignore` | 🔨→✅ | W6 (今回実装) | krkr2doc | `[ignore]` ブロック終端。exp= が偽なら通過するだけ（真の場合は `skipToEndignore` が直接ここまで飛ぶため実行されない）。 |
| `endindent` | 🔨→✅ | W1 (今回実装) | krkr2doc | indent を解除（行頭 x を 0 に戻す）。text.rs の CharControl::IndentClear。 |
| `endlink` | ✅ | 対応済み | krkr2doc | 選択肢リンク終端。tags.ts endlink → KagPlayer.endLink。 |
| `endmacro` | ✅ | 対応済み | krkr2doc | マクロ定義終端。captureMacro が消費するため実行時には来ない。 |
| `endnowait` | ✅ | 対応済み | krkr2doc | 瞬間表示モード解除。tags.ts endnowait。 |
| `endscript` | 🔨→✅ | W6 (今回実装) | krkr2doc | `[iscript]` ブロック終端。parser.ts が `[iscript]`〜`[endscript]` 間を生テキストとして収集し 1 コマンドへ折り畳むため、実行時には来ない（`macro`/`endmacro` と同型。壊れたスクリプト向けの安全網として no-op ハンドラも登録）。 |
| `entersebuf` | 🔨→✅ | W2 (今回実装) | sourceforge | 受理して no-op。Noiria の SE バッファは buf= 指定で動的解決（上限 16 固定確保済み）のため事前確保が不要（出典: sourceforge。krkr2doc 索引には無い拡張タグ）。 |
| `er` | ✅ (W3 拡張) | 対応済み | krkr2doc | カレントメッセージレイヤのみクリア。tags.ts er → `clearMessage()`（カレント版）。W3 で cm/ct と実装を分離。 |
| `erasebookmark` | 🔨→✅ | W6 (今回実装、W7 で真の削除に修正) | krkr2doc | `place=`（`save{N}` スロット）。新規 op `sys.saveDelete`（`engine_state.rs`/`ops_sys.rs`/`tsint_engine.rs`、`api.ts`/`noiria.d.ts` に配線、V8/tsint 両対応）でスロットファイルを実削除する（存在しなければ no-op）。削除後はセーブメニューが「（空きスロット）」表示に戻る（旧実装は空 JSON 書き込みで代替していたため戻らなかった。W7 で修正、`docs/implementation-notes.md`「2026-07-12」参照）。`[disablestore]` 中は禁止。 |
| `erasemacro` | 🔨→✅ | W6 (今回実装) | krkr2doc | `name=`。`KagPlayer.macros` から該当マクロを削除（`Map.delete`、存在しない名前は無害な no-op）。 |
| `eval` | ✅ | 対応済み | krkr2doc | 式を評価（結果は表示しない）。tags.ts eval → vars.evalStmt。 |
| `fadebgm` | ✅ | 対応済み (W2 拡張) | krkr2doc | BGM 音量フェード。W2 で time= に対応（api.audio.fadeTo）。time 省略/0 は従来通り即時変更。 |
| `fadeinbgm` | 🔨→✅ | W2 (今回実装) | krkr2doc | storage= time=(既定1000) loop=(既定true) volume= でフェードイン再生。 |
| `fadeinse` | 🔨→✅ | W2 (今回実装) | krkr2doc | SE フェードイン再生。buf=(既定0) 対応。 |
| `fadeoutbgm` | ✅ | 対応済み | krkr2doc | BGM フェードアウト停止。tags.ts fadeoutbgm。 |
| `fadeoutse` | 🔨→✅ | W2 (今回実装) | krkr2doc | SE フェードアウト停止。buf= 対応。 |
| `fadepausebgm` | 🔨→✅ | W2 (今回実装) | krkr2doc | time= でフェードアウト後に一時停止（stop ではなく再生位置保持）。復帰は [resumebgm]（音量も復元）。 |
| `fadese` | 🔨→✅ | W2 (今回実装) | krkr2doc | SE を volume= へ time= かけてフェード。buf= 対応。 |
| `font` | 🔨→✅ | W1 (今回実装、edgecolor= は W7 配線) | krkr2doc | 文字装飾状態 (currentFontState: size/bold/color/edgeColor/face) を部分適用。`face=` は B1 で追加（そのセグメントだけ基準フォントを差し替える `TextSegment.font_family`。名前が合う同梱フォントを優先、合わなくても同梱フォントがあればシステムフォントより優先）。tags.ts font、hex カラーは KagPlayer.hexColor。`edgecolor=0xRRGGBB` は Rust 側 (`TextSegment.edge_color`) が W1 で実装済みだったが tags.ts への配線が漏れていたため W7 で `applyFontAttrs` に 1 行追加して接続した。 |
| `freeimage` | ✅ | 対応済み | krkr2doc | 画像レイヤ解放。tags.ts freeimage。 |
| `glyph` | 🔨→✅ | W4 (今回実装、fix= は W7 で実装) | krkr2doc | `line= fix= left= top=`。クリック待ち記号のカスタム画像（`[l]`/`[p]` の非スキップ・非オート待ちで表示）。line 省略で既定（無表示）に戻す。`fix=false`（既定、KAG3 準拠）はクリック待ち開始時に `api.text.rects` の最終グリフ矩形からペン位置を計算し、その右隣へグリフレイヤを配置する「ペン位置追従」（本文が無い場合はメッセージウィンドウ左上マージン位置へフォールバック）。`fix=true` は従来通りの固定位置（left=/top= 省略時はメッセージウィンドウ右下）。 |
| `goback` | 🔨→✅ | W6 (今回実装、trail 永続化は W7) | krkr2doc | 直前の `[record]` 位置へ戻るクイックバック。履歴リング (`KagPlayer.trail`、上限 30 件) の末尾を消費（pop）して復元する（`pos.index` による厳密な再開 + カレントレイヤ表示内容の復元、下記 `tempsave` 参照）。記録が無ければ warn。`[disablestore]` 中は禁止。`trail` は `[save]`/`[load]` をまたいでセーブデータへ永続化される（W7、`docs/implementation-notes.md`「2026-07-12」参照）。`[startanchor]`/`[gotostart]` とは独立（W7 で分離、下記 `gotostart` 参照）。 |
| `gotostart` | 🔨→✅ | W6 (今回実装、W7 で KAG3 準拠に修正) | krkr2doc | `[startanchor]`（既定 enabled=true）で記録した専用アンカースロット（完全スナップショット）へ戻る（消費しない）。`[startanchor enabled=false]` はアンカーを破棄する。KAG3 準拠（W7 で修正、旧実装は `[record]`/`trail[0]` を流用していた。`docs/implementation-notes.md`「2026-07-12」参照）。アンカー未設定なら warn。`[disablestore]` 中は禁止。`[record]`/`[goback]` の trail とは独立。 |
| `graph` | 🔨→✅ | W3 (今回実装) | krkr2doc | `storage=` でメッセージ内インライン画像（1 グリフ扱い: advance=画像幅、ベースライン下端揃え、行高は不変ではみ出し許容）。`TextSegment.inline_image` + `TextEngine` 画像キャッシュ。ファイル I/O は `EngineState::text_print` 側で実施（text.rs は assets 非依存のまま）。 |
| `hact` | 🔨→✅ | W6 (今回実装) | krkr2doc | 簡易対応: テキストのみバックログへ記録される（本文は通常の printText 経路をそのまま通る）。クリッカブルなアクション自体は非対応（インスタンスごと初回のみ warn。`button`/`link` で代替可能）。 |
| `hch` | 🔨→✅ | W3 (今回実装) | krkr2doc | 縦中横。縦書きモード自体が無いため**平文フォールバック**（`text=` をそのまま印字、インスタンスごと初回のみ warn ログ）。 |
| `hidemessage` | 🔨→✅ | W3 (今回実装) | krkr2doc | 全メッセージレイヤを一時非表示。次の左クリックで復帰（既存の右クリック非表示と同じ `msgVisible` フラグを共有、名前欄プレートも連動）。 |
| `history` | ✅ | 対応済み | krkr2doc | バックログ記録の on/off。tags.ts history → backlogEnabled。 |
| `hr` | 🔨→✅ | W6 (今回実装) | krkr2doc | 現在の段落を確定してからバックログへ区切り線（罫線文字列）を 1 エントリとして追加する（`KagPlayer.insertBacklogRule()`）。 |
| `if` | ✅ | 対応済み | krkr2doc | 条件分岐開始。tags.ts if → vars.evalExpr + skipToNextBranch。 |
| `ignore` | 🔨→✅ | W6 (今回実装) | krkr2doc | `exp=` が真ならブロックを `[endignore]` まで読み飛ばす（`skipToNextBranch` と同型の走査。`if`/`ignore` を同じ depth に含める入れ子カウント、純関数 `scanIgnoreBlock` で実装・単体テスト済み）。KAG3 本来の「入力の一時無視」ではなく「条件付きスキップブロック」として実装（`docs/implementation-notes.md`「2026-07-11 W6」参照）。 |
| `image` | ✅ (W5 拡張) | 対応済み | krkr2doc | 画像レイヤへ画像を読み込み・配置。tags.ts image。W5 で `clip=`（`"x,y,w,h"` 形式、`extract_region` で部分矩形を抽出）・`flipud=`（上下反転）・`fliplr=`（左右反転）・`grayscale=`（グレースケール化、ITU-R BT.601 近似係数）に対応。いずれか指定時のみ拡張ロード `layer.loadImageEx`（新規 op `layer_load_image_ex`）を使い、未指定時は従来通り `layer.loadImage`。適用順は clip 抽出 → fliplr → flipud → grayscale。 |
| `indent` | 🔨→✅ | W1 (今回実装) | krkr2doc | 現在のペン x を以降の折返し行頭として記憶。text.rs の CharControl::IndentSet。 |
| `input` | 🔨→✅ | W4 (今回実装) | krkr2doc | `title= prompt= initial= name=`。自作モーダルテキスト入力（player.ts `openInputDialog`）。**任意のクリックでも「現在の内容で確定」扱いにする**（`--autotest` 等、位置を把握しない自動クリックでもハングしない設計。IME 非対応は `edit` と同様）。 |
| `iscript` | 🔨→✅ (制限付き) | W6 (今回実装) | krkr2doc | `[iscript]`〜`[endscript]` 間の生テキストを 1 コマンドとして収集（parser.ts）し、実行時に `vars.evalStmt`（`new Function` 相当）で **TS として評価**する（TJS 互換ではない）。tsint も `Function` コンストラクタを実行時コンパイルで提供済みと実装着手前にソース確認・実機検証の両方で確認したため、V8/tsint 両エンジンで同一の経路。既存の `eval`/`emb` タグと同じ信頼モデル（シナリオ作者を信頼）。 |
| `jump` | ✅ | 対応済み (B1 で欠落時の挙動を追加) | krkr2doc | シナリオジャンプ。tags.ts jump → requestJump。存在しないシナリオ/ラベルへの jump/call は error ログ + メッセージ窓に「シナリオ 'x.ks' が見つかりません」等を出してクリック待ち→その位置で停止（`[s]` 相当、メニューからのロード等で再開可）。次のコマンドへ落下実行しない。 |
| `l` | ✅ | 対応済み | krkr2doc | 行末クリック待ち（改行なし）。tags.ts l → waitAdvance。 |
| `laycount` | 🔨→✅ | W3 (今回実装) | krkr2doc | `messages=N` でメッセージレイヤ数を変更（既定 1、**上限 8** にクランプ。実装上の上限、`docs/implementation-notes.md` 参照）。減分時にカレントが範囲外になると message0 へ戻る。 |
| `layopt` | 🔨→✅ | W3 (今回実装) | krkr2doc | `layer= page= visible= opacity= left= top= index=`。メッセージレイヤ/前景レイヤ両対応（メッセージレイヤ名にマッチしなければ前景レイヤの汎用プロパティ設定にフォールバック）。`autohide` は W3 範囲外のため対象外。page= はメッセージレイヤに対しては効かない（fore 描画固定）。 |
| `link` | ✅ | 対応済み | krkr2doc | 選択肢リンク開始。tags.ts link → beginLink。 |
| `load` | 🔨→✅ | W6 (今回実装) | krkr2doc | `place= ask=`。`place` を `save{N}` スロット名へマップし、既存の `KagPlayer.load()` をそのまま呼ぶ（F9/メニューと共通経路）。`ask=true` は W4 の `askYesNo` で確認してから実行。`[disablestore]` 中は禁止（`load()` 自体にガードを内蔵、F9/メニュー経由も含めて禁止される）。 |
| `loadplugin` | ➖ | 対象外 | krkr2doc | Kirikiri プラグイン (.dll) 機構。Noiria は TS の import 機構が上位互換のため対象外。 |
| `locate` | 🔨→✅ | W1 (今回実装) | krkr2doc | 現在のペン位置 (x, 任意で y) を直接指定。text.rs の CharControl::Locate（制御専用セグメント）。 |
| `locklink` | 🔨→✅ | W4 (今回実装) | krkr2doc | リンク・ボタン/チェックボックス・クリックによる文章送りを一括ロックする（`KagPlayer.linksLocked` + `ButtonManager.globalLocked`）。Enter/Space キーでの送りは引き続き有効。 |
| `locksnapshot` | 🔨→✅ | W6 (今回実装) | krkr2doc | `onLabel()` の自動スナップショット更新（F5/メニュー保存が使う `this.snapshot`）を凍結する。`[unlocksnapshot]` で解除。`[disablestore]` とは独立（優先順位は `docs/implementation-notes.md`「2026-07-11 W6」参照）。 |
| `macro` | ✅ | 対応済み | krkr2doc | マクロ定義開始。tags.ts macro → captureMacro。 |
| `mapaction` | ➖ | 対象外 | krkr2doc | クリッカブルマップの領域アクション定義。同上、button/link で代替。 |
| `mapdisable` | ➖ | 対象外 | krkr2doc | クリッカブルマップの無効化。同上、button/link で代替。 |
| `mapimage` | ➖ | 対象外 | krkr2doc | クリッカブルマップ画像 (.ma 形式) の読み込み。button/link で代替可能なため対象外。 |
| `mappfont` | ➖ | 対象外 | krkr2doc | プリレンダリング済みフォントデータの管理。Noiria は自作ラスタライザ (font/raster) で都度ラスタライズするため不要。 |
| `move` | ✅ (W5/W7/B12 拡張) | 対応済み | krkr2doc | レイヤ位置/不透明度アニメーション。tags.ts move（KAG 準拠の accel 符号）。`path=(x1,y1,o1)(x2,y2,o2)...` の複数区間を `Scene::Waypoint`/`tween_queue` で継ぎ目なく連結する。`spline=false`（既定）は折れ線、`spline=true` は現在位置を先頭点とする KAG `SplineMover` 互換の自然三次スプライン（指定点は最低 2 点）で x/y を補間する。opacity は省略可（直前値を継承）かつ spline 時も線形補間。`time=` は総時間として、呼出時に native が採取した実効位置から各区間の幾何距離（x/y のユークリッド距離）へ比例配分し、距離 0 の path は均等割りへフォールバックする。`accel=` は全区間へ一律適用（`docs/implementation-notes.md`「2026-07-12」参照）。新しい move とスナップショット復元は進行中 path の残区間を破棄する。 |
| `nextskip` | 🔨→✅ | W4 (今回実装) | krkr2doc | 次の `[l]`/`[p]`/`[s]` の待ちを 1 回だけスキップモードで消費するワンショット指定（`mode.skip` を立て、待ちの消費後に自動で戻す）。 |
| `nowait` | ✅ | 対応済み | krkr2doc | 瞬間表示モード。tags.ts nowait。 |
| `openvideo` | 🔨→✅ | W6 (B4 拡張) | krkr2doc | `slot=0 layer= storage= loop=`。動画を開くだけで再生・先頭フレーム表示のどちらも行わない（`EngineState::video_open`、`playvideo` が使う `prepared` スロットへ格納）。既存の単一同時再生モデル（`videos.current`）とは別に `videos.prepared`（こちらも単一スロット）を追加。 |
| `p` | ✅ | 対応済み | krkr2doc | 改ページクリック待ち。tags.ts p → waitAdvance（自動クリアはしない。明示的に cm/ct/er が必要）。 |
| `pausebgm` | 🔨→✅ | W2 (今回実装) | krkr2doc | BGM 即時一時停止（mixer の paused、再生位置保持）。 |
| `pausevideo` | 🔨→✅ | W6 (B4 拡張) | krkr2doc | `slot=0`。再生中の動画をその場で一時停止する（再生位置保持）。`EngineState::video_pause`（`VideoHandle.paused` フラグ、再生スレッドはフレームを進めず 30ms 間隔でポーリングし続ける）。 |
| `pimage` | 🔨→✅ | W5 (今回実装、sx=/sy=/sw=/sh= は W7) | krkr2doc | `layer= page= storage= clip=（または KAG3 標準の sx=/sy=/sw=/sh=） dx=/x= dy=/y=`。画像レイヤに部分画像（clip/sx.. 省略時は全体）を (dx, dy) を左上として重ね描画する。W4 で実装済みの `layer_blit_image` op をそのまま共用（新規 op なし）。`sx=/sy=/sw=/sh=`（コピー元矩形）を `clip=` の別名として受理する（W7 で追加、KAG3 標準の属性名に対応。`clip=` が指定されていればそちらを優先）。dx=/dy= の意味付けはこのエンジン独自の解釈（`docs/implementation-notes.md`「2026-07-12」参照）。 |
| `playbgm` | ✅ | 対応済み | krkr2doc | BGM 再生。tags.ts playbgm。 |
| `playse` | ✅ | 対応済み (W2 拡張) | krkr2doc | SE 再生。W2 で buf=(0..15、既定0) に対応（チャンネル "se0".."se15"、多重再生可）。 |
| `playvideo` | 🔨→✅ | W6 (B4/B20 拡張) | krkr2doc | `slot=0 layer=`。`storage=` 省略時は `[openvideo]`/`[preparevideo]` で準備済みの動画（`prepared`）の再生を開始する（`EngineState::video_play_prepared`）。`storage=` 指定時は既存の `[video]` タグと同じ即時再生（`api.video.play`）にフォールバックする。B20 で可変解像度 VP9 を表示フレームごとの寸法でレイヤへ反映する。 |
| `position` | ✅ (W3 拡張) | 対応済み | krkr2doc | メッセージウィンドウの位置・サイズ変更。W3 で `layer=`（対象レイヤ指定、省略時カレント）と `visible=` に対応。サイズ変更時は非カレントの対象レイヤも即時再配置する。カレントレイヤは文字送り中なら reveal を保持して再配置し、完了後は従来どおり hover 色を反映する。名前欄はどちらでも更新する。`page=` は受理するが見た目には効かない（テキストレイヤは fore 描画固定）。 **B2**: KAG3 標準の `frame=`（窓サイズ一致の 1 枚画像を窓の背景にする。9-slice ではない。`/` を含まなければ `assets/fg/` 配下。`frame=""` で解除。指定中は窓の地色（`color`）は描かず画像が背景になる。窓と一体で、右クリック/`[hidemessage]`/`visible=false` で隠れ、設定の「窓の濃さ」は画像の不透明度に掛かる）、`color=0xRRGGBB`（地色。アルファは `opacity=` か既存値のまま）、`marginl=`/`margint=`/`marginr=`/`marginb=`（本文の余白。右/下を省略すると左/上と同じ）に対応。`framekey=`・`vertical=` は未対応。 |
| `preparevideo` | 🔨→✅ | W6 (B4/B20 拡張) | krkr2doc | `slot=0 layer= storage= loop=`。`storage=` 指定時は動画を開き、B4 では省略時に既存の `prepared` を使う。現在の開始フレームを自身の寸法で表示し、同じ位置へ戻して再生開始位置を保持する。 |
| `ptext` | 🔨→✅ | W3 (今回実装) | krkr2doc | `layer= page= text= x= y=` で任意レイヤへ直接テキスト描画（既存 text ops + W1 の locate 制御トークンを流用）。ホバー/リンク用ミラー (segs) には載らない・バックログにも記録しない。 |
| `quake` | ✅ | 対応済み | krkr2doc | 画面揺れ。tags.ts quake → api.sys.quake。 |
| `r` | ✅ | 対応済み | krkr2doc | 改行。tags.ts r → KagPlayer.newline。 |
| `rclick` | 🔨→✅ | W4 (今回実装、call の復帰は B1) | krkr2doc | `enabled= jump= call= storage= target=`。右クリック動作のカスタム。属性なしの `[rclick]` は既定（KAG 標準の全メッセージレイヤ表示トグル）へ戻す。`enabled=false` で無効化、`jump=true`/`call=true` で `storage=`/`target=` へのジャンプ/コールに差し替え。**B1**: `call=true` は表示中のメッセージを消さず、サブルーチンが消しても `[return]` で元の表示を復元し、右クリック時に `[l]`/`[p]`/`[s]` で待っていたならその待ちへ戻る（マクロ内の待ちは待ちの直後へ戻る）。`jump=true` は従来どおり `[jump]` と同じくメッセージを消す（KAG3 本家はフックのジャンプでメッセージを消さないが、このエンジンの `[jump]` タグに揃えた）。 |
| `record` | 🔨→✅ | W6 (今回実装、trail 永続化は W7) | krkr2doc | 現在位置（正確な pc、カレントレイヤの表示内容込み）を履歴リング (`KagPlayer.trail`、上限 30 件) へ積む明示記録。KAG3 本来の「操作列の記録」ではなく「巻き戻し用スナップショットの記録」として実装（`[goback]` が使う。`[gotostart]` は W7 から独立の `[startanchor]` アンカーを使う、上記参照）。`[disablestore]` の影響は受けない（`[record]` 自体はメモリ操作のみでディスクに触れないため。ただし `trail` は `[save]`/`[load]` をまたいでセーブデータへ永続化される、W7）。 |
| `resetfont` | 🔨→✅ | W1 (今回実装) | krkr2doc | currentFontState を fontDefaults へリセット。tags.ts resetfont。 |
| `resetstyle` | 🔨→✅ | W1（align/linespacing は B13） | krkr2doc | `currentFontState` を `styleDefaults` へ戻し、カレントメッセージレイヤの行揃え・行間もそのレイヤの `defaultLineStyle` へ戻す。既存本文があれば、その場で全文を新しい行設定へ再配置し、進行中の文字送り状態は保持する。 |
| `resetwait` | 🔨→✅ | W3 (今回実装) | krkr2doc | wait 基準時刻のリセット。`[wait mode=until]` が無い現実装では**基準時刻（`KagPlayer.waitBaseMs`）の保存のみ**で、他に参照箇所なし（効果範囲の詳細は `docs/implementation-notes.md` 参照）。 |
| `resumebgm` | 🔨→✅ | W2 (今回実装) | krkr2doc | 一時停止した BGM の再開。fadepausebgm 後は本来音量へ復元してから再開。 |
| `resumevideo` | 🔨→✅ | W6 (B4 拡張) | krkr2doc | `slot=0`。一時停止した動画の再開（`EngineState::video_resume`、`paused` フラグを false に戻す）。 |
| `return` | ✅ | 対応済み | krkr2doc | call/macro から戻る。tags.ts return → popReturn。 |
| `rewindvideo` | 🔨→✅ | W6 (B4 拡張) | krkr2doc | `slot=0`。再生中/一時停止中または準備済みの動画を 0-based フレーム 0 へ戻す。B4 の `VideoSource::seek_frame(0)` と同じ厳密シーク経路を使う。 |
| `ruby` | 🔨→✅ | W1 (今回実装) | krkr2doc | ルビ。次の 1 回の printText にのみ乗る（pendingRuby）。行送り一律 +ruby_extra 方式（設計判断 1）、セグメント途中折返しは先頭文字基準の簡易対応（設計判断 2）。 |
| `s` | ✅ | 対応済み | krkr2doc | スクリプト停止（次のジャンプ要求まで待機）。tags.ts s → waitForJump。 |
| `save` | 🔨→✅ | W6 (今回実装) | krkr2doc | `place= ask=`。`place` を `save{N}` スロット名へマップし、既存の `KagPlayer.save()` をそのまま呼ぶ（F5/メニューと共通経路）。`ask=true` は W4 の `askYesNo` で確認してから実行。`[disablestore]` 中は禁止（`save()` 自体にガードを内蔵、F5/メニュー経由も含めて禁止される）。 |
| `seopt` | 🔨→✅ | W2 (今回実装) | krkr2doc | volume= をグローバル SE 音量（settings.seVol）へ反映（bgmopt と同方式）。再生中 SE には遡及しない（次回再生から）。 |
| `setbgmlabel` | ➖ | 対象外 | krkr2doc | BGM のループ区間ラベル設定。自作 OGG/Vorbis デコーダは .sli 等のループチューナ形式に非対応のため対象外。 |
| `setbgmstop` | 🔨→✅ | W2 (今回実装) | krkr2doc | BGM 演奏終了（非ループ自然終端）時に storage=/target= へジャンプ予約。明示 stop では発火しない。--autotest では音声スレッド不在のため発火しない（受理のみ）。 |
| `showhistory` | 🔨→✅ | W6 (今回実装) | krkr2doc | 既存の `openBacklog()`（H キー/ホイール操作と同じ実体）をタグから呼ぶ。 |
| `startanchor` | 🔨→✅ | W6 (今回実装、W7 で KAG3 準拠に修正) | krkr2doc | `enabled=`（既定 true）。実行時点の完全スナップショット（`KagPlayer.makeFullSnapshot()`）を専用アンカースロットへ記録する。`[gotostart]` はここへ復帰する。`enabled=false` はアンカーを破棄する。KAG3 準拠（W7 で修正、旧実装は「`[s]` 到達ごとの自動 `[record]` の on/off」だった。`[s]` ごとの自動記録機能自体は廃止し、`[record]` による明示記録のみに一本化した。`docs/implementation-notes.md`「2026-07-12」参照）。 |
| `stopbgm` | ✅ | 対応済み | krkr2doc | BGM 停止。tags.ts stopbgm。 |
| `stopmove` | 🔨→✅ | W5 (今回実装) | krkr2doc | move アニメーションの途中停止。既存 `api.layer.moveSkip()`（`Scene::skip_all_tweens`）をそのまま使う。全レイヤの move を目標値（`path=` の場合は最終区間の値）で即完了させる。新規 op なし。 |
| `stopquake` | 🔨→✅ | W5 (今回実装) | krkr2doc | quake の途中停止。新規 op `op_quake_stop`（`EngineState::quake_stop` が `scene.quake` を即座に `None` へ）。 |
| `stopse` | ✅ | 対応済み (W2 拡張) | krkr2doc | SE 停止。W2 で buf= に対応。 |
| `stoptrans` | 🔨→✅ | W5 (今回実装) | krkr2doc | trans の途中停止。既存 `api.trans.skip()`（`trans_skip`）と同義のタグとして登録（新規 op なし）。 |
| `stopvideo` | ✅ (B4 拡張) | 対応済み | krkr2doc | `slot=0`。動画再生を同期的に inactive にし、旧 worker の描画・イベントを世代で遮断する。 |
| `store` | 🔨→✅ | W6 (今回実装) | krkr2doc | `place=`。変数 (f) のみを `store{N}` スロットへシリアライズ保存する（レイヤ/BGM/位置等を含む `save` とは別粒度）。対応する restore タグは無い（対応表・ウェーブ指示ともに restore タグの列挙が無いため、write-only ユーティリティとして実装）。`[disablestore]` 中は禁止。 |
| `style` | 🔨→✅ | W1（edgecolor= は W7、align/linespacing は B13） | krkr2doc | `font` と同じ `currentFontState` を操作するエイリアス（`edgecolor=` 含む）。`align=left/center/right` は折返し・改行後の各 visual line を独立して整列し、`linespacing=` は既定行送り (`fontSize × 1.6`) に signed px で加算する（実効値は最低 1px）。行設定はメッセージレイヤ別の単一状態で、変更時はそのレイヤの既存本文を即時に全文再配置する。専用 reflow はラスタと追記用 glyph count だけを更新し、進行中の文字送り状態を保持する。`cm`/`er` ではリセットしない。`[locate]` を含む行は指定した絶対 x を保つため整列しない（`indent` は通常どおり整列）。非有限値・単精度範囲外を含む不正値は warn して無視する。 |
| `tempload` | 🔨→✅ | W6 (W7/B12/B14 拡張) | krkr2doc | `place=`（既定 0）。place>=0 はディスクの `"temp{place}"` スロット（`sys.saveRead`）から、負数は `KagPlayer.tempSlots`（メモリ上のみの Map）から復元する（KAG3 で負の place がメモリ専用スロットを指す慣習に対応、W7 で修正。`docs/implementation-notes.md`「2026-07-12」参照）。復元前に進行中のレイヤ移動と残り path を破棄する。B14 で BGM の再生位置と pause 状態も復元する。データが無ければ warn。`[disablestore]` 中は禁止。 |
| `tempsave` | 🔨→✅ | W6 (W7/B14 拡張) | krkr2doc | `place=`（既定 0）。`makeFullSnapshot()`（位置 `pos.index` + カレントレイヤの表示内容 `curSegs`/`curText` + BGM の `positionMs` を含む完全スナップショット）を保存する。place>=0 はディスクの `"temp{place}"` スロット（`sys.saveWrite`、通常セーブ `"save{N}"` とは別プレフィックスでセーブメニューには混ざらない）へ、負数は `KagPlayer.tempSlots`（メモリ上のみの Map）へ（KAG3 で負の `place` がメモリ専用スロットを指す慣習に対応、W7 で修正。`docs/implementation-notes.md`「2026-07-12」参照）。旧セーブに位置が無い場合は 0ms から再開する。`[disablestore]` 中は禁止。 |
| `timeout` | 🔨→✅ | W4 (今回実装、W7 で KAG3 準拠に修正、B1 で一時停止対応) | krkr2doc | `time= storage= target=`。KAG3 準拠のクリック待ちタイムアウト（`KagPlayer.setTimeoutJump`）: time ms 経過してもクリック待ち（`waitAdvance`/`waitForJump`、`[l]`/`[p]`/`[s]`）中でなければジャンプせず、満了を記録するだけにする。次にクリック待ちへ入った時点で即発火する。発火したら 1 回発火型として自動解除（`[ctimeout]` を待たない）。クリックで待ちが解決されてもタイマー自体は解除されない（次の待ちで再び満了していれば発火する）。`[ctimeout]` で明示的に解除できる。W7 で修正（`docs/implementation-notes.md`「2026-07-12」参照）。 |
| `title` | ✅ | 対応済み | krkr2doc | ウィンドウタイトル設定。tags.ts title → api.sys.title。 |
| `trace` | 🔨→✅ | W6 (今回実装) | krkr2doc | `exp=` を評価して `api.sys.log` へ出力する（`emb` の表示版と同じ経路、表示はしない）。 |
| `trans` | ✅ (W5 拡張) | 対応済み | krkr2doc | トランジション開始（crossfade/universal、W5 で scroll を追加）。tags.ts trans。`method=scroll`: `from=`（left/right/top/bottom、既定 left）で新ページが入ってくる辺を指定する `TransMethod::Scroll`（`Pixmap::scroll_from`）。`stay=`（stayfore/staynone/staybase 相当）は受理するが、このエンジンの合成は常に画面全面を覆う不透明フレーム同士のブレンドのため描画上は区別しない（旧ページは常にその場に留まり新ページに覆われる 1 通りの見た目のみ実装、詳細は `docs/implementation-notes.md`「2026-07-11 W5」参照）。`layer=`（真のレイヤ単位トランジション）は対象外: 受理して全画面（従来どおり）にフォールバックし、`KagPlayer` インスタンスごとに初回だけ warn する。 |
| `unlocklink` | 🔨→✅ | W4 (今回実装) | krkr2doc | `[locklink]` の解除。 |
| `unlocksnapshot` | 🔨→✅ | W6 (今回実装) | krkr2doc | `[locksnapshot]` の解除。 |
| `video` | ✅ (B4/B20 拡張) | 対応済み | krkr2doc | 動画レイヤ再生。tags.ts video（自作 MJPEG/VP9(IVF) デコーダ、codec/mf_video.rs 系）。B4 で 0-based `frame=` / ms 単位 `position=` の厳密 seek（両方指定時は frame 優先）と slot=0 を追加。storage 省略時は current/prepared の位置だけを変更する。可変解像度 VP9 は各表示フレームの寸法でレイヤを差し替える。 |
| `videoevent` | 🔨→✅ | B4 (今回実装) | krkr2doc | `slot=0 frame=N`。指定表示フレーム N の描画時に period event を 1 回発火し、自動解除する。 |
| `videolayer` | ➖ | 対象外 | krkr2doc | 動画の出力先レイヤ切替。Noiria は video タグの layer= 属性で代替済み。 |
| `videosegloop` | 🔨→✅ | B4 (今回実装) | krkr2doc | `slot=0 start= end=`。`[start,end)` を反復し、end-1 表示後に start へ戻って segLoop event を発火する。負数・非整数・start>=end は warn + no-op。VP9/MF は rewind + decode-forward のため遠い区間の反復コストが大きい。 |
| `wa` | 🔨→✅ | W5 (今回実装) | krkr2doc | `layer=`。`[animstart]` のフレームアニメーションが停止する（`[animstop]`、または `loop=false` の .asd を最後まで再生し終える）まで待つ。`KagPlayer.waitWhile()`（複数 await タグの jumpRequest ガード、既存ルール踏襲）経由で実装。 |
| `wait` | ✅ | 対応済み (B1 で一時停止対応) | krkr2doc | 時間待ち。tags.ts wait → KagPlayer.wait。メニュー/バックログ/モーダル表示中の時間は数えない（閉じたら残りを待つ）。オート待ち・`[timeout]` も同様。 |
| `waitclick` | 🔨→✅ | W4 (今回実装) | krkr2doc | 左クリックを 1 回待つだけ（`api.waitClick()` を再利用）。文章送りの完了状態や skip/auto モードには影響しない。 |
| `waittrig` | 🔨→✅ | W4 (今回実装、kag グローバルは W7) | krkr2doc | `name=`。指定名のトリガ発火まで待つ（`KagPlayer.waitTrigger`）。発火は KAG3 準拠の `[eval exp="kag.trigger('name')"]`（`kag` は `evalExpr`/`evalStmt` の第 3 引数として全呼び出し箇所へ渡される KagPlayer インスタンス、W7 で追加）から行う。後方互換で `[eval exp="sys.player.trigger('name')"]`（`sys.player` も同じ KagPlayer インスタンス）も引き続き使える。 |
| `wb` | 🔨→✅ | W2 (今回実装) | krkr2doc | BGM フェード完了待ち。canskip=(既定true) でスキップ中は即抜け。--autotest では isFading が常に false のため即抜け。 |
| `wc` | 🔨→✅ | W3 (今回実装) | krkr2doc | `n=` 文字分の時間待ち（n × 1000/現在cps ms、cps=0 なら待ちなし）。クリック待ちは伴わない（実装解釈は `docs/implementation-notes.md` 参照）。 |
| `wf` | 🔨→✅ | W2 (今回実装) | krkr2doc | SE フェード完了待ち。buf=/canskip= 対応（wb の SE 版）。 |
| `wheel` | 🔨→✅ | W4 (今回実装) | krkr2doc | `storage= target=`。ホイール入力で target へジャンプする常設フック（player.ts `eventJumps.wheel`。既定のホイール上=履歴表示より優先）。`[cwheel]` で解除。 |
| `wl` | 🔨→✅ | W2 (今回実装) | krkr2doc | BGM 演奏終了待ち（非ループ再生の自然終端）。canskip= 対応。再生中でなければ即完了（--autotest のハング防止フォールバック兼用）。ループ再生中に置くと実再生では永久待ち（KAG3 同様）。 |
| `wm` | ✅ | 対応済み | krkr2doc | move 完了待ち。tags.ts wm → waitMove。 |
| `wp` | 🔨→✅ | B4 (今回実装) | krkr2doc | `slot=0 for=loop\|period\|prepare\|segLoop` の latch を待つ。for 省略は any。停止・未再生時は即時復帰し、再生世代が変わった待機も旧イベントを新再生へ持ち越さず終了する。 |
| `wq` | ✅ | 対応済み | krkr2doc | quake 完了待ち。tags.ts wq。 |
| `ws` | ✅ | 対応済み (W2 拡張) | krkr2doc | SE 再生完了待ち。W2 で buf= に対応。tags.ts ws → api.audio.waitEnd。 |
| `wt` | ✅ | 対応済み | krkr2doc | トランジション完了待ち。tags.ts wt → waitTransition。 |
| `wv` | ✅ (B4 拡張) | 対応済み | krkr2doc | `slot=0`。動画再生完了待ち。停止・未再生時は即時復帰する。 |
| `xchgbgm` | 🔨→✅ | W2 (今回実装) | krkr2doc | 現 BGM をフェードアウトしつつ新 BGM をフェードイン（storage= time= volume= loop=）。内部チャンネル "bgm" ⇄ "bgm2" のトグルで実現（KagPlayer.bgmCh）。 |

## ウェーブ割当

- **✅ 対応済み (48)**: `backlay call cancelskip close cm ct delay else elsif emb endif endlink
  endmacro endnowait er eval fadebgm fadeoutbgm freeimage history if image jump l link macro move
  nowait p playbgm playse position quake r return s stopbgm stopse stopvideo title trans video wait
  wm wq ws wt wv`
- **🔨 W1（実装済み, 11）**: `font resetfont style resetstyle deffont defstyle ruby ch locate indent
  endindent` — `docs/implementation-notes.md`「2026-07-11 W1: 文字装飾系タグ」参照。
  当初保留した `style`/`defstyle` の `align=`/`linespacing=` も 2026-07-26 B13 で対応済み。
- **🔨→✅ W2 音声 (16)**: `bgmopt clearbgmstop fadeinbgm fadeinse fadeoutse fadepausebgm fadese pausebgm
  resumebgm seopt setbgmstop wb wf wl xchgbgm entersebuf`（`entersebuf` は出典 sourceforge、受理して
  no-op）。あわせて既存 `playse`/`stopse`/`ws` に `buf=`（SE バッファ 0..15）、`fadebgm` に `time=` を
  拡張 — `docs/implementation-notes.md`「2026-07-11 W2: 音声系タグ」参照。
- **🔨→✅ W3 複数メッセージレイヤ (11)**: `autowc copylay current graph hch hidemessage laycount layopt
  ptext resetwait wc`。あわせて既存 `cm`/`ct`/`er` の実装を分離（cm=全レイヤ、ct=cm+カレントを
  message0 へ、er=カレントのみ。既定 1 レイヤ構成では見た目は従来と同一）、`position` に `layer=`/
  `visible=` を拡張。制限: laycount 上限 8、`page=` 属性はテキストレイヤが fore 描画固定のため
  効果なし、`hch` は平文フォールバック、`resetwait` は基準時刻保存のみ、`layopt autohide` 対象外 —
  `docs/implementation-notes.md`「2026-07-11 W3: 複数メッセージレイヤ」参照。
- **🔨→✅ W4 UI・ボタン・クリック制御 (21)**: `button cancelautomode cclick checkbox click clickskip
  commit ctimeout cwheel edit glyph input locklink nextskip rclick timeout unlocklink waitclick
  waittrig wheel ask`（`ask` は出典 sourceforge）。新規モジュール `runtime/kag/buttons.ts`
  （`ButtonManager`）と新規 op `layer_blit_image`（3 状態ボタン画像の部分矩形ブリット、W5 の
  `pimage` と共用予定）を追加。button/checkbox/edit の活性化は `[s]` 停止中のみ（KAG 準拠）。
  制限: `edit`/`input` は IME 非対応、`ask`/`input` は厳密な行位置クリックではなく「任意の 1
  クリックで必ず閉じる」設計（ヘッドレス自動入力でのハング回避）—
  `docs/implementation-notes.md`「2026-07-11 W4: UI・ボタン・クリック制御」参照。
  （`glyph fix=` の動的追従と `waittrig` の `kag.trigger()` 対応は 2026-07-12 に実装済み、
  「KAG3 との意味論差異一覧」参照。）
- **🔨→✅ W5 トランジション/アニメ/画像 (7)**: `animstart animstop pimage stopmove stopquake stoptrans
  wa`。あわせて既存 `trans` に `method=scroll`（`from=` 方向、`stay=` は受理のみ描画非区別）、
  `move` に `path=`（折れ線区間列）、`image` に `clip=`/`flipud=`/`fliplr=`/`grayscale=` を拡張。
  先行して W4 で発見された `copylay` の既知バグ（コピー先メッセージレイヤの位置が恒久的にずれる）
  を修正（`copy_position` 引数の追加）。新規モジュール `runtime/kag/anim.ts`（`AnimManager`/
  `parseAsd`、.asd 最小サブセット）と新規 op `op_layer_move_path`/`op_layer_load_image_ex`/
  `op_quake_stop` を追加。制限: `trans layer=` は全画面フォールバック（初回 warn）、`stay=` は
  描画上区別しない、`.asd` は最小サブセット
  （`セル番号,graphic,wait` + `loop=` のみ、`seg=`/`module=` 等のフル構文は対象外） —
  `docs/implementation-notes.md`「2026-07-11 W5」参照。（`move path=` の時間配分は当初区間数で
  均等割りだったが、2026-07-12 に区間の距離比例へ修正済み。）
- **🔨→✅ W6 ストア系+動画拡張+残余 (32)**: `clearsysvar clearvar copybookmark disablestore endhact
  endignore endscript erasebookmark erasemacro goback gotostart hact hr ignore iscript load
  locksnapshot openvideo pausevideo playvideo preparevideo record resumevideo rewindvideo save
  showhistory startanchor store tempload tempsave trace unlocksnapshot`。新規スロット/履歴の管理
  （`KagPlayer.tempSlots`/`trail`）と新規動画 ops（`op_video_open`/`op_video_prepare`/
  `op_video_play_prepared`/`op_video_pause`/`op_video_resume`/`op_video_rewind`/
  `op_video_is_paused`）を追加。parser.ts に `[iscript]`〜`[endscript]` の生テキスト収集モードを
  新設。制限: `store` に対応する restore タグは無い（write-only）、`ignore` は KAG3 本来の
  「入力の一時無視」ではなく「条件付き
  スキップブロック」として実装、`iscript` は TJS 互換ではなく TS として評価（V8/tsint 両対応）、
  `hact` はテキスト記録のみでアクション非対応、`trail`（履歴リング）は `sf.trail` 相当、上限
  30 件 — `docs/implementation-notes.md`「2026-07-11 W6」参照。（`erasebookmark` の空 JSON
  書き込み代替・`gotostart` の「最初の記録位置」解釈・`trail` の非永続化は、いずれも 2026-07-12
  に KAG3 準拠へ修正済み。`docs/implementation-notes.md`「2026-07-12」参照。）
- **🔨→✅ B4 動画フレーム seek + event (5)**:
  `videoevent wp cancelvideoevent videosegloop cancelvideosegloop`。表示フレーム番号基準の
  `VideoSource::seek_frame`、period/loop/prepare/segLoop latch、再生世代隔離を追加。
  `preparevideo` は storage 省略時に openvideo 済み素材を preview し、その seek 位置を維持する。
  詳細は `docs/implementation-notes.md`「2026-07-27 B4」参照。
- **➖ 対象外 (10)**: `loadplugin` / `mapimage mapaction mapdisable` / `mappfont` /
  `videolayer clearvideolayer` /
  `setbgmlabel clearbgmlabel` / `cursor`。理由は対応表の各行を参照（要約: プラグイン機構は TS の
  `import` が上位互換、クリッカブルマップは `.ma` 形式依存で `button`/`link` が代替、プリレンダフォント
  は自作ラスタライザにより不要、動画出力先切替は `video layer=` で代替、BGM ラベルは
  `.sli` 等のループチューナ形式に非対応、`cursor` はヘッドレス検証不能かつ実利が薄い）。W6 で
  tags.ts に warn（「未対応タグ」ではなく「対象外: 理由」）1 回 + no-op のハンドラを一括登録した
  （`STUB_REASONS` テーブル、`Object.entries(...).forEach(...)` で配線。tsint 実機検証で
  `for...of` + クロージャの組み合わせが全イテレーションの束縛を共有してしまう不具合を発見したため
  `for...of` ではなく `forEach` を使っている。詳細は `docs/implementation-notes.md`
  「2026-07-11 W6」参照）。

合計: 48（✅） + 11（🔨→✅ W1） + 16（🔨→✅ W2） + 11（🔨→✅ W3） + 21（🔨→✅ W4） + 7（🔨→✅ W5） +
32（🔨→✅ W6） + 5（🔨→✅ B4） + 10（➖） = **161**
（✅ 対応済み合計 48 + 11 + 16 + 11 + 21 + 7 + 32 + 5 = **151** + ➖ 対象外 **10** = **161**。
全 161 行が ✅/➖ で埋まり、KAG3 タグ全対応が完了した）。

## KAG3 との意味論差異一覧（移植時の注意）

対応表の備考に散在している「KAG3 と挙動・解釈が異なる点」の集約。KAG3 のシナリオを移植する際は
まずここを確認すること（各項目の詳細・理由は対応表の該当行と `docs/implementation-notes.md` 参照）。

### 挙動が KAG3 と異なるもの（移植時に書き換え・見直しが必要）

- **`wc`**: クリック待ちではなく「n × 1000/現在cps ms の時間待ち」。
- **`ask`**: 「はい」の行に厳密ヒットしないクリックは**既定で「いいえ」として必ず閉じる**
  （自動化・誤クリック時にハングしない設計を優先）。
- **`ignore`**: 「条件付きスキップブロック」として実装（解釈は対応表・実装ノートに明記）。
- **`clickskip`**: 「クリックによるスキップモード解除の可否」という解釈で実装。
- **`iscript`**: 本文は TJS ではなく **TS として評価**される。
- **`clearsysvar`**: sf 全消去の直後に設定既定値・未読判定を再適用する（壊れた状態を残さない）。
- **`bgmopt`/`seopt`**: `volume=` は**設定メニューと同じグローバル音量**を書き換える
  （プレイヤーの設定値が変わる）。
- **`wl`**: 再生中でなければ即完了（ヘッドレス対応のフォールバック）。ループ再生中の BGM に
  置くと KAG3 同様に永久待ちになる。

（2026-07-12 時点で `timeout`/`gotostart`/`startanchor`/`record`・`goback`・`gotostart` の trail
永続化/`tempsave`・`tempload`/`waittrig` は KAG3 準拠に修正済みのためこの一覧から解消した。
`stopmove` は KAG3 でも全レイヤ対象の動作でありそもそも差異ではなかったため、誤って一覧に
載っていた項目として削除した。詳細は対応表の該当行と `docs/implementation-notes.md`
「2026-07-12」参照。）

### 機能制限（受理するが簡略実装・非対応）

- **`trans`**: `layer=` は全画面トランジションへフォールバック（warn 1 回）。`method=scroll` の
  `stay=` は受理するが描画上の区別なし。
- **`animstart`（.asd）**: 独自の最小サブセット（`セル,graphic,wait` 行 + `loop=`）のみ。
  `seg=`/`module=` 等のフル構文は非対応。
- **`move`**: `path=` はスプライン（`spline=true`、B12 で対応）にも対応。時間配分は区間の距離比例
  （2026-07-12 に均等割りから修正済み）。
- **`glyph`**: `fix=true` は固定位置のみ（動的なペン位置追従は既定の `fix=false` 側で対応済み、
  2026-07-12 修正）。
- **`hch`**: 縦中横は平文フォールバック / **`hact`**: テキストのみ記録（アクション非対応）。
- **`edit`/`input`/`ask`**: 自作オーバーレイで IME 非対応。
- **`entersebuf`**: 受理して no-op（SE バッファは `buf=0..15` を動的解決）。
- **`pimage`**: `dx=/dy=` の意味付けはエンジン独自解釈（`sx=/sy=/sw=/sh=`/`clip=` はコピー元矩形
  として KAG3 互換、2026-07-12 に sx=/sy=/sw=/sh= を追加）。
- **`font`**: `shadow=` は対象外。`face=` はセグメント単位の family 指定（B1 で対応、ゲーム同梱フォントがあれば
  family 名が一致しなくても同梱フォントがシステムフォントより優先。`face=user`/空指定は既定へ戻す）。
  OpenType/CFF（`.otf`・'OTTO' の `.ttf`）は非対応で、起動時に error ログを出して無視する。
- **メッセージレイヤの `page=`**: `current`/`position`/`layopt` とも受理のみで描画に影響しない。

## Noiria 独自拡張タグ（KAG3 標準には無い）

上記の対応表には含めない、Noiria 固有の拡張タグ。README.md のタグ一覧にも掲載している。

| タグ | 概要 |
| --- | --- |
| `playvoice` | ボイスチャンネルでの音声再生（次の行/ページへ送る操作（クリック/Enter/スキップ送り）で自動停止、B1。オート送りはボイス終了を待ってから待ち時間を数える。`stopvoice` とペア。KAG3 標準では `voice` は `wave`/`playse` 系に統合されることが多く、Noiria は明示タグとして独立させている）。 |
| `stopvoice` | ボイス再生停止。 |
| `autowait` | オートモードの待ち時間（ms）を直接指定するタグ（KAG3 標準の相当タグは設定 UI 経由が一般的）。 |
| `screenshot` | セーブサムネイル用スクリーンショットを明示的に撮る。 |
| `name` | 名前欄プレートの表示/非表示切り替え（`[cm]`/`[er]` では維持され、明示的に変えるまで残る Noiria の慣習）。 |
| `sysbar` | `visible=false/true`。`KagOptions.systemButtons` で常設したシステムボタンの表示を切り替える（B2。タイトル画面で隠す用。ロードすると表示に戻る）。 |
| `titlescreen` | Noiria 拡張（B3）。`KagOptions.ui: "modern"` と `KagOptions.titleScreen`（ロゴ・サブタイトル・メニュー項目）で宣言した専用タイトル画面を出し、項目が選ばれるまで止まる（`[s]` と同じくジャンプ要求で進む）。classic では未対応（warn して `[s]` として動く）。項目の `action`（continue/gallery/config/exit）と `target`/`storage`/`exp` は `README.md`「モダン UI (B3)」。 |
| `autosave` | `enabled=false/true`。選択肢（リンクのある `[s]`）での自動保存（スロット `auto`）の有効/無効（B2。タイトル画面で止める用。ロードすると有効に戻る）。 |
