// noiria:api — ネイティブ ops の高級ラッパ。
// krkrZ における TJS2 のグローバル API 相当。ゲームスクリプトと KAG プレイヤーの両方が使う。

// deno_core が公開する ops テーブル
const ops = (globalThis as any).Deno.core.ops;

export type Page = "fore" | "back";
export type RGBA = [number, number, number, number];
export type TextAlign = "left" | "center" | "right";
export type VideoEventKind = "loop" | "period" | "prepare" | "segLoop";

export interface LayerProps {
  x?: number;
  y?: number;
  z?: number;
  opacity?: number;
  visible?: boolean;
}

export interface MoveArgs {
  toX?: number;
  toY?: number;
  toOpacity?: number;
  timeMs: number;
  /// KAG 準拠: 0=線形, >0=加速 (ease-in), <0=減速 (ease-out)。省略時 0。
  accel?: number;
}

/// [image]（W5 拡張）の clip=/flipud=/fliplr=/grayscale= 属性。
export interface ImageLoadOpts {
  clip?: { x: number; y: number; w: number; h: number };
  flipH?: boolean;
  flipV?: boolean;
  grayscale?: boolean;
}

/// `layer.panel` の指定（モダン UI）。色は straight RGBA。省略した項目は「なし」。
export interface PanelSpec {
  w: number;
  h: number;
  radius?: number;
  /// 塗りの上端色（`horizontal` なら左端色）。
  top: RGBA;
  /// 塗りの下端色（省略は top の単色）。
  bottom?: RGBA;
  horizontal?: boolean;
  border?: RGBA;
  borderW?: number;
  /// ソフトシャドウ。pad（パネル外側の余白）に収まる大きさにすること。
  shadow?: { blur: number; dy: number; alpha: number };
  /// 0 より大きければ、グラデーションを先頭の ramp px だけで（滑らかに）top→bottom へ変え、以降は bottom の単色にする。
  ramp?: number;
  /// 出力画像はパネル矩形の外側へ pad px ずつ広がる（レイヤ位置を pad だけ引くのは呼び出し側）。
  pad?: number;
}

export interface TextStyle {
  fontFamily?: string;
  fontSize?: number;
  lineHeight?: number;
  color?: RGBA;
  outlineColor?: RGBA;
  outlineWidth?: number;
  boxW?: number;
  boxH?: number;
  marginX?: number;
  marginY?: number;
  align?: TextAlign;
}

export interface Segment {
  text: string;
  color: RGBA | null;
  size?: number;
  bold?: boolean;
  edgeColor?: RGBA;
  ruby?: string;
  locate?: { x: number; y?: number };
  indent?: boolean;
  /// [graph storage=...]（W3）: 解決済みパス。1 グリフ扱いのインライン画像として挿入する。
  inlineImage?: string;
  /// [font face=]: このセグメントだけ基準フォントを差し替える family 名。
  fontFamily?: string;
}

export interface GlyphRect {
  x: number;
  y: number;
  w: number;
  h: number;
  byte_start: number;
  byte_end: number;
  line_i: number;
}

export interface InputEvent {
  /// key: キーダウン（オートリピートは届かない）。keyup: キーアップ（key に同じ名前）。
  /// ウィンドウを閉じる操作は kind:"key", key:"WindowClose" で届く（InputHub が先に処理する）。
  kind: "click" | "move" | "wheel" | "key" | "keyup" | "quit";
  x: number;
  y: number;
  button?: "left" | "right" | "middle" | "other";
  key?: string;
  delta?: number;
  t: number;
}

// ---- レイヤ ----

export const layer = {
  create(kind: "image" | "text" = "image"): number {
    return ops.op_layer_new(kind);
  },
  free(id: number): void {
    ops.op_layer_free(id);
  },
  loadImage(id: number, path: string, page: Page = "fore"): [number, number] {
    return ops.op_layer_load_image(id, page, path);
  },
  /// [image]（W5 拡張）: clip=/flipud=/fliplr=/grayscale= を適用してから読み込む
  /// （`layer_load_image` の拡張版、既存内容を消して置き換える点は同じ）。
  loadImageEx(id: number, path: string, opts: ImageLoadOpts, page: Page = "fore"): [number, number] {
    return ops.op_layer_load_image_ex(id, page, path, {
      clip: opts.clip ?? null,
      flipH: opts.flipH ?? false,
      flipV: opts.flipV ?? false,
      grayscale: opts.grayscale ?? false,
    });
  },
  clearImage(id: number, page: Page = "fore"): void {
    ops.op_layer_clear_image(id, page);
  },
  fill(id: number, w: number, h: number, rgba: RGBA, page: Page = "fore"): void {
    ops.op_layer_fill(id, page, w, h, rgba[0], rgba[1], rgba[2], rgba[3]);
  },
  msgbox(id: number, w: number, h: number, radius: number, rgba: RGBA, page: Page = "fore"): void {
    ops.op_layer_msgbox(id, page, w, h, radius, rgba[0], rgba[1], rgba[2], rgba[3]);
  },
  set(id: number, props: LayerProps, page: Page = "fore"): void {
    ops.op_layer_set_props(id, page, props);
  },
  /// fore の内容を back へコピー (KAG の backlay)。id 省略で全レイヤ。
  backlay(id?: number): void {
    ops.op_backlay(id === undefined ? -1 : id);
  },
  /// セーブ領域の `<slot>.png`（セーブ時に書いたサムネイル）をレイヤへ読み込む。`GameDir`
  /// （ゲームの資産）とは別の経路で、slot は英数字/`_`/`-` のみ。規定寸法（240x135）より大きい
  /// 旧版の画面サイズ PNG は縮小して読む。無い/壊れていれば例外を投げる。
  /// w/h を指定すると（保存寸法に関わらず）ちょうどその寸法へ拡縮して読む。省略は規定寸法に収める。
  loadSaveThumb(id: number, slot: string, page: Page = "fore", w = 0, h = 0): [number, number] {
    return ops.op_layer_load_save_thumb(id, page, slot, w, h);
  },
  /// 画像を読み、面積平均でちょうど w x h へ拡縮してレイヤへ設定する（ギャラリーのサムネイル）。
  loadImageScaled(id: number, path: string, w: number, h: number, page: Page = "fore"): [number, number] {
    return ops.op_layer_load_image_scaled(id, page, path, w, h);
  },
  /// グラデーション・角丸・枠線・ソフトシャドウつきのパネルをレイヤの絵にする（モダン UI）。
  panel(id: number, spec: PanelSpec, page: Page = "fore"): void {
    const bottom = spec.bottom ?? spec.top;
    const border = spec.border ?? [0, 0, 0, 0];
    const sh = spec.shadow;
    ops.op_layer_panel(
      id,
      page,
      [
        spec.w,
        spec.h,
        spec.radius ?? 0,
        spec.top[0],
        spec.top[1],
        spec.top[2],
        spec.top[3],
        bottom[0],
        bottom[1],
        bottom[2],
        bottom[3],
        border[0],
        border[1],
        border[2],
        border[3],
        spec.borderW ?? 0,
        sh ? sh.blur : 0,
        sh ? sh.dy : 0,
        sh ? sh.alpha : 0,
        spec.pad ?? 0,
        spec.horizontal ? 1 : 0,
        spec.ramp ?? 0,
      ],
    );
  },
  /// 別レイヤ間で内容をコピーする (KAG の [copylay]、W3。W5 で copyPosition を追加)。
  /// copyPosition=false（既定 true）はコピー先の x/y を変えない（メッセージレイヤ宛の
  /// 既知バグ修正、`docs/implementation-notes.md`「2026-07-11 W5」参照）。
  copy(srcId: number, srcPage: Page, dstId: number, dstPage: Page, copyPosition = true): void {
    ops.op_layer_copy(srcId, srcPage, dstId, dstPage, copyPosition);
  },
  /// 画像ファイルの部分矩形 (clip 省略で全体) を既存レイヤへ (dx, dy) を左上として
  /// 上書き合成する ([button] の 3 状態画像切替、W5 [pimage] と共用の汎用 op)。
  blitImage(
    id: number,
    path: string,
    page: Page = "fore",
    dx = 0,
    dy = 0,
    clip?: { x: number; y: number; w: number; h: number },
  ): void {
    ops.op_layer_blit_image(id, page, path, dx, dy, clip ?? null);
  },
  /// KAG の [move]: 現在値から to へ位置/不透明度をアニメーションする。
  /// 省略したフィールドは現在値を維持する（動かさない）。
  move(id: number, args: MoveArgs, page: Page = "fore"): void {
    ops.op_layer_move(id, page, args);
  },
  /// 画像の「見える幅」（左端から何画素ぶん合成するか。テキストには効かない）を即座に決める。
  /// 省略（未設定）は全幅。進行中の `cropTo` は止まる。
  setCrop(id: number, w: number, page: Page = "fore"): void {
    ops.op_layer_set_crop(id, page, w);
  },
  /// 画像の「見える幅」を今の値から w へ tween する。
  /// 位置/不透明度の `move` とは独立に動く（アンダーラインが左から伸びる等）。
  cropTo(id: number, w: number, timeMs: number, accel = 0, page: Page = "fore"): void {
    ops.op_layer_crop_move(id, page, w, timeMs, accel);
  },
  cancelMove(id: number, page: Page = "fore"): void {
    ops.op_layer_move_cancel(id, page);
  },
  /// KAG の [move path=]（W5/B12）: 複数区間の move。points は区間ごとの MoveArgs。
  /// 各区間の toX/toY/toOpacity を省略すると直前の区間（先頭は現在値）を引き継ぐ。
  movePath(id: number, points: MoveArgs[], page: Page = "fore", spline = false): void {
    ops.op_layer_move_path(id, page, points, spline);
  },
  movePathByDistance(
    id: number,
    points: MoveArgs[],
    totalTimeMs: number,
    page: Page = "fore",
    spline = false,
  ): void {
    ops.op_layer_move_path_by_distance(id, page, points, spline, totalTimeMs);
  },
  /// 全レイヤの move を即座に完了させる（スキップモード用）。[stopmove]（W5）も使う。
  moveSkip(): void {
    ops.op_layer_move_skip();
  },
  /// いずれかのレイヤで move が進行中か。
  moving(): boolean {
    return ops.op_layer_moving();
  },
};

// ---- テキスト ----

export const text = {
  style(id: number, style: TextStyle): void {
    ops.op_text_style(id, style);
  },
  /// 戻り値: { total, added } グリフ数。
  print(
    id: number,
    segments: Segment[],
    append: boolean,
    cps: number,
  ): { total: number; added: number } {
    return ops.op_text_print(id, segments, append, cps);
  },
  reflow(id: number): void {
    ops.op_text_reflow(id);
  },
  complete(id: number): void {
    ops.op_text_complete(id);
  },
  isDone(id: number): boolean {
    return ops.op_text_is_done(id);
  },
  clear(id: number): void {
    ops.op_text_clear(id);
  },
  rects(id: number): GlyphRect[] {
    return ops.op_text_rects(id);
  },
};

// ---- トランジション ----

export interface TransArgs {
  method: "crossfade" | "universal" | "scroll";
  timeMs: number;
  rule?: string;
  vague?: number;
  /// [trans method=scroll]（W5）: 新ページが入ってくる辺。省略時 left。
  from?: "left" | "right" | "top" | "bottom";
  /// [trans method=scroll]（W5）: stayfore|staynone|staybase を受理するが、このエンジンの
  /// 合成（画面全面を覆う不透明フレーム同士のブレンド）では描画上区別しない
  /// （`docs/kag-tag-compat.md`「trans」備考参照）。
  stay?: "stayfore" | "staynone" | "staybase";
}

export const trans = {
  begin(args: TransArgs): void {
    ops.op_trans_begin(args);
  },
  skip(): void {
    ops.op_trans_skip();
  },
  wait(): Promise<void> {
    return ops.op_trans_wait();
  },
};

// ---- オーディオ ----

/** チャンネル名: "bgm" | "bgm2" | "voice" | "se0".."se15"（"se" は "se0" の別名）。
 * SE バッファが動的なため string に緩めている（不正な名前はネイティブ側でエラー）。 */
export type Channel = string;

export const audio = {
  play(
    ch: Channel,
    path: string,
    opts: {
      loop?: boolean;
      volume?: number;
      fadeInMs?: number;
      startMs?: number;
      paused?: boolean;
    } = {},
  ): void {
    ops.op_audio_play(
      ch,
      path,
      opts.loop ?? false,
      opts.volume ?? 1.0,
      opts.fadeInMs ?? 0,
      opts.startMs ?? 0,
      opts.paused ?? false,
    );
  },
  stop(ch: Channel, fadeOutMs = 0): void {
    ops.op_audio_stop(ch, fadeOutMs);
  },
  volume(ch: Channel, v: number): void {
    ops.op_audio_volume(ch, v);
  },
  isPlaying(ch: Channel): boolean {
    return ops.op_audio_is_playing(ch);
  },
  position(ch: Channel): number {
    return ops.op_audio_position(ch);
  },
  waitEnd(ch: Channel): Promise<void> {
    return ops.op_audio_wait_end(ch);
  },
  /** 一時停止（再生位置を保持したまま無音化）。 */
  pause(ch: Channel): void {
    ops.op_audio_pause(ch);
  },
  /** 一時停止からの再開。 */
  resume(ch: Channel): void {
    ops.op_audio_resume(ch);
  },
  /** 現在音量から v へ ms かけて線形フェードし、完了時に then を適用する。 */
  fadeTo(ch: Channel, v: number, ms: number, then: "none" | "stop" | "pause" = "none"): void {
    ops.op_audio_fade_to(ch, v, ms, then);
  },
  /** フェード進行中か。 */
  isFading(ch: Channel): boolean {
    return ops.op_audio_is_fading(ch);
  },
  /** 非ループ再生が自然終端したか（明示 stop では立たない）。 */
  isFinished(ch: Channel): boolean {
    return ops.op_audio_is_finished(ch);
  },
};

// ---- システム ----

export const sys = {
  log(msg: unknown, level: "info" | "warn" | "error" | "debug" = "info"): void {
    ops.op_log(level, String(msg));
  },
  now(): number {
    return ops.op_now();
  },
  wait(ms: number): Promise<void> {
    return ops.op_wait(ms);
  },
  quit(): void {
    ops.op_quit();
  },
  title(t: string): void {
    ops.op_set_title(t);
  },
  readText(rel: string): string {
    return ops.op_read_text(rel);
  },
  exists(rel: string): boolean {
    return ops.op_file_exists(rel);
  },
  saveWrite(slot: string, json: string): void {
    ops.op_save_write(slot, json);
  },
  saveRead(slot: string): string | null {
    return ops.op_save_read(slot);
  },
  saveList(): { slot: string; json: string }[] {
    return ops.op_save_list();
  },
  /** スロットを真に削除する（[erasebookmark] の KAG3 準拠実装、W7）。存在しなければ no-op。 */
  saveDelete(slot: string): void {
    ops.op_save_delete(slot);
  },
  screenshot(slot: string): void {
    ops.op_screenshot(slot);
  },
  /// 今の画面を規定寸法（240x135）へ縮小して保持する（ファイルへは書かない）。メニューを開く
  /// 直前に呼び、メニュー上のセーブにメニューが写り込まないようにする。
  /// w/h 省略（0）は規定寸法 240x135。モダン UI は大きいカード用に大きく撮る。
  thumbCapture(w = 0, h = 0): void {
    ops.op_thumb_capture(w, h);
  },
  /// セーブ一覧のサムネイル（`auto`/`quick`/`saveN` の PNG）を w x h の寸法で別スレッドに先読みする。
  /// 初回にメニューを開くときの復号待ちを、タイトル画面などの待ち時間へ逃がす（表示には影響しない）。
  prefetchSaveThumbs(w: number, h: number): void {
    ops.op_prefetch_save_thumbs(w, h);
  },
  /// ゲーム資産の画像を w x h の縮小サムネイル（`layer.loadImageScaled` と同じ結果）にして別スレッドに
  /// 先読みする（ギャラリーを初めて開くときの復号待ちを避ける。表示には影響しない）。
  prefetchImagesScaled(paths: string[], w: number, h: number): void {
    ops.op_prefetch_images_scaled(paths.join("\n"), w, h);
  },
  /// 保持中のサムネイルを `<slot>.png` へ書く（未取得なら今の画面を取ってから書く）。
  thumbSave(slot: string): void {
    ops.op_thumb_save(slot);
  },
  /// 保存日時の表示用文字列（ローカル時刻 `YYYY/MM/DD HH:MM`。ヘッドレスは固定値）。
  dateString(): string {
    return ops.op_wall_clock_string();
  },
  quake(ax: number, ay: number, ms: number): void {
    ops.op_quake(ax, ay, ms);
  },
  /// [stopquake]（W5）: 画面揺れを即座に停止する。
  quakeStop(): void {
    ops.op_quake_stop();
  },
};

// ---- 動画 ----

export const video = {
  play(layerId: number, path: string, loop = false): void {
    ops.op_video_play(layerId, path, loop);
  },
  playAt(
    layerId: number,
    path: string,
    loop: boolean,
    frame: number | null,
    positionMs: number | null,
  ): void {
    ops.op_video_play_at(layerId, path, loop, frame === null ? -1 : frame, positionMs === null ? -1 : positionMs);
  },
  stop(): void {
    ops.op_video_stop();
  },
  isPlaying(): boolean {
    return ops.op_video_is_playing();
  },
  async waitEnd(): Promise<void> {
    while (ops.op_video_is_playing()) await sys.wait(60);
  },
  /// [openvideo]（W6）: 動画を開くだけで再生は開始しない。
  open(layerId: number, path: string, loop = false): void {
    ops.op_video_open(layerId, path, loop);
  },
  /// [preparevideo]（W6/B4）: open 済み動画の現在の開始フレームを表示する。
  prepare(layerId: number, path: string, loop = false): void {
    ops.op_video_prepare(layerId, path, loop);
  },
  prepareOpened(layerId: number): void {
    ops.op_video_prepare_opened(layerId);
  },
  /// [playvideo]（storage= 省略時、W6）: open/prepare 済みの動画の再生を開始する。
  playPrepared(layerId: number): void {
    ops.op_video_play_prepared(layerId);
  },
  /// [pausevideo]（W6）: 再生中の動画をその場で一時停止する。
  pause(): void {
    ops.op_video_pause();
  },
  /// [resumevideo]（W6）: 一時停止からの再開。
  resume(): void {
    ops.op_video_resume();
  },
  /// [rewindvideo]（W6）: 先頭へ巻き戻す。
  rewind(): void {
    ops.op_video_rewind();
  },
  /// 一時停止状態か。
  isPaused(): boolean {
    return ops.op_video_is_paused();
  },
  seekFrame(target: number): void {
    ops.op_video_seek_frame(target);
  },
  seekPosition(positionMs: number): void {
    ops.op_video_seek_position(positionMs);
  },
  setPeriodEvent(frame: number): void {
    ops.op_video_set_period_event(frame);
  },
  cancelPeriodEvent(): void {
    ops.op_video_cancel_period_event();
  },
  setSegmentLoop(start: number, end: number): void {
    ops.op_video_set_segment_loop(start, end);
  },
  cancelSegmentLoop(): void {
    ops.op_video_cancel_segment_loop();
  },
  takeEvent(kind: VideoEventKind | "any"): boolean {
    return ops.op_video_take_event(kind);
  },
  generation(): number {
    return ops.op_video_generation();
  },
};

// ---- 入力 ----
// ネイティブは単一のイベントポンプだけを提供し、待ち合わせはここで多重化する。

type Handler = (ev: InputEvent) => void;

class InputHub {
  private handlers = new Set<Handler>();
  private closeHandler: (() => void) | null = null;
  private started = false;
  private quitted = false;

  private start(): void {
    if (this.started) return;
    this.started = true;
    this.pump();
  }

  private async pump(): Promise<void> {
    while (true) {
      const ev: InputEvent = await ops.op_input_next_event();
      if (ev.kind === "quit") {
        this.quitted = true;
        break;
      }
      // WindowClose は通常ハンドラへ流さず、状態（メニュー表示中・[wait] 中・トランジション待ち等）に
      // 関係なくここで即処理する。ハンドラ未登録（KagPlayer を使わないゲーム）なら素直に終了する。
      if (ev.kind === "key" && ev.key === "WindowClose") {
        if (this.closeHandler) this.closeHandler();
        else sys.quit();
        continue;
      }
      for (const h of [...this.handlers]) h(ev);
    }
  }

  /// ウィンドウを閉じる操作の処理を登録する（1 つだけ。保存などをしてから sys.quit() を呼ぶ想定）。
  onWindowClose(handler: () => void): void {
    this.start();
    this.closeHandler = handler;
  }

  on(handler: Handler): () => void {
    this.start();
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }

  waitFor(pred: (ev: InputEvent) => boolean): Promise<InputEvent> {
    return new Promise((resolve) => {
      const off = this.on((ev) => {
        if (pred(ev)) {
          off();
          resolve(ev);
        }
      });
    });
  }
}

export const input = new InputHub();

/// 左クリックまたは Enter/Space を待つ（ノベルの「送り」操作）。
export function waitAdvance(): Promise<InputEvent> {
  return input.waitFor(
    (e) =>
      (e.kind === "click" && e.button === "left") ||
      (e.kind === "key" && (e.key === "Enter" || e.key === " ")),
  );
}

export function waitClick(): Promise<InputEvent> {
  return input.waitFor((e) => e.kind === "click" && e.button === "left");
}
