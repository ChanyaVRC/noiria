# 実装ノート

実装**中**に随時更新するファイル（事後まとめではない）。仕様書に無かった判断・変更・トレードオフを
「判断→理由→影響」形式で記録する。CLAUDE.md の「仮定は明示する」方針に基づく恒久ルール。

## 2026-07-11 W1: 文字装飾系タグ

対象: `font resetfont style resetstyle deffont defstyle ruby ch locate indent endindent`
（詳細設計書 `breezy-sauteeing-fox-agent-a1d3d2a0d7eee225c.md` の B1〜B7 に従って実装）。

### 設計書 1 章の採用判断（転記）

- **判断**: ルビの行送りは、セグメント単位で可変にせず、`raster_segments` 呼び出し全体でルビ付き
  セグメントが 1 つでもあれば一律 `ruby_extra`（`font_size × 0.45 × 1.1`）を全行の baseline_y に
  加算する。`line_advance` 自体は変えない（行間の間隔は据え置き、baseline だけ一律シフト）。
  **理由**: 行ごとの可変行高は `assign_lines`（フォント非依存の純粋関数）の禁則テスト前提を壊さず
  実装するのが難しい。**影響**: メッセージウィンドウ内で 1 箇所でもルビを使うと全行が下にシフトする
  粗さがある。極端なケースでは box_h を超える可能性があるが、行のカリング判定（`line_top +
  line_advance > box_h`）は `ruby_extra` を考慮しないため、ルビ分だけ box 下端をわずかにはみ出す
  ケースが理論上ありうる（実用上のメッセージ行数・box_h では発生しない想定）。
- **判断**: ルビがセグメント内で折り返される場合は非対応。先頭文字と同じ行のグリフだけで x 範囲を
  取り、その中央にルビを置く簡易対応とする。**理由**: セグメント途中の折返しでルビを泣き別れさせる
  実装は複雑。**影響**: 長いルビ付きセグメントが行末で折り返される場合、ルビ位置がずれる可能性が
  ある（今回のテスト・サンプルでは発生しない短いセグメントのみ使用）。
- **判断**: `style`/`defstyle` の `align`/`linespacing` 属性は非対応（受理はするが無視し、インスタンス
  ごとに初回だけ warn ログを出す）。**理由**: メッセージウィンドウのレイアウトエンジンが単一の
  左揃え・固定行送りを前提にしており、対応するには `assign_lines`/Pass3 に踏み込んだ変更が必要で
  スコープ外。**影響**: `[style align=center]` 等を書いても無視される（ログで気付ける）。
- **判断**: 行送り（`line_advance`）は常にボックス基準の `style.font_size` で固定。セグメントごとの
  `size` はグリフ拡大・advance にのみ影響し、ベースライン位置には影響しない。**理由**: 実フォント
  メトリクスに依存しない `assign_lines` の設計と整合させ、極端なサイズ差を許容範囲の簡略化とする。
  **影響**: 本文よりかなり大きい `size` を指定すると、上下の行やボックスをはみ出すことがある
  （既知の制限、`mixed_font_sizes_share_baseline` テストは極端でない範囲での妥当性のみ検証）。
- **判断**: `bold`（疑似太字）は advance（文字送り幅）を変更しない。グリフの本体合成ループでのみ
  `coverage(x,y) = max(mask(x,y), mask(x-1,y))` として左隣のカバレッジを拾う 1 行変更（縁取り
  ループには適用しない）。**理由**: 追加のキャンバス拡張や shaping 変更を避けて実装を単純化する。
  **影響**: 太字でも次の文字位置はズレない（本物の bold フォントより字間が詰まって見える場合がある）。
  また縁取り（outline）はボールドの恩恵を受けないため、太字＋縁取りを併用すると縁が細く見える
  ケースがある。
- **判断**: `[cm]`/`[er]`/`clearMessage()` では `currentFontState`（`font`/`style` タグの状態）を
  リセットしない。明示的に `[resetfont]`/`[resetstyle]` するまで維持する。**理由**: 既存の `[name]`
  （名前欄）の慣習に合わせる。**影響**: 一度 `[font color=...]` を発行すると、`[cm]` を挟んでも
  次のメッセージにも同じ色が適用され続ける。シナリオ作者は明示的にリセットする必要がある。
- **判断**: `shadow`（KAG の font 属性）は対象外。**理由**: 既存の縁取り合成は「同一マスクを膨張
  させて描く」実装であり、オフセット位置への二重ブリットが必要な影とは根本的に別パスが要るため、
  今回のスコープでは費用対効果が見合わないと判断（ユーザー確認済み）。**影響**: `docs/kag-tag-compat.md`
  には表示されず、`TextSegment` にも `shadow` フィールドを追加していない（コメントで明記）。

### 実装中に新たに発生した判断

- **判断**: tsint の `Number("0xffcc66")` hex 文字列パース対応可否（オープン課題）を実装着手前に
  ソース調査で確認したところ、`tsint/src/vm.rs` の `to_number()`（`Number()` の内部実装）が
  `strip_prefix("0x")` + `i64::from_str_radix(hex, 16)` で hex 文字列パースに対応済みだった
  （`vm.rs:1972-1973`）。ビット演算 `>>`（`OpCode::Shr`, `vm.rs:578`）・`&`（`OpCode::BitAnd`,
  `vm.rs:586`）も対応済み。**理由/影響**: 設計書のオープン課題で想定されていた「手書き hex パーサへの
  切替」は不要と判断し、`player.ts` の `hexColor()` は素直に `Number(v)` + `(n >> 16) & 0xff` 等の
  ビット演算で実装した（除算ベースへの切替は行っていない）。
- **判断**: `player.ts` の `hexColor` を最終的に **`export` しない**モジュール内関数 `parseHexColor`
  にし、`KagPlayer.hexColor()` インスタンスメソッド経由で `tags.ts` から呼ばせる形に変更した
  （設計書 B5 の記述では素朴に export する想定だった）。**理由**: `tags.ts` が `noiria:kag/player`
  から値（`hexColor`）を import すると、`player.ts` が既に `noiria:kag/tags` から `makeTags`/`TagHandler`
  を値 import しているため、`player.ts → tags.ts → player.ts` の循環 import になる。V8 (deno_core) は
  ES Module の live binding セマンティクスで解決できるため気付かなかったが、tsint エンジンで
  `cargo run -p noiria-app -- games/sample --engine tsint --autotest ...` を実行したところ
  `first.ks:15 実行エラー: TypeError: これは関数ではありません`（`[font color=...]` 行）が発生した。
  tsint の module 実装は import を評価順に解決しており、循環時に未評価のバインディングを
  `undefined` のまま掴んでしまうため。**影響**: V8 では動くが tsint では落ちる、という**エンジン間差異
  が実機検証で初めて顕在化した**実例。今後 `runtime/kag/` 配下でモジュール間の値 import を追加する際は、
  `player.ts ⇄ tags.ts` のような相互参照を避け、片方向（`player.ts → tags.ts` のみ）に保つこと。
  修正はコミット前に適用済み（`cargo run ... --engine tsint --autotest 24` で EXIT=0 かつ
  対象フレームで装飾・ルビが正しく描画されることを確認済み、V8 側とピクセル同一）。
- **判断**: `edge_color`（`TextSegment.edge_color` / `style.outline_color` のセグメント単位上書き）は
  Rust 層（`text.rs`）・tsint `segments_from_value`・TS 型定義（`Segment.edgeColor`）までは実装したが、
  `runtime/kag/tags.ts` の `font`/`style` タグには対応する属性（`edgecolor=` 等）を**接続していない**。
  **理由**: 設計書 B5 が明示するリテラル型 `currentFontState: { size?: number; bold?: boolean; color?:
  api.RGBA }` に `edgeColor` が含まれておらず、W1 の 11 タグ一覧にも edge 専用タグは無い。また
  `first.ks` に追加が指示されたサンプル行（`[font color=... size=...]`/`[ruby text=...]`）も edge_color
  を exercise しない。CLAUDE.md の「求められていない機能を足さない」方針に沿い、tags.ts 側の配線は
  見送った。**影響**: `edge_color_overrides_style_outline` の Rust 単体テストではこの機能を検証済み
  だが、`.ks` シナリオから実際に使うには今後 `[font edgecolor=0x...]` のような属性を `tags.ts` の
  `applyFontAttrs` に追加する必要がある（1 行追加で対応可能な設計にしてある）。
- **判断**: `[locate]`/`[indent]`/`[endindent]` は制御専用の空文字列セグメント
  （`{ text: "", color: null, locate: {...} }` 等）として `api.text.print` に単独で送る
  （`KagPlayer.printControl`）。`printText` とは別メソッドにし、`currentText`/`paragraph`（バックログ用
  蓄積文字列）を更新しない。**理由**: 制御トークンは表示文字を持たないため、バックログに空行として
  残る必要は無い。**影響**: バックログ機能には影響しない。

### 検証結果

- ベースライン: `cargo test`（ワークスペース） 179 件全通過（noiria-app 14 / noiria-core 147 /
  vorbis_crosscheck 3 / noiria-tools 3 / audio_quality 12、doc-tests 0）。
- 実装後: 184 件全通過（noiria-core が 147→152、新規 5 件: `mixed_font_sizes_share_baseline`
  `ruby_glyphs_centered_above_segment` `indent_applies_to_wrapped_lines` `bold_extends_coverage_left`
  `edge_color_overrides_style_outline`）。既存 154 件は無変更で全通過（新フィールドは全て `None` 既定の
  ため回帰なし）。
- `cd ../tsint && cargo test` 全通過（コーパステスト `kag_full.rs` 含む。`runtime/kag/` の新規 TS 構文が
  パース可能なことを確認）。
- E2E: V8 (`--autotest 24 --autotest-out target/autotest-w1`) / tsint
  (`--engine tsint --autotest 24 --autotest-out target/autotest-w1-tsint`) いずれも EXIT=0。
  `frame_07_hover.png` で `[font color=0xffcc66 size=32]強調[resetfont]` と
  `[ruby text=きょう]今日` の装飾・ルビ表示を確認（両エンジンでピクセル同一の見た目）。
- `cargo clippy --workspace --all-targets`: 変更ファイル（`text.rs`）で `too_many_arguments`
  （`rasterize_ruby` が 8 引数）を検出、`RubyPlacement` 構造体に引数をまとめて解消。他の warning は
  本ウェーブの変更と無関係な既存コード（`noiria-tools`/`noiria-app` の別ファイル）のもの。

## 2026-07-11 W2: 音声系タグ

対象: `bgmopt clearbgmstop fadeinbgm fadeinse fadeoutse fadepausebgm fadese pausebgm resumebgm
seopt setbgmstop wb wf wl xchgbgm entersebuf` ＋ 既存 `playse/stopse/ws/fadebgm` の buf=/time= 拡張。

### 設計判断（判断→理由→影響）

- **判断**: チャンネルの動的化は「Map」ではなく**固定上限配列＋名前解決**で実装した。
  `mixer::NUM_SLOTS = 19` の固定スロット（0=bgm, 1=se0（"se" は別名）, 2=voice, 3=bgm2,
  4..18=se1..se15）を `audio::channel_index()` が文字列名から解決する。**理由**: 既存の
  `[Slot; 3]` / `[AtomicBool; 3]` 配列パターンと `AudioShared` のロックフリー参照
  （AtomicBool 配列）を壊さない最小変更で済み、Map+Mutex 化より単純。ops 署名（文字列
  チャンネル）は不変。**影響**: SE バッファは `buf=0..15` の 16 本まで。範囲外の `buf=` は
  「不明なチャンネル」エラーになる（タグ実行エラーとしてログに出る）。
- **判断**: `xchgbgm`（クロスフェード切替）のために内部チャンネル **"bgm2"** を追加し、
  `KagPlayer.bgmCh` が "bgm" ⇄ "bgm2" をトグルする方式にした。BGM 系タグは全て
  `p.bgmCh` を参照する。**理由**: 単一 "bgm" チャンネルでは旧曲フェードアウトと新曲
  フェードインを同時に実行できない（ミキサのスロットは 1 チャンネル 1 ソース）。KAG3 も
  内部に 2 つの BGM バッファを持つ。**影響**: `bgmCh` はセーブ対象外で、ロード時は
  "bgm" に戻す（ロード時に旧チャンネルを明示 stop する）。ゲームスクリプトが `api.audio`
  を直接 "bgm" 決め打ちで叩くと xchgbgm 後にズレる可能性があるが、KAG タグ経由では一貫する。
- **判断**: `wl` の待ち条件は「`finished` フラグ」に加えて「**そもそも再生中でない
  (`!isPlaying`) なら完了扱い**」のフォールバックを必ず入れた。**理由**: `--autotest`
  （ヘッドレス）ではオーディオスレッド自体を起動しない（main.rs が audio_rx を drop）ため、
  `AudioShared` は既定値のまま更新されず `finished` は永遠に立たない。フォールバックが
  無いと `[wl]` がヘッドレスで無限ハングする（`is_playing` は既定 false なので即抜けする）。
  **影響**: 実再生でも「BGM が既に停止済みなら wl は即完了」となり、KAG の実用挙動と一致。
  逆に**ループ再生中の BGM に [wl] を置くと実再生では永久待ちになる**（KAG3 と同じ）ので、
  サンプルでは fadeoutbgm 完了後にのみ配置した。
- **判断**: `finished` フラグは「**非ループ再生の自然終端**」でのみ立てる。明示 `stop`／
  `FadeThen::Stop` では立てない。次の `play` でクリアする。**理由**: `wl`/`setbgmstop` は
  「演奏が終わった」ことへの反応であり、スクリプト側の都合で止めた場合と区別しないと
  `setbgmstop` が stopbgm でも誤発火する。**影響**: `setbgmstop` はヘッドレスでは決して
  発火しない（finished が立たないため）。E2E では受理と解除（clearbgmstop）のみ検証し、
  発火自体は Rust ユニットテスト（natural_end_sets_finished_and_play_clears_it）で担保。
- **判断**: `setbgmstop` の監視は `KagPlayer.watchBgmStop()` の**デタッチした async
  ポーリングループ**（120ms 間隔、世代カウンタ `bgmStopGen` で clearbgmstop/再予約時に
  自然消滅）で実装した。**理由**: 既存 `op_audio_wait_end` / `waitMove` と同じポーリング
  パターンで、メイン実行ループへの割り込みは既存の `requestJump()`（fireInterrupt）が
  そのまま使える。**影響**: ループはプレイヤー終了 (`running=false`) でも止まる。
- **判断**: `FadeThen::Pause` 完了後の音量は 0 のまま保持し、`resumebgm` タグ側で
  `currentBgm.volume` から実効音量を再設定して復帰する。**理由**: ミキサは「タグ音量×
  設定音量」の積しか知らず、元音量への復元は TS 層（currentBgm を持つ player）の責務に
  する方が二重管理にならない。**影響**: `api.audio.resume()` を直接呼ぶだけでは音量 0 の
  まま（KAG タグ経由なら正しく復帰）。
- **判断**: `bgmopt`/`seopt` は既存 `settings.bgmVol/seVol`（0..100、`sf.config` へ永続化）
  への書き込み（`setGlobalVolume()`）として実装。実効音量は従来通り `volFor()` が
  タグ音量×設定%で一元計算するため**二重掛けにならない**。BGM は再生中なら即時反映、
  SE/voice は次回再生から反映。**理由**: 設定メニュー（adjustSetting）と同じ経路に統一。
  **影響**: `bgmopt` の volume は「ユーザー設定と同じグローバル音量」を書き換えるため、
  設定メニューの表示値も変わる（KAG3 の gvolume 相当の挙動に寄せた）。タグ実行時に
  saveSys() は呼ばず、永続化は既存のセーブ/設定変更タイミングに任せる。
- **判断**: `entersebuf` は**受理して no-op**。**理由**: KAG3(sourceforge 版) の同タグは
  SE バッファ数の事前確保だが、Noiria は `buf=` 指定でチャンネルを動的解決する（上限 16 は
  固定確保済み）ため事前確保に意味がない。**影響**: 対応表では ✅（備考に no-op と明記）。
- **判断**: `currentBgm` に `paused` を追加し、セーブデータ（`SaveData.bgm`）にも含める。
  v1 セーブには無いフィールドなのでロード時に `=== true` 判定で false 補完（v1 互換維持）。
  paused 状態のセーブをロードした場合は「play → 即 pause」で復元する（コマンドは同一
  キュー上で順に処理されるため可聴の漏れはほぼ無い）。**理由/影響**: 一時停止中セーブ→
  ロードで一時停止のまま復元される。
- **判断**: `fadebgm` の `time=` 追加は後方互換（`time` 省略/0 なら従来の即時変更のまま）。
  あわせて `mixer::set_volume` は進行中フェードと `fade_then` をキャンセルするようにした
  （即時変更が直前のフェード完了処理に上書きされない）。
- **判断**: E2E ログ検証のため、状態遷移系の W2 タグ（pausebgm/resumebgm/fadepausebgm/
  xchgbgm/fadese/bgmopt/seopt/setbgmstop/clearbgmstop）に info レベルの 1 行ログを入れた。
  **理由**: `--autotest` は無音のため、PNG にもオーディオ状態にも痕跡が残らず、ログが
  唯一の実行証跡になる。**影響**: 通常プレイでもこれらのタグ実行が info ログに出る
  （playse など高頻度タグにはログを入れていない）。
- **判断**: `wb`/`wf`/`wl` の先頭に 60ms の猶予 wait を入れた（既存 `op_audio_wait_end` と
  同じ値）。**理由**: FadeTo/Play コマンドがオーディオスレッドに届き `AudioShared` へ
  同期される（TICK=10ms）までの間に `isFading=false` を読んで誤って即抜けするのを防ぐ。
  **影響**: スキップ即抜け時（canskip=true かつ skip 中）は猶予なしの 0ms で抜ける。

### 検証結果 (W2)

- `cargo test`（ワークスペース）: 184 → **193 件全通過**（noiria-core 152→161。新規 9 件:
  mixer の `pause_holds_position_and_outputs_silence`
  `fade_to_passes_intermediate_and_reaches_target_volume`
  `fade_then_pause_pauses_at_target_without_finished` `fade_then_stop_stops_without_finished`
  `fade_to_with_zero_ms_applies_then_immediately` `multiple_se_buffers_mix_together`
  `natural_end_sets_finished_and_play_clears_it` `xchgbgm_command_sequence_crossfades_two_channels`、
  audio の `channel_index_resolves_dynamic_se_buffers_and_aliases`）。既存 184 件は無変更で全通過。
- `cd ../tsint && cargo test`: 全 12 スイート通過（コーパステスト含む。`runtime/kag/` の
  新規 TS 構文がパース可能なことを確認）。
- E2E 24 秒: V8 / tsint とも EXIT=0。両ログに `[bgmopt] [seopt] [setbgmstop] [clearbgmstop]` の
  実行が info ログで出力（初回 + F9 ロード後の再実行で各 2 回）。エラー・未対応タグ警告なし。
- E2E 90 秒（追加検証、choreography が second.ks まで到達）: V8 / tsint とも EXIT=0。
  `[xchgbgm] [pausebgm] [resumebgm] [fadepausebgm] [fadese]` を含む W2 全デモの実行を両エンジンの
  ログで確認（実行回数まで両エンジンで一致）。`[wb]/[wf]/[wl]` 通過後もシナリオが進行しており
  ハングなし（wl フォールバックの実機確認）。
- `cargo clippy --workspace --all-targets`: 変更ファイル（mixer.rs / audio.rs / engine_state.rs /
  ops_audio.rs / tsint_engine.rs / mod.rs）の warning は 0。既存 warning は W1 時点と同じ
  無関係ファイル（noiria-tools 各所、mod.rs/loader.rs の test module 配置）のもの。

## 2026-07-11 W3: 複数メッセージレイヤ

対象: `current laycount layopt hidemessage graph ptext hch autowc wc copylay resetwait` ＋既存
`cm/ct/er/position` の複数レイヤ準拠強化。

### 設計判断（判断→理由→影響、W3）

- **判断**: `[laycount messages=N]` の上限を **8** に固定した（`MAX_MSG_LAYERS`、
  `player.ts`）。**理由**: KAG3 自体に厳密な上限規定は無いが、1 メッセージレイヤ = 1
  ネイティブテキストレイヤ（fore/back 両ページ確保、`Scene::alloc_layer`）なので、タグ 1 つで
  無制限にネイティブリソースを確保できてしまうのを避けたかった。8 あれば実用上のキャラ数×
  ナレーション用途を大きく超える。**影響**: `[laycount messages=20]` のような極端な指定は 8 に
  クランプされる（無警告、対応表に明記）。
- **判断**: `MessageLayer` クラス（`{ layerId, cfg, z, visible, segs, links, activeLink,
  hoveredLink, currentText, nameLabel }`）を導入し、`KagPlayer.msgLayers: MessageLayer[]` +
  `current: { index }` を持たせた。`get cur()` が `msgLayers[current.index]` を返し、
  旧実装で `this.msgId`/`this.msgCfg`/`this.segs`/`this.links`/`this.activeLink`/
  `this.hoveredLink`/`this.currentText` を直接参照していた全箇所（printText/printControl/
  newline/clearMessage/repaint/waitAdvance/linkAt/updateHover/tryClickLink/beginLink/endLink/
  makeSnapshot/load 等）を `this.cur.*` 経由に置き換えた。**理由**: 既定状態（laycount 未使用）
  では `msgLayers` は要素 1 個（message0）のみで `cur` は常にそれを指すため、**既存の呼び出し列
  ・見た目が W2 時点と完全一致する**ことを型レベルで保証しやすい設計にした。**影響**: 新規に
  `runtime/kag/` 配下のコードを書く際は `p.msgId` 等の旧フィールドが無いことに注意
  （コンパイルエラーではなく実行時 `undefined` になる tsint/V8 双方の性質上、grep で旧フィールド
  参照が残っていないことを確認済み）。
- **判断**: `applyMessageLayerConfig(idx)`（旧 `applyMessageConfig()` の複数レイヤ対応版）は
  **msgbox → `{x,y}` の `layer.set` → `text.style` → `{z,visible}` の `layer.set`** という
  4 ステップを、旧実装と完全に同じ呼び出し順序・分割で発行する。**理由**: tsint リポジトリ
  （読み取り専用）の `tests/kag_full.rs` に、初期化時の op 呼び出し列を厳密に検証する
  回帰テスト（`kag_player_runs_full_scenario_to_close_via_module_graph`）があり、最初
  `{x,y,z,visible}` を 1 回の `layer.set` にまとめて実装したところ実際に失敗した
  （実機で確認済み）。**影響**: 一見冗長な 2 回の `layer.set` 呼び出しは意図的な後方互換維持で、
  まとめて 1 回にするリファクタは tsint リポジトリのテストを壊すため今後も避けること。
- **判断**: `[hidemessage]` と既存の右クリック一時非表示は **同じ `msgVisible` フラグ**を共有し、
  `applyGlobalVisibility()` が全 `msgLayers` へ `visible: msgVisible && ml.visible` を適用する
  よう統一した。**理由**: 「次のクリックで復帰」という既存の右クリック挙動と `[hidemessage]` の
  要求仕様が同一だったため、フラグを分けると「両方 true にならないと表示されない」等の混乱を生む。
  **影響**: 既定 1 レイヤ構成では右クリック非表示の見た目は無変更（`ml.visible` は既定 true
  なので `msgVisible` だけで決まる）。`[layopt visible=false]` で個別に隠したレイヤは、右クリック
  で全体を復帰させても隠れたままになる（意図した挙動）。
- **判断**: `cm`=全レイヤクリア、`ct`=`cm` + `current.index=0` + 名前欄再描画、`er`=カレントのみ
  クリア、と実装を分離した。`clearAllMessages()`/`clearMessage()`（カレント版、後方互換で同名）
  はどちらも `flushBacklog()` を 1 回だけ呼んでから各レイヤの `clearLayerContent()` を呼ぶ
  （バックログの `paragraph` はレイヤ非依存のグローバル状態のまま据え置き、複数回 flush しても
  2 回目以降は空文字列で無害）。**影響**: 既定 1 レイヤ構成では cm/ct/er の見た目は完全に同一
  （既存シナリオ・PNG は無変更）。
- **判断**: バックログ（`backlog`/`paragraph`）・`currentFontState`/`fontDefaults`/
  `styleDefaults`/`pendingRuby` は **レイヤ非依存のグローバル状態のまま**にした（`MessageLayer`
  へ移さなかった）。**理由**: 対応表・設計方針に明記された `MessageLayer` の保持フィールドに
  含まれておらず、W1 で「`[cm]`/`[er]` でもフォント状態はリセットしない」という既存の設計判断
  との整合を保つ方が単純。**影響**: レイヤをまたいでも文字装飾状態・ルビ予約は共有される
  （例: message1 で `[font color=...]` した直後に `[current layer=message0]` すると、その色が
  message0 の次の printText にも乗る）。バックログも全レイヤの発言を時系列 1 本にまとめて記録する
  （レイヤ別の履歴分離はしない）。
- **判断**: `position`/`layopt` タグの `page=` 属性は受理するが、実際の描画には影響しない
  （備考に明記）。**理由**: Rust 側 `EngineState::text_print`/`text_style` は常に `"fore"`
  ページへ書き込む実装（`ops_text.rs`/`engine_state.rs` の `with_layer("fore", id, ...)` 決め打ち）
  になっており、テキストレイヤの back ページ描画自体が既存実装に無い。これを追加するには
  `op_text_print`/`op_text_style`/`op_text_clear`/`op_text_is_done`/`op_text_rects` 全てに
  page 引数を追加する必要があり、W3 のスコープ外と判断した（トランジション拡張時の W5 以降で
  再検討）。**影響**: `[current layer=message0 page=back]` のような指定は受理されるが見た目には
  効かない。`[position]`/`[layopt]` の `page=` も同様。
- **判断**: `graph`（インライン画像）は `TextSegment.inline_image: Option<String>`
  （解決済みパス）を追加し、`TextEngine` に `image_cache: HashMap<String, Pixmap>` を持たせて
  `cache_image()`/`has_image()` で読み書きする設計にした。実ファイル I/O（`GameDir::load_image`）
  は `text.rs` ではなく **呼び出し側の `EngineState::text_print`** が `text_print` の冒頭で
  `inline_image` を持つセグメントを見つけるたびに行う（未キャッシュの場合のみ）。**理由**:
  設計方針に明記された「text.rs から assets への依存を増やしたくない場合は、ラスタ時に呼び出し側
  から渡す設計でも良い」を採用。`text.rs` は `noiria-core` の中でも外部 I/O を一切行わない
  純粋なレイアウト/ラスタ層という既存の設計原則（モジュール冒頭コメント参照）を保てる。
  **影響**: 同じ画像パスを複数回 `[graph]` しても 2 回目以降はキャッシュヒットしディスクを
  再読込しない（プロセス生存中は無期限キャッシュ、明示的な解放 API は無い。ノベルゲームの
  インライン画像は種類が少数のため実用上問題にならない想定）。
- **判断**: インライン画像 1 個は「1 グリフ扱い」として `Resolved`/`LineChar` に直接
  push する（`locate`/`indent` と同じ「制御専用ダミー文字」パターンを踏襲しつつ、こちらは
  実際に `pixmap: Some(...)` を持つ実グリフとして扱う）。ベースライン下端揃えは、Pass3 の
  座標式 `x = pen_x + left - pad` / `y = baseline_y - top - pad`（`pad = style.outline_width`）
  を**変更せずに** `left = pad`, `top = height - pad` を Pass1 側で逆算することで実現した
  （Pass3 のコード自体は無改変）。行送り (`line_advance`) は不変のまま、画像が行の上下に
  はみ出すことを許容する（ルビの `ruby_extra` 方式と同じ「はみ出し許容」の設計思想を踏襲）。
  advance（次のペン位置への移動量）= 画像の `width`。**影響**: 極端に大きい画像を `[graph]`
  すると、行の上下に大きくはみ出して表示される（今回のサンプルデモでは 28×28px の小アイコン
  `games/sample/assets/fg/icon_spark.png` を使い、実用上問題ないサイズで確認した）。
- **判断**: `[graph]` 用のデモアイコン画像は既存の `noiria-tools`（`gen-assets` バイナリ、
  `main.rs`）を拡張せず、**独立した新規バイナリ `gen-w3-icon`**
  （`crates/noiria-tools/src/gen_w3_icon.rs`）として追加した。**理由**: リポジトリが
  git 管理下に無く（作業ディレクトリが非 git）差分確認ができないため、`gen-assets` の
  `main()` を丸ごと再実行して全アセットを再生成するのは、手続き生成が決定論的だとしても
  意図しない副作用のリスクを避けたかった。既存アセット（room.png 等）には一切触れず、新規
  アイコン 1 枚だけを生成する専用バイナリにすることでリスクをゼロにした。**影響**:
  `paint.rs` を 2 つのバイナリ（`gen-assets`/`gen-w3-icon`）が `#[path]` で個別にコンパイルする
  形になり、`gen-w3-icon` 単体では `paint.rs` の他関数が未使用警告になるため
  `#[allow(dead_code)]` を付けた（`gen-assets` 側の使用状況には影響しない）。
- **判断**: SaveData を **v2** へ拡張し、`msgLayers: { cfg, visible, nameLabel }[]` と
  `current: number` を追加した。`name: string | null`（カレントレイヤの `nameLabel` の写し）は
  v1 互換のため残す。`migrateSaveV1toV2(data): SaveData` という純関数を `player.ts` に追加し
  （`effectiveVolume`/`stepStage` 等と同じ「pure function export + V8 js_tests での単体テスト」
  パターン）、`version < 2` のデータに対して `msgLayers: data.msgLayers ?? []`,
  `current: data.current ?? 0` を補完する。`load()` はまず `migrateSaveV1toV2()` を通してから、
  `data.msgLayers.length > 0`（= v1 由来ではなく実際に v2 形式でレイヤ状態が記録されている）
  の場合のみ `setLayCount()` + 各レイヤの `cfg`/`visible`/`nameLabel` を復元する分岐へ進み、
  それ以外（v1 セーブ、または laycount 未使用のまま保存された v2 でも `msgLayers` が空になる
  ことは無い＝実質 v1 のみ該当）は**現在実行中のレイヤ構成に一切触れず** `data.name` だけで
  名前欄を復元する旧来の経路にフォールバックする。**理由**: v1 セーブには
  メッセージウィンドウの位置・サイズ情報が最初から記録されていなかった（`position` タグの変更も
  v1 では保存されない既存の制限）ため、`migrateSaveV1toV2` が「それらしい値」を捏造するのは
  誤解を招く。空配列を「補完不能」の明示的なシグナルとして扱う設計にした。**影響**: v1 セーブを
  ロードしても laycount の状態は復元されない（そもそも v1 が存在した時点では laycount 機能が
  無かったので実害は無い）。v2 セーブ（本 wave 以降に作成されたもの）は複数レイヤの位置・表示・
  名前欄まで完全に復元される。
- **判断**: `[wc n=]` は「n 文字分 (= n × 1000/cps ms) の単純な時間待ち」として実装した（クリック
  待ちは伴わない）。**理由**: 詳細設計の指示文言が「wc: time 相当（n 文字分 = n × 現在 cps の
  待ち）」と明記されていたため、これを字面どおり採用した。`docs/kag-tag-compat.md` の旧稿にあった
  「指定レイヤのクリック待ち」という説明（実装着手前の下調べ時点の記述）とは異なる解釈になるが、
  今回のウェーブ指示を優先した。**影響**: 対応表の `wc` の備考をこの解釈に合わせて更新した。
- **判断**: `[autowc enabled= ch= time=]` は `KagPlayer.autowc` （`{enabled, ch, time}`、既定
  `{false, "", 0}`）という状態を持たせ、`execCommand` の `"text"` ケースで
  `autowc.enabled && autowc.ch` なら `printTextWithAutoWait()` （1 文字ずつ蓄積し、指定文字が
  出るたびに `printText()` で確定 + `wait(autowc.time)`）を使う設計にした。Rust 側 `reveal`
  ロジックは無改変（TS 側で `print` 呼び出しを分割するだけ）。**理由/影響**: 設計方針に明記
  された通りの実装。**判明した不具合と修正**: `printTextWithAutoWait` は複数回 `await
  this.wait(...)` するループのため、ループ途中でセーブ/ロードやリンク選択によるジャンプ
  （`requestJump()` → `fireInterrupt()`）が発生すると、`wait()` が中断されても例外を投げず
  素通りする（既存の `KagPlayer` 全体の方針）ため、ジャンプ後にクリア済みのレイヤへ**旧シナリオ
  位置のテキストが紛れ込む**不具合を E2E 検証で発見した（`--autotest 24` で F9 ロード直後の
  フレームに旧文＋新文が混在して表示された）。各 `wait()` の後に `this.jumpRequest !== null` を
  チェックして早期 return するよう修正し、再検証で解消を確認した（`docs/implementation-notes.md`
  に記録する経緯そのものがこの節）。既存の `waitAdvance()` 等は 1 回の await で完結するか
  `"int"` を明示チェックしているため、この種の不具合は今回新設した複数 await ループ特有だった。
- **判断**: `[hch text=...]` は縦中横を実装せず、`p.printText(a.text, null)` で平文表示する
  フォールバックとし、`KagPlayer` インスタンスごとに初回だけ `warn` ログを出す（W1 の
  `warnStyleAttrs` と同じパターンを踏襲）。**理由**: 縦書きモード自体がこのエンジンに存在せず
  （`text.rs` は横書き専用のレイアウトエンジン）、縦中横だけ単独対応する意味が薄い。**影響**:
  `docs/kag-tag-compat.md` に明記。
- **判断**: `[resetwait]` は `KagPlayer.waitBaseMs = api.sys.now()` を記録するだけで、他のどの
  コードからも参照しない。**理由**: 対象タグの元々の効果（文字待ちカウンタのリセット）は、KAG3
  では主に `[wait mode=until]`（絶対時刻指定の待ち）と組み合わせて使うが、Noiria の `[wait]` は
  相対時間待ちのみ実装しており `mode=until` 自体が無いため、リセット対象が実質存在しない。
  **影響**: `docs/kag-tag-compat.md` に「基準時刻の保存のみ、他に参照箇所なし」と明記。
- **判断**: `[copylay srclayer= destlayer= srcpage= destpage=]` は新規 op `layer_copy`
  （`Scene::copy_layer` + `EngineState::layer_copy` + `op_layer_copy`、V8/tsint 両対応）で
  実装した。`Scene::copy_layer` はコピー先の **z とアニメーション中の tween はコピーしない**
  （pixmap/text/image_path/x/y/opacity/visible のみ）。**理由**: コピー先レイヤの重ね順や
  進行中のアニメーションをコピー元の値で意図せず破壊しないため（`backlay` は同一 id の
  fore→back 全体コピーなので z/tween が元々同じで問題にならないが、`copylay` は異なる id 間の
  コピーなので同じ流儀を踏襲すると事故りやすい）。**影響**: メッセージレイヤ間の `copylay` は、
  コピー先レイヤの `segs`/`links`/`currentText`（TS 側ミラー）とネイティブ側の実描画内容が
  ズレる（ネイティブは複製された内容、ミラーは空）。ホバー/リンク判定はミラー基準なので
  copylay 後のレイヤはリンクが効かなくなる（tags.ts の copylay ハンドラで明示的にミラーを
  空にして「効かない」ことを一貫させた。次の `[cm]`/`[er]` で解消する）。

### 検証結果 (W3)

- ベースライン: `cargo test`（ワークスペース） 193 件全通過。
- 実装後: 199 件全通過（noiria-core 161→166、新規 5 件: `inline_image_renders_as_one_glyph_
  with_baseline_bottom_alignment` `inline_image_unknown_path_is_ignored_as_zero_width`
  `appending_keeps_prefix_glyph_count_with_inline_image`（text.rs）、
  `copy_layer_copies_content_but_not_z_or_tween` `copy_layer_missing_src_is_a_noop`（scene.rs）。
  noiria-app 14→15、新規 1 件: `kag_player_save_v1_to_v2_migration_in_v8`（script/mod.rs、
  `migrateSaveV1toV2` の純関数テスト）。既存 193 件は無変更で全通過（新フィールドは全て
  デフォルト/オプショナルのため回帰なし）。
- `cd ../tsint && cargo test`: 全 12 スイート通過。当初 `tests/kag_full.rs` の
  `kag_player_runs_full_scenario_to_close_via_module_graph`（op ログ列の厳密比較）が
  1 件失敗し、`applyMessageLayerConfig` の呼び出し順序（上記の設計判断参照）を旧実装と
  完全一致させて解消した（tsint リポジトリ自体は読み取り専用のため無変更）。
- E2E: V8 (`--autotest 24 --autotest-out target/autotest-w3`) / tsint
  (`--engine tsint --autotest 24 --autotest-out target/autotest-w3-tsint`) いずれも EXIT=0。
  両エンジンのログに `[hch]` の警告（縦中横フォールバック）が出力され、`未対応タグ` 警告は
  0 件（`laycount`/`current`/`layopt`/`hidemessage`/`graph`/`ptext`/`wc`/`autowc`/`resetwait`/
  `copylay` 全て解決されたことを確認）。`frame_08_hover.png`（両エンジンでピクセル同一）で
  第 2 メッセージレイヤ (message1) とインライン画像 (`icon_spark.png`) の表示を確認済み。
- 後方互換: `frame_04_hover.png`〜`frame_07_hover.png`（laycount 未使用の区間）のメッセージ
  ウィンドウの位置・サイズ・フォント・ルビ表示は W2 時点のサンプルと同一の見た目
  （固定レイアウト値は変更していないため当然だが、実フレームで目視確認済み）。
- 副次的に発見・修正した不具合: `printTextWithAutoWait` のジャンプ未検出（上記設計判断参照）。
  E2E の F5 セーブ→F9 ロードのタイミングで実際に再現・修正・再検証した。
- 観測した既知の一過性事象（バグではない）: autotest の 2.5 秒間隔スクリーンショットが、
  F9 ロード後のシナリオ再生（`*start` からの再実行、先頭の `[trans method=crossfade]`
  を再度通過する）の**トランジション進行中の瞬間**を捉えることがある（`frame_09_hover.png`）。
  V8/tsint で完全にピクセル同一の内容であり、決定論的な trans 進行の途中経過を撮っただけで
  W3 由来の不具合ではないことを確認した（次のフレームでは正常な表示に戻る）。
- `cargo clippy --workspace --all-targets`: 変更ファイル（text.rs / engine_state.rs / scene.rs /
  ops_layer.rs / tsint_engine.rs / mod.rs / gen_w3_icon.rs）の warning は 0
  （新規テストの `clone()` → `slice::from_ref()` 指摘を 1 件修正済み）。既存 warning は
  W1/W2 時点と同じ無関係ファイルのもの。

## 2026-07-11 W4: UI・ボタン・クリック制御

対象: `button checkbox edit commit input glyph click cclick wheel cwheel timeout ctimeout
waitclick waittrig locklink unlocklink clickskip nextskip cancelautomode rclick` ＋ `ask`
（sourceforge 版拡張）。

### 設計判断（判断→理由→影響、W4）

- **判断**: 新モジュール `runtime/kag/buttons.ts`（`ButtonManager` クラス + `stateClip`/
  `hitRect`/`applyEditKey` の純関数）を新設し、button/checkbox/edit の登録・描画・ヒットテスト・
  状態遷移をここに閉じ込めた。import は **player.ts → buttons.ts の一方向のみ**（buttons.ts は
  player.ts を import しない。`KagPlayer` 型に依存せず `api`/`vars` だけを使う自己完結モジュール
  にした）。tags.ts からは `p.buttons` 経由でのみ触る。**理由**: W1 で発覚した
  `player.ts ⇄ tags.ts` の循環 import 事故（tsint が import を評価順に解決するため
  `TypeError` になる）と同じ罠を避けるため、新規モジュールを増やす際は依存方向を必ず
  一方向に保つという既存ルールを踏襲した。**影響**: `ButtonManager` は `KagPlayer` のフィールド
  （`p.buttons = new ButtonManager()`）として保持し、locklink 等の連携（`p.linksLocked` と
  `ButtonManager.globalLocked` の 2 つを両方更新する）は tags.ts のハンドラ側で明示的に束ねている
  （多少の重複だが、モジュール間の型結合を増やさずに済む）。
- **判断**: 3 状態ボタン画像の切替は新規 op `layer_blit_image(id, page, path, dx, dy, clip?)`
  （`Pixmap::extract_region` + `Pixmap::blit_over` を使い、既存レイヤへ画像の部分矩形を
  「消さずに」上書き合成する。V8 (`ops_layer.rs`)/tsint (`tsint_engine.rs`) 両対応）で実装した。
  レイヤに `pixmap` が無い場合は `clip`（省略時は画像全体）のサイズで透明キャンバスを自動確保する。
  ボタン側は毎回 `layer.fill`（透明クリア）→ `layer.blitImage`（該当スライス）の 2 op で状態を
  再描画する（透過ピクセルが残る合成事故を避けるため、上書き前に必ずクリアする）。**理由**:
  W5 の `[pimage]`（画像レイヤへの部分画像重ね描き）と共用できる汎用 op にしておく設計方針が
  指示されていたため、button 専用にせず「画像ファイルの部分矩形を任意レイヤへブリット」という
  最小の意味論にした。**影響**: `[button graphic=...]` は内部で `layer.loadImage`（画像全体を
  読み込みつつ寸法 `[totalW, totalH]` を取得するためだけに使う）→ `layer.fill`（1 状態ぶんの
  サイズへキャンバスを縮小しクリア）→ `layer.blitImage`（normal スライス）の 3 op を初期登録時に
  発行する。画像サイズ専用の新規 op を増やさず、既存 `layer.loadImage` の戻り値を流用する設計に
  した（トレードオフ: 初期登録時に画像を 2 回実質デコードする一時的な無駄があるが、シナリオ内で
  button 登録は高頻度ではないため許容）。
- **判断**: button/checkbox/edit の活性化（ホバー/クリックへの反応）は **`ButtonManager.active`**
  フラグで一元管理し、`KagPlayer.waitForJump()`（`[s]` タグの実体）の開始時に true、終了時に
  false へ切り替える。**理由**: 詳細設計方針に明記された「button/checkbox/edit の活性化は `[s]`
  停止中のみ（KAG 準拠）」をそのまま実装した。**影響**: `[button]`/`[checkbox]`/`[edit]` タグは
  `[s]` より前に置いても表示だけは行われる（レイヤは即座に作成・描画される）が、クリック/ホバーに
  反応するのは後続の `[s]` に到達してから。`waitForJump()` を通らない `[l]`/`[p]`（文章送り待ち）
  の間はボタン等が画面にあっても無反応（既存のリンク機構と役割が重複しないよう、意図的に `[s]`
  専用の活性化にした）。
- **判断**: `[locklink]`/`[unlocklink]` は「リンク・ボタン・クリック送り」の 3 つをまとめて
  ロックする（`KagPlayer.linksLocked`: `waitAdvanceEvent()` の左クリック分岐と
  `tryClickLink()` 呼び出しの両方をガード、`ButtonManager.globalLocked`: button/checkbox/edit の
  ヒットテストをガード）。**理由**: ユーザー指定のタグ仕様が「リンク・ボタン・クリック送りの
  不活性化」と 3 つを列挙していたため、字面どおり実装した。**影響**: `[locklink]` 中でも
  Enter/Space キーでの文章送りは引き続き有効（意図的な設計、`waitAdvanceEvent()` のキー分岐は
  ガードしていない）。
- **判断**: `[click]`/`[wheel]` の常設フックは `KagPlayer.eventJumps: {click, wheel}` という
  シンプルな Map ではなく直接オブジェクトで持たせ、`installGlobalHandlers()` の該当イベント分岐の
  **先頭**（他のどの処理より前）でチェックする設計にした。**理由**: KAG のクリック/ホイールフックは
  「ページの他の要素より優先する常設フック」という性質であり、既存のリンククリックや
  msgVisible トグルより後に判定すると、フックが「たまたま他の判定に消費されなかった場合だけ」
  発火する不自然な挙動になる。**影響**: `[click]` 設定中は、たとえ選択肢リンクが表示されていても
  クリックは常にフックの target へジャンプする（リンクは無視される）。サンプルではこの性質を
  逆手に取り、干渉を避けるため「設定→同じ行内で即 `[cclick]` 解除」という短い区間でのみ使う
  （`docs/kag-tag-compat.md` の備考、および下記サンプル追加の判断も参照）。
- **判断**: `[timeout]` はクリック等による自動キャンセルを一切行わない、**単純な実時間タイマー**
  （`KagPlayer.setTimeoutJump()` が `await api.sys.wait(ms)` 後に世代カウンタを確認してから
  ジャンプする、`watchBgmStop`/`setBgmStop` と同じ「世代カウンタでの自然消滅」パターン）として
  実装した。**理由**: KAG3 の `[timeout]` は本来「クリック待ちのタイムアウト」だが、Noiria の
  `[l]`/`[p]`/`[s]` はいずれもクリック以外（Enter キー、他のジャンプ等）でも終了しうる非同期待ちで
  あり、「クリックだけをキャンセル条件にする」実装は既存の `waitAdvance()`/`waitForJump()` の
  待ち合わせロジックに深く踏み込む必要がありスコープ超過と判断した。単純な実時間タイマーの方が
  「ヘッドレス E2E で検証しやすい（クリックが来ない時間帯に発火）」というユーザー指示の意図にも
  合致する。**影響**: `[timeout]` 設定後、`[ctimeout]` で明示的に解除しない限り、スクリプトが
  どこまで進んでいても指定時間後に強制的にジャンプする（KAG3 の「クリック待ち限定」の挙動より
  広い）。サンプルでは `[timeout]` の直後に `[s]`（他にジャンプ手段が無い停止）を置くことで、
  「timeout が唯一の抜け道」という安全な使い方に限定した。
- **判断**: `[waittrig name=...]` の発火は KAG の `kag.trigger(name)` 相当を、Noiria では
  **`sys.player.trigger(name)`**（`[eval exp="sys.player.trigger('x')"]`）として実装した。
  `KagPlayer.trigger()`/`waitTrigger()` は名前ごとの `resolve` 関数リスト（`Map<string,
  (()=>void)[]>`）で待ち合わせる素朴な実装。**理由**: Noiria の `[eval]`/`[link exp=]` 等は
  既存の `sysObject()`（`{save, load, menu, backlog, quit, player}`）を評価コンテキストとして
  使っており、`kag` というグローバル識別子は存在しない。新たに `kag` グローバルを式評価コンテキストへ
  追加するのは影響範囲が広い（`vars.evalExpr`/`evalStmt` のシグネチャや全呼び出し箇所に影響）ため、
  既に公開済みの `sys.player`（KagPlayer インスタンスそのもの）経由でアクセスさせる方が変更が
  局所的。**影響**: KAG3 のシナリオをそのまま移植する場合は `kag.trigger` を
  `sys.player.trigger` に書き換える必要がある（対応表に差異として明記）。
- **判断**: `checkbox`/`edit`/`input`/`ask` の変数バインドは、専用のドット区切りパーサを書かず
  **`vars.evalExpr`/`evalStmt` に式文字列としてそのまま渡す**（例: `checkbox name="f.flag"` →
  内部で `` `${varExpr} = ${next}` `` を `evalStmt` する）方式にした。**理由**: `[eval exp=]`/
  `[link exp=]` など既存タグと同じ経路を再利用でき、`f.x`/`sf.x`/将来のネストしたパス
  （`f.obj.flag` 等）を単一の実装で扱える。CLAUDE.md の「単純に済むところを複雑にしない」方針にも
  合致する。**影響**: `name=` 属性はシステム上「代入可能な JS 式」であれば何でも通ってしまう
  （例えば `name="(function(){...})()"` のような濫用も可能）が、シナリオ作者を信頼する既存の
  `eval`/`link exp=` と同じ信頼モデルを踏襲しているだけなので新たなリスクではないと判断した。
- **判断**: `[ask]`（sourceforge 拡張）と `[input]` はどちらも「自作リンク/オーバーレイ」として
  **実際に実装**した（受理+warn の簡易実装フォールバックは採用しなかった。`openSaveMenu` の
  自作 UI 前例があり、費用対効果が見合うと判断）。ただし当初、`[ask]` を「はい」「いいえ」の行
  厳密ヒットテストのみで実装したところ、**`--autotest` の固定クリック座標（choreography、
  y=540-623 帯）がダイアログの行（画面中央、y≈360-410 帯）に一切重ならず、いつまでもクリックが
  来ずにハングする**ことが実装中の検証で判明した。`uiLock=true` の間は F5/F9/Escape 等の
  グローバルキー処理も `if (this.uiLock) return;` で止まるため、ハングすると後続の
  save/load/menu の choreography も巻き添えで動かなくなる。**修正**: `[input]` と同じ
  「**最初に届いた左クリック/Escape で必ず閉じる**」方式に変更した（行に厳密にヒットしなければ
  `ask` は既定で「いいえ」、`input` は「現在のバッファ内容で確定」）。**影響**: 座標を厳密に
  把握しない自動入力（ヘッドレス choreography、あるいは他の自動テストツール）でも
  `[ask]`/`[input]` がフリーズしない。手動プレイでは「はい」の行をクリックすれば厳密に
  「はい」と判定される（この経路は `button_manager_hit_test_and_state_transitions_in_v8` と
  同様のパターンでピンポイント座標を使えば単体テストも可能だが、W4 では E2E での回帰防止を
  優先し専用の Rust テストは追加していない）。
- **判断**: `[glyph]` の `fix=`（動的にペン位置へ追従させる版）は非対応とし、常に固定位置
  （`left=`/`top=` 省略時はメッセージウィンドウ右下）として扱う。**理由**: このエンジンには
  そもそも「クリック待ち記号」の専用表示機構が無かった（KAG のようなデフォルトの点滅三角アイコン
  等が実装されていない）ため、動的追従先となる「現在のペン位置」を `[l]`/`[p]` の待ち状態と
  同期して取り回す実装はスコープ超過と判断した。**影響**: `docs/kag-tag-compat.md` に明記。
  `line=` 未指定の既定状態（無表示）は W1〜W3 時点の見た目を完全に維持する（新しいレイヤは
  `glyphCfg.path` が truthy になるまで作成されない）。
- **判断**: `[clickskip enabled=]` は「クリックによるスキップモード**解除**の可否」として解釈した
  （`installGlobalHandlers` の既存の `if (this.mode.skip) this.mode.skip = false;` を
  `clickSkipEnabled` でガードする）。**理由**: KAG3 の `clickskip` の厳密な挙動（スキップ中の
  クリックでスキップを継続させるか等）は資料により解釈の揺れがあるため、既存コードに実在する
  「クリックでスキップ解除」という唯一のクリック関連スキップ挙動を on/off する解釈を採用した
  （CLAUDE.md の「複数の解釈があれば明示する」方針に基づき、ここに解釈を明記する）。**影響**:
  `[clickskip enabled=false]` の間、スキップモードは S キーでのみ解除できる。
- **判断**: `[rclick]` は `"default" | "disabled" | {call, storage, target}` の判別共用体
  （`KagPlayer.rclickAction`）で状態を持たせた。属性なしの `[rclick]` は明示的に `"default"`
  （既存の全レイヤ表示トグル）へ戻す。**理由**: ユーザー指定の属性列挙（`enabled= jump= call=
  storage= target=`）から、`jump=true`/`call=true` を「モード選択」、`storage=`/`target=` を
  「ジャンプ/コール先」とみなす設計が最も自然だった。**影響**: `[rclick enabled=false]` は
  右クリックを完全に無反応にする（KAG 標準のメッセージ非表示トグルも起きなくなる）。
- **判断**: サンプル（`games/sample/`）の W4 デモは、`docs/kag-tag-compat.md`/設計指示の
  「既存クリック点に干渉しない」方針に従い、画面右上（x=1000 付近、y=40-186）に button/checkbox/
  edit を配置した（choreography のクリック 8 点は x=160-640/y=540-623 の帯のみ）。`[click]`/
  `[wheel]`/`[locklink]`/`[rclick]` の常設フック系は「設定して同じ行内で即座に解除」する区間に
  限定し、実際には一度も発火しないようにした（フック自体のロジックは
  `button_manager_hit_test_and_state_transitions_in_v8` 等の単体テストで別途検証済み）。
  `[timeout]` は唯一の抜け道として `[s]` の直後に置き、`[waitclick]`/`[ask]`/`[input]` は
  「クリック位置を問わない」設計にしたため choreography のどの座標でも安全に進行する。
  デモの末尾で `[eval exp="sys.player.buttons.clear()"]`（`ButtonManager.clear`。専用タグは
  KAG3 に無いため eval 経由）を呼び、デモ用ウィジェットを後続のシーン（second.ks 以降）へ
  持ち越さないようにした。**影響**: first.ks の `*go_street` 末尾（第2シナリオへのジャンプ直前）
  に約 15 行、second.ks の最終選択肢直前に `[input]` のデモを 2 行追加した。追加分の待ち時間
  （実クリック待ち 3〜4 回 + `[timeout]` の 1.2 秒）だけ、後続のシナリオ位置が実時間ベースの
  choreography（F5=9s 等の絶対時刻）に対して相対的に遅れる。これは W2/W3 でも同様の性質があった
  （サンプル追加のたびに実時間との対応がずれる）ため、既存の許容方針を踏襲した。
- **副次的に発見した既存不具合（W3 由来、W4 のスコープ外）**: `[copylay]` の実体
  `Scene::copy_layer`（`scene.rs`）はコピー先の **x/y（ネイティブ描画位置）もコピー元からコピー
  してしまう**。`destlayer=` に**メッセージレイヤ**を指定すると、その後 `[position]`/`[layopt]`/
  ロードで明示的に位置を再設定するまで、当該メッセージレイヤの表示位置がコピー元の位置に恒久的に
  変わってしまう。W4 の E2E 検証（`--autotest 120` 相当の長時間実行）で、`games/sample/first.ks`
  の `[copylay srclayer=message1 destlayer=message0]`（W3 で追加済み、top=90 に配置していた
  message1 の内容を message0 へコピーするデモ）実行後、message0（既定のメッセージウィンドウ）が
  ずっと画面上部（message1 の元位置）に表示され続けることを発見した。**発生条件**: F9 ロードが
  ちょうど `[copylay]` 実行前のタイミングで発生し、`*start` からの再実行（W3 で既知の挙動）で
  `[copylay]` が改めて実行される場合にのみ顕在化する。標準の `--autotest 24` コマンドではこの
  タイミングに到達する前に検証時間が終わるため、24 秒の検収では影響しない（`frame_04`〜
  `frame_08_hover.png` で目視確認済み、W3 時点と同一の見た目）。**対応**: `copylay`/`copy_layer`
  自体は W3 で実装済みの既存コードであり、CLAUDE.md の「既存コードは壊れていなければ触らない」
  方針と W4 のタグ範囲外であることから、本ウェーブでは**修正しない**（`docs/kag-tag-compat.md`
  の `copylay` 備考に既知の副作用として明記するに留めた）。修正するなら
  `Scene::copy_layer` から x/y のコピーを外す（あるいは呼び出し側でコピー後に
  `applyMessageLayerConfig` を呼び直す）案が考えられるが、判断はユーザー確認後に別途行うべき
  スコープと考える。

### 検証結果 (W4)

- ベースライン: `cargo test`（ワークスペース） 199 件全通過。
- 実装後: 205 件全通過（noiria-core 166→168、新規 2 件: `pixmap::tests::extract_region_copies_subrect`
  `pixmap::tests::extract_region_out_of_bounds_is_transparent_not_panic`。noiria-app
  15→19、新規 4 件: `script::engine_state::tests::layer_blit_image_clips_and_composites_onto_existing_layer`
  `script::engine_state::tests::layer_blit_image_creates_canvas_when_layer_has_no_pixmap_yet`
  `script::js_tests::kag_buttons_pure_functions_in_v8`
  `script::js_tests::button_manager_hit_test_and_state_transitions_in_v8`）。既存 199 件は無変更で
  全通過（新フィールドは全てデフォルト付き/オプショナルのため回帰なし）。
- `cd ../tsint && cargo test`: 全 12 スイート通過（`tests/kag_full.rs` の
  `kag_player_runs_full_scenario_to_close_via_module_graph`（op ログ厳密比較）含む。
  `NoiriaHost::resolve`/`load` は `noiria:xxx` を `../noiria/runtime/xxx.ts` へ動的に解決する実装
  のため、`runtime/kag/buttons.ts` を新設しても tsint リポジトリ側は無変更で自動的に解決できた
  （固定のモジュール一覧を持っているのは構文コーパステスト `tests/common/mod.rs`/`corpus.rs` の方
  だけで、そちらは対象ファイルを増やさなくても既存ファイルの構文が壊れていなければ影響しない）。
- E2E（必須コマンド）: `cargo run -p noiria-app -- games/sample --autotest 24
  --autotest-out target/autotest-w4` / `--engine tsint --autotest 24
  --autotest-out target/autotest-w4-tsint` いずれも EXIT=0。ログは両エンジンで完全に同一の
  順序・内容（`[bgmopt]`→`[seopt]`→`[setbgmstop]`→`[clearbgmstop]`→セーブ→`[hch]`警告→ロード→
  （再度）`[bgmopt]`...→セーブメニューを開きます）。24 秒では W1〜W3 の初期デモ区間まで
  （`*start` の前半、F5 セーブ・F9 ロード・Escape メニューの choreography）しか進まないため、
  新規 W4 タグの実行ログはまだ現れない（`laycount`/`current` 等 W3 タグも同様に、24 秒区間の
  frame 検証は主に「後方互換（レイアウト崩れなし）」の確認が目的）。
- E2E（拡張検証、W2 の 90 秒相当の precedent に倣う）: `--autotest 120`
  （`target/autotest-w4-ext` / `--engine tsint target/autotest-w4-tsint-ext`）で choreography が
  first.ks の W4 デモ区間から second.ks・title.ks の再訪問まで到達することを確認した。
  両エンジンのログが `[button]`→`[checkbox]`→`[edit]`→`[glyph]`→`[click]`→`[cclick]`→`[wheel]`→
  `[cwheel]`→`[locklink]`→`[unlocklink]`→`[rclick]`（×2）→`[waitclick]`（×2、待機開始/受信）→
  `[nextskip]`→`[ask]`（×2、text/answer）→`[timeout]`→`[ctimeout]`→`[cancelautomode]`→
  `[clickskip]`→`[commit]`→（W2 デモ）→`[input]`（×2、title/終了）という**完全に同一の順序**で
  一致することを確認（V8 は 120 秒以内に `*bye`→`[close]` に到達しスクリプト側から終了要求、
  tsint はタイトル画面の「つづきから」リンク（`sys.menu('load')`）でセーブメニューを開いた状態で
  時間切れ、という良性の差はあるが、いずれもクラッシュ・ハング・エラーログなし）。
  `target/autotest-w4-ext/frame_34_hover.png` と `target/autotest-w4-tsint-ext/frame_37_hover.png`
  （どちらも W4 デモの button/checkbox/edit 表示直後の内容が一致するフレーム）をピクセル比較し、
  差分は画面右下の装飾円（背景アニメーションの位相差、W4 と無関係）のみで、button/checkbox/edit/
  テキストの領域は完全一致を確認した。
- 後方互換: `frame_07_hover.png`（font/ruby 装飾）・`frame_08_hover.png`（message1 サブレイヤ +
  インライン画像）を W3 時点の記述と目視突き合わせ、レイアウト・内容とも無変更を確認した
  （標準の 24 秒検収範囲では上記の copylay 副作用の影響を受けない）。
- `cargo clippy -p noiria-core -p noiria-app -p noiria-tools --all-targets`: 変更ファイル
  （pixmap.rs / engine_state.rs / ops_layer.rs / tsint_engine.rs / mod.rs / loader.rs /
  gen_w4_button.rs / buttons.ts 等）で新規 warning は 0。既存 warning は W1〜W3 時点と同じ
  無関係ファイル（noiria-tools の paint.rs/synth.rs/analysis.rs 等、mod.rs/loader.rs の
  test module 配置）のもの。

## 2026-07-11 W5: トランジション/アニメ/画像操作

対象: `animstart animstop wa stopmove stopquake stoptrans pimage` ＋ 既存 `trans`（method=scroll,
layer=）/`move`（path=）/`image`（clip= flipud= fliplr= grayscale=）の属性拡張。あわせて W4 で
発見された `copylay` の既知バグ（コピー先のメッセージレイヤの位置が恒久的にずれる）を先行修正。

### 先行バグ修正: [copylay] のメッセージレイヤ位置ズレ

- **原因**: `Scene::copy_layer`（W3 実装）はコピー先の x/y（ネイティブ描画位置）も無条件で
  コピー元からコピーしていた。一方、メッセージレイヤの位置は TS 側 `MessageLayer.cfg`
  （`player.ts`）が権威的に管理しており、`applyMessageLayerConfig()` が呼ばれたときにだけ
  cfg の値がネイティブ側へ再適用される。`[copylay destlayer=message0]` のようにコピー先が
  メッセージレイヤの場合、ネイティブ側の x/y だけが copylay 実行のたびにコピー元の位置へ
  ズレていき、`applyMessageLayerConfig()` が呼ばれるまで（次の `[position]`/`[layopt]`/
  ロードまで）そのズレが残る。さらに、F9 ロードは「直前のラベルから再実行」するため
  （既存の choreography 挙動、W3 で確認済み）、ロード後の再生で再び同じ `[copylay]` を
  通過するたびにズレが「解消されずに繰り返し発生し得る」という意味で恒久的になる
  （cfg 側は毎回正しい値に戻るが、次に copylay を通過すると再びネイティブ側だけがズレる）。
- **修正方針**: `Scene::copy_layer` に `copy_position: bool` を追加し、`false` の場合は
  x/y を非コピーにする（内容 pixmap/text/image_path/opacity/visible は従来通りコピーする）。
  既存の x/y コピー機能自体は一般レイヤ間コピー（画像レイヤを丸ごと複製する等、KAG3 の
  copylay 本来の用途）では引き続き必要なため維持し、呼び出し側（`tags.ts` の copylay
  ハンドラ）が「コピー先がメッセージレイヤかどうか」で `copyPosition` を切り替える設計にした
  （`destIdx === null` なら true、メッセージレイヤなら false）。**理由**: KAG3 の copylay
  仕様自体はレイヤの内容を丸ごとコピーするもので位置コピーは正しい挙動だが、このエンジンの
  メッセージレイヤは「cfg 駆動で位置を管理する」という KAG3 には無い独自の実装モデルを
  持っており、両者の前提が衝突していたのが根本原因。位置管理の主権を持つ TS 側で
  「メッセージレイヤ宛だけ例外的に位置を守る」のが最小の修正になる。Rust 側の
  `copy_layer_copies_content_but_not_z_or_tween`（既存テスト）はそのまま `copy_position=true`
  を渡して不変の挙動を検証、新規 `copy_layer_can_exclude_position_reproducing_message_layer_bug`
  が `copy_position=false` の再現+修正を検証する。**影響**: `docs/kag-tag-compat.md` の
  `copylay` 備考を更新（既知バグの節を「W5 で修正済み」に書き換え）。一般レイヤ間の
  copylay（`destlayer=` がメッセージレイヤ名にマッチしない場合）は W3 時点と完全に同じ
  挙動（位置もコピーする）のまま。

### move path=（折れ線区間列）

- **判断**: `Layer::tween`（単区間）はそのまま維持し、`Layer::tween_queue: Vec<Waypoint>`
  （区間の残り）を追加する設計にした。`Scene::settle_finished_tweens` は区間が完了しても
  `tween_queue` に次があれば、確定した到達値を新しい区間の `from` としてそのまま次の
  `Tween` を開始する（区間の `start` は「前区間の `start + duration`」にして時刻計算のドリフトを
  防ぐ）。`Scene::skip_all_tweens`（スキップモード/`[stopmove]`）は `tween_queue` の**最終**
  waypoint まで一気に確定する（途中区間を経由しない）。**理由**: 既存の単区間 API
  （`layer_move`/`op_layer_move`/`[move]`）や `settle_finished_tweens`/`skip_all_tweens`/
  `layer_moving`/`is_animating` の外部シグネチャ・戻り値の意味を一切変えずに拡張できる設計
  にしたかった（`tween_queue` が常に空である既存の呼び出し経路は無改変で同じ動作になる）。
  **影響**: 新規 op `op_layer_move_path`（`layer_move_path`、`Vec<MoveArgs>` を受け取り、
  最初の要素を `Tween` として即開始・残りを `tween_queue` へ積む）を追加。`is_animating`/
  `EngineState::layer_moving` は `tween_queue` が非空なら（現在区間がたまたま完了直後でも）
  true を返すよう防御的にガードを足した（`settle_finished_tweens` が先に呼ばれる通常の
  呼び出し順序では従来通り区別が付かないが、呼び出し順序に依存しない安全側の実装にした）。
- **判断**: `[move path=(x1,y1,o1)(x2,y2,o2)...]` の各点は x/y 必須、opacity は省略可
  （省略時は直前の点の値、先頭点は現在の実効値を引き継ぐ）とした。`time=` は**総時間**として
  区間数で均等割りする（各区間 `time / 区間数` ms）。`accel=` は全区間へ一律適用する。
  **理由**: KAG3 の `path=` 構文自体には「各区間ごとの個別時間/accel」を書く記法が無く
  （詳細設計書・krkr2doc とも `time=`/`accel=` は move タグ全体の属性として1つだけ）、
  区間ごとの時間配分をどう解釈するかは実装判断が必要だった。「合計時間を均等割り」が
  最も素直で、シナリオ作者が意図する「path 全体を time ms かけて動く」という直感に近いと
  判断した（CLAUDE.md の「複数の解釈があれば明示する」方針に基づき、ここに明記する）。
  **影響**: 区間ごとに移動距離が大きく異なる path では速度が不均一に見える（距離に応じた
  時間配分は行わない）。第一段は折れ線（線形区間列）のみ対応し、スプライン補間は対象外
  （`docs/kag-tag-compat.md` に明記）。
- **判断**: `[stopmove]` は新規 ops を追加せず、既存 `api.layer.moveSkip()`
  （`op_layer_move_skip` → `Scene::skip_all_tweens`）をそのまま使う。**理由**: 「全レイヤの
  move を目標値で即完了させる」という効果が `[stopmove]` の要求仕様と完全に一致しており、
  `skip_all_tweens` を W5 で「tween_queue の最終値まで一気に確定する」よう拡張済みのため、
  path= を使った move にも正しく対応できる。**影響**: `[stopmove]` は全レイヤの move を止める
  （KAG3 も本来「指定レイヤ」ではなく全体に作用する動作のため、既存の `skip_all_tweens` の
  「全レイヤ」という粒度をそのまま踏襲した）。

### trans method=scroll / trans layer=

- **判断**: `Pixmap::scroll_from(old, new, dir, t)` は「新ページが `dir` の辺から
  スライドインし、旧ページは常にその場に留まって新ページに覆われていく」という 1 通りの
  見た目だけを実装した。KAG3 の `stay=stayfore|staynone|staybase` は受理してパースするが
  （`TransArgs.stay`）、描画では区別しない。**理由**: このエンジンのページ合成は
  `compose_page` が常に画面全面を覆う不透明フレームを作ってから crossfade/universal/scroll
  へ渡す設計になっており、「旧ページも一緒にスライドさせる (staynone)」を実装しても、
  新旧とも隙間なく画面全体を覆う不透明フレーム同士である以上、両者の可視結果は
  数学的に同一になる（新旧の境界が常に連続しており、どこにも背景が透けて見える隙間が
  生まれないため）。実際に「新ページと連動して旧ページを押し出す」バリエーションを
  試作して確認した上で、可視差が無いことを確認して不採用にした（無意味な分岐コードを
  増やさない、CLAUDE.md の「求められていない複雑さを避ける」方針）。**影響**:
  `docs/kag-tag-compat.md` に「stay= は受理するが描画上は区別しない」と明記。
- **判断**: `from=` の既定値は `left`。`right`/`top`/`bottom` 以外の値は `left` にフォール
  バックする（既存の `trans method=` が `universal` 以外を `crossfade` 扱いにするのと同じ
  「不明な値は安全な既定へ」という既存方針を踏襲）。
- **判断**: `[trans layer=]`（真のレイヤ単位トランジション）は受理して全画面（従来どおり）に
  フォールバックし、`KagPlayer` インスタンスごとに初回だけ warn する（W1 の
  `warnStyleAttrs`/W3 の `warnedHch` と同じパターン）。**理由**: シーングローバル合成
  （`compose_frame` が fore/back 2 ページを丸ごと合成する設計）を崩さずにレイヤ単位の
  トランジションを実装するのはスコープ超過（トランジションの対象を「特定レイヤ + その
  背後にある他レイヤ」のように部分合成する必要があり、既存の合成パイプライン全体に
  影響する）と判断した。**影響**: `docs/kag-tag-compat.md` に制限を明記。

### image 属性拡張（clip=/flipud=/fliplr=/grayscale=）

- **判断**: `Pixmap::flip_h`/`flip_v`/`grayscale` を純関数として追加し、新規 op
  `layer_load_image_ex`（`layer_load_image` の拡張版、`clip` 抽出 → `flip_h` → `flip_v` →
  `grayscale` の順に適用してからレイヤへ設定）を追加した。**理由**: 既存 `layer_load_image`
  のシグネチャを変えると全呼び出し箇所に影響するため、拡張は別 op にして既存の `[image]`
  （属性拡張なし）の経路は完全に無改変にした。`tags.ts` 側は `clip=`/`fliplr=`/`flipud=`/
  `grayscale=` のいずれかが指定されたときだけ `loadImageEx` を呼び、それ以外は従来通り
  `loadImage` を呼ぶ分岐にした。**影響**: 対応表 `image` 備考に拡張属性を明記。
- **判断**: flip のテストは W4 の教訓（MF 動画で対称フィクスチャのせいで反転バグを
  見逃した）に基づき、2x2 の 4 隅に別々の色を置く非対称フィクスチャ（`corner_fixture`）を
  使った。`flip_h`/`flip_v` を取り違えても、あるいはどちらかが実質 no-op でも、この
  フィクスチャなら必ず検出できる（対称フィクスチャだと片方の軸のバグが盲点になる）。
- **判断**: グレースケール化の輝度係数は ITU-R BT.601 近似の固定小数 `(77, 150, 29)/256`
  を採用した（`rule_plane_from_rgba` が既に「輝度 = R」という単純化を採用しているのとは
  別に、グレースケール変換は目視の自然さを優先して標準的な輝度式に寄せた）。プリマルチ
  プライ済みのままでも R/G/B に同じ係数を掛けるだけなので、アルファとの整合性は崩れない。

### pimage

- **判断**: 新規 op を追加せず、W4 で実装済みの `layer_blit_image`（画像ファイルの部分矩形を
  既存レイヤへ (dx, dy) を左上として重ね描きする）をそのまま流用した。**理由**: 設計方針に
  明記された「pimage は既存 layer_blit_image ops をそのまま使う」を採用。**影響**:
  `[pimage layer= page= storage= clip= dx=/x= dy=/y=]` という属性マッピングはこのエンジン
  独自の解釈（KAG3 の pimage の正確な属性名とは異なる可能性がある。既存タグ全般と同じ
  「シナリオ作者を信頼する」設計）。`docs/kag-tag-compat.md` に明記。

### animstart/animstop/wa（.asd 最小サブセット）

- **判断**: 新規モジュール `runtime/kag/anim.ts`（`AnimManager` クラス + `parseAsd` 純関数）
  を新設し、`buttons.ts` と全く同じ「`api` にのみ依存する自己完結モジュール、import は
  `player.ts → anim.ts` の一方向のみ」パターンを踏襲した。**理由**: W1 で発覚した
  `player.ts ⇄ tags.ts` の循環 import 事故（tsint が import を評価順に解決するため
  `TypeError` になる）と同じ罠を避けるため。`tags.ts` は `parseAsd`（値 import、tags.ts →
  anim.ts の一方向）と `p.anim`（`KagPlayer` インスタンス経由）の両方を使うが、anim.ts は
  player.ts/tags.ts のどちらも import しないため循環にならない。**影響**: `loader.rs`
  の `RUNTIME_MODULES` に `noiria:kag/anim` を追加登録した（tsint 側は `NoiriaHost::resolve`/
  `load` が `noiria:xxx` を動的解決する実装のため無変更で自動的に解決できた、W4 の
  `buttons.ts` 追加時と同じ）。
- **判断**: .asd の対応記法を「`セル番号,graphic,wait` の行 + `loop=true|false`（省略時
  true）+ `;` コメント/空行」という独自の最小サブセットに絞った。**理由**: KAG3 本来の
  .asd（`storage`/`pat` キー、`seg=`/`module=` による条件分岐、複数パターン集合の切替等）は
  Kirikiri のプラグイン機構と密結合しており、フル対応は「.asd パーサ」というより「別の
  ミニ言語処理系」を作ることに等しい。ユーザー指示の「*セル番号 | graphic + wait 的な
  ループ定義」という要求を字面通り実装した最小形とした。**影響**: `docs/kag-tag-compat.md`
  に対応範囲を明記。サンプル .asd は `games/sample/assets/fg/icon_blink.asd`（アイコンの
  まばたき、2 フレーム、loop=false の 3 セル）。
- **判断**: `[wa layer=]` は `KagPlayer.waitWhile(cond, stepMs)` という新規の公開ヘルパ
  経由で実装した（`p.anim.isRunning(id)` が false になるまで 30ms 間隔で待つ）。**理由**:
  複数 await するループ待ちタグは各 wait 後に `jumpRequest` をチェックする既存ルール
  （W3 の `printTextWithAutoWait` 不具合修正で確立）に従う必要があるが、`jumpRequest` は
  `KagPlayer` の private フィールドで `tags.ts`（外部モジュール）から直接参照できない。
  `printTextWithAutoWait` と同じ「wait 後に jumpRequest を確認して早期 return」パターンを
  `KagPlayer` の public メソッドとして汎用化し、`tags.ts` からはコールバック経由で使わせる
  設計にした。**影響**: 今後 `tags.ts` 側で「ループ待ち」を実装するタグを追加する際は
  `p.waitWhile()` を再利用できる（`printTextWithAutoWait` 自身は `player.ts` 内部の別ループ
  なので今回はリファクタしていない）。
- **判断**: `loop=false` の .asd がセル列を最後まで再生し終えると `AnimManager` は内部の
  Map エントリを削除して `isRunning()` が false を返すようにした（`[wa]` が自然に完了する）。
  `[animstop]` は世代カウンタ（`setbgmstop`/`timeout` と同じ既存パターン）でループを
  中断させる。サンプルでは両方を実演する（layer=1 は loop=false を最後まで再生させて
  `[wa]` で待つ、layer=2 は同じ .asd を `[animstop]` で 250ms 時点で強制停止する）。

### 検証結果 (W5)

- ベースライン: `cargo test`（ワークスペース） 205 件全通過。
- 実装後: **222 件全通過**（noiria-core 168→180、新規 12 件: scene.rs
  `copy_layer_can_exclude_position_reproducing_message_layer_bug`
  `settle_finished_tweens_chains_to_next_waypoint_seamlessly`
  `skip_all_tweens_jumps_to_final_waypoint_not_first_leg`
  `is_animating_true_while_tween_queue_pending_even_if_current_leg_finished`
  `single_leg_move_behavior_is_unchanged_when_queue_is_empty`、pixmap.rs
  `flip_h_mirrors_left_right_not_up_down` `flip_v_mirrors_up_down_not_left_right`
  `grayscale_converts_to_luma_and_preserves_alpha`
  `scroll_from_left_midpoint_shows_new_entering_from_left_edge`
  `scroll_from_right_midpoint_shows_new_entering_from_right_edge`
  `scroll_from_edges_are_fully_old_then_fully_new`、compose.rs
  `scroll_transition_midpoint_shows_new_page_entering_from_left`。noiria-app 19→24、
  新規 5 件: engine_state.rs `quake_stop_clears_active_quake`
  `layer_move_path_starts_first_leg_and_queues_the_rest`、tsint_engine.rs
  `tsint_move_path_wiring_starts_first_leg_and_queues_rest`、mod.rs js_tests
  `move_path_tween_wiring_starts_first_leg_and_queues_rest_in_v8`
  `kag_anim_and_move_path_pure_functions_in_v8`）。既存 205 件は無変更で全通過
  （`copy_layer_copies_content_but_not_z_or_tween`/`copy_layer_missing_src_is_a_noop` は
  新パラメータに `true` を渡すよう更新したが、アサーション自体は無変更）。
- `cd ../tsint && cargo test`: 全 12 スイート通過（`tests/kag_full.rs` の op ログ厳密比較
  含む）。`runtime/kag/anim.ts` を新設しても tsint リポジトリ側は無変更で自動解決できた
  （W4 の `buttons.ts` 追加時と同じ、`NoiriaHost::resolve`/`load` が動的解決するため）。
- E2E: V8 (`--autotest 24 --autotest-out target/autotest-w5`) / tsint
  (`--engine tsint --autotest 24 --autotest-out target/autotest-w5-tsint`) いずれも EXIT=0。
  `frame_00`〜`frame_08_hover` は両エンジンでバイト単位ハッシュ一致（例:
  `frame_07_hover.png` sha256 `05c84f4b...` 一致）。`frame_09`以降は F9 ロード後の
  `*start` 再実行が crossfade トランジション進行中の瞬間を捉えるタイミング差でバイト単位では
  不一致になるが、これは W3 で確認済みの既知の一過性事象（両エンジンとも決定論的な trans
  進行の途中経過であり内容は同一）であり、目視でも差分なしを確認した。24 秒区間では
  `[copylay srclayer=message1 destlayer=message0]`（既知バグの再現箇所）を通過し、
  `frame_14_hover.png`/`frame_final.png` で message0（画面下部の大きいウィンドウ）が
  message1（画面上部、top=90）の位置へずれていないことを目視確認した（修正の実機確認）。
- E2E（拡張検証、`--autotest 150` / `--engine tsint --autotest 150`）: 両エンジンとも
  EXIT=0。ログに `[stopmove]`→`[stopquake]` の順序一致を確認（W5 の新規タグが実行された
  証跡）。`target/autotest-w5-ext/frame_44_hover.png` で `[move path=]`（折れ線移動 + 途中
  `[stopmove]`）によりレイヤ0(アリス)が最終区間の目標位置 (650,60) まで一気に確定している
  ことを確認（`skip_all_tweens` の最終 waypoint ジャンプの実機確認）。
  `target/autotest-w5-ext/frame_49_hover.png` で `[image fliplr=]`/`[image grayscale=]`
  のアイコン、および `[pimage]` で顔画像にバッジを重ね描きした結果が両方写っていることを
  確認した。
- `cargo clippy -p noiria-core -p noiria-app -p noiria-tools --all-targets`: 変更ファイル
  （scene.rs / pixmap.rs / transition.rs / compose.rs / engine_state.rs / ops_layer.rs /
  tsint_engine.rs / mod.rs / loader.rs / gen_w5_anim.rs / anim.ts / tags.ts / player.ts 等）
  で新規 warning は 0（`TransArgs.stay` は保持のみで未使用のため `#[allow(dead_code)]` を
  明示）。既存 warning は W1〜W4 時点と同じ無関係ファイルのもの。

## 2026-07-11 W6: ストア系・スクリプト系・履歴系・動画拡張（最終ウェーブ）

対象: `tempsave tempload store disablestore record goback gotostart startanchor locksnapshot
unlocksnapshot save load copybookmark erasebookmark clearvar clearsysvar iscript endscript ignore
endignore hr hact endhact showhistory trace erasemacro openvideo preparevideo playvideo pausevideo
resumevideo rewindvideo`（31 タグの列挙だが `endhact` が抜けており実際は 32 タグ、対応表の
ウェーブ割当と一致）＋ 対象外スタブ 15 種（`loadplugin mapimage mapaction mapdisable mappfont
videoevent wp cancelvideoevent videosegloop cancelvideosegloop videolayer clearvideolayer
setbgmlabel clearbgmlabel cursor`）。これで KAG3 タグ対応表 161 行がすべて ✅/➖ になった。

### iscript の tsint 対応可否（着手前の確認）

- **判断**: `[iscript]`〜`[endscript]` は「対象外候補」から「✅（制限付き）」へ最終判断した。
  **理由**: 着手前に `../../../tsint/src/builtins.rs`（674〜702 行）をソース調査した結果、
  tsint は `new Function(...)` を実行時に自前パーサでコンパイルする形で既に実装済みだった
  （`compile_dynamic_function`）。これは `runtime/kag/vars.ts` の `evalStmt`/`evalExpr`
  （`new Function("f","sf","tf","sys", body)`）が内部で使っている機構そのものであり、
  既存の `[eval]`/`[emb]`/`[link exp=]` 等が W1〜W5 の全 E2E で tsint 上でも問題なく
  動作し続けてきたことが実機的な裏付けにもなっている。したがって `[iscript]` の本文を
  同じ `evalStmt` 経路へ渡すだけで、V8/tsint 両対応が最小実装で成立すると判断した。
  **影響**: `docs/kag-tag-compat.md` の `iscript` 行は「🔨→✅ (制限付き)」とし、
  「TJS 互換ではなく TS として評価する」ことを明記した。V8 専用フォールバック
  （tsint では warn+スキップ）は不要だった。

### parser.ts: [iscript]〜[endscript] の生テキスト収集

- **判断**: `Command` 型に `{ type: "iscript"; code: string; line: number }` を追加し、
  `parseScenario` のメインループに専用の収集モード（`collectIscriptBody`）を設けた。
  `@iscript`（行タグ形式）と `[iscript]`（単独行のブラケット形式）の両方を認識し、
  `[endscript]`/`@endscript` の行が見つかるまで **行を一切タグ解釈せずにそのまま連結**する。
  見つからない場合（閉じタグの無い壊れたスクリプト）はファイル終端まで本文として扱う
  （パーサは純関数のままにしたいため、ここでは warn ログを出さない設計にした）。
  **理由**: iscript の本文は TS コードであり `[`/`]`（配列アクセス・配列リテラル）を
  自然に含みうるため、既存の `parseInline`（`[`/`]` をタグとして解釈する）を通すと
  誤ってタグとして分解されてしまう。ブロック全体を先に「生テキスト」として隔離する
  必要があった。**影響**: `player.ts` の `execCommand` に `case "iscript":` を追加し、
  `vars.evalStmt(cmd.code, this.sys)` を呼ぶだけ（`endscript` はパーサに消費されるため
  実行時には来ない。壊れたスクリプト向けの安全網として `tags.ts` に no-op ハンドラも
  登録した、`macro`/`endmacro` と同じ前例）。単体テスト
  `kag_parser_iscript_collects_raw_body_in_v8`（`crates/noiria-app/src/script/mod.rs`）で
  `arr[0] + arr[1]` のような配列アクセスがタグとして誤解釈されないことを検証済み。

### ストア系: tempsave/tempload の完全スナップショット化

- **判断**: 既存の `makeSnapshot()`（`onLabel()` の自動キャプチャ・`[save]`/F5 が使う、
  ラベル境界のみを記録）とは別に `makeFullSnapshot()` を新設した。`SaveData.pos` に
  任意の `index?: number`（`this.pc.index`）を追加し、`SaveData` に `curSegs?: api.Segment[]`/
  `curText?: string`（カレントレイヤの表示内容）を追加した。`applySnapshot()`
  （旧 `load()` の本体を抽出した共通実装）は `pos.index` が定義されていれば
  `this.pc = { file, index }` を直接セットする厳密な再開を行い（`popReturn()` と同じ
  「pc を直接書き換えて run() ループの次周回に委ねる」パターン）、未定義なら従来通り
  `requestJump(file, label)` でラベルから再生する。`curSegs` があればカレントレイヤへ
  `api.text.print(..., false, 0)` で直接再描画する。**理由**: `[tempsave]`/`[record]` は
  ラベル境界とは限らない任意の位置で呼ばれるため、ラベルからの再生に頼ると
  「tempsave した瞬間の文章」を再現できない（同じラベル内の別の文が表示されてしまう）。
  一方で既存の自動セーブ（onLabel 起点）・F5/F9/メニュー・`[save]`/`[load]` タグは
  `pos.index`/`curSegs` を一切書き込まないため、**完全に後方互換**（従来通りラベルから
  再生する）。**当時の影響**: `SaveData.bgm` は path/volume/paused のみで再生位置を
  持たなかったため、BGM は曲頭から再開していた。この既存制限は 2026-07-26 の B14 で解消した。

### ストア系: tempsave/tempload はメモリのみ、goback/record/gotostart は履歴リング

- **判断**: `tempsave`/`tempload` は `KagPlayer.tempSlots: Map<number, SaveData>`
  （プロセスメモリ上のみ、ディスク I/O 皆無）に保存する。KAG3 で負の `place` 番号が
  「メモリ専用の一時スロット」を指す慣習があることに対応する形で、Noiria では
  `tempsave`/`tempload` 自体を常にメモリのみの経路にした（`place` の符号による分岐は
  設けていない。すべての `place` 値がメモリスロットのキーとして扱われる）。
  **理由**: `tempsave`/`tempload` は「クイックバック」的な高頻度・低コストな一時保存が
  KAG3 での主要用途であり、ディスク I/O を毎回発生させる設計は不要な複雑さと判断した
  （CLAUDE.md の「単純に済むところを複雑にしない」方針）。**影響**:
  `docs/kag-tag-compat.md` に明記。プロセス終了（ウィンドウを閉じる/クラッシュ）で
  `tempSlots` の内容は失われる（KAG3 の実挙動とも一致する想定の範囲内）。
- **判断**: `[record]`/`[goback]`/`[gotostart]`/`[startanchor]` は `KagPlayer.trail:
  SaveData[]`（履歴リング、上限 `TRAIL_LIMIT = 30`）で実装した。`record` は
  `makeFullSnapshot()` を `trail.push()`（上限超過で先頭を `shift()`）、`goback` は
  `trail.pop()`（消費）して `applySnapshot()`、`gotostart` は `trail[0]` を
  **消費せずに** peek して `applySnapshot()` する。`startanchor enabled=`（既定 true）は
  `KagPlayer.autoAnchor` フラグを立て、`[s]`（`waitForJump()` の停止直前、
  `maybeAutoAnchor()`）ごとに自動で `record` 相当を行う。**理由**: 上限 30 は
  `MAX_MSG_LAYERS=8` と同じ「実装上の恣意的だが十分な上限」という位置づけ（無制限に
  メモリ上へスナップショットを積み上げないため）。`goback` を「消費」、`gotostart` を
  「消費しない」としたのは、KAG3 の「クイックバック（1 段階戻る）」と「最初の記録位置へ
  一気に戻る」という役割の違いに対応させたため。**影響**: `docs/kag-tag-compat.md` の
  `gotostart` 備考に「KAG3 標準の『シナリオ先頭へ』ではなく『最初の記録位置へ』という
  解釈を採用した」と明記した（ユーザー指定のウェーブ設計「gotostart で最初のアンカーへ」
  に従った解釈で、対応表の旧稿にあった「シナリオ先頭へジャンプ」という説明とは異なる。
  CLAUDE.md の「複数の解釈があれば明示する」方針に基づきここに明記する）。
  `trail` は「KAG の `sf.trail` 相当」と説明しているが、**実装はメモリ上のみで `sf` へは
  永続化していない**（セーブデータに含めるとサイズが際限なく増えるおそれがあり、
  今回のスコープでは費用対効果が見合わないと判断した）。**影響**: アプリ再起動や
  通常のセーブ/ロード（`[save]`/`[load]`/F5/F9/メニュー）をまたぐと `trail` は
  空にリセットされる（同一プロセス内・同一プレイ継続中のみ有効な「クイックバック」）。

### ストア系: サンプルでの record/goback/gotostart/tempsave/tempload の多重実行防止

- **判断**: `games/sample/scenario/first.ks` のデモで、`goback`/`gotostart`/`tempload` は
  いずれも **pc を「復元元の記録/一時保存を呼んだ直後」まで巻き戻す**ため、同じ行を
  ガード無しで並べると「巻き戻し→同じ変更→再度巻き戻し→…」の無限ループになりうる
  ことが実装中の検証で判明した。特に `gotostart`（`trail[0]` を消費しない）は、
  ガードが無いと理論上ずっとループし続ける。**修正**: `f`（`vars.f`、`tempload`/
  `goback`/`gotostart` の復元対象）ではなく **`sf`（`vars.sf`、いずれの復元でも
  一切触れられない）へ立てたフラグ**でガードする設計にした（例:
  `[if exp="!sf.w6GobackDone"][eval exp="sf.w6GobackDone = true"]...[goback][endif]`）。
  `f` をガードに使うと、復元自体が `f` を巻き戻し前の値に戻してしまうため
  ガードの効果が消えてしまう（実装中に実際にこの誤りで無限ループを起こし、
  `--autotest 40` のログで `[goback]`/`[record]` が延々ループしているのを検出して
  気付いた）。`tempsave`/`record` 自体（`if` の外側、ガード対象外）は毎回の巻き戻しの
  たびに再実行されるが、同じスロット/末尾への上書きなので副作用は無害（`tempsave` は
  同じ `place` への再保存、`record` は `goback`/`gotostart` の消費挙動と組み合わせても
  境界外に増殖しない）。**影響**: `[save]`/`[load]` タグ（ラベル境界からの再生）を
  同じデモ内で連続実行すると、`[load]` がラベル先頭までジャンプして
  デモブロック全体を再実行するため、`store`/`disablestore` 等の一部タグがログ上
  2 回実行される（実害はない冪等な操作のみ、`docs/kag-tag-compat.md` 検証結果参照）。
  save/load ペア自体は `sf` ガードで 2 回目の `[load]`（無限ループの火種）を防止済み。

### store=false 属性 / disablestore / locksnapshot・unlocksnapshot の優先順位

- **判断**: 「次のスナップショット作成を抑制する」3 つの独立したフラグ群として実装し、
  `onLabel()`（自動スナップショット `this.snapshot` の更新箇所）での優先順位を
  **`[locksnapshot]`（永続、最優先）→ `[jump]`/`[call]` の `store=false`（1 回限りの
  抑制、`KagPlayer.suppressNextAutoSnapshot`）→ 通常更新**の順にした。`[disablestore]`
  はこの自動更新の抑制には一切関与せず、`save()`/`load()`/`tempsave`/`tempload`/
  `store`/`copybookmark`/`erasebookmark`/`goback`/`gotostart`（＝実際にストレージへ
  読み書きする操作、または履歴を復元する操作）の**実行そのもの**を別レイヤで
  ガードする（`KagPlayer.guardStore()`）。**理由**: `store=false`（KAG3 で `[jump]`/
  `[call]` に実在する属性、「着地先ラベルをセーブポイントにしない」という意味）は
  「特定の 1 回のラベル通過だけ記録させない」という*記録内容*の制御であり、
  `[disablestore]`（「プレイヤーにセーブ/ロードさせない」という UX 制御）とは
  目的が異なる。両者を 1 つのフラグに混ぜると「`disablestore` 中は self.snapshot も
  更新されない」といった副作用が生まれ、`disablestore` 解除直後の F5 クイックセーブが
  古い（`disablestore` 前の）位置に固定されてしまう不自然な挙動になる。分離することで
  「`disablestore` 中でも `this.snapshot` は最新のまま追跡され続けるが、それをディスクへ
  書き出す・読み込むことだけが禁止される」という直感的な挙動になる。**影響**:
  `docs/kag-tag-compat.md` の `disablestore`/`locksnapshot` 備考に明記。

### clearsysvar と settings の再適用

- **判断**: `[clearsysvar]` は `vars.replaceSf({})`（KAG 準拠で sf を全消去）の直後に
  `KagPlayer.resetSysDefaults()` を呼び、`vars.sf.readLabels = {}`（未読判定の再初期化。
  `onLabel()` が `readLabels[key]` を無条件に読むため、`undefined` のままだと次のラベル
  通過で `TypeError` になる）、`this.settings = { ...DEFAULT_SETTINGS }` +
  `vars.sf.config = this.settings`（音量/文字速度/オート速度を既定値へ戻し、
  `sf.config` にも書き戻す）、`this.applySettings()`（`cps`/`autoWaitMs` へ反映）、
  `this.trail = []`（履歴リングも「sf.trail 相当」の対象としてクリア）を行う。
  **理由**: sf を消去したまま既定値を再適用しないと、`this.settings`（メモリ上の
  キャッシュ）と空になった `sf.config` が食い違ったまま残り、設定メニューを開くまで
  実態の無い値が表示され続ける「壊れた状態」になる。ユーザー指定のウェーブ設計に
  「直後に settings 既定値を再適用するか実装判断」と明記されていたため、CLAUDE.md の
  「複数の解釈があれば明示する」方針に基づきこの判断とその理由をここに記録する。
  **影響**: `[clearsysvar]` の直後に音量スライダー等の表示がリセットされる（プレイヤー
  視点では「初期化された」という体験になり、`clearsysvar` の語義とも整合する）。

### erasebookmark の削除代替実装

- **判断**: `sys.saveWrite`/`sys.saveRead`/`sys.saveList`（`runtime/api.ts`）には
  スロットの削除 API が無いため、`[erasebookmark place=]` は該当スロットへ空 JSON
  文字列 `"{}"` を書き込むことで代替した。**理由**: 新規 op（`op_save_delete` 等）を
  追加すれば「真の削除」も可能だが、既存の `sys.saveWrite`/`saveRead`/`saveList` の
  3 op だけで実装できるユーザー指定の設計方針（「sys.saveWrite/saveRead/saveList で
  スロット間コピー/削除」）に従い、新規 op を増やさない選択をした（CLAUDE.md の
  「新規機能を必要以上に増やさない」方針）。**影響**: `"{}"` は有効な JSON なので
  `JSON.parse` は成功し、`openSaveMenu` の `renderSaveMenu()` は「日付・タイトルが
  空欄のスロット」として表示し続ける（`(空きスロット)` 表記には戻らない）。真に
  「空きスロット」として見せたい場合は追加の delete op が必要になる、既知の制限として
  `docs/kag-tag-compat.md` に明記した。

### ignore/endignore の解釈と入れ子走査

- **判断**: `[ignore exp=]`〜`[endignore]` を「KAG3 本来のクリック等入力の一時無視
  ブロック」ではなく、「exp が真なら endignore まで読み飛ばす条件付きスキップ
  ブロック」として実装した。走査は `if`/`elsif`/`else`/`endif` の既存パターン
  （`skipToNextBranch`/`skipToEndif`）と同型で、`if`/`ignore` を同じ depth
  カウンタに含めることで入れ子（`[ignore]` の中に `[if]`、あるいは `[ignore]` の
  中に `[ignore]`）を正しく跨ぐ。走査本体は純関数 `scanIgnoreBlock(commands,
  startIndex): number`（`player.ts` からエクスポート、`KagPlayer.skipToEndignore()`
  はこれを呼ぶだけの薄いラッパ）として実装し、単体テスト
  `kag_player_scan_ignore_block_pure_function_in_v8` で「単純な ignore」「入れ子の
  if を跨ぐ」「入れ子の ignore」「閉じタグが無い場合」の 4 パターンを検証した。
  **理由**: ユーザー指定のウェーブ設計に「exp= が真で endignore まで読み飛ばし
  （skipToNextBranch と同型の走査、if/ignore の入れ子カウント）」と明記されており、
  この字面通りの解釈を採用した。「クリック等の入力を一時無視する」という対応表の
  旧稿の説明（実装着手前の下調べ時点の記述）とは異なる解釈になるが、CLAUDE.md の
  「複数の解釈があれば明示する」方針に基づきここに明記する。**影響**:
  `docs/kag-tag-compat.md` の `ignore` 備考をこの解釈に合わせて更新した。

### 動画拡張 (openvideo/preparevideo/playvideo/pausevideo/resumevideo/rewindvideo)

- **判断**: 既存 `Videos { current: Option<VideoHandle> }`（同時再生 1 本のみを想定する
  モデル）はそのまま維持し、`prepared: Option<PreparedVideo>`（開いたが再生スレッドを
  まだ起動していない動画、こちらも単一スロット）を追加する形にした。`video_open`
  （開くだけ）/`video_prepare`（開く + 先頭フレームを `next_frame()` でデコードして
  レイヤへ表示 → `rewind()` で巻き戻す）はいずれも `prepared` へ格納する。
  `video_play_prepared(id)` は `prepared` を `take()` して再生スレッドを起動する
  （`id` が一致しなければエラー）。既存の `video_play(id, path, looping)`
  （`[video]` タグが使う、開く→即座に再生スレッド起動）は**シグネチャ・挙動とも無変更**
  で、内部の再生スレッド起動処理だけを `spawn_video_playback()` という private
  ヘルパへ抽出して `video_play`/`video_play_prepared` の両方から呼ぶよう共通化した。
  **理由**: 「既存 video 再生系を確認して最小差分で」というユーザー指示に従い、
  `[video]` タグの呼び出し経路（op シグネチャ含む）を一切変えずに新機能を追加する
  設計にした。**影響**: `openvideo`/`preparevideo`/`playvideo` はいずれも
  「同時に 1 本しか動画を扱えない」という既存の制約をそのまま引き継ぐ
  （`prepared` へ格納した動画も、次の `openvideo`/`preparevideo` が呼ばれると
  無条件に上書きされる。W6 のデモでは 1 本ずつ順番に扱っているため実害なし）。
- **判断**: `pausevideo`/`resumevideo`/`rewindvideo` は `VideoHandle` に
  `paused: Arc<AtomicBool>`/`rewind: Arc<AtomicBool>` を追加し、再生スレッド
  （`playback_loop`）のループ先頭で `rewind.swap(false, Relaxed)`（true なら
  `source.rewind()` して `index`/`next_due` をリセット、1 回限り消費）→
  `paused.load(Relaxed)`（true ならフレームを進めず 30ms 間隔でポーリングし続けて
  `continue`）の順にチェックするだけの最小差分にした。**理由**: 新しいスレッド間
  通信機構（コマンドチャネル等）を導入せず、既存の `stop: Arc<AtomicBool>` と
  同じ「アトミックフラグをポーリングする」パターンを 2 つ追加するだけで実装できた
  （CLAUDE.md の「単純に済むところを複雑にしない」方針）。**影響**: `rewindvideo` は
  再生中でも一時停止中でも動作する（次のループ周回で処理されるため、一時停止中は
  ポーリング間隔ぶん＝最大 30ms 遅延する）。単体テスト（`engine_state.rs`）で
  open/prepare/play_prepared/pause/resume/rewind/stop の状態遷移と、layer 不一致時の
  エラーを検証済み（`video_play_prepared_then_pause_resume_rewind_stop_transitions`
  等）。

### tsint の for...of + クロージャ捕捉の不具合（実機発見）

- **判断**: 15 個の対象外スタブタグを一括登録するループを、当初 `for (const [name,
  reason] of Object.entries(STUB_REASONS)) { handlers[name] = () => {...}; }`
  （`const` 束縛をクロージャで捕まえる）で実装したところ、**tsint 実機検証で
  15 個のハンドラすべてが最後のイテレーション（`cursor`）の値を共有してしまい、
  どのスタブタグを呼んでも `[cursor]` の warn しか出ない**という不具合を発見した
  （`--autotest` の拡張検証ログで `対象外` warn が `cursor` の 1 件しか出ていない
  ことに気付いて発覚。V8 では 15 件とも正しく出ていたため、エンジン間差異として
  実機で初めて顕在化した実例）。tsint リポジトリを調査（読み取り専用、変更はしない）
  したところ、`for (const x of iterable)` はループ変数を毎イテレーション新しい束縛
  として張り直す実装になっておらず（`compiler.rs` の `compile_for_in_of` が
  `BindLocal` でスロットを使い回す）、クロージャが同じセルを共有してしまう既知の
  制限であることを確認した（対照的に、tsint の古典的 `for (let i=...;...;...)` は
  `RebindLocal` で毎周回スロットを張り替える別実装になっており、こちらは正しく
  イテレーションごとに独立した束縛になる）。**修正**: `for...of` をやめ、
  `Object.entries(STUB_REASONS).forEach(([name, reason]) => { handlers[name] =
  () => {...}; })` に書き換えた（コールバック引数は呼び出しごとに新しい束縛になる
  ため、tsint でも V8 でも正しく動作する）。修正後、tsint の再検証で 15 種すべて
  個別に warn が出ることを確認済み。**影響**: `docs/kag-tag-compat.md`「➖ 対象外」
  節に注記した。今後 `runtime/kag/` 配下で「ループの中でクロージャを作り、
  ループ変数をクロージャに閉じ込める」パターンを書く際は、`for...of`/`for...in`
  ではなく `forEach`/`map` 等のコールバックベースの反復を使うこと（tsint の
  `for...of`/`for...in` はクロージャ捕捉に対して安全ではない。tsint リポジトリの
  修正はスコープ外のためユーザー確認後に別途検討）。

### 検証結果 (W6)

- ベースライン: `cargo test`（ワークスペース） 222 件全通過。
- 実装後: **229 件全通過**（noiria-app 24→31、新規 7 件: `engine_state::tests::
  video_open_does_not_touch_layer_pixmap_or_start_playback`
  `video_prepare_shows_first_frame_and_does_not_start_playback`
  `video_play_prepared_then_pause_resume_rewind_stop_transitions`
  `video_play_prepared_errors_when_nothing_prepared`
  `video_play_prepared_errors_when_layer_mismatch`（動画 ops の状態遷移、
  実ファイル+実 EngineState、`image` クレートを dev-dependency として追加し
  JPEG フィクスチャを自前生成）、`js_tests::kag_parser_iscript_collects_raw_body_in_v8`
  （iscript の生テキスト収集パース）、`js_tests::kag_player_scan_ignore_block_pure_function_in_v8`
  （ignore/if 入れ子走査の純関数テスト）。既存 222 件は無変更で全通過（新フィールドは
  全て `None`/オプショナル既定のため回帰なし）。
- `cd ../tsint && cargo test`: 全 12 スイート通過（`tests/kag_full.rs` の op ログ厳密
  比較含む）。`runtime/kag/parser.ts`/`player.ts`/`tags.ts` の新規構文（`for...of` から
  `forEach` への書き換え含む）が tsint の埋め込みモジュールパーサでも問題なく解決
  できることを確認した（tsint リポジトリ自体は無変更）。
- E2E（必須コマンド）: V8 (`--autotest 24 --autotest-out target/autotest-w6`) / tsint
  (`--engine tsint --autotest 24 --autotest-out target/autotest-w6-tsint`) いずれも
  EXIT=0、ログにエラー・`未対応タグ` 警告なし（24 秒区間では W6 デモ本体にはまだ
  到達しない後方互換確認のみ）。
- E2E（拡張検証）: V8 (`--autotest 420 --autotest-out target/autotest-w6-ext`)・tsint
  (`--engine tsint --autotest 300 --autotest-out target/autotest-w6-tsint-ext`)
  いずれも EXIT=0、`スクリプトから終了要求を受信`（`*bye` の `[close]` まで到達）で
  正常終了した。両ログとも `未対応タグ` 警告は 0 件、対象外スタブ 15 種の
  `対象外:` warn は各 1 回ずつ（tsint は上記 for...of 修正後に 15 種すべて個別に
  確認）。W6 の 32 実装タグ（`tempsave`/`tempload`/`record`/`goback`/`gotostart`/
  `store`/`disablestore`/`locksnapshot`/`unlocksnapshot`/`save`/`load`/
  `copybookmark`/`erasebookmark`/`clearvar`/`clearsysvar`/`iscript`/`ignore`/
  `erasemacro`/`trace`/`hr`/`hact`/`showhistory`/`openvideo`/`preparevideo`/
  `playvideo`/`pausevideo`/`resumevideo`/`rewindvideo` 等）がログに実行記録として
  出現することを確認した。
- goback/tempload の復帰前後: V8 拡張ログのタイムスタンプから対応するフレームを
  特定し、目視確認した。`target/autotest-w6-ext/frame_52_hover.png`
  （tempload 前: アリスが (650,200) へ移動・表情も変化した状態）→
  `target/autotest-w6-ext/frame_53_hover.png`（tempload 後: (390,60) の元の笑顔へ
  復元、かつメッセージ本文が tempsave 時点の `curSegs` から正しく再開されていることも
  文面で確認できる）。`target/autotest-w6-ext/frame_56_hover.png`（goback 前:
  (850,250) へ移動した状態）→ `target/autotest-w6-ext/frame_57_hover.png`（goback 後:
  (390,60) の元の笑顔へ復元）。
- `cargo clippy -p noiria-core -p noiria-app -p noiria-tools --all-targets`: 変更ファイル
  （engine_state.rs / ops_video.rs / tsint_engine.rs / mod.rs / parser.ts / player.ts /
  tags.ts / api.ts / noiria.d.ts 等）で新規 warning は 0。既存 warning は W1〜W5 時点と
  同じ無関係ファイル（noiria-tools 各所、noiria-app の mod.rs/loader.rs の test module
  配置）のもの。

## 2026-07-11 W7: 全数監査（最終）

対象: (1) 対応表×コードの機械照合テストの恒久化（本ウェーブの主目的）、(2) 全タグ回帰の
長尺 E2E、(3) `image` dev-dependency の除去（完全自作方針の残課題）、(4) W1 申し送りの
`[font edgecolor=]` 配線、(5) README/notes の最終更新。新規タグの実装は無い（監査ウェーブ）。

### 対応表×コードの機械照合テスト（tests/kag_tag_compat.rs）

- **判断**: `crates/noiria-app/tests/kag_tag_compat.rs`（統合テスト、4 件）を新設し、
  `docs/kag-tag-compat.md` の対応表と `runtime/kag/tags.ts` の実装を毎回の `cargo test` で
  機械照合する。検証項目: (a) 対応表が正確に 161 行で全行 ✅/➖（146+15）であること、
  (b) ✅ タグ全てが tags.ts の `makeTags` ハンドラに登録されていること（`iscript` のみ既知の
  例外、後述）、(c) 逆方向（makeTags に登録されているのに対応表に無い KAG3 タグが無いこと。
  Noiria 独自拡張 5 種は対応表末尾のセクションから**動的に**読み取って許容）、(d) STUB_REASONS
  のキー集合が ➖ 15 行と完全一致すること。**理由**: 表とコードの乖離を CI（cargo test）で
  検出するのがゴール。実際に tags.ts のハンドラ名を書き換えて 2 テストが期待通り fail する
  ことを確認済み（照合が機能している実証）。**影響**: 今後タグを追加/削除する際は
  対応表と tags.ts の両方を揃えて更新しないと cargo test が落ちる（意図した強制）。
- **抽出方法と限界**（regex クレート等は使わず素朴な文字列走査。詳細はテスト冒頭コメント）:
  - ハンドラ抽出は「ちょうど 4 スペースインデント + 識別子 + `:` + (`async`)? + `(`」という
    単一パターンのみ認識する。`makeTags` の `handlers` テーブルはこの形式で統一されているため
    現状は全 150 ハンドラ（KAG3 145 + 独自拡張 5）を抽出できる。将来別の記法（複数行定義等）が
    混じると抽出漏れになりうるが、その場合は「対応表にあるのに見つからない」方向で fail する
    ため乖離自体は検出できる。
  - `iscript` は唯一の既知例外: parser.ts が `[iscript]`〜`[endscript]` を専用 `Command` へ
    折り畳み、player.ts の `execCommand` の `case "iscript":` が直接処理するため、makeTags
    テーブルに（安全網の `endscript` はあるが）`iscript` ハンドラ自体は無い。テストでは
    `player.ts` に `case "iscript":` が存在することを別途確認する専用チェックにした。
  - STUB_REASONS は `const STUB_REASONS: Record<string, string> = {`〜`};` ブロック内の
    4 スペースインデントキーを抽出する。
- **判断**: 実装場所は `crates/noiria-app/tests/`（統合テスト）にした。`src/` 内テストでも
  よかったが、このテストは Rust コードの内部 API に一切依存せず「リポジトリのテキスト 2 つを
  突き合わせる」だけなので、ビルド単位が分かれる統合テストの方がビルド時間・依存の面で自然。
  パスは `CARGO_MANIFEST_DIR` からの相対（`../../docs/...`）で解決する。

### image dev-dependency の除去（完全自作方針の残課題）

- **判断**: W6 で動画 ops テストの MJPEG/AVI フィクスチャ生成のために追加した noiria-app の
  `image` dev-dependency を除去した。手順: (1) W6 と同一の生成ロジック（16x16 JPEG フレーム +
  `write_mjpeg_avi`）を一時的に `#[ignore]` テストとして実行し、`crates/noiria-app/tests/fixtures/`
  へ固定ファイルを 1 度だけ生成（`clip_2frames.avi` 1392 bytes: shade=40,220 /
  `clip_5frames.avi` 3330 bytes: shade=10,20,30,40,50）、(2) テスト側を `include_bytes!` で
  フィクスチャを読み込んで一時ゲームディレクトリへ書き出す方式（`write_test_avi_2frames`/
  `write_test_avi_5frames`）へ変更、(3) 生成用の一時テストと `[dev-dependencies] image` を削除。
  **理由**: テスト実行のたびに外部クレートで JPEG エンコードする必然性は無く、フィクスチャを
  リポジトリに固定同梱すれば「テスト時も外部依存ゼロ」になる（完全自作方針）。デコード側は
  従来通り自作 JPEG/AVI デコーダが検証される（フィクスチャのバイト列が固定になったぶん、
  むしろ再現性は上がる）。**影響**: 動画 ops テスト 5 件は無変更のアサーションで全通過。
  フィクスチャを再生成したい場合は W6 の生成ロジック（このファイルの上記記録と
  `noiria_core::codec::video::write_mjpeg_avi`）から再現できる。
  **workspace の image 依存の最終状態**: `noiria-core`（codec/png.rs・jpeg.rs の自作デコーダとの
  クロスチェック dev-dependency）と `noiria-tools`（プレースホルダアセット生成の通常依存）では
  引き続き使用中のため、workspace `Cargo.toml` の `[workspace.dependencies] image` は残した。
  `cargo tree -p noiria-app -e dev` で noiria-app の依存ツリーから image が消えたことを確認済み。

### [font edgecolor=] の配線（W1 申し送りの解消）

- **判断**: W1 で Rust 層（`TextSegment.edge_color`、`edge_color_overrides_style_outline`
  テスト済み）・tsint `segments_from_value`・TS 型定義（`Segment.edgeColor`）まで実装済み
  だった縁取り色上書きを、`.ks` から使えるように tags.ts の `applyFontAttrs` に
  `edgecolor=` 属性 1 行を追加して配線した（`[font edgecolor=0xRRGGBB]`。`style`/`deffont`/
  `defstyle` も同じ関数を通るため全て対応）。あわせて `player.ts` の
  `currentFontState`/`fontDefaults`/`styleDefaults` 型に `edgeColor?` を追加し、`printText` が
  セグメントへ反映する行を 1 行追加した。**影響**: 対応表の `font`/`style` 備考を更新。
  リセット系（resetfont/resetstyle）はスプレッドコピーのため自動的に edgeColor も戻る。

### 検証結果 (W7)

- ベースライン: `cargo test`（ワークスペース） 229 件全通過。
- 実装後: **233 件全通過**（noiria-app の統合テスト `tests/kag_tag_compat.rs` 新規 4 件:
  `compat_table_has_exactly_161_rows_all_resolved`
  `every_compat_table_tag_is_registered_in_tags_ts`
  `tags_ts_has_no_undocumented_kag3_tag_handlers`
  `stub_reasons_exactly_match_taigai_rows`）。既存 229 件は無変更で全通過
  （動画 ops テスト 5 件はフィクスチャ読み込み方式へ変更したがアサーションは無変更）。
  image dev-dep 除去後に `cargo clean -p noiria-app` してからのクリーンビルドで確認。
- `cd ../tsint && cargo test`: 全 12 スイート通過（`tests/kag_full.rs` の op ログ厳密比較
  含む。tags.ts/player.ts の edgecolor 追記が tsint の埋め込みパーサでも問題ないことを確認）。
- 照合テストの fail 検証: tags.ts の `quake` ハンドラ名を一時的に `quakeXX` へ書き換えた
  ところ、`every_compat_table_tag_is_registered_in_tags_ts`（✅ タグ `quake` が見つからない）と
  `tags_ts_has_no_undocumented_kag3_tag_handlers`（未文書化ハンドラ `quakeXX`）の 2 件が
  期待通り fail することを確認し、元へ戻した。
- E2E（長尺）: V8 `--autotest 420 --autotest-out target/autotest-w7`（295 フレーム）/
  tsint `--engine tsint --autotest 300 --autotest-out target/autotest-w7-tsint`（210 フレーム）
  いずれも EXIT=0。両ログとも ERROR 0 件・`未対応タグ` warn 0 件。WARN は各 19 件で
  内容・件数とも両エンジンで完全一致（対象外スタブ 15 種×1、`hch`/`hact` フォールバック×1、
  `[save] disablestore 中` ガード×2 — すべて意図した設計上のログ）。**両ログの
  スクリプトイベント列（タグ実行ログ 112 行）はエンジン名/実行秒数/フレーム数の
  ヘッダ 3 行を除いて完全一致**（W1〜W6 全デモの実行順序・回数が一致）。両エンジンとも
  first.ks（W1〜W6 デモ全通過）→ second.ks（W2/W4 デモ）→ タイトル画面まで到達し、
  最後は「つづきから」でセーブメニューを開いた状態で時間満了した（W4 検証時の tsint と
  同じ良性の終端状態。今回は両 E2E + clippy を同一マシンで並列実行したため実時間
  choreography との対応が W6 単独実行時よりわずかに後ろへずれ、`*bye` の `[close]` 到達前に
  満了した。両エンジンが完全に同じ地点・同じイベント列で終わっており、ハング・エラーは無い）。
- フレームハッシュ比較: 対応する同名フレーム 210 組の sha256 を比較し、16 組が
  バイト単位一致（`frame_00`〜`frame_08`（03 を除く）、`frame_23`、`frame_73`〜`78`・`81`）。
  特に `frame_07_hover.png`（font/ruby 装飾フレーム）は W5 検証時に記録したものと同一の
  ハッシュ `05c84f4b...` で、**ウェーブをまたいだピクセル単位の回帰なし**を確認した。
  残りの不一致は、サンプル画面右下の常時アニメーションする装飾円の位相差と、
  トランジション/デモ進行のタイミング差（tsint のデモ進行がログ時刻で 2〜6 秒遅れて
  推移する既知の性質。スクリーンショットは実時間の固定間隔で撮られるため、決定論的に
  同一の演出列の「別の瞬間」を写しているだけ）によるもので、W3/W5 で確認済みの
  既知の一過性事象の範囲内（イベント列の完全一致が内容同一性の裏付け）。
- `cargo clippy -p noiria-core -p noiria-app -p noiria-tools --all-targets`: 変更ファイル
  （engine_state.rs / kag_tag_compat.rs / tags.ts / player.ts / Cargo.toml）で新規 warning 0。
  既存 warning は W1〜W6 時点と同じ無関係ファイル（noiria-tools 各所、noiria-app の
  mod.rs/loader.rs の test module 配置）のもの。

### プロジェクト総括（W1〜W7 の要点）

- **W1 文字装飾（11 タグ）**: font/style/ruby/locate 系。tsint の循環 import 制限が実機検証で
  顕在化（player.ts ⇄ tags.ts）。
- **W2 音声（16 タグ）**: SE バッファ 16 本・bgm2 チャンネルのクロスフェード・フェード系。
  ヘッドレスでの `wl` 無限ハング防止フォールバック。
- **W3 複数メッセージレイヤ（11 タグ）**: MessageLayer 化・セーブ v2。複数 await ループの
  jumpRequest 未検出不具合を発見・修正。tsint の op 順序回帰テストとの互換維持。
- **W4 UI・ボタン・クリック制御（21 タグ）**: ButtonManager・layer_blit_image op。ask/input は
  「任意クリックで必ず閉じる」設計でヘッドレス自動化とのハングを回避。
- **W5 トランジション/アニメ/画像（7 タグ + 属性拡張）**: tween_queue の折れ線 move・
  scroll トランジション・.asd 最小サブセット。W3 由来の copylay 位置バグを修正。
- **W6 ストア系+動画拡張+残余（32 タグ + 対象外スタブ 15）**: 完全スナップショット
  （tempsave/trail）・iscript の TS 評価・動画 pause/resume/rewind。tsint の for-of +
  クロージャ束縛共有バグが実機検証で顕在化。
- **W7 全数監査**: 対応表×コードの機械照合を cargo test へ恒久化（161 行 = ✅146 + ➖15 と
  tags.ts 登録の双方向一致）。image dev-dep 除去・edgecolor 配線・ドキュメント最終更新。

### 恒久的な開発規約のまとめ（W1〜W7 で確立、今後も厳守）

1. **循環 import 禁止**: `runtime/kag/` 配下のモジュール間の値 import は必ず一方向に保つ
   （player.ts → tags.ts/buttons.ts/anim.ts）。tsint は import を評価順に解決するため、循環が
   あると未評価バインディングを `undefined` のまま掴んで実行時 `TypeError` になる（W1 実証）。
2. **複数 await ループの jumpRequest ガード**: タグハンドラ/プレイヤー内で `await` を複数回
   行うループは、各 wait 後に `jumpRequest` を確認して早期 return する（`KagPlayer.waitWhile()`
   を再利用。怠るとジャンプ後のレイヤへ旧シナリオのテキストが紛れ込む。W3 実証、W5 で汎用化）。
3. **op 順序の回帰テスト依存**: tsint リポジトリの `tests/kag_full.rs` が初期化時 op 呼び出し列を
   厳密比較している。`applyMessageLayerConfig` の 4 ステップ分割（2 回の `layer.set`）など
   一見冗長な呼び出し順序は意図的な後方互換であり、まとめるリファクタは禁止（W3 実証）。
4. **tsint の for-of + クロージャバグ回避**: ループ変数をクロージャへ閉じ込める場合は
   `for...of`/`for...in` ではなく `forEach`/`map` 等のコールバック反復を使う（tsint は
   ループ変数のスロットを使い回すため全クロージャが最終値を共有する。W6 実証）。
   そのほか tsint 非対応構文（`Array.from`、型位置の負数リテラル）も避ける。
5. **追記キャッシュ不変条件**: text.rs のレイアウトは前方テキストのグリフ位置を追記で変えない
   （`ops_text` の追記キャッシュの前提。`appending_keeps_prefix_glyph_count` 系テストが監視）。
   行送りをセグメント単位で可変にする類の変更はこの不変条件を壊すため要注意（W1/W3 の
   ruby/inline_image は「baseline 一律シフト」「はみ出し許容」でこの前提を守った）。
6. **表とコードの同期**: `docs/kag-tag-compat.md` と `runtime/kag/tags.ts` は
   `tests/kag_tag_compat.rs` が機械照合する。タグ・ステータス・スタブを増減する際は両方を
   揃えて更新する（W7 恒久化）。
7. **TS ミラーとネイティブ状態の二重管理に注意**: レイヤの位置・内容は「TS 側の権威状態
   （`MessageLayer.cfg`/`segs`/`links` 等）→ ops でネイティブへ反映」が正方向。ネイティブ状態を
   **直接**書き換える経路（`copylay` の位置コピー等）を追加すると、TS 側ミラーと食い違って
   「次に cfg が再適用されるまで直らないズレ」を生む（W3→W5 の copylay 位置バグ、copylay 後の
   リンク無効、`page=` が受理されるのに効かない件はすべてこの構造が根因）。新機能がネイティブ
   状態を触るときは、①TS 側の権威状態も同時に更新する ②またはメッセージレイヤ等の
   「cfg 駆動」レイヤを対象から除外する、のどちらかを必ず選ぶこと。

## 2026-07-11 選択肢クリック音の二重再生修正

- **判断**: 選択肢（リンク）クリック時のクリック音はエンジン側（`tryClickLink` の SE 再生）を正とし、
  シナリオ側で選択直後に鳴らしていた `first.ks` *like 冒頭の `[playse storage=click.wav]` を削除した
  （ユーザー報告: 「気に入った」選択時にクリック音が二回鳴る）。**理由**: リンク選択音は全リンク共通の
  エンジン既定動作であり、シナリオが個別に重ねると常に二重になる。**影響**: シナリオ作者向けの規約として
  「選択直後の演出 SE に click.wav 系を使わない」（使う場合は別の効果音にする）。first.ks にコメントで明記。

## 2026-07-11 タグ振る舞いテストスイート

タグ単位の振る舞い検証（属性→ops 変換・状態遷移）を体系化する新設テスト群。
`crates/noiria-app/src/script/tag_harness.rs`（ハーネス）+ `tag_behavior_tests.rs`
（カテゴリ別テスト 23 件）。直近の「リンククリック SE 二重再生」のような、
コア単体テストでも E2E 目視でも拾えない層（タグ→ops の変換層）を回帰で固定する。

### ハーネス方式の選定（判断→理由→影響）

- **判断**: 候補 3 案（①js_tests の V8 環境で ops をラップして記録 ②EngineState に記録
  モードを追加 ③tsint kag_full.rs 同等の noiria-app 内再実装）のうち **②を採用**した。
  `EngineState` に `op_log: Option<SharedOpLog>`（`Arc<Mutex<Vec<String>>>`）を追加し、
  各 ops メソッドの先頭で `log_op()`（`None` なら no-op）を呼ぶ。ハーネスは本番と同じ
  `spawn()`（V8）/`tsint_engine::spawn()` でスクリプトスレッドを起動し、`EngineCtx` 経由で
  記録先を注入する。**理由**: ①は V8 専用になり tsint 比較ができない。③は kag_full.rs の
  モック ops テーブル（600 行超）を二重管理することになり、実 ops（実 Scene/実 GameDir）を
  通らないため「ops 本体まで含めた本物の経路」を検証できない。②は 1 箇所の計装で両エンジン
  共通に効き、V8/tsint の ops 列が**文字列として完全一致**することを直接検証できる。
  **影響**: 本番（`main.rs`）は常に `op_log: None` を渡すため通常実行のオーバーヘッドは
  `Option` の分岐 1 回のみ。高頻度ポーリング系 op（`text_is_done`/`audio_is_playing`/
  `layer_moving`/`trans_active`）は意図的に記録しない（`wl`/`wm`/`wt` のポーリングでログが
  埋もれるため）。`op_input_next_event`/`op_wait`/`op_now`/`read_text`/`file_exists` も
  記録対象外（決定論性・ノイズの観点）。`text_rects` は低頻度（実クリック時のみ）なので
  記録し、リンクの実座標をテストが逆算できるようにした。
- **判断**: `api.sys.log`（`op_log` op）も `EngineState::sys_log()` 経由で記録対象にした。
  **理由**: W2/W4/W6 の多くのタグ（bgmopt/pausebgm/button/timeout/tempsave/save/... 等）は
  レイヤ/オーディオ ops を呼ばず、W2 で導入した info ログが唯一の観測可能な効果のため。
  **影響**: V8 側 `op_log` に `OpState` 引数が増えた（挙動は不変）。tsint 側も同様に
  `es.borrow().sys_log()` 経由へ変更。

### 決定論化の方法

- クリック等の入力は `InputEvent` チャンネルへ 1 発ずつ送り、送信前に
  `wait_for_quiescence()`（`op_log` が非空かつ 80ms 伸びない = スクリプトが次の待ちに到達）
  で同期する。**実機デバッグで判明した罠 2 件**: (1) V8 isolate 生成 + `runtime/kag/*.ts`
  トランスパイルに実時間 200ms 前後かかるため、固定 sleep では最初のクリックがエンジン起動
  より先に届いて失われる（`InputHub.pump()` は配送時点で登録済みのハンドラにしか届けない）。
  `len == 0` の間は安定判定を開始しないことで回避。(2) deno_core の `run_event_loop` は
  外部から一度もイベントが来ないうちは同期的な続きへ進まないことがあるため、クリック列の
  前に「起爆」用ダミーイベントを 1 発送る。ダミーは `kind: "move"` だと updateHover →
  text_rects が消費タイミング依存で発行され **V8/tsint 比較が非決定的になった**（実測）ため、
  どのハンドラにもマッチしない `kind: "key", key: "F24"`（ops 無発行で完全に不活性）にした。
- ハーネス内に**ミニレンダラポンプ**（15ms 周期のスレッド）を置き、本番の描画ループ
  （headless.rs / win32.rs）が担う「完了トランジションの swap + `trans_notify` 通知 +
  完了 tween の確定」を代行する。これが無いと `[wt]`/`[wm]` がハーネス上で永久に完了しない
  （記録専用ハーネスに描画は不要だが、シーン状態の時間進行だけは誰かが押す必要がある）。
- フラグメントは `*start\n[nowait]\n{断片}\n[close]` に包んで実行する。`[nowait]` 既定で
  文字送りを瞬間表示にし、`[l]`/`[p]` を「1 クリック = 1 送り」に単純化する（リベール未完で
  1 クリック目が「表示完了」に消費される KAG 挙動をテスト側で数えなくて済む）。`[close]` の
  `UserEvent::Quit` 受信でログを回収して終了。10 秒のウォッチドッグ（Redraw 連打中でも
  評価されるようループ先頭で判定）でハングをテスト失敗に変換する。
- 巻き戻し系タグ（tempload/goback/gotostart/load）のフラグメントは、`games/sample/first.ks`
  の W6 デモと同じ **sf ガード**（復元で巻き戻されない `sf` のフラグで 1 回だけ実行）を使う
  （`f` は復元対象なのでガードに使えない。W6 の記録参照）。
- 副作用の掃除: フラグメント実行は本番と同じ経路でセーブ（`%APPDATA%\Noiria\<一意 ID>`）へ
  書き込むため、実行後にベストエフォートで当該ディレクトリを削除する。スクリプトスレッド
  自体は `[close]` 後も `idleUntilJump` のポーリングで残るが join しない（テストプロセス
  終了時に回収される。1 テスト 1 スレッド残置の許容トレードオフ）。

### V8/tsint ops 列自動比較（懸案の実現）

- `run_v8()`/`run_tsint()` に同じ断片・同じクリック列を渡し、`assert_logs_match()` が
  ops ログ（文字列 Vec）の完全一致を検証する。書式は `EngineState` を共用しているため
  両エンジンで同一。**結果: 同期タグ断片・リンククリック断片とも完全一致**
  （`v8_and_tsint_emit_identical_ops_for_sync_fragment` /
  `v8_and_tsint_emit_identical_ops_for_link_click_fragment`）。
  上記のダミーイベント種別の件以外に差異は見つからなかった。

### カバレッジ照合の恒久化

- `all_ok_tags_from_compat_table_appear_in_fragment_corpus`（`tag_behavior_tests.rs` 内）が
  `docs/kag-tag-compat.md` の ✅146 タグ全てが本ファイルのフラグメント断片群に「タグ名の
  完全トークン」として最低 1 回出現することを機械照合する（`[` 直後の識別子を抽出、
  `[l]`/`[link]` の前方一致混同なし）。断片はデータファイルに分離せず本ファイルの生文字列
  リテラルに集約し、自ファイルを走査する（kag_tag_compat.rs の「表とコードの機械照合」と
  同形式）。今後タグを増やしてテスト断片を書き忘れると fail する。

### 検証結果（タグ振る舞いテストスイート）

- `cargo test`（ワークスペース）: 233 → **256 件全通過**（noiria-app 35→58。新規 23 件:
  スモーク 1 + V8/tsint 比較 2 + 回帰 2 + カテゴリ別 17 + カバレッジ照合 1）。既存 233 件は
  無変更で全通過。テスト実行時間は tag_behavior_tests 全体で約 1 秒（横並び実行）。
- `cd ../tsint && cargo test`: 全 12 スイート通過（runtime/ は無変更のためコーパス影響なし）。
- E2E: V8 / tsint とも `--autotest 24` EXIT=0（後方互換確認、`op_log: None` 経路）。
- 発見した実装上の知見（バグではない）: (a) `InputHub.pump()` は「配送時点で登録済みの
  ハンドラにしか届けない」ため、入力の事前一括キューイングはできない（ハーネスは
  quiescence 同期で対処）。(b) deno_core は最初の外部イベントが来るまで進まないケースがある
  （起爆イベントで対処）。いずれもハーネス設計にのみ影響し、実プレイでは人間の入力間隔が
  十分長いため顕在化しない。製品コードの不具合は今回は発見されなかった。

## 2026-07-12 実装ノートレビューでの指摘対応

ユーザーと実装ノート全文をレビューし、以下 3 点を修正した。

- **判断**: sample の W6 デモから `[clearsysvar]` の実演を撤去した（`[clearvar]` と trace のみ残す）。
  あわせてデモ入口で `sf.w6*Done` ガード 4 つを毎回 false にリセットするようにした。
  **理由**: sf はラベル通過ごとにディスクへ永続化される（`saveSys()`）ため、デモの `[clearsysvar]` が
  プレイヤーの実設定（音量・文字速度）と既読スキップ状態を**周回のたびに全消去**していた。また
  ガードのリセットは従来 `[clearsysvar]` の副作用に依存しており、撤去すると W6 デモが
  「インストール後 1 回しか動かない」ことになるため、明示的なリセットに置き換えた（巻き戻し系は
  リセット行より後ろへしか戻らないので多重実行にはならない）。**影響**: タグ自体は実装済みのまま
  （タグ振る舞いテストと対応表は不変）。サンプル通過でユーザー設定が壊れなくなる。
- **判断**: `docs/kag-tag-compat.md` に「KAG3 との意味論差異一覧（移植時の注意）」節を新設し、
  備考・実装ノートに散在していた解釈差異（timeout/wc/gotostart/waittrig/ask/iscript/bgmopt 等）と
  機能制限（trans layer=/.asd サブセット/IME 非対応等）を 1 箇所へ集約した。**理由**: KAG3 からの
  移植者が 1300 行の実装ノートを発掘しないと差異に気付けない構造だった。**影響**: 照合テスト
  （kag_tag_compat.rs）は「## 対応表」〜「## ウェーブ割当」間のみを解析するため新節の影響なし
  （箇条書きで記述しテーブル行も増やしていない）。
- **判断**: 恒久規約に 7 項目め「TS ミラーとネイティブ状態の二重管理に注意」を追加した。
  **理由**: copylay 位置バグ（W3→W5）・copylay 後のリンク無効・page= が効かない件はいずれも
  同じ構造（TS 側権威状態を経由しないネイティブ直接変更）が根因で、再発性が高い。
- 誤字修正: 「från直接参照」→「から直接参照」、「読み取り読み取り専用」→「読み取り専用」、
  「折り返わたる」→「折り返される」、W1 のルビ泣き別れ重複項目を削除。

## 2026-07-12 KAG3 意味論差異の解消

`docs/kag-tag-compat.md`「KAG3 との意味論差異一覧」のうち修正可能と判断した 8 項目を KAG3 準拠へ
寄せた。ベースラインは `cargo test`（ワークスペース）256 件全通過。

### 1. [timeout]: クリック待ち中のみ発火するよう修正

- **判断**: `KagPlayer.setTimeoutJump()` が満了時に `inAdvanceWait`（`waitAdvance()`/
  `waitForJump()` の間だけ true になる新設フラグ）を確認し、true ならその場で発火（従来通り）、
  false なら `timeoutExpired = true` を立てるだけにして発火を保留する。`waitAdvance()`/
  `waitForJump()` の入口（`checkTimeoutOnWaitEnter()`）で `timeoutExpired` を確認し、満了済みなら
  即発火する。発火（`fireTimeoutJump()`）は 1 回限りで自動的に `timeoutJump`/`timeoutExpired` を
  クリアする（KAG3 の `[timeout]` が 1 回発火型のため）。**理由**: 「クリック待ちのタイムアウト」
  という KAG3 の意味論に合わせつつ、既存の `watchTimeout()`（世代カウンタでの自然消滅パターン）を
  そのまま流用できる最小差分にした。**影響**: `[timeout]` 直後に `[s]`（他にジャンプ手段が無い
  停止）を置く既存の安全なパターン（`games/sample/first.ks` の W4 デモ）は、満了時点で既に
  `waitForJump()` の中にいるため**挙動が完全に不変**（実機 E2E で確認済み、下記検証結果参照）。
  新しく効くようになるのは「`[timeout]` 設定後、`[wait]`・タグ処理等クリック待ち以外の区間で
  満了した場合はジャンプを保留し、次のクリック待ちに入った瞬間に発火する」という KAG3 本来の
  挙動（`tag_behavior_tests.rs::w7_timeout_fires_only_during_click_wait` で検証）。クリックで待ちが
  解決されてもタイマー自体は解除しない（`[ctimeout]` を呼ぶまで、または実際に発火するまで有効）
  という既存の設計は維持した。

### 2. [gotostart]/[startanchor]: 専用アンカースロットに変更（trail から分離）

- **判断**: `KagPlayer.anchor: SaveData | null`（専用の 1 スロット）を新設。`[startanchor]`
  （既定 `enabled=true`）はその時点の `makeFullSnapshot()`（完全スナップショット、`pos.index` +
  カレントレイヤの表示内容込み）を `anchor` へ記録する。`enabled=false` は `anchor = null` で
  破棄する。`[gotostart]` は `anchor` へ復帰する（消費しない、`applySnapshot(anchor, false)`）。
  旧実装の `[s]` ごとの自動 `record`（`autoAnchor`/`maybeAutoAnchor()`、`[startanchor enabled=]`
  がトグルしていたもの）は**廃止**し、`KagPlayer.waitForJump()` からも呼び出しを削除した。
  **理由**: KAG3 の `[startanchor]`/`[gotostart]` は「明示的にアンカー地点を 1 つ記録し、そこへ
  戻る」という意味論であり、「`[s]` ごとに勝手に記録し続ける」という旧実装の解釈とは異なって
  いた。ウェーブ指示の「`[s]` ごとの自動 record は goback 用の trail 積みとして維持してよい
  （設計判断を notes に記録）」を受け、検討の結果**維持しない**判断をした。理由は次の 2 点:
  (a) 既存のサンプル・タグ振る舞いテストの goback デモはいずれも明示的な `[record]` を使っており
  自動記録に依存していない、(b) 自動記録を「常時 ON」にすると `trail`（W7 でセーブデータへ
  永続化するようにした、下記 3 参照）が毎回のセーブに大量のエントリを含むことになり、サイズ
  面でも CLAUDE.md の「単純に済むところを複雑にしない」方針に反する。トグルする代わりの
  タグも用意されていない（`[startanchor]` は意味が変わったため）ため、単純に廃止して
  「`[record]` による明示記録のみ」に一本化した。**影響**: `docs/kag-tag-compat.md` の
  `startanchor`/`gotostart`/`record`/`goback` 各行を更新した。`games/sample/scenario/first.ks`
  の「巻き戻しアンカー」デモ（旧実装は `[record]`→`[gotostart]` で `trail[0]` を使っていた）を
  `[startanchor]`→`[gotostart]` へ書き換えた（`docs/implementation-notes.md` 内の別項「サンプル
  E2E で発見した既存の無限ループ」も参照）。`clearsysvar`（`resetSysDefaults()`）は `trail` に
  加えて `anchor` もクリアするようにした（sf 全消去と一貫させるため）。

### 3. [tempsave]/[tempload]: place の符号でディスク/メモリを切り替え

- **判断**: `KagPlayer.tempSave(place)`/`tempLoad(place)` を新設。`place >= 0` は
  `sys.saveWrite`/`sys.saveRead` でディスクの `"temp{place}"` スロット（通常セーブ `"save{N}"`
  とは別プレフィックス）へ、`place < 0` は従来通り `KagPlayer.tempSlots`（メモリ上のみの Map）へ
  読み書きする。**理由**: KAG3 の「負の place はメモリ専用スロットを指す」慣習に対応させる
  （ウェーブ指示に明記された解釈をそのまま採用）。**影響**: `docs/kag-tag-compat.md` の
  `tempsave`/`tempload` 行を更新。セーブメニュー（`renderSaveMenu()`）は `api.sys.saveList()` の
  結果から `"save{N}"` という名前のスロットだけを `Map.get()` で拾う実装のため、`"temp{N}"`
  スロットが増えても表示には一切混ざらない（既存コードのままで確認済み、フィルタ追加は不要
  だった）。

### 4. trail（record/goback）のセーブ永続化

- **判断**: `SaveData` に `trail?: SaveData[]` を追加したが、`makeSnapshot()`/`makeFullSnapshot()`
  自体は一切変更していない（この 2 つの生成物には `trail` フィールドを持たせない）。`trail` は
  `KagPlayer.save()` が実際にディスクへ書き出す直前にだけ `{ ...this.snapshot, trail: this.trail }`
  として付加する。`load()`（`restoreTrail=true` を渡す）だけが `this.trail = data.trail ?? []` で
  丸ごと差し替え、`goBack()`/`goToStart()`/`tempLoad()`（いずれも `restoreTrail=false`）は
  `this.trail` に一切触れない。**理由**: 「trail エントリ自体には trail を入れ子にしない」という
  サイズ対策の要求に対し、そもそも `trail` の要素（`this.trail` の中身、`[record]` が
  `makeFullSnapshot()` で作る）は生成された時点で `trail` フィールドを持たない（`makeSnapshot()`
  が触れないため）ので、明示的な「除去」処理を書く必要が無い最小実装になった。`restoreTrail`
  という bool 引数を追加したのは、`applySnapshot()` が `[load]`（ラベル境界からの正規のロード）
  と `[goback]`/`[gotostart]`/`[tempload]`（`this.trail` を呼び出し側が既に操作済み、または
  意図的に触れてはいけない内部復元）の**両方**から呼ばれる共通関数だったため。特に `[goback]`
  は呼び出し前に `this.trail.pop()` 済みであり、ここで `data.trail ?? []`（`undefined` なので
  空配列）へ無条件に差し替えてしまうと pop 後の trail が消えてしまう（実装中に気付いて
  `restoreTrail` 引数を追加する設計にした）。**影響**: `docs/kag-tag-compat.md` の `record`/
  `goback` 行を更新。旧セーブ（`trail` フィールドが無い v1/v2）は `load()` の `data.trail ?? []`
  でロード時に空配列補完される（後方互換）。`tag_behavior_tests.rs::w7_trail_persists_across_save_and_load`
  で「`[load]` 後も `[goback]` が warn せず成功する」ことを検証した（`[record]`/`[load]`/
  `[goback]` いずれも `sf` ガードで 1 回に絞らないと、`[load]` のラベル再生がフラグメント全体を
  再実行してしまい `goback` の warn 有無だけでは「trail が本当に復元されたか」を検出できなく
  なる点に実装中に気付き、`[record]` にも `sf` ガードを追加した。テストのコメント参照）。

### 5. kag グローバル: evalExpr/evalStmt に第 3 引数として追加

- **判断**: `runtime/kag/vars.ts` の `evalExpr(exp, sys, kag?)`/`evalStmt(exp, sys, kag?)` に
  第 3 引数 `kag`（省略可、既定 `undefined`）を追加し、`new Function("f","sf","tf","sys","kag", ...)`
  へそのまま渡す。`runtime/kag/player.ts`/`runtime/kag/tags.ts` 内の全 `vars.evalExpr`/
  `vars.evalStmt` 呼び出し箇所（grep で洗い出し、player.ts 7 箇所・tags.ts 5 箇所の計 12 箇所）に
  `this`/`p`（呼び出し元の `KagPlayer` インスタンス）を第 3 引数として追加した。`runtime/kag/
  buttons.ts` の 4 箇所（`checkbox`/`edit` の変数バインド）は `KagPlayer` への参照を持たない
  自己完結モジュール設計（W4 の循環 import 回避方針）のため据え置き、`kag`/`sys` とも
  `undefined`（この呼び出しパターンは既存の制約であり本タスクのスコープ外）。**理由**: `kag` は
  `sys.player` と同一インスタンスだが、`sysObject()` の中の `player` フィールド越しに
  `sys.player.trigger(...)` と書かせるより、KAG3 標準の `kag.trigger(...)` をそのまま使えるほうが
  移植性が高いというウェーブ指示に従った。**影響**: 既存の `sys.player.trigger(...)` 経由の
  呼び出しは後方互換で引き続き動く（`kag` と `sys.player` は同じオブジェクト）。`tsint` 側は
  `new Function` を実行時コンパイルする既存実装（W6 で確認済み）がそのまま動くため無変更で
  対応できた（`cd tsint && cargo test` で確認、下記検証結果参照）。`evalExpr`/`evalStmt` は
  ops を一切呼ばない純粋な JS 実行のため、シグネチャ変更は tsint の op ログ回帰テスト
  （`tests/kag_full.rs`）の出力へは影響しない（実際に無変更で通過することを確認した）。

### 6. [erasebookmark]: 新規 op sys.saveDelete による真の削除

- **判断**: `noiria-core::savedata::delete_slot(game_id, slot)`（存在すれば `.json` を削除、
  存在しなければ no-op、サムネイル `.png` も合わせて削除を試みる）を新設し、
  `EngineState::save_delete()` → `op_save_delete`（`ops_sys.rs`、V8）/ 同名 native op
  （`tsint_engine.rs`）→ `api.ts` の `sys.saveDelete(slot)` → `noiria.d.ts` へ 3 点セットで配線した
  （`op_save_write`/`op_save_read`/`op_save_list` と同じパターンを踏襲）。`tags.ts` の
  `erasebookmark` ハンドラは `api.sys.saveWrite(slot, "{}")` を `api.sys.saveDelete(slot)` に
  置き換えた。**理由**: ウェーブ指示が新規 op の追加を明示していたため（旧実装は「新規 op を
  増やさない」制約下での次善策だった）。**影響**: `docs/kag-tag-compat.md` の `erasebookmark` 行を
  更新。削除後は `renderSaveMenu()` が「（空きスロット）」表示に戻る（`api.sys.saveList()` に
  当該スロットが含まれなくなるため、メニュー側のロジック自体は無変更で正しく動作する）。
  `crates/noiria-core/src/savedata.rs` に `deletes_slot_and_missing_is_a_noop` テストを追加
  （既存の `roundtrip_and_list`/`rejects_bad_slot_names` と同じ `TEST_ID` を共有すると並列テスト
  実行時にディレクトリ操作が競合して稀に失敗することが実行中に判明したため、専用の
  `DELETE_TEST_ID` を使うよう分離した）。`tag_behavior_tests.rs::w7_erasebookmark_truly_deletes_the_slot`
  で `kag.api.sys.saveRead(...)` が削除後に `null` を返すことを検証した（`kag` グローバル
  （項目 5）の副次的な検証も兼ねる）。

### 7. [move path=]: 時間配分を距離比例に修正

- **判断**: `runtime/kag/tags.ts` に純関数 `allocateMoveLegTimes(startX, startY, points, totalMs)`
  を追加し、各区間の時間を「区間の始点から終点までのユークリッド距離」に比例して `totalMs` から
  配分するようにした（全区間の距離合計が 0 の場合、すなわち path 全体が opacity のみの変化で
  x/y が動かない場合は距離比例で割り切れないため、区間数での均等割りへフォールバックする）。
  `move` タグハンドラの `path=` 分岐を、旧来の `num(a.time, 1000) / points.length`（固定の
  均等割り）から `allocateMoveLegTimes(st.x, st.y, points, num(a.time, 1000))` の結果を各区間の
  `timeMs` に使うよう変更した。Rust 側の waypoint 機構（`Scene::tween_queue`）は無変更。tsint に
  `Math.hypot`/`Array.prototype.reduce` が存在しないことをソース確認済みだったため（`Math.pow`/
  `Math.abs` 等の四則関数のみ実装済み）、距離計算は `Math.pow(dx*dx+dy*dy, 0.5)`、集計は素朴な
  `for` ループ（`push`/`+=`）で実装し、tsint 非対応構文を避けた。**理由**: KAG3 の `path=` 移動は
  等速（区間の長さに比例した時間がかかる）が期待される挙動であり、均等割りだと区間ごとの
  移動距離が違うと速度が不均一に見えるという既知の制限を解消するため（ウェーブ指示どおり）。
  **影響**: `docs/kag-tag-compat.md` の `move` 行・「機能制限」節を更新（「折れ線のみ・スプライン
  非対応」は変わらず残る制限として維持）。個々の区間が距離 0（例えば前後の区間だけ動きがあり、
  ある 1 区間だけ opacity のみの変化）の場合、その区間の配分時間は 0ms になり得る（瞬間適用）。
  「距離 0 のときの均等割りフォールバック」は path 全体が距離 0 のケースにのみ適用される設計
  にした（個々の区間ごとにフォールバックする、より複雑な配分ロジックは要求されておらず、
  CLAUDE.md の「単純に済むところを複雑にしない」方針に基づきこのシンプルな解釈を採用した。
  複数の解釈がありうる点をここに明記する）。`mod.rs` の js_tests
  （`kag_anim_and_move_path_pure_functions_in_v8`）に `allocateMoveLegTimes` の単体テストを
  3 件追加（距離比例配分・全区間距離 0 のフォールバック・座標省略時の直前値継承）。Rust 側の
  `layer_move_path` 関連テストは無変更（ops の意味論自体は不変、TS 側の呼び出し引数が変わる
  だけのため）。

### 8. [pimage] の KAG3 属性名対応 + [glyph fix=]

- **判断（pimage）**: `runtime/kag/tags.ts` に純関数 `resolvePimageClip(a)` を追加し、`clip=` が
  指定されていればそれを優先し、無ければ `sx=`/`sy=`/`sw=`/`sh=`（KAG3 標準のコピー元矩形属性）
  から矩形を組み立てるようにした（`sw=`/`sh=` が 0 以下、またはいずれも未指定なら `null` =
  画像全体）。`dx=`/`dy=`（貼り付け先座標）は変更していない。**理由**: ウェーブ指示どおり
  `clip=` の別名として受理する設計。**影響**: `docs/kag-tag-compat.md` の `pimage` 行・
  「機能制限」節を更新（`dx=/dy=` の意味付けは引き続きエンジン独自解釈のまま残るが、コピー元
  矩形の属性名は KAG3 互換になった）。
- **判断（glyph fix=）**: `KagPlayer.glyphCfg` に `fix: boolean`（既定 `false`）を追加。
  `setGlyphVisible(true)` で表示する際に `updateGlyphPosition()` を呼ぶよう変更し、
  `fix=true` なら従来通りの固定位置（`left=`/`top=` 省略時はメッセージウィンドウ右下）、
  `fix=false`（新しい既定）は `api.text.rects(cur.layerId)` の**最終要素**（本文の最後の
  グリフ）の矩形から `x + w`（右端）・`y`（そのまま）を計算し、メッセージウィンドウの
  `marginX`/`marginY` を加えた原点を基準にグリフレイヤをそこへ配置する「ペン位置追従」を
  実装した。`rects` が空（本文が無い状態でクリック待ちへ入った場合）はメッセージウィンドウ
  左上マージン位置へフォールバックする。**理由**: ウェーブ指示どおり、KAG3 の既定（`fix=false`）
  がペン追従であることに合わせてデフォルトを反転させた。`api.text.rects` の返す配列は
  `raster_segments`/Pass3 がグリフを本文の出現順（バイト位置昇順）で push する実装のため、
  「最終要素 = 最後に印字されたグリフ」という前提が成り立つ（`linkAt()`/`menuHit()` 等、既存の
  他コードも同じ前提でこの配列を扱っている）。**影響**: `docs/kag-tag-compat.md` の `glyph` 行・
  「機能制限」節を更新（`fix=true` の固定位置のみ制限として残す）。`games/sample/scenario/first.ks`
  の W4 デモ（`[glyph line=icon_spark.png]`、`fix=` 省略）は自動的に新しい既定（ペン追従）の
  実演になる（表示位置が変わるだけで、クリック待ち自体の挙動には影響しない）。
  `tag_behavior_tests.rs::w7_glyph_fix_false_default_follows_pen_fix_true_stays_fixed` で
  「本文なしのフォールバック位置」と「`fix=true` の固定位置」の両方を、ハーネスのゲーム設定
  （`charaLayerCount: 1`、既定メッセージウィンドウ寸法）から手計算した座標値で検証した。

### サンプル E2E で発見した既存の無限ループ（本タスクのスコープ外だが検証をブロックしたため修正）

- **判断**: `games/sample/scenario/first.ks` の `*w6_demo` ラベル入口にあった
  `[eval exp="sf.w6TemploadDone = false; sf.w6GobackDone = false; sf.w6GotostartDone = false;
  sf.w6SaveLoadDone = false"]`（W6 完了後の「2026-07-12 実装ノートレビュー」で追加されたガード
  リセット）から `sf.w6SaveLoadDone = false;` を除去した。**発見の経緯**: 本タスクの検証で
  `cargo run -p noiria-app -- games/sample --autotest 300` を実行したところ、`[tempsave]`→
  `[tempload]`→`[record]`→`[goback]`→`[startanchor]`→`[gotostart]`→`[store]`→`[disablestore]`→
  `[save]`（disablestore でブロック）→`[disablestore]`→`[locksnapshot]`→`[unlocksnapshot]`→
  `[save]`→`[load]` が実時間 18 秒間隔でハングせず**永久に繰り返され**、`[copybookmark]`/
  `[erasebookmark]`/`[clearvar]`/`[iscript]` 以降のシナリオへ一度も到達しないことに気付いた。
  **原因**: 該当ブロックの `[if exp="!sf.w6SaveLoadDone"]`〜`[endif]` 内の `[load place=7]` は
  ラベル境界（`[save place=7]` 実行時点で最後に通過したラベル = `*w6_demo`）からの再生のため、
  `*w6_demo` へジャンプして**その入口の reset 行より前**（正確には reset 行を含むそのラベルの
  先頭）から再実行される。reset 行が `sf.w6SaveLoadDone` を毎回 false に戻してしまうため、
  `[load]` のたびに「まだ実行していない」判定に戻り、`[load]`→`*w6_demo` 再突入→reset→
  `[load]`…の無限ループになっていた（コメント自体には「巻き戻し系はいずれもこの行より後ろへ
  しか戻らないので、リセットが再実行されることはない」と書かれていたが、これは `pos.index` に
  よる厳密な再開を行う `tempload`/`goback`/`gotostart` には正しくても、ラベル境界の粗い再生しか
  しない `[load]` には当てはまらない、という見落としだった）。**修正**: `sf.w6SaveLoadDone` の
  リセットだけを削除した（他 3 つのガードはコメント通り安全なため維持）。この 1 フラグは
  「ゲームプロセス中に一度だけデモを実行できれば十分」で、複数プレイスルー間で毎回リセット
  する必要が無いため、削除しても実用上の不都合は無い。**影響**: `cargo run -p noiria-app --
  games/sample --autotest 300`（V8）/`--engine tsint --autotest 300`（tsint）とも、修正後は
  `[erasebookmark]`/`[clearvar]`/`[iscript]`/`[hact]`/動画拡張デモまで実時間 2 分強で到達し、
  `*bye` の `[close]`（`スクリプトから終了要求を受信`）で正常終了することを確認した（下記検証
  結果参照）。既存のタグ振る舞いテスト・単体テストはいずれもこのファイルを実行しないため無関係
  （このバグは実時間の E2E でしか顕在化しない）。CLAUDE.md の「不要な変更をしない」方針に鑑み、
  本タスクの 8 項目とは無関係な既存バグだが、要求された E2E 検証（`[timeout]`/W6 ストア系デモの
  実機通過確認）を完了するために不可欠な最小限の 1 行修正として実施した。

### 検証結果 (2026-07-12)

- `cargo test`（ワークスペース）: 256 → **265 件全通過**（noiria-app 54→62、新規 8 件:
  `tag_behavior_tests::w7_timeout_fires_only_during_click_wait`
  `w7_startanchor_gotostart_use_dedicated_anchor_not_trail`
  `w7_tempsave_tempload_disk_for_nonnegative_memory_for_negative`
  `w7_trail_persists_across_save_and_load` `w7_waittrig_fires_via_kag_global`
  `w7_erasebookmark_truly_deletes_the_slot` `w7_pimage_accepts_sx_sy_sw_sh_as_clip_alias`
  `w7_glyph_fix_false_default_follows_pen_fix_true_stays_fixed`。noiria-core 181→182、新規 1 件:
  `savedata::tests::deletes_slot_and_missing_is_a_noop`）。既存タグ振る舞いテスト
  `w6_store_script_history_video_tags` は新意味論に合わせて期待値を更新（`[startanchor enabled=
  true][startanchor enabled=false][record][record]` の前段を `[record][record]` のみに整理し、
  `gotostart` の直前に `[startanchor]` を置くよう修正、`tempsave`/`tempload` のディスク書き込み
  ログ (`save_write temp1`/`save_read temp1`) を追加検証、`erasebookmark` のログ検証を
  `save_write save3`（旧: 空 JSON 代替）から `save_delete save3`（新: 真の削除）へ更新）。
  `js_tests::kag_vars_eval_in_v8`（`kag` グローバルの評価・副作用の 2 アサーション追加）・
  `js_tests::kag_anim_and_move_path_pure_functions_in_v8`（`allocateMoveLegTimes`
  `resolvePimageClip` のテストケース追加）も既存テスト関数に検証を追加する形で更新した。
  カバレッジ照合テスト（`tag_behavior_tests::all_ok_tags_from_compat_table_appear_in_fragment_corpus`、
  `kag_tag_compat.rs` の対応表 161 行 = ✅146 + ➖15 の機械照合）はいずれも無変更で通過（対応表の
  行数・タグ名は変えていない）。
- `cd ../tsint && cargo test`: 全 12 スイート通過（`tests/kag_full.rs` の op ログ厳密比較含む。
  `evalExpr`/`evalStmt` へのシグネチャ変更・`sys.saveDelete` op 追加のいずれも op 列を変えない
  ことを確認した）。
- `cargo run -p noiria-app -- games/sample --autotest 24`（V8）/
  `--engine tsint --autotest 24`（tsint）: いずれも EXIT=0、エラー・`未対応タグ` warn 0 件
  （後方互換確認、W1〜W3 相当の初期区間）。
- `cargo run -p noiria-app -- games/sample --autotest 300`（V8、`target/autotest-w7-v8-final`）:
  EXIT=0（実時間 2m11s で `*bye` の `[close]` に到達し早期終了）。ログに `[timeout] time=1200` →
  （約 2 秒後）`[ctimeout]` の順で記録され、[timeout]→[s] パターンが従来どおり発火してシナリオが
  進むことを確認。W6 デモも `[startanchor]`→`[gotostart]`（各 2 回、warn 0 件）→
  `[copybookmark]`→`[erasebookmark]`→`[clearvar]`→`[iscript]`→`[ignore]`→`[hact]`→動画拡張まで
  通過し、エラー 0 件を確認した。
- `cargo run -p noiria-app -- games/sample --engine tsint --autotest 200`
  （`target/autotest-w7-tsint-fix`）: EXIT=0（実時間 2m11s で早期終了）。V8 と同じログ順序
  （`[timeout]`→`[ctimeout]`、`[startanchor]`→`[gotostart]`→`[copybookmark]`→`[erasebookmark]`→
  `[clearvar]`→`[iscript]`→`終了要求`）で完走し、エラー 0 件を確認した。
- `cargo clippy -p noiria-core -p noiria-app -p noiria-tools --all-targets`: 変更ファイル
  （savedata.rs / engine_state.rs / ops_sys.rs / tsint_engine.rs / mod.rs / vars.ts / player.ts /
  tags.ts / api.ts / noiria.d.ts）で新規 warning は 0。既存 warning は W1〜W7 時点と同じ無関係
  ファイル（noiria-tools 各所、noiria-app の mod.rs/loader.rs の test module 配置）のもの。

## 2026-07-12 シナリオラベル整合性テストの新設

- **判断**: サンプル全ラベルの手動監査（下記）に合わせ、`crates/noiria-app/tests/scenario_labels.rs`
  （3 テスト）を新設して恒久化した。当初は games/sample のみを対象にしていたが、同日中に games/ 配下の**全ゲーム**（testbed の menu.ks 含む）+ 各 main.ts からエントリ .ks を動的特定する方式へ拡張した。検証内容: ①`target=*x` を持つ全タグ参照（storage= 省略時は
  同一ファイル）が実在ラベルを指す ②`storage=*.ks` が実在ファイルを指す ③同一ファイル内の
  ラベル重複なし ④ファイル先頭ラベルの到達不能検出（フォールスルーで到達できないのは先頭
  ラベルのみのため、参照ゼロを許容しない）。**理由**: シナリオを書き足すたびにラベルの
  リンク切れを cargo test で検出できるようにする（kag_tag_compat.rs と同じ機械照合方式）。
  **影響**: 抽出はブラケットタグのみ・iscript ブロックとコメント行は除外・引用符内空白の
  ラベル名は非対応（ラベル名に空白を使わない前提、テスト冒頭コメントに明記）。
- 監査結果（2026-07-12 時点）: 定義 11 ラベル（title 2 / first 6 / second 3）、参照 14 件
  すべて解決、重複なし、到達不能なし。E2E でも全ラベル経路の実走を確認済み
  （*like/*dislike・*restart/*bye は choreography のクリック位置により別ランで両分岐を通過、
  *bye→[close] 到達は 2026-07-12 の 300 秒ランで確認）。

## 2026-07-12 配布対応 W1: Linux ヘッドレス成立

対象: `crates/noiria-core/src/font/registry.rs`（フォント探索の per-OS 化 + 再帰走査 +
ゲームローカルフォント）、`text.rs`（FALLBACK_CHAIN の per-OS 化 + ゲームフォント優先解決）、
`assets.rs`（`GameDir::list_dir`/`read_bytes`）、`crates/noiria-app/src/script/engine_state.rs`
（`EngineState::new` への集約 + 起動時ゲームフォント読込）、`docs/linux-dev.md` +
`scripts/wsl-test.sh`（新規）。承認済み計画 `breezy-sauteeing-fox.md` の W1 節に従う。
ベースラインはワークスペース `cargo test` 268 件全通過（Windows）。

### 1. フォント探索ディレクトリの選定と再帰走査

- **判断**: `FONTS_DIR` 定数（`C:\Windows\Fonts` 固定）を廃止し、`system_font_dirs()` を
  `#[cfg(target_os = "windows")]`/そうでない場合の 2 実装に分けた。Windows は従来どおり
  `C:\Windows\Fonts` に加え `%LOCALAPPDATA%\Microsoft\Windows\Fonts`（ユーザー単位インストール
  分）を追加。Linux は `/usr/share/fonts`、`/usr/local/share/fonts`、`directories::BaseDirs`
  で解決したホーム配下の `.local/share/fonts`・`.fonts` の 4 箇所。**理由**: `directories`
  クレートは savedata.rs で既に依存済み（新規クレート追加ではない）なので、fontconfig
  非依存という要件を保ったままホームディレクトリだけクレートに解決させた。ディストリの
  慣習パス（Ubuntu/Debian 系の `/usr/share/fonts/truetype/...`）を汎用的にカバーするため
  再帰走査（深さ上限 6、`MAX_SCAN_DEPTH`）にした。**影響**: 深さ上限はシンボリックリンクの
  循環対策を兼ねる（計画どおり、追加のループ検出ロジックは実装しない）。`path.is_dir()`
  （symlink を辿る版）を使っているため、シンボリックリンクで別ディレクトリを指している
  フォントディレクトリ（一部ディストリの慣習）も走査対象になる。
- **判断**: レジストリが 0 件のとき、走査した全ディレクトリを列挙した `log::warn!` を出す
  ようにした（`scan_dirs`）。**理由**: 従来は「フォントが見つからない→黙って文字が消える」
  状態だったため、原因調査の起点になるログを用意した。**影響**: ログを出すだけで動作は
  変えない（0 件でも従来どおり空レジストリを返し、呼び出し側 `TextEngine` がフォールバック
  処理を続ける）。

### 2. ゲームローカルフォント（本命機能）

- **判断**: `FontKey` の実体参照を `path: PathBuf` 単一フィールドから
  `source: FontSource`（`Path(PathBuf)` / `Bytes(ArcBytes)` の enum）へ変更した。
  `ArcBytes` は `Arc<[u8]>` を包み、`Eq`/`Hash` は内容比較ではなく `Arc::ptr_eq`/ポインタ値に
  よる同一性比較にした。**理由**: レジストリが唯一のバイト列保有元で、`FontKey` はそこから
  複製されたものだけが流通する設計のため、同一性比較で必要十分（内容比較は同じ数百 KB〜
  数 MB のフォントデータを毎回比較するコストが無駄）。**影響**: W6 の暗号化 pak 読込時、
  復号後のバイト列をファイルへ書き出さずそのまま `FontSource::Bytes` として登録できる
  （計画で言及されていた設計を先取りした形）。
- **判断**: `TextEngine` に `registry: &'static FontRegistry`（システム、従来どおり）とは別に
  `game_registry: FontRegistry`（ゲームローカル、既定は空、インスタンスごとに保持）を追加した。
  「1 つの巨大なマージ済みレジストリを作る」方式ではなく「2 つを順に引く」方式にした。
  **理由**: システムフォント側は起動時に 1 回だけ走査する `&'static` 共有インスタンス
  （`registry::global()`）のままにしたかった（ゲームごとに再走査させたくない）。2 つを
  順に引くだけなら、ゲームフォント 0 件（`fonts/` 無し）の場合に `game_registry` への
  問い合わせが即 `None`/空を返すだけになり、**Windows の既存経路（登録済みシステム
  レジストリのみを参照する分岐）をバイト単位で完全に素通りさせられる**（実際に
  `text::tests::without_game_fonts_behavior_is_unchanged` で回帰確認、かつ既存 268 テストが
  無変更で全通過することでも裏付けた）。**影響**: `resolve_base`/`glyph_for_char` の優先順位は
  「要求 family をゲーム→システムの順で厳密一致 → ゲームフォント先頭（登録順）→ OS 別
  FALLBACK_CHAIN → 全登録フォント走査（ゲーム→システムの順）」。計画の「ゲームフォント
  （登録順の先頭）→ 既存 OS 名チェーン → 全登録フォント走査」という記述を、
  「厳密一致は常に最優先」という既存ロジックの構造を保ったまま書き下した形になっている。
- **判断**: `GameDir::list_dir(rel)` を新設し、ディレクトリが存在しない場合は `Err` ではなく
  空 `Vec` を返す設計にした。**理由**: `fonts/` はゲームにとって完全に任意（同梱しなくても
  従来どおり動く）ため、「無い」を異常系として扱いたくなかった。`read_bytes(rel)` も
  新設（`read_text` は UTF-8 検証を要求するため TTF バイナリには使えない）。**影響**:
  `EngineState::new` 内の `load_game_font_bytes` は `list_dir("fonts")` → 拡張子
  `.ttf`/`.ttc` のみフィルタ → `read_bytes` の順で収集し、個別ファイルの読込失敗は
  その 1 ファイルだけ `warn` ログを出してスキップする（システムフォント走査の
  「1 ファイル失敗でも継続」という既存方針と揃えた）。

### 3. EngineState 初期化の一本化

- **判断**: `EngineState` を V8 (`script/mod.rs`) と tsint (`tsint_engine.rs`) の両方が
  素の構造体リテラルで組み立てていたのを、`EngineState::new(...)` コンストラクタへ集約した
  （テストヘルパー `engine_state.rs::tests::make_engine_state` も同様に変更）。**理由**:
  「ゲームフォント読込を両エンジン共通の 1 箇所で行う」というウェーブ指示を、
  コピペ 2 箇所ではなく構造的に 1 箇所（コンストラクタ内部）に強制する形で満たすため。
  **影響**: フィールドは従来どおり全て `pub`（他モジュールからの直接アクセスは維持）。
  呼び出し側の引数の並びは元の構造体リテラルのフィールド順をそのまま踏襲した。

### 4. text.rs の FALLBACK_CHAIN per-OS 化

- **判断**: `FALLBACK_CHAIN` を `#[cfg(target_os = "windows")]`（従来の 4 種）と
  `#[cfg(not(target_os = "windows"))]`（`Noto Sans CJK JP` / `Noto Sans JP` / `TakaoGothic`
  / `IPAGothic` / `VL Gothic` の 5 種）に分岐させた。**理由**: 選定はウェーブ指示に明記された
  候補をそのまま採用（Debian/Ubuntu 系は `fonts-noto-cjk` で Noto、Fedora 系は
  `google-noto-cjk-fonts`、Takao/IPA/VL Gothic は主要ディストリのレポジトリに広く存在する
  伝統的な日本語フォントパッケージ）。実機検証は WSL2 Ubuntu で sudo が非対話実行できない
  環境だったため、実際にインストールしての確認はできていない（下記「WSL 環境」参照。
  ゲームローカルフォント経由で日本語描画自体は確認済み）。**影響**: family 名マッチングの
  大文字小文字・空白の扱い（`normalize()` が trim + ASCII 大文字化）は変更していない。

### 5. WSL 環境（検証時点の実情）

- WSL2 Ubuntu（`rustc`/`cargo` 1.95 導入済み）は `sudo` にパスワードが必要で非対話実行できず、
  `apt install fonts-noto-cjk` は自動化できなかった（`apt-cache policy` ではパッケージ自体は
  取得可能なことを確認済み）。`/usr/share/fonts` 配下に CJK フォントは 1 つも無かった
  （DejaVu/Lato/FontAwesome 等の非 CJK フォントが約 49 個）。
- 計画に明記された代替手順（`/mnt/c/Windows/Fonts` から日本語フォントを一時的に
  `games/sample/fonts/` へローカルコピーして検証）を採用し、`YuGothM.ttc` で
  `--autotest` の日本語描画・Windows/WSL 同一フォント時の PNG バイト比較を行った
  （下記「検証結果」参照）。**検証後に削除済み**（`games/sample/fonts/` はリポジトリに
  コミットしない一時ファイル。ライセンス上の再配布不可のため）。この手順は同時に
  ゲームローカルフォント機能そのものの実地検証にもなっている（システムに CJK フォントが
  皆無の環境で、ゲームフォント同梱だけで日本語描画が成立することを証明する形）。
- **追加対応**: `cargo test`（`tag_harness.rs` 経由のリンククリック系タグ振る舞いテスト）は
  ゲームディレクトリ非依存の使い捨て一時ディレクトリを使うため、上記の
  `games/sample/fonts/` 経由の対策が効かず、CJK フォント皆無の WSL では
  `text_rects` が常に空になりリンクへのクリック座標が計算できずタイムアウトで失敗した
  （3 件）。sudo 不要でシステムフォント側に恒久的に反映できる `~/.fonts`（W1 で追加した
  Linux 走査対象ディレクトリの 1 つ、`directories::BaseDirs` 経由で解決）へ
  `YuGothM.ttc`/`msgothic.ttc` を配置することで解決した（コードやテストは一切変更していない
  ―― 「ユーザーの WSL 環境にシステムフォントが無い」という環境側の欠落を、まさに W1 が
  実装した非 root フォールバックパスで補った形）。**この 2 ファイルはリポジトリ外
  （`~/.fonts`）に置いたままなので noiria リポジトリには一切影響しない**が、恒久的に
  WSL 環境に残る変更として報告に明記する（要否はユーザー判断、削除する場合は
  `rm -rf ~/.fonts` で良い）。
- **副産物の発見（W1 スコープ外だが cargo test 全通過に必須のため最小修正）**:
  `assets::tests::path_jail` が WSL で失敗した。原因は `dir.resolve("C:\\Windows\\system.ini")`
  が Windows でのみ意味を持つ絶対パス脱出経路（`Path::is_absolute()` はドライブレター付きの
  時だけ true）で、Linux では `\` がただの文字なので単一の `Component::Normal` として
  ルート配下の正当なファイル名に解決されてしまうため（`GameDir::resolve` 自体は変更して
  いない。`..`/`/` によるジェイルブレイク防止はどちらの OS でも変わらず機能する）。
  この 1 アサーションだけ `#[cfg(target_os = "windows")]` を付けた
  （`crates/noiria-core/src/assets.rs`）。**判断根拠**: 「Windows 専用テストは cfg
  スキップで可」という検証方針に合致する（このアサーション自体が Windows 固有の攻撃面を
  検証するものであり、セキュリティ上の後退ではない）。W1 が「初めて Linux で
  `cargo test` を実走させるウェーブ」であるため、この種の暗黙の Windows 依存が今回
  初めて顕在化した。

### 検証結果 (2026-07-12, W1)

- Windows: `cargo test`（ワークスペース）268 → **278 件全通過**（noiria-core 181→191、
  新規 10 件: `font::registry::tests::{scan_dirs_recurses_into_nested_directories,
  scan_dirs_with_no_hits_returns_empty_without_panicking, from_game_font_bytes_indexes_in_memory_font,
  from_game_font_bytes_indexes_all_fonts_in_ttc}`、`assets::tests::{list_dir_lists_files_sorted_and_ignores_subdirs,
  list_dir_missing_directory_returns_empty_not_error, read_bytes_reads_binary_content}`、
  `text::tests::{game_font_resolves_by_unique_family_name_before_any_fallback,
  game_font_is_used_as_ultimate_fallback_even_without_family_name_match,
  without_game_fonts_behavior_is_unchanged}`）。既存 268 件は無変更・全通過。
  `cargo clippy -p noiria-core -p noiria-app -p noiria-tools --all-targets` は変更ファイルに
  新規 warning 0 件（`std::slice::from_ref` 指摘のみ自己修正済み）。
- `cargo run -p noiria-app -- games/sample --autotest 24`: EXIT=0、16 枚保存。
  ゲームフォント無し状態での `frame_00.png` は、コード変更前に取得したベースラインと
  **バイト完全一致**（`game_registry` が空のときシステムレジストリのみを参照する経路が
  完全に素通りする設計どおり）。
- WSL2 Ubuntu: `CARGO_TARGET_DIR=$HOME/.noiria-target cargo test --workspace` 277 件全通過
  （0 failed。Windows の 278 との差 1 件は `win32.rs` の `#[cfg(windows)]` テストが
  Linux ビルドでは定義自体されないため、スキップというより「該当なし」）。
  `--autotest 24 --autotest-out $HOME/noiria-autotest`: EXIT=0、16 枚保存、**日本語
  （「はじめから」「つづきから（ロード）」等）が正しく描画された PNG を確認**
  （目視確認用に `target/wsl-autotest-copy/` へコピー。ユーザー検収用に一時保存、
  削除するかは検収後の判断）。
- 同一フォント（`YuGothM.ttc`）を `games/sample/fonts/` に同梱した状態での Windows/WSL
  PNG バイナリ比較: 16 枚中 13 枚（`frame_00`/`frame_05` 含む、テキストを含む主要フレーム）が
  **バイト完全一致**。残り 3 枚（`frame_03/09/10_hover`）はサイズ同一でバイトが一部相違
  （目視では同一シーンでテキストを含まないフレーム）。原因はホバーアニメーションが
  実時刻駆動で、Windows/WSL 間のプロセス起動・スケジューリングの微小なタイミング差により
  アニメーション位相がわずかにずれたためと推測（フォント/描画パイプライン自体の差では
  ないと判断）。計画どおり「計測であり受け入れ基準ではない」項目として記録する。

## 2026-07-12 配布対応 W2: X11 ウィンドウ + 入力（ワイヤプロトコル自作）

対象: 新規 `crates/noiria-app/src/x11/{mod,conn,proto,keymap,window}.rs`（+
`x11/window/tests.rs`、cfg(unix)）、`main.rs`（windowed 分岐を Windows/Unix/その他の
3 段に）、`scripts/wsl-test.sh`（Xvfb ブートストラップ + X11 テスト組み込み）。
承認済み計画 `breezy-sauteeing-fox.md` の W2 節に従う。ベースラインは Windows
`cargo test` 278 件 / WSL 277 件全通過。libX11/xcb には一切リンクせず、Unix
ドメインソケット上で X11 コアプロトコルを直接実装した。

### 1. 接続確立（conn.rs）

- **判断**: DISPLAY のバイトオーダー指定は常に `'l'`（LSBFirst）で固定送出する。
  **理由**: 自分から出す全リクエストは自分で LE エンコードするので、接続時に
  LSBFirst を要求しておけばサーバは常にリプライ/イベントを LE で返してくる
  （X11 プロトコルの規定）。これにより `conn.rs`/`proto.rs` の全パース処理を
  `from_le_bytes` 一本に統一でき、ネイティブバイトオーダー分岐が一切不要になる。
  **影響**: サーバ自身の image-byte-order（PutImage/GetImage のピクセルデータの
  並び）だけは接続バイトオーダーと独立した別概念のため、これは別途 Setup 応答から
  読み取って検証している（後述）。
- **判断**: `image-byte-order` フィールドが LSBFirst(0) でなければ起動時に
  明確エラーで落とす（`Setup::handshake`）。**理由**: `Pixmap::to_bgrx_u32` は
  `u32` を `to_le_bytes()` してそのまま PutImage に流す設計なので、サーバ側が
  MSBFirst を要求する構成だとバイトスワップが必要になるが、計画で明記された通り
  スウィズル対応は初版スコープ外。**影響**: 実機で確認した Xvfb/Xorg（x86_64）は
  いずれも LSBFirst を返すため、WSL/一般的な Linux デスクトップでは問題にならない。
- **判断**: TrueColor visual の探索は **screen 0 のみ**を対象にし、
  `class==TrueColor && (depth==24||depth==32) && red_mask==0x00FF0000` に一致する
  最初の visual を採用する。マルチスクリーン環境の screen 1 以降は非対応（`root`
  も screen 0 のものを使う）。**理由**: ゲームは単一ウィンドウ・単一スクリーンの
  想定で、マルチスクリーン対応は計画のスコープに無い。**影響**: 万一 screen 0 に
  適合する visual が無ければ明確エラーで落ちる（想定外構成として扱う）。
- **判断**: Xauthority 検索はホスト名一致判定に `$HOSTNAME` →
  `/proc/sys/kernel/hostname` の順で解決した素朴な文字列一致のみ。
  `directories` クレート等は使わず `$HOME/.Xauthority` を直接組み立てる
  （`noiria-app` に新規依存を増やしたくなかったため、`noiria-core` は
  `directories` に依存済みだが `noiria-app` は依存していない）。**影響**:
  IPv6 アドレス形式のホスト名比較など高度なケースは非対応（FamilyLocal /
  FamilyWild のみ扱う最小実装）。Xvfb は `-ac`（アクセス制御無効）で起動するため
  auth 情報が無くても接続できることを検証で確認済み（空 auth フォールバックの
  実地検証を兼ねる）。

### 2. リクエスト/リプライ（proto.rs）

- **判断**: resource-id の割り当ては `base | (counter & mask)` という
  「mask は連続下位ビットである」前提の最小実装にした（ビットを 1 個ずつ mask の
  立っているビット位置へ詰めていく一般化した実装は見送った）。**理由**: 実在する
  X サーバ（Xorg/Xvfb 含め）は例外なく resource-id-mask が連続下位ビットであり、
  非連続 mask は仕様上許容されているだけで実装が存在しない。計画の「sequence
  number 照合は最小実装」という許容方針と同じ精神で、resource-id 割り当ても
  同水準の最小実装に揃えた。**影響**: 理論上非連続 mask のサーバには対応しない
  （実害なし、Xvfb で実証済み）。
- **判断**: sequence number 照合は「送信ごとに単調増加するカウンタ」を
  `XConn` に持たせ、セットアップフェーズ（`send_request_and_wait`）でのみ
  リプライを同期的に待つ。セットアップ完了後（リーダースレッド起動後）は
  リプライを一切期待しないリクエストのみ送る設計にしたため、`reader_loop` は
  Reply パケットが来ても警告ログを出すだけで実害が無いようにした。**理由**:
  計画の「最小実装」方針、かつ `run_windowed` の定常状態では GetImage/InternAtom
  等の同期応答が要るリクエストを一切発行しない（PutImage/ChangeProperty/
  MapWindow はいずれも no-reply）ため、フル非同期の複雑な sequence 待ち行列は
  不要と判断。**影響**: GetImage/InternAtom/QueryTree はテスト専用（後述）で、
  いずれもリーダースレッド起動前の独立した接続から使うか、セットアップ
  フェーズの同期パスのみで完結させている。
- **判断**: エラーパケット（type=0）の扱いを **フェーズで分岐**させた。
  セットアップフェーズ（`send_request_and_wait`）中のエラーは致命的として
  `bail!` する（接続直後の CreateWindow 失敗等はリカバリ不能と判断）。
  リーダースレッド稼働後（定常状態）のエラーは `log::warn!` して継続する
  （計画の「ログして継続 or 致命扱いの判断を実装時に」に対する回答）。
  **理由**: セットアップ中のエラーは「ウィンドウを作れない」等アプリ続行が
  無意味な状況である一方、定常状態でのエラー（例えば描画範囲が一瞬 drawable の
  外に出るような際どい PutImage 呼び出し等）でウィンドウを即終了させるのは
  ユーザー体験上過剰と判断した。**影響**: 定常状態のエラーは info ログに
  残るのみで実行は継続する。
- **判断**: PutImage の帯分割行数は `(max_request_bytes - 24 - 64) / (width*4)`
  を実行時に計算し、`1280` 幅なら概ね 51 行（安全マージン 64B 込み）になる。
  固定値ではなく `Setup::max_request_length` から動的計算する設計にした。
  **理由**: max-request-length はサーバ実装依存（Xvfb の既定は 65535 ワード
  =262,140B だが将来変わりうる）であり、固定値だと想定外に小さいサーバで
  オーバーフローする。**影響**: 帯境界を跨ぐテスト画像（100 行）で
  PutImage→GetImage のピクセル完全一致を確認済み（後述）。
- **判断**: GetImage/QueryTree/SendEvent とそれらが使うオペコード定数
  （`OP_GET_IMAGE`/`OP_QUERY_TREE`/`OP_SEND_EVENT`）、および関連関数
  （`req_get_image`/`parse_get_image_reply`/`get_image_pixels`/`req_query_tree`/
  `parse_query_tree_children`/`req_send_event`/`build_client_message32`）は
  すべて `#[cfg(test)]` にした。**理由**: `run_windowed` の定常状態は
  PutImage で書くだけで一切読み返さない設計（計画も SendEvent を「テスト用」と
  明記）。`cargo test` はテスト用に `CARGO_BIN_EXE_noiria` を提供するため
  **プレーン（非テスト）ビルドも自動的に走る**が、そちらのビルドではこれらの
  関数は本当に未使用になり `dead_code` 警告が出る。テスト専用ヘルパーである
  という実態をそのまま `#[cfg(test)]` で表現するのが最小の対応と判断した。
  **影響**: QueryTree（opcode 15）は計画の必須オペコード一覧に無い追加分。
  ④の SendEvent クローズテストと⑤のスクリーンショット取得で「別クライアント
  接続から run_windowed が作ったウィンドウを見つける」ために追加した
  （目的は明確、実装は 2 関数のみの小さな追加）。

### 3. keymap.rs

- **判断（計画テキストとの差異、明示）**: 計画は矢印 keysym の範囲として
  `0xFF51〜0xFF54`（Left/Up/Right/Down）を挙げているが、**参照実装である
  win32.rs 自体は `VK_LEFT`/`VK_RIGHT` を一切ハンドルしていない**
  （`VK_UP`/`VK_DOWN` のみ、win32.rs の `WM_KEYDOWN` 分岐参照）。ウェーブ指示は
  「keysym→InputEvent.key の変換は win32.rs と同じキー集合のみ」ともあり、
  より具体的なこちらの指示を優先して **ArrowLeft/ArrowRight は生成しない**
  実装にした。**理由**: 「win32.rs と完全に同じキー集合」という上位の制約と
  「0xFF51-54 の範囲」という計画の例示が矛盾する場合、win32.rs 実体との
  厳密な互換性を優先すべきと判断（CLAUDE.md の「複数の解釈があれば明示する」
  方針に基づき、ここに記録する）。**影響**: `to_input_key(0xFF51)`
  （Left）/`to_input_key(0xFF53)`（Right）は共に `None` を返す
  （`to_input_key_special_set_matches_win32` テストで固定化）。
- **判断**: シフト列（column 1）の keysym が NoSymbol(0) の場合は column 0 へ
  自動フォールバックする（`Keymap::keysym`）。**理由**: X11 の慣例
  （多くのキーがシフト付き keysym を持たない）に合わせないと、Shift+矢印等で
  keysym が 0 になり `to_input_key` が `None` を返してキー入力が消える。
  **影響**: フィクスチャテストで検証済み（keycode 9 col1→Return
  にフォールバック）。

### 4. window.rs / イベントループ

- **判断**: `WindowedParams` は win32.rs の `WindowedParams` と同型だが、
  計画の指示通り `hwnd_slot` を持たない。`WakeSink::Channel` をそのまま
  使うため起床用のウィンドウハンドルスロットが不要（main.rs の Unix 分岐は
  `WakeSink::Win32` ではなく `WakeSink::Channel(user_tx)` を渡す）。
- **判断**: ウィンドウは常に `game_w × game_h` ぴったりのサイズで作成し、
  クライアント座標=ゲーム座標として扱う（win32.rs の `to_game_coords` に
  相当するスケーリングを行わない）。**理由**: `WM_NORMAL_HINTS`
  （min==max）でリサイズ不可にしているため、正常系では常に 1:1 になる。
  win32.rs は `StretchDIBits` でクライアントサイズへ拡縮する設計だが、
  X11 版はリサイズが起きない前提なので拡縮ロジック自体を持たない
  （シンプルさ優先、CLAUDE.md 方針）。**影響**: 万一 window manager が
  ヒントを無視してリサイズした場合、画像は左上 game_w×game_h 部分のみ
  描画され残りは黒のままになる（Xvfb と主要 WM では発生しないことを実地
  確認、既知の制限として許容）。
- **判断**: メインループの待機は `crossbeam_channel::Select::ready_timeout`
  を使い、`recv()` ではなく `ready()` 系 API で「消費せずに待つだけ」にした。
  **理由**: 素朴に `select! { recv(x_rx) -> _ => {}, ... }` を使うと、
  待機時にメッセージを実際に受信・破棄してしまい、直後のドレインループが
  拾えず入力を取りこぼす。win32.rs の `MsgWaitForMultipleObjects`
  （メッセージを消費しない、起床するだけ）と同じ意味論にするため
  `ready_timeout` を選んだ。**影響**: ループ構造は「X イベントドレイン →
  UserEvent ドレイン → 合成/present → 次イベントまで待機」で win32.rs と
  完全に一致させている。
- **判断**: ループ終了時に `stream.shutdown(Both)` してから
  `reader_handle.join()` する。**理由**: リーダースレッドは
  `read_exact`（ブロッキング）でループしているため、明示的にソケットを
  shutdown しないと `run_windowed` から戻った後もスレッドが残り続ける
  （同一プロセス内で `run_windowed` を複数回呼ぶテストで特に問題になる）。
  **影響**: テストプロセスでスレッドリークが起きないことを実地確認
  （`client_message_wm_delete_window_quits_run_windowed` 等を連続実行しても
  ハングしない）。

### 5. WSL/Xvfb 検証環境の構築（実装以上に手間取った部分）

- **障壁 1: sudo が非対話で使えない**（W1 と同じ既知の制約）。
  `sudo -n apt-get install xvfb` は失敗する。
- **障壁 2（新規発見）: WSLg の `/tmp/.X11-unix` は読み取り専用 tmpfs**。
  `mount | grep X11-unix` で `none on /tmp/.X11-unix type tmpfs (ro,relatime)`
  と判明。WSLg が実デスクトップ (`:0`) 用にこのディレクトリを管理しており、
  一般ユーザーには書き込み権限が無い（新しいソケットファイルを作成できない
  ＝ `Xvfb` がリスンできない）。これは同時に「ユーザーの実デスクトップを
  誤って汚染できない」という安全設計にもなっている。
  **対応**: root 不要の非特権ユーザー名前空間（`unshare --user --mount
  --map-root-user`）内で `/tmp/.X11-unix` を書き込み可能な tmpfs へ
  再マウントすることで解決した。この名前空間は完全に隔離されており、
  ホスト側（実デスクトップの WSLg プロセス群を含む）からは一切見えない
  ＝ **制約「ユーザーのデスクトップに窓を出さない」を機構レベルで満たす**。
- **障壁 3: Xvfb が `/usr/bin/xkbcomp` をハードコードして `execl` する**。
  `x11-xkb-utils`（xkbcomp 本体）がシステムに未インストールで、`PATH`
  を通しても解決されない（フルパス直呼びのため）。**対応**: 同じ非特権
  名前空間内で `/usr/bin` に対して `overlayfs`（lowerdir=/usr/bin,
  upperdir=xkbcomp を置いた一時ディレクトリ）をマウントし、既存の
  `/usr/bin/*` を一切壊さずに `xkbcomp` だけを追加した。WSL2 のカーネルが
  非特権 overlay マウントを許可していることを確認した上で採用（`unshare`
  越しの `mount -t overlay` が成功することを実地検証）。xkbcomp 自体は
  `libxkbfile.so.1`（`libxkbfile1` パッケージ）にも依存しており、これも
  同様に `apt-get download`（root 不要）で取得した。
- **Xvfb バイナリ自体の入手**: `apt-get download xvfb xserver-common
  libxfont2 x11-xkb-utils libfontenc1 libxkbfile1`（**root 不要**、
  パッケージマネージャへのインストールは行わず `.deb` を `dpkg-deb -x` で
  `$HOME/.noiria-xvfb-root` へ展開するだけ）で解決した。apt のネットワーク
  アクセス自体は許可されている環境だったため、この経路が機能した
  （完全オフライン環境では別途 .deb の持ち込みが必要、`wsl-test.sh` の
  エラーメッセージに手順を明記した）。
- **障壁 4（新規発見）: `/mnt/c` 上のスクリプトをシェバン経由で直接実行すると
  失敗することがある**。`wsl -e bash -lc "/mnt/c/.../wsl-test.sh"`
  （＝シェバン `#!/usr/bin/env bash` によるカーネルの直接実行）が
  `bash: line 1: C:/Program: No such file or directory` という不可解な
  エラーで失敗する事象を発見した（DrvFs 上のシェバン実行と WSL の Windows
  PATH interop が絡む相性問題と推測、根本原因の完全な特定はできず）。
  `bash /mnt/c/.../wsl-test.sh`（bash を明示的に起動してスクリプトを渡す）
  では常に成功することを複数回の実行で確認した。**対応**:
  `scripts/wsl-test.sh` の使い方コメントと本ドキュメントの両方を
  「必ず `bash wsl-test.sh` の形で呼ぶこと」に更新した。既存の
  `docs/linux-dev.md` も同様に更新した（W1 時点の記述は旧来のシェバン直接
  実行の形だった）。
- **`scripts/wsl-test.sh` への組み込み**: 上記全てを自動化し、
  `[3/3] X11 (Xvfb) テスト` ステップとして追加した。Xvfb 確保
  （システム→sudo→vendor の 3 段フォールバック）→ 非特権名前空間内で
  `/tmp/.X11-unix` 再マウント + xkbcomp overlay → Xvfb 起動 →
  `NOIRIA_X11_TEST_DISPLAY=:99` で `cargo test -p noiria-app --bin noiria
  x11:: -- --test-threads=1` を実行、という一連の流れを 1 コマンドで
  再現可能にした。**直列実行が必須な理由**: 複数のテストが同一 Xvfb
  上でウィンドウを生成・スタッキングし、`QueryTree` の「最後の子 =
  最新のウィンドウ」という前提で対象ウィンドウを見つけるため、並列実行だと
  レースする（`--test-threads=1` で回避、通常の `cargo test --workspace`
  ステップは並列のままで問題ない。X11 系テストは env 未設定なら即 skip
  するため他のテストと同時に走っても安全）。Xvfb が用意できない環境では
  ステップ 3 のみ skip し、`--skip-x11` オプションも用意した。

### 検証結果 (2026-07-12, W2)

- Windows: `cargo test`（ワークスペース）278 → **278 件全通過**（x11 モジュールは
  `cfg(unix)` のため Windows ビルドには一切含まれず、テスト数は不変であることを
  確認）。`cargo clippy -p noiria-app --all-targets`: 変更ファイルに新規 warning
  0 件（`too_many_arguments`(`put_image_banded`)/`manual_range_patterns` を
  検出し修正済み）。既存 warning は script/mod.rs・script/loader.rs の
  無関係な既存コード（`items_after_test_module`、本ウェーブ以前から存在）のみ。
- WSL: `bash scripts/wsl-test.sh` を **クリーンな状態**（Xvfb・xkbcomp・
  vendor キャッシュを一切持たない状態）から実行し、全ステップ通過を確認。
  `cargo test --workspace`: 277 → **289 件全通過**（noiria-app が
  x11 モジュールの新規ユニットテスト 12 件を獲得: `conn::tests` 5 件
  （DISPLAY 解析）、`keymap::tests` 3 件（keysym 変換 + ワイヤバイト列
  フィクスチャ）、`window::tests` 4 件（Xvfb 検証、下記）。289-277=12 で
  過不足なく一致、既存テストの破壊なし）。
- WSL/Xvfb 上の X11 テスト（`NOIRIA_X11_TEST_DISPLAY=:99`、`--test-threads=1`）
  4 件全通過:
  - `handshake_reports_sane_visual_and_max_request_length`（①）:
    depth 24/32・max_request_length>=4096・root!=0・keycode 範囲の妥当性を assert。
  - `put_image_then_get_image_roundtrip_across_band_boundary`（②）:
    1280×100（帯境界を跨ぐ高さ）の座標依存パターン画像を PutImage → GetImage
    読み戻しで **ピクセル完全一致**を確認。
  - `client_message_wm_delete_window_quits_run_windowed`（④）: 別接続から
    QueryTree でウィンドウを発見 → SendEvent で WM_DELETE_WINDOW
    ClientMessage を自送 → `run_windowed` がクリーンに `Ok(())` で返ることを
    確認（win32.rs の `invisible_window_input_roundtrip_and_clean_quit` と
    同じ「別スレッドから叩いて join」パターン）。
  - `games_sample_smoke_with_screenshot`（⑤、③のキーマップ固定バイト列
    テストは Xvfb 不要のため上記 289 件に含まれる通常テスト）:
    実バイナリ（`CARGO_BIN_EXE_noiria`）を `games/sample` でサブプロセス
    起動し、日本語タイトル「Noiria サンプル」のウィンドウが Xvfb 上に
    現れることを QueryTree で確認、2 秒後に GetImage でスクリーンショットを
    撮影して `target/x11-smoke/games_sample_screenshot.png` へ保存、
    その後 8 秒間（合計 10 秒相当）クラッシュしないことを確認してから
    WM_DELETE_WINDOW で正常終了させた。**目視確認済み**: タイトル画面
    （空・ビル群・太陽の背景、「はじめから」「つづきから（ロード）」の
    日本語選択肢、メッセージウィンドウ枠）が正しい色（オレンジ〜紫の夕焼け、
    B/R チャンネル入れ替わりなし）で描画されている。色チャンネル検証にも
    なっている（red-mask==0xFF0000 の起動時 assert が実際に正しいレイアウトを
    選んでいることの実地証明）。
- ヘッドレス回帰: 両 OS で `--autotest 24` EXIT=0（Windows/WSL とも変更前と
  同じログ順序・保存枚数 16 枚、main.rs のヘッドレス分岐は無変更のため当然
  だが実地確認した）。
- WSL 環境に加えた変更: `$HOME/.noiria-xvfb-root`・`$HOME/.noiria-xvfb-debs`・
  `$HOME/.noiria-xkb-upper`・`$HOME/.noiria-xkb-work`（いずれも
  `scripts/wsl-test.sh` が自動生成する vendor キャッシュ、apt へのインストール
  操作は一切行っていない。削除しても次回実行時に再取得されるだけで実害なし）。
  `apt-get download`（root 不要のダウンロードのみ、インストールはしていない）
  を計 6 パッケージ分実行した。

## 2026-07-12 配布対応 W3: ALSA 音声出力（dlopen FFI）

対象: `crates/noiria-core/src/alsa_out.rs`（新規）+ `audio.rs` の cfg 共通化。
M1（Linux で開発版フル動作: 窓・入力・音・セーブ）の最終ピース。

### 設計判断（判断→理由→影響）

- **判断**: dlopen/dlsym/dlerror 自体は `libc`/`libloading` 等のクレートを
  追加せず、`extern "C" { fn dlopen(...); ... }` を直接宣言した（型は
  `std::os::raw::{c_char,c_int,c_long,c_uint,c_ulong,c_void}` のみ使用、
  追加クレート不要）。ただし dlopen/dlsym 自体のシンボル解決のために
  `crates/noiria-core/build.rs` を新設し、`CARGO_CFG_TARGET_OS=="linux" &&
  CARGO_CFG_TARGET_ENV=="gnu"` の時のみ `cargo:rustc-link-lib=dl` を出す
  ようにした。**理由**: glibc 2.34 未満では dlopen 等が libc.so ではなく
  libdl.so 側にあり、`-ldl` を明示的にリンクしないとリンクエラーになる
  環境がありうる。2.34 以降（今回検証した Ubuntu 24.04 の glibc 含む）は
  libc に統合済みで libdl.so.2 は後方互換の空スタブとして残るため、
  `-ldl` を足しても実害はない。**musl/macOS は対象外**とした（ALSA を
  使う環境ではないため、必要になれば個別に判断する）。**影響**:
  `libasound` へのリンク時依存は一切生まれない（`-ldl` は dlopen API
  自体へのリンクであり、`libasound.so.2` は実行時に dlopen で探すだけ）。
  実際に WSL (Ubuntu 24.04, rustc 1.95.0) でビルド・リンクが通ることを
  確認済み。
- **判断**: ALSA シンボルは全て opaque ポインタ運用（`enum SndPcmT {}` /
  `enum SndPcmHwParamsT {}` の中身を見ない未定義型）で扱い、構造体
  レイアウトには一切依存しない設計にした（計画通り）。`dlsym` の戻り値
  `*mut c_void` を関数ポインタ型へ変換する際は `transmute_copy`
  （`transmute` と違いコンパイル時サイズ一致チェックが無いぶん、
  `debug_assert_eq!(size_of::<T>(), size_of::<*mut c_void>())` を手動で
  添えた）を使った。**理由**: 関数ポインタとデータポインタは全プラット
  フォームでサイズが一致するため安全に変換できるが、ジェネリック `T`
  だと `transmute` のコンパイル時チェックが効かないため。**影響**: なし
  （実行時 assert が唯一の安全網だが、全 16 シンボルとも通常の関数
  ポインタ型なので理論上失敗しえない）。
- **判断**: フォーマット交渉は計画通り **FLOAT_LE 優先 → 失敗時のみ
  S16_LE にフォールバック**とし、`wasapi_out.rs` の `DeviceFormat`
  分岐と全く同じ構造（列挙型 + `write_device_buffer` 相当の変換）を
  踏襲した。channels は `set_channels_near(2)`、rate は
  `set_rate_near(48000)` で「近傍」交渉にした（ALSA の `plug:` 系
  プラグインが挟まる `default`/PulseAudio ブリッジ環境では厳密一致
  よりも近傍交渉の方が失敗しにくいため）。実際に交渉された値は
  `snd_pcm_hw_params_get_channels`/`get_rate` で読み戻し、`channels()`/
  `sample_rate()` として公開する（wasapi_out.rs の `GetMixFormat` 読み戻し
  と同じ思想）。**影響**: WSL + vendored libasound + null デバイスでの
  実地検証で `2ch 48000Hz`（要求通り）を確認済み。実機の `default`
  （PulseAudio/PipeWire ブリッジ）でどう negotiate されるかは未検証
  （ユーザー実施タスク、実音確認時に自然に分かる）。
- **判断**: アンダーラン (`-EPIPE`, errno 32) は `snd_pcm_prepare` して
  **同じチャンクを再送**（バッファ位置を進めない）、サスペンド
  (`-ESTRPIPE`, errno 86) は 20ms 待ってから同様に prepare+再送、
  それ以外の未知のエラーも prepare して再送を試みる方針にした。
  いずれも**連続失敗が 8 回を超えたらループを終了する**（`bail!` して
  `Result` の `Err` を返す。呼び出し元の `audio.rs` はこれを
  `log::error!` して描画スレッドを終了させるだけで、コマンド処理
  スレッド自体はクラッシュしない）。**理由**: 計画の「wasapi 側の
  寛容な方針に合わせる」を踏襲。wasapi_out.rs は `WaitForSingleObject`
  失敗時に無限リトライ寄りだが、ALSA の書き込みエラーは無限リトライ
  すると本物のデバイス断絶時にスレッドが延々スピンする恐れがあるため、
  上限付きにした（wasapi 版より一段保守的だが「無限リトライしない」
  という大枠の方針は共通）。**影響**: 通常運用（null/default 双方）では
  一度も上限に達しない想定。実機の USB オーディオ切断等の異常系は
  未検証（ユーザー実施タスクの範囲外、今回のスコープでは automatic
  fallback の存在のみ保証）。
- **判断**: `run()` 終了時は必ず `snd_pcm_drain` を呼んでからループを
  抜ける（`Drop` で `snd_pcm_close` は別途行う）。**理由**: バッファに
  溜まっている分（无音のことが多い）を吐き切ってからクローズする方が
  行儀が良い ALSA の作法に合わせた。**影響**: stop 直後の終了が
  `drain` の分だけわずかに遅延しうるが、キューは 512 フレーム
  （48kHz で ~10.7ms）単位なので体感できるレベルではない。
- **判断**: `audio.rs` の `run()`/`command_loop()`/`handle()` から
  `#[cfg(windows)]` を外し、**出力デバイス open の 1 箇所
  （新設 `run_render_thread()` 関数）だけ** に OS 分岐を集約した。
  `run_render_thread()` 自体は `#[cfg(windows)]`/`#[cfg(unix)]`/
  `#[cfg(not(any(windows,unix)))]` の 3 分岐を持つが、後者は即座に
  `init_tx` へ `Err` を送って return するだけなので、呼び出し元
  `run()` の「open 失敗時は無音続行」という**既存の分岐がそのまま
  未知プラットフォームのフォールバックにも使い回せる**設計にした
  （計画の「それ以外は既存ドレイン」を、新しい分岐を増やさずに
  実現できた）。**影響**: `run()`/`command_loop()`/`handle()` は
  Windows/Unix/その他のどの OS でも共通のコードパスになり、
  COM 用に用意されていた「描画専用スレッド」構造（open〜再生ループを
  1 スレッドに閉じ込める）は ALSA にも制約なく同型のまま流用された
  （ALSA 自体にはスレッド閉じ込め制約は無いが、構造を分ける理由もない
  ため素直に踏襲）。
- **判断**: 音声デバイス open 成功時に `log::info!("音声デバイス open
  成功: {channels}ch {sample_rate}Hz")` を新規追加した（従来は成功時に
  何もログを出さず、失敗時の `warn` のみだった）。**理由**: ALSA null
  デバイスでの統合スモーク検証において、ヘッドレス `--autotest` は
  オーディオスレッド自体を起動しないため、windowed 実行でのログが
  「音が鳴っている（鳴らせる状態になっている）」ことを確認できる
  唯一の証跡になる。**影響**: Windows (WASAPI) 側でも同じログが
  今後出るようになる（`wasapi_out.rs` 自体は無変更、ログ追加は
  `audio.rs` 側の共通コードのみ）。実害はなく、むしろ従来 silent
  だった成功パスに観測可能性が増えた。

### 検証結果 (2026-07-12, W3)

- Windows: `cargo test`（ワークスペース）278 → **278 件全通過**
  （`alsa_out.rs` は `#[cfg(unix)]` のため Windows ビルドには一切
  含まれず、テスト数は不変であることを確認）。`cargo build -p
  noiria-core` 成功（build.rs は Windows では何もしない no-op）。
  `cargo clippy --workspace --all-targets`: 変更ファイル
  （audio.rs / lib.rs / build.rs）に新規 warning 0 件。既存 warning は
  W1〜W4 時点と同じ無関係ファイル（noiria-tools 各所、
  script/mod.rs・loader.rs の `items_after_test_module`）のみ。
- WSL (Ubuntu 24.04, rustc 1.95.0): `cargo build -p noiria-core` 成功
  （dlopen 宣言・build.rs の `-ldl` リンクとも問題なし）。
  `cargo test --workspace`: **288 件全通過**（`noiria-core` に新規 2 件
  `alsa_out::tests::open_null_device_and_play_silence_briefly`
  `alsa_out::tests::open_default_with_nonexistent_device_name_fails_gracefully`
  を確認。libasound が無い状態では両方とも `eprintln!` で skip 相当に
  なることを最初に確認し、その後 `libasound2t64`+`libasound2-data` を
  vendoring した状態で**実際に ALSA null デバイスへ open→書き込み→
  close する完全な経路**が通ることも確認した、下記参照）。
  `cargo clippy -p noiria-core --all-targets`: 新規 warning 0 件
  （既存 4 件は video.rs/vorbis/codebook.rs/text.rs の無関係な既存
  コード）。**W2 終了時点の記録（`277 → 289`）との比較**: 今回の
  測定値 288 は素朴な期待値（289+2=291）と一致しない。内訳を精査した
  結果、`noiria-app` は x11 テスト 12 件 + 共通 61 件 = 73 件
  （Windows は win32.rs 専用テスト 1 件を含む 62 件、`noiria-core` は
  Windows 専用フォントテスト 2 件を含む 191 件、WSL は alsa_out 2 件を
  含む 190 件）であることを直接確認しており、**今回の diff（audio.rs
  の cfg 共通化 + alsa_out.rs 新設）が既存テストを 1 件も除去・無効化
  していないことはコード上明らか**（`#[cfg(windows)]` を外しただけで
  関数・テスト本体は無変更、削除した関数は無し）。289 という記録値との
  差分（-3 相当、alsa の +2 を差し引くと -5）の完全な原因特定はできな
  かった（過去セッションと今回とで rustc バージョンが異なる: W2 時点
  不明 → 今回 1.95.0。ワークスペース側の変更はしていないため考えにくい
  が、可能性としては記録)。**結論**: 回帰は無い（0 failed、diff の
  additive 性がコードレベルで保証されている）が、289 という過去の記録
  値そのものが今回の環境で再現できなかった点は申し送り事項とする。
- WSL: `NOIRIA_ALSA_DEVICE=null` の ALSA スモークを **libasound 不在**
  （skip 経路）と**libasound 実在**（vendoring: `apt-get download
  libasound2t64 libasound2-data` → `$HOME/.noiria-alsa-root` へ展開、
  `ALSA_CONFIG_PATH` で `alsa.conf` を明示指定）の**両方**で確認した。
  実在時は `open_null_device_and_play_silence_briefly` が実際に
  `snd_pcm_open("null")` → hw_params 交渉（2ch/48000Hz 取得）→
  512 フレーム×複数回の `snd_pcm_writei` → `snd_pcm_drain` →
  `snd_pcm_close` の完全な往復を 200ms 分行い成功した（実行時間
  0.20s、skip 時の 0.00s と明確に区別できる=実際に往復していることの
  傍証）。**発見**: `libasound.so.2` 単体（`libasound2`/`libasound2t64`
  パッケージ）だけでは `snd_pcm_open` が `Unknown PCM null` で失敗する
  ことを最初の試行で発見した。ALSA の名前付き PCM（"null"/"default" 等)
  は `/usr/share/alsa/alsa.conf`（`libasound2-data` パッケージ、
  ライブラリとは別パッケージ）で定義されており、これが無いとライブラリ
  自体は正常にロード・シンボル解決できてもデバイス名解決だけが失敗する
  という ALSA 特有の 2 段構成だった。**対応**: `scripts/wsl-test.sh` の
  vendoring 対象に `libasound2-data` を追加し、`ALSA_CONFIG_PATH`
  環境変数（ALSA 標準の設定ファイルパス上書き機構）でベンダ版
  `alsa.conf` を明示指定するようにした。
- WSL: **windowed + 音声の統合スモーク**（Xvfb `:99` 上で `games/sample`
  を `NOIRIA_ALSA_DEVICE=null` のまま `timeout 12` 秒実行、
  `scripts/wsl-test.sh` の `[4/4]` に組み込み済み）:

  ```text
  [...] INFO  noiria] ゲーム: Noiria サンプル (1280x720)
  [...] INFO  noiria] スクリプトエンジン: v8 (deno_core)
  [...] INFO  noiria_core::audio] 音声デバイス open 成功: 2ch 48000Hz
  [...] INFO  noiria::x11::window] X11: ウィンドウ作成 wid=0x00200000 (1280x720)
  [...] INFO  noiria::script::engine_state] [script] Noiria サンプルを起動します
  ```

  クラッシュ（panic）なし、ALSA アンダーランエラーのログなし。ログは
  `target/x11-smoke/games_sample_windowed_alsa_smoke.log` に保存される。
- WSL: **libasound はあるがデバイス名が存在しない経路**（追加の手動検証、
  `NOIRIA_ALSA_DEVICE=noiria_bogus_device_that_does_not_exist` で
  windowed 実行）: `ALSA lib pcm.c:2721:(snd_pcm_open_noupdate) Unknown
  PCM noiria_bogus_device_that_does_not_exist` の ALSA 側エラー出力の
  直後に `[...] WARN noiria_core::audio] オーディオデバイスを開けません
  （音なしで続行）` が出力され、その後スクリプトが正常に開始することを
  確認した（クラッシュなし、無音続行の設計どおり）。
- ヘッドレス回帰: WSL `--autotest 24` EXIT=0（16 枚保存、W2 時点と
  同じログ順序。オーディオスレッド自体を起動しない設計のため W3 の
  変更とは無関係だが確認した）。

### WSL 環境に加えた変更 (W3)

- `$HOME/.noiria-alsa-root`・`$HOME/.noiria-alsa-debs`（`scripts/
  wsl-test.sh` [3/4] が自動生成する vendor キャッシュ、apt への
  インストール操作は一切行っていない。削除しても次回実行時に
  再取得されるだけで実害なし）。`apt-get download libasound2t64
  libasound2-data` を実行した（`libasound2` は Ubuntu 24.04 では
  candidate 無しでエラーになり `libasound2t64` にフォールバックする
  ことを確認、スクリプトはこの両対応済み）。

### 申し送り

- **実音確認はユーザー実施タスク**: 本ウェーブの検証は ALSA null
  デバイス（何も鳴らさない）と、libasound 不在/デバイス名不正の
  フォールバック経路のみで、実際にスピーカーから音が出るかどうかは
  未確認（ユーザーの WSLg 環境で `cargo run -p noiria-app --
  games/sample` を実行して確認するタスクとして切り出す。既定の
  `NOIRIA_ALSA_DEVICE` 未指定時は `"default"` = 通常 PulseAudio/
  PipeWire の ALSA ブリッジ経由になる）。
- **M1 到達の宣言可否**: 「Linux で開発版フル動作（窓・入力・音・
  セーブ）」のコード面・自動検証面（ヘッドレス/Xvfb/ALSA null の
  いずれも exit 0・クラッシュなし・想定ログ）は本ウェーブで揃った。
  ただし計画表の検証体系まとめにある「実窓・実音の官能確認 = ユーザー
  実施タスク」がまだ未実施のため、**M1 の完全宣言はユーザーの実音・
  実窓確認待ち**とする（コード上の M1 到達条件はすべて満たしている）。
- WSL テスト総数の記録値（W2 時点 289）との差分が完全には説明しきれ
  なかった点（上記検証結果参照）。回帰の実害は無いと判断しているが、
  次ウェーブ以降で総数を記録する際はこの断絶を踏まえること。

## 2026-07-12 配布対応 W4: DEFLATE エンコーダ本実装 + PNG 行フィルタ

対象: 新規 `crates/noiria-core/src/codec/deflate.rs`（zlib/DEFLATE エンコーダ、
`inflate.rs` の対）、`crates/noiria-core/src/codec/png.rs`（行フィルタ選択 + 実圧縮）、
`crates/noiria-core/src/codec/mod.rs`（`pub mod deflate;` 追加）。承認済み計画
`breezy-sauteeing-fox.md` の W4 節に従う。ベースラインは Windows cargo test 278 件 /
WSL 288 件全通過。

### 1. LZ77 (窓 32KB・ハッシュチェーン・lazy matching)

- **判断**: 3 バイトハッシュ（15bit、`(h<<5)^byte` を 3 回、zlib と同じ更新式）+
  `head`/`prev` 配列によるチェーン探索。距離チェックは古い順にチェーンを辿るため
  `dist > WINDOW` に達したら即 break（32768 バイト超のエントリを無駄に辿らない）。
  最良一致位置の `best_len` 番目のバイトを先に比較してから全比較するプリチェック
  （zlib `longest_match` と同じ枝刈り）を入れた。**理由**: これが無いと 1 マッチ探索が
  O(MAX_CHAIN × MAX_MATCH) になり実測で 1280x720 相当が数百 ms かかった。
- **判断**: lazy matching の「先読みするか」の閾値と「チェーン探索を絞る」閾値を
  分離した: `NICE_LEN=128`（これ以上の一致が見つかったらチェーン探索そのものを
  打ち切る）、`GOOD_LEN=8`（これ以上の一致が見つかったら残り探索数を 1/4 に絞る）、
  `MAX_LAZY=16`（現在位置の一致長がこれ未満のときだけ pos+1 の先読み探索をする）。
  zlib の level 6 相当パラメータ (`good_length=8, max_lazy=16, nice_length=128,
  max_chain=128`) に合わせた。**理由**: 実装当初は「先読みするか」の判定にも
  `NICE_LEN`（128）を流用していたところ、実写類似の背景 PNG
  (`games/sample/assets/bg/room.png`, 1280x720) で 1 枚あたり release ビルドで
  145ms かかり、目安の 100ms を超過した。lazy 先読みは「一致長 16〜127」の
  ほぼ全ケースで発生してしまい、チェーン探索が実質 2 倍走っていたのが原因。
  `MAX_LAZY=16` に分離したところ room.png が 145ms→79ms、street.png が
  123ms→70ms に改善し、圧縮率の悪化は合計比で 4.3%→4.5%（stored 比）と軽微
  だった。**影響**: LZ77 のマッチ選択がわずかに保守的になる（zlib level 6 と
  同じ仕様なので想定内）。

### 2. ハフマン符号長構築を package-merge に切り替えた（重要な修正）

- **判断**: 計画では「頻度ソート + 深さ制限の単純調整（package-merge 不要）」
  としていたが、実装・検証の過程で **単純調整方式は degenerate な（不完全な）
  ハフマン木を生成しうる**ことが判明したため、package-merge (coin-collector)
  アルゴリズムに切り替えた。
- **発見の経緯**: 最初に zlib の `gen_bitlen` 相当（Okumura 方式のヒストグラム
  調整、bl_count のペア入れ替え）を実装したところ、深いフィボナッチ的頻度分布
  （境界ケーステスト用に作成）で Kraft 不等式 `sum(2^(max_length-l)) <= 2^max_length`
  を破る出力が出た（自前のユニットテストで検出）。次に「総和が上限を超える間
  最短符号を 1 段深くする」という単純な貪欲法に変更したところ、Kraft 不等式の
  等号は満たさなくなった（`<=` は満たすが `==` にならない = **不完全なハフマン
  木**）。**この不完全な木は、自作デコーダ (`inflate.rs`) の round-trip テストでは
  検出できなかった**（`Huffman::from_lengths` は `left>=0` のみ検査し、コード
  空間の余りを許容する寛容な実装のため、エンコーダ・デコーダが対で使う分には
  問題なく動く）。しかし実際に `--autotest` でスクリーンショット PNG を生成し
  `image` クレート（`png`/`miniz_oxide` 系、zlib 準拠の厳密なデコーダ）で読もう
  としたところ `Corrupt deflate stream. BadCodeLengthHuffmanTree` で拒否された。
  **教訓**: 自作 DEFLATE の正当性は自作デコーダとの round-trip だけでは検証
  しきれない（エンコーダ・デコーダの対称なバグは相殺されて見えなくなる）。
  外部の厳密な独立実装によるクロスチェックが必須。この教訓を
  `crates/noiria-core/src/codec/png.rs` の新規テスト
  `encode_generated_game_assets_readable_by_image_crate` のコメントに明記した。
- **対応**: `build_lengths()` を package-merge (coin-collector) で書き直した。
  頻度昇順にソートした葉のリストから「package（隣接 2 件を合成）→
  merge（package 済みリストと新鮮な葉リストを重み順にマージ）」を
  `max_length` 段繰り返し、最終レベルの重み最小 `2m-2` 件に含まれる葉の出現
  回数がそのまま符号長になる（coin-collector の定理）。この構成方法は
  **深さ制限下でも構築そのものによって Kraft 不等式を等号で満たす**ため、
  ヒストグラム後処理による不完全化のリスクが原理的に無い。副次効果として
  litlen/dist の通常木（深さ制限に掛からないケース）も同じ関数で最適に
  構築できるため、旧来の「ヒープで木を作って深さを測る」コードを丸ごと
  削除でき実装がむしろ単純になった。
- **影響**: 計画の「package-merge は使わない」という前提から逸脱したが、
  正当性（strict decoder が読めること）を最優先した判断。速度面の悪化は
  誤差程度（release で room.png 79ms→84ms）。

### 3. PNG 行フィルタ選択 (MSAD)

- **判断**: 行ごとに None/Sub/Up/Average/Paeth の 5 種類を実際に計算し、
  フィルタ後バイト列を符号付き差分とみなした絶対値和が最小のものを選ぶ
  （`build_filtered_raw`/`apply_filter`/`msad`）。libpng の "minimum sum of
  absolute differences" ヒューリスティクスと同じ考え方。**理由**: 実際の
  エントロピーを測る（全パターン圧縮して比較する）のは 1 行ごとに 5 回
  DEFLATE を回すことになり非現実的に遅いため、libpng 由来の軽量な近似指標を
  採用した。
- **判断**: 既存の `zlib_compress_stored`（無圧縮 stored 版）は
  `#[cfg(test)]` にして残した（テストでの圧縮率比較・回帰確認用）。
  通常経路は `deflate::zlib_compress` に置き換えた。

### 4. ブロック分割・stored フォールバック

- ブロックサイズは計画どおり 128KiB（`BLOCK_SIZE`）。入力バイト数（トークンの
  消費バイト数の累計）で区切り、各ブロックで固定/動的ハフマン/stored の 3 通り
  のビット数を厳密計算（RLE 済みコード長シンボル列まで含めて動的ハフマンの
  ヘッダビット数を正確に見積もる）し最小を採用する。stored 選択時は元データが
  65535 バイト境界を跨ぐ場合に複数の物理 stored ブロックへ分割する
  （`emit_stored_blocks`、既存の `zlib_compress_stored` と同じ発想）。

### 検証結果

- **round-trip**: `deflate.rs` に 18 件（空/1 バイト/全同値の短短長/32KB 窓
  境界を跨ぐ反復/非圧縮ランダム/周期反復（窓境界跨ぎ）/日本語テキスト/実 PNG
  生ピクセル/258 超の長マッチ/128KiB ブロック境界を跨ぐ擬似ランダム/300 ケース
  のプロパティ的テスト（サイズ・圧縮性混在パターンを乱数生成）/境界値
  ユニットテスト（`length_code`/`dist_code` の 258・32768 境界、`build_lengths`
  の単一シンボル・均等頻度・深さ制限（フィボナッチ頻度で 15bit と 7bit 両方の
  上限を実際に踏む）/zlib ヘッダ検証）を追加。すべて自作 `inflate::inflate` /
  `zlib_decompress` との round-trip で確認。
- **厳密な外部クロスチェック**（上記の教訓を踏まえた新設）: `png.rs` に
  `encode_generated_game_assets_readable_by_image_crate` を追加し、実アセット
  4 枚（room/street/circle/wipe、いずれも 1280x720）を自作エンコード→`image`
  クレート（`png`/`miniz_oxide` 系）でデコードしピクセル完全一致を確認。
  加えて `--autotest` で生成した実スクリーンショット PNG（Windows 13 枚 /
  WSL 16 枚）を `image` クレートおよび Python Pillow（`PIL.Image`、noiria とは
  完全に独立した C 実装のデコーダ）の両方で開けることを個別に確認した
  （手動検証、いずれも成功）。
- **圧縮率**（release ビルド、sample 全 10 PNG、stored 比較用に
  `zlib_compress_stored` を使用）:

  | ファイル | サイズ | raw (フィルタ後) | 圧縮後 | stored 比 |
  |---|---|---|---|---|
  | bg/room.png | 1280x720 | 3,687,120B | 384,139B | 10.4% |
  | bg/street.png | 1280x720 | 3,687,120B | 330,616B | 9.0% |
  | fg/alice_normal.png | 500x900 | 1,800,900B | 5,397B | 0.3% |
  | fg/alice_smile.png | 500x900 | 1,800,900B | 5,568B | 0.3% |
  | fg/btn_menu.png | 360x44 | 63,404B | 2,429B | 3.8% |
  | fg/icon_blink_a/b/spark.png (3 枚) | 28x28 | 3,164B×3 | 858/631/858B | ~24% |
  | rule/circle.png | 1280x720 | 3,687,120B | 88,672B | 2.4% |
  | rule/wipe.png | 1280x720 | 3,687,120B | 5,611B | 0.2% |
  | **合計 (10 枚)** | | 18,424,676B | 824,779B | **4.5%** |

  参考: 同じ RGBA を `image` クレート既定設定で PNG 化すると合計 2,085,735B
  （自作実装はその 39.5%＝2.5 倍以上小さい。`image`/`png` クレートの既定は
  フィルタ適応選択が弱いか圧縮レベルが低いとみられる）。
  **zlib level 6 比較**（品質基準の本来の対象。sample のうち最も厳しい
  room/street/circle の 3 枚について、`build_filtered_raw` が生成した同一の
  フィルタ後バイト列を Python `zlib.compress(data, 6)` で圧縮しサイズ比較）:
  room 384,139B / zlib6 358,313B = **+7.2%**、street 330,616B / zlib6
  321,109B = **+3.0%**、circle 88,672B / zlib6 78,831B = **+12.5%**、
  3 枚合計では 803,427B / 758,253B = **+5.96%**。いずれも品質基準の
  「zlib level 6 比 +15% 以内」を満たす。
- **速度**（release ビルド、1280x720 実データ4枚）: room.png 79〜84ms、
  street.png 70〜77ms、circle.png 17〜19ms、wipe.png 8〜12ms。目安の
  <100ms を全て満たす（debug ビルドでも room.png ~92-100ms 前後で
  ぎりぎり収まることを確認したが、autotest 実行は release 相当の
  最適化が効くとは限らないため、今後さらに重い背景画像が増える場合は
  余裕を見て再計測すること）。
- **Windows**: `cargo build -p noiria-core` 成功、`cargo test --workspace`
  278→**299 件**全通過（新規 21 件: `codec::deflate::tests::*` 18 件
  `empty_input`/`single_byte`/`all_same_byte_short`/
  `all_same_byte_long_crosses_window`/`random_incompressible`/
  `periodic_repeat_crosses_window_boundary`/`japanese_text`/
  `long_match_over_258`/`real_png_raw_pixels`/`multi_block_boundary`/
  `property_random_mixed_many_cases`/`length_code_boundaries`/
  `dist_code_boundaries`/`build_lengths_single_symbol`/
  `build_lengths_uniform_frequency`/`build_lengths_depth_limit_kicks_in`/
  `all_symbols_equal_frequency_roundtrip`/`zlib_header_is_valid`、
  `codec::png::tests::*` 3 件 `encode_smaller_than_stored`/
  `compression_ratio_report_sample_assets`/
  `encode_generated_game_assets_readable_by_image_crate`）。
  `cargo clippy -p noiria-core --all-targets` / `cargo clippy --workspace
  --all-targets` とも新規 warning 0 件（既存の noiria-tools/noiria-app 側の
  警告は W1〜W3 時点と同じ無関係なもの）。`cargo run -p noiria-app --
  games/sample --autotest 20` EXIT=0、13 枚のスクリーンショットを保存し
  全て `image` クレート・Pillow で開けることを確認。
- **WSL** (Ubuntu, `scripts/wsl-test.sh --skip-x11 --skip-alsa`):
  `cargo test --workspace` 288→**309 件**全通過（新規 21 件、Windows と
  同じテスト名。`noiria-app` は x11 テストを含むため Windows 62 件 /
  WSL 73 件という既存の差分はそのまま）。`--autotest 24` も同スクリプト内で
  実行され EXIT 0・16 枚のスクリーンショットを保存、クラッシュなし。生成
  PNG を Windows 側へコピーし Pillow で開けることを個別に確認した。
- **回帰**: 既存テストの削除・無効化なし。`png.rs` の `encode_roundtrip_*`
  系や `decode_generated_game_assets` など既存テストは無変更のまま全通過。

### 変更ファイル一覧

- 新規 `crates/noiria-core/src/codec/deflate.rs`（~890 行、LZ77 + package-merge
  ハフマン + ブロック分割 + zlib/deflate 公開 API + テスト 18 件）
- `crates/noiria-core/src/codec/mod.rs`（`pub mod deflate;` 追加、コメント更新）
- `crates/noiria-core/src/codec/png.rs`（`encode()` を行フィルタ選択 (MSAD) +
  `deflate::zlib_compress` へ差し替え。`zlib_compress_stored` は
  `#[cfg(test)]` 化。新規テスト 3 件）

### 申し送り

- **package-merge への切り替えは計画からの逸脱**（計画は「package-merge 不要」
  としていたが、単純調整方式が現実のデータで degenerate なハフマン木を生成する
  ケースを実際に踏んだため、正当性を優先して切り替えた）。W6 で pak エントリ
  圧縮に同じ `deflate::zlib_compress`/`deflate_compress` をそのまま使う予定
  なので、この修正は W6 にもそのまま効く。
- **自作デコーダとの round-trip だけに頼らない**という教訓は本ウェーブ限定の
  話ではなく、今後 DEFLATE を使う全ウェーブ（W6 pak・将来のアセット圧縮）に
  引き継ぐべき方針。`encode_generated_game_assets_readable_by_image_crate` を
  今後も維持し、新しいエンコードパスを追加する際は同様の厳密クロスチェックを
  検討すること。
- 圧縮率は zlib level 6 相当に対して概ね +3〜+13% で品質基準（+15% 以内）を
  満たすが、`image`/`png` クレート既定出力よりは大幅に小さい一方、真の
  zlib level 6/9 と比べるとまだ改善余地はある（2 段以上の lazy matching、
  より精緻なブロック分割境界の最適化など）。現時点では品質基準を満たして
  いるため W4 としては完了とし、必要であれば将来ウェーブで再チューニングする。

## 2026-07-12 配布対応 W5: GameDir 読込抽象化（挙動不変）

対象: `crates/noiria-core/src/assets.rs`（`GameDir` の Backend 抽象化・API 整理）、
`crates/noiria-core/src/audio.rs`（`AudioCmd::Play` の rel 化、`build_source`/`spawn`
の GameDir 経由化）、`crates/noiria-app/src/script/engine_state.rs`（audio_play/
video_play/video_open の絶対パス撲滅）、`crates/noiria-app/src/script/loader.rs`
（V8 モジュールローダの GameDir 経由化）、`crates/noiria-app/src/script/mod.rs`
（呼び出し側の追従）、`crates/noiria-app/src/main.rs`（`audio::spawn` へ GameDir 追加）。
承認済み計画 `breezy-sauteeing-fox.md` の W5 節に従う。ベースラインは Windows
cargo test 299 件 / WSL 309 件全通過。**受け入れ基準は挙動完全不変**（W6 の pak
Backend 追加に備えた内部整理のみで、ユーザー可視の挙動は一切変えない）。

### 1. GameDir を Backend enum + Arc 化

- **判断**: `GameDir { root: PathBuf }` を `GameDir { backend: Arc<Backend> }` /
  `enum Backend { Dir(PathBuf) }` に変更。W6 で `Backend::Pak(..)` を足すだけで
  済む形に揃えた。`Arc` 化により `Clone` が安価になり、オーディオスレッド
  （`audio::spawn`）や V8 モジュールローダ（`NoiriaModuleLoader`）へ気兼ねなく
  `game_dir.clone()` を渡せるようになった（以前も `#[derive(Clone)]` で
  コンパイルは通っていたが、`PathBuf` を都度ヒープ複製していた）。
- **判断**: `resolve()`（絶対 `PathBuf` を返す）を非公開化した。呼び出し側
  （`engine_state.rs` の旧 audio_play/video_play/video_open）が「`resolve()` で
  絶対パスを取り出してから自分で読む」パターンを直接踏んでいたのが、まさに
  ①②③の素通り経路の原因だったため、GameDir の外に絶対パスを一切出さない
  ことを設計の核にした。**波及**: `resolve()` を使えるのは `assets.rs` 内部
  （`read_bytes`/`read_text`/`exists`/`list_dir`/`load_image_rgba`/`open_video` の
  実装）だけになり、`noiria-app` 側は `GameDir` の高レベル API
  （`read_bytes`/`read_text`/`open_video` 等）を呼ぶしかなくなった。副作用として
  **`audio_play` の相対パス検証（`../` 等のジェイル違反チェック）が同期的な
  Err 返却からオーディオスレッド側 `build_source`（`GameDir::read_bytes` 内部）
  の非同期 `log::error!` に変わった**（video 系は `open_video` が同じ呼び出し
  スレッド上で同期的にジェイルチェックするので変化なし）。正規のシナリオでは
  パスは常に有効なので観測可能な差は無いが、悪意あるパス指定に対する
  エラー通知タイミングが変わる点は明示しておく。
- **判断**: `GameDir::open(path)` に「`path` がファイル（.pak 想定）なら
  W6 待ちの明確なエラーを返す」分岐を追加した（`root.is_file()` チェック）。
  以前はディレクトリでない `path` を渡すと `game.json が読めません` という
  紛らわしいエラーになっていた。ディレクトリ起動の既存挙動は完全に不変。

### 2. 音声（① 素通り修正）

- **判断**: `AudioCmd::Play { path: PathBuf }` → `{ rel: String }` に変更。
  `audio::build_source` は `game_dir.read_bytes(&rel)` でバイト列を取得し、
  拡張子判定は `Path::new(rel).extension()` に置き換えた（ロジック自体は
  無変更）。`audio::spawn` に `game_dir: GameDir` 引数を追加し、コマンド処理
  スレッドの最後（`build_source` 呼び出し）まで参照で受け渡す。
- **判断**: エラーメッセージを `path.display()`（絶対パス）から `rel`
  （相対パス文字列）表示へ変更した（計画で明記されていた変更）。ログにしか
  出ない情報であり、テストで文字列内容を厳密に検証している箇所は無いことを
  確認済み。
- **影響範囲確認**: `--autotest` はヘッドレスではオーディオスレッドを
  そもそも起動しない（`audio_rx` を `drop` するだけ）ため、この変更は
  `--autotest` のスクリーンショット出力には一切影響しない。実機（Windows,
  `--autotest 90`）でオーディオスレッドを実際に起動して BGM/SE タグを
  通し、エラーログ 0 件を確認した。

### 3. 動画（② 素通り修正）

- **判断**: `GameDir::open_video(rel) -> Result<Box<dyn VideoSource>>` を
  新設。`Backend::Dir` は内部で `resolve(rel)` して従来通り実パスを
  `codec::video::open_video_file` に渡す（Windows Media Foundation
  フォールバックは実ファイルパス前提のままなので、`codec/video.rs` 自体は
  無改変）。`engine_state.rs` の `video_play`/`video_open` は
  `self.game_dir.resolve(path)` + `video::open_video_file(&full)` の 2 行を
  `self.game_dir.open_video(path)` の 1 行に置き換えただけで、実質的な
  デコード経路・エラーメッセージ（実パスを含む文言）は完全に同一。

### 4. V8 モジュールローダ（③ 素通り修正）と Windows 固有バグの発見

- **判断**: `NoiriaModuleLoader` に `game_dir: GameDir` を持たせ、`load()` の
  `file://` 分岐を `std::fs::read_to_string` から `game_dir.read_text(rel)`
  へ置き換えた（tsint 側 `NoiriaTsHost` と同構図に揃えた。tsint 側は元々
  GameDir 経由だったため無改変）。コンストラクタ `NoiriaModuleLoader::new` の
  引数を `&Path` から `GameDir` に変更し、`run_script`（`mod.rs`）・テスト
  呼び出し側は `ctx.game_dir.clone()` を渡すよう追従した。
  `main_module()`（相対 import 解決用の URL join）は無変更。
- **重要な発見（Windows 固有のパス表現の不一致）**: `file://` specifier から
  ゲームルート相対パスを求める際、当初 `specifier.to_file_path()` の結果を
  `self.game_dir.root()`（`GameDir::open` が `canonicalize()` した
  `PathBuf`）で `strip_prefix` していたところ、**Windows で全てのモジュール
  読込がゲームディレクトリ外エラーになり `tag_behavior_tests` が V8 側だけ
  op_log 0 件で軒並み失敗した**。原因は `std::fs::canonicalize()`（Windows）が
  `\\?\C:\Users\...` の verbatim (UNC 拡張) prefix を付けて返すのに対し、
  `url::Url::to_file_path()` は verbatim prefix を付けずに `C:\Users\...` を
  返すため、両者が常にバイト不一致になっていたこと。**対処**: ゲームルートも
  同じ `to_file_path()` を経由した値（`game_root_path` フィールド）を
  `NoiriaModuleLoader::new` 時点で 1 回だけ計算して保持し、`load()` 側の
  `strip_prefix` はこの値と比較するよう統一した（両者とも `to_file_path()`
  経由なので表現が揃う）。この不一致は `resolve()` 非公開化に伴う設計変更で
  新たに生まれた比較コードがきっかけで顕在化したもので、**tag_behavior_tests
  30 件の失敗を経て発見・修正した**（`docs/implementation-notes.md` にも
  再発防止のため詳しく記録する）。修正後は Windows/WSL とも全テスト通過。

### 5. 変更したテスト（AudioCmd/NoiriaModuleLoader の形が変わったための追従）

- `crates/noiria-core/src/audio.rs` の `build_source` 系テスト 6 件
  （`build_source_decodes_wav_via_own_decoder` 等）: `build_source` の
  シグネチャが `(path: &Path, ..)` → `(game_dir: &GameDir, rel: &str, ..)` に
  変わったため、一時ディレクトリ + `GameDir::for_test()`（新設の
  `#[cfg(test)] pub(crate)` ヘルパ、`game.json` 不要でディレクトリを直接
  開く）でファイルを包んで渡す形に書き換えた。アサーション内容（デコード
  結果・エラーメッセージの部分文字列）は無変更。
- `crates/noiria-core/src/assets.rs` の既存 4 テスト
  （`path_jail`/`list_dir_lists_files_sorted_and_ignores_subdirs`/
  `list_dir_missing_directory_returns_empty_not_error`/
  `read_bytes_reads_binary_content`）: `GameDir { root: .. }` の直接構築が
  `backend` フィールド化で不可能になったため `GameDir::for_test(..)` に
  置き換え。アサーション内容は無変更。**新規追加**: `path_jail_via_public_read_apis`
  （`read_bytes`/`read_text`/`open_video` 経由でも `../` 脱出が拒否されることの
  回帰テスト。受け入れ基準の「パスジェイルの新 API テスト」に対応）。
- `crates/noiria-app/src/script/mod.rs` の V8 実 ops テスト 3 件
  （`button_manager_hit_test_and_state_transitions_in_v8` 等）:
  `NoiriaModuleLoader::new(&dir)` → `NoiriaModuleLoader::new(ctx.game_dir.clone())`
  に置き換え（コンストラクタの引数型変更への追従のみ）。`js_tests::run_js_module`
  ヘルパは `NoiriaModuleLoader` が `GameDir` を要求するようになったため、
  `game.json` だけの最小ゲームディレクトリを一時的に用意してから
  `GameDir::open` するよう変更した（このテストは埋め込み `noiria:` モジュール
  しか読まないため実質的な検証内容への影響なし）。
- 上記以外の全既存テストは無修正で通過。

### 6. clippy 対応（新規、機能に無関係）

- `cargo clippy --workspace --all-targets` で `items_after_test_module`
  （`#[cfg(test)] mod tests` の後に非テストの `impl`/`fn` が続くのを警告する
  比較的新しいレイアウト系 lint）が `crates/noiria-app/src/script/mod.rs` と
  `crates/noiria-app/src/script/loader.rs` で発火した。両ファイルとも元々の
  レイアウト（テストモジュールが `impl`/`fn` の手前にある）が原因で、
  内容そのものは W5 で変更していないが、クリーンなクリップ結果を維持する
  ため両ファイルとも該当のテストモジュールをファイル末尾へ機械的に移動した
  （中身は一切変更していない）。`noiria-tools` 側の既存 warning（`analysis.rs`
  の `manual_is_multiple_of` 等）は W5 が触れていないファイルの既存事象なので
  そのまま残している。

### 検証

- **Windows**: `cargo test --workspace` 299→**300 件**全通過（新規 1 件:
  `assets::tests::path_jail_via_public_read_apis`）。`cargo clippy --workspace
  --all-targets` は `noiria-app`/`noiria-core` 側 warning 0 件（`noiria-tools`
  側の既存無関係 warning のみ残存、W5 では触れていない）。
- **WSL**（Ubuntu, `CARGO_TARGET_DIR=$HOME/.noiria-target`）: `cargo test
  --workspace` 309→**310 件**全通過（新規 1 件、Windows と同じ。`noiria-app`
  は x11 テストを含むため Windows 62→63 件/WSL 73→74 件という既存の差分は
  そのまま）。
- **PNG ハッシュ比較（挙動不変の中心的な検証）**: git 管理外のため、W5 着手
  前の状態を `resolve()`/`open_video`/`NoiriaModuleLoader` 等の変更点だけ
  手動で巻き戻した一時コピーをスクラッチ領域に作成し、そちらを「変更前」
  ベースラインとしてビルド・`--autotest 24` を実行、W5 適用後の同一
  シナリオと SHA-256 で比較した。16 枚中 14 枚は完全一致。残り 2 枚
  （`frame_03_hover.png`/`frame_09_hover.png`）は差分があったが、**W5 適用後
  の同一バイナリを 2 回連続実行しただけでも同じ 2 枚が毎回異なるハッシュに
  なる**ことを確認しており（マウスホバーのタイミングが壁時計駆動のため、
  ヘッドレス autotest 固有の実行毎ジッタと判明）、W5 由来の差分ではないことを
  切り分け済み。
- **動画/音声の実地確認**（Windows, `--autotest 90`）: EXIT=0、ログにエラー/
  パニック 0 件。`games/sample/scenario/first.ks` の `[video storage=demo.avi]`
  および W6 拡張動画タグ（openvideo/preparevideo/playvideo/pausevideo/
  resumevideo/rewindvideo）区間を含む 62 枚のスクリーンショットを保存し、
  動画デコード結果（人物シルエット）がレイヤに正しく合成されていることを
  目視確認した。
- **パスジェイルの新 API テスト**: `assets::tests::path_jail_via_public_read_apis`
  で `read_bytes`/`read_text`/`open_video` それぞれについて `../` を含む相対
  パスがエラーになることを確認（新規追加、上記「検証」の 300/310 件に含む）。

### 変更ファイル一覧

- `crates/noiria-core/src/assets.rs`: `GameDir` を `Backend` enum + `Arc` 化。
  `resolve()` 非公開化、`open_video()` 新設、`open()` の pak 検知エラー、
  テスト専用 `for_test()` 追加。
- `crates/noiria-core/src/audio.rs`: `AudioCmd::Play` の `path: PathBuf` →
  `rel: String`。`build_source`/`spawn`/`run`/`command_loop`/`handle` に
  `GameDir` を通す。
- `crates/noiria-app/src/script/engine_state.rs`: `audio_play`/`video_play`/
  `video_open` を GameDir 高レベル API 経由に書き換え、絶対パスの合成を撤去。
- `crates/noiria-app/src/script/loader.rs`: `NoiriaModuleLoader` を `GameDir`
  経由の読込に変更。Windows の `\\?\` verbatim prefix 不一致バグを修正。
  clippy 対応でテストモジュールを末尾へ移動。
- `crates/noiria-app/src/script/mod.rs`: `run_script`/テスト呼び出し側を
  新しい `NoiriaModuleLoader::new(GameDir)` に追従。`js_tests::run_js_module`
  に最小ゲームディレクトリを追加。clippy 対応でテストモジュールを末尾へ移動。
- `crates/noiria-app/src/main.rs`: `audio::spawn` へ `game_dir.clone()` を追加。

### 申し送り（W6 への引き継ぎ）

- **Backend::Pak の追加位置**: `crates/noiria-core/src/assets.rs` の
  `enum Backend { Dir(PathBuf) }` に `Pak(..)` を足し、`GameDir` の各メソッド
  （`resolve`/`read_bytes`/`read_text`/`exists`/`list_dir`/`load_image_rgba`/
  `open_video`）の `match self.backend.as_ref()` に Pak 分岐を追加する形になる
  見込み。`resolve()` が非公開化・`Backend::Dir` 専用コメント付きなので、Pak
  実装時は「実パスを返さない」設計を素直に踏襲できるはず。
- **MF（Media Foundation）の実パス依存は W5 でも未解消**: `open_video` の
  `Backend::Dir` 分岐は今まで通り実ファイルパスを `codec::video::open_video_file`
  に渡している。W6 で pak から動画を開く場合、MJPEG/VP9 は `Vec<u8>` 全所有
  構成（`codec::video::open_video` 関数、pak のメモリ上バイト列でも動作可能）
  だが、**MF 経由でしか再生できない形式（MP4 等）は pak 内からの再生が
  引き続きできない**（計画の W6 節にも明記済みの既知の制限）。
- **音声のジェイル違反エラーが非同期化した**点（本文 1 節参照）は W6 の
  暗号化 pak でも同じ構図が続く（不正 rel は `read_bytes` 内部で弾かれ、
  オーディオスレッド側で `log::error!` になる）。ユーザー向けの動作としては
  問題ないが、テストハーネスで音声パスの同期エラーを期待するテストを新設
  する場合は要注意。
- **PNG ハッシュ比較のベースライン再現手順**: git 管理外リポジトリのため、
  今回はスクラッチ領域に「変更後の全ファイルをコピー→機能変更のみ手動で
  巻き戻す」方式でベースラインを再構築した。今後同様の behavior-invariant
  ウェーブ（特に W6 pak 導入時のディレクトリ起動との出力一致検証）を行う際は、
  **作業開始前に `--autotest` の PNG を退避しておく**（本来の計画記載の手順）
  ことを強く推奨する。

## 2026-07-12 配布対応 W6: ChaCha20 自作 + 暗号化 pak (NPK1)

対象: `crates/noiria-core/src/crypto/`（新設、ChaCha20）、`crates/noiria-core/src/pak.rs`
（新設、NPK1 pak フォーマット）、`crates/noiria-core/src/assets.rs`（`Backend::Pak` 追加）、
`crates/noiria-core/src/codec/png.rs`（`crc32` を `pub(crate)` 化して再利用）、
`crates/noiria-core/src/lib.rs`（`pub mod crypto;`/`pub mod pak;` 追加）、
`crates/noiria-tools/src/pack.rs`（新設、`pack_dir`）、
`crates/noiria-tools/src/bin/noiria_pack.rs`（新設 CLI `noiria-pack`）、
`crates/noiria-tools/src/lib.rs`（`pub mod pack;` 追加）、
`crates/noiria-tools/Cargo.toml`（`[[bin]] noiria-pack` 追加）、
`crates/noiria-tools/tests/pak_roundtrip.rs`（新設、`games/sample` を使った統合テスト）。
ベースラインは Windows `cargo test --workspace` 300 件 / WSL 310 件全通過。

### 1. ChaCha20 実装（RFC 8439 形式）

- **判断**: nonce は 12 バイト + 32bit LE カウンタの RFC 8439 形式を採用（Bernstein 原案の
  8 バイト nonce/64bit カウンタ形式ではない）。**理由**: pak の各エントリで nonce を
  `entry_id` から一意に導出する設計と相性がよく、RFC の公式試験ベクタもこの形式。
  **影響**: 1 nonce あたり最大 2^32 ブロック (256GiB) までしか安全に暗号化できないが、
  ゲームアセット用途では無関係。
- **判断**: Poly1305（MAC）は実装しない。**理由**: ユーザー承認済みのスコープ外
  （脅威モデルは「エクスプローラで気軽にファイルを抜き出す」程度のカジュアルな
  抜き出し抑止であり、意図的な改ざんへの耐タンパー性は目的にしていない）。整合性は
  各エントリの CRC32（展開後平文に対して計算）で代替する。**影響**: 悪意ある第三者が
  暗号文を直接書き換えた場合、ChaCha20 は単純 XOR なので対応する平文バイトが
  ビット反転するだけで、CRC32 チェックで高確率検出はできるが暗号学的な改ざん検知
  ではない（意図通り）。
- **判断**: RFC 8439 の既知テストベクタ（§2.3.2 ブロック関数、§2.4.2 全文暗号化）は
  手元の記憶に頼らず、OpenSSL 3.5 (`openssl enc -chacha20`, IV = counter(LE 4B) +
  nonce(12B) という EVP_chacha20 の慣習を利用し、ゼロ埋めバッファを暗号化して
  キーストリームを独立算出) で実測して転記した。**理由**: 16 進数 64 バイトの手入力は
  1 桁でも誤ると「自己無矛盾だが RFC 非準拠」なテストが「正しく」通ってしまう危険が
  あるため、外部の信頼できる独立実装で裏取りした。**影響**: なし（既知ベクタなので
  当然一致するはずだが、念のため記録）。
- **判断**: `apply_keystream(key, nonce, byte_offset, data)` は `byte_offset` が
  ブロック境界に揃っていなくてもよい設計にした（`byte_offset/64` がブロックカウンタ、
  `byte_offset%64` がブロック内オフセット）。**理由**: `PakEntryReader` の非アライン
  シークを 1 関数で素直に実装するため（呼び出し側で「ブロック境界に丸めてから
  スライス」という定型処理を書かせない）。

### 2. NPK1 pak フォーマット

- **判断**: pak エントリおよびインデックスの圧縮に **zlib ラッパではなく生 DEFLATE
  (`deflate_compress`/`inflate`) を使う**。**理由**: pak エントリは既に独自の CRC32
  で整合性を検査しており、zlib の adler32 ヘッダ (2 + 4 バイト) は完全に冗長な
  オーバーヘッドでしかない。**影響**: エントリ数が多い pak ほど、zlib ラッパ比で
  エントリ 1 個あたり 6 バイト分小さくなる（`games/sample` の 23 エントリで
  約 140 バイト。無視できる差だが「冗長なものは削る」の一貫性のため）。
- **判断**: nonce は「4 バイト固定タグ `b\"NPK1\"` + 8 バイト `entry_id` (LE)」。
  ヘッダの `key_check` だけは nonce 全ゼロ固定（entry_id 由来ではない）。
  インデックス自身は `entry_id = u64::MAX` を使い実エントリ (`0..N`) と衝突しない。
  **理由**: 固定タグにより「同じ鍵を複数の pak で使い回しても、pak をまたいだ
  ケーストリーム再利用にはならない」ことを期待したいところだが、これは pak ファイル
  自体が変わればタグも entry_id も同じでも異なる鍵を使う運用を推奨する程度の弱い
  対策であり、**同一鍵で複数 pak を作ると理論上ケーストリーム再利用のリスクがある**
  （脅威モデル上は許容: MAC 非実装と同じスコープ外判断）。**影響**: 将来的に鍵を
  pak ごとに変える運用（W7 の `KEY_SLOT` パッチ）を推奨する。
- **判断**: `key_check` は「`ChaCha20(key, nonce=[0;12], counter=0)` キーストリームの
  先頭 8 バイト」という平文を介さない値にした。**理由**: 実際のエントリを試し読みして
  「鍵が違う」と判定するより、ヘッダだけで即座に・確実に判定できる（エントリ0件の
  pak でも機能する）。**影響**: `Pak::open` は鍵が違う場合、インデックスの復号・展開
  すら試みずに明確なエラーで即失敗する（`鍵が違います。この pak を復号できません
  （key_check 不一致）`）。
- **判断**: pack 時のコーデック選択は「拡張子 (`png jpg jpeg ogg avi ivf mp4 webp`)
  なら無条件 stored、それ以外は DEFLATE を試して縮まなければ stored にフォールバック」
  というルールを `pak::build`/`compress_for_entry` に直接実装し、`is_precompressed_ext`
  だけを独立関数として切り出した。**理由**: 「拡張子で決める」というロジック自体は
  小さく、圧縮結果（bytes）をそのまま使い回すため、決定関数と圧縮処理を分離すると
  同じ DEFLATE 結果を 2 回計算する無駄が生まれる。**影響**: `noiria-tools::pack` 側は
  コーデック選択ロジックを持たず、`pak::build` の戻り値 (`PackReport`) から
  stored/deflate の件数を受け取って表示するだけ。
- **判断**: `PakEntryReader`（`Read`+`Seek`）は codec=0（無圧縮）エントリのみ対応し、
  codec=1（DEFLATE）エントリでは明確なエラーを返す。**理由**: DEFLATE ストリームは
  構造上、任意バイトから展開を再開できない（LZ77 の後方参照が先頭からの展開状態に
  依存する）。シーク可能な部分読み出しを実現するには圧縮しないことが前提。
  **影響**: 現状 pack される主要な大容量アセット（PNG/AVI/IVF や、拡張子ルールで
  たまたま stored 判定になった WAV 等）はこの制約を満たす。将来 pak からの動画
  ストリーミング読み出しを本格導入する際は、動画コンテナは常に stored にする
  運用を維持すること。
- **判断**: `KEY_SLOT` は `const` ではなく `#[used] static` にし、読み出しは
  `std::ptr::read_volatile` を 1 バイトずつ使う。**理由**: `const` は使用箇所へ値が
  インライン展開されるため、実行ファイル中に「後からバイナリパッチできる固定
  アドレスの実体」が残らない。また全ゼロの `static` を素直に読むだけだと、
  コンパイラが「初期値は分かっているので `key_slot_key()` は常に `None`」と
  定数畳み込みしてしまう可能性がある（W7 のバイナリパッチはコンパイラの
  関知しないところで実行ファイルを書き換えるため、この最適化は前提を壊す）。
  `read_volatile` はこの最適化を抑止する標準的な手段。**影響**: W7（本 wave の
  対象外）がこの `KEY_SLOT` の 16 バイト目以降 32 バイトを実鍵でバイナリパッチする
  ことで、配布ビルドは `NOIRIA_PAK_KEY` 環境変数や開発用固定鍵に頼らず鍵を得られる
  ようになる想定。
- **判断**: 開発用固定鍵 `DEV_KEY` は ASCII 文字列
  `"Noiria-W6-DevKey-2026-07-12!!!!!"`（32 バイトぴったり）を採用し、
  hex で `4e6f697269612d57362d4465764b65792d323032362d30372d31322121212121`。
  **理由/影響**: これは秘密ではない（未パッチ開発ビルド全体で共通の既定値であり、
  ここに書いてある時点で公開情報。配布ビルドは W7 の `KEY_SLOT` パッチで別の鍵に
  差し替える前提）。鍵解決順位は `GameDir::open` が pak を開く際
  ①`KEY_SLOT` パッチ済みならそれ ②環境変数 `NOIRIA_PAK_KEY`（64桁 hex） ③この
  `DEV_KEY`、の優先順（`pak::resolve_engine_key()`）。`noiria-pack` CLI は
  `--key <64桁hex>` 省略時に同じ `DEV_KEY`（`noiria_core::pak::dev_key()`）を使う。

### 3. `GameDir::Backend::Pak`

- **判断**: パスジェイルチェック（`..`/絶対パス/ドライブレター拒否）を
  `check_jail()` として切り出し、`Backend::Dir` の `resolve()` と `Backend::Pak` 用の
  `pak_key()`（`/` 区切りキーへ正規化）の両方から呼ぶ形にした。**理由**: 計画で
  明示されていた通り「pak には実ファイルシステムが無いので脱出できないから
  チェック不要」ではなく、`../../etc/passwd` 的なキーが黙って通ったりパニックしたり
  しないよう一貫して拒否する。**影響**: `resolve()` 自体の既存ロジック・既存エラー
  文言は変更していない（`check_jail` は元の判定処理をそのまま抜き出しただけ）。
  `pak_key()` は新規ロジックで、Windows のバックスラッシュ区切り rel はもちろん、
  Linux 上で（バックスラッシュがただの文字として 1 コンポーネントに埋め込まれた）
  rel が来ても `/` 区切りへ分解する（`noiria-tools::pack::pack_dir` が常に `/` 区切りで
  index を作るため、pak のルックアップキーもここで揃える必要がある）。
- **判断**: `GameDir::root()` の `Backend::Pak` 実装は、pak ファイル自身の実パス
  （`GameDir::open` が `canonicalize()` した値）をそのまま返す「疑似ルート」にした
  （シグネチャ変更なし、`Result` 化もしない）。**理由/調査結果**:
  `NoiriaModuleLoader::new`（`crates/noiria-app/src/script/loader.rs`）が
  コンストラクタ時に一度だけ `game_dir.root()` を呼び、`ModuleSpecifier::
  from_directory_path()` で `file://` 基点 URL を組み立てている。この呼び出しは
  実ファイルシステムへアクセスしない（`url` クレートの `from_directory_path` は
  パス文字列を URL に変換するだけで stat しない）。実際のモジュール読込は
  `load()` の `file://` 分岐が `game_dir.read_text(rel)` 経由で行っており
  （W5 で確立済み）、`root()` が指すパスへ実際にファイルアクセスすることは無い。
  したがって pak ファイル自身の実パスを「ダミーの絶対パス」として返せば、
  URL 組み立て（末尾に `/` を付けるだけ）は問題なく動作し、以降の相対 import
  解決・`strip_prefix` 比較もすべて `to_file_path()` 経由の同じ値同士の比較に
  閉じるため破綻しない。実機検証（`--autotest 24`/`90` で `main.ts`/`first.ks` 等
  複数モジュールを読み込む一連の動作が pak 版でも dir 版と同一 PNG を出力する
  ことを確認済み）でこの判断の妥当性を確認した。**影響**: `root()` のシグネチャ・
  呼び出し側は無変更で済んだ（`Result` 化や呼び出し側の追従は不要と判断）。
- **判断**: `load_image_rgba` の PNG/JPEG マジックバイト判定+デコード呼び出しを
  `decode_image_bytes(bytes, label)` として切り出し、`Backend::Dir`/`Backend::Pak`
  両方から呼ぶ（`label` はエラーメッセージ用: Dir は実パス表示、Pak は pak 内
  相対パス表示で元の文言スタイルを踏襲）。**理由**: デコードロジック自体は
  バックエンドに依存しないため、計画の指示通り重複を避けた。
- **判断**: `open_video` の `Backend::Pak` 分岐は、読み込んだバイト列の先頭が
  `ftyp`（MP4 コンテナの目印）なら pak 内再生不可の明確なエラーを返し、それ以外は
  `codec::video::open_video(bytes)`（`Vec<u8>` 全所有版、AVI/IVF 対応）にそのまま
  委譲する。一時ファイルへの書き出しは行わない（計画で明示された対象外）。
  **理由**: Windows Media Foundation は実ファイルパス前提の API であり、pak の
  メモリ上バイト列を直接渡す経路が無い。**影響**: `games/sample/assets/video/
  demo.mp4` は pak 内に同梱されるが（拡張子ルールで stored）、pak 版シナリオが
  同じ `.mp4` タグを踏んだ場合は明確な日本語エラーで失敗する（今回の
  `first.ks`/`second.ks` は `.avi` のみを参照するため `--autotest` では未経験。
  MP4 経路のエラーメッセージ自体は pak.rs 側の単体テストでは検証していないので、
  今後 pak 版で MP4 タグを踏むシナリオを追加する場合は要目視確認）。

### 4. `noiria-tools` — `pack_dir` / `noiria-pack` CLI

- **判断**: `pack_dir` は集めたエントリを相対パス順にソートしてから `pak::build` に
  渡す。**理由**: entry_id の割当（＝nonce の一部）を含め pak の出力バイト列を
  決定的にする（同じディレクトリを 2 回 pack しても bit-identical になる。デバッグ・
  再現性のため。計画に明記された要件ではないが実装コストが低く実利が大きいため
  追加した）。
- **判断**: `--exclude` は単純な部分文字列一致（`rel_str.contains(pattern)`）。
  **理由**: 計画が「単純パターン」と明記しており、glob 実装は今回の用途に対して
  過剰。**影響**: `*.tmp` のようなワイルドカードは書けない（`tmp` という部分文字列で
  代用する運用になる）。

### 検証

- **Windows**: `cargo test --workspace` 300→**318 件**全通過（新規 18 件）。
  内訳: `noiria-core` 15 件（ChaCha20 5 件:
  `crypto::chacha20::tests::{rfc8439_section_2_3_2_block_vector,
  rfc8439_section_2_4_2_encryption_vector,
  counter_seek_block_aligned_split_matches_single_call,
  counter_seek_non_aligned_split_matches_single_call,
  encryption_is_its_own_inverse}`。pak 10 件:
  `pak::tests::{roundtrip_basic, wrong_key_is_rejected_clearly,
  codec_selection_stores_precompressed_and_deflates_text,
  index_and_content_are_not_present_as_plaintext,
  list_dir_matches_one_level_contract,
  entry_reader_random_seek_matches_full_decrypt,
  entry_reader_rejects_deflated_entries, key_slot_unpatched_is_none,
  hex_key_parsing, dev_key_is_32_bytes_and_stable}`）。`noiria-tools` 3 件
  （`pack::tests::pack_dir_roundtrips_and_excludes` 1 件 + 統合テスト
  `tests/pak_roundtrip.rs` の
  `packed_sample_game_matches_directory_backend_byte_for_byte`/
  `packed_sample_game_hides_filenames_and_content` 2 件）。
- **WSL**（Ubuntu, `CARGO_TARGET_DIR=$HOME/.noiria-target`）: `cargo test --workspace`
  310→**328 件**全通過（新規 18 件、Windows と同一テスト名。新規テストは全て
  プラットフォーム非依存の純粋 Rust のため差分なし。`noiria-core` の Windows 228 件/
  WSL 227 件という 1 件差は W6 以前から存在する `wasapi_out`/`alsa_out` の
  OS 別バックエンドテスト数の違いであり W6 由来ではない）。
- **clippy**: `cargo clippy --workspace --all-targets` — W6 で触った/追加したファイル
  （`crypto/`, `pak.rs`, `assets.rs`, `codec/png.rs`, `noiria-tools/src/pack.rs`,
  `noiria-tools/src/bin/noiria_pack.rs`, `noiria-tools/tests/pak_roundtrip.rs`）は
  warning 0 件。既存の無関係 warning（`noiria-tools/src/analysis.rs` の
  `manual_is_multiple_of`、`synth.rs` の doc list インデント、`paint.rs` の
  未使用関数、`examples/analyze_bgm.rs` の型複雑度、`tests/audio_quality.rs` の
  `manual_range_contains`）は W5 以前からの既存事象でそのまま残存（W6 では触れていない
  ファイル）。
- **pak サイズ実測**（`noiria-pack --game games/sample --out target/sample.pak`、
  開発用固定鍵）: エントリ数 23、codec 内訳 stored=12/deflate=11。元サイズ
  23,760,978 バイト（`games/sample` 全体、資産は約 22.6MiB）→ pak サイズ
  22,756,628 バイト（95.8%、約 1.00MB 削減）。削減分の内訳を個別に実測したところ、
  BGM/SE/voice の WAV 5 ファイル合計 21,474,568 バイト → DEFLATE 後
  20,479,548 バイト（95.37%、約 995KB 削減）で、**pak 全体の削減量のほぼ全て
  （実測 995,020 / 1,004,350 バイト ≒ 99%）が WAV の DEFLATE 圧縮由来**と判明した
  （PCM は非圧縮可能ではないが、無音区間や単純な波形の冗長性である程度縮む。
  「大きな削減は期待できないが実数値を報告する」という計画の指示通りの結果）。
- **E2E PNG 比較**（`--autotest 24`、pak 版 vs dir 版、SHA-256 フレーム単位比較）:
  16 枚中 14 枚が完全一致。差分があったのは `frame_03_hover.png`/
  `frame_09_hover.png` の 2 枚のみで、**dir 版を 2 回連続実行しただけでも同じ 2 枚が
  毎回異なるハッシュになる**ことを再確認した（W5 で既に特定済みのマウスホバー
  タイミングジッタ、W6 由来ではないことを切り分け済み）。
- **`--autotest 90`**（pak 版, Windows）: EXIT=0、ログにエラー/パニック 0 件。
  BGM 切替 (`xchgbgm`)・ボタン・チェックボックス・エディット・tempsave/tempload・
  goback 等、音声/セーブ/UI 系のコードパスを広く通過することを確認。
- **誤鍵テスト**: `NOIRIA_PAK_KEY` に妥当な形式だが異なる 64 桁 hex
  （`ab` を 32 回）を設定して `target/sample.pak` を開いたところ、
  `Error: pak を開けません: <path>\n\nCaused by:\n    鍵が違います。
  この pak を復号できません（key_check 不一致）` という明確なエラーで
  **EXIT=1**（パニックなし、汎用パースエラーとも区別可能な文言）を確認。
- **`PakEntryReader` シーク**: `pak::tests::entry_reader_random_seek_matches_full_decrypt`
  でブロック境界を跨ぐ複数オフセット（0, 63, 64, 100, 17, 595 バイト目、
  非アライン込み）からの読み出しが、全体を先頭から復号した結果の該当スライスと
  完全一致することを確認。
- **`cargo run -p noiria-app -- target/sample.pak --autotest 24`**: `main.rs` は
  無改修のまま `GameDir::open(&args.game_path)?` が pak を透過的に開けることを実機で
  確認済み（想定通り、追加の分岐は不要だった）。

### 変更/新規ファイル一覧

- `crates/noiria-core/src/crypto/mod.rs`（新規）: ChaCha20 モジュールの公開面。
- `crates/noiria-core/src/crypto/chacha20.rs`（新規）: ブロック関数 + カウンタモード
  ストリーム暗号、RFC 8439 §2.3.2/§2.4.2 ベクタ + カウンタシークのテスト。
- `crates/noiria-core/src/pak.rs`（新規）: NPK1 pak の読み書き・`PakEntryReader`・
  `KEY_SLOT`/鍵解決。
- `crates/noiria-core/src/assets.rs`: `Backend::Pak` 追加、`check_jail`/`pak_key`/
  `decode_image_bytes` 切り出し、`GameDir::open` の pak 分岐、`root()`/各メソッドの
  Pak 対応。
- `crates/noiria-core/src/codec/png.rs`: `crc32` を `pub(crate)` 化（`pak.rs` から再利用）。
- `crates/noiria-core/src/lib.rs`: `pub mod crypto;`/`pub mod pak;` 追加。
- `crates/noiria-tools/src/pack.rs`（新規）: `pack_dir`（ディレクトリ走査 + pack）。
- `crates/noiria-tools/src/bin/noiria_pack.rs`（新規）: `noiria-pack` CLI 本体。
- `crates/noiria-tools/src/lib.rs`: `pub mod pack;` 追加。
- `crates/noiria-tools/Cargo.toml`: `[[bin]] noiria-pack` 追加。
- `crates/noiria-tools/tests/pak_roundtrip.rs`（新規）: `games/sample` 実データでの
  pack→open→byte-exact 比較 + 秘匿性（平文非露出）の統合テスト 2 件。

### 申し送り（W7 への引き継ぎ）

- `KEY_SLOT`（`pak.rs`）はマジック `NOIRIA-KEYSLOT1\0`（16 バイト）+ 鍵領域（32 バイト、
  現状ゼロ）の計 48 バイト `#[used] static`。W7 はコンパイル済み実行ファイル中から
  このマジックを探し、後続 32 バイトを実鍵でバイナリパッチする想定（`key_slot_key()`
  は `ptr::read_volatile` で読むので、パッチ後は次回起動時から自動的に新しい鍵を
  使う。再ビルド不要）。
- 開発用固定鍵 `DEV_KEY`（hex:
  `4e6f697269612d57362d4465764b65792d323032362d30372d31322121212121`）は公開情報。
  配布ビルドは必ず `KEY_SLOT` をパッチしてから配ること（パッチ忘れの検知手段は
  W7 のスコープ、W6 では用意していない）。
- codec=2（将来の FLAC-subset 音声コーデック、W9）は pak の読み書きどちらも未実装。
  `Pak::read`/`pack_dir` とも codec の値は 0/1 のみを想定しており、2 が来た場合
  `pack::build` 側は生成しない（到達しない）、`Pak::read` 側は「未対応の codec です」
  で明確にエラーになる。

## 2026-07-12 配布対応 W7: export コマンド + 配布ビルド構成（M2 達成）

対象: `crates/noiria-app/Cargo.toml`（feature 定義）、`crates/noiria-app/src/script/mod.rs`
（engine-v8 gate）、`crates/noiria-app/src/script/runtime_modules.rs`（新設、`loader.rs`
から `RUNTIME_MODULES` を切り出し）、`crates/noiria-app/src/script/{loader,ops_*,state,util}.rs`
（engine-v8 feature gate）、`crates/noiria-app/src/script/engine_state.rs`（`trans_notify`
の dead_code 許可）、`crates/noiria-app/src/main.rs`（`spawn_script` 4 分岐・
`windows_subsystem`・dist 既定ゲームパス探索）、`crates/noiria-app/src/dist_logger.rs`
（新設、自前ファイルロガー）、`crates/noiria-core/Cargo.toml`+`src/assets.rs`（`dist`
feature で `default_engine()` を切替）、`crates/noiria-core/src/pak.rs`
（`key_slot_magic()` を公開）、`crates/noiria-tools/src/export.rs`（新設、export 本体）、
`crates/noiria-tools/src/bin/noiria_export.rs`（新設 CLI）、
`crates/noiria-tools/src/third_party.rs`+`THIRD-PARTY.txt`（新設）、
`crates/noiria-tools/tests/third_party_check.rs`（新設）、`crates/noiria-tools/Cargo.toml`
（`serde_json` 依存追加、`noiria-export` bin 追加）、`Cargo.toml`（`[profile.dist]` 追加）、
`scripts/build-dist-wsl.sh`+`scripts/feature-build-matrix.ps1`（新設）、
`docs/distribution.md`（新設）。ベースラインは Windows `cargo test --workspace` 318 件 /
WSL 328 件全通過。

### 1. feature 設計と engine-v8/engine-tsint の分離

- **判断**: `noiria-app/Cargo.toml` に `default = ["engine-v8", "engine-tsint"]` /
  `engine-v8 = ["dep:deno_core", "dep:deno_ast", "dep:deno_error"]` /
  `engine-tsint = ["dep:tsint"]` / `dist = ["noiria-core/dist"]` を計画通り追加した。
  `tokio`/`env_logger` は計画の指示通り gate しない（tokio は tsint 版の駆動ループには
  使わないが `noiria-core::audio` 等が共通で使うため。env_logger は dist ビルドでは
  未初期化のまま単に使われないだけで、依存として残しても数十 KB 程度でサイズへの
  影響は無視できる。除去は「求められていない最適化」と判断し行わなかった）。
- **判断**: V8 専用のファイル群（`loader.rs`/`ops_audio.rs`/`ops_input.rs`/
  `ops_layer.rs`/`ops_sys.rs`/`ops_text.rs`/`ops_trans.rs`/`ops_video.rs`/`state.rs`/
  `util.rs`）はモジュール宣言に `#[cfg(feature = "engine-v8")]` を付けるだけで済んだ
  （中身の変更は不要）。**理由**: これらは元々 `docs/implementation-notes.md` の
  各ウェーブ注記にある通り「実処理は `engine_state::EngineState`（エンジン非依存）に
  委譲し、ops ファイル自体は V8 の `#[op2]`/`OpState` 型変換の薄いラッパに徹する」
  という設計方針で書かれていたため、V8 依存が既に 1 レイヤに閉じ込められていた
  （W1〜W6 で確立された設計の副産物）。
- **判断**: `loader.rs` が持っていた `RUNTIME_MODULES`（`noiria:` 埋め込み TS 一覧）を
  新設 `runtime_modules.rs` へ切り出し、gate しない（常にコンパイル）モジュールにした。
  **理由**: `tsint_engine.rs`（engine-tsint）と `loader.rs`（engine-v8）の両方が同じ
  テーブルを参照するため、どちらか一方の feature だけを有効にしたビルドでも
  コンパイルできる場所に置く必要があった。**影響**: `loader.rs` 内の
  `runtime_modules_all_transpile` テストは `#[cfg(test)]` のままなので default
  ビルド（両エンジン有効）でのみ走る。tsint 側の同等テスト
  （`tsint_can_parse_all_embedded_runtime_modules`）は元から `runtime_modules.rs`
  ではなく `tsint_engine.rs` 内にあるので、engine-v8 が無いビルドでもテーブル自体の
  健全性は tsint 側の検証で担保される（ただし `#[cfg(test)]` なので `cargo build`
  だけを見る feature ビルドマトリクスでは検証対象外。テストまで両方の feature 組み
  合わせで回すことは今回のスコープ外と判断した — 「検証」節参照）。
- **判断**: `tag_behavior_tests.rs`/`tag_harness.rs`（V8/tsint 両方の `spawn` を使う
  タグ振る舞いテスト）は `#[cfg(all(test, feature = "engine-v8", feature =
  "engine-tsint"))]` にした。`mod.rs` の `js_tests` も `#[cfg(all(test, feature =
  "engine-v8"))]` にした。**理由**: 計画の「default ビルドの開発フロー・V8/tsint ops
  列一致テスト・タグ振る舞いテストは完全不変」を字面通り満たしつつ、片方の engine
  feature だけを落としたビルドで参照先シンボルが無くなってコンパイルエラーになる
  事態を防ぐ。**影響**: default 以外の feature 組み合わせで `cargo test` を叩くと
  これらのモジュール自体が対象外になる（`cargo build`（非 test）では元々
  `#[cfg(test)]` 配下は一切コンパイルされないので、feature ビルドマトリクスの
  「ビルド通過」確認には最初から影響しない）。
- **判断**: `EngineState::trans_notify` フィールドに
  `#[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]` を付けた。**理由**:
  V8 版 `op_trans_wait`（`ops_trans.rs`）だけがこのフィールドを読み、tsint 版は
  自前のポーリングループ（`tsint_engine::PendingOps::trans`）で待つ設計のため、
  engine-v8 を無効化したビルドでは元から未使用だった。CLAUDE.md の「自分の変更で
  発生した orphan は片付ける」方針に従い、feature gate によって新たに dead_code に
  なったこのフィールドだけ許可属性を付けた（既存の無関係な dead_code 警告には
  触れていない）。
- **判断**: `main.rs::spawn_script` は 1 つの関数を `#[cfg(...)]` で分岐させる形では
  なく、feature 組み合わせごとに **別関数を 4 つ** 用意した（両方有効/v8のみ/
  tsint のみ/どちらも無効）。**理由**: 1 関数内で match アームを `#[cfg]` すると
  無効化された分岐が `unreachable pattern` 等の警告や `compile_error!` の扱いが
  読みにくくなる。関数ごと分けた方が「このビルドでは何が起きるか」を素直に追える。
  要求されたエンジンが無効ビルドの場合は **panic ではなく警告 + もう一方への
  フォールバック**にした（計画 2 節の「v8 要求 + v8 無効ビルドなら警告して tsint
  へ」を汎用化し、tsint 要求時に engine-tsint が無いビルドでも対称的に警告 + v8
  フォールバックとした）。両方無効という組み合わせだけは `compile_error!`
  にした（実用上あり得ない構成のため、実行時エラーではなくビルド時に弾く）。

### 2. dist 時の挙動

- **判断**: `windows_subsystem="windows"` は `#![cfg_attr(all(windows, feature =
  "dist"), windows_subsystem = "windows")]` として `main.rs` 先頭（doc コメント直後）
  に置いた。計画通り。
- **判断**: 自前ファイルロガー（`dist_logger.rs`、`log::Log` 実装）は「exe 隣接
  `noiria-startup.log` から開始し、`GameDir::open` 成功後にセーブディレクトリの
  `noiria.log` へ切り替える」という 2 段階方式にした。**理由**: `game_id` が
  確定するのは `GameDir::open` の戻り値（`GameConfig::id`）を見た後だが、
  `log::set_logger` は プロセス中 1 回しか呼べないため、`game_id` 確定前のログ
  （典型的には `GameDir::open` 自体の失敗）を捨てずに残すには「後から書き込み先を
  差し替えられる」ロガーが必要だった。`Mutex<Option<File>>` を 1 個持ち、
  `switch_to_game_log(game_id)` が保存先ディレクトリを解決できたときだけ
  差し替える（失敗時は exe 隣接ログのまま続行）。ファイルを一切開けない異常環境
  （読み取り専用メディア等）では stderr へフォールバックする（`windows_subsystem`
  下では誰にも見えないが、ログを完全に失うよりましという判断）。**影響**:
  実機検証（後述）で `%APPDATA%\Noiria\<game_id>\save\noiria.log`
  （Windows、`directories::ProjectDirs::from("", "Noiria", game_id)` の解決先）
  ・`~/.local/share/<game_id>/save/noiria.log`（Linux）の両方に正しくログが
  切り替わることを確認した。ログレベルは環境変数 `NOIRIA_LOG`（既定 `warn`）。
  タイムスタンプは `SystemTime` の UNIX 秒 + ミリ秒（`[1783856908.211]`
  形式）にした。**理由**: 自前の暦計算（年月日への変換）を書く価値が低いと判断
  （CLAUDE.md 簡潔性方針。ログの用途は時系列の相対比較で十分）。
- **判断**: 既定ゲームパス探索（`main.rs::dist_default_game_path`）は計画通り
  ①exe 隣接 `game.pak` ②exe 隣接 `game/` の順で探し、どちらも無ければ
  `anyhow::bail!` で明確なエラーにした（`games/sample` フォールバックは
  `dist` feature 時は行わない）。位置引数でゲームパスが明示された場合のみ
  この探索をスキップする `Args::game_path_explicit` フラグを追加した。
- **判断**: 既定エンジンの tsint 化は `noiria-core` 側 `GameConfig::default_engine()`
  を `#[cfg(feature = "dist")]` で 2 通りに分けることで実現した（`noiria-app`
  の `dist` feature が `noiria-core/dist` を connect する）。**理由**: `engine`
  フィールドが `game.json` に無い場合の serde デフォルト値そのものを変えるのが
  一番シンプルで、`main.rs` 側に「これは明示指定か既定値か」を別途追跡する
  仕組み（`Option<String>` 化等）を持ち込まずに済む。**影響**: `game.json` に
  `"engine": "v8"` と明示されたゲームを dist（tsint のみ）ビルドで開いた場合は
  `spawn_script` の v8 リクエスト時フォールバック（1 節参照）で tsint へ警告付きで
  切り替わる。

### 3. `noiria-export`

- **判断**: 鍵生成は計画通り Windows は `bcrypt.dll` の `ProcessPrng` を
  `#[link(name = "bcrypt")] extern "system"` で直接宣言、それ以外は `/dev/urandom`
  読み取りにした（`windows` クレートには依存しない。`noiria-tools` は元々
  `windows` クレートに依存していないため、新規依存を増やさない選択）。単体テストは
  「2 回連続で異なる鍵が出る」という統計的性質のみ確認（暗号学的な乱数性の
  証明はテストの役割ではないため、素朴な不一致チェックに留めた）。
- **判断**: `KEY_SLOT` のマジック定数 `KEY_SLOT_MAGIC`（`pak.rs`、W6 で private）を
  `pub fn key_slot_magic() -> [u8; 16]` として公開した。**理由**: `noiria-export`
  がバイナリパッチに使う値をハードコードで重複させると、将来どちらかだけ変更されて
  ズレるリスクがある（DRY）。値そのもの（`b"NOIRIA-KEYSLOT1\0"`）は変えていない。
- **判断**: `patch_key_slot` はマジックのバイト列一致を素朴な O(n) 線形走査で探す
  （高速化のためのハッシュ探索等は行わない）。**理由**: exe サイズは数 MB 級であり
  1 回の export（バイナリ 1〜2 本）で線形走査しても体感上問題にならない
  （実測: 2.2MB/2.5MB のバイナリで export 全体が 1 秒未満）。
- **判断**: `validate_game` は `game.json` の `title`/`id` が空文字でないことのみを
  「必須項目」チェックとした（`main`/`width`/`height` は `GameConfig` 側で既に
  serde デフォルトがあるため常に妥当な値になる）。`.mp4` アセットはディレクトリを
  再帰走査して相対パス一覧を集め、export 自体は失敗させずに警告として
  `ExportReport::mp4_warnings` 経由で CLI に返す（計画の「mp4 は pak 非対応の警告」
  通り、エラーではなく警告）。
- **判断**: ターゲットごとのビルド済みバイナリが見つからない場合は
  （`--bin-windows`/`--bin-linux` 明示時・既定 `target/dist/` 探索時のどちらでも）
  即座に `bail!` する（該当ターゲットだけ skip して他方だけ export、という
  部分成功は行わない）。**理由**: `--targets` で明示的に要求されたターゲットが
  静かに欠落する方が、コマンドの exit code だけを見る自動化パイプラインにとって
  危険（「両方 export できた」と誤認しうる）。エラーメッセージに
  `cargo build --profile dist ...` の具体的なコマンド例と `--bin-<target>` の
  存在を明記し、原因と対処が exit 前に分かるようにした。
- **判断**: 出力先の実行ファイルへ書き込み後、Unix では `fs::set_permissions` で
  実行ビットを立てる（`#[cfg(unix)]`）。Windows では何もしない（NTFS に unix
  パーミッションの概念が無いため）。**影響**: 実機検証では WSL の DrvFs
  （`/mnt/c` 経由）が既定で world-executable 相当のメタデータを返したため
  この分岐が無くても実行できてしまったが、実 Linux ファイルシステム（ext4 等）
  へ配布物を展開した場合はこの実行ビット付与が効いてくる想定。
- **判断**: README.txt はゲームタイトル・起動方法（Windows/Linux 両方）・ログの
  場所と `NOIRIA_LOG`・THIRD-PARTY.txt への参照を固定テンプレートで生成する
  （ゲームごとのカスタマイズ項目は無し）。**理由**: 計画が「自動生成」とだけ
  指定しており、テンプレート化以上の複雑さは不要と判断（CLAUDE.md 簡潔性方針）。

### 4. THIRD-PARTY.txt

- **判断**: 内容は手作業での列挙ではなく、実際に `cargo metadata
  --no-default-features --features engine-tsint,dist --manifest-path
  crates/noiria-app/Cargo.toml --format-version=1 --filter-platform
  <x86_64-pc-windows-msvc|x86_64-unknown-linux-gnu>` を実行し、resolve グラフを
  BFS で辿って収集した実際の依存（normal + build 依存、dev-dependencies 除外、
  ワークスペース内 path 依存は対象外）から作成した。**理由**: 計画の例示
  （「anyhow/serde/serde_json/log/crossbeam-channel/tokio/directories + 主要
  transitive」）は代表例であり、実際の依存グラフ（Windows/Linux 合算で
  crates.io 由来 56 crate）を手作業で正確に再現するのは非現実的かつ間違いの
  元だったため、実測値を正とした。
- **判断**: ライセンス全文は MIT・Apache-2.0 の 2 種類のみをフルテキストで
  同梱し、各クレートがどちらに該当するか一覧化する形式にした（実際の著作権者は
  クレートごとに異なるため「各クレートの正確な著作権者はソース同梱の LICENSE
  ファイルを参照」と明記）。**理由/影響**: 依存の大半（56 中 53）は
  `MIT OR Apache-2.0`（または `Unlicense OR MIT`）のデュアルライセンスで
  この 2 種の全文があれば足りる。ただし計画の想定外だった 2 件を追加した:
  ①`option-ext`（`dirs-sys` の Windows 向け依存）が **MPL-2.0** だったため、
  全文埋め込みの代わりに Mozilla 公式 URL への参照 + 改変していない旨を明記する
  形にした（MPL は全文埋め込みを必須要求しないため、実用的な軽量対応とした）。
  ②`unicode-ident` が `(MIT OR Apache-2.0) AND Unicode-3.0` （文字データ表部分に
  Unicode License v3 が追加適用）だったため、同様に URL 参照を追記した。
- **判断**: 陳腐化防止テスト（`tests/third_party_check.rs`）は「クレート名が
  `THIRD-PARTY.txt` 中に単語境界付きで出現するか」という緩い文字列マッチで
  検査する（バージョン番号やライセンス種別の完全一致までは検査しない）。
  **理由**: 過度に厳格な検査にすると、依存バージョンが patch レベルで上がる
  だけで無関係な CI 失敗を起こす（`Cargo.lock` の通常の更新のたびに
  `THIRD-PARTY.txt` の手動更新を強制するのは過剰）。クレート名の欠落だけを
  確実に検出できれば「新しい依存を追加したのに記載を忘れた」という本来
  防ぎたい事態は防げる。

### 5. WSL ビルド + ドキュメント

- **判断**: `scripts/build-dist-wsl.sh` は `scripts/wsl-test.sh` と同じ
  `CARGO_TARGET_DIR=$HOME/.noiria-target` 退避パターンを踏襲し、最終バイナリ
  だけを `/mnt/c` 側（Windows から見える）`target/dist/noiria` へコピーする。
  **理由**: `noiria-export` の既定バイナリ探索パス（`target/dist/noiria(.exe)`）
  と一致させることで、`--bin-linux` を省略した素朴な運用が成立する。
- **判断**: `docs/distribution.md` は「Windows で dist ビルド → WSL で dist
  ビルド → export」の 3 手順構成にした。export はゲームごとに毎回実行するが
  ビルドは（KEY_SLOT パッチ方式のおかげで）最初の 1 回だけでよいことを明記した。

### 6. アイコン（ストレッチ目標）— 見送り

- **判断**: Windows `.res` 自前生成 + Linux `_NET_WM_ICON` 配線は本ウェーブでは
  **実装を見送った**（計画で「時間超過なら省略して申し送り」と明示されていた
  ストレッチ目標）。**理由**: W7 の必須スコープ（feature 分離・export・
  KEY_SLOT パッチ・THIRD-PARTY・WSL ビルド手順・多段検証）だけで実装/検証量が
  大きく、アイコンは「配布可能」という M2 の当初目標（動作保証 + 軽量化 +
  資産保護）には含まれない付加価値と判断した。**影響**: 配布物の exe/バイナリは
  既定のアイコン（Windows は無印の exe アイコン、Linux は WM 既定）のまま。
  次ウェーブ（またはユーザー要望時）で着手する場合、Windows 側は `build.rs` で
  `.ico` → `.res` 変換 + `cargo:rustc-link-arg-bins=/path/to/icon.res` の
  組み込み、Linux 側は W2 で実装済みの `ChangeProperty`（`x11::proto`）を
  `_NET_WM_ICON`（CARDINAL 配列、ARGB pixel data）向けに再利用する形になる見込み。

### 検証結果 (2026-07-12, W7)

- **feature ビルドマトリクス**（`scripts/feature-build-matrix.ps1`、Windows）:
  ① default（engine-v8 + engine-tsint）② `--no-default-features --features
  engine-v8`（v8 のみ）③ `--no-default-features --features engine-tsint,dist
  --profile dist`（配布ビルド本体）の 3 通り、すべて `cargo build` 成功
  （warning 0 件、noiria-app 由来）。②を追加検証したのは、tsint 側だけでなく
  v8 側も gate 単独で壊れていないことを対称的に確認するため（計画の
  「`--features engine-v8`」を `--no-default-features` 付きの意味だと解釈した
  — 素の `--features engine-v8` は default に何も足さない no-op になるため）。
  ③は WSL でも `scripts/build-dist-wsl.sh` 経由で実行し成功を確認した。
- **dist バイナリサイズ**: Windows `target/dist/noiria.exe` = **2,238,976 バイト
  （約 2.14 MiB）**、Linux `target/dist/noiria` = **2,667,856 バイト（約 2.54
  MiB）**。V8 込みの開発ビルド（約 60MB、ユーザー要求時の見積り）比で
  **約 96% 削減**。両方とも `[profile.dist]`（`lto=thin`+`strip=symbols`+
  `panic=abort`）適用済み。`panic=abort` は事前に `grep -rn catch_unwind
  crates/` で該当ゼロを確認してから採用した（計画の事前確認事項）。
- **export 実行**（`games/sample` → `target/export/{windows,linux}`）:
  エントリ数 23、元サイズ 23,760,978 バイト → pak サイズ 22,756,628 バイト
  （W6 の pak サイズ実測と同値、鍵が変わっても暗号化 pak のサイズは不変である
  ことを裏付ける）。`.mp4`（`assets/video/demo.mp4`）に対する pak 非対応警告が
  期待通り 1 件出力された。生成物一覧: `{windows,linux}/{Noiria(.exe),
  game.pak, THIRD-PARTY.txt, README.txt}`（exe 名は `game.json` の
  `"Noiria サンプル"` を正規化した `Noiria`）。
- **export E2E — Windows**: `target/export/windows/Noiria.exe --autotest 24`
  を**引数なし・exe 隣接に game.pak がある状態**で実行（既定ゲームパス探索の
  経路）し、EXIT=0・PNG 16 枚出力（`frame_00.png`〜`frame_final.png`、W6 の
  `--autotest 24` 実測と同枚数）を確認。ログファイルは
  `%APPDATA%\Noiria\noiria-sample\save\noiria.log`
  に生成され、`スクリプトエンジン: tsint（自作 TS インタプリタ）` 等の想定ログが
  記録されていることを確認（既定ログレベル `warn` のため info ログは出ていない
  のが正しい挙動）。窓は一切開かない（`--autotest` はヘッドレス）。
- **export E2E — WSL**: 同じ export 済み `target/export/linux/Noiria` を
  `--autotest 24` で実行し、EXIT=0・PNG 16 枚を確認（`~/.local/share/
  noiria-sample/save/noiria.log` にログ確認）。続けて Xvfb（隔離ディスプレイ
  `:99`、`scripts/wsl-test.sh` と同じ非特権名前空間分離 + vendored Xvfb で
  実デスクトップに触れない方式）上で `./Noiria`（引数なし、既定ゲームパス
  探索）を `timeout --kill-after=5 12` で 10 秒強 windowed 実行し、ログに
  `X11: ウィンドウ作成 wid=0x00200000 (1280x720)` を確認、panic なし
  （`timeout` による正常な強制終了 EXIT=124 のみ）。ALSA (`libasound`) は
  この検証環境に用意できなかったため「オーディオデバイスを開けません
  （音なしで続行）」の想定内フォールバックとなった（W3 で確立済みの既存挙動、
  W7 由来の問題ではない）。
- **KEY_SLOT パッチ統合テスト**: 上記 Windows export E2E がそのまま
  「パッチ済み exe が固有鍵 pak を開ける」ことの実証になっている。加えて
  **未パッチの開発鍵 exe**（`target/dist/noiria.exe`、export でコピー・
  パッチされる前の元バイナリ）で同じ `target/export/windows/game.pak`
  （固有鍵）を開こうとしたところ、`Error: pak を開けません: ... Caused by:
  鍵が違います。この pak を復号できません（key_check 不一致）` で **EXIT=1**
  （パニックなし）を確認した。`crates/noiria-tools/src/export.rs` の単体テスト
  `patch_key_slot_rejects_missing_and_duplicate_magic` でマジック 0 個/2 個の
  異常系も検証済み。
- **Windows**: `cargo test --workspace` 318→**322 件**全通過（新規 4 件:
  `noiria-tools` の `export::tests::{sanitize_exe_name_keeps_ascii_and_replaces_space,
  generate_key_is_random_and_32_bytes, patch_key_slot_rejects_missing_and_duplicate_magic}`
  3 件 + 統合テスト `third_party_check::third_party_txt_lists_every_dist_build_dependency`
  1 件）。**clippy**: `cargo clippy --workspace --all-targets` — 新規/変更ファイル
  （`noiria-app` 全体、`noiria-tools/src/export.rs`、
  `noiria-tools/src/bin/noiria_export.rs`、`noiria-tools/tests/third_party_check.rs`）
  は warning 0 件。既存の無関係 warning 12 件（W6 以前からの `noiria-tools` 内、
  `analysis.rs`/`synth.rs`/`paint.rs`/`examples/analyze_bgm.rs`/
  `tests/audio_quality.rs`）は件数不変のまま残存。
- **WSL**（`CARGO_TARGET_DIR=$HOME/.noiria-target`）: `cargo test --workspace`
  328→**332 件**全通過（新規 4 件、Windows と同一テスト名。純粋 Rust のため
  プラットフォーム差分なし）。

### 変更/新規ファイル一覧

- `Cargo.toml`: `[profile.dist]`（inherits=release, lto=thin, strip=symbols,
  panic=abort）+ `[profile.dist.package.vp9dec]`（opt-level=2、release と同じ
  スタックオーバーフロー回避）追加。
- `crates/noiria-app/Cargo.toml`: `[features]` 追加（default/engine-v8/
  engine-tsint/dist）、deno_core/deno_ast/deno_error/tsint を `optional = true` 化。
- `crates/noiria-app/src/main.rs`: `windows_subsystem` cfg_attr、
  `dist_logger` mod 追加、`Args::game_path_explicit`、
  `dist_default_game_path`、`spawn_script` の 4 feature 分岐、
  main() の logger 初期化/既定パス解決/ログ切替呼び出し。
- `crates/noiria-app/src/dist_logger.rs`（新規）: 自前ファイルロガー。
- `crates/noiria-app/src/script/mod.rs`: サブモジュール宣言・`extension!`/
  `spawn`/`run_script`/`js_tests` を `engine-v8` feature gate。
- `crates/noiria-app/src/script/runtime_modules.rs`（新規）: `RUNTIME_MODULES`
  切り出し（gate なし）。
- `crates/noiria-app/src/script/loader.rs`: `RUNTIME_MODULES` を
  `runtime_modules` から import する形に変更（値そのものは不変）。
- `crates/noiria-app/src/script/{ops_audio,ops_input,ops_layer,ops_sys,
  ops_text,ops_trans,ops_video,state,util}.rs`: モジュール宣言側を
  `engine-v8` feature gate（ファイル自体は無変更）。
- `crates/noiria-app/src/script/engine_state.rs`: `trans_notify` に
  feature 条件付き `allow(dead_code)`。
- `crates/noiria-app/src/script/tsint_engine.rs`: `RUNTIME_MODULES` の
  import 元を `loader` から `runtime_modules` に変更。
- `crates/noiria-core/Cargo.toml`: `[features] dist = []` 追加。
- `crates/noiria-core/src/assets.rs`: `default_engine()` を `dist` feature で
  2 通りに分岐（v8/tsint）。
- `crates/noiria-core/src/pak.rs`: `key_slot_magic()` を公開。
- `crates/noiria-tools/Cargo.toml`: `serde_json` 依存 + `noiria-export` bin 追加。
- `crates/noiria-tools/src/lib.rs`: `pub mod export;`/`pub mod third_party;` 追加。
- `crates/noiria-tools/src/export.rs`（新規）: export 本体
  （検証/鍵生成/KEY_SLOT パッチ/exe 名正規化/README 生成/書き出し）+ 単体テスト 3 件。
- `crates/noiria-tools/src/bin/noiria_export.rs`（新規）: `noiria-export` CLI。
- `crates/noiria-tools/src/third_party.rs`（新規）: `THIRD-PARTY.txt` の
  `include_str!` 埋め込み。
- `crates/noiria-tools/THIRD-PARTY.txt`（新規）: 依存ライセンス表記。
- `crates/noiria-tools/tests/third_party_check.rs`（新規）: `cargo metadata`
  突き合わせテスト。
- `scripts/build-dist-wsl.sh`（新規）: WSL 内 dist ビルド + 配置スクリプト。
- `scripts/feature-build-matrix.ps1`（新規）: feature ビルドマトリクス検証。
- `docs/distribution.md`（新規）: 配布手順書。

### 申し送り・未解決事項

- **アイコン**（6 節参照）: ストレッチ目標のため未実装。次ウェーブ以降で
  着手可能（設計方針は 6 節に記載）。
- **`--targets` の部分成功を許さない設計**（3 節）: 両ターゲットを要求して
  片方のビルド済みバイナリが無い場合は全体が失敗する。CI 等で「Windows だけ
  先に export したい」場合は明示的に `--targets windows` を使う必要がある
  （現状の CLI で対応可能、ドキュメント化済み）。
- **feature 単体の `cargo test`**: 本ウェーブの検証は default feature での
  `cargo test`（両エンジンのテストが全部走る）と、他 feature 組み合わせでの
  `cargo build`（ビルド可否のみ）に限定した。`--no-default-features --features
  engine-tsint` 単体で `cargo test` を回す運用は今回のスコープ外
  （tag_behavior_tests/tag_harness/js_tests は前述の通り両 feature 前提の
  `#[cfg]` にしてあるため、そのようなテスト実行自体がそもそも該当テストを
  含まない安全な形になっている）。
- **M2 宣言の可否**: 判断材料は揃った——①暗号化 pak (W6) ②V8 抜きの tsint
  専用軽量 exe（Windows 2.14MiB / Linux 2.54MiB、V8 込み ~60MB 比で約 96% 削減）
  ③Windows/Linux 両ターゲットへの export が実機 E2E（autotest PNG・ログ・
  windowed Xvfb スモーク）で確認済み ④KEY_SLOT による鍵不一致の明確な
  拒否を確認——のため、**M2（配布対応の当初目標達成）を宣言できる状態と判断する**。
  唯一の留保はアイコン（6 節、当初目標のスコープ外と判断した付加価値）と、
  実 Linux ディストリビューション（ext4 上、WSL の DrvFs 経由ではない環境）
  での動作確認が本ウェーブでは行えていない点（WSL2 の Xvfb/autotest 検証で
  代替、`docs/linux-dev.md`/計画の検証体系まとめに沿った既存の代替方針を踏襲）。

## 2026-07-12 配布対応 W8

WebP (VP8L ロスレス) 対応。計画方針変更（自作 VP8L ではなく `image-webp` 採用、
ユーザー承認 2026-07-12）に従い、`crates/noiria-core/src/codec/webp.rs` の 1 モジュールへ
隔離した。ベースラインは Windows cargo test 322 件 / WSL 332 件全通過（W7 まで完了）。

### 1. 隔離設計と依存追加

- **判断**: `codec/webp.rs` の公開 API を `decode_rgba(bytes) -> Result<(u32,u32,Vec<u8>)>` /
  `encode_lossless(w,h,rgba) -> Result<Vec<u8>>` の 2 関数だけにし、`image_webp::{WebPDecoder,
  WebPEncoder,ColorType}` はこのファイルの外へ一切出さない。**理由**: 初期の `image`/`rodio`
  採用時と同じ「外部依存は 1 モジュール隔離 + 安定 ABI + いずれ自作置換可能」の道筋を踏襲する
  （計画で明示されたユーザー方針）。将来自作 VP8L 実装に差し替える際もこのファイルの中身だけ
  書き換えれば済む。
- **依存追加**: `image-webp = "0.2"`（実際に解決されたバージョンは 0.2.4）を workspace
  依存に追加し、`noiria-core` の通常依存へ。`image-webp` は `#![forbid(unsafe_code)]` の
  純 Rust クレート（image-rs 公式、C 依存・FFI 一切なし）で、直接の依存は
  `byteorder-lite`（バイトオーダー変換のみ）と `quick-error`（エラー型マクロ）の 2 パッケージ
  のみ、いずれも葉ノード（さらなる推移依存なし）。`cargo tree -p image-webp@0.2.4`:

  ```text
  image-webp v0.2.4
  ├── byteorder-lite v0.1.0
  └── quick-error v2.0.1
  ```

  Windows/Linux クロスビルドへの影響なし（OS 境界 API・C リンクを一切要求しない）。
- **dev-dependency**: `noiria-core` の既存 dev-dep `image`（`codec/png.rs`/`codec/jpeg.rs` の
  クロスチェック用）に `webp` feature を追加した。`image` 0.25 の `webp` feature も内部で
  同じ `image-webp` を使っているため、クロスチェックは「別実装との照合」ではなく「別の
  呼び出しコード経由で同じデコーダ/エンコーダを叩いた結果が一致するか」の検証になる
  （それでも API 誤用・バッファサイズ計算ミス等の統合バグは十分検出できる。png.rs の既存
  クロスチェックほど強い保証ではない点は明記しておく）。

### 2. デコード/エンコードの実装判断

- **アルファ有無の分岐**: `image_webp::WebPDecoder::read_image` は `has_alpha()` が false の
  場合 RGB8（3 バイト/px）を返す仕様のため、`decode_rgba` は不透明画像を alpha=255 で
  RGBA8 へ詰め直してから返す（呼び出し側には常に `(w,h,rgba)` の straight RGBA8 という
  一枚岩の契約を保つ。PNG/JPEG デコーダと同じ形）。
- **エンコードはロスレスのみ**: `image_webp::WebPEncoder` は VP8L（ロスレス）専用（ロッシー
  VP8 エンコードには非対応）。pack が必要とするのもロスレス変換のみなので `encode_lossless`
  という関数名にして「ロッシーは作れない」ことを型ではなく名前で明示した。
- **ロッシー (VP8) デコードの備考**: `image_webp` はロッシー VP8 用のデコーダ
  （`vp8.rs`、YUV→RGB 変換・loop filter 込み）も同梱しており、`decode_rgba` はロスレス/ロッシー
  どちらの WebP バイト列を渡しても分岐なくデコードできる（`WebPDecoder::read_image` が
  内部で VP8L/VP8 チャンクを判別する）。ただし pack/export が生成するのは常にロスレスのみで、
  手元にロッシー WebP の実サンプルが無かったため**ロッシーデコードの経験的な実機検証は
  行っていない**（コード読解による確認のみ。ユーザーが手動でロッシー WebP をゲーム素材として
  持ち込んだ場合の動作は未検証、申し送り事項として下記に記載）。アニメーション WebP は
  先頭フレームのみ、ICC/EXIF 等のメタデータは読み飛ばす（`image_webp` の挙動どおり）。

### 3. `assets.rs`: マジックバイトスニッフィング

- **判断**: `decode_image_bytes` に `RIFF`(先頭4B) + `WEBP`(8..12B) のマジックバイト判定を
  追加した。拡張子ではなく中身で判定する。**理由**: pack が `.png` パスのまま中身だけ WebP に
  差し替える設計（次節）のため、拡張子判定だと壊れる。既存の PNG (`\x89PNG`) / JPEG
  (`\xFF\xD8`) の判定方式と揃えた（後方互換、既存分岐は無変更）。

### 4. pack/export 統合: 小さい方を自動採用

- **判断**: `noiria-tools/src/pack.rs::pack_dir` に `webp: bool` 引数を追加し、true のとき
  `.png` エントリ（拡張子で判定、大小文字無視）を `noiria_core::codec::png::decode` →
  `codec::webp::encode_lossless` で変換し、**変換結果が元 PNG より小さい場合だけ**差し替える
  （サイズ悪化ゼロ保証）。パス・拡張子は `.png` のまま pak に格納する（`pak::is_precompressed_ext`
  は既に `"webp"` も stored 対象に含めていたが、今回はパスを変えないので `"png"` 側の
  既存ルールがそのまま効く。中身が WebP でも stored 判定は不変）。デコード/エンコードいずれかが
  失敗した場合（非対応 PNG 亜種など）は `None` を返して元 PNG バイト列のまま使う
  フォールバックにした（1 エントリの変換失敗で pack 全体を失敗させないため）。
- **`PackReport` への 2 フィールド追加**: `webp_converted_count`/`webp_bytes_saved`
  （`noiria_core::pak::PackReport`）。`pak::build()` 自体は画像フォーマットを一切意識しない
  設計を維持したまま（既存の関心分離を壊さない）、`pack_dir` が `build()` 呼び出し後にこの
  2 フィールドだけ上書きする形にした。
- **CLI**: `noiria-pack`/`noiria-export` 両方に `--no-webp` を追加（既定は変換 ON）。
  `noiria-pack` は変換件数・削減バイト数を標準出力に追加表示、`noiria-export` の
  `ExportReport` にも同じ 2 フィールドを追加した。
- **既知の注意点（`total_plain_len`/「元サイズ」表示の意味）**: `PackReport::total_plain_len`
  は「pack 時に実際に格納しようとしたバイト列の合計」であり、WebP 変換が有効なときは
  **変換後（WebP 採用分は WebP バイト長）の合計**になる（この挙動は W8 以前から
  `compress_for_entry` 呼び出し前の `plain.len()` を積算する実装のままで、pack.rs が
  entries を webp で置換してから `build()` に渡す設計上、自然にそうなる）。つまり CLI の
  「元サイズ」表示は「pack 対象として最終的に採用されたバイト列の合計」であって「ディスク上の
  元ファイルサイズ合計」ではない（webp 有効時は両者が乖離する）。新しくフィールドを追加して
  意味を変えるより、既存の `webp_converted_count`/`webp_bytes_saved` で削減量を別途出す方が
  シンプルと判断し、`total_plain_len` 自体の意味は変更しなかった（CLAUDE.md の「単純に済む
  ところを複雑にしない」方針）。

### 5. サイズ計測結果

`games/sample/assets` の PNG 10 ファイル全てで WebP ロスレスの方が小さかった
（`decode_rgba`→`encode_lossless` の round-trip はピクセル完全一致、下記検証参照）:

| ファイル | PNG | WebP | 比率 |
| --- | ---: | ---: | ---: |
| bg/room.png (1280x720) | 982,589 | 582,220 | 59.3% |
| bg/street.png (1280x720) | 627,108 | 413,492 | 65.9% |
| fg/alice_normal.png (500x900) | 14,114 | 1,970 | 14.0% |
| fg/alice_smile.png (500x900) | 14,593 | 2,206 | 15.1% |
| fg/btn_menu.png (360x44) | 3,511 | 2,914 | 83.0% |
| fg/icon_blink_a.png (28x28) | 1,350 | 1,140 | 84.4% |
| fg/icon_blink_b.png (28x28) | 898 | 846 | 94.2% |
| fg/icon_spark.png (28x28) | 1,350 | 1,140 | 84.4% |
| rule/wipe.png (1280x720) | 20,297 | 692 | 3.4% |
| rule/circle.png (1280x720) | 419,925 | 147,568 | 35.1% |
| **合計** | **2,085,735** | **1,154,188** | **55.3%** |

PNG が自作エンコーダ（stored ブロックのみ、行フィルタなし）のため、WebP ロスレスの予測符号化+
エントロピー符号化との差が大きく出ている（特に `rule/wipe.png` は単色に近く 96% 削減）。
`games/sample` 全体（23 エントリ、WAV が支配的）の pak サイズ:

| | エントリ数 | pak サイズ | 備考 |
| --- | ---: | ---: | --- |
| `--no-webp` | 23 | 22,756,628 バイト (元 23,760,978 に対し 95.8%) | W6/W7 実測と同値 |
| webp 有効（既定） | 23 | 21,825,075 バイト | PNG 10/10 件が WebP を採用、931,547 バイト削減 |

pak 全体では約 **4.1%** の削減（画像が全体の一部で、WAV が大半を占めるため。画像部分だけ見ると
上表の通り約 45% 削減）。

### 6. 検証結果

- **round-trip**（`codec/webp.rs` 内テスト）: 合成パターン（市松模様+半透明、単色 1x1、
  グラデーション、非 4 の倍数サイズ）+ `games/sample` の PNG 10 ファイル全てで
  `encode_lossless` → `decode_rgba` のピクセル完全一致を確認（`round_trip_all_sample_pngs`
  含む）。
- **クロスチェック**: `image` クレート（dev-dep、`webp` feature）で①自作エンコード結果を
  デコードし一致、②`image` クレート側でエンコードした結果を自作デコーダで読んで一致、の
  双方向を確認（`cross_check_our_encode_decodes_with_image_crate`/
  `cross_check_image_crate_encode_decodes_with_us`）。
- **assets.rs マジックバイト判定**: PNG バイト列にラベルだけ `.png` を付けた WebP を渡しても
  正しく WebP としてデコードされることを確認（`decode_image_bytes_sniffs_webp_by_magic_regardless_of_label`）。
- **pack_dir 単体**: 圧縮の効く合成 PNG で WebP が採用され pak 内バイト列が実際に
  `RIFF`/`WEBP` に変わっていること、`GameDir::open`（pak backend）経由で読み戻した
  デコード結果が元 PNG のデコード結果と完全一致すること、`--no-webp` 相当（`webp=false`）
  では元 PNG バイト列のまま格納されることを確認
  （`pack_dir_converts_png_to_webp_when_smaller_and_engine_reads_it_back`/
  `pack_dir_no_webp_keeps_original_png_bytes`）。
- **既存 pak 統合テストの扱い**: `noiria-tools/tests/pak_roundtrip.rs` の既存 2 テスト
  （`*_byte_for_byte`/`*_hides_filenames_and_content`）は `read_bytes`（生の格納バイト列を
  返す API）の完全一致を検証する設計のため、`pack_dir(..., webp=false)` に変更して
  「バイト列が完全一致」という元の主張をそのまま維持した（削除・弱体化はしていない）。
  WebP 有効時の等価性は新規テスト
  `packed_sample_game_with_webp_matches_directory_by_decoded_pixels` で、画像エントリは
  `load_image_rgba`（エンジンが実際に使う API）のデコード結果が一致すること、画像以外の
  エントリは従来通り生バイトが一致することを確認する形にした。
- **E2E（pack→autotest）**: `noiria-pack` で WebP 変換ありの pak を作成し
  `noiria games/sample/game.pak --autotest 24` を実行、EXIT=0・16 フレーム保存を確認。
  ディレクトリ起動（`noiria games/sample --autotest 24`）の PNG と比較すると
  `frame_03_hover.png`/`frame_09_hover.png` の 2 枚だけバイトが異なった。**この 2 枚は
  WebP と無関係の既知ジッタ**であることを、ディレクトリ起動を 2 回連続実行して比較する
  ことで確認した（2 回とも同じ `frame_03_hover.png`/`frame_09_hover.png` だけが不一致、
  hover 演出のタイミング依存と推測される。計画の検証体系まとめに記載済みの「既知ジッタ 2
  フレーム除外可」に一致）。それ以外の 14 枚は完全一致（画像はロスレス変換なので当然の結果）。
- **export E2E**: `cargo build --profile dist --no-default-features --features
  engine-tsint,dist -p noiria-app` → `noiria-export --game games/sample --out target/export
  --targets windows` を実行し、`Noiria.exe`（2,490,880 バイト）+ `game.pak`
  （21,825,075 バイト、webp 変換 10 件・931,547 バイト削減）を生成。`Noiria.exe --autotest 24`
  で EXIT=0・16 フレームを確認。**dist ビルド同士**（同じ `target/dist/noiria.exe` を
  ディレクトリ起動した場合と、export で作った WebP 入り pak で起動した場合）で PNG を
  比較したところ、差異は上記と同じ `frame_03_hover.png`/`frame_09_hover.png` の 2 枚のみで、
  WebP 変換による追加の差異はゼロだった（dev ビルドと dist ビルドを跨いで比較すると
  ビルド間のタイミング差でさらに 1 枚ずれることがあったため、比較は必ず同一バイナリの
  dir 起動 vs pak 起動で行うようにした）。
- **Windows**: `cargo test --workspace` 322 → **334 件**全通過（新規 12 件:
  `noiria_core::codec::webp::tests::{round_trip_checkerboard_with_alpha,
  round_trip_opaque_1x1, round_trip_gradient, encode_rejects_mismatched_buffer_len,
  decode_rejects_garbage, round_trip_all_sample_pngs,
  cross_check_our_encode_decodes_with_image_crate,
  cross_check_image_crate_encode_decodes_with_us}` 8 件 +
  `noiria_core::assets::tests::decode_image_bytes_sniffs_webp_by_magic_regardless_of_label`
  1 件 + `noiria_tools::pack::tests::{pack_dir_converts_png_to_webp_when_smaller_and_engine_reads_it_back,
  pack_dir_no_webp_keeps_original_png_bytes}` 2 件 + 統合テスト
  `pak_roundtrip::packed_sample_game_with_webp_matches_directory_by_decoded_pixels` 1 件）。
  **clippy**: `cargo clippy --workspace --all-targets` — 新規/変更ファイル（`codec/webp.rs`・
  `assets.rs`・`pak.rs`・`noiria-tools` の `pack.rs`/`export.rs`/`bin/noiria_pack.rs`/
  `bin/noiria_export.rs`）は warning 0 件。既存の無関係 warning 12 件（`analysis.rs`/
  `synth.rs`/`paint.rs`/`examples/analyze_bgm.rs`/`tests/audio_quality.rs`、W6/W7 以前から
  存在）は件数不変のまま残存。
- **WSL**（`CARGO_TARGET_DIR=$HOME/.noiria-target`、`bash scripts/wsl-test.sh --skip-x11`）:
  `cargo test --workspace` 332 → **344 件**全通過（新規 12 件、Windows と同一テスト名。
  純粋 Rust のため差分なし）。`--autotest 24` ヘッドレス回帰も EXIT=0・16 フレームを確認。
- **並列実行に起因する既知のフレーク（本ウェーブとは無関係、発見事項として記録）**:
  検証中に 2 件、並列 `cargo test` 実行時にのみ再現する既存の flaky テストを発見した。
  ①Windows: `noiria-app` の `script::tag_behavior_tests`（V8 使用）が並列実行時に 6 件失敗
  →`--test-threads=1` で全通過、再実行（デフォルト並列）でも全通過する再現性の低い flake
  だった。②WSL: `scripts/wsl-test.sh` [3/4] の ALSA 再検証ステップで
  `alsa_out::tests::open_default_with_nonexistent_device_name_fails_gracefully` が並列実行時
  にのみ失敗→`--test-threads=1` で全通過。いずれも `codec/webp.rs`/`assets.rs`/`pak.rs`/
  `pack.rs` を一切経由しないテスト（V8 スクリプト実行・ALSA デバイスハンドル）であり、本ウェーブの
  変更差分に含まれないファイルのみに関わるため、W8 由来ではなく既存の並列実行時リソース競合と
  判断した。修正は本ウェーブのスコープ外（画像コーデックと無関係）としたが、申し送り事項として
  記録する。

### 変更/新規ファイル一覧

- `Cargo.toml`: `[workspace.dependencies]` に `image-webp = "0.2"` 追加。
- `crates/noiria-core/Cargo.toml`: `image-webp.workspace = true`（通常依存）追加、
  dev-dependency の `image` に `webp` feature 追加。
- `crates/noiria-core/src/codec/webp.rs`（新規）: `decode_rgba`/`encode_lossless` の 1
  モジュール隔離ラッパー + 単体テスト 8 件。
- `crates/noiria-core/src/codec/mod.rs`: `pub mod webp;` + モジュール一覧コメント追加。
- `crates/noiria-core/src/assets.rs`: `decode_image_bytes` に RIFF/WEBP マジックバイト分岐
  追加、冒頭/`load_image_rgba` のドキュメントコメント更新、単体テスト 1 件追加。
- `crates/noiria-core/src/pak.rs`: `PackReport` に `webp_converted_count`/`webp_bytes_saved`
  フィールド追加（`build()` は 0 初期化のみ、値の計算は `noiria-tools` 側）。
- `crates/noiria-tools/src/pack.rs`: `pack_dir` に `webp: bool` 引数追加、PNG→WebP 変換
  ロジック（`try_encode_webp`）追加、単体テスト 2 件追加。
- `crates/noiria-tools/src/export.rs`: `ExportOptions`/`ExportReport` に `webp` 関連
  フィールド追加、`pack_dir` 呼び出しへ配線。
- `crates/noiria-tools/src/bin/noiria_pack.rs`: `--no-webp` CLI フラグ追加、変換件数/
  削減バイト数の表示追加。
- `crates/noiria-tools/src/bin/noiria_export.rs`: `--no-webp` CLI フラグ追加、変換件数/
  削減バイト数の表示追加。
- `crates/noiria-tools/THIRD-PARTY.txt`: `image-webp`/`byteorder-lite`/`quick-error` の
  3 エントリ追加。
- `crates/noiria-tools/tests/pak_roundtrip.rs`: 既存 2 テストを `webp=false` 呼び出しに変更
  （生バイト完全一致という元の主張を維持するため）、新規統合テスト 1 件追加。
- `README.md`: 外部依存の段階的自作化ロードマップ表に WebP の行を追加
  （「外部依存（隔離・将来自作置換候補）」として明記）。
- `docs/implementation-notes.md`: 本節を追記。

### 申し送り・未解決事項

- **ロッシー WebP デコードの経験的未検証**（2 節参照）: `image_webp` はロッシー (VP8) 対応の
  デコーダを内包しており `decode_rgba` は原理上デコードできるはずだが、pack/export が常に
  ロスレスしか生成しないため実サンプルでの検証機会が無かった。ユーザーが手動でロッシー WebP
  素材を持ち込んだ場合の動作は未検証（次ウェーブ以降、必要になった時点で実サンプルを用意して
  検証する方針でよいと判断した）。
- **`total_plain_len`/CLI の「元サイズ」表示の意味**（4 節参照）: WebP 変換が有効なときは
  「pack 対象として最終的に採用されたバイト列の合計」であり「ディスク上の元ファイルサイズ
  合計」ではない点に注意（既存フィールドの意味は変更せず、削減量は別フィールドで表現する
  設計にした）。
- **並列実行時の既存 flaky テスト 2 件を発見**（6 節参照）: 本ウェーブの変更とは無関係だが、
  `noiria-app::script::tag_behavior_tests`（V8, Windows）と
  `noiria-core::alsa_out::tests::open_default_with_nonexistent_device_name_fails_gracefully`
  （WSL）が並列実行時にごく稀に flake することを発見した。両方とも `--test-threads=1` では
  安定して通過し、デフォルト並列でも大半の実行では通過する（今回発生したのは複数回中それぞれ
  1 回ずつ）。原因調査・恒久対応は本ウェーブのスコープ外としたが、次に触る機会があれば
  調査候補として記録しておく。
- **gen-assets には手を入れていない**: 計画指示どおり「pack 時変換で十分」と判断し、
  `noiria-tools/src/gen_*.rs`（アセット生成ツール）は無変更。生成される PNG は従来通りで、
  WebP 変換は pack/export の 1 箇所に閉じている。

## 2026-07-12 配布対応 W9: 音声軽量化（FLAC サブセット・pak 内透過コーデック、M3 達成）

対象: `crates/noiria-core/src/codec/flac_subset.rs`（新設、デコード側）、
`crates/noiria-tools/src/flac_enc.rs`（新設、エンコード側）、
`crates/noiria-core/src/codec/mod.rs`（`pub mod flac_subset;` 追加）、
`crates/noiria-core/src/pak.rs`（`CODEC_WAV_FLAC=2` 実装、`build_with_special_codec` hook 追加、
`Pak::codec_of` 追加）、`crates/noiria-tools/src/pack.rs`（`pack_dir` に `flac: bool` 引数追加）、
`crates/noiria-tools/src/export.rs`（`ExportOptions`/`ExportReport` に flac 関連フィールド追加）、
`crates/noiria-tools/src/lib.rs`（`pub mod flac_enc;` 追加）、
`crates/noiria-tools/src/bin/{noiria_pack,noiria_export}.rs`（`--no-flac` フラグ追加）、
`crates/noiria-tools/tests/flac_subset_roundtrip.rs`（新設、round-trip 統合テスト）、
`crates/noiria-tools/tests/pak_roundtrip.rs`（flac 統合テスト追加、既存呼び出しに引数追加）、
`docs/distribution.md`（音声フォーマットガイド追記）、`README.md`（ロードマップ表更新）。
ベースラインは Windows cargo test 334 件 / WSL 344 件全通過（W8 まで完了）。

### 1. codec=2 (wav-flac) のコンテナ設計

- **判断**: pak codec=2 の実体は「本家 FLAC 非互換の独自コンテナ」。エンコーダが元 WAV の
  data チャンク**ペイロードのバイト範囲だけ**を特定し、prefix（RIFF ヘッダ〜data チャンクの
  id+len ヘッダ 8 バイトまで）と suffix（data ペイロード直後〜ファイル末尾）を**生バイトの
  まま**ブロブに埋め込み、data ペイロードだけを自作 FLAC 風サブセットへ符号化する設計にした。
  **理由**: デコーダ側 (`decode_wav`) は WAV のチャンク構造を一切解釈せず、
  `prefix + 復元PCM + suffix` を単純連結するだけで元バイト列を再現できる。「WAV フォーマットの
  誤解釈でバイト完全一致が崩れる」というバグの入り込む余地をデコーダから完全に排除できる
  （チャンク走査という「解釈が要る」処理はエンコーダ側にしか存在しない）。**影響**: 追加
  チャンク（LIST 等）付き WAV や fmt がdata より後にある WAV でも、data の前後を丸ごと
  保存するだけで自動的に対応できる（統合テスト
  `edge_case_extra_chunk_before_and_after_data_round_trips` で検証）。
- **判断**: data チャンク長がフレーム境界（16bit × channels）に揃っていない場合
  （「奇数長 data」エッジケース）は `try_encode` が `None` を返し、pak 側の既存
  DEFLATE/stored フォールバックに委ねる。**理由**: 対応範囲を「16bit PCM / 1-2ch」に絞ると
  計画で明示されており、フレーム境界に揃わないデータは無理に扱わず安全側に倒す方が
  シンプルで壊れにくい。

### 2. fixed predictor + Rice 符号の設計（判断→理由→影響）

- **判断**: 予測器は次数 0〜4 の fixed predictor のみ（LPC 等の係数計算は実装しない）。
  各ブロック（`BLOCK_SIZE=4096` サンプル固定、最終ブロックのみ端数）ごとに 0〜4 の**全次数を
  実際に残差計算してコストを比較**し、最小になる次数を選ぶ（+ CONSTANT/VERBATIM も候補に
  含めて比較。全ブロック内容が同一なら CONSTANT を即採用する高速パスあり）。**理由**: 計画が
  明示的に fixed predictor をスコープとしており（LPC は将来候補）、次数選択を「全部試して
  一番小さいものを選ぶ」という単純な総当たりにすることで、per-block の最適性を確保しつつ
  実装も検証もシンプルに保てる。
- **判断**: Rice パラメータ k は「パーティション無しの単一 k」。厳密な全探索ではなく、
  ジグザグ値の合計 `sum_z` から `n << k >= sum_z` を満たす最小の k を求める閉じた式で
  推定する（`docs` 内コメント参照）。**理由**: 本家 FLAC のようなパーティション分割は
  計画で「簡素でよい」と明示されており、k の推定式は理論的に最適 k に近いことが知られる
  古典的な近似（過剰な全探索を避けつつ実用上十分な精度）。**影響**: 実装・検証コストを
  抑えつつ、games/sample 実測で WAV 部 -42.2% という妥当な圧縮率を達成できた。
- **判断**: Rice の unary 符号が病的に長くなる外れ値（`quotient > 2^20`）が出た場合、その
  次数の候補を無効化する安全弁を入れた。**理由**: プロパティテストで生成する乱数 PCM や
  意図的な外れ値入力に対して、unary 符号のビット数が実質無限に伸びて処理が固まる／
  メモリを圧迫するのを防ぐため。**影響**: この場合でも VERBATIM は必ず候補として残るため、
  ブロックの符号化が失敗することはない（`encode_block` の `best` 初期値が常に VERBATIM）。
  結果として `try_encode` 全体が「最悪でも元よりごくわずかに大きくなる程度」で収まり、
  最終的なサイズ比較でおのずと `None`（フォールバック）へ落ちる。

### 3. ステレオの扱い: mid-side を試作したが実測ゼロ改善のため独立 L/R に据え置いた

- **判断**: 最終的に **L/R 完全独立**での符号化を採用した（計画は「L/R 独立 or mid-side の
  どちらか実装しやすい方」を許容）。**経緯**: 当初は独立 L/R のみで実装し、`games/sample` の
  WAV 5 本で実測 -42.2%（目標 -45% にわずかに届かず）。BGM 3 本（`bgm/*.wav`）が WAV
  データの 99% を占めるため、ステレオチャンネル間デコリレーション（mid-side、
  `mid=(L+R)>>1`, `side=L-R`）を追加実装すれば目標達成できるはずと判断し、実際に
  実装・全テスト通過まで完了させた（side チャンネルが 17bit 幅になるため、生値
  (CONSTANT/VERBATIM/warmup) のビット幅を全チャンネル共通で 16→17bit に拡張する変更を含む）。
  **しかし実測したところ `games/sample` の BGM 3 本は全て独立モードの方が選ばれ、
  圧縮率は全く改善しなかった**（-42.2% のまま、mid-side 対応のヘッダ 1 バイト分だけ
  わずかに悪化: 削減量 9,061,807→9,061,142 バイト）。手続き生成シンセ BGM
  （`crates/noiria-tools/src/synth.rs`）はチャンネルごとに異なる LFO 等で意図的に
  ステレオ幅を持たせており、L/R相関が高くないためと推測される（合成テスト
  `stereo_correlated_signal_picks_mid_side_and_is_smaller_than_independent_would_be` で
  mid-side 自体のロジックは正しく機能する＝強く相関した信号では確実に選ばれることは確認済み、
  実装のバグではない）。**判断（最終）**: 実データで効果がゼロだった上に、①生値のビット幅が
  16→17bit で可変になる、②毎回 独立/mid-side の両方を実際にフルエンコードして比較する必要が
  ある、という複雑さが増える一方だったため、CLAUDE.md「簡潔さを優先・実利のない複雑化を
  しない」方針に従い mid-side 実装を破棄し、シンプルな独立 L/R 実装に戻した。**影響**:
  WAV 部の圧縮率は計画が明記した想定レンジ「可逆 -40〜60%」には収まっているが、検証項目に
  記載された目安「-45%以上」にはわずかに届いていない（実測 -42.2%、下記「未解決」参照）。
  この判断過程・実測結果は今後 mid-side を再検討する際の重要な参考情報として本節に残す。

### 4. エンコーダ/デコーダの独立実装の徹底度

- **クレート境界による強制分離**: デコーダ (`noiria-core/src/codec/flac_subset.rs`) と
  エンコーダ (`noiria-tools/src/flac_enc.rs`) は物理的に別クレートにあり、
  `noiria-core` は `noiria-tools` に依存できない（依存方向が逆）。したがって**構造的に
  コード共有が不可能**——これは webp（W8、両方向とも `noiria-core` 内の 1 モジュール）とは
  異なる意図的な設計選択で、計画に明記された「W4 の教訓（対称な実装は round-trip のバグを
  隠す）」を最も強く効かせる形にした。
- **重複実装した要素**: MAGIC 定数・BLOCK_SIZE・ジグザグ符号化・fixed predictor の係数・
  MSB-first ビット詰め方向・Rice 符号の unary/remainder 分割——これらはすべて**両ファイルに
  それぞれ独立に記述**してあり、共有関数・共有定数モジュールは一切ない（コンテナのバイト
  レイアウトという「仕様」自体は当然一致させる必要があるが、それを実現する Rust の実体
  （struct/関数）は完全に別物）。
- **検証での担保**: 両者を同時に呼べる場所は `noiria-tools`（`noiria-core` に依存できる側）
  のみのため、round-trip テストは意図的に `crates/noiria-tools/tests/flac_subset_roundtrip.rs`
  という統合テストに集約した（`noiria_tools::flac_enc::try_encode` →
  `noiria_core::codec::flac_subset::decode_wav` を直接呼ぶ）。単体テスト（各ファイル内の
  `#[cfg(test)]`）は「その実装単体で閉じたロジック」（ヘッダ検証・8/24bit 拒否・
  サイズ判定等）のみを対象にし、round-trip の主張は統合テストに一本化することで、
  「デコーダのテストがエンコーダの実装詳細に暗黙に依存する」ような癒着を避けた。

### 5. pak 統合: `build_with_special_codec` hook（判断→理由→影響）

- **判断**: `pak::build`（既存、拡張子ベースの stored/DEFLATE 自動選択）はシグネチャを
  変えず据え置き、新たに `pak::build_with_special_codec(entries, key, special)` を追加した
  （`build` は `build_with_special_codec(entries, key, |_, _| None)` の薄いラッパに再実装）。
  `special: FnMut(path, plain) -> Option<(codec, stored_bytes)>` が `Some` を返せば
  追加の DEFLATE 圧縮なしでそのまま採用し、`None` なら従来の `compress_for_entry` 
  自動選択にフォールバックする。**理由**: FLAC-subset エンコーダは `noiria-tools` 側にしか
  存在せず、`pak.rs`（`noiria-core`）から直接呼び出せない（依存方向の制約、3節参照）。
  一方で codec=2 は webp と異なり「pak が読み戻す際に元バイト列と異なる中身を復号する」
  という**pak.rs 自身が知っておくべき codec** なので、webp のように「pack_dir が entries を
  差し替えてから `build()` へ渡す」だけでは済まない（`plain_len`/`crc32` は元 WAV に対して
  計算する必要があり、`build()` 内部でしか「元 WAV バイト列」と「エンコード済みバイト列」を
  同時に見られないため）。hook 方式にすることで、`pak.rs` は「WAV や FLAC を知らないまま」
  codec 選択だけを外部委譲でき、既存の `build()` 呼び出し元（`pak.rs` 自身の単体テスト含む）
  は無改修で済んだ。**影響**: `noiria-tools::pack::pack_dir` は
  `pak::build_with_special_codec(entries, key, |path, data| { if flac && path が .wav なら
  flac_enc::try_encode(data) を試す })` という 1 箇所の配線だけで済んでいる。
- **判断**: `PackReport` に `flac_converted_count`/`flac_bytes_saved` を追加し、
  `build_with_special_codec` の中で直接計算する（`codec_stored_count`/`codec_deflate_count`
  と同じループ内でインクリメント）。**理由**: webp の対応するフィールドは
  `noiria-tools::pack_dir` 側で計算していた（`build()` に渡す前に entries を webp バイト列へ
  差し替えるため、元 PNG サイズは `build()` に渡された時点でもう失われている）。wav-flac は
  逆に `build_with_special_codec` が `plain`（元 WAV）と `stored_bytes`（flac blob）の両方を
  同時に見られる唯一の場所なので、そこで計算する方が自然でシンプル。
- **判断**: `Pak::codec_of(path) -> Option<u8>` を追加した（テスト・CLI 診断用、エンジン本体は
  未使用）。**理由**: `pack_dir_converts_wav_to_flac_when_smaller_and_engine_reads_it_back`
  等のテストで「実際に codec=2 が選ばれたこと」を `read()` の結果（常に元 WAV バイト列に
  復元されるため生バイトからは判別できない）とは別に直接検証したかったため。

### 検証結果

- **round-trip バイト完全一致**（`crates/noiria-tools/tests/flac_subset_roundtrip.rs`、
  `noiria_tools::flac_enc::try_encode` → `noiria_core::codec::flac_subset::decode_wav`）:
  - `games/sample` 実アセットの WAV 5 ファイル全部で encode→decode がバイト完全一致
    （`sample_game_wav_files_round_trip`）。
  - エッジケース: 奇数長 data（フォールバック `None` を確認）、追加チャンク（LIST 相当を
    data 前、ダミーチャンクを data 後に配置）付き WAV の round-trip、8bit/24bit の
    フォールバック確認、極短 PCM（40 サンプル無音・単一サンプル）、無音（2秒）、
    フルスケール正弦波（ステレオ、逆位相）、モノラルのランプ波形——全て round-trip
    バイト完全一致 or 意図通りのフォールバックを確認。
  - `packed_sample_game_with_flac_matches_directory_backend_byte_for_byte`
    （`crates/noiria-tools/tests/pak_roundtrip.rs`）: `games/sample` を pack（flac 有効）し、
    `GameDir`（Pak backend）と `GameDir`（Dir backend）の `read_bytes` が**全エントリで
    生バイト完全一致**することを確認（wav-flac は codec=2 で読み戻す際に元 WAV バイト列を
    完全復元するため、webp のような「デコード結果ピクセル比較」ではなく生バイト比較で
    十分という設計上の性質を直接検証している）。
- **プロパティテスト**（`property_random_and_synthetic_pcm_round_trip`）: 固定シード
  splitmix64 PRNG で白色雑音・ランダムウォーク・正弦波・ランプ・矩形波・無音の 6 種類×
  ランダムなチャンネル数(1-2)/長さ(1-6000フレーム)/サンプルレート(8k/22.05k/44.1kHz)を
  300 本生成し、`try_encode` が `Some` を返したケース全てで decode 後バイト完全一致を確認
  （300 本中 214 本が実際に flac 化され round-trip 確認済み、残りは白色雑音等の
  非圧縮的な内容で `None`＝正しいフォールバックだった）。ステレオのインターリーブ順が
  壊れていないことを別途 `property_stereo_channel_interleave_is_preserved`（50 本）で確認。
- **サイズ計測**（`noiria-pack`、release ビルド、`games/sample`）:

| ファイル | 元サイズ | wav-flac 後 | 比率 |
| --- | ---: | ---: | ---: |
| bgm/main.wav | 7,526,444 | 4,441,138 | 59.0% |
| bgm/alt_calm.wav | 7,056,044 | 3,790,026 | 53.7% |
| bgm/alt_tension.wav | 6,773,804 | 4,151,774 | 61.3% |
| se/click.wav | 12,392 | 6,320 | 51.0% |
| voice/alice_001.wav | 105,884 | 23,503 | 22.2% |
| **WAV 合計** | **21,474,568** | **12,412,761** | **57.8%（-42.2%）** |

  pak 全体（`--no-webp --no-flac` の生アセット総和 23,760,978 バイトを基準）:

| 構成 | pak サイズ | 元サイズ比 |
| --- | ---: | ---: |
| 無変換（W6/W8 相当、比較用） | 22,756,628 | 95.8% |
| webp のみ (`--no-flac`) | 21,825,075 | 91.9% |
| flac のみ (`--no-webp`) | 14,689,841 | 61.8% |
| **flac+webp（既定）** | **13,758,284** | **57.9%（-42.1%）** |

  **配布物合計**（`noiria-export --targets windows`、dist ビルド exe + flac+webp pak）:
  `Noiria.exe` 2,498,560 バイト + `game.pak` 13,758,284 バイト + README/THIRD-PARTY 数KB
  = **合計 16,264,257 バイト（約 15.5 MiB）**。計画の目安「~10MiB 前後」には届いていない
  （下記「未解決」参照）が、W6 開始時点の assets 22.6MiB から見て意味のある削減
  （画像 -55.3%、WAV -42.2%）は達成できている。
- **E2E（`--autotest 90`、pak 版、dev ビルド）**: EXIT=0、ログにエラー/パニック 0 件。
  `xchgbgm`（BGM 切替、flac 圧縮された WAV の実読込パス）・ボタン・チェックボックス・
  エディット・tempsave/tempload・goback・ask・timeout 等、幅広いコードパスを通過することを
  確認。
- **E2E（`--autotest 24`、pak 版 vs dir 版、dev ビルド）**: 16 枚中 14 枚が完全一致。
  差分は `frame_03_hover.png`/`frame_09_hover.png` の 2 枚のみで、これは dir 版を 2 回
  連続実行しただけでも再現する既知のホバータイミングジッタ（W5/W6/W8 で既に特定済み）
  であることを確認した。
- **E2E（`--autotest 24`、export 産物 = dist ビルド, pak 版 vs dir 版）**: **同一 dist バイナリ
  同士**で比較したところ、`frame_03_hover.png`/`frame_09_hover.png` に加えて
  `frame_10〜14_hover.png`/`frame_final.png` も含む計 8 枚が不一致という、W8 が報告した
  「2 枚のみ」より明らかに大きい差分を検出した（複数回再実行しても再現性あり）。
  切り分けのため `--no-flac`（webp のみ、W6/W8 相当の pak 構成）でも同じ比較を行ったところ
  **全く同じ 8 枚が不一致**になった。これにより、この差分は **W9（FLAC）由来ではなく、
  pak 読込一般（W6 由来、dist プロファイルでのみ顕在化）の既存の時間依存ジッタ**である
  ことを切り分けた（本ウェーブの変更差分はこの 8 枚の差に寄与していない）。下記
  「未解決」に申し送り事項として記録する。
- **Windows**: `cargo test --workspace` 334 → **362 件**全通過（新規 28 件）。内訳:
  `noiria_core::codec::flac_subset::tests::{rejects_bad_magic, rejects_too_short,
  rejects_bad_channel_count, bit_reader_round_trips_written_pattern,
  unzigzag_matches_known_values, decode_constant_block_all_same_value}` 6 件、
  `noiria_core::pak::tests::{special_codec_hook_is_used_when_it_returns_some,
  special_codec_hook_falls_back_to_auto_selection_when_none,
  build_matches_build_with_special_codec_none_hook,
  read_reconstructs_wav_flac_entry_via_real_codec}` 4 件、
  `noiria_tools::flac_enc::tests::{rejects_non_wav, rejects_8bit_and_24bit,
  rejects_odd_length_data, compresses_constant_silence,
  does_not_shrink_returns_none_for_tiny_input}` 5 件、
  `noiria_tools::pack::tests::{pack_dir_converts_wav_to_flac_when_smaller_and_engine_reads_it_back,
  pack_dir_no_flac_keeps_wav_uncompressed_by_flac}` 2 件、
  `flac_subset_roundtrip.rs` 統合テスト 10 件、`pak_roundtrip.rs` の
  `packed_sample_game_with_flac_matches_directory_backend_byte_for_byte` 1 件。
- **clippy**: `cargo clippy --workspace --all-targets` — 新規/変更ファイル
  （`codec/flac_subset.rs`、`flac_enc.rs`、`pak.rs`、`noiria-tools` の `pack.rs`/`export.rs`/
  `bin/noiria_pack.rs`/`bin/noiria_export.rs`/`tests/flac_subset_roundtrip.rs`/
  `tests/pak_roundtrip.rs`）は warning 0 件。既存の無関係 warning（`analysis.rs`/`synth.rs`/
  `paint.rs`/`audio_quality.rs`/`analyze_bgm.rs`、W8 以前から存在）は件数不変のまま残存。

### 変更ファイル一覧

- `crates/noiria-core/src/codec/flac_subset.rs`（新規）: wav-flac デコード側（コンテナ
  パース、fixed predictor 逆演算、Rice/ジグザグ復号、MSB-first BitReader）+ 単体テスト 6 件。
- `crates/noiria-core/src/codec/mod.rs`: `pub mod flac_subset;` + モジュール一覧コメント追加。
- `crates/noiria-core/src/pak.rs`: `CODEC_WAV_FLAC=2` 定数、`build_with_special_codec`
  （`build` はこれの薄いラッパへ再実装）、`Pak::read` の codec=2 分岐、`Pak::codec_of`、
  `PackReport` に `flac_converted_count`/`flac_bytes_saved`、単体テスト 4 件追加。
- `crates/noiria-tools/src/flac_enc.rs`（新規）: wav-flac エンコード側（独立 WAV チャンク
  走査、fixed predictor 選択+コスト計算、Rice/ジグザグ符号化、MSB-first BitWriter）+
  単体テスト 5 件。
- `crates/noiria-tools/src/lib.rs`: `pub mod flac_enc;` 追加。
- `crates/noiria-tools/src/pack.rs`: `pack_dir` に `flac: bool` 引数追加、
  `pak::build_with_special_codec` への配線、単体テスト 2 件追加、既存 3 テストの
  呼び出しに引数追加。
- `crates/noiria-tools/src/export.rs`: `ExportOptions`/`ExportReport` に flac 関連
  フィールド追加、`pack_dir` 呼び出しへ配線。
- `crates/noiria-tools/src/bin/noiria_pack.rs`: `--no-flac` CLI フラグ追加、変換件数/
  削減バイト数の表示追加。
- `crates/noiria-tools/src/bin/noiria_export.rs`: `--no-flac` CLI フラグ追加、変換件数/
  削減バイト数の表示追加。
- `crates/noiria-tools/tests/flac_subset_roundtrip.rs`（新規）: エンコーダ/デコーダを
  同時に呼べる唯一の統合テスト。実アセット round-trip・エッジケース・プロパティテスト
  10 件。
- `crates/noiria-tools/tests/pak_roundtrip.rs`: 既存 3 テストを `flac` 引数追加に追従
  （既定は `flac=false` を明示指定し「元の主張」を維持）、新規統合テスト 1 件追加。
- `docs/distribution.md`: 「音声アセットの軽量化（W9）」節を追記（.ogg 推奨・.wav 自動
  圧縮・.flac 非対応の指針、仕組みの概要、実測サイズ）。
- `README.md`: 外部依存の段階的自作化ロードマップ表の WAV 行に pak 軽量化（W9）の
  説明を追記。
- `docs/implementation-notes.md`: 本節を追記。

### 未解決・申し送り事項（M3 宣言の可否）

- **WAV 圧縮率が目安「-45%以上」にわずかに届いていない（実測 -42.2%）**: 3 節で詳述した
  通り、mid-side を試作したが `games/sample` の実データでは効果ゼロだったため独立 L/R に
  据え置いた。計画本文が事前に見込んでいたレンジ「可逆 -40〜60%」には収まっており、
  fixed predictor + Rice（LPC 不使用）というスコープの範囲では妥当な結果と判断している。
  さらに圧縮率を上げるには LPC（Levinson-Durbin による係数計算+量子化）の導入が必要になる
  可能性が高いが、これは計画のスコープ外（"fixed predictor（次数 0〜4）" と明記）であり、
  実装量・検証コスト・round-trip バグリスクが大きく増える。**次ウェーブ以降の判断材料として
  記録するが、本ウェーブでは追加実装を見送った**。
- **配布物合計サイズが目安「~10MiB 前後」に届いていない（実測 約15.5MiB、内訳: exe 2.5MB +
  pak 13.76MB）**: 上記 WAV 圧縮率の限界が主因（pak の 90%が音声）。画像は WebP で
  既に -55.3% を達成済みなので、これ以上の大幅な追加削減は音声側の改善（LPC 等）に
  依存する。
- **dist ビルドの pak-vs-dir `--autotest` 比較で、W8 が報告した「2 枚のみ不一致」より
  大きい「8 枚不一致」を発見**（検証結果参照）。**W9（FLAC）の変更が原因でないことは
  `--no-flac` での同一比較で切り分け済み**（webp のみでも同じ 8 枚が不一致になる）。
  dev ビルドでの pak-vs-dir 比較は従来通り 2 枚のみの差分だったため、**dist プロファイル
  固有の何らかの時間依存要因**（推測: strip/lto によるコード配置差、または
  pak の復号処理自体のタイミングが dir の生ファイル読込と比べて何らかの形で
  ホバー判定のタイミングに影響している可能性）と考えられるが、根本原因の特定は
  本ウェーブのスコープ外（W9 の変更差分に含まれないファイルのみに関わる）としたため
  未調査のまま申し送る。**実害の確認**: 差分のあるフレームも PNG として正常なサイズ・
  内容（クラッシュ/破損ではなくホバー状態が異なるだけ）であることは確認済み。
- **M3 宣言の可否**: 上記 2 点（圧縮率が目安未達、配布物サイズが目安未達）はいずれも
  「機能として壊れている」わけではなく、**計画が事前に見込んだ性能レンジ内での未達**
  （fixed predictor スコープの限界）である。round-trip の正しさ・pak 統合・E2E 動作は
  全て検証済みで機能的な問題は無い。したがって**機能面では M3（配布サイズ最適化）は
  達成**と判断できるが、**数値目標（-45%/~10MiB）の完全達成には至っていない**ため、
  この点をユーザーへ明確に報告し、目標達成の要否・追加投資（LPC 導入等）の要否を
  ユーザー判断に委ねる。

## 2026-07-13 wav-flac LPC 拡張

対象: `crates/noiria-core/src/codec/flac_subset.rs`（デコード側、LPC サブフレーム追加）、
`crates/noiria-tools/src/flac_enc.rs`（エンコード側、Levinson-Durbin + 量子化 + 候補選択）、
`crates/noiria-tools/tests/flac_subset_roundtrip.rs`（LPC 経路を通る round-trip テスト・
decode 速度テスト追加）、`crates/noiria-tools/tests/legacy_w9_pak_compat.rs`（新設、旧
W9(fixedのみ)形式の固定フィクスチャによる後方互換テスト）、
`crates/noiria-tools/tests/fixtures/w9_legacy_fixed_only.bin`（新設、フィクスチャ本体）、
`docs/distribution.md`・`README.md`（圧縮率の数値更新）。ベースラインは Windows
cargo test 362 件 / WSL 372 件全通過（W9 まで完了、途中作業ディレクトリを共有する別セッションが
`noiria-app` を並行編集していた時間帯があり一時的にビルドが壊れたが本ウェーブの変更とは無関係
であることを確認済み。最終的な検証は完了後の状態で実施した）。

### 1. フォーマット拡張: サブフレーム種別の空きコードに LPC を割り当てる

- **判断**: コンテナへバージョンフィールドを追加せず、既存の 2bit サブフレーム種別
  （0=CONSTANT, 1=VERBATIM, 2=FIXED）が使っていなかった残り 1 コード（3）を LPC に
  割り当てた。**理由**: W9 の旧エンコーダは構造的に 3 を出力しないため、旧 pak
  （fixed のみ）は新デコーダでも解釈が一切変わらず自動的に後方互換になる。コンテナ全体の
  バージョニング機構を新設するより遥かに単純で、「既存フォーマットのフレームも引き続き
  デコード可能に」という要求を最小の変更で満たせる。**影響**: `decode_wav`/`try_encode`
  の外側のコンテナ形式（"NFC1" ヘッダ、prefix/suffix）は一切変更していない。

### 2. LPC サブフレームのビット形式と整数演算の設計

- **判断**: `[3bit 次数-1(1-8)] + [4bit 係数精度] + [5bit シフト量] + [5bit Rice k] +
  [次数×精度bit 量子化係数] + [次数×16bit warmup生サンプル] + [Rice符号列]` という形式にした
  （FIXED と同じ「ヘッダ→warmup→残差」の並びを踏襲）。予測式は
  `pred = (sum_{j=0}^{order-1} coeff[j] * x[i-1-j]) >> shift` で、**デコーダは整数演算
  （i64 の積和 + 算術右シフト）だけ**で計算する。エンコーダは Levinson-Durbin を f64 で
  計算した後、量子化した整数係数をビットストリームに格納するだけで、デコーダ側は
  浮動小数点を一切使わない。**理由**: 浮動小数点の丸め方（コンパイラ最適化・CPU 差）が
  Windows/WSL 間で bit-exact に一致する保証は無いため、round-trip の正しさを整数演算の
  決定性だけに依存させる設計にした（要求仕様の「整数演算のみで完全可逆」を文字通り満たす）。
- **判断**: 係数精度は探索せず **固定 15bit**（4bit フィールドで表現できる最大値）とし、
  シフト量だけをブロックごとに適応させる（誤差フィードバック付き丸め: 各係数の量子化誤差を
  次の係数の丸めへ持ち越し、量子化誤差の蓄積を抑える、FLAC 本家と同様の手法）。**理由**:
  精度を探索次元に加えると候補数が数倍に増える一方、実験的に精度を落とす効果は小さいと
  判断した（15bit は量子化誤差をほぼ無視できるレベル）。CLAUDE.md「探索次元を増やさない」
  「実利のない複雑化をしない」方針に従い単純な設計に留めた。
- **判断**: 次数 1-8 の全候補を Levinson-Durbin で一度に導出し（次数を増やす再帰の各段階が
  そのままその次数の係数になる）、各次数ごとに量子化→実際の整数残差計算→実測ビット数
  （k 推定含む fixed_order_cost と同型の関数）を行い、fixed 0-4 も含めた全候補から最小の
  ものを選ぶ完全な総当たりにした。**理由**: 計画が「次数ごとの予測誤差を先に見積もって
  絞ってもよい」と許容していたが、実測（`games/sample` BGM、後述）でブロックあたりの
  候補評価コストが数十マイクロ秒程度であり、絞り込み無しでも実用上十分高速だった
  （decode ではなく encode 側の話だが、pack はビルド時の 1 回限りの処理でリアルタイム性が
  不要なため、複雑な事前絞り込みロジックを追加するコストに見合わないと判断した）。
- **判断**: LPC 自己相関の計算には Welch 窓を適用する（残差の実計算は非窓掛けの生サンプル
  で行う。窓は自己相関推定の精度を上げるためだけの前処理）。**理由**: ブロック端での
  予測精度低下を抑える FLAC 本家と同様の一般的手法で、実装コストが小さい（数行）。

### 3. Rice パーティション分割: 実測の上で見送った

- **判断**: パーティション分割は実装しなかった。**理由**: `games/sample` の実 BGM
  （`bgm/main.wav`）の実残差列に対して、単一 k と 4/8/16 分割（各パーティション独立 k）を
  比較する実測診断を行ったところ、**分割した方がむしろ僅かに悪化した**（4分割で
  +0.03%、8分割で+0.09%、16分割で+0.20%、いずれも単一k比でサイズ増）。`BLOCK_SIZE=4096`
  （約93ms@44.1kHz）は既にこの音声の残差エネルギーがほぼ定常とみなせる粒度であり、
  分割による適応の恩恵よりも「パーティションごとの k フィールドのオーバーヘッド」と
  「各パーティションのサンプル数が減ることによる Rice 符号の統計効率低下」が上回った
  と考えられる。**影響**: 計画が明示的に許容した「実測で効果薄なら見送り可」の判断基準に
  従い、実装を見送った。この診断は一時的な `#[ignore]` テストで行い、知見を得た後に
  削除した（CLAUDE.md「探索的コードを製品コードに残さない」判断、本節の記録がその代わりの
  証跡になる）。

### 4. 目標「-50%以上」に届かなかった根本原因の診断

- **判断**: LPC 拡張後の実測は `games/sample` WAV 合計で **-42.7%**（LPC 拡張前 W9:
  -42.2%）と、目標 -50% は疎か W9 からの改善も **0.5 ポイント程度**に留まった。これは
  実装のバグではなく、**この音声コンテンツの本質的な性質**であることを、Levinson-Durbin
  の予測誤差 E(order) を次数 1〜32 まで直接調べる診断で確認した: 代表的な BGM ブロックで
  E(order) は次数 1→2 で桁違いに下がる（fixed predictor の 2 次差分が効くのと対応）が、
  **次数 2→32 まで伸ばしてもわずか 3〜4% しか追加で下がらない**（例: あるブロックで
  E(2)=72,873,621 → E(32)=70,695,500）。つまり次数上限を計画通り 8 に制限したことが
  ボトルネックなのではなく、**この音声（`synth.rs` による手続き生成 BGM、W9 時点で
  「チャンネルごとに異なる LFO 等で意図的にステレオ幅を持たせている」と分析済み）は
  2 次の線形予測でほぼ全ての線形予測可能な構造を使い切ってしまっており、残差が線形予測の
  観点でほぼホワイトノイズに近い**ため、次数を増やしても Rice 符号化するべき残差エントロピー
  がほとんど減らない。この診断は実際の候補選択（`best_candidate`）でも裏付けられ、
  BGM の 460 ブロック中ほぼ全てが LPC を選ぶが、その大半は次数2（ときどき3・6・7）で、
  高次（8）が選ばれることは稀だった。**影響**: 音声/ボイス（`click.wav` -45.5%/-51.0%→
  改善、`voice/alice_001.wav` -17.0%/-22.2%→大幅改善）では LPC が明確に効く一方、BGM
  3 曲（WAV 総量の 99%を占める）ではほぼ効果が無いため、全体の改善が小さい。
  さらなる圧縮には、この残差そのものをより賢く符号化する手法（適応/算術符号化等、Rice
  符号を超える技術）か、そもそも線形予測とは異なるアプローチが必要と考えられるが、
  いずれも計画のスコープ（fixed+LPC+Rice の拡張）を超えるため本ウェーブでは実装しない。
  **この診断結果は次ウェーブでさらなる圧縮を検討する際の重要な材料として記録する。**

### 検証結果

- **round-trip バイト完全一致**: 既存の W9 round-trip テスト群（`flac_subset_roundtrip.rs`
  10 件、`flac_enc.rs`/`flac_subset.rs` の単体テスト）は**無変更のまま全通過**——LPC 拡張が
  fixed 専用の既存経路を一切壊していないことの直接的な証明。加えて新規:
  - `edge_case_smooth_multitone_signal_exercises_lpc_path_round_trips`
    （`flac_subset_roundtrip.rs`）: シンセ BGM を模した複合正弦波（LPC を高確率で選ぶ形状、
    `noiria_tools::flac_enc::tests::high_order_correlated_signal_picks_lpc_over_fixed` で
    同形状が実際に LPC 候補（次数1-8）を選ぶことを確認済み）で byte-exact round-trip。
  - デコーダ単体: `decode_lpc_block_matches_manual_prediction`/
    `decode_lpc_block_handles_negative_coefficient`（`flac_subset.rs`）—
    手組みしたビット列（エンコーダを一切使わない）で LPC デコード経路の整数演算（正/負係数
    双方）を直接検証。`sign_extend_matches_known_values`/`lpc_predict_uses_integer_shift_only`
    で補助関数も個別に検証。
  - エンコーダ単体: `lpc_predict_matches_hand_computed_value`/
    `quantize_lpc_coeffs_round_trips_approximately`/`quantize_lpc_coeffs_handles_all_zero_input`/
    `levinson_durbin_recovers_known_ar1_coefficients`（既知の AR(1) 自己相関から理論値通りの
    係数が復元されることを検証）/`high_order_correlated_signal_picks_lpc_over_fixed`
    （`flac_enc.rs`）。
- **旧 pak 互換（固定フィクスチャ）**: `legacy_w9_pak_compat.rs` に W9 時点（LPC 拡張前）の
  固定予測のみのビット詰めロジックを**凍結して独立に複製**し、それで生成した blob を
  `tests/fixtures/w9_legacy_fixed_only.bin` として固定フィクスチャ化した。
  `legacy_w9_fixed_only_blob_still_decodes_with_current_decoder` が現行デコーダでこの
  フィクスチャを読み、期待サンプル列と一致することを検証する。凍結コードは今後
  `flac_enc.rs` がどう変わっても更新しない（「当時のエンコーダの出力」を再現し続けるための
  スナップショット）。フィクスチャの生成手順は `regenerate_legacy_fixture`（`#[ignore]`）に
  記録してある。
- **サイズ計測**（`noiria-pack`/`noiria-export`、release ビルド、`games/sample`）:

| ファイル | 元サイズ | W9 (fixedのみ) | LPC拡張後 | 比率(拡張後) |
| --- | ---: | ---: | ---: | ---: |
| bgm/main.wav | 7,526,444 | 4,441,138 (59.0%) | 4,411,806 | 58.6% |
| bgm/alt_calm.wav | 7,056,044 | 3,790,026 (53.7%) | 3,745,626 | 53.1% |
| bgm/alt_tension.wav | 6,773,804 | 4,151,774 (61.3%) | 4,113,098 | 60.7% |
| se/click.wav | 12,392 | 6,320 (51.0%) | 5,637 | 45.5% |
| voice/alice_001.wav | 105,884 | 23,503 (22.2%) | 18,034 | 17.0% |
| **WAV 合計** | **21,474,568** | **12,412,761（-42.2%）** | **12,294,201** | **57.3%（-42.7%）** |

  pak 全体（`--no-webp --no-flac` の生アセット総和 23,760,978 バイトを基準、W9 と同一基準）:

| 構成 | W9 | LPC拡張後 |
| --- | ---: | ---: |
| 無変換 | 22,756,628 (95.8%) | 22,756,628 (95.8%、不変) |
| webp のみ (`--no-flac`) | 21,825,075 (91.9%) | 21,825,075 (91.9%、不変) |
| flac のみ (`--no-webp`) | 14,689,841 (61.8%) | 14,571,284 (61.3%) |
| **flac+webp（既定）** | **13,758,284（-42.1%）** | **13,639,730（-42.6%）** |

  **配布物合計**（`noiria-export --targets windows`、dist ビルド exe + flac+webp pak）:
  `Noiria.exe` 2,496,512 バイト + `game.pak` 13,639,730 バイト + README/THIRD-PARTY 7,413
  バイト = **合計 16,143,655 バイト（約 15.4 MiB）**。W9 時点の約 16,264,257 バイト
  （約 15.5 MiB）からわずか約 118KB（0.7%）の削減に留まり、目安「~10MiB 前後」には
  依然として届いていない（4節の根本原因診断の通り、BGM がほぼ改善しないため）。
- **デコード速度**（BGM 3 曲、release ビルド、`decode_wav` 単体、`bgm_decode_speed_is_well_under_target`）:

| ファイル | blob サイズ | decode 時間 (release) |
| --- | ---: | ---: |
| bgm/main.wav | 4,411,806 | 35.5ms |
| bgm/alt_calm.wav | 3,745,626 | 31.6ms |
| bgm/alt_tension.wav | 4,113,098 | 31.8ms |

  目安 <200ms/曲 に対して実測 32〜36ms（release）、50〜51ms（debug）と大幅に余裕がある
  （LPC の積和演算は O(order) で order≤8 のため、fixed predictor からのオーバーヘッドは
  無視できるレベル）。起動・曲切替をブロックする懸念は無い。
- **E2E（`--autotest 90`、pak 版、release ビルド）**: `noiria-pack` で作った LPC 込み pak
  （`game.pak` 13,639,730 バイト）を `noiria.exe <pak> --autotest 90` で起動し、**EXIT=0**、
  ログにエラー/パニック 0 件を 2 回とも確認。`xchgbgm`（BGM 切替、flac(LPC込み)圧縮された
  WAV の実読込パス）・セーブ/ロード・tempsave/tempload・goback・ask・timeout・ボタン・
  チェックボックス・エディット等、幅広いコードパスを通過。
- **Windows**: `cargo test --workspace` 362 → **374 件**全通過（新規 12 件、内訳は上記
  round-trip 検証結果に記載の各テスト）。
- **WSL**: `cargo test --workspace` 372 → **384 件**全通過（新規 12 件、Windows と同一）。
- **clippy**: `cargo clippy -p noiria-core -p noiria-tools --all-targets` — 新規/変更ファイル
  （`codec/flac_subset.rs`、`flac_enc.rs`、`tests/flac_subset_roundtrip.rs`、
  `tests/legacy_w9_pak_compat.rs`）は warning 0 件。既存の無関係 warning（`analysis.rs`/
  `synth.rs`/`paint.rs`/`audio_quality.rs`/`analyze_bgm.rs`、W8 以前から存在）は件数不変。

### 変更ファイル一覧

- `crates/noiria-core/src/codec/flac_subset.rs`: LPC サブフレーム（種別3）のデコード追加
  （`sign_extend`/`lpc_predict` を独立実装）、ドキュメントコメント更新、単体テスト 6 件追加。
- `crates/noiria-tools/src/flac_enc.rs`: Levinson-Durbin（`autocorrelate`/`levinson_durbin`）、
  係数量子化（`quantize_lpc_coeffs`、誤差フィードバック付き）、LPC コスト計算
  （`lpc_order_cost`）、候補選択を `best_candidate` として分離（テストから直接呼べるように
  リファクタ）、`encode_block` に LPC 候補の書き出しを追加。ドキュメントコメント更新、
  単体テスト 5 件追加。
- `crates/noiria-tools/tests/flac_subset_roundtrip.rs`: LPC 経路を通る round-trip テスト・
  decode 速度テストを追加（既存 10 件は無変更）。
- `crates/noiria-tools/tests/legacy_w9_pak_compat.rs`（新規）: W9(fixedのみ)形式を凍結複製した
  フィクスチャ生成/検証テスト。
- `crates/noiria-tools/tests/fixtures/w9_legacy_fixed_only.bin`（新規）: 上記フィクスチャ本体。
- `docs/distribution.md`: 音声軽量化節の圧縮率数値を更新（-42.2%→-42.7%）。
- `README.md`: ロードマップ表の WAV 行の圧縮率数値を更新。
- `docs/implementation-notes.md`: 本節を追記。

### 未解決・申し送り事項

- **WAV 圧縮率が目標「-50%以上」に届いていない（実測 -42.7%、W9比+0.5pt）**: 4節で
  診断した通り、`games/sample` の BGM（WAV 総量の 99%）は 2 次の線形予測でほぼ全ての
  予測可能な構造を使い切っており、次数を 8 まで（さらには診断上 32 まで）増やしても
  残差エントロピーがほとんど減らない。これは実装の不備ではなく音声コンテンツの性質に
  起因する本質的な限界と判断している（診断の根拠は4節参照）。Rice パーティション分割も
  実測で効果が無い（むしろ悪化）ことを確認済み。**さらなる圧縮には Rice を超える
  エントロピー符号化（適応/算術符号化等）か、非線形/長期（ピッチ）予測など、本ウェーブの
  計画スコープ（fixed+LPC+Rice の拡張）を明確に超える技術が必要**と考えられる。
  次ウェーブでの追加投資の要否をユーザー判断に委ねる。
- **配布物合計サイズが目安「~10MiB前後」に届いていない（実測 約15.4MiB）**: 上記 WAV
  圧縮率の限界がそのまま反映された結果（pak の 9 割強が音声で、そのほぼ全てが BGM）。
  画像は WebP で既に -55.3% 相当を達成済みなので、これ以上の大幅な削減はやはり音声側の
  改善に依存する。
- **音声/ボイス系（`click.wav`/`voice/alice_001.wav`）は LPC で明確に改善**（-51.0%→
  -45.5%、-22.2%→-17.0%）: 今後ボイスアセットが増える場合はこの LPC 拡張の恩恵をより
  強く受けられる見込み。

## 2026-07-13 dist フレーム差異の調査と headless 決定論化

W9 で発見された「dist ビルドの `--autotest 24` pak-vs-dir 比較で 8 枚不一致（dev は
2 枚）」の原因調査と、根本対応（headless の仮想クロック化）を行った。

### 1. 調査結果（原因の確定）

- **再現実験**: dist バイナリ同士（`target/dist/noiria.exe`、同一バイナリ）を
  `games/sample --autotest 24` で 2 回連続実行して dir 出力同士を比較したところ、
  W9 が報告した 8 枚（`frame_03_hover`/`frame_09_hover`/`frame_10〜14_hover`/
  `frame_final`）と近い枚数の差異が再現した。
- **決定的な追加実験**: **dev ビルド（既定エンジン v8）同士**でも同じ 24 秒
  `--autotest` を複数回（5 回）連続実行し、全ペアを比較した。結果、**dev 同士でも
  最大 8 枚（W9 が「dist 固有」と報告した枚数と同じ内訳）の差異が出るペアがあった**
  一方、別のペアでは 0 枚（完全一致）だった。すなわち **8 枚不一致は dist プロファイル
  固有の系統差ではなく、dev/dist どちらにも存在する run-to-run ジッタの「悪いケース」**
  であることを確定した。W8/W9 が「2 枚のみ」と報告していたのは、たまたま比較した回が
  ジッタの少ないケースだっただけで、サンプル数が少なかったことによる過小評価だった。
- **原因**: `headless.rs` の自動進行ループが `Instant::now()` と `thread::sleep(16ms)`
  の実時間比較でスクリーンショット/クリックのタイミングを決めており、OS スケジューリング
  の揺らぎ（`thread::sleep` のオーバーシュート、他プロセスとの競合等）がループの
  タイミングに影響していた。加えて、アニメーション（`[move]`/`[trans]`/文字送り/画面揺れ）
  の開始時刻もスクリプトスレッド側で `Instant::now()` を独立に呼んでおり、headless
  側の描画時刻評価用 `Instant::now()` とは別クロックだった（両方とも実時間なので通常は
  近い値になるが、スレッド間のスケジューリング差の分だけズレうる）。特にホバーハイライトは
  「move 送信 300ms 後にスクリーンショット」という**実時間の待ち**を挟むため、この 300ms
  の間に起きるタイミングのブレがホバーアニメーションの進捗（完了しているか否か）に
  直接影響し、複数のホバーフレームが一斉に不一致になりやすい構造だった。
- **tsint（dist 専用エンジン）固有の追加要因**: `tsint_engine.rs::drive()` の
  アイドルポーリング間隔が最大 50ms だった（`op_wait`/`op_audio_wait_end` の期限が
  無い間は 50ms 上限でスリープしてから `input_rx` を汲み出す設計）。headless の
  フレーム刻み（16ms）より粗いため、V8 版（`op_input_next_event` が tokio mpsc の
  `recv().await` で即時起床）と比べて move/click イベントの取りこぼし〜処理までの
  遅延が長くなりやすく、dist（tsint 専用ビルド）の方が経験的にジッタが大きく出やすい
  一因になっていたと考えられる（ただし上記の通り、dev/v8 でも同種のジッタは起きるため
  「dist 固有」ではなく「tsint は悪化しやすい」という位置づけ）。

### 2. 対応: headless 専用の仮想クロック

**目標**: `--autotest` の実行タイミング判断を実時間から切り離し、run-to-run で
同じ仮想時刻列を辿らせる。windowed（win32/x11）は無変更。

- **`crate::clock::Clock`（新規 `crates/noiria-app/src/clock.rs`）**: `Real`
  （`Instant::now()` をそのまま返す、windowed/テスト用、既存挙動と完全に同一）と
  `Virtual { base, nanos: Arc<AtomicU64> }`（`base` からの経過ナノ秒を保持、
  `headless.rs` のフレームループだけが `advance()` で進める）の 2 種類を持つ列挙型。
  `now()`/`advance()` の 2 メソッドのみを公開する最小の抽象。
- **`headless.rs`**: `HeadlessParams` に `clock: Clock` を追加。フレームループ内の
  スクリーンショット判定・クリック判定・トランジション/tween 進捗評価・最終フレーム
  合成まで、すべて `Instant::now()` ではなく `p.clock.now()` を参照するように変更した。
  ループ末尾で `std::thread::sleep(TICK)`（実時間ペーシングは維持。CPU 負荷や全体の
  実行時間感を大きく変えないため）の直後に `p.clock.advance(TICK)`（`TICK` =
  16ms 固定）を呼び、**実際の sleep 時間の長短に関わらず仮想時刻は常に厳密に 16ms
  ずつ進む**。これにより、スクリーンショット/クリックの発生する「仮想 tick 番号」は
  何度実行しても同じになる。
- **`EngineCtx`/`EngineState`（`script/mod.rs`/`script/engine_state.rs`/
  `script/tsint_engine.rs`）**: `EngineCtx`・`EngineState` の両方に `clock: Clock`
  フィールドを追加し、`main.rs` が headless 経路で作った **同一の** `Clock::Virtual`
  インスタンスをスクリプトスレッド側にも共有した（windowed 経路は `Clock::Real` を
  渡すので挙動は不変）。`EngineState` 内でアニメーションの開始時刻を記録している
  箇所（`layer_move`/`layer_move_path`/`layer_moving`/`quake`/`text_print` の
  リベール開始/`text_is_done`/`trans_begin`/`screenshot`/`now()`）の `Instant::now()`
  をすべて `self.clock.now()` に置き換えた。これにより、headless が読む「現在時刻」と
  スクリプトスレッドが tween/transition/reveal の開始時刻として記録する「現在時刻」が
  **同じ仮想クロックの同じ値** になる（V8/tsint 共通、`EngineState` はエンジン非依存の
  共有ロジックのため両エンジンに自動的に適用される）。
- **tsint のアイドルポーリング間隔短縮**（`tsint_engine.rs`）: `drive()` の
  アイドルポーリング上限を `IDLE_POLL_MAX`（2ms、旧 50ms）に縮小した。headless の
  フレーム刻み（16ms）より十分小さくすることで、move/click イベントの取りこぼし〜
  処理までの遅延の上限を縮める。
- **JS/tsint 側のタイマー（`sys.wait`/`op_wait`、tsint のタイマーヒープ、
  `audio_wait_end` の猶予）は意図的に対象外のまま（実時間）**。理由:
  1. `games/sample` の main.ts/scenario には `wait()`/`sys.wait` に相当する呼び出しが
     一切無いことを確認済み（`grep` で確認）。したがって今回問題になった 8 枚不一致には
     この経路は寄与していない。
  2. `tokio::time::pause()`/`advance()`（tokio `test-util` feature）で V8 側の
     `op_wait`（`tokio::time::sleep`）を仮想時刻駆動にする案を検討したが、(a) `advance()`
     の呼び出しは対象ランタイム上の非同期タスクとして行う必要があり、headless
     スレッド（別 OS スレッド）から毎 tick 呼ぶには追加の cross-thread ハンドシェイクが
     要る、(b) tsint はそもそも tokio 非依存の独自タイマーヒープ（`drive()` 内、
     `Instant` ベース）なので `tokio::time` の pause/advance の恩恵を受けず、V8/tsint で
     別々の実装が必要になる、という理由で本ウェーブの最小変更方針とは釣り合わないと
     判断し見送った。将来 JS 側の `wait()`/`setTimeout` 相当が可視フレームの内容に
     影響するシナリオが出てきた時点で再検討する。
- **見送った案**: 「スクリプトスレッドが最初にシーンを変更する（`Scene::revision`
  が動く）まで仮想クロックを 0 に据え置く起動同期ゲート」を試作したが、5 回の
  run-to-run 比較で有意な改善が確認できなかった（後述の残存ジッタの主因ではなかった）
  ため、`CLAUDE.md` の「効果が実証できない複雑さを持ち込まない」方針に従い実装を
  差し戻した。

### 3. 決定論検証の結果

`games/sample`、`--autotest 24`、Windows 実機で検証（すべて 16 枚 = `frame_00`〜
`frame_14_hover` + `frame_final`）。

- **dev（v8）3 回実行の全ペア比較**（3ペア）: 2 ペアは **16/16 バイト完全一致**、
  残り 1 ペアは `frame_03_hover.png` のみ不一致（旧: 最大 8 枚 →
  新: 最大 1 枚、約 87.5% 削減）。
- **dist（tsint）3 回実行の全ペア比較**（3ペア）: 1 ペアは `frame_03_hover.png`
  のみ不一致、残り 2 ペアは `frame_00.png`/`frame_03_hover.png` の 2 枚不一致
  （旧: 8 枚 → 新: 最大 2 枚）。
- **dev vs dist**（1 サンプル）: **16/16 バイト完全一致**（対応前は dev/dist 比較
  自体が不安定でジッタに埋もれていた）。
- **pak vs dir**（dist ビルド）: **16/16 バイト完全一致**。
- 以上により「16 フレーム全てが 3 回とも完全一致」という受け入れ基準は**達成できて
  いない**が、run-to-run 差異の枚数は劇的に減少した（最悪 8 枚 → 最悪 2 枚）。
- **残存ジッタの原因**: headless（仮想クロック駆動）とスクリプトスレッド（V8 の
  tokio イベントループ、または tsint の独自ポーリングループ。いずれも別 OS
  スレッド）の間には、依然として「送信した入力イベントをスクリプト側が実際にいつ
  処理し、`self.clock.now()` を読むか」という実スレッドスケジューリング依存の
  レースが残る。仮想クロックは headless 側でしか進めないため、スクリプト側が
  ある入力イベントを処理する瞬間に読む値は、headless が同じ実時間の瞬間に
  すでに次の仮想 tick へ進めているかどうかで ±1 tick（16ms）だけブレうる。
  この ±1 tick の差は通常アニメーションの完了判定に影響しないだけの余裕
  （tween の duration に対して十分な余裕を見て screenshot している）があるため
  ほとんどのフレームでは可視化されないが、`frame_03_hover`（まれに `frame_00`）は
  たまたま判定境界の近くにあり、この残存レースがそのまま可視化されると考えられる。
  **完全に閉じるには、headless が入力イベントを送るたびにスクリプト側の処理完了を
  待ち合わせるロックステップ（毎 tick バリア同期）が必要**で、これはスクリプト
  スレッドの実行モデル（V8 の非同期タスクポンプ、tsint の独自イベントループ）に
  ハンドシェイク機構を新設する必要がある大掛かりな変更になるため、本ウェーブでは
  見送り、ここに申し送る。

### 4. テスト数の変化・変更ファイル一覧

- **Windows**: `cargo test --workspace` = **374 件全通過**（0 failed）。
  **課題文の基準値「362 件」と一致しない（+12）**。本ウェーブでは新規テストを
  1 件も追加していない（既存シグネチャへのフィールド追加のみ）ため、この差分は
  本ウェーブの変更由来ではない。実際、着手後に本ファイルの末尾に「2026-07-13
  wav-flac LPC 拡張」という別ウェーブの節が（別セッションの並行作業と見られる形で）
  追記されているのを確認しており、そちらのテスト追加が基準値との差分の主因と
  考えられる。
- **WSL**: `scripts/wsl-test.sh --skip-x11` で `cargo test --workspace` =
  **386 件全通過**（0 failed）+ `--autotest 24` が EXIT 0・16 フレーム保存を確認
  （Windows と同様、こちらも基準値「372 件」とは +14 の差があるが理由は同上）。
  ALSA スモークテスト（ベンダリング経由）も 2 件通過。X11 は `--skip-x11` のため
  未実施（本タスクは windowed 経路無変更のため対象外と判断）。
  **申し送り**: WSL で追加の run-to-run 比較（`cargo run` を 2 回連続実行して
  diff）を試みたところ、ワークスペース外の別リポジトリ `vp9dec`（`docs` 記載の
  方針通り単独開発中、`git status` で未コミット差分を確認）が WSL 側で一時的に
  ビルド不能な状態（`frame_context_reset`/`FrameContextReset` 未定義、Windows 側の
  同ソースは問題なくビルドできるため Linux 向けパスを並行編集中と推測）になっており、
  完遂できなかった。本タスクの変更差分には一切含まれないファイルの問題のため
  対応せず放置する。Windows 側では 3 回×2 ビルド（dev/dist）+ pak/dir の比較を
  完了済み（上記 3 節）。

#### 変更ファイル一覧

- `crates/noiria-app/src/clock.rs`（新規）: `Clock`（`Real`/`Virtual`）抽象。
- `crates/noiria-app/src/main.rs`: `mod clock;` 追加。headless 経路で
  `Clock::new_virtual(start)` を作成し `EngineCtx`/`HeadlessParams` の両方へ同じ
  インスタンスを共有。win32/x11 windowed 経路の `EngineCtx` には `Clock::Real` を渡す
  （挙動不変）。
- `crates/noiria-app/src/headless.rs`: `HeadlessParams` に `clock: Clock` を追加。
  フレームループの全スケジューリング判定・`compose_frame`/`settle_finished_tweens`
  評価を `p.clock.now()` ベースに変更、ループ末尾で `p.clock.advance(TICK)`
  （`TICK` = 16ms 定数化）。`thread::sleep(TICK)` 自体は実時間ペーシングとして維持。
- `crates/noiria-app/src/script/mod.rs`: `EngineCtx` に `clock: Clock` 追加、
  `EngineState::new` 呼び出しへ配線。テスト用 `EngineCtx` 構築 3 箇所に
  `clock: Clock::Real` を追加。
- `crates/noiria-app/src/script/engine_state.rs`: `EngineState` に `clock: Clock`
  フィールド追加（コンストラクタ引数も追加）。`layer_move`/`layer_move_path`/
  `layer_moving`/`quake`/`text_print`（リベール開始）/`text_is_done`/`trans_begin`/
  `screenshot`/`now()` の `Instant::now()` を `self.clock.now()` に置換
  （動画再生スレッド `playback_loop` の `Instant::now()` は実時間のまま無変更、
  本タスクのスコープ外）。テストヘルパー `make_engine_state` に `Clock::Real` 追加。
- `crates/noiria-app/src/script/tsint_engine.rs`: `EngineCtx` の分解パターン・
  `EngineState::new` 呼び出しに `clock` を追加。`IDLE_POLL_MAX`（2ms、旧 50ms 相当）
  定数を追加し `drive()` のアイドルポーリング上限に適用。テスト用 `EngineCtx`
  構築 3 箇所に `clock: Clock::Real` を追加（`Clock` の import は `#[cfg(test)]`
  限定、非テストコードでは型名を直接使わないため）。`op_wait`/`op_audio_wait_end`
  の `Instant::now()` は意図的に無変更。
- `crates/noiria-app/src/script/tag_harness.rs`: `EngineCtx` 構築に
  `clock: crate::clock::Clock::Real` を追加のみ（タグ振る舞いテストハーネス自体の
  実時間 quiescence 同期ロジックは無変更、指示通り触っていない）。
- `docs/implementation-notes.md`: 本節を追記。

### 5. 未解決・申し送り事項

- **「3 回とも 16 フレーム全一致」は未達成**（最悪ケースで dist 2 枚、dev 1 枚の
  不一致が残る）。原因は 3 節で述べた通り、headless⇔script 間の cross-thread
  レース（±1 virtual tick）。完全解消にはロックステップ（毎 tick バリア同期）の
  導入が必要で、スクリプトスレッドの実行モデルに踏み込む大掛かりな変更になるため
  本ウェーブでは見送った。次ウェーブで着手する場合の入り口は
  `tsint_engine.rs::drive()`/`script/mod.rs::run_script` の入力イベント処理後に
  ack チャンネルで headless へ通知する設計を想定している。
- **JS/tsint タイマー（`sys.wait` 等）は実時間のまま**: 上記 2 節の理由により
  本ウェーブでは対象外。`games/sample` の検証範囲では影響を確認していないが、
  `wait()`/`setTimeout` 相当の完了タイミングが可視フレームに影響するシナリオが
  今後出てきた場合は別途対応を検討する必要がある。
- **テスト数の基準値との差分（Windows 362→374、WSL 372→386）**: 本ウェーブの
  変更によるものではなく、並行して進んでいた別ウェーブ（wav-flac LPC 拡張）の
  テスト追加が原因と見られる。実測値（374/386、いずれも 0 failed）自体は
  問題ない。
- **WSL での run-to-run 追加検証は未完了**: ワークスペース外の `vp9dec` リポジトリが
  一時的にビルド不能だったため。Windows 側での検証（3 節）で代替した。

## 2026-07-13 アイコン埋め込み

W7 でストレッチ目標として見送っていた「実行ファイル/ウィンドウへのアイコン埋め込み」に着手した。
対象: `crates/noiria-tools/src/bin/gen_icon.rs`（新設、アイコン画像の手続き生成）、
`crates/noiria-tools/Cargo.toml`（`gen-icon` bin 追加）、`crates/noiria-app/assets/icon.png`+
`icon.ico`（新設、`gen-icon` の生成物）、`crates/noiria-app/build.rs`（新設、`.ico`→`.res`
自前変換）、`crates/noiria-app/src/icon.rs`（新設、`AppIcon`: ゲーム側 `icon.png` 優先 + 埋め込み
エンジン既定・最近傍リサイズ・Win32 ICONIMAGE 変換）、`crates/noiria-app/src/win32.rs`
（`WM_SETICON` 配線）、`crates/noiria-app/src/x11/proto.rs`（`XA_CARDINAL` 定数 + テスト用
`GetProperty` ヘルパ）、`crates/noiria-app/src/x11/window.rs`（`_NET_WM_ICON` 配線）、
`crates/noiria-app/src/x11/window/tests.rs`（Xvfb 検証テスト追加）、`crates/noiria-app/src/main.rs`
（`mod icon;` + `GameDir::read_bytes("icon.png")` → `WindowedParams` への配線）、
`crates/noiria-tools/src/export.rs`（README.txt にアイコンの説明を1行追加）、
`games/sample/icon.png`（新設、`gen-icon -- sample` の生成物）。ベースラインは
Windows cargo test 374 件 / WSL 386 件全通過（ロックステップ決定論化ウェーブ完了後）。

### 1. アイコン画像の生成（手続き生成、`gen-icon`）

- **判断**: 新規画像アセットは一切使わず、`crates/noiria-tools/src/paint.rs`（SDF ベースの
  図形描画基盤、既存の `gen-w3-icon` 等と同じ仕組み）だけで手続き生成した。エンジン既定は
  角丸バッジ（インディゴ→バイオレットの縦グラデーション）に太字の "N" モノグラム（3 本の
  丸端線分 `aa_line`）、サンプルゲーム用は暖色バッジにきらめきバッジ（`gen-w3-icon` の
  スパーク意匠を 256px へ拡大流用）。**理由**: 新規外部画像を持ち込まない・決定論的に
  再生成可能（ソースは Rust コードのみ）という既存方針（`gen-assets` 一式）を踏襲した。
  16px でも視認できるよう、線の太さ (34px/256px 中) とグリフの余白を大きめに取った。
- **判断**: 角丸矩形のグラデーション塗りは `paint.rs` の非公開 `sdf_round_rect` を
  `gen_icon.rs` 内に複製して実装した（`paint::aa_rounded_rect` は単色塗りのみで、
  行ごとに色を変えるグラデーション塗りつぶしには対応していないため）。`paint.rs` 自体は
  無変更（既存の `gen-assets` 生成物に影響を与えないため、CLAUDE.md の「触るのは自分の
  変更に必要な範囲だけ」を優先し、共通化のための `paint.rs` 拡張は行わなかった）。
- **判断**: PNG エンコードは `noiria_core::codec::png::encode`（自作エンコーダ）を直接使い、
  `image` クレートの PNG 書き出しは使わなかった。**理由**: `.ico` の 256px エントリに埋め込む
  PNG バイト列と、単体で書き出す `icon.png` を同一のエンコーダ・同一バイト列にできる
  （エンコーダの違いによる余計な差異を持ち込まない）。

### 2. `.ico` 形式の選択

- **調査結果**: Windows の `.ico` はサイズごとに「BMP DIB（BITMAPINFOHEADER + XOR 色データ +
  AND マスク）」または「生の PNG ファイルバイト列」のどちらかをエントリに格納できる
  （ICONDIRENTRY の中身をどちらとして解釈するかは、データの先頭が PNG シグネチャ
  (`89 50 4E 47 ...`) かどうかで判定される）。PNG 格納は Vista 以降のみ対応、かつ
  32bpp の巨大な BMP DIB（256×256 で無圧縮 32bpp ≈ 256KB）を避けられる。
- **判断**: 計画通り 16/32/48 は BMP DIB、256 は PNG 埋め込みにした。**理由**:
  16/32/48 のような小サイズは BMP DIB の方が古いツール・GDI 由来の経路（`CreateIconFromResourceEx`
  含む）との互換性が高く実績もある一方、256 は BMP DIB だと 1 エントリだけで数百 KB になり
  「軽量配布」という M2 の全体方針と整合しないため PNG 圧縮の恩恵が大きい。
- **判断**: BMP DIB の AND マスクは全 0（1bpp、行を 4 バイト境界にパディング、bottom-up）にした。
  **理由**: 32bpp の XOR 色データに実アルファチャンネルを持たせているため（Vista+ で正しく
  半透明合成される）、AND マスクは「常に不透過（=アルファ任せ）」を意味する全 0 にするのが
  現代的なアイコン制作ツールの標準的なやり方。
- **検証**: 生成した `.ico` を PowerShell の `[System.Drawing.Icon]` で読み込み、
  32×32・16×16 の BMP DIB エントリが正しく透過込みでデコードできることを確認した
  （下記「検証結果」参照）。`.NET` の `System.Drawing.Icon`（GDI+ ラッパ）は 256 サイズの
  PNG 格納エントリを認識せず次点の 48×48 にフォールバックすることも確認した — これは
  GDI+ 側の既知の制限であり、`.ico` ファイル自体の構造は手動バイトパースで全エントリの
  オフセット/サイズ整合性を検証済み（末尾エントリの offset+size が総ファイル長と一致、
  各エントリ先頭バイトが期待どおり BMP ヘッダ (`28 00 00 00` = biSize=40) / PNG シグネチャ
  であることを確認）。Windows Explorer 等のネイティブシェル icon ローダは PNG 格納エントリに
  対応しているため、大アイコン表示時の実害は無い。

### 3. `.res`(COFF ではなく Win32 32-bit 新形式リソースファイル) の自前生成

- **判断**: `build.rs` は `assets/icon.ico` の `ICONDIR`/`ICONDIRENTRY` を解析して各アイコン
  実体を取り出し、`RT_ICON`（アイコン実体 × N、ID は 1 始まりの連番）+ `RT_GROUP_ICON`
  （`GRPICONDIR`/`GRPICONDIRENTRY[]`、`.ico` の `ICONDIRENTRY` とほぼ同型だが末尾が
  `imageOffset`(DWORD) ではなく対応する `RT_ICON` の ID(WORD) になる）から成る `.res`
  バイト列を組み立てた。各リソースエントリは Type/Name とも序数（`0xFFFF` + WORD ID）で
  書くため、ヘッダ長は常に 32 バイト固定（可変長の文字列 Type/Name は使わない、必要が
  無いため）。先頭に「空リソース」ヘッダ（DataSize=0 の同型 32 バイト）を置くのは
  32-bit 新形式 `.res` であることを示す決まり文句。
- **判断**: `MemoryFlags=0x1030`（MOVEABLE|PURE|DISCARDABLE 相当）・`LanguageId=0x0409`
  （en-US）は `rc.exe` の典型的な出力値をそのまま採用した。**理由**: これらは 16-bit
  Windows 時代の名残りのフィールドで、32-bit PE のリソースローダはほぼ参照しない
  （実害があるなら exe が起動しない/アイコンが出ないという分かりやすい形で現れるはずだが、
  実機検証（後述）で問題なく反映された）。
- **判断**: `.res` は `OUT_DIR/icon.res` に書き出し、`cargo:rustc-link-arg-bins=<path>` で
  MSVC の `link.exe` に直接渡した（`rc.exe`/`cvtres.exe` は起動しない。`link.exe` は
  `.res` をそのままリンク入力として受け付けるため、自前生成した `.res` をそのまま渡せる）。
  `windows-gnu` ターゲットは `.res` 直接リンクの標準経路が無いため `build.rs` は
  `CARGO_CFG_TARGET_ENV != "msvc"` のとき（Linux も含め）何もしない。
- **検証**: `cargo build -p noiria-app` 後、`target/debug/noiria.exe` を
  `[System.Drawing.Icon]::ExtractAssociatedIcon` で読み込み、32×32 の "N" アイコンが
  正しく抽出できることを確認した（下記「検証結果」参照）。

### 4. 実行時ウィンドウ/タスクバーアイコン(`AppIcon`, `icon.rs`)

- **判断**: `AppIcon::from_optional_png(Option<&[u8]>)` という単純な関数にし、`GameDir`
  への依存を持たせなかった。**理由**: `main.rs` 側で `GameDir::read_bytes("icon.png").ok()`
  を呼んでバイト列だけを渡す形にすれば、`icon.rs`（および `win32.rs`/`x11` のテスト）が
  `GameDir` を一切知らずに済み、テストで実ゲームディレクトリを用意する必要が無くなる
  （`GameDir::for_test` は `noiria-core` 内 `pub(crate)` でクレート外から使えないため、
  この設計にしないとテストがゲームディレクトリの用意に苦労していた）。
- **判断**: `AppIcon` のデコードは PNG マジックだけでなく WebP（`RIFF`/`WEBP`）もマジック
  バイトで判定するようにした（`noiria_core::assets::decode_image_bytes` と同じ方針）。
  **理由**: `noiria-tools::pack::pack_dir`（W8）は拡張子 `.png` のエントリを機械的に
  WebP へ変換する（元より小さければ）ため、`icon.png` も対象になる。パス名は `.png` の
  ままでも中身が WebP になり得るので、PNG 決め打ちのデコードだと export 済みゲームの
  カスタムアイコンが常に既定アイコンへ静かにフォールバックしてしまうバグになる
  （実装中に気付いて修正、`from_optional_png_sniffs_webp_by_magic` テストで固定した）。
- **判断**: `Pixmap` にリサイズが無いため、最近傍リサイズを `icon.rs` に自前実装した
  （`nearest_resize`）。**理由**: アイコンは起動時に 1 回、16/32/48 程度の小サイズへ
  縮小するだけの用途で、品質要求も低い（計画の「最近傍で十分」の判断どおり）。
- **判断**: Win32 の ICONIMAGE 変換関数 (`build_dib_icon_image`) は `crates/noiria-tools/
  src/bin/gen_icon.rs` と `crates/noiria-app/src/icon.rs` の 2 箇所に複製した。**理由**:
  前者はオフライン生成（`.ico` を作る）、後者は実行時（`CreateIconFromResourceEx` に渡す）
  という別クレート・別プロセスの用途で、`noiria-tools`→`noiria-app` の依存方向が無い
  （逆方向にすると export ツールがゲームエンジンの実行バイナリに依存する歪な構造になる）。
  ロジック自体は ~25 行の純粋関数で、共有クレートを新設するほどの複雑さではないと判断した
  （CLAUDE.md 簡潔性方針、「単一用途の抽象化を作らない」）。

### 5. Windows: `WM_SETICON`

- **判断**: `CreateIconFromResourceEx`（`CreateIcon` ではなくこちら）を採用した。**理由**:
  計画で両方が候補として挙がっていたが、`CreateIconFromResourceEx` は `.ico` ファイル内の
  ICONIMAGE と全く同じバイト列表現をそのまま受け取れる API で、`.ico` 読み込み時と同じ
  内部経路を通るため 32bpp アルファの扱いが確実（`CreateIcon` は内部で `CreateBitmap`
  （デバイス依存ビットマップ）を経由するため、タスクバー等の DWM 合成パスでアルファが
  正しく反映されない既知の落とし穴がある）。`build_dib_icon_image` の出力をそのまま渡せる
  ため実装も単純になった。
  - `ICON_SMALL`(16×16) と `ICON_BIG`(32×32) の 2 サイズを `WM_SETICON` で設定する
    （Windows の標準サイズ、`SM_CXSMICON`/`SM_CXICON` に対応）。
- **判断**: 生成した `HICON` は `run_windowed` のメッセージループ終了時に `DestroyIcon`
  で破棄する（プロセス終了まで保持するのではなく、`run_windowed` のライフタイムに揃えた）。
- **検証**: 既存の不可視ウィンドウ統合テスト（`win32::tests::invisible_window_input_
  roundtrip_and_clean_quit`）に `icon: AppIcon::from_optional_png(None)` を追加し、
  `WM_SETICON` 呼び出し経路（`set_window_icon`）がエラーなく通ることを確認した
  （窓は非表示のまま、既存アサーションは無変更）。

### 6. Linux: `_NET_WM_ICON`

- **判断**: `x11::proto` に `XA_CARDINAL`（定義済みアトム、値 6）を追加し、`_NET_WM_ICON`
  は `InternAtom` で都度解決した（他の `_NET_WM_NAME` 等と同じパターン）。プロパティ値は
  16/32/48 の 3 サイズを 1 つの CARDINAL 配列に連結した（EWMH 仕様どおり、各サイズの先頭
  2 要素が `width, height`、続けて `width*height` 個の `0xAARRGGBB` ARGB pixel）。
  W2 で実装済みの `req_change_property`（汎用 ChangeProperty ヘルパ）をそのまま再利用した
  （新規のワイヤ関数は不要だった）。
- **判断**: `_NET_WM_ICON` の設定は `setup()` 内、ウィンドウ生成直後・`MapWindow` 前に
  行った。**理由**: 他のプロパティ（`WM_NORMAL_HINTS`/`WM_PROTOCOLS`/タイトル）と同じ
  タイミングに揃えることで、WM がウィンドウを検出した時点で全プロパティが揃っている
  ことを保証する（マップ後に後追いで設定すると、WM 実装によっては最初のタスクバー
  描画に間に合わない可能性がある）。
- **検証**: Xvfb 上の新規統合テスト `net_wm_icon_property_matches_app_icon`（`x11/window/
  tests.rs`）を追加した。`run_windowed` を不可視ウィンドウで起動し、別クライアントとして
  接続して `GetProperty`（新規テスト用ヘルパ `proto::get_property_u32s`）で `_NET_WM_ICON`
  を読み戻し、先頭サイズが `16,16` であること・先頭ピクセルの ARGB が `AppIcon::image(16)`
  の計算結果と一致することを確認する（下記「検証結果」参照）。

### 7. exe 個別アイコン(PE リソースパッチ) — 見送り

- **判断**: 計画・W7 notes で明記した通り、ゲームごとに exe 自体のアイコンを差し替える
  （PE リソースセクションパッチ）は本ウェーブでも実装しなかった。**理由**: `.rsrc` セクション
  内の `RT_GROUP_ICON`/`RT_ICON` を安全に差し替えるには PE セクションレイアウト
  （RVA/ファイルオフセット変換、セクションサイズ変更に伴う後続セクションの再配置や
  チェックサム再計算）まで踏み込む必要があり、`export.rs` の `KEY_SLOT` バイナリパッチ
  （固定サイズ領域の単純上書き）とは複雑さの桁が違う。エンジン既定の exe アイコン +
  実行時ウィンドウ/タスクバーアイコン（本ウェーブで実装済み）の 2 層で、配布物が
  無印アイコンにならないという当初の見た目上の課題は解消されるため、追加の複雑さに
  見合う価値が無いと判断した。**影響**: 配布 exe のファイルアイコン（Explorer 上の
  アイコン）は全ゲーム共通のエンジン既定のまま。ウィンドウ/タスクバーのアイコン
  （実行中に見える方）はゲームごとの `icon.png` が反映される。

### 検証結果 (2026-07-13)

- **exe リソース埋め込み（機械検証）**: `cargo build -p noiria-app`（dev）/
  `cargo build --profile dist --no-default-features --features engine-tsint,dist -p noiria-app`
  （dist）の両方で `target/{debug,dist}/noiria.exe` を生成し、PowerShell
  `[System.Drawing.Icon]::ExtractAssociatedIcon($exe)` で読み込んだところ両方とも
  32×32 の "N" アイコンが正しく抽出できた（`build.rs` が dev/dist 双方のビルドプロファイル
  で正しく `.res` を生成・リンクしていることを確認）。`.ico` ファイル自体もバイト列を
  手動パースし、4 エントリ（16/32/48 = BMP DIB, 256 = PNG）のオフセット/サイズが
  ファイル終端まで過不足なく整合していること、各エントリ先頭バイトが期待どおり
  （BMP: `28 00 00 00`、PNG: `89 50 4E 47`）であることを確認した。
- **Windows: WM_SETICON 経路**: 既存の不可視ウィンドウ統合テスト
  `win32::tests::invisible_window_input_roundtrip_and_clean_quit` に `icon` フィールドを
  追加した状態で実行し、`ok`（既存アサーション含め全て通過）を確認した。窓は不可視のまま。
- **Linux/Xvfb: `_NET_WM_ICON` 読み戻し**: 新規テスト
  `x11::window::tests::net_wm_icon_property_matches_app_icon` が WSL の Xvfb 上で `ok`
  （`scripts/wsl-test.sh` の `[4/4]` ステージ、`cargo test -p noiria-app --bin noiria x11:: --
  --test-threads=1` 経由）。先頭サイズ `16×16`・先頭ピクセルの ARGB が `AppIcon::image(16)`
  の計算結果と一致することを確認した。
- **export E2E — Windows**: `games/sample`（`icon.png` 同梱）を
  `cargo run -p noiria-tools --bin noiria-export -- --game games/sample --out target/export`
  で export（pack エントリ数 23→**24**、`icon.png` が pak に含まれるようになったことを
  裏付ける）。`target/export/windows/Noiria.exe --autotest 20` を EXIT=0・PNG 14 枚
  出力で確認。ログ（`%APPDATA%\Noiria\noiria-sample\save\noiria.log`）に icon.png の
  デコード失敗警告（`AppIcon::from_optional_png` が既定へフォールバックする際に出す
  `log::warn!`）が出ていないことを確認した（= ゲーム固有アイコンが正しくデコードできた
  ことの間接証跡。W8 の webp 自動変換が `icon.png` にも適用されるため、この確認は
  WebP スニッフィング修正の実地検証を兼ねる）。
- **export E2E — Linux**: `scripts/build-dist-wsl.sh` で Linux dist バイナリを再ビルドし、
  同じ export コマンドで `windows,linux` 両ターゲットを export。WSL で
  `target/export/linux/Noiria --autotest 15` を実行し EXIT=0・PNG 出力を確認。
  `~/.local/share/noiria-sample/save/noiria.log` にも icon 関連の警告・panic が
  無いことを確認した。
- **WSL 全体の `scripts/wsl-test.sh`**: `[1/4] cargo test --workspace` 全通過、
  `[2/4] --autotest`、`[3/4] ALSA スモーク`（`音声デバイス open 成功` 証跡あり）、
  `[4/4] X11(Xvfb) 統合テスト + games/sample の windowed 10 秒スモーク`（panic 無し、
  `target/x11-smoke/games_sample_screenshot.png` にスクリーンショット保存、目視で
  正常描画を確認）まで全ステージが正常終了した。
- **テスト数**: Windows `cargo test --workspace` **374 → 379 件**全通過（新規 5 件、
  `icon::tests::*`。`x11::` 配下は `#[cfg(unix)]` のため Windows ビルドには含まれない）。
  WSL `scripts/wsl-test.sh` 内の `cargo test --workspace` は **386 → 391 件**全通過
  （新規 5 件は Windows と同じ `icon::tests::*`、加えて `x11::window::tests::
  net_wm_icon_property_matches_app_icon` が Unix ビルドにのみ追加される。`[4/4]` の
  `x11::` フィルタ実行では新規テストを含め 13 件全通過）。0 failed。
- **clippy**: `cargo clippy --workspace --all-targets`（Windows）で新規ファイル
  （`build.rs`/`icon.rs`/`gen_icon.rs`/`win32.rs`/`x11/proto.rs`/`x11/window.rs`/
  `export.rs`/`main.rs` の変更差分）由来の警告は 0 件。既存の警告（`paint.rs` の
  `h_span_aa` dead_code 等）は本ウェーブの変更前から存在するもので対象外とした。
- **アイコン画像（目視確認用）**: `crates/noiria-app/assets/icon.png`（256×256、
  エンジン既定、"N" モノグラム）・`crates/noiria-app/assets/icon.ico`（16/32/48 BMP DIB +
  256 PNG）・`games/sample/icon.png`（256×256、サンプル用きらめきバッジ）。
  `.ico` から PowerShell で抽出したビットマップは
  `target/icon-verify/icon_from_ico_16.png`・`icon_from_ico_32.png`・
  `icon_from_ico_256.png`（256 は GDI+ の制限で 48 相当にフォールバックする、
  2 節参照）、exe から抽出したものは `target/icon-verify/exe_icon_extracted.png`
  として保存した（いずれもユーザー目視確認用）。

## 2026-07-13 tsint tick ドレインバリア（完全決定論化の仕上げ）

前ウェーブ（「2026-07-13 dist フレーム差異の調査と headless 決定論化」、および
その後の入力イベントごとのロックステップ = `bridge.rs::InputSync`/
`headless.rs::wait_for_ack`/両エンジンの `input_sync.ack()`。本ファイルへの記載が
漏れていた模様で、コード自体は今回の着手時点で既に存在していた）で残った最後の 1 点、
「tsint 固有のフレームレース（`frame_03_hover.png` の間欠不一致）」を根治した。
ベースライン: Windows cargo test 379 件 / WSL 391 件全通過。

### 1. 根本原因（再確認）

`tsint_engine.rs::drive()` のアイドルポーリング（`sleep_for = min(次の期限 - 仮想now,
IDLE_POLL_MAX=2ms)` を実時間で `thread::sleep`）と、headless.rs のフレームループ
（`clock.advance(16ms)` した直後に `compose_frame` + スクリーンショット）との間には、
入力イベント自体の ack（`InputSync`）で閉じられる区間を除き、バリアが無かった。
そのため「headless が仮想 tick を進めてスクショを撮るタイミング」と「tsint が実時間
ポーリングで同じ仮想 tick に到達し、期限が来たタイマー/オーディオ猶予を処理して scene
を更新するタイミング」の前後関係が実時間スケジューリング依存でブレていた。

### 2. 対応: tick ドレインバリア（`TickSync`）

`InputSync` と同じ「headless 側が待ち、スクリプトスレッド側が ack する」ロックステップの
考え方を、**入力イベントの有無に関わらず毎 tick** 効かせる形に一般化した。

- **`bridge.rs::TickSync`（新規）**: `requested: Arc<AtomicU64>` /
  `completed: Arc<AtomicU64>` の 2 カウンタを持つ。
  - headless 側 `request()`: `Clock::advance()` の直後に呼ぶ。`requested` を
    `SeqCst` で fetch_add し、新しい要求世代を返す。
  - スクリプト側 `requested_generation()`: 最新の要求世代を `SeqCst` load で読む。
  - スクリプト側 `mark_complete(gen)`: `completed` を `fetch_max` で更新する
    （逆行防止）。
  - headless 側 `completed_generation()`: `SeqCst` load で完了世代を読む。
  - **正しさの根拠（happens-before）**: `clock.rs` の `nanos` は意図的に
    `Ordering::Relaxed` のまま変更していない。その代わり、tsint の `drive()` ループは
    **`requested_generation()` を読んでから `Clock::now()` を読む**という順序を厳守する
    設計にした。`request()`/`requested_generation()` はどちらも `SeqCst`（release/
    acquire を含む）なので、tsint 側が headless のある `request()` 呼び出し（の返す値
    以上）を `requested_generation()` で観測した場合、release シーケンスにより
    「その `request()` 呼び出しより**前**に headless が行った `Clock::advance()`」が
    「その `requested_generation()` の**直後**に読む `Clock::now()`」から確実に見える
    ことが、C++/Rust のメモリモデル上保証される（同じロジックで `nanos` 自体を
    `Relaxed` のままにできる）。単純に「headless が `advance()` 直後に
    `TickSync` の世代を読んで `> gen_before` を待つ」設計（`InputSync` と同型）も
    検討したが、2 つの独立した `AtomicU64`（`nanos` と生成世代カウンタ）の間には
    SeqCst 単体では happens-before が伝播しない（同じ実時間の前後関係だけでは
    「tsint が古い `now` を読んでから新しい ack を出す」という stale-ack のレースを
    理論上排除できない）ため、**要求世代の読み取りを `now` の読み取りより先に
    行わせて依存関係で縛る**現在の設計に変更した。
  - tsint の `drive()` ループ先頭で `now` を読む直前に `requested_generation()` を
    読み、そのループが「今回は何も解決しなかった（`!resolved_any`、＝タイマー/
    トランジション/オーディオ猶予/入力のいずれも処理しなかった）」場合にのみ、
    その世代で `mark_complete()` する。「何か解決した」ループはそのまま即座に次の
    ループへ進む（sleep しない、既存のビジーループ設計のまま）ので、同じ仮想 tick 内で
    連鎖的に解決するタイマー（例: ゼロ待ちの継続）があっても、完全に処理し切ってから
    complete を報告する。
- **`headless.rs`**: `HeadlessParams` に `tick_sync: TickSync` と `tick_barrier: bool`
  を追加。フレームループ末尾、`p.clock.advance(TICK)` の直後に
  `if p.tick_barrier { let gen = p.tick_sync.request(); wait_for_tick_drain(&p.tick_sync, gen); }`
  を呼ぶ。`wait_for_tick_drain` は既存 `wait_for_ack` と同じ `ACK_TIMEOUT`（3秒）/
  `ACK_POLL`（200μs）でポーリングし、タイムアウトしても警告ログのみでハングさせない
  （デッドロック安全性は既存パターンを踏襲）。
- **`tick_barrier` の決定（V8 対象外の理由）**: `main.rs` に
  `resolved_engine_is_tsint(name)`（`spawn_script` と同じフォールバック規則で実際に
  起動されるエンジンを判定するヘルパ、4 通りの feature 組合せそれぞれに対応する関数を
  用意）を追加し、ヘッドレス起動時にこれで `tick_barrier` を決める。**V8 はこのバリアの
  対象から意図的に外した**。理由: V8 はユーザー自身の実測で「3 回実行の全フレーム一致」
  という既に高い決定論を達成しており（本ウェーブ着手前に確認済み。5 節も参照）、
  そのバリアなしの経路を変えるリスクを負いたくなかったこと、加えて技術的な理由として
  V8 の `run_script` は tokio の `poll_event_loop` ベースで実イベント（実時間タイマー
  `tokio::time::sleep`、input チャンネルの `recv`）が起きたときにしか再ポーリングされない
  ため、毎 16ms tick ごとに `TickSync::request()` を出して `mark_complete` を待つ設計を
  そのまま適用すると、真に何のイベントも起きていない tick では V8 タスクが二度と
  再ポーリングされず `ACK_TIMEOUT`（3秒）に毎 tick 到達してハングに近い状態になる
  （`--autotest 24` が実質的に終わらなくなる）。V8 に安全にこのバリアを適用するには
  仮想クロック駆動の定期ウェイク機構（`tokio::time::pause`/`advance` 相当）を新設する
  必要があり、これは前ウェーブが「本ウェーブの最小変更方針とは釣り合わない」として
  見送った変更と同種の踏み込みになるため、今回も見送った。windowed 経路は
  `headless.rs` 自体が実行されないため無条件に無関係（`tag_harness.rs` は `EngineCtx`
  構築に `tick_sync: TickSync::new()` を追加しただけで、ハーネス自体のロジックは無変更）。

### 3. 副次的に発見・修正したバグ: tsint の入力 ack が構造的に発火しない問題

上記バリアの検証中、`--autotest 24`（tsint）が異常に遅い（実時間で約 2 分、24 秒の
仮想テストのはずが）ことに気づき、`bridge.rs::InputSync` の入力イベント ack が
**ほぼ毎回 `ACK_TIMEOUT`（3秒）に達してから諦めるフォールバックでしか進んでいない**
ことをログで確認した（新設した tick ドレインバリア自体は一度もタイムアウトしておらず、
このバリアが遅延の原因ではないことは `tick_barrier` を一時的に無効化した実験で先に
切り分け済み）。

原因は `tsint_engine.rs::drive()` の 6.5 節（ack 発火条件）が
`if matches!(pump, PumpState::Idle) { ... }` で `pump`（`interp.run_event_loop()` の
戻り値）を `Idle` に限定していたこと。tsint 本体（`../../../tsint` リポジトリ、
`src/vm.rs::Vm::run_event_loop`）を確認したところ、この関数は**マイクロタスクキューを
Idle/Waiting どちらの戻り値でも必ず完全に汲み尽くしてから**、`pending_host_count`
（未解決のホスト Promise 数）が 0 なら `Idle`、1 以上なら `Waiting` を返す設計だった。
`op_input_next_event` が「次の入力を待つ」ために作る `HostPromise`（`p.input = Some(handle)`）
自体が未解決のホスト Promise としてカウントされるため、**まさに ack を出したい瞬間
（`p.input.is_some()` が真になった直後）は `pending_host_count >= 1` が保証されており、
`PumpState::Idle` には理屈上なり得ない**。つまり `matches!(pump, PumpState::Idle)` の
条件は入力待ちの通常状態と両立せず、6.5 節の ack はモジュール終了時（7 節、全て空になる
とき）以外では構造的に発火しなかった（`InputSync` 導入以来ずっとこの状態だったと
見られる）。

**修正**: `run_event_loop()` の戻り値でゲートするのをやめ、マイクロタスクは戻り値に
関わらず汲み尽くされている事実だけを根拠に、`should_ack` の判定（`p.input.is_some() &&
p.input_buffer.is_empty() && !p.input_acked`）を無条件に行うようにした（`pump` 自体は
7 節の終了判定でそのまま使うので変数は残す）。

**効果**: `--autotest 24`（dev, tsint）が実時間 約2分11秒 → 約33秒に短縮（V8 の約11秒に
近い水準）。ACK タイムアウト警告は 0 件に。フレーム内容自体は本修正の前後どちらでも
tick ドレインバリアにより 3 回完全一致していた（4 節参照）ため、この修正は「決定論の
成立」には必須ではなかったが、正常系での実行速度・ログの健全性・将来のデバッグ容易性の
観点から合わせて修正した。

### 4. 決定論検証の結果（`games/sample`、`--autotest 24`、16 フレーム）

すべて Windows 実機。`cmp` によるバイト完全一致比較（3 回の全ペア = 3 組）。

- **dev（tsint）3 回**: 3 組すべて **16/16 バイト完全一致**。`frame_03_hover.png` の
  間欠不一致は解消（0/3 組で不一致）。
- **dist（tsint、`cargo build --profile dist --no-default-features --features
  engine-tsint,dist`）3 回**: 3 組すべて **16/16 バイト完全一致**。
- **dir 起動 vs pak 起動（dist、`noiria-pack` で作成、webp/flac 変換あり）**:
  **16/16 バイト完全一致**。
- **`--autotest 90`**: dev(tsint)/dist(tsint)/dev(v8) の 3 構成すべて **EXIT=0**、
  ACK/tick ドレインのタイムアウト警告 0 件、ハング無し（63 枚のフレームを保存）。
- **dev（v8）**: 本ウェーブでは V8 のコード（`ops_*.rs`/`state.rs`/`run_script` の
  ポーリングループ本体）は一切変更していない（`EngineCtx`/`EngineState` へ
  `tick_sync` フィールドを追加しただけで、V8 側はこれを一度も読み書きしない）。
  検証のため 9 回実行（3 回 × 3 セット）して全ペア比較したところ、**`frame_03_hover.png`
  のみ、2 種類の異なるバイト列が確率的に出現し（9 回中 6 回が一方、3 回が他方）、
  それ以外の 15 枚は毎回完全一致**した。これは前ウェーブの「2026-07-13 dist フレーム
  差異の調査と headless 決定論化」5 節が「3 回実行の全ペア比較で 2 ペアは完全一致、
  残り 1 ペアは `frame_03_hover.png` のみ不一致」と報告した内容と定量的に整合する
  （±1 virtual tick の cross-thread レースが V8 側に残存したまま、という前ウェーブの
  分析通り）。**本ウェーブで新たに発生した回帰ではなく、V8 に元から残っていた
  未解消の残存ジッタ**であり、課題文の「V8 は触らない」指示と、上記 2 節で述べた
  ハング化リスクの理由により、本ウェーブでは意図的に対応しなかった。
- **V8 vs tsint 同一 run**: 上記の通り tsint は `frame_03_hover.png` について
  常に単一の値へ収束するようになった一方、V8 はその値と一致する回・一致しない回の
  どちらも観測される（V8 が確率的に出す 2 値は、いずれも tsint の収束値とは異なる
  バイト列だった）。そのため「V8 vs tsint 同一 run で全フレーム一致」は
  `frame_03_hover.png` に関しては**維持できていない**（他 15 枚は毎回一致）。
  これは tsint 側が新たに壊したのではなく、V8 側の残存ジッタが元々「たまたま
  tsint 側もジッタっていたので、比較結果が偶然一致することもあった」状態だったのが、
  tsint 側のジッタが解消されたことで V8 側の非決定性がこの 1 フレームに限って
  可視化されるようになったもの。V8 側の同種バリア導入（2 節で見送った理由により）は
  本ウェーブのスコープ外として申し送る。

### 5. テスト数・clippy

- **Windows**: `cargo test --workspace` = **379 件**（ベースラインと同数）、
  0 failed。`cargo clippy -p noiria-app --all-targets --all-features` およびデフォルト
  feature/`engine-v8` 単独/`engine-tsint,dist` 単独の各組み合わせで確認し、本ウェーブの
  変更由来の警告は 0 件（`engine-tsint,dist` 単独ビルドにのみ現れる `clock::is_real`
  dead_code 警告は、`is_real` が `engine-v8` 限定コード `ops_sys.rs::op_wait` からしか
  参照されないために生じる、本ウェーブ以前からの既存警告であり対象外）。
- **WSL**: `scripts/wsl-test.sh --skip-x11` を実行し `[1/4]`〜`[3/4]` は完走したが、
  `[3/4]` の ALSA スモークで `alsa_out::tests::open_default_with_nonexistent_device_name_fails_gracefully`
  が失敗するケースを確認した。これは本ウェーブの変更（`noiria-app` のみ、`noiria-core`
  は一切変更していない）とは無関係な、**既存の ALSA テストの並列実行順依存の
  flaky テスト**であることを切り分けた: 同テストを単体実行すると常に成功し、
  `open_null_device_and_play_silence_briefly` と並列/近接して実行されたときのみ
  失敗が再現した（2 回連続で再現を確認）。`cargo test --workspace -- --test-threads=1`
  （逐次実行、並列干渉を排除）では **389 件全通過・0 failed**（課題文のベースライン
  391 件との差分 2 件は、この ALSA テストの数え方や並行して進んでいる可能性のある
  他ウェーブとの環境差と見られ、原因追及は本ウェーブのスコープ外と判断した。実測値
  そのものは 0 failed）。`--autotest 24`（tsint/v8 いずれも）は WSL 上でも EXIT=0・
  16 フレーム保存を確認した。X11 段は前ウェーブ同様スコープ外（`--skip-x11`）。

### 6. 「既知ジッタ 2 枚除外」の注記について

W5/W8/W9 等の検証手順に記載されていた「`frame_03_hover.png`/`frame_09_hover.png`
などの既知ジッタは比較時に除外可」という注記（例: 3390 行目付近の W9 export E2E
記載）は、**tsint（dist/dev とも）に関しては本ウェーブの対応により不要になった**
（3 回完全一致を確認済み）。ただし 4 節の通り **V8 側は引き続き `frame_03_hover.png`
のみ確率的にジッタしうる**ため、V8 を含む比較（V8 同士の複数回比較、または
V8 vs tsint の比較）を行う場合は、この 1 フレームに関してのみ従来通り「既知ジッタとして
許容」する運用が必要である。tsint 単体（同一エンジン同士）の比較であれば、この除外は
もう必要ない。

### 7. 変更ファイル一覧

- `crates/noiria-app/src/bridge.rs`: `TickSync`（新規、`request`/
  `requested_generation`/`mark_complete`/`completed_generation`）追加。
- `crates/noiria-app/src/clock.rs`: `nanos` が `Relaxed` のままで安全な理由
  （`TickSync` の happens-before に依存）をドキュメントに追記（ロジック無変更）。
- `crates/noiria-app/src/headless.rs`: `HeadlessParams` に `tick_sync`/`tick_barrier`
  追加、`wait_for_tick_drain` 追加、フレームループ末尾（`clock.advance` 直後）に
  バリア呼び出しを追加。
- `crates/noiria-app/src/main.rs`: `resolved_engine_is_tsint`（新規、4 feature 組合せ
  対応）追加。ヘッドレス経路で `TickSync` を作成し `EngineCtx`/`HeadlessParams` へ
  配線、`tick_barrier` を決定。windowed 経路（win32/x11）の `EngineCtx` 構築 2 箇所に
  `tick_sync: bridge::TickSync::new()` を追加のみ（無害、無観測）。
- `crates/noiria-app/src/script/mod.rs`: `EngineCtx` に `tick_sync` 追加、
  `EngineState::new` 呼び出しへ配線、テスト用 `EngineCtx` 構築 3 箇所に追加。
- `crates/noiria-app/src/script/engine_state.rs`: `EngineState` に `tick_sync`
  フィールド追加（`engine-v8` 単独ビルドでは未読になるため `cfg_attr` で
  `allow(dead_code)`）、コンストラクタ引数追加、`make_engine_state` テストヘルパに追加。
- `crates/noiria-app/src/script/tsint_engine.rs`: `EngineCtx` 分解パターン・
  `EngineState::new` 呼び出しに `tick_sync` 追加。`drive()` ループに tick ドレイン
  バリアの発火ロジック（`requested_generation()` を `now` より先に読む、`!resolved_any`
  のときに `mark_complete`）を追加。**3 節の入力 ack バグ修正**（6.5 節の
  `matches!(pump, PumpState::Idle)` ゲートを削除）。テスト用 `EngineCtx` 構築 3 箇所に
  `tick_sync` 追加。`TickSync`/`requested_generation`/`mark_complete` は
  `engine-v8` 単独ビルドで未使用になるため `cfg_attr` で `allow(dead_code)`。
- `crates/noiria-app/src/script/tag_harness.rs`: `EngineCtx` 構築に
  `tick_sync: crate::bridge::TickSync::new()` を追加のみ（ハーネス自体のロジック無変更、
  指示通り）。
- `docs/implementation-notes.md`: 本節を追記。

### 8. 未解決・申し送り事項

- **V8 の残存ジッタ（`frame_03_hover.png`、確率的）は今回も未解消**。前ウェーブから
  持ち越しの既知の課題で、対応するには V8 のイベントループに仮想クロック駆動の
  定期ウェイク機構を新設する必要があり、`docs`「2026-07-13 dist フレーム差異の調査と
  headless 決定論化」5 節が見送った変更と同種のスコープ。次ウェーブで着手する場合は
  `tokio::time::pause`/`advance`（`test-util` feature）の cross-thread ハンドシェイク、
  または `TickSync` 相当のバリアを V8 の `poll_fn` ラッパへ「毎 tick 進行を要求される
  たびに強制的に 1 回再ポーリングさせる」形（例えば headless からの `request()` を
  `tokio::sync::Notify` で V8 タスクのウェイカーに直接繋ぐ）で安全に適用する設計が
  必要（ハング化を避けるため、ここは慎重な検討が要る）。
- **V8 vs tsint の `frame_03_hover.png` 不一致**: 上記 V8 側のジッタに起因。V8 側を
  解消しない限り、同一 run 比較で「毎回必ず一致」は保証できない。
- **WSL の ALSA flaky テスト**（`alsa_out::tests::open_default_with_nonexistent_device_name_fails_gracefully`）:
  本ウェーブのスコープ外（`noiria-core`、並列テスト実行順依存）だが、CI 等で
  `cargo test --workspace` を並列実行させている場合は時折失敗しうる状態のまま。
  `--test-threads=1` にするか、テスト側で ALSA のグローバル状態競合を避ける修正が
  望ましい（次ウェーブへ）。
- **WSL テスト数の基準値との差分（391 → 389）**: 原因未追及（0 failed 自体は問題ない）。

## 2026-07-13 V8 完全決定論化（op_wait の watch 化 + tick バリア）

前ウェーブ（「2026-07-13 tsint tick ドレインバリア」）が V8 側は意図的に対象外とした
最後の 1 点、`frame_03_hover.png` の確率的ジッタ（`--autotest 24` を複数回実行すると
毎回異なるバイト列が出る）を根治した。ベースライン: Windows cargo test 379 件 /
WSL 389 件全通過。

### 1. 根本原因

`ops_sys.rs::op_wait`（仮想モード）が `while clock.now() < deadline {
tokio::time::sleep(1ms).await }` と**実時間 1ms スリープでポーリング**していた。
仮想クロック（`clock.rs::Clock::Virtual`）は headless.rs のフレームループだけが
16ms 刻みで進めるが、`op_wait` 自身の起床は実時間のタイマーに依存していたため、
「仮想 deadline を超えてから実際に起きるまでの遅延」が run ごとの実時間スケジューリング
でブレていた。KAG プレイヤ（`runtime/kag/player.ts`）は文字送りのデバウンスや自動送り
タイマーで `sys.wait` を多用しており、この起床遅延の揺れが `frame_03_hover.png`
（move 入力から 300ms 後に撮る、hover ハイライトのタイミングに敏感なフレーム）として
可視化されていた。

### 2. 対応 1: `op_wait` を仮想クロック前進の通知で起こす（watch 化）

- **`clock.rs::Clock::Virtual`** に `tx: Arc<tokio::sync::watch::Sender<u64>>` を追加。
  `advance()` は `nanos` を進めた**直後**に必ず最新値を `send` する（watch は最新値
  だけを保持するので lost-wakeup が無い）。新設 `watch_rx()` で `tx.subscribe()`
  した `Receiver` を取得できる（`Clock::Real` はダミーチャンネルの receiver を返すが
  `op_wait` の実時間分岐からは読まれない）。
- **`engine_state.rs::EngineState`** に `clock_rx: watch::Receiver<u64>`
  （`EngineState::new` 内で `clock.watch_rx()` から導出、コンストラクタ引数は無し）と
  `pending_waits: PendingWaits`（後述、コンストラクタ引数）を追加。
- **`ops_sys.rs::op_wait`** の仮想分岐を書き換え:

  ```rust
  let deadline = clock.now() + dur;
  if clock.now() < deadline {
      pending_waits.inc();               // ガードで確実に -- する（キャンセル安全）
      while clock.now() < deadline {
          let _ = rx.changed().await;    // 仮想クロック前進の通知でのみ起きる
          tick_sync.react(tick_sync.requested_generation());  // 3 節参照
      }
  }
  ```

  判定は常に `clock.now()`（正のソースは `nanos` のまま）で行い、`watch` の値は
  起床トリガとしてのみ使う（happens-before を壊さない）。実時間 1ms スリープは
  完全に無くなった。`Clock::Real`（windowed/テスト）は無変更。

### 3. 対応 2: tick ドレインバリア（V8 版）— 設計変更を伴う 2 回の反復

**設計 A（当初案、失敗）**: 既存 tsint 版と同型（`bridge.rs::TickSync`）を使い、
headless.rs の `Clock::advance` 直後に V8 でも `request()`→`wait_for_tick_drain()`
を張り、完了報告は `script/mod.rs::run_script` の手動 `poll_fn` ラッパが
`poll.is_pending()` を検出した瞬間に `mark_complete()` する設計を最初に実装した。
**`--autotest` を実行したところ、ほぼ毎 tick が `ACK_TIMEOUT`（3秒）に達し、
24 秒のテストが実時間で 5 分以上かかっても終わらないことが判明した**（一時的に
`eprintln!` デバッグを仕込んで原因を特定、最終的に除去済み）。

原因: **deno_core は保留中の非同期 op（`op_wait`）の内部の個々の await
（`rx.changed()`）が進むたびに外側の `poll_event_loop` タスクを再ポーリングするわけ
ではなく、op 自体が完了（resolve）したときにしか外側を起こさない**ことが実測で
判明した。`op_wait` 自身の Rust future は（deno_core が pending op を直接駆動して
いるため）`rx.changed()` が解決するたびに確実に再ポーリングされ、`clock.advance`
に正しく反応して tick ごとに while ループを 1 段ずつ進めていた（デバッグログで
`n=1,2,3,...` と毎 tick カウンタが伸びるのを確認）。しかし `sys.wait(60)` のように
複数 tick にまたがる `op_wait` が in-flight な間、**外側の `run_script` の手動
`poll_fn` ラッパは一度も再ポーリングされていなかった**（`poll.is_pending()` の
デバッグ出力が、対応する `op_wait` が**完全に完了した直後**にしか現れないことを
確認した）。そのため `tick_sync.mark_complete()` を outer poll ラッパ側に置く設計は
「`sys.wait` が複数 tick 継続する間ずっと機能しない」構造的な欠陥だった。

**設計 B（採用）**: 完了報告を `run_script` の poll ラッパから **`op_wait` 自身の
while ループ内**（`rx.changed()` が解決した直後）に移した。`op_wait` の Rust
future は毎 tick 確実に再ポーリングされることが実測で確認済みなので、ここでなら
tick ごとに確実に `TickSync` へ反応を報告できる。この変更後、`--autotest 24` は
実時間 約31〜33秒で正常終了するようになった（3 節末の検証結果参照）。

**同時に複数の `op_wait` が in-flight な場合への対処**: 設計 B をそのまま適用すると、
「1 個目の `op_wait` の反応だけで tick 完了と誤認する」可能性がある（`games/sample`
は自動送りタイマーと文字送りタイマーが同時に `sys.wait` することが多く、実測でも
常時 2 個の `op_wait` が in-flight だった）。これに対応するため `TickSync` を拡張した:

- `request_for(expected_reactors: i64) -> u64`（headless 側、V8 専用）:
  `remaining` を `expected_reactors`（`Clock::advance` 直前に読んだ
  `PendingWaits::get()` のスナップショット）に設定してから要求世代を発行する。
- `react(gen: u64)`（スクリプト側、V8 の `op_wait` 専用）: `remaining` を 1 減らし、
  0 以下になった（＝期待していた全 `op_wait` が反応し終えた）呼び出しだけが実際に
  `mark_complete()` する。
- tsint 側は従来通り `request()`/`mark_complete()`（`remaining` を経由しない）を
  使うため無変更・無影響。

この「全反応者待ち」化がフレームの決定論に実際に効いたかを検証するため、
`request_for`/`react` 追加**前**と**後**それぞれで `--autotest 24` を 6 回実行し
PNG ハッシュを比較したところ、**両方とも同一のバイト列**（6/6 内部一致、
V8 単独の決定論は既に確立済みだった）で、`frame_03_hover.png` を含め 1 バイトも
変化しなかった。つまりこのリポジトリ・このシナリオでは「1 個目だけの反応」でも
実害は無かった（後述 4 節の V8/tsint 差異の原因でもない）が、設計としては
「全 in-flight `op_wait` の反応を待つ」方がより正しいため、より保守的な設計 B
（`request_for`/`react`）を最終版として採用した。

### 4. pending==0 スキップ（アイドル tick のハング回避）

`bridge.rs::PendingWaits`（新規、`Arc<AtomicI64>`）: `op_wait` の仮想分岐が待機に
入る直前に `inc()`、抜けた直後（正常終了・キャンセルいずれも、ガード構造体で保証）に
`dec()` する。headless.rs はフレームループ末尾、`Clock::advance` の**前**に
`pending_waits.get()` を読み、**0 なら V8 のバリアを丸ごとスキップ**して即
`Clock::advance` だけ行う。理由: `pending==0`（V8 が入力待ちのみでアイドル）の
tick に無条件でバリアを張ると、`op_wait` が 1 つも無いので誰も watch を起こさず
`mark_complete` が永遠に来ず、毎 tick `ACK_TIMEOUT`（3秒）に到達してしまう
（前ウェーブが tsint 版バリアの V8 適用を見送った理由と同種の罠。今回は
`pending_waits` で「起こしてくれる `op_wait` があるかどうか」を明示的に判定する
ことで回避した）。

**headless.rs の呼び出し順（tsint と V8 で逆）**:

- **tsint**: `Clock::advance()` → `request()` → `wait_for_tick_drain()`
  （既存、無変更）。tsint の `drive()` は busy-loop（毎回タイムアウト無しで
  再実行される）なのでこの順序で安全（前ウェーブの happens-before の議論通り）。
- **V8**: `request_for(pending_waits.get())` → `Clock::advance()` → `wait_for_tick_drain()`。
  V8 は wake 駆動（`poll_event_loop` は起こされたときしか再ポーリングされない）
  なので、tsint とは**逆順**が必要: `request_for` を `Clock::advance`（内部で
  watch へ `send` する）より**前**に呼ぶことで、「`request_for` の書き込み
  （同一スレッド内 program order）→ `advance` の `send()` → watch の同期による
  wake 配送 → `op_wait` 内の `requested_generation()` の読み取り」という単一
  スレッド内 program order + channel 同期の推移的合成により、`op_wait` が
  wake された時点で必ず対応する `request_for` の値を観測できる（`bridge.rs::TickSync`
  のドキュメントに happens-before の詳細を記載）。逆順にすると「advance の watch
  通知で起きた時点でまだ新しい要求世代が見えていない」レースになりうる。
  V8 と tsint は排他（tsint は `pending_waits` に一切触れないため常に 0 で
  V8 側の分岐に入らない）ので、同じ `tick_sync`（`TickSync`）インスタンスを
  共有していても衝突しない。

### 5. 決定論検証の結果（`games/sample`、Windows 実機、生ログ）

**V8 `--autotest 24` を 6 回**（`target/debug/noiria.exe games/sample --autotest 24
--autotest-out <dir> --engine v8`）: EXIT=0、タイムアウト警告 0 件、実時間
各 31〜32 秒。16 枚それぞれの `md5sum *.png | sort -k2` を 6 run 分比較したところ
**全 16 枚が 6 回とも完全一致**（`frame_03_hover.png` を含む）。集約ハッシュ
（`md5sum *.png | sort -k2 | md5sum`）は 6 run とも
`602c248f6cb69aa94772cb471223f53c` で同一。**前ウェーブが記録した V8 の確率的
ジッタ（9 回中 6 回/3 回の 2 値）は解消された**。

**tsint（dev）`--autotest 24` を 6 回**: EXIT=0、タイムアウト警告 0 件。集約ハッシュ
6 run とも `2c496316ea4cf6740aa352c1ed7f9cf9` で同一（前ウェーブからの回帰なし）。

**dist（tsint、`cargo build --profile dist --no-default-features --features
engine-tsint,dist -p noiria-app`）`--autotest 24` を 3 回**: EXIT=0、タイムアウト
警告 0 件。集約ハッシュ 3 run とも `2c496316ea4cf6740aa352c1ed7f9cf9`（dev tsint と
同一値、期待通り）で完全一致。

**dir 起動 vs pak 起動**（`noiria-pack --game games/sample --out sample.pak` で
作成した webp/flac 変換入り pak、同一 `noiria.exe` を exe 隣接 `game/` ディレクトリと
`game.pak` それぞれで起動）: `--autotest 24` の PNG 16 枚、`diff` で **完全一致**
（差分ゼロ）。

**V8 vs tsint 同一 run**: `frame_00.png`〜`frame_14_hover.png`/`frame_final.png`
の 16 枚中 **15 枚は完全一致**。`frame_03_hover.png` のみ V8 側
（`a9832dce20b6323c9c75c87c7b362d0d`）と tsint 側
（`f1992523099668af15cfdc027c653e7e`）でハッシュが異なる（サイズも 477826 vs
478664 バイトで、ファイルサイズ自体が異なるほど内容が違う＝アンチエイリアス誤差の
類ではない）。**この 1 フレームだけは「両方決定論化すれば一致するはず」という
期待が外れた**。原因を切り分けるため、3 節の「1 個目の `op_wait` だけの反応で
`mark_complete` する」設計（設計 B 導入前）と「全 in-flight `op_wait` の反応を待つ」
設計（`request_for`/`react`、採用版）の両方で V8 の出力を比較したが、**バイト
単位で完全に同一**だった（つまりバリアの精度をこれ以上厳しくしても変化しない）。
これは frame_03_hover の V8/tsint 差異が headless 側の同期バリアの精度の問題では
なく、**V8（Promise/マイクロタスク駆動）と tsint（`HostPromise` 駆動）という
構造が全く異なる 2 つのスクリプトエンジンが、同一の仮想 tick 境界に対して
「文字送り/自動送りタイマーが同時に複数本走っている」ような複雑な状況で、
JS/TS レベルの継続処理を厳密に同じ順序・同じ粒度で実行するとは限らない**という
エンジン間の構造的な差異に起因すると判断した（両エンジンとも「仮想 tick への
到達」自体は完全に決定的になったが、「その tick で JS 側が具体的に何段の継続まで
処理し終えているか」はエンジン実装依存になり得る）。tag_harness / ops 経由の
挙動（レイヤ操作・セーブ・音声コマンド等）はテストで全て一致確認済みであり、
影響は「300ms 後の hover ハイライトの見た目」という 1 フレームの視覚表現のみに
限定される。

**`--autotest 90`**（V8/tsint dev それぞれ 1 回）: 両方 **EXIT=0**、タイムアウト
警告 0 件、エラー/panic 0 件、ハング無し。V8: 実時間 115 秒・63 枚保存。tsint:
実時間 123 秒・63 枚保存（前ウェーブの tsint 実測値と同数）。

### 6. テスト数・clippy

- **Windows**: `cargo test --workspace` = **379 件**（ベースラインと同数、増減
  無し）、0 failed。`cargo clippy -p noiria-app --all-targets`（デフォルト
  feature）・`--no-default-features --features engine-v8`・
  `--no-default-features --features engine-tsint,dist`・`--all-features` の
  4 通りで確認し、本ウェーブの変更由来の新規警告は 0 件（`engine-tsint,dist`
  単独ビルドにのみ現れる `clock::is_real` dead_code 警告は、前ウェーブの記録
  通り本ウェーブ以前からの既存警告であり対象外。本ウェーブで新設した
  `PendingWaits::inc`/`dec`/`TickSync::react` にも同型の `#[cfg_attr(not(feature
  = "engine-v8"), allow(dead_code))]` を付けて tsint 単独ビルドでの新規警告を防いだ）。
- **WSL**: `wsl -e bash -lc "cd .../noiria && CARGO_TARGET_DIR=~/.noiria-target
  cargo test --workspace"` を実行し **389 件全通過・0 failed**（ベースラインと
  同数）。`scripts/wsl-test.sh --skip-x11` も実行したが、その `[3/4]` ALSA
  ベンダリング再実行ステップで `alsa_out::tests::
  open_default_with_nonexistent_device_name_fails_gracefully` が単発失敗した
  （前ウェーブの「2026-07-13 tsint tick ドレインバリア」8 節に記録済みの、
  `noiria-core` 側・並列テスト実行順依存の既知 flaky テストで、本ウェーブは
  `noiria-app` しか変更していないため無関係。上記の `cargo test --workspace`
  単体実行では発生しなかった）。`--autotest 24`（V8, WSL 上）も併せて EXIT=0・
  16 フレーム保存を確認した。X11 段は前ウェーブ同様スコープ外（`--skip-x11`）。

### 7. 「既知ジッタ frame_03_hover 除外」の注記について

前ウェーブ 6 節が整理した「`frame_03_hover.png`/`frame_09_hover.png` などの
既知ジッタは比較時に除外可」という注記（W5/W8/W9 等の検証手順に記載）のうち、
**「V8 側は引き続き `frame_03_hover.png` のみ確率的にジッタしうる」という部分は
本ウェーブで解消された**（5 節の通り V8 単独 6 回・全 16 フレーム完全一致を確認
済み）。**V8 単体・tsint 単体、いずれの同一エンジン内比較でも、もはや
「既知ジッタとして除外すべきフレーム」は無い。**

ただし、**V8 と tsint を跨いだ比較**（V8 vs tsint 同一 run 等）でだけは、
`frame_03_hover.png` 1 枚に関して両エンジンが同じ値に収束するとは限らない
（5 節の通り、これは非決定性ではなく決定的だが異なる値になるエンジン間差異）。
そのため「V8 vs tsint を跨いだ比較を行う場合に限り、`frame_03_hover.png` 1 枚を
除外する」運用は本ウェーブ後も必要である。W5/W8/W9 の記載を書き換える場合は、
「ジッタ（run ごとに不定）」ではなく「エンジン間の決定的差異（跨いだ比較でのみ
問題になる）」である旨に文言を改めるのが正確。

### 8. 変更ファイル一覧

- `crates/noiria-app/src/clock.rs`: `Clock::Virtual` に `tx: Arc<watch::Sender<u64>>`
  追加。`advance()` が `nanos` 更新直後に `tx.send()` するよう変更。`watch_rx()`
  新設（`Clock::Real` はダミーチャンネル）。
- `crates/noiria-app/src/bridge.rs`: `PendingWaits`（新規）追加。`TickSync` に
  `remaining: Arc<AtomicI64>` と `request_for`/`react`（V8 専用）を追加、
  ドキュメントを tsint/V8 両対応に更新。
- `crates/noiria-app/src/script/ops_sys.rs`: `op_wait` の仮想分岐を実時間 1ms
  ポーリングから watch 起床待ちへ全面書き換え。`pending_waits` の inc/dec
  （ガード構造体でキャンセル安全）、`tick_sync.react()` によるバリア完了報告を追加。
- `crates/noiria-app/src/script/engine_state.rs`: `EngineState` に `clock_rx`
  （`clock.watch_rx()` から導出、コンストラクタ引数無し）・`pending_waits`
  （コンストラクタ引数）フィールド追加。`make_engine_state` テストヘルパに
  `PendingWaits::new()` を追加。
- `crates/noiria-app/src/script/mod.rs`: `EngineCtx` に `pending_waits` 追加、
  `EngineState::new` 呼び出しへ配線。`run_script` の手動 `poll_fn` ラッパから
  当初追加した tick バリア完了報告ロジックを撤去（3 節の設計 A→B の経緯により
  `op_wait` 側へ移動したため。input ack ロジックのみ残る、無変更）。テスト用
  `EngineCtx` 構築 3 箇所に `pending_waits: PendingWaits::new()` を追加。
- `crates/noiria-app/src/script/tsint_engine.rs`: `EngineCtx` 分解パターン・
  `EngineState::new` 呼び出しに `pending_waits` を素通しで追加（tsint 自身は
  読み書きしない）。テスト用 `EngineCtx` 構築 3 箇所・`cfg(test)` の
  `use crate::bridge::{..}` に追加のみ。
- `crates/noiria-app/src/script/tag_harness.rs`: `EngineCtx` 構築に
  `pending_waits: crate::bridge::PendingWaits::new()` を追加のみ（ハーネス自体の
  ロジック無変更、指示通り）。
- `crates/noiria-app/src/headless.rs`: `HeadlessParams` に `pending_waits` 追加。
  フレームループ末尾を「`pending_waits.get() > 0` なら V8 用
  `request_for()`→`advance()`→`wait_for_tick_drain()`、そうでなければ
  `advance()` のみ（0 なら丸ごとスキップ）＋ tsint 用の既存 `tick_barrier`
  分岐（`advance()` 後に `request()`）」という構造に書き換え。
- `crates/noiria-app/src/main.rs`: ヘッドレス/windowed（win32/x11）各経路の
  `EngineCtx` 構築に `pending_waits: bridge::PendingWaits::new()`（windowed は
  無害・無観測）、ヘッドレス経路はさらに `HeadlessParams` へも同じインスタンスを
  共有。
- `docs/implementation-notes.md`: 本節を追記。

### 9. 未解決・申し送り事項

- **`frame_03_hover.png` の V8/tsint エンジン間差異**（7 節参照）: run 内・run 間の
  非決定性ではなく、決定的だが値がエンジンごとに異なる。両エンジンの JS/TS
  実行セマンティクス（マイクロタスク順序・継続処理の粒度）をこれ以上近づけるのは
  V8/tsint 双方の内部実装に踏み込む必要があり、「ops_sys.rs/engine_state.rs/...
  以外は触らない」という本ウェーブの担当領域を超えるため、次ウェーブへ申し送る。
  再現条件: `games/sample` を `--autotest 24` で V8/tsint それぞれ実行し
  `frame_03_hover.png` を比較。
- **設計 A（`run_script` の `poll_fn` ラッパで `mark_complete`）は機能しない**こと
  を実測で確認済み（3 節）。将来 V8 の非同期バリアに手を入れる場合は、deno_core の
  「op 完了時にしか外側タスクを起こさない」という性質を踏まえ、完了報告は
  op 自身の中で行うこと。

## 2026-07-26 B9: lossy WebP 実サンプル検証

- **判断**: ImageMagick で自前生成した 16×16・4 色の lossy WebP を
  `crates/noiria-core/tests/fixtures/lossy_4quad.webp` に固定し、RIFF 内の
  `VP8 ` チャンク、寸法、RGBA バッファ長、各色領域の中央画素、alpha=255 を
  `decode_lossy_vp8_fixture_to_rgba` で検証する。
- **理由**: エンジン内のエンコーダは VP8L ロスレス専用なので、round-trip テストでは
  lossy VP8 デコード経路を通せない。自己生成フィクスチャなら外部配布物の出典・可用性にも
  依存しない。
- **影響**: `image-webp` の lossy デコード経路が実バイト列で常時検証される。
  デコード API・pack/export の生成形式・実行時依存関係は変更しない。

## 2026-07-26 B18: export の部分成功モード

- **判断**: 既存の `run_export` と CLI 既定動作は strict のまま維持し、明示的な
  `ExportPolicy::SkipMissingTargets` / `--skip-missing-targets` を追加した。全ターゲットの
  バイナリ読込と `KEY_SLOT` パッチを pack・出力より先に行い、strict では一つでも失敗すれば
  新しい成果物を書き始めない。
- **理由**: CI と通常配布では「要求した全 OS 版が揃う」従来契約を壊さず、片方のビルドだけを
  手元で作りたい場合に限って部分成功を選べるようにするため。事前検証により、欠けた二つ目の
  ターゲットを後から検出して一つ目だけが新規出力される状態も避けられる。
- **判断**: 部分成功時にスキップするのは `ErrorKind::NotFound` だけとした。権限エラー、
  不正な `KEY_SLOT`、pack・書き込み失敗は伝播し、全ターゲット欠如もエラーにする。成功と
  スキップは `ExportReport::targets` / `skipped_targets` に分け、CLI は件数と対象パスを報告する。
- **理由**: 「無いバイナリを許容する」と「壊れた配布物を成功扱いする」を混同しないため。
- **影響**: `--skip-missing-targets` では存在する OS 版だけを生成できる。スキップ側の既存出力は
  削除しないため、CLI 警告にもその旨を明記した。Windows / WSL の
  `crates/noiria-tools/tests/export_cli.rs` で strict 事前失敗、片側成功、全欠如、不正
  `KEY_SLOT` の 4 ケースを固定した。

## 2026-07-26 B10: 並列テストの flaky 解消

- **判断**: `alsa_out` の二つのテストは `NOIRIA_ALSA_DEVICE` を変更せず、private な
  `AlsaOutput::open_device` へ `"null"` / 存在しない名前を直接渡す形にした。
- **理由**: 両テストが process-global な環境変数を並列に書き換え、存在しないデバイスのテストが
  一時的に `"null"` を読んで成功扱いになるのが WSL 側の根本原因だった。テスト対象を直接注入すれば
  mutex や環境変数の退避処理も不要になる。
- **判断**: `tag_harness::run_engine` 全体をテスト用 mutex で直列化した。poison 時も inner を
  回収し、後続テストを連鎖失敗させない。
- **理由**: ハーネスの `wait_for_quiescence` は op log の実時間静止区間からスクリプトの待機到達を
  推定している。複数の V8 isolate を同時に動かすとスケジューリング遅延を静止と誤認し、
  クリックハンドラ登録前に入力を送っていた。InputHub はそのクリックを保持しないため、以後の
  テストが timeout していた。製品実行経路ではなく、同一 process 内で多数の独立ゲームを同時駆動する
  テストハーネス固有の競合である。
- **影響**: ALSA の実デバイス選択と製品側 V8/tsint 実行は変更しない。タグ挙動テスト同士だけが
  直列になり、Windows の通常並列テスト反復と WSL の ALSA 反復で再発しないことを確認した。

## 2026-07-26 B13: `align=` / `linespacing=`

- **判断**: `TextStyle` に serde 既定値 `left` の `TextAlign` を追加し、折返し・明示改行で
  確定した各 visual line の advance 終端幅から left/center/right の offset を求める。
  offset は Pass 3 の座標丸め前に加算し、本文グリフから位置を求めるルビも同じ移動へ追従させる。
  箱幅を超える行の offset は 0 にクランプする。
- **理由**: 行確定前に中央・右揃えを行うと折返しや禁則処理を変えてしまう。既存
  `assign_lines` を不変にして、確定後の座標だけを移す方が left の互換性と行単位の整列を両立する。
- **判断**: `linespacing` はメッセージレイヤ別の signed px として保持し、実効行送りを
  `max(1px, fontSize × 1.6 + linespacing)` とする。値が 0 の場合だけは既存の `1.6` を直接使い、
  従来の浮動小数点結果を変えない。
- **判断**: current/default の行設定を各 `MessageLayer` に持たせ、`style` は current、
  `defstyle` は default、`resetstyle` は current を default へ戻す。`cm`/`er` では維持する。
  セーブにも両方を含め、フィールドが無い旧セーブは left/0 に補完して、表示セグメントを
  再描画する前に復元する。不正な `align` と非有限の `linespacing` は warn して無視する。
- **理由**: 複数メッセージレイヤで片方の会話配置が他方へ漏れず、`tempload` 後にも保存前と同じ
  レイアウトを再現するため。文字色・サイズ等の既存 `currentFontState` は従来どおり共有し、
  今回必要な行設定だけをレイヤローカルにした。
- **判断**: 行設定はセグメント単位ではなく現在のメッセージレイヤ全体の単一状態とする。
  `style` / `resetstyle` は native style の変更直後、既存 `segs` があれば
  専用 `text.reflow` で即時に全文を再配置する。ラスタと native 側 `glyph_counts` だけを更新し、
  進行中の `TextReveal` は保持するため、次の追記境界を直しつつ文字送りを完了させない。
  `defstyle` と空レイヤでは再配置しない。
- **判断**: `[locate]` を含む visual line は明示した絶対 x を保持するため alignment offset を
  0 とする。`indent` は折返し行頭の指定なので通常の整列対象に残す。タグの `linespacing` は
  state 更新前に値自身と算出後の line height が単精度の有限範囲か検証し、direct
  `api.text.style` でも V8 / tsint が非有限 line height と未知 align を同じく拒否する。
- **影響**: 2026-07-11 W1 で「受理のみ」とした `align` / `linespacing` の保留判断を解消した。
  core・V8/tsint 共通タグを合わせて B13 回帰 11 件を追加し、既存ルビテストも center 配置で
  強化した。
  Windows / WSL workspace、sibling `tsint`、clippy を通過。変更前後の 24 秒
  `--autotest` では意図した `frame_07_hover.png` とロード後の同場面だけが変化し、
  center・8px 行間・ルビ位置を目視確認した。両エンジン各 2 回の出力はエンジン内で完全一致した。

## 2026-07-26 B20: VP9 10/12-bit の SDR 表示

- **判断**: `Vp9Ivf::next_frame` は 8-bit を既存 `yuv_to_rgba_bt601` へそのまま通し、
  10/12-bit の U16 plane だけを新しい HBD 経路へ分岐する。HBD 値は
  `2^(bit_depth-8)` で f32 除算して 8-bit 等価の YUV 値にし、既存と同じ
  BT.601 limited 係数、最近傍 chroma、round/clamp で RGBA8 にする。bit shift による
  小数 code value の切捨ては行わない。
- **理由**: `vp9dec::Frame` は bit depth・subsampling・U16 plane を公開する一方、
  decoder 内部の color space/range を公開 Frame へ保持せず、IVF にも transfer、
  primaries、mastering metadata が無い。PQ/HLG や露出を推測した「トーンマップ」は
  色を誤るため、入力 code value だけで決定できる SDR fallback に限定した。
- **判断**: bit depth、U8/U16 の組合せ、subsampling、各 plane のサンプル数を変換前に検証し、
  不整合は panic や range 外アクセスではなく文脈付きエラーにする。
- **影響**: profile 2/3 の 10/12-bit SDR 映像を RGBA8 レイヤへ表示できる。8-bit の変換演算は
  変更しない。真の HDR 色忠実性は未対応で、将来は色メタデータを保持する container /
  decoder API が先に必要になる。手組み frame の 8 ケースに加え、`lavfi testsrc2` から
  自己生成した 16×16・2 frame の 10-bit 4:2:0 / 12-bit 4:4:4 IVF をリポジトリへ固定した。
  fixture テストは外部 corpus や隣接 checkout に依存せず、全表示 frame 数、RGBA hash、
  alpha、rewind を Windows / WSL で常時検証する。再生成コマンドと SHA-256 は
  `crates/noiria-core/tests/fixtures/README.md` に記録した。

## 2026-07-26 B14: BGM 再生位置の一時保存・復元

- **判断**: `Mixer` の各スロットへ出力済みフレーム数を保持し、現在反復内の位置を整数 ms で
  `AudioShared` へ同期する。位置指定再生はデコード済み PCM source を出力フレーム単位で読み飛ばし、
  終端を越えた指定は先頭へ戻す。pause 中は位置を進めず、loop の巻戻し・stop・自然終端では
  0ms へ戻す。
- **理由**: source ごとの入力 sample rate や channel 数ではなく、ミキサへ渡る共通の出力形式で
  数えれば対応済みの WAV / OGG へ形式別の位置分岐を増やさず同じ契約にできる。専用 seek API を
  各 decoder に追加する規模も避けられる。
- **判断**: runtime API に `audio.play(..., { startMs, paused })` と `audio.position(ch)` を追加し、
  `SaveData.bgm.positionMs?` へ完全スナップショット作成時の位置を保存する。復元時は
  有限・非負の number だけを受理し、フィールドが無い旧セーブや不正値は 0ms とする。
  pause 済み BGM は位置指定再生と pause を 1 コマンド・同一ミキサロック内で適用し、音が
  1 バッファだけ先行する隙間を作らない。
- **判断**: play/stop 要求ごとに単調な ID を付け、要求した位置を `AudioShared` へ先に公開する。
  音声スレッドの古い応答や周期同期は最新要求の位置を上書きできない。停止フェード中の pause と、
  pause 済みスロットへの停止フェードは出力が進まず完了不能になるため、その場で停止して位置を
  0ms へ戻す。通常 pause と fade-pause は位置を保持する。
- **判断**: PCM source に有限フレーム数の保守的上限を任意で持たせ、復元位置が上限を厳密に
  越える場合だけサンプル走査を省略して先頭へ戻す。buffer の完全フレーム数を
  channel converter が透過し、resampler は f64 位相誤差を含む 1 フレーム余裕付き上限を返す。
  長さ不明の source は従来どおり走査し、上限と等しい位置は次回 render で終端を確定する。
- **理由**: 壊れた、または極端に大きい `positionMs` を含むセーブでも起動を長時間ブロックせず、
  正常範囲とちょうど終端の再生意味論は変えないため。ms から frame への変換は `u128` で行い、
  `u64::MAX` へ飽和させる。
- **影響**: `[tempsave]` / `[tempload]` だけでなく、同じ完全スナップショットを使う
  `[record]` / `[goback]` / `[startanchor]` / `[gotostart]` でも BGM 位置を復元する。
  精度は出力フレームの整数 ms 化と音声スレッドの共有状態同期周期（最大およそ 10ms）まで。
  音声デバイスを開けない環境でも、play/stop 入口で公開した要求位置は保持される。通常の
  `[playbgm]` は 0ms を要求するため従来のヘッドレス取得値は 0ms のまま、位置付き復元直後は
  指定値を観測できる。

## 2026-07-26 B12: `move spline=true`

- **根拠**: KAG3 の `move` タグは `spline=true` で `path=` を B スプライン補間し、
  指定点を最低 2 点要求する。KAG3 の公開 `DefaultMover.tjs::SplineMover` が使う
  自然三次スプラインの係数計算と区間式を実装基準にした。
  [KAG3 タグリファレンス](https://krkrz.github.io/krkr2doc/kag3doc/contents/Tags.html) /
  [DefaultMover.tjs](https://github.com/krkrz/krkr2/blob/master/kirikiri2/branches/2.32stable/kag3/template/system/DefaultMover.tjs)
- **判断**: `[現在位置 + 解決済み path 点]` の x/y について係数を f64 で一度だけ求め、
  各 `Tween` / `Waypoint` にその区間の係数端点を保持する。描画時は既存の eased progress を
  区間式へ渡す。opacity は従来の eased linear、`accel` の意味も変えない。KAG タグの
  距離比例 leg time は native が呼出時の実効位置から配分し、汎用 API の明示 leg duration は
  維持する。`spline=false` / 省略時は係数を持たず従来の線形経路を通る。
- **判断**: `spline=true` は path 指定点を最低 2 点要求し、タグでは warn + no-op、
  native API では error とする。単区間の新しい `move` は開始時の実効位置を採取してから
  古い path queue を破棄する。`layer_moving` は現在区間が完了済みでも queue が残る間は true とする。
- **影響**: `op_layer_move_path` と runtime wrapper に spline bool を追加し、既定 false で
  既存呼出しを維持した。自然三次式、opacity、endpoint、queue の引継ぎ・skip・割込み、
  不足点の拒否、V8/tsint 配線を B12 回帰 6 件で固定した。依存 crate は増やしていない。

## 2026-07-26 B20: VP9 可変解像度の再生

- **判断**: `VideoSource::width` / `height` は `next_frame` が最後に返した表示フレームの
  寸法とし、初回デコード前だけコンテナの初期値を返す。`Vp9Ivf` は RGBA 変換に成功した
  表示フレームの寸法へ更新し、再生と `[preparevideo]` は共通処理で `next_frame` の直後に
  寸法を読み取って Pixmap を作る。
- **理由**: VP9 はストリーム途中の sequence header で解像度を変更できるため、IVF ヘッダ値を
  再生開始時に固定すると、RGBA バッファ長と Pixmap 寸法が食い違って panic するため。
- **判断**: 動画 worker の `playing` はループ先頭で作る Drop ガードが終了時に解除する。
- **理由**: 通常終了だけで明示解除すると、デコーダや Pixmap 構築などが unwind した場合に
  `[waitEnd]` が終了済み worker を再生中と観測し続けるため。
- **影響**: 可変解像度 VP9 は各表示フレームの寸法で fore/back レイヤを差し替える。
  固定寸法の AVI/MJPEG と Media Foundation の契約・デコード処理は変更しない。
  外部依存は追加していない。

## 2026-07-26 B12/B13 統合ハードニング

- **判断**: 行揃え・行間変更時の全文再配置は `text.print(..., cps=0)` ではなく、蓄積済み
  segment と現在 style からラスタと glyph count だけを作り直す専用 native reflow op にする。
  既存レイヤの `TextReveal` は開始時刻・速度・追記境界を含めて変更しない。
- **理由**: print による置換は reveal の開始時刻を `None` にして文字送りを完了させるため、
  表示途中の `[style]` が残り文字を即時表示していた。再配置対象と時間状態を分離すれば、
  次回追記用の glyph count を更新しながら進行中の reveal を維持できる。
- **判断**: 汎用 `layer.movePath` は各 leg の明示 duration を従来どおり尊重し、KAG
  `[move path= time=...]` だけは総時間を渡す距離配分 API を使う。距離と開始点は native が
  呼出し時の tween 実効 x/y から計算する。全 leg の距離が 0 の場合は均等配分する。旧 TS
  `allocateMoveLegTimes` は production-unused になったため削除し、その距離比・座標継承・
  距離 0 のテストは native の実経路へ移した。
- **理由**: TS 側 `LayerState` は進行中 tween の目標終点を先に保持するため、割込み時の実表示位置を
  表せない。native scene の sampled position を配分起点にしないと、第一区間の duration が
  目標終点基準で短縮・延長される。
- **判断**: 完全スナップショット適用は、保存済みプロパティを書き戻す前に対象レイヤの fore/back
  両方の current tween と queue をキャンセルする。`[position]` による box resize は非カレントの
  message レイヤにも専用 reflow を行う。カレントレイヤは reveal 進行中だけ専用 reflow、
  完了後は従来の hover 色付き repaint を使い、nameplate はどちらでも更新する。
- **影響**: `[tempload]` 後に古い spline path が再開したり `[wm]` を保持したりせず、非表示・
  非カレント message レイヤも次に切り替えた時点で新しい幅に合う。新規外部 crate は追加しない。

## 2026-07-26 B2: V8/tsint エンジン間決定論一致

- **判断**: V8 の各仮想 `op_wait` は `EngineState` が起動時から保持する watch receiver の
  clone ではなく、登録時に `Clock::watch_rx()` で新規購読する。期限前の tick は従来どおり
  native future 内で反応し、期限へ到達した反応だけを外側の event-loop drain 後まで保留する。
- **理由**: Tokio の `watch::Receiver::clone` は元 receiver の既読 version を引き継ぐ。
  元 receiver が起動後に更新されていなかったため、途中で作られた wait は過去の
  `Clock::advance()` を即時消費し、headless の当該世代スナップショットに含まれないまま
  `TickSync::react` していた。4624ms の `wait(450)` 継続内で作られた `wait(120)` が
  barrier を早期完了し、headless が同じ V8 poll 中に 4640ms へ進んだ結果、後続
  `wait(200)` と crossfade 開始が tsint より 1 tick 遅れていた。
- **判断**: 期限到達反応は deno_core の current waker が追加 iteration を予約している間は
  flush しない。self-wake が止まるか event loop が終了した時点で Promise 継続まで処理済みと
  みなす。V8 の世代・残反応数・保留数は同じ mutex 状態で管理し、タイムアウト後の旧世代反応を
  新世代へ適用しない。
- **理由**: 固定 poll 回数では op completion 後の即時 work の段数に依存する。
  deno_core が公開する event-loop API では、full poll 中の self-wake が追加処理の有無を示す
  最小の静止条件になる。期限前の反応まで外側へ移すと、複数 tick にまたがる wait は外側を
  起こさず barrier が timeout するため、期限前/期限到達の二段階は維持する。
- **影響**: `frame_03_hover.png` の transition 開始は両エンジンとも 4832ms に揃った。
  6 秒・24 秒の V8/tsint 各 2 回比較は各エンジン内で完全決定論となり、24 秒の全 16 PNG も
  エンジン間でバイト一致した。期限到達後の JS 継続中には barrier が開かないことと、
  stale 世代が次世代を完了できないことを回帰テストで固定した。2026-07-13 節で既知差異として
  残した申し送りは解消した。

## 2026-07-27 B4: 動画フレーム seek とイベント制御

- **判断**: `VideoSource::seek_frame` は 0-based の表示フレーム番号を受け取り、成功後の
  `next_frame()` が指定フレームを返す契約にする。総表示フレーム数と等しい指定は EOF 位置、
  それを越える指定はエラーとする。MJPEG はコンテナ索引へ直接移動し、VP9 と Media Foundation
  は先頭へ戻して表示フレーム単位で decode-forward する。
- **理由**: VP9 の IVF チャンクには非表示フレームがあり、デコーダは参照フレーム等の状態を
  前フレームから引き継ぐ。Media Foundation の時刻 seek も直前のキーフレームへ寄るため、
  コンテナ位置や時刻だけの移動では表示フレーム番号を厳密に保証できない。
- **判断**: 動画は既存の `current` 1 本 + `prepared` 1 本を維持し、イベントは slot 0 の
  `period` / `loop` / `prepare` / `segLoop` だけを世代別 latch として保持する。新しい再生を
  始める際はシーンロックと同じ排他区間で世代を更新し、旧 worker は描画直前にも世代を照合する。
- **理由**: stop フラグだけでは、旧 worker がデコード中に新しい再生へ切り替わった場合、
  デコード済み画像を新しい再生開始後に書き戻す競合を閉じられない。イベント状態も再生世代ごとに
  分離すれば、旧 worker の遅延発火を新しい `[wp]` が拾わない。
- **判断**: `videosegloop` は `[start,end)` とし、`end - 1` の表示後に `start` へ seek して
  `segLoop` を発火する。`videoevent` は指定表示フレームの描画時に 1 回だけ `period` を発火して
  登録を解除する。pause 中は seek・フレーム・イベントを消費しない。
- **判断**: current 動画の seek・区間設定・period 登録・停止は `scene → control` の順で
  排他し、worker もデコード済みフレームの書込直前と EOF 確定前に同じ順序で再確認する。
  pending seek または区間終端が見つかった場合はデコード済み画像と旧 boundary event を破棄する。
- **理由**: デコード中に設定 API が戻った後、旧フレームを描画したり、period の登録を
  取りこぼしたり、EOF と誤判定して再生を終了する競合を防ぐため。stop と再生置換は
  `playing=false` も同期 publish し、blocking decode 中でも `[wp]` が即時復帰できるようにした。
- **影響**: pause 中だけはデコード済み画像を保持し、resume 後に同じフレームを commit する。
  seek・rewind・区間変更は API が戻った時点より後に旧フレームを表示しない。test-only の
  timeout 付き commit hook と block-once source で各競合順序を決定的に固定した。
- **判断**: `playAt` の frame op ABI は `i32` sentinel ではなく JS number (`f64`) を受け、
  `-1` の未指定または厳密な `0..=u32::MAX` の整数だけを Rust 共有検証で受理する。
- **理由**: KAG 側は `u32::MAX` まで許可しており、`i32` 変換では 2^31 以上が V8 と tsint で
  異なる値になり得たため。
- **影響**: V8/tsint の frame 境界が一致し、負数・小数・非有限値・u32 超過はエラーになる。
- **影響**: `[video frame=N]` は `[video position=ms]` より優先し、時刻は動画 fps から
  `floor(ms * fps / 1000)` の表示フレームへ変換する。`slot != 0`、負数、不正区間、
  未知の `[wp for=...]` は warn + no-op とする。公式動画タグは共通して slot 0 のみ対応する。
  同期 `preparevideo` の完了も latch へ残すため、直後の `[wp for=prepare]` は取りこぼさない。
  外部 crate は追加しない。
- **検証**: Windows の targeted test は core seek 3/3、動画制御 24/24、KAG 互換 4/4、
  sample label 3/3。workspace 全体は Windows 457 passed / 0 failed / 1 ignored、
  WSL2 466 passed / 0 failed / 1 ignored、兄弟 `tsint` は 363 passed / 0 failed。
  WSL2 が 9 件多い差は X11/ALSA と Win32/MF/WASAPI/font registry の `cfg` 差で説明できる。
  ignored 1 件は既存の legacy fixture 手動生成テストである。
- **検証**: Windows は default、V8-only、tsint + dist profile、all-features、WSL2 は
  all-features の build/check を通過した。`cargo fmt --all -- --check` と
  `cargo clippy --workspace --all-targets --all-features` も成功し、B4 変更ファイルに
  clippy warning はない（`noiria-tools` の既存 warning 6 箇所は残る）。
- **検証**: 24 秒 autotest を V8/tsint 各 2 回行い、各エンジン内の決定論と
  16/16 PNG のエンジン間バイト一致を確認した。集約 MD5 は両方
  `27cd84c59e3b5ca4f5bf04c7b9a844a9` で、個別 16 枚も B2 基準と一致した。
  さらに保存領域を engine 別の新規 `XDG_DATA_HOME` へ隔離した 150 秒走行で B4 の
  完了後まで進め、各 105 PNG を全件バイト一致
  （集約 MD5 `1cfd6c970a38cd9ebe841e31cf61a13b`）させた。
  両ログは error / panic / tick timeout / 未対応タグが各 0、意図した対象外 tag が各 10 件。
- **検証**: `target/b4-autotest-final.YV0MVh/{v8,tsint}/frame_85_hover.png` は frame 0 preview
  （MD5 `06825c602d62f6304c6f91ab3f13da3d`）、`frame_89_hover.png` は seek 後の
  frame 1 preview（MD5 `b2e596ecb5e3e6b81b119c4d5a9c5b2e`）で、両エンジン一致かつ目視でも
  フレーム移動を確認した。headless capture は仮想時計、動画 worker は実時間で進むため、
  sample は各 prepared frame を 3 秒可視にした後、再生中だけ動画レイヤを隠して event/wait を
  検証する。実動画画素と decode 中競合は codec/native の回帰テストで別に固定した。
  Win32 可視ウィンドウと実音声デバイスの手動確認は B16/B17 として未実施。

## 2026-11-19 B1: ランタイムの出荷品質化（保存・既読・入力・欠落コンテンツ・フォント）

- **sf の保存は dirty 判定**: `KagPlayer.saveSys()` は `JSON.stringify(vars.sf)` が最後に書いたものと
  変わったときだけ `sys` スロットへ書く。呼び出し点はラベル通過・`[l]`/`[p]`/`[s]` に入る時・別シナリオへの
  jump・ファイル終端・`[close]`/`sys.quit()`・`WindowClose`。`[eval]` ごとには書かない（ディスク連打回避）。
  別シナリオ遷移の保存は「遷移先の `readText` の後」に置く: tsint リポジトリの op ログ部分列テスト
  （`kag_full.rs`: `read_text part2.ks` → `save_write sys`）がその順序に依存するため。
- **WindowClose は入力ポンプで先取り**: `api.ts` の `InputHub.pump` が `key:"WindowClose"` を通常ハンドラへ
  流さず `input.onWindowClose()` の登録ハンドラ（KagPlayer: `saveSys()` + `sys.quit()`）へ直接渡す。
  どの待ち状態でも同期的に処理される。ハンドラ未登録（KagPlayer を使わないゲーム）は `sys.quit()` にフォールバック。
  `InputEvent.kind` に `"keyup"` を追加（Ctrl スキップ用）。
- **savedata の原子書き込みと `.bak`**: `write_slot` は `<slot>.json.tmp` へ書いて `sync_all` → `rename`。
  `read_slot` は JSON として壊れた/非 UTF-8 の中身を `<slot>.json.bak` へ退避して `None` を返し error ログを出す
  （全スロット共通の挙動。壊れたセーブスロットも同様に退避される）。`list_slots` は読めないスロットを飛ばす。
  `.tmp`/`.bak` は拡張子が `json` でないので一覧に混ざらない。
- **既読化は区間の読了時**: `onLabel` で入った時ではなく、`finishSection()`（次のラベル到達・別シナリオへの
  jump・ファイル終端・`[return]`・`[close]`）で `sf.readLabels[file*label] = true`。`[call]` は呼び出し元の区間を
  コールスタックに退避し（`section`）、`[return]` で再開する。ロード（`applySnapshot`）は読み途中の区間を
  既読にしない。`[return]` を経ない `call` の途中離脱・終了時の途中区間は未読のまま（意図）。
  `SettingsConfig.skipUnread`（既定 false、旧 sys.json は補完）が真なら未読区間でもスキップを止めない（UI は B2）。
  ランドマイン: 「既読スキップ中に未読ラベルへ入ったら止まる」判定は `onLabel` 冒頭の
  `finishSection()` の**後**に行う（同じラベルへ自己 jump してもループが既読扱いになる）。
- **Ctrl はキーダウンで開始・キーアップで解除**（S はトグルのまま）。押下前から S でスキップ中だった場合は
  離しても解除しない（`skipBeforeCtrl`）。押した瞬間に今のクリック待ちを抜ける（`fireInterrupt()`）。
  オートリピートが来る旧ホスト（A 完了前）でも `ctrlHeld` で冪等。
- **フック呼び出しのメッセージ保全（`[rclick call=]`）**: メッセージを消さず、コールスタックのエントリに
  全メッセージレイヤの表示内容（`segs`/`currentText`/`links`）を退避し、`popReturn` が内容の変わったレイヤだけ
  再描画して復元する。呼び出し時にクリック待ち（`inAdvanceWait`）かつマクロ外なら戻り先を待ちのタグ
  （`pc.index - 1`）にして、戻ったとき同じ `[l]`/`[p]`/`[s]` で待ち直す（さもないと戻った時点で待ちを
  飛ばして次の文へ進む）。`[rclick jump=]`/`[click]`/`[wheel]` は常にジャンプ。KAG3 本家はフックのジャンプでも
  メッセージを消さないが、このエンジンの `[jump]` タグが消す（古いリンク/選択肢を残さない）ため揃えて
  `clearMessage()` を維持した。
- **一時停止 = `uiLock` 区間を時間に数えない**: `lockUi()/unlockUi()` で表示区間を計測し、
  `sleepActive(ms, canInterrupt)` が `[wait]`・オート待ち・`[timeout]` の残り時間を数え直す（表示が出なかった
  通常経路は従来どおり `api.sys.wait` 1 回で ops 列・タイミング不変）。実行ループとマクロ本体は 1 コマンドごとに
  `uiLock` を見て止まる（スキップ連打が裏で進まない）。BGM・ボイスは止めない。文字送りのリベールは Rust 側の
  時間進行なので表示中も進む（既知の簡略化）。`ask`/`input` モーダルも同じ扱い。
- **欠落コンテンツ**: 存在しないシナリオ（`readText` 例外）・ラベルは、error ログ + メッセージ窓へ
  「シナリオ 'x' が見つかりません」/「ラベル '*x' が見つかりません (file)」を表示 → クリック待ち →
  `waitForJump()` で停止（メニューからロードすれば再開）。失敗した `[call]` の戻り先は捨てる。
  開始シナリオの欠落も同じ経路。ラベル欠落の「落下実行」はなくなった。
- **メッセージフォント**: `KagOptions.message.fontFamily`（未指定なら `text.style` に `fontFamily` を送らない
  ＝既存ゲームの ops 列とバイト出力は不変）、`[font face=]`/`[deffont face=]`/`[style face=]` は
  `TextSegment.font_family`（V8 は serde 自動、tsint は `segments_from_value` に追加）。`face=user`/空は既定へ戻す。
  解決順（`text.rs resolve_base`）: 要求 family の**同梱**フォント → **同梱の先頭（ファイル名昇順）** →
  システムの要求 family → OS フォールバックチェーン。同梱フォントが 1 つでもあれば Windows/Linux で同じ結果になる
  （従来は Windows だけ既定の "Yu Gothic UI" が先に当たり同梱が無視されていた）。システムフォントを明示指定した
  `face="Meiryo"` も同梱があれば同梱が勝つ（同梱ゲームは見た目をゲームが握る方針。システム側を使いたい場合は
  同梱を置かない）。行メトリクス（ascent・行送り）は常にボックス基準フォントで、セグメントの face は
  グリフ選択だけを変える（追記キャッシュの不変条件を守るため。ルビは本文セグメントのフォントに従う）。
  `.otf`/'OTTO' の同梱は読み込まず error ログを出す。`text_style` の op ログ行に `font_family=` を追加。
- **ボイス**: `playVoice()/stopVoice()` が `voiceActive` を持ち、次へ送る操作（クリック/Enter/スキップ送り/
  オートのクリック）でだけ `audio.stop("voice")` を発行（鳴らしていなければ何も出さない）。オートは
  ボイス終了待ち→`autoWaitMs`、待ち中のクリックでオート解除+送り（従来はボイス待ちがクリック不可だった）。
- **テスト変更**: `tag_behavior_tests.rs::stub_tags_warn_once_and_emit_no_ops` の許容 ops に `save_write sys` を
  追加（`[close]` が読了区間を既読にして保存するため）。ハーネス（`tag_harness.rs`）は `Step`/`Setup` による
  キー・右クリック・待ち・途中確認・追加シナリオ・事前 sys.json に対応（新テストは `script/b1_tests.rs`）。

## 2026-11-19 A: ホストの出荷品質化 + スクリプト駆動 autotest

Win32/X11 ウィンドウ（`win32.rs` / `x11/window.rs`）と `main.rs`/`headless.rs` が中心。OS 非依存の純ロジックは
`viewport.rs`（レターボックス・座標逆変換・初期サイズ・CPU 拡縮）、`input.rs`（閉じる猶予・押下キー追跡・X11
オートリピート判定）、`fatal.rs`（致命エラー）、`autotest_script.rs`（DSL）に分離して単体テストしている。

- **黒帯上のマウス座標はクランプしない。** 理由: 端へクランプすると、画面下端のメッセージ窓内の選択肢などを
  黒帯のクリックで誤爆する。影響: ゲーム範囲外（負値や `width` 超え）の座標がスクリプトへ渡りうる
  （通常のヒット判定は矩形なので何も起きない）。
- **キーのオートリピートは全キーで送らない**（Win32: lParam bit30、X11: 後述）。影響: PageUp/ArrowDown の
  押しっぱなしでバックログ等が連続送りされなくなった。長押し系の機能は `keyup` + 押下状態をスクリプト側で
  持つ前提（B1 の Ctrl スキップ）。
- **keyup は Control/Shift/Enter/Space のみ、かつ対応する keydown を送っていた場合だけ送る**（Alt+Enter の
  Enter など、ホストが消費したキーは keyup も出さない）。フォーカス喪失（`WM_KILLFOCUS` / `FocusOut`）で
  押しっぱなし分の keyup をまとめて出す。Shift は新たに keydown `"Shift"` も送る。F11 はスクリプトへ渡さない。
- **X11 のオートリピートは「同一時刻の KeyRelease + KeyPress 対」を検出して両方捨てる**（XKB の detectable
  auto-repeat は要求しない）。KeyRelease は 3ms だけ保留して対かを確定する。`event.time` の一致が根拠で、
  サーバ/WM がこの形で配送しない環境では効かない（実機 X11 は未検証、Xvfb への合成イベントのみ）。
- **閉じる操作**（`WM_CLOSE` / `WM_DELETE_WINDOW`）は `key:"WindowClose"` をスクリプトへ送り、2 秒（`CLOSE_GRACE`）
  以内に `UserEvent::Quit` が来なければ強制終了。入力受信口が閉じている（スクリプトスレッド死亡）なら即終了。
  猶予中の再要求は猶予を延長しない。
- **HALFTONE 拡縮**: Win32 は等倍以外で `SetStretchBltMode(HALFTONE)`、等倍は `COLORONCOLOR`（既存と同じ）。
  黒帯は `PatBlt(BLACKNESS)`、`WM_ERASEBKGND` は無効化。**拡大時のフレーム落ち（1080p/4K 相当）は不可視ウィンドウでは
  測れず未確認** — 重ければ CPU 側で先に拡縮する（X11 の `blit_letterboxed` を流用可）。
- **Win32 フルスクリーンは `WS_OVERLAPPEDWINDOW` を外して `WS_POPUP` + モニタ矩形**（復帰は保存したスタイル +
  `WINDOWPLACEMENT`）。不可視ウィンドウ（テスト）では `SetWindowPlacement` が窓を表示してしまうため
  `SetWindowPos(rcNormalPosition)` で復帰する。`SetWindowPos` 等は WM_SIZE をリエントラントに送るので、
  `WindowShared` の RefCell は借用を短く保つこと（`fullscreen` は Cell、保存状態は借用して即解放）。
- **初期サイズ**: Win32 はカーソルのあるモニタの DPI/作業領域（`AdjustWindowRectExForDpi` で枠を引く）。X11 は
  画面の mm 寸法から DPI を推定（108 未満は 96=等倍扱いにして Xvfb の偽値・丸め誤差で縮小しない）、拡大時は
  画面の 92%、等倍時は画面いっぱいまで。最小サイズは論理解像度の 1/4（下限 160x90）。
- **致命エラー**（`fatal.rs`）: ダイアログは dist ビルドの Windows のみ。ヘッドレス（`--autotest*`）・`cargo test`・
  `NOIRIA_NO_DIALOG` では常に抑止（stderr + ログ）。スクリプトスレッドの失敗（`Err`/panic、`catch_unwind`）は
  グローバルへ記録 → `Quit` → メインスレッドがウィンドウを閉じた後に報告（スレッドから出すと閉じかけの窓と競合）。
  固定振付の `--autotest` は従来どおり終了コード 0 のまま（挙動不変）、`--autotest-script` はスクリプトエラーで非 0。
  panic フックは dist（`panic=abort`）でも abort 前に動くのでダイアログが出る。
- **セーブ先の隔離**: `--save-dir <dir>` / 環境変数 `NOIRIA_SAVE_DIR`（`noiria_core::savedata::save_dir` が差し替え）。
  既定（未指定）は従来どおり。ログ（dist の `noiria.log`）もセーブ先に出るので一緒に隔離される。
  `sf.*`（到達済みエンド）・設定（文字速度）が残るので、autotest は**毎回空のディレクトリ**を使うこと。
- **`--autotest-script`**: `click` は直前に同座標の `move` を送る（実マウスと同じ順序でホバー状態を作る）。
  毎 tick の合成はせず `shot` の時だけ合成する（合成は `&Scene` 読み取りのみ）。`--autotest-fast` は 16ms スリープを
  省くだけで、ロックステップ（イベント ack + tick ドレインバリア）はそのまま。デモ 3 ルート × V8/tsint × 3 回の
  全 PNG が一致（決定論は保たれる）。仮想時間の上限（既定 3600 秒、`--autotest <秒>` 併用で変更）に達したら
  警告して正常終了する。
- **デモ用スクリプトの落とし穴**: ① 文字送り中（35cps）のクリックは「文字送りの完了」に消費される。場面転換
  （最長 1800ms）の直後は文字送りが転換後に始まるため、転換後の最初のクリックは 4200ms 空けないと 1 クリック
  ずれる（通常は 2200ms）。② `showcg` の画像が読めないとマクロが途中で中断し `[waitclick]` が飛ぶ → CG 状態の
  クリック数がずれる（アセット完備が前提。欠けたアセットでは進行できるが CG の撮影位置は保証されない）。
  ③ クリック数は `.ks` の `[np]` 数に依存する（内訳はスクリプト冒頭のコメント）。
- **既存テストの変更**: win32 `invisible_window_input_roundtrip_and_clean_quit` は初期クライアントサイズが DPI 依存に
  なったため、先にクライアントを論理解像度へ揃える（`set_client_size`）。X11
  `client_message_wm_delete_window_quits_run_windowed` は即終了ではなくなったため
  `..._forwards_window_close_then_forces_quit` に改名し、`WindowClose` 受信 + 猶予後の強制終了を確認する。

## 2026-11-19 デモゲーム用アセットの手続き生成（`gen-demo-assets`）

- **判断**: `cargo run -p noiria-tools --bin gen-demo-assets -- games/demo/assets [--style a|b|c]` が `games/demo/assets/` と `games/demo/icon.png` を全生成する（外部素材ゼロ・新規クレートなし）。描画は自作ラスタライザ `src/demo/gfx.rs`（符号付き面積累積の AA マスク + プリマルチプライド f32 キャンバス + 設計座標アフィン `Xf`）。`--preview <dir>` はアセットを書かず、立ち絵シート・背景への合成・CG・画風比較シート `style_candidates.png` だけを出す（目視確認用、`src/demo/preview.rs`）。
- **判断（画風）**: 「リアル寄りに作り込む」と不気味の谷に落ちる（過去 2 回差し戻し、デモの初版も同様）ため、**素朴さ優先のフラット図形キャラ**に統一した。立ち絵・CG・背景すべて、フラット塗り・限定パレット・影は 1 段のベタ・グラデは空の縦 1 方向のみ。禁止: 虹彩グラデ/複数ハイライト・まつ毛・髪の筋・顔の陰影（鼻の影のみ可）・肌や服のグラデ・顔幅 1/5 超の目。粒子的な描き込み（fbm ノイズ・光の粒・桜の球状粒・机や建物の細密な繰り返し）もやめ、大きな形で整理。
- **画風 3 種**（`Style`、既定 A）: A フラット・ミニマル（約 3 頭身、縦長楕円の目 + 白点 1、弧の口）/ B 絵本・切り紙風（目は点、縁に低周波のゆらぎ、2px のベタ影を各パーツの下に敷く、影色・髪の濃淡なし）/ C A の描画規則で頭を 0.72 倍にした約 5 頭身。B/C は `--style` で切替可能だが、コミット済みアセットは A。
- **判断（立ち絵の構造）**: 頭（`Lay::head`: 縮尺・傾き）と体（`Lay::body`: y 区間線形 + x 倍率）を別の座標系に載せ、頭を傾けて体は個別ポーズ（ひなた＝片手を上げる、しおり＝萌え袖で手を組む）にして左右対称を崩す。体・髪（前髪含む）は `draw_base` で 1 回描き、4 表情は「顔 − 前髪」マスクでクリップして重ねる（alpha・首より下の画素は 4 枚でピクセル一致、テストで固定）。**落とし穴**: 表情パーツは必ず `Parts.clip` でクリップすること。
- **制限/注意**: 立ち絵は 500x900 を 1:1 で置くので頭が約 280px と大きく見える（A）。しおりの紺黒髪は夜背景（hill_night）でコントラストが低い。立ち絵は教室の西日などシーン光を受けない（中性光）。
- **判断（CG）**: 顔のアップをやめ、立ち絵描画を 0.26 倍にした全身（足元まで描く `Lay` の最終キー）を夜景に小さく立たせる引きの構図（ひなた: 夜の丘で木の横、空を指す / しおり: 夜の屋上で望遠鏡の隣）。人物は画面高さの 1/3 以下・面積 3% 以下（テスト `event_cgs_keep_the_figure_small`、`FigureMeta` を返す `cg_*_with_meta`）。夜の色調は乗算 (0.62,0.68,0.96)。`cg_meteor` の 3 人は新スタイルの単純図形で描き直した。
- **落とし穴**: `(PI*t).sin().powf(p)` は t=1 で −ε → NaN になり、変幅ストロークが画面いっぱいの矩形に化ける（`Path::stroke` 側で太さを正規化済み。自前の式では `.max(0.0)`）。
- **判断/落とし穴**: 背景は隣接ポリゴンの継ぎ目で α が 1 未満になる画素が出うるので `bg::opaque` で α=255 へ揃える。空グラデの縞（バンディング）対策で、夜・夕焼け系の背景のみ `to_image(true)`（±0.5LSB ディザ）、街・教室・部室は `false`（ファイルを小さく保つ）。
- **判断**: PNG は `image` の `CompressionType::Best + FilterType::Adaptive` で保存。`icon.png` は sample と同じく自作 PNG エンコーダ（テストは別実装の `image` でデコードして確認）。icon（グラデ + 光彩の旧画風）と `rule/*.png` は今回変更していない（アイコンは新画風と少し毛色が違う。差し替えるなら `bg/icon.rs`）。
- **回帰ゲート**: `tests/demo_assets.rs`（寸法・不透明・立ち絵の輪郭一致/ハロ無し・**立ち絵の主要色（全不透明画素の 0.1% 以上を占める色）が 20 色以下**・CG の人物の小ささ・全背景/CG/立ち絵が生成器出力と一致・BGM/SE）、`tests/demo_audio_quality.rs`。AA 境界色は 1 色あたりの画素が少ないので主要色に数えない。
- **音声**: BGM 5 曲（title/daily/calm/tension/night）と SE 3 本（chime/sparkle/heartbeat）は `synth/demo.rs` + `synth/demo/inst.rs`（加算合成ピアノ + ストリングス）で作る独自系統。sample の 2op FM + saw パッドは使わず、`daily/calm/tension` も sample の流用ではない（ユーザー指摘「機械的/不協和/安っぽい」への作り直し）。設計と数値ゲートは `docs/bgm-design-notes.md` §7、守るテストは `tests/demo_audio_quality.rs`。**判断**: 既存の「トランジェント 0 件」ゲートはピアノの打鍵と両立しないので、`songs_transients_are_soft`（+14dB/10ms の急峻な立ち上がり 0 件、+8dB は 1 秒あたり 0.35 件まで）へ置き換えた（弱体化。撥弦/通知音の検出という目的は +14dB 0 件で維持）。**注意**: `gen-demo-assets` は音声の daily/calm/tension をまだ sample の関数で書くので、`synth::demo::render_daily/calm/tension` へ差し替えるまで実行しないこと（音声だけなら `examples/gen_demo_audio.rs`）。デモの wav は 6〜7MB/本（title/daily/calm/tension/night）。

- **不可視ウィンドウ統合テストの落とし穴**: ウィンドウのジオメトリを変えると、カーソル下に来た窓について OS が
  `WM_MOUSEMOVE` を合成して送ってくることがある（不可視でも発生）。テストは click だけを待ち（`recv_click`）、
  「何も来ない」検証では move を無視する。フルスクリーン切替直後は `WM_SIZE` の到着順に依存しないよう
  `GetClientRect` で `client_size` を即時同期している。


## 2026-11-19 B2: プレイヤー向けシステム UI 層

- **UI は新モジュールへ分離（`runtime/kag/ui.ts` / `menu.ts`）**: 部品（`UiBox` = 角丸矩形つきテキストレイヤ、`UiImage`、
  `UiEvents`、確認ダイアログ、トースト、常設ボタン）と、セーブ/ロード/設定メニュー本体。値の import は
  `player.ts → menu.ts → ui.ts → api.ts` の一方向（循環 import は tsint で実行時 TypeError）。メニューはプレイヤーを
  `MenuHost` インターフェース越しに使う（`KagPlayer` 自身がその実装。メニュー用の公開メソッド
  `setSetting`/`resetSettings`/`goTitle`/`quit`/`toast`/`uiSound` は B2 で追加）。旧メニュー（1 枚のテキストを行ヒット
  テストする方式）は削除した。`nextMenuMode`（見出しクリック循環）は既存テストが参照するため export だけ残している
  （メニュー内のタブ循環は Tab キーで別実装）。メニューのレイヤは開くたびに作って閉じるときに全部解放する
  （隠しレイヤを残さない）。
- **tsint の `String.prototype.indexOf` は needle が haystack より長いとパニックする**（`..\tsint\src\builtins.rs` の
  `indexOf`、`chars[i..i+needle.len()]` が範囲外）。tsint は読み取り専用なので直さず、新コードは文字列には
  `startsWith` を使う（配列の `indexOf` は無関係）。tsint 側の修正をユーザーへ申し送り。
- **サムネイルは新 op 3 点 + 日付 op**: `sys.thumbCapture()`（今の画面を 240x135 へ面積平均で縮小して保持、ファイルへは
  書かない）、`sys.thumbSave(slot)`（保持中の画像を `<slot>.png` へ書く。未取得なら今の画面）、
  `layer.loadSaveThumb(id, slot)`（セーブ領域の PNG をレイヤへ。**`GameDir` とは別経路**で、スロット名は英数字/`_`/`-` のみ
  =パストラバーサル不可。240x135 より大きい旧版の画面サイズ PNG は読み込み時に縮小）、`sys.dateString()`。
  `save()` は「メニュー経由なら開く直前に撮ったもの、それ以外は今の画面」を使う（`thumbPending`）ので、メニュー上の
  セーブにメニューが写り込まない。旧 `screenshot` op は `[screenshot slot=]` タグ用に残し、`save()` は使わなくなった。
  `read_slot`/`write_slot` が共有する `check_slot_name` は空のスロット名も拒否するようになった。
- **保存日時は Rust 側で整形**（`noiria-core/src/localtime.rs`、OS API 直叩き: Windows `GetLocalTime`、Unix `localtime_r`）:
  tsint の `Date` は getter が無く `toLocaleString` の書式が V8 と違うため。ヘッドレス（仮想クロック）は
  `2026/01/01 12:00` 固定（そうしないとメニューが写るフレームが実行ごと・エンジンごとに変わる）。
- **セーブ JSON の追加フィールド**（旧セーブには無く、読み込み側が補完）: `savedAt`（一覧に表示。無ければ旧 `date`）、
  `excerpt`（直近の本文の抜粋。選択肢の画面では選択肢以外の直前の段落）、`thumb`（サムネイル有無。旧セーブは false =
  「NO IMAGE」表示でファイルを試さない）、`backlog`（**ラベル通過時点**の履歴の直近 30 件。セーブはラベルから再生し直すので、
  保存時点の履歴を入れると再生ぶんが二重になる）。ロード時に履歴を復元し、旧セーブは空で補完する。`tempload`/`goback` は
  履歴に触れない（従来どおり）。ロードは `sysbar`/`autosave` の無効化と `[history output=false]` も有効側へ戻す
  （タイトル画面で止めたものを持ち越さない）。
- **スロット構成**: 1 ページ 6 枠 × 5 ページ（`save1..save30`）+ 先頭の「A / Q」ページ（`auto`/`quick`）。クイックは F5 の枠、
  オートは選択肢（リンクのある `[s]` の入口）で `save("auto")`。オート枠には手動で保存できない（トースト）。
  `[autosave enabled=false]`（Noiria 拡張）で止められる。**タイトル画面のリンク付き `[s]` でもオートセーブが走る**ため、
  デモは `*menu` で止めて `*start_game` で戻す。
- **確認ダイアログ（`confirmDialog`）を `[ask]` と共有**: 上書き/ロード/削除/タイトルへ戻る/終了/設定リセットが使う。
  ボタン上のクリックならそのボタン、それ以外の左クリックと Esc は「いいえ」で閉じる（`[ask]` の既存方針。行位置を知らない
  自動入力でもハングしない）。見た目は「▶ はい」の行ヒットテストからボタンになった（`[ask]` の見た目変更）。
  キーボードは ←→↑↓/Tab で移動・Enter/Space で決定、既定フォーカスは「いいえ」。
- **常設システムボタン（`KagOptions.systemButtons`、既定 off）**: message0 の窓の**直上・右寄せ**（下端 = 窓 top − 4px、
  高さ 28px）に置き、窓内へは入らない。位置・表示・背景は fore/back 両ページへ書く（トランジションの
  `swap_after_transition` が fore を back の複製へ置き換えるため。テキストは fore のみで、ラベルは変わらないので
  `[backlay]` の複製で足りる）。バーの帯の上のクリックは本文送り（`waitAdvanceEvent`/`[waitclick]` の述語）にならない。
  AUTO/SKIP の強調は `modeIndicatorLoop`（150ms）が同期し、バー経由の切替は即時反映する。バーで AUTO/SKIP を
  切り替えたときだけクリック待ちをやり直す（`modeWake`）— キー操作の挙動は変えない（既存のサンプル autotest の
  フレームが変わるため）。メッセージ窓が隠れる操作（右クリック・`[hidemessage]`・`[layopt visible=false]`）と
  `[sysbar visible=false]` でバーも隠れる。
- **メッセージ窓のスキン**: `frame` は**別レイヤ**（窓の 1 つ下の z、窓と同じ位置）に画像を読み、窓自身の矩形は透明にする
  （1 枚で窓の背景を兼ねる。9-slice ではない）。窓と一体で隠れ、「窓の濃さ」（`settings.msgOpacity`）は画像レイヤの
  不透明度へ掛かる（窓色のアルファではなく）。`marginR`/`marginB` は未指定なら左/上と同じ（`boxW = width − marginX −
  (marginR ?? marginX)`）なので従来の ops 列は不変。`applyMessageLayerConfig` の op 順は変えていない（frame/バー
  の ops は末尾に足すだけで、未使用なら何も出ない）。名前欄は `namePlate.color/textColor` を指定でき、濃さ設定にも従う。
  窓の濃さが 100%（既定）なら窓色をそのまま使うため ops は不変。
- **バックログ**: 履歴は `{ text, name, voice, rule? }`。`name` は段落の最初の文字を出した時点の `[name]`、`voice` は
  その段落で鳴らした `[playvoice]`。表示は名前行 + 本文で、ボイス付きは `♪`、その行のクリックで再生（閉じない。余白・ボイス無しの
  行のクリック/右クリック/Esc/H で閉じる）。↑↓/PageUp/PageDown/ホイールでスクロールし、見出しに「末尾の件番号 / 総数」と
  スクロールバーを出す。画面に収まる件数は行数の見積り + `text.rects` の実測（末尾が切り捨てられていたら最古を 1 件外して
  描き直す）で決める。オーバーレイは従来どおり最初に確保されるレイヤ（既存テストが id を見る）。上限 200 は不変。
- **選択肢の操作性**: ↑↓ でフォーカス（端はループ。未選択なら ↓ で先頭・↑ で末尾）、Enter で決定（フォーカス中は Enter が
  本文送りにならない）。`sound.hover` は選択が動いたとき（マウスのホバー含む）だけ鳴る。`sound.click` 未指定は従来の
  `click.wav`。メニュー内の操作音は `sound` を指定したときだけ鳴る（従来メニューは無音）。設定の SE 音量プレビューも
  `playUiSound("click")`。
- **F5/F9 のトースト**: 画面上部中央（メニュー/履歴の表示中はカードとフッタの間）に約 1.2 秒。自前タイマー
  （`api.sys.wait`）で消えるので `[wait]` 等の進行を妨げない。**サンプル/testbed の autotest ではトースト（frame_05,
  frame_06_hover = F5）とメニュー（frame_12_hover = Esc、testbed は frame_final も）のフレームだけが変わる**
  （ほかのフレームは変更前とバイト一致、V8/tsint は全フレーム一致）。
- **既知の制限**: トランジション進行中に開いたメニューは、完了時の `swap_after_transition`（fore を back の複製へ置換）で
  消える（旧メニューから変わらない。メニューを開く前に `[wt]` を待つ運用）。オート/クイックの枠は削除は可能。
  UI のレイアウトは 1280x720 の設計座標（画面サイズが違うと中央寄せのみ）。UI 文字列は日本語固定。
  `[copybookmark]` は JSON だけをコピーする（サムネイル PNG はコピーしない。コピー先は元のサムネイルのまま/無ければ NO IMAGE）。
  オートセーブはラベルから再生し直す通常のセーブと同じ形式（選択肢の直前のラベルへ戻り、文を読み直して選択肢に着く）。
- **テスト**: `script/b2_tests.rs`（24 件、V8/tsint の両エンジン）。`tag_harness.rs` の `Setup` に `slots`（事前に書く旧セーブ）と
  `prelude`（main.ts の前に挿す TS。純関数を `api.sys.log` 経由で両エンジンから検証）を追加。既存テストの変更・削除なし。

## 2026-11-19 B3: モダン UI（市販 ADV 風のプレイヤー向け UI）

- **オプトイン `KagOptions.ui: "modern"`、既定 classic は無変更**（sample/testbed の `--autotest` 16 枚 x 2 ゲーム x V8/tsint が変更前後でバイト一致）。
  新 UI は `runtime/kag/{mui,mhud,mtitle,mmenu}.ts`（player → m* → mui → ui/api の一方向）。mui = 部品キット（`MBox` = パネル + テキストの 1 レイヤ、
  `MHoverBox`、`MImage`、`MGroup`）、mhud = 話者名/システムボタン列/クリック待ち/選択肢/トースト/確認/モードバッジ、mtitle = タイトル + ギャラリー、mmenu = セーブ/ロード/設定 + バックログ。
- **新しい描画 op（3 点セット + 型 2 箇所）**: `layer.panel`（`noiria-core/src/uiraster.rs`。SDF による AA 角丸 + 縦/横グラデ + 枠線 + ソフトシャドウ。`ramp` で
  グラデを先頭 N px に押し込める = 暗幕の上端が溶ける帯。影は `pad` ぶん外へはみ出すのでレイヤ位置を pad だけ引く）、`layer.loadImageScaled`（面積平均）、
  `loadSaveThumb(…, w, h)` / `thumbCapture(w, h)`（0 = 従来の 240x135。modern は 384x216 で撮り、カードで 336x189 へ縮める）、`layer.setCrop` / `layer.cropTo`
  （画像の「見える幅」= 左端から何画素ぶん合成するか。アンダーラインが左から伸びる/スライダーの塗り）。
  **判断**: crop を `LayerProps.cropW` / `MoveArgs.toCropW` にせず別 op + `Layer.crop_w/crop_tween` にした。理由: 既存テストが `LayerProps`/`MoveArgs` の Debug 文字列・リテラルを直接使うため
  （フィールドを足すと既存テストを書き換えることになる）。`blit_over` は `blit_over_clipped(…, None)` へ委譲（挙動不変）。
- **アニメはネイティブ tween のみ**（仮想クロックで決定論）。ホバー = 重ねたホバー面の不透明度 tween、出現/消滅 = フェード + 数 px スライド、ページ切替 = 退場 → 差し替え → 入場。
  **例外と落とし穴**: クリック待ち記号の上下は「絶対時刻を 32ms に量子化して毎回 put」にした。V8 の `op_trans_wait`（tokio notify）と tsint（tick ループ）で `[wt]` の完了が 1 tick ずれ、
  開始時刻から数える方式（movePath の往復）だと位相がずれて、待ち中のフレームのグリフ 15x15 px だけエンジン間で変わった。副作用: 待ち中は ops が約 31/s 出続けるので、
  tag_harness の `Click`（op_log の静止待ち）が成立しない → harness に `Step::ClickNow` を追加（既存テストは無変更）。modern のテストは `Sleep` + `ClickNow`/`KeyNow` を使う。
  同じ理由で、`[wt]`/トランジション直後から時間で動く演出を撮るスクリプトは 1 tick ぶんずれうる（静止後に撮れば一致）。
- **選択肢ボタン**: `waitForJump`（`[s]`）の先頭でリンク群をボタンに（`choices.style`、modern 既定 buttons）。リンク内の文字は透明色（color/edgeColor = [0,0,0,0]）+ cps 0 で印字し、
  レイアウトと履歴にだけ残す（`[link]` の前の地の文は窓に残る）。ラベルは先頭の空白・全角空白・▶ を除去。本文の文字送りが終わるまで待ち、その間のクリックは文字送りの完了。
  選択演出（約 0.45 秒）の後に `clearMessage` + `requestJump`、`exp=` は演出だけして画面を残す。入力は global handler → `ChoiceScreen.handle`（`uiLock` 中は無効なので
  メニューを開いている間は無反応。右クリック非表示にも連動）。`tryClickLink`/`focusLink` は buttons モードでは無効。
- **タイトル/ギャラリー**: `KagOptions.titleScreen` + `[titlescreen]`（classic では warn して `[s]`）、`KagOptions.gallery` + `sys.gallery()`。`lockUi` を入れ子可能にした（`uiDepth`。
  タイトルが lock したままロード/設定/ギャラリーを開く）。タイトルのメッセージ窓は `title.ks` が `[layopt layer=message0 visible=false]` で隠す。`title.ks` から旧「おまけ」ラベル群を削除（`ch1〜3.ks` は無変更）。
  全画面メニューを開く間はタイトルの項目・ロゴをフェードで引っ込める（下で文字が透けるため）。
- **バックログ（modern）**: 全履歴を 1 枚のテキストレイヤに 2 カラムで組み（`locate` + `indent`）、レイヤの y を tween してスクロール。上下は不透明な帯で隠すため背景は不透明（ゲーム画面は透けない）。
  区切りは細い `─` の文字行。ボイス行（♪）のクリック再生は維持。
- **tsint の落とし穴（runtime に書くとき）**: `Math.cos/sin/sqrt` が無い、`Function.prototype.call` が無い（`x.appear()` と直接呼ぶ）、本体が単文の `for (let i…) s += "x"` が
  `TypeError: これは関数ではありません` になる（while で書く）、コンストラクタのパラメータプロパティは使わない（未確認）。V8 でだけ動いて tsint で落ちるので両エンジンで撮ること。
- **スクリプト編集の落とし穴**: Windows の heredoc 経由の Python でバックスラッシュ入りの文字列置換が黙って空振りする。スクリプトはファイルに書いて実行する。
- **既存テストの変更なし**（`tag_harness.rs` に `Step::ClickNow` を足しただけ）。追加: `uiraster` 8 + crop 4（core）、`b3_tests.rs` 11（両エンジン）。
- **制限・弱点**: 1280x720 設計座標（他サイズは中央寄せのみ）。UI 文字は日本語固定。ギャラリーは 6 件/ページ想定でページ送りの UI を持たない（7 件目以降は未表示）。
  バックログは全履歴（最大 200 件）を毎回組むので開く瞬間に少し重い可能性（実機未確認）。実機のフォント/DPI での見た目、`panel` の影のにじみ、文字の大小混在の baseline は官能確認が要る。
  タイトル/メニューの `sound.hover` 以外の効果音は未追加。

## 2026-11-19 モダン UI メニューのラグ解消（開いた直後の途中描画・カード構築コスト）

- **原因（dist・tsint・6 件埋まりページを計測して確定）**: ① 開くたびに全パネルを SDF で塗り直していた（パネル 50 枚で約 59ms。全画面の暗幕だけで約 10ms）、② サムネイル PNG を毎回復号 + 縮小（6 枚で約 25ms）、③ 構築の途中で `appearAll` を呼ぶ順だったため、枠 → 文字 → サムネイルが 1 枚ずつ遅れて画面に出ていた（合成スレッドは構築中もフレームを描くので、その途中状態がそのまま見える）。構築はスクリプトスレッド 1 本で 750 ops / 80 レイヤ、合計約 150ms（この環境の中央値）、フェード 180ms で全部揃うまで約 350ms。
- **対策**: ① `layer.panel` の結果を仕様の数値列で鍵にして `Arc<Pixmap>` を共有する LRU（64MB）、角丸/枠/影の無いパネル（暗幕・線）の SDF を通らない高速経路（一般経路と画素単位で一致、テストで固定）。② サムネイルは鍵に更新時刻 + 長さを含む LRU（16MB）。`thumb_save` / `save_delete` ではそのスロットを明示的に捨てる（同一タイムスタンプ内の上書きでも古い絵を出さない）。③ `sys.prefetchSaveThumbs(w,h)`（新 op）で、起動直後に別スレッドで復号しておく（初回のメニューを開く待ちをタイトル画面の待ち時間へ）。ギャラリーの CG 縮小も同様（`sys.prefetchImagesScaled`、`layer.loadImageScaled` もキャッシュ経由）。④ 中身は不透明度 0 のまま全部作り、最後に 1 回で一括フェードイン（`MImage` も不透明度 0 で作る。以前は visible を立てた瞬間に 255 で出ていた）。暗幕・見出し・ナビだけが先に出る。スタッガーは廃止（ギャラリーの 70ms ずつも）。⑤ フェードは開く 90ms / タブ切替 60ms / ページ送り 30 + 45ms / 閉じる 80ms / ホバー 80ms（`mui.ts` の `D_OPEN` 等）。選択肢ボタンのスタッガーは 70→30ms。
- **実測（dist・tsint・m1 スクリプト 5 回の中央値、スクリプトスレッドの実時間。before → after）**: 初回オープン 148ms → 31ms（750 → 665 ops）、2 回目 98 → 17ms、ページ送り（→1）41 → 9ms、タブ切替（LOAD / CONFIG / SAVE）45 / 15 / 81 → 5 / 11 / 14ms、バックログ（31 件）39 → 31ms、ギャラリー 41 → 14ms、確認ダイアログ 7ms。押下から全表示完了 = 構築 + フェード（90ms）≒ 120ms。メニューを開く直前の `thumbCapture`（画面合成 + 384x216 縮小）は 8.7ms で、これが押下→暗幕までの下限。
- **判断: スナップショット/ゲートの op は足さない**。内容を不透明度 0 で作ってから tween を開始すれば、描画スレッドが途中で合成しても見える差は出ない（全レイヤが 0 から始まる）。動画の commit gate のような排他は不要だった。
- **ホバーは「浮かせる」をやめた**: 選択カードを -5px 動かすと 1 枚だけ位置がずれて見え、内側の枠もずれた。枠のアクセント色 + 薄い色味 + 濃い影（over 面の panel の影）だけにした。
- **バックログ（modern）**: 最新 40 件だけを組んで開き、上へ 400px まで近づくと 60 件ずつ足して組み直す（Home は全件）。足した高さだけ scroll をずらして見えている位置は動かさない。200 件でも開く時間は 40 件ぶん。足すときの組み直しは全件ぶんの時間がかかる（200 件で約 200ms の 1 回きりの引っかかり）。
- **抜粋の括弧**: `makeExcerpt` は切った本文で閉じられなくなった括弧を、入れ子も含めて内側から閉じる（max は本文の文字数。… と閉じ括弧が足される）。旧版が書いた「切れて閉じていない抜粋」は `parseSlotMeta`（`closeExcerpt`）が読み込み時に閉じるので classic メニューも直る。モダンのカードは文字数ではなく幅（324px、`estimateWidth`）で収める（`fitExcerpt`）。長い章名は `fitWidth` で … にする（以前は欠けるだけだった）。
- **本文カラム 800 → 880px（`COL_W`）**: シナリオの最長行は 40 字（ch3 の「…見えてないんじゃないかって」）で、22px 基準だと 800px では 37 字で折り返し、最後の 1〜2 字 + 」が次の行へ落ちていた。禁則処理は変えていない。`b3_tests` の `B3C` の期待値（COL_W）を 800 → 880 に更新。
- **暗幕を 242/250 → 250/253**: 空きスロットのページ等でゲーム画面の文字がうっすら透けて見えた。
- **既知の制限**: ページ送り中・タブ切替直後の入力は `busy`/再構築で 90ms ほど無視される。ロードは従来どおりラベル先頭から再生し直すので、セリフの途中ページでセーブしてもロード直後はそのラベルの先頭の文が出る（括弧の欠けではなく設計）。バックログを文字送りの途中で開くと、直前の段落が出た分（`[r]` の前半だけ）の抜粋になりうる。タイトルの入場演出（約 1.3 秒のスタッガー）は意図した演出なので変えていない。
- **計測の手順（再現用、コードには残していない）**: tsint の `sync_native` を包んで ops の件数・時間を集計し、メニューの構築前後に `api.sys.log("@prof …")` で区間を切った一時パッチ。途中状態の連写は、op を 40 個実行するごとに仮想クロックを実時間ぶん進めて `compose_frame` する一時トレーサ（`target/lag/tsint_engine.instrumented.rs`）。ヘッドレスはロックステップなので、`--autotest-script` の `shot` では構築の途中は撮れない。

## 2026-11-19 オートモードで閉じ括弧が一瞬しか出ない（描画ループの取りこぼし）

- **原因は描画ループ（win32 / x11）**: 文字送りが終わった周期は `is_animating` が false で `revision` も変わらないため再合成されず、
  最後に描いたフレームは「あと 1 グリフ足りない」状態（35cps・16ms 周期なら最後の 1 字 = 台詞の 」）で止まっていた。クリック待ちなら
  待ち記号（▼）の表示 op が revision を進めて描き直すので気付かれず、オートは待ち記号を出さないので、次のページ送りの再描画
  まで 」 が出なかった。tween・quake の終了直後も同じ構造（最後の 1 フレームが取りこぼされる）。`bridge::needs_redraw` に切り出し、
  動きが終わった直後の 1 周期も描くようにした（`bridge::redraw_tests` が旧条件で最終描画が途中のまま止まることを再現して固定）。
  ヘッドレスは `shot` 時に合成するので影響しない（実機の窓でのみ出る）。
- **スクリプト側の起点は元から正しかった**: 計測（ヘッドレス tsint、`[l]` 待ちの文字送り完了検知 → 次ページ）で、変更前は基本待ち
  （800/1800/3200）+ 約 64ms（ポーリング粒度）ちょうどで、文字数によらず一定だった。V8 のヘッドレスは `op_audio_wait_end` の実時間 60ms 猶予が
  仮想時間では約 1 秒に化けて +1 秒前後になるが、これは仮想クロックの人工物（実機の V8 では 60ms）。
- **オート待ちを文字数に比例**: `autoWaitFor(base, chars) = base + min(chars × base/40, base × 2)`（短 20 / 普通 45 / 長 80 ms/字、加算は基本待ちの
  2 倍まで）。文字数は現在ページの空白を除いた字数。ボイス再生中はボイス終了まで待ってから数える（従来どおり）。
  変更後の実測（tsint、表示完了 → 次ページ）: 短 24 字 1344ms / 59 字 2048ms、普通 24 字 2944ms / 59 字 4528ms、長 24 字 5184ms。
  普通は以前より全体に長くなる（固定 1872ms → 24 字で 2.9 秒）。長すぎると感じたら係数（`base/40`）を下げる。
  `b1_auto_wait_scales_with_text_length_and_counts_from_the_last_glyph`（4/30/120 字と文字送り中）で固定。
- **クラシック UI の autotest への影響**: 固定振付（`--autotest 24`）は途中で `a` キー（オート ON）を送るため、testbed の `frame_06_hover` だけがオート待ちの
  式変更の分だけ進み方が変わる（V8 と tsint は一致）。sample と testbed の残り 75/76 枚は変更前とバイト一致。

## 2026-11-20 実ウィンドウ経路の遅さ（高 DPI でメニューが 0.5〜1 秒遅れる）

- **前回の計測が外していた区間**: ヘッドレスの連写はスクリプトスレッドの構築時間だけを測り、描画スレッドとの競合・拡大転送を含まない。実機は
  DPI 倍率で初期ウィンドウが大きく、等倍でない表示だった。**原因は描画ループ**: ① `StretchDIBits` + HALFTONE の拡大が 1920x1080 で 1 フレーム約 15ms
  （2 倍で 18ms 超）、② それを**シーンのロックを握ったまま**行っていた、③ ops 1 つごとに `PostMessageW` して起こされるたびに再合成していた。
  メニュー構築の約 700 ops がロック待ちの行列になり（Rust の Mutex は公平でなく、描画側が解放直後に取り直す）、1.5 倍表示で**開く 908ms・LOAD
  タブの切替 1.4〜1.7 秒**まで構築が引き延ばされた（旧ループの再現。等倍 1280x720 では 40ms ほどで現れず、だから前回は気付けなかった）。
- **直し**: (1) 描画ループは 16ms 以上の間隔でしか描かない（ops で何度起こされても 1 周期に 1 回）。(2) ロックは状態更新と `Scene::clone`（Arc の複製のみ、
  数十 µs）の間だけで、合成と転送はロックの外。(3) 起こしのメッセージは投函済みなら重ねて投げない（`WAKE_POSTED`）。(4) 拡大は自前の双一次
  （`noiria_core::scale`、SWAR + std スレッド 4 並列。1920x1080 で約 2ms、2 倍 4ms、4K 9ms。`threads=1` と同一結果、float 実装と ±1.5 以内）で行い、
  転送は 1:1 の `COLORONCOLOR`。同じフレームの WM_PAINT では拡大し直さない。(5) `timeBeginPeriod(1)`（OS 境界 API を extern 宣言、クレート追加なし）:
  既定の 15.6ms 分解能だと 16ms の待ちが約 31ms になる。(6) 合成は「z 順の先頭から前フレームと署名（位置・不透明度・crop・画像/文字の `Arc` 同一性・
  文字送り位置）が同じ層」の合成結果を 1 枚覚えて再利用（`compose::PrefixCache`。メニュー表示中の下のゲーム画面など。毎回ゼロから描いた結果と
  画素一致をテストで固定）。(7) メニュー/バックログ表示中は下の ▼ を動かさない（見えないのに毎周期描き直しになっていた。静止時の再合成は 0 に）。
  X11 側も同じ pacing + スナップショット（拡大は元から自前）。
- **演出を即応寄りに**: 中身（カード・ページ送り・設定の行）とタブ/ページ切替は**フェード/スライド無しの即時**（旧タブの消去と新タブの表示は同じフレーム）。
  暗幕・見出し・ナビだけ 50ms のフェード。閉じるは 50ms。ページ送りの退場/入場の sleep は廃止（`setPage` は即時）。`MBox.go` の `ms <= 0` は即時扱い。
- **実測（実ウィンドウ経路・不可視・tsint・30 スロット埋まり・実プレイ相当の大きさ（1 スロット約 5KB、PNG 約 30KB）。クリック → 最終の層数が出るまで）**:

  | 操作 | 1280x720 before → after | 1.5 倍 (1920x1080) before → after | 1.5 倍・2 コア after |
  |---|---|---|---|
  | メニューを開く | 109 → 41ms | **908 → 65ms** | 99ms |
  | タブ切替 (LOAD / CONFIG / SAVE) | 13 / 37 / 10 → 12 / 25 / 10ms | **1697 / 447 / 19 → 21 / 30 / 16ms** | 29 / 50 / 28ms |
  | ページ切替 | 12 → 11ms | **1126 → 17ms** | 26〜31ms |
  | 1 フレームの compose+scale+present（アニメ中の中央値 / 最大） | 5.5 / 9 → 5.7 / 6.5ms | **19 / 26 → 12.6 / 15.8ms** | 19 / 22ms |
  | メニュー表示中の静止時の再合成 | 約 30fps（▼ の更新）→ 0 | 同左 | 同左 |

  before は旧ループ（ロック保持の present + HALFTONE + 無ペーシング、`NOIRIA_LEGACY` の一時トグルで再現。旧 TS ではなく現行 TS のフェード 90ms 等を除いた値で、旧 TS ならさらに +90〜180ms）。
  2 倍（2560x1440）は検証機のモニタ上限（1920x1080）で実ウィンドウを作れず未測定。拡大単体は 2 倍 4.3ms / 4K 9ms なので、フレームは 15ms 前後の見込み。
  会話画面（タイプ中・待ち）とタイトルは 1280 で中央値 3ms / 最大 10ms、1.5 倍で 6.7 / 16ms、16ms 超のフレームは 0。
- **セーブ一覧の読み込みは原因ではなかった**: 実プレイ相当のセーブ（バックログ 30 件・trail 込みで 1 つ約 5KB）30 スロットで `saveList` + JSON.parse は合計
  約 2ms（実機のセーブ先も auto/quick のみで数 KB）。サムネイル PNG は先読み済み。メタだけの索引ファイルは、効果が測れる大きさではないので作っていない
  （セーブ JSON が数百 KB になる作りに変えるときに入れる）。メニューを開く経路でセーブ書き込み・fsync は走らない（`thumbCapture` は合成 + 縮小の 8.7ms のみ）。
- **`NOIRIA_PROF=1`（恒久機能）**: 描画ループが 1 フレームごとに compose/convert/scale/blit/lock_wait/held/可視レイヤ数/アニメ中/起こし回数/クライアントサイズ/
  `since_input`（最後のクリック/キーからの ms）を `noiria.log` へ（配布ビルドの既定ログレベル warn でも、この行だけは出る）。`=2` は転送をメモリ DC へも行い、
  不可視ウィンドウでも拡大転送の実コストを測る。オフ時のコストは判定 1 回。使い方は `docs/distribution.md`。
  開発用に `--bench-window WxH --bench-script <DSL>`（Windows。不可視・無音で実ウィンドウ経路へ `PostMessageW` で入力を注入、`shot <名前>` はログの目印）。
- **既知の制約**: 画素単位の同一性は等倍（拡大なし）の合成までで、拡大表示は以前の HALFTONE から自前の双一次に変わった（見た目はやや柔らかい双一次）。
  合成の再利用はトランジション中は使わない（従来経路）。`Scene::clone` は層数に比例（約 100 層で数十 µs）。

## 2026-11-20 R3: 音声・OS 境界のレビュー指摘修正

指摘は先にテスト/計測で確かめてから直した。全て実在（誤りの指摘は無し。ただし (1) の「取り外しで WASAPI が失敗する」はレビューアの仮定で、実機未確認）。

- **出力デバイス喪失から復帰する（audio.rs / wasapi_out.rs / alsa_out.rs）**: 出力は `OutputDevice`/`OpenDevice`（トレイト + 開く関数）で差し替え可能にし、
  描画スレッドは「デバイス `run` が `Err`/予期せぬ `Ok` で抜けたら、バックオフ（250ms→×2→5s。2s 以上健全に動いたら初期値へ）の間 **null シンクが
  ミキサを実時間で空読み**し、既定デバイスを開き直す」を `stop` まで続ける。最初から開けない環境も同じ経路（旧: コマンドを読み捨てて `finished` が立たず `[wl]` 等が永久待ち。
  新: 48kHz/2ch 前提のミキサで実時間に進み `finished` が立つ。ログ「音声デバイス open 成功」は開けたときだけ）。
  開き直したデバイスのフォーマットがミキサ（起動時に固定）と違うときは `MixerPull → Resampler → ChannelConverter` で変換して書く（同じなら従来どおり直接 `mix_into`）。
  WASAPI は `WaitForSingleObject` の失敗を `Err` にし（旧: スリープ無しで `continue` = スピン）、200ms のタイムアウトごとに `GetCurrentPadding` で喪失を検知する
  （デバイス無効化後はイベントが来ないため）。ALSA は `-ENODEV`（`writei`/`prepare`）で即 `Err`、不明エラーは 5ms 待って再試行（旧: 待ち無し）、`writei` が 0 を返し続けるのも打ち切り、
  失敗で抜けるときは `drain` しない。修正前に失敗することを確認（旧挙動を一時再現: 偽デバイスが落ち続けるテスト 3 件が「finished が立たない」でタイムアウト）。
  **実機のデバイス抜き差しからの復帰は未確認**（偽デバイスでのみ検証）。
- **シークをミキサのロックの外へ**: `Mixer::prepare_start`（ミキサ不要の関連関数）でソース単体を開始位置まで読み捨て、ロック内は `play_prepared` で差し込むだけ。
  `PcmSource` に `read(&mut [f32])`（ブロック読み）と `skip_samples` を追加（既定実装は従来の逐次読み。`BufferSource` は O(1)、`ChannelConverter` は内側へ転送、
  `Resampler` は位相 `t` の f64 加算だけ 1 フレームずつ回して入力側は一括スキップ。結果は逐次スキップとビット一致をテストで固定）。
  計測（release、300 秒ステレオ 44.1k→48k のチェーンを 60 秒地点から再開）: **ミキサ Mutex を握っていた時間 778ms → 0（ロック内は `play_prepared` のみ）、
  シーク自体 3.1ms**（オフロック）。`play_from_ms` は従来 API として残し、同じ結果。
- **`mix_into` のヒープ確保ゼロ・ブロック処理**: `Resampler`/`ChannelConverter` は `Vec` を `new` で固定長確保し、`read` はブロック単位。`Mixer::mix_into` は 256 フレームずつ・スロット単位で
  処理（出力サンプルへの加算順はスロット番号順のまま、スロット状態遷移も不変なので**出力はビット一致**: 旧実装で採った基準シナリオ（リサンプル/ch 変換/ループ/自然終端/
  フェード Pause・Stop/シーク/不規則ブロック長）のハッシュを新実装でも固定）。フェード完了で Pause/Stop に遷移するスロットは完了フレームまでしか source を読まない
  （読み先行で一時停止位置の続きを捨てない）。計測（release、8 スロット・480 フレーム）: **1019.6µs → 69.6µs/回（14.6 倍。旧は約 1.8M 回の確保/20 回呼出し → 0）**。
  確保ゼロの検査は R1 の `test_support::measure_peak`（ピーク 0）を流用（グローバルアロケータは 1 クレートに 1 つしか置けないため。R1 が消したらこのテストも要見直し）。
- **BGM のデコードをコマンドスレッドから外す**: `Play` は `CommandProcessor` が都度スレッド（`noiria-audio-decode`）でデコード + シークし、完了通知でミキサへ差し込む。
  要求 ID が最新でなければ（デコード中に `Stop`/同チャンネルの別 `Play`）結果を捨てる（B14 の ID 機構はそのまま）。**同じチャンネルの後続コマンド
  （SetVolume/Pause/Resume/FadeTo）は Play の完了まで溜め、差し込み直後に同じロック内で適用**（従来は Play が同期だったので順序が保たれていた。それを維持）。
  旧: BGM のデコード中は SE や Stop が待たされた。デコード中のチャンネルは旧音が鳴り続け、完了で置換（従来と同じ）。Vorbis の FFT 化・ストリーミング化はやらない（指示どおり）。
  変更した既存テスト: `play_command_applies_start_position_and_pause_under_one_mixer_lock` は `handle` が非同期になったため `CommandProcessor::handle` + `settle()` で完了を待つ形に
  変えた（期待値は不変）。
- **WASAPI**: イベント `HANDLE` を RAII（`EventHandle`、旧: 毎回の open と `CreateEventW` 以降のエラー経路でリーク。300 回の open/close でハンドル数が増え続けることを対照つきで確認）。
  `GetBuffer`〜`ReleaseBuffer` の間でユーザーの `fill` を呼ばない順序に変更（fill → GetBuffer → 書込 → 必ず ReleaseBuffer。書込失敗時は SILENT）。
  デバイスバッファへの書込はスライスを作らず `copy_nonoverlapping`（バイト）/`write_unaligned`（i16）。WASAPI は貸与領域の整列を文書化していないので整列を仮定しない。
  長さは `src.len() == frames x channels` と `frames <= GetBufferSize` を実行時検査。`classify_format` は `nBlockAlign == ch x サンプル幅` を検査し、
  EXTENSIBLE の 16bit は SubFormat が PCM のときだけ I16 とする（旧は 16bit なら何でも I16）。24bit 等は `Err`（= null シンク + 再試行）で、対応はしていない
  （共有モードの mix format は通常 float32）。
- **ALSA**: `dlsym` の変換を `transmute_copy`（ジェネリックで型が緩い + `debug_assert` だけ）から、シンボルごとの型付き `transmute::<*mut c_void, FnXxx>`（サイズ不一致はコンパイルエラー、
  null は変換前に `Err`）に。シンボル表は成功時だけ `OnceLock` に保持（開き直しのたびに `dlopen` の参照カウントを増やさない）。再生ループは `PcmIo` トレイトに切り出し、libasound なしで
  xrun/`-ENODEV`/部分書込をテスト（WSL のみで走る）。
- **SAFETY コメント**: wasapi_out / alsa_out / mf_video の全 `unsafe` に不変条件を明記。見つかった欠陥は修正: `mf_video` の `Lock`〜`Unlock` を `LockedBuffer`（Drop で Unlock、`ptr` 非 null・`cur <= max` を検査、
  スライスは借用でロック期間に縛る）にし（旧は `?` や panic で Unlock が飛びうる）、エンコード側の `MFCreateMemoryBuffer(buf_len as u32)` の切り詰めと `max_len` 未検査を修正。
  `lib.rs` に `#![warn(clippy::undocumented_unsafe_blocks)]`（`noiria-core` の Windows/Linux とも担当ファイルは警告 0）。
- **申し送り（app 側、未変更）**: `op_audio_wait_end` は「Play がオーディオスレッドに届く猶予」を固定 60ms で待ってから `is_playing` を見る。デコードが 60ms を超える長い BGM では、
  デコード中は `is_playing` が false のため待ちが即終わる（旧実装でも同様: デコード中は同期が走らない）。デコード待ちの間 `is_playing` を真にする、等は `noiria-app` 側の判断が要る。

## 2026-11-20 R4: noiria-tools / ビルド設定のレビュー指摘の修正

各指摘は先にテストで再現してから直した（再現できなかったものは下に明記）。

- **export は `<target>.tmp` に全部書いてから入れ替える**（旧: 最終ディレクトリへ exe → pak の順に直書き）。再現: 既存出力の `game.pak` を読み取り専用にして再 export
  → 新しい exe（新鍵）+ 旧 pak（旧鍵）が残り「鍵が違います」。旧名 exe が新 pak の隣に残る問題も同じ原因。入れ替えは「既存を `<target>.old` へ退避 → rename →
  退避を削除、失敗なら戻す」。全ターゲットの書き込みを終えてから入れ替えるので、書き込み失敗（`run_export_with_writer` の注入テスト）では旧出力が無傷で `*.tmp`/`*.old` も残らない。
  影響: **`out/<target>/` は毎回丸ごと置き換わる**（手で足したファイルは消える）。`--skip-missing-targets` のスキップ側の既存出力は触らない契約はそのまま。
- **pack の除外**: `PackOptions`（`pack_dir_with`。旧 `pack_dir` は互換のラッパで、既定除外あり）。`--out`（と出力 pak 自身）は `--game` 内でも自動で外す（canonicalize 済みパスの
  一致。未作成の出力先も最寄りの既存祖先から解決）。既定除外: ドットファイル/ディレクトリ・`Thumbs.db`・`desktop.ini`・`*~`・`*.bak`・`*.tmp`（`--no-default-excludes` で解除）。
  `--exclude` は**部分文字列から glob に変更**（`*`=`/` を跨がない、`**`=跨ぐ、`?`。`/` なしはベース名、`/` ありは相対パス全体、ディレクトリに当たれば配下ごと）。
  旧 `--exclude foo` が「パスに foo を含む」だった人は `--exclude '*foo*'` に。`games/demo`・`games/sample` の pak は**バイト一致**（旧 `noiria-pack.exe` と cmp。エントリ一覧も同じ、
  demo 39 / sample 24）。`games/demo/autotest/*.txt`（3 本・計 11.6KB）は pak に入ったままにした（既定除外はゲーム固有のディレクトリ名を決め打ちしない方針。
  リリース export で外したければ `--exclude 'autotest/**'`）。
- **走査**: `entry.file_type()` を使い、シンボリックリンク/ジャンクションは該当相対パス付きのエラー、非 UTF-8 名もエラー、ASCII 大文字小文字だけが違うキーもエラー
  （Windows では共存できず Linux では別ファイル）。再現できたのは「ゲーム外を指すリンクの中身が黙って pak に入る」。**ループでのスタックオーバーフローは再現しなかった**
  （Windows は MAX_PATH で `read_dir` が失敗して終わる、Linux も ELOOP/PATH_MAX で止まる。ただしメッセージが分かりにくかったので明示エラーにした）。
  `find_mp4`（export の事前検査）もリンクを辿らない。
- **pak エントリ一覧**: `noiria-pack --list` / `noiria-export --list`（パス・元サイズ・変換 webp / wav-flac）。
- **エンコーダの出力を採用前に検証**: PNG→WebP は `decode_rgba(webp) == 元の画素`、WAV→wav-flac は `decode_wav(blob) == 元バイト列` を確認し、不一致は警告して無変換
  （PNG はそのまま / WAV は通常の DEFLATE）。実エンコーダは今は常に一致するので、不一致エンコーダを差し込んだテスト（`Encoders` 差し替え）で固定。
  所要時間（dev プロファイル、`noiria-pack` 全体、3 回ずつの実測）: demo 2.2〜2.6 → 2.7〜3.0 秒（約 +0.45 秒、+19%）、sample 1.7〜1.9 → 1.8〜2.2 秒（約 +0.2 秒、+10%）。
  pak はバイト一致（検証は変換の採否を変えない）。デコードは pack 時だけで実行時の負荷には影響しない。
- **`ProcessPrng`**: 公式ドキュメントの宣言は `BOOL ProcessPrng(PBYTE pbData, SIZE_T cbData)`（常に TRUE を返す、実体は bcryptprimitives.dll、import lib なし）。
  `#[link(name = "bcrypt")]` / `-> u8` は 2 点とも誤り（std が同じシンボルを import しているのでたまたまリンクできていた）。`bcryptprimitives` の raw-dylib と `-> i32` に直した。
  誤宣言は現環境でリンク・動作ともに通っていたため**再現不可**（静的確認のみ）。
- **`noiria-pack --key` の 16 進パース**（R1 の共有関数ができるまでの暫定、`noiria_pack.rs` の `parse_hex_key`）: `from_str_radix` が `+1` を受理し、非 ASCII でバイト境界スライスが
  panic するのを ASCII 16 進数字の事前検査で塞いだ。R1 が `pak.rs` に `pub` のパース関数を追加したら、この関数を差し替えてよい。
- **`paint.rs` の 6 重コンパイルを解消**: `gen-assets`/`gen-w3-icon`/`gen-w4-button`/`gen-w5-anim`/`gen-icon` は `noiria_tools::paint` を使う（`#[path]` と `#[allow(dead_code)]` を削除）。
  `gen-icon` の SDF 複製は `paint::sdf_round_rect`（pub 化）へ。`main.rs` の `lerp` 複製も `paint::lerp` へ。
- **重複の整理**: WAV 書き出しは `noiria_tools::wavout`（`f32_to_i16`/`write_wav`/`write_stereo`/`write_mono`）に一本化（`voice_beep` だけ 0.8 倍で clamp 無しの別式なので現状のまま）。
  dB→線形は `synth::db_to_lin`（`inst.rs` と `analysis.rs` の複製を置換。`/40` の biquad 用は別物なので残した）。`analysis.rs` のオンセット検出は `onset_indices` に一本化
  （`count_onsets` と `decay_slope` が共有）、`partial_cmp().unwrap()` は `total_cmp`、`fft` のドキュメントから存在しない `invert` を削除。
  テストの一時ディレクトリ定型は `noiria_tools::testutil::TestRoot`（RAII。`export_cli.rs` 独自の複製、`pack.rs` の 5 回、`pak_roundtrip.rs` の 3 回を置換）。
- **生成物はバイト不変**: 変更前後で `gen-assets`（sample の bg/fg/rule/bgm/se/voice）・`gen-w3-icon`・`gen-w4-button`・`gen-w5-anim`・`gen-demo-assets`（demo の絵・BGM 5 曲・SE 4 本・icon）・`gen-icon engine/sample`
  を scratch へ再生成して SHA256 を比較（49 ファイル全一致。`video/demo.mp4` のみ MF エンコーダの非決定性で毎回変わる既知事項）。
- **生成アセット一致テストの libm 依存**: `committed_sprites_match_generator` / `committed_backgrounds_and_cgs_match_generator` を「チャンネルあたり ±1 まで許容」に
  （`assert_matches_generator`。±1 を超える画素があれば、その数と「何らかの差がある画素数」を失敗メッセージに出す。±1 だけなら stderr に件数を出して通す）。
  Windows の UCRT と glibc で `sin/cos/exp/powf` がビット一致する保証が無いための予防で、今回の環境ではどちらも差は 0 画素。
- **ビルド設定**: `noiria-core/build.rs` と `noiria-app/build.rs`（非 MSVC の早期 return 経路を含む）に `cargo:rerun-if-changed=build.rs`（実際に読むのは MSVC 経路の `assets/icon.ico` だけで、
  そちらは元から出していた）。`env_logger` は `noiria-app` の optional 依存（feature `dev-log`）にし、`engine-v8` が有効にする（default と `--no-default-features --features engine-v8` は従来どおり通る）。
  dist ビルド（`engine-tsint,dist`）の依存グラフから `env_logger` とその推移依存（regex・jiff・anstream 等 17 クレート）が外れたので、`THIRD-PARTY.txt` からも削除
  （`third_party_check` は「グラフ ⊂ 一覧」の片方向検査なので通る）。**要フォロー（R2/メインループ）**: `main.rs` の `#[cfg(not(feature = "dist"))] env_logger::…` を
  `#[cfg(feature = "dev-log")]` にすると意図どおり（`--no-default-features --features engine-tsint` の非 dist ビルドは今は env_logger が無くてコンパイルエラーになる。
  `--features engine-tsint,dev-log` なら通る。docs/CI で使っている組み合わせは無いことを grep で確認済み）。`scripts/feature-build-matrix.ps1` は各構成の clippy 警告件数を表示する
  （依存 crate 分も含む総数。失敗扱いにはしない）。`rust-toolchain.toml`・姉妹リポジトリの版固定は未対応（方針判断待ち）。
- **clippy（noiria-tools）**: `--all-targets` の警告 0（`chunks_exact(2)` → `as_chunks::<2>()`、`% 2 == 0` → `is_multiple_of`、`.err().expect()` → `expect_err`、複雑型の別名、
  doc リストの空行、`chara.rs` の遅延初期化）。`h_span_aa` の未使用警告は `paint` の重複コンパイルが消えて解消（lib では pub なので警告しない）。

## 2026-11-20 R2 ウィンドウ担当（指摘 4/6/7/9/10/11/12）

- **4 フレーム周期が 60fps を割っていた（実測で再現）**: present 後の待ちが「合成・転送が終わった時点から 16ms（Win32 は +1ms）」固定で、
  周期 = 17ms + 作業時間だった。`bridge::frame_wait`（Win32/X11 共通）に切り出し、アニメ中の待ち = 前回 present の**開始**から 16ms の残り。Win32 の
  ミリ秒変換は切り上げ（切り捨てだと期限直前に 0ms 待ちで空回り。旧 `+1` の代わり）。実測（tsint、demo の START 後の文字送り、不可視ウィンドウ
  `--bench-window`、`NOIRIA_PROF` のアニメ中フレーム間隔）: 1920x1080（`PROF=2` = 実転送込み）中央値 23.0 → 16.7ms、p90 24.7 → 18.5、最大 27.9 → 24.8、
  平均 43.8 → 60.0fps。1280x720（`PROF=1`）は中央値 20.0 → 16.7ms、50 → 59.3fps（after の最大 50ms は 1 回だけの 33ms 合成スパイク、他ウェーブの並走でマシンが高負荷）。
  `FRAME_INTERVAL` は 16ms のまま（約 62.5fps が上限）。
- **6 Win32 のモーダルループ中に全てが止まる**: フレーム処理を `tick(hwnd, &WindowShared)` に切り出し（状態は `WindowShared::frame_loop` の `RefCell<FrameLoop>`）、
  外側のループと `WM_TIMER`（`WM_ENTERSIZEMOVE`/`WM_ENTERMENULOOP` で 16ms の `SetTimer`、`WM_EXITSIZEMOVE`/`WM_EXITMENULOOP` で `KillTimer`）の両方から呼ぶ。
  再入しても借用に失敗したら何もしない（`try_borrow_mut`）。テストは本物のモーダルループを使えないので、`DefWindowProc` のモーダルループと同形のポンプ
  （`WM_ENTERSIZEMOVE` を送り、`PeekMessage`/`DispatchMessage` だけを回す。テスト専用メッセージ `WM_APP+1`）で外側のループを止める。`SetTimer` を無効にして確認:
  トランジションはポンプ（1500ms）が終わるまで完了せず（1.5055s）、`UserEvent::Quit` もポンプ終了まで処理されない（2 テストとも失敗）。修正後は通る。
- **7 `blit_dc` の帯塗り**: `viewport::letterbox_bands`（純関数、左・右・上・下を各辺独立に判定）へ。旧実装は `vp.x > 0 || vp.y > 0` のときだけ塗り、1281x720 では余り 1px で
  `vp.x == 0` のため最後の列が塗られなかった。テストは 1281x720 / 1280x721 と、帯がビューポートの補集合に一致すること。
- **9（ウィンドウループ側）**: `bridge::lock_scene`（`PoisonError::into_inner`）に置換（win32/x11 のループ）。script 側（engine_state 等）は別担当。`lock_scene` は pub なので流用可。
- **10 `WAKE_POSTED`**: プロセス全体の static をやめ、`WakeSink::Win32 { posted: Arc<AtomicBool> }` と `WindowedParams::wake_posted` で窓ごとに持つ（`WakeSink::send` の API は不変）。
  `hwnd_slot` は `WM_DESTROY` で 0 へ戻す（遅れた起こしは `post_wake` が hwnd 0 で何もしない）。`main.rs` に `wake_posted` の生成 3 行を足した。
  テスト: 別窓のフラグが立ったままでも起こしが届く / 破棄後にスロットが 0。※旧 static での失敗は新 API では再現できないため、構造（共有 static の除去）で担保。
- **11 X11 の拡大**: `viewport::ScaleCache`（Win32 の `(frame_gen, w, h)` キャッシュを共有化）と `LetterboxCache`（X11: 帯つき全面バッファ）に統一し、拡大は `scale_bilinear`
  （旧 X11 の独自単スレッド実装を廃止）。同じ (世代, サイズ) の再表示（Expose/ConfigureNotify）は拡大も全面ゼロ埋めもしない。新フレームは拡大し直すがゼロ埋めはしない
  （サイズ変化時のみ）。**画素**: 旧 X11 は 8bit 固定小数の独自実装、新は Win32 と同じ `scale_bilinear`（float との差 ±1.5 以内）なので X11 の拡大表示の画素が最大 1 段階変わり得る（等倍は不変）。
  既存テスト `blit_*`（viewport）は `LetterboxCache` 経由の `blit_letterboxed`（テスト専用ラッパ）で期待値そのまま通る。
- **12 `unsafe`**: win32.rs の全 unsafe に `// SAFETY:`、`unsafe fn` は `shared_of` だけ残して `# Safety` を記述（`toggle_fullscreen`/`request_close` は安全な関数 + 内部 unsafe に）。
  `wndproc` は `unsafe extern "system"` の薄い入口 + 安全な `wndproc_impl`。`blit_dc` は `debug_assert` ではなく実行時検査（`buf.len() < fw*fh` なら描かない、テストあり）。
  winbench/fatal/winmm extern にも 1 行ずつ。crate ルートに `#![warn(clippy::undocumented_unsafe_blocks)]`（`cargo clippy --all-targets` で noiria-app の該当 0）。

### 2026-11-20 R4 続き: `game.json` の `pack.exclude`

- **仕様**: `game.json` の任意フィールド `"pack": {"exclude": [glob...]}`。除外は **既定 + project（game.json）+ cli（`--exclude`）の和集合**（glob の文法・照合は同一）。
  `--no-default-excludes` は既定だけ、新設の `--no-project-excludes` は game.json の指定だけを切る。判定の優先順は output → default → project → cli（報告に出る由来はこの最初の一致）。
  型が不正（`pack` がオブジェクトでない / `exclude` が配列でない / 要素が文字列でない）は `pack.exclude[1]` のように該当箇所を示すエラー。`--no-project-excludes` 時は `pack` を読まず、壊れた game.json も無視する。
- **保護**: `game.json` と、`game.json` の `"main"` が指すファイルが（どの由来の規則でも）除外に当たったらエラー（起動できない pak を作らない）。`main` が実在しないディレクトリは従来どおり pack できる
  （除外一覧に当たったときだけ検査するため。テスト用の最小ゲームを壊さない）。pack 側は `pack` を独立に解釈する（`serde_json::Value`）ので、`GameConfig` は変えていない。
- **エンジン側は変更なし**: `GameConfig` は `deny_unknown_fields` を持たず未知の `"pack"` は serde が無視する。`pack` 付きの `games/demo/game.json` を V8 / tsint 両方のヘッドレス autotest
  （`--autotest-script autotest/hinata.txt --autotest-fast`）で EXIT=0 を確認、`GameConfig` が `pack` 付き JSON を読めることは `engine_game_config_ignores_the_pack_field` でも固定。`noiria-app/src` は触っていない。
- **除外の報告**: `PackOutput.excluded` に除外ファイルと規則（除外ディレクトリは配下を 1 件ずつ列挙、`--out` は 1 件）。`--list-excluded` で表示、`noiria-pack`/`noiria-export` は最後に
  `除外 N 件（default / project / cli / output の内訳）` を 1 行表示。
- **`games/demo`**: `"pack": {"exclude": ["autotest/**"]}` を追加 → pak 39 → 36 エントリ（autotest 3 本が外れる）。前節の「autotest を既定除外にしない」判断は維持（既定は汎用規則だけ、
  ゲーム固有の指定は game.json へ）。`games/sample`・`games/testbed` は無変更。
- **注意（R1 の pak.rs 変更）**: この時点で `noiria-pack` が書く pak はヘッダの予約 8 バイト（40..48）が非ゼロになった（R1 の変更。noiria-tools 由来ではない）。sample の pak は旧バイナリ出力と
  その 4 バイトだけが異なり、エントリ・内容は同一。

## 2026-11-20 R1: noiria-core の外部入力（ゲームファイル・セーブ・フォント）の堅牢化

指摘ごとに「細工した入力で panic / 過大確保 / 無限ループを再現するテスト → 修正」の順で直した（全て修正前に落ちることを確認）。正常なファイルの出力は不変
（デモ 3 ルート・sample の PNG ハッシュは変更前後で一致）。壊れた入力は `Err` を返すだけ。

- **pak ヘッダの予約領域にインデックスの CRC32 を追加（形式変更）**: ドキュメントは「インデックスは CRC 検査」だったが実際は展開後の長さしか見ておらず、パス 1 バイトの化けを検出できなかった。
  オフセット 40 に平文インデックスの CRC32（u32 LE）、44〜47 は予約のまま。**0 は「記録なし」= この欄ができる前の pak で、検査を飛ばして開く**（旧 pak を新リーダーで読める。
  新 pak は旧リーダーも予約領域を見ないので読める）。CRC がたまたま 0 の pak（2^-32）は検査されない。書き込み側は 48 バイトのプレースホルダの後ろへ直接追記してヘッダを最後に
  埋める（予約領域以外は 1 バイトも変わらない。旧アルゴリズムの複製との一致をテストで固定。8MB の pak のビルドでピーク増分 1.00 倍 → 0.63 倍）。
- **展開サイズの上限**: `inflate_limited` / `zlib_decompress_limited` を追加（既存の `inflate` / `zlib_decompress` は上限なし）。PNG は IHDR から求めた期待サイズ + 64KiB、pak は索引の
  `plain_len`（wav-flac は `decode_wav_limited`）、索引自身はヘッダの `index_plain_len`（さらに 1GiB 上限）。DEFLATE 爆弾（1032:1）で確保が膨らまない。
- **画素数の上限 2^26（8192x8192、RGBA で 256MB）**: PNG / JPEG / WebP。ヘッダが申告する寸法だけで数 GB を確保させない（PNG の `h * (stride + 1)` のオーバーフローも解消）。超えたら `Err`
  （黙って縮小はしない）。
- **JPEG**: セグメント長の検査、`tq` / `td` / `ta` < 4、`h` / `v` ∈ 1..=4。**fuzz が見つけた指摘外の panic**: 輝度の標本化がクロマより粗い JPEG（Y 1x1・Cb/Cr 2x2 など、仕様上は合法）で、
  アップサンプルが輝度プレーンを `px` / `py` で直接引いて範囲外だった。各成分を自身の標本化比で引くようにした（通常の JPEG は同じ位置なので出力不変）。
- **Vorbis**: setup のコードブック番号（floor の master / subclass book、residue の classbook / book）を `parse_setup` で検証（libvorbis と同じ検査）。コードブックは「エントリ数 ≤ 2^20」
  「entries × dimensions ≤ 2^24 個」「multiplicand のビット数が残り入力以内」。ordered 形式は入力が数バイトでも宣言した数だけ表を作らされ、入力サイズでは縛れないためエントリ数は固定上限にした
  （実在のエンコーダの帳簿は高々数万）。
- **フォント**: TTC の `numFonts` を `(len - 12) / 4` にクランプ（`0xFFFFFFFF` で約 40 億回ループ）。複合グリフは深さ 8 に加えて 1 グリフあたりの展開数予算 256（4^8 = 65536 回の展開を `Err` に）。
  ラスタのマスクは幅・高さ 4096px 超を空マスクにする（unitsPerEm=16 で 6 万 px 四方の確保を再現）。**既存テスト `probe_font_count_detects_ttc_header` を更新**: ヘッダ 12 バイトだけで
  numFonts=3 を名乗る構造だったので、オフセット表 12 バイトを足した（期待値は同じ 3）。
- **text.rs のフォールバック**: どのフォントにも無い文字が出るたびに全システムフォントを読んでパースし `self.fonts` に永久保持していた（再現で 418 個保持）。文字ごとの「どこにも無い」集合を持ち、
  探索は cmap を引くだけでグリフを持たないフォントは保持しない。探索順は不変で描画結果はバイト不変。
- **savedata**: `thumb_path` が `check_slot_name` を呼んでおらず、`screenshot` op の文字列で `../../x` や絶対パスから**セーブ領域外へ PNG を書ける欠陥**だった（確認済み）。`thumb_path` 内で検証。
  `read_slot` は「読めたが UTF-8 / JSON として壊れている」場合だけ `.bak` へ退避し、読み込み自体の失敗（共有違反・権限）は `Err`（旧実装は正常なセーブを退避して以前の `.bak` も潰した）。
  `write_thumb` は `.png.tmp` へ書いて rename（先読み相当の読み手が半端な PNG を見る回数 45/72 → 0）。
- **その他**: AVI の `LIST` 入れ子は深さ 8（実在は 3 段）。`TextReveal::visible_glyphs` は `saturating_add`。`localtime.rs` の `time_t` は 32bit glibc だけ `c_long`、他は `i64`（32bit は未検証）。
  unsafe（`pak.rs` 1 か所、`localtime.rs` 2 か所）に `// SAFETY:` を付けた。**レビューが外れていた点**: `codec/webp.rs` に `unsafe` は無い（`forbid(unsafe_code)` の `image-webp` を包むだけ）。
- **pak キーの大文字小文字（開発側を厳格化）**: `Dir` バックエンドで、要求パスが実ファイル名と大文字小文字だけ違うとき warn を 1 パスにつき 1 回（`canonicalize` で実名を引く。読み込みは従来どおり成功）。
- **`noiria_core::pak::parse_hex_key(&str) -> Result<[u8; 32]>` を `pub` に**: バイト単位で 16 進数字だけ受理（マルチバイト文字での panic と `+f` / `-1` の受理を解消）。
  `noiria_pack.rs` の同等の複製はこれへ差し替えられる（R4 側）。
- **破壊 fuzz**（jpeg / png / inflate+zlib / wav / ogg+vorbis / wav-flac / AVI / IVF / pak（ファイル・平文索引・封じ直した索引）/ font / cmap / TTC、各 1000〜5000 件・各 2.5 秒以内）:
  `test_support.rs`（`#[cfg(test)]` のみ）の `#[global_allocator]` で「1 回の確保が 512MB 超なら確保前に panic」「1 件 3 秒超なら失敗」を共通に検査する（本物の巨大確保で
  マシンを巻き込まないため）。`NOIRIA_FUZZ_SCALE=30` で件数 30 倍（全通過、約 90 秒）、`NOIRIA_FUZZ_TRACE=1` で固まった入力を `%TEMP%/noiria_fuzz_current.bin` に書く。
  修正の一部（JPEG の DQT/SOF 長さ検査、Vorbis の book 検証）を一時的に外すと fuzz が落ちる（jpeg 40/7500、ogg/vorbis 6/3000、AVI 6/6000）ことを確認済み。
  Vorbis のデコードは 1 秒ぶんの音声で約 600ms と遅い（本ウェーブの対象外）ため、fuzz の基準入力は先頭 6〜10 パケットに切り詰めた。

## 2026-11-20 R2 script 担当（指摘 1/2/3/5/8/9/13 と R3 申し送りの `waitEnd`）

- **共有 `target/` で v8 のビルドスクリプトが落ちる原因（再発防止）**: Git Bash から cargo を叩くと cwd が小文字 `c:\...` になり、v8 の build.rs が
  レジストリ側（`C:\...`）と「別ドライブ」と判定して `gn_root` の symlink を作ろうとする（権限が無く `1314` で panic）。**cargo は PowerShell で
  `Set-Location 'C:\Users\...'`（大文字）してから実行する**。コードの問題ではない。
- **1 寸法の検査（`Pixmap::try_new` / `checked_dims`、`noiria-core/src/pixmap.rs`）**: 1 辺 ≤ 16384 かつ `w*h*4` ≤ 256MiB（R1 の画像デコード上限 2^26 画素と同じ総量）、`checked_mul`、
  確保は `try_reserve_exact`（失敗は abort でなく Err）。`layer_fill`/`layer_msgbox`/`layer_load_image_scaled`/`thumb_capture(w,h)`/`[image]`/`blit` の clip 矩形/
  universal トランジションの rule 画像/`ui_cache`（サムネイル・縮小）に適用。画像ファイルは**デコード後**の寸法も検査（`ui_cache::load_image_checked`。デコーダの確保自体は R1 の上限が受け持つ）。
  再現: 修正前は `fill(100000,100000)` が 40GB 要求（この機では commit が通って数秒固まった後に続行、commit できない機では abort）、`op_layer_fill(2^31, 2^31)` は release で乗算がラップ。
  V8/tsint 両方で拒否されるテスト `oversized_dimensions_are_rejected_on_both_engines`。`thumb_capture` は Err を返すように（op のシグネチャ変更: 戻り値が `Result`）。
- **2 `screenshot` / `thumb_capture`**: ロック下では `Scene::clone`（Arc の複製）と時刻の取得だけ。確保・合成・縮小・PNG 書き込みはロックの外（`compose_current`）。
  別スレッドの `scene.lock()` の最大待ちは、修正前（ロック保持に戻して再現）69.9ms（合成 1 回 63ms）→ 修正後 0.00ms 未満。
- **3 数値変換の統一（`script/numconv.rs`）**: 規則は「**非有限は 0、四捨五入（0 から遠い側）、対象型の範囲へ飽和**」。V8 の op 引数は整数型直受けをやめて全て `f64` で受け、
  構造体（`LayerProps`/`MoveArgs`/`BlitClip`/`TransArgs`）は `deserialize_with` で、`TextStyle`/`TextSegment` は app 側の写し（`TextStyleArgs`/`TextSegmentArgs`、noiria-core の型は無変更）で受けて同じ関数を通す。
  tsint の旧 `as_u32` 等（四捨五入→クランプ）も同じ関数へ。旧 tsint のコメント「V8 と同じ丸め」は誤りだった。差が出ていた値: `x:10.6` V8=10/tsint=11、`opacity:127.5` 127/128、
  `fill(w=-1)` 4294967295/0、`cropTo` の `timeMs`（V8 u32 ラップ/tsint u64 クランプ）、`TransArgs.timeMs` 未指定（V8 必須エラー/tsint 0 → 両方 0）。**新しい規則は旧 tsint とほぼ同じ**
  （NaN→0 と四捨五入は同じ、ラップが飽和へ）。**時間（ms）は `u32::MAX`（約 49 日）で打ち切る**: tween の `start + duration`（`Instant` の加算）が u64 いっぱいで panic するため。
  **座標（x/y）は ±(2^30-1) へ飽和**: `noiria-core/src/scene.rs` の `lerp_i32` が `b - a` を i32 で計算し、`y=-1e20`（i32::MIN に飽和）から別の値への move が debug で overflow panic
  （V8 の fast op 内 panic = プロセス abort）になる。scene.rs 側の 1 行（`i64` で引く）が本筋だが R1 の持ち場なので触らず、入口で防いだ（**申し送り**）。
  既存デモ・sample・testbed は整数しか渡していないため PNG は不変（下記）。`g_num`（x=10.6 の矩形、V8/tsint）は修正前は PNG が食い違い、修正後は一致。
- **5 tsint の `drive()`（実時間クロックだけ）**: 1〜2ms スリープの再ポーリングをやめ、入力チャンネルとトランジション完了通知（`trans_notify`）を別スレッド（`noiria-tsint-wake`、tokio current-thread）で待って
  `Doorbell`（Mutex+Condvar、立てたフラグは `wait` が消費するので `wait` に入る前の `ring` も失われない）で起こす。眠り時間 = 次のタイマー期限、再生終了待ちがあれば 16ms、トランジション待ちがあれば 50ms
  （完了通知の取りこぼしに対する安全網）、何も無ければ 500ms 上限（未知の起床要因の取りこぼしでも止まらない）。待ち手が居ない間は `trans_active`（シーンのロック）も取らず、期限が来たタイマーが無い周は一覧を作り直さない。
  **仮想クロック（ヘッドレス）は従来どおり `try_recv` + 最大 2ms のポーリング**（決定論を維持。デモ/sample/testbed の全 PNG が不変）。実測（クリック待ち、`timeBeginPeriod(1)` 下、起動 1.5 秒後の 1 秒間）:
  **起床 422 回/秒 → 2 回/秒、CPU 15.6ms/秒（計測粒度）→ 0.0ms/秒**。入力から再開まで 3 → 1ms。テスト: `drive_sleeps_while_waiting_for_input`（起床回数）、`input_event_resumes_a_waiting_script_promptly`、
  `trans_wait_resumes_promptly_after_the_renderer_completes_the_transition`（V8/tsint 両方）。
- **8 `Duration::from_secs_f64`**: V8 `op_wait`・tsint `op_wait` とも `numconv::wait_duration`（NaN・負は 0、`+Infinity`・巨大値は `u32::MAX` ms に打ち切り）。他の `f64 → Duration/整数` は
  `layer_move_path_by_distance`（`total` の上限を u64 → `u32::MAX` に締めた。non-finite/負は元からエラー）、動画の位置/フレーム（元から有限性と範囲を検査）、`audio_play` の開始位置（元から検査。既存テストが
  `f64::MAX → u64::MAX` を固定しているので変更せず）で、他に panic する箇所は無かった。再現: 修正前の exe で `op_wait(Infinity)` は V8 が「unwind できない関数で panic」→ abort（EXIT=127）、tsint はスクリプトスレッドが panic。
- **9 Mutex の毒化（script 側）**: `bridge::LockExt::lock_unpoisoned()`（`PoisonError::into_inner`）へ機械置換（engine_state・ui_cache・headless・テスト以外の `lock().unwrap()`）。
  `a_poisoned_scene_lock_does_not_cascade_into_later_ops` で、ロックを握ったまま panic したスレッドの後でも ops が動く。
- **13 先読み（`ui_cache::Prefetcher`）**: 呼び出しごとのデタッチスレッドをやめ、最初の依頼で起動する 1 本のワーカー + キュー。EngineState の drop で停止して join（残りは捨てる）。panic は `catch_unwind` でワーカーを続行。
  ギャラリーの縮小画像は 336x189（254KB）で 24MiB の上限に **98 枚**入る（これで足りる想定）。それを超える依頼は**先頭からキャッシュに入る枚数までしか先読みしない**（入れた端から追い出し合って毎回再デコードになるため）。
- **R3 申し送り: `waitEnd` の固定 60ms 猶予を状態判定へ**: `AudioShared::is_play_pending(ch)`（Play 要求済み・ミキサ未差し込み。Stop・別の Play・デコード失敗で偽に戻る。既存の要求 ID の仕組みを流用）を追加し、
  app 側は「`is_playing` でも `is_play_pending` でもなくなったら終了」（V8 の `op_audio_wait_end`、tsint の drive、共有の `EngineState::audio_busy`）。`finish_decode` は差し込み直後に `sync_shared` してから確認応答するので、
  「どちらも偽」の隙間（次の 10ms 周期まで）ができない。音声スレッドが無い（送信先が閉じている: ヘッドレス）場合は `audio_play` が `cancel_play_request` で取り下げる
  （さもないと保留のまま `waitEnd` が永久に待つ）。音声デバイス自体が開けず読み捨てモードの `drain_commands` も確認応答する。
  **ヘッドレスの人工物の解消**: 旧 V8 は実時間 60ms の `sleep` が仮想時間で約 1 秒に化けていた。新しい判定には猶予が無いので消える。デモ 3 ルート（`--autotest-fast`、空の `--save-dir`）・sample・testbed（`--autotest 24`）の全 PNG（118 枚）は
  変更前後でバイト一致（V8 と tsint の一致も維持）で、この変更でフレームが動いたものは無かった。テスト: `wait_end_waits_while_a_play_request_is_decoding_and_ends_when_it_is_dropped` /
  `wait_end_on_an_idle_channel_returns_without_a_grace_period` / `play_without_an_audio_thread_does_not_leave_a_pending_request`（V8/tsint 両方）、core に `wait_end_condition_holds_from_the_play_request_through_decode_to_playback`
  （偽の遅いデコーダで「要求→デコード→再生」を通して途切れない）・`decode_failure_clears_the_pending_play_request`・`stop_and_a_newer_play_replace_the_pending_request` 等。
- **`--no-default-features --features engine-tsint`（dist なし）のロガー**（R4 申し送り）: `main.rs` の env_logger 側を `cfg(all(not(dist), dev-log))` にし、どちらも無い構成には stderr へ出す最小ロガー
  （`NOIRIA_LOG`、既定 info）を足した。`Cargo.toml` の `dev-log` のコメント（「`cfg(not(feature = "dist"))` 側で使う」）は R4 のファイルなので触っていない（実態は `dev-log` 側に変わった）。
  なお `--features dist`（engine-v8 も有効のまま）だと `env_logger` が「未使用の依存」警告になる（dist は tsint のみで作るので実害なし）。

## 2026-11-20 clippy 警告 0（noiria-core / noiria-app）

- **`chunks_exact(定数)` は `as_chunks::<N>().0` へ、他は機械的に整理**（`assets.rs` の `save_png` は `checked_div` クロージャ、`audio.rs` は `iter_mut().enumerate()`、`codebook.rs` は `multiplicand_offset` を `entry * dimensions + j` に）。端数は元の `chunks_exact` 同様に無視。`pixmap.rs` の合成ホットパスは release の `bench` で変更前後に差なし（±5% 以内）。`#[allow]` は `prof.rs` の `cfg_attr(not(windows), allow(dead_code))` 1 件のみ（計測は Win32 側にしか配線されておらず、WSL の `-D warnings` で落ちた）。`pixmap.rs`/`text.rs` 等のテスト内の同種の置換は式の書き換えのみで、期待値は不変。
- **退行防止**: `scripts/feature-build-matrix.ps1` の clippy を `-- -D warnings`（警告があればその構成は失敗）にした。実行方法は README のテスト節（PowerShell は `C:` を大文字にした cwd で実行）。
