# Backlog

未着手・保留の作業項目を優先度順に記録する。完了済みの経緯は `implementation-notes.md`、
仕様の互換範囲は `kag-tag-compat.md` を参照。動画デコーダ (vp9dec) 側の backlog は
`../../vp9dec/docs/backlog.md`（SIMD 最適化・profile 1-3 対応など）。

各項目の根拠（ドキュメント節・コード位置）は 2026-07-17 のリポジトリ走査で確認済み。
根拠のない項目は載せていない。

## 直近の出来事（記録）

- **2026-11-22: B23 画像キャッシュ（対応済み）** — `UiCaches::images`（64 MiB のバイト上限つき LRU キャッシュ）と `load_image_cached` を追加し、`layer_load_image` / `layer_load_image_ex` / `layer_blit_image` でデコード済み `Arc<Pixmap>` を共有。`TextEngine::image_cache` も 16 MiB 上限の LRU とした。
- **2026-11-22: B8 ストリーミング音声デコード（対応済み）** — `StreamingVorbisSource` と `StreamingWavSource` を実装し、`audio::build_source` の全量 PCM デコードを定数メモリの逐次デコードへ置換。`GameDir::open_reader` 経由でファイル・pak 双方のストリーミング再生・シーク・ループに対応。
- **2026-11-19: B2 プレイヤー向けシステム UI 層（対応済み）** — システムメニューを刷新（セーブ/ロード/設定タブ、サムネイル・保存日時・章名・本文冒頭つきの 6 枠 × 5 ページ + オート/クイック枠、確認ダイアログ、タイトルへ戻る/終了、未読もスキップ・窓の濃さ・初期設定リセット、キーボード操作）。F5/F9 のトースト、選択肢でのオートセーブ、常設システムボタン（Q.SAVE/Q.LOAD/SAVE/LOAD/AUTO/SKIP/LOG/MENU）、メッセージ窓の frame/color/margin と名前欄の色、話者名・ボイスつきバックログ（セーブ時点の履歴を復元）、選択肢の ↑↓+Enter と hover/click 音。新 op `thumbCapture`/`thumbSave`/`layer.loadSaveThumb`/`dateString`。デモへ適用済み。
- **2026-11-19: A ホストの出荷品質化 + スクリプト駆動 autotest（対応済み）** — Win32/X11 ウィンドウを
  リサイズ可（アスペクト保持レターボックス、マウス座標の逆変換は `viewport.rs` の純関数）にし、起動時
  サイズをモニタ DPI 倍（非整数可・作業領域に収める）、`WM_DPICHANGED` 追随、F11 / Alt+Enter の
  ボーダーレスフルスクリーン（X11 は `_NET_WM_STATE_FULLSCREEN`）を追加。キーのオートリピートは送らず
  `keyup` を送出、閉じる操作は `WindowClose` をスクリプトへ送って 2 秒の猶予後に強制終了。dist ビルドの
  致命エラーは `MessageBoxW`（ヘッドレス/テストでは出さない）。`--autotest-script`（行指向 DSL）・
  `--autotest-fast`・`--save-dir` と、デモ 3 ルート用スクリプトを追加。
- **2026-07-27: B4 フレーム精度の動画 seek とイベント制御（対応済み）** —
  0-based 表示フレームまたは ms 位置への厳密 seek、`videoevent`、`videosegloop`、
  `cancelvideoevent`、`cancelvideosegloop`、`wp` を slot 0 へ実装した。区間は
  `[start,end)`、period は 1 回発火型、loop/prepare/segLoop は latch とする。
  再生世代と scene→control の commit gate により、decode 中の seek・区間設定・
  イベント登録・停止が旧フレームや旧イベントを新しい状態へ漏らさないようにした。
- **2026-07-26: B2 V8/tsint エンジン間決定論一致（対応済み）** — V8 の期限到達
  `op_wait` は Promise 継続を drain してから tick barrier を完了し、新規 wait は
  登録時点の virtual-clock watch version から待機する形へ変更。`frame_03_hover.png` の
  1 tick 差を解消し、両エンジンの 24 秒 autotest 全 16 フレームがバイト一致した。
- **2026-07-26: B12 `move spline=true`（対応済み）** — KAG3 標準の属性を追加し、
  現在位置と `path=` の指定点を KAG `SplineMover` と同じ自然三次スプラインで補間する。
  x/y だけを曲線化し、opacity・既存の時間配分・accel は従来どおり。V8/tsint と
  queue/skip/割込みを含む Windows / WSL 回帰テストで確認した。
- **2026-07-26: B14 BGM 再生位置の一時保存・復元（対応済み）** —
  ミキサの出力フレーム位置を音声スレッドから参照できるようにし、完全スナップショットへ
  `positionMs` を保存する。`tempload` / `goback` / `gotostart` はその位置から BGM を作り直し、
  一時停止中なら位置指定再生と pause を同じミキサロック内で適用する。要求 ID により連続した
  play/stop と位置同期の競合を防ぎ、旧セーブや不正値は 0ms へ補完する。有限音源の長さ上限で
  巨大な復元位置の走査を省略し、停止フェード中の pause は停止を即時確定する。
- **2026-07-26: B20 VP9 10/12-bit の SDR 表示（対応済み）** — U16 plane を
  bit depth に応じて 8-bit 等価値へ小数精度を保って縮尺し、既存 BT.601 limited
  変換で RGBA8 化する経路を追加。8-bit 経路は演算不変。自己生成した hermetic な
  10-bit 4:2:0 / 12-bit 4:4:4 IVF、odd 4:2:0、4:2:2、エラー経路、rewind を
  Windows / WSL で確認した。
  再生中に解像度が変わる VP9 は表示フレームごとの寸法でレイヤを更新し、worker の panic 時も
  再生中フラグを解除する。
  PQ/HLG の真の HDR トーンマップは入力メタデータ不足のため下表に残す。
- **2026-07-26: B13 `align=` / `linespacing=`（対応済み）** — `style` / `defstyle` /
  `resetstyle` をメッセージレイヤ別の行設定へ配線。折返し・改行後の各行を
  left/center/right で独立整列し、signed px の行間を最低 1px にクランプする。
  行設定は現在レイヤの既存全文へ即時リフローし、追記時の reveal 境界も同時に更新する。
  `[locate]` を含む行は絶対 x を保ち、旧セーブは left/0 へ補完。不正値の V8/tsint parity と
  決定論 PNG 比較まで確認した。
- **2026-07-26: B10 並列テストの flaky 解消（対応済み）** — ALSA テストは
  process-global な `NOIRIA_ALSA_DEVICE` の書き換えをやめ、明示デバイスを直接開く形へ変更。
  V8/tsint タグハーネスは実時間の静止区間で待機到達を推定するため、ハーネス実行を直列化して
  ハンドラ登録前のクリック早撃ちを防止した。Windows 通常並列と WSL ALSA の反復で再発なし。
- **2026-07-26: B18 export 部分成功モード（対応済み）** — 既定の strict 契約を維持したまま
  `--skip-missing-targets` を追加。全ターゲットを出力前に検証し、明示オプション時だけ
  見つからない OS バイナリをスキップする。全欠如・権限エラー・不正 `KEY_SLOT` は失敗する。
  Windows / WSL の API・CLI 回帰テスト 4 件で確認。
- **2026-07-26: B9 lossy WebP 実サンプル検証（対応済み）** — 自前生成した
  16×16 の lossy VP8 WebP（`VP8 ` チャンク）を固定フィクスチャとして追加し、
  `codec::webp::decode_rgba` の寸法・RGBA 展開・4 色の代表画素を回帰テストで確認。
  外部画像や実行時の外部ツールには依存しない。
- **2026-07-17: vp9dec の破壊的 API 変更への追随（対応済み）** — vp9dec の設計刷新
  （`decode_frame` が `Vec<DecodedFrame>` を返す形へ変更、2026-07-16）により
  `codec/video.rs::Vp9Ivf` がビルド不能になっていたのを修正。表示フレームは
  チャンク毎に高々1つ（`frame: Some`）を拾う形へ。video 6/6・codec 103/103・
  ワークスペース全テスト green を確認済み。今後 vp9dec 側で API を変えた場合は
  noiria の `cargo check -p noiria-core` を破壊検知として即時に回すこと。

## A. 長期ロードマップ（外部依存の段階的自作化 — README の表が正）

| # | 項目 | 規模 | 根拠 |
|---|---|---|---|
| A1 | V8/deno_core の完全置換・削除（自作 `tsint` は `--engine tsint` で選択可能。並行検証期間中のためデフォルトは V8 のまま） | 大 | README ロードマップ表 |
| A2 | MPEG-1 デコーダ自作（「次の自作候補」。JPEG の IDCT/Huffman を再利用可能） | 大 | README / video.rs の `bail!("MPEG-1 は未対応…")` |
| A3 | WebP (VP8L) の自作（`image-webp` は swap 前提で `codec/webp.rs` に隔離済み） | 大 | README / implementation-notes |

※ Media Foundation（`mf_video.rs`）は OS 境界 API として自作化対象外（README 明記）。
※ HEVC は特許のため自作せず MF のまま（決定事項、backlog ではない）。

## B. 具体項目（近〜中期）

### 機能ギャップ（大きめ）

| # | 項目 | 規模 | 状態 |
|---|---|---|---|
| B6 | 高度なテキストシェーピング（合字・カーニング・複雑スクリプト。自作フォントエンジンは 1字1グリフ+禁則のみ。簡易ルビは対応済み） | 大 | 保留 |
| B3 | 音声圧縮 -50% 目標（実績 -42.7%、配布合計 ~15.4MiB vs 目標 ~10MiB。算術符号/適応符号 or ピッチ/長期予測が必要） | 大 | 保留（コンテンツ起因の限界と判断済み・再開はユーザー判断） |
| B1 | ゲーム毎 exe のファイルアイコン（PE `.rsrc` パッチ。ウィンドウ/タスクバーは対応済み、Explorer 上のみ既定アイコン） | 中〜大 | 明示保留 |

### 機能ギャップ（中小）

| # | 項目 | 規模 |
|---|---|---|
| B5 | pak 内 MP4 再生不可（MF が実ファイルパス要求。`.avi`/`.ivf` 推奨、export 時警告あり） | 中 |
| B20 | 真の HDR（PQ/HLG）→SDR トーンマップと色忠実性。10/12-bit SDR の RGBA8 表示は 2026-07-26 対応済みだが、現行の公開 `vp9dec::Frame` / IVF には transfer・primaries・mastering metadata が無く、HDR かを判定できない。メタデータを保持するコンテナ/API 経路が前提 | 中 |
| B7 | IME 対応（`edit`/`input`/`ask`。現状 `WM_CHAR` の確定文字のみ） | 中 |
| B11 | `hch`（縦中横）はプレーンテキストにフォールバック / `hact` はテキスト記録のみ | 中 |
| B19 | BGM シンセの主観品質チューニング（voice 毎 AM depth 等、指摘があれば） | 小 |
| B21 | BGM のループポイント指定（現状は曲全体のループのみ。`setbgmlabel` は対象外スタブ） | 中 |
| B22 | レイヤ単位のトランジション（`[trans layer=]` は全画面トランジションへフォールバック） | 中 |
| B24 | UI 文字列の多言語化（システムメニュー/確認ダイアログ/トーストは日本語固定） | 中 |
| B25 | ゲームパッド入力（メニュー/選択肢はキーボード・マウスのみ） | 中 |
| B26 | CFF/OTF フォント（`.otf`/OTTO の同梱は読み込まず error ログ。TrueType のみ） | 中 |
| B27 | プログレッシブ JPEG / 16bit PNG（自作デコーダが非対応。8bit 非インターレース PNG とベースライン JPEG のみ） | 中 |

### 検証・品質負債

| # | 項目 | 規模 |
|---|---|---|
| B15 | 素の Linux 実機（native ext4）での実行検証（現状 WSL2 Xvfb 代替のみ） | 小 |
| B16 | Win32/X11 可視ウィンドウの最終目視確認（ユーザー実機作業）: DPI 拡大の初期サイズ・リサイズ/レターボックス・F11/Alt+Enter・HALFTONE 拡縮時のフレーム落ち・実クリック位置 | 小 |
| B17 | 実音声デバイス再生の手動確認（自動化対象外として切り出し済み） | 小 |

## vp9dec 連携（参照）

- 動画シーム（`codec/video.rs`）は VP9 統合済み: `open_video_file()` が
  `DKIF`+`VP90` を検出して `Vp9Ivf`（自作デコーダ）を MF より優先。
- HD 実用（1080p30）には vp9dec 側の SIMD 最適化（vp9dec backlog P1、進行中）が前提。
  完了後、エンジン内で HD seek・区間ループの性能と実再生品質を検証する。
