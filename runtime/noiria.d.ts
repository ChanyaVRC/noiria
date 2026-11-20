// Noiria エンジン API 型定義 (ゲーム作者のエディタ補完用)
// tsconfig.json の paths で "noiria:*" をこのファイルに向けると補完が効きます。

declare module "noiria:api" {
  export type Page = "fore" | "back";
  export type RGBA = [number, number, number, number];
  export type TextAlign = "left" | "center" | "right";
  export type VideoEventKind = "loop" | "period" | "prepare" | "segLoop";
  /** "bgm" | "bgm2" | "voice" | "se0".."se15"（"se" は "se0" の別名）。SE バッファが動的なため string。 */
  export type Channel = string;

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
    accel?: number;
  }

  /** [image]（W5 拡張）の clip=/flipud=/fliplr=/grayscale= 属性。 */
  export interface ImageLoadOpts {
    clip?: { x: number; y: number; w: number; h: number };
    flipH?: boolean;
    flipV?: boolean;
    grayscale?: boolean;
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
    inlineImage?: string;
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
    kind: "click" | "move" | "wheel" | "key" | "keyup" | "quit";
    x: number;
    y: number;
    button?: "left" | "right" | "middle" | "other";
    key?: string;
    delta?: number;
    t: number;
  }

  export interface TransArgs {
    method: "crossfade" | "universal" | "scroll";
    timeMs: number;
    rule?: string;
    vague?: number;
    from?: "left" | "right" | "top" | "bottom";
    stay?: "stayfore" | "staynone" | "staybase";
  }

  /** `layer.panel` の指定（モダン UI）。色は straight RGBA。 */
  export interface PanelSpec {
    w: number;
    h: number;
    radius?: number;
    top: RGBA;
    bottom?: RGBA;
    horizontal?: boolean;
    border?: RGBA;
    borderW?: number;
    shadow?: { blur: number; dy: number; alpha: number };
    /** 0 より大きければグラデーションを先頭の ramp px だけで変え、以降は bottom の単色。 */
    ramp?: number;
    pad?: number;
  }

  export const layer: {
    create(kind?: "image" | "text"): number;
    free(id: number): void;
    loadImage(id: number, path: string, page?: Page): [number, number];
    /** セーブ領域の `<slot>.png`（サムネイル）を読む。無ければ例外。 */
    loadSaveThumb(id: number, slot: string, page?: Page, w?: number, h?: number): [number, number];
    /** 画像を読み、面積平均でちょうど w x h へ拡縮してレイヤへ設定する。 */
    loadImageScaled(id: number, path: string, w: number, h: number, page?: Page): [number, number];
    /** グラデーション・角丸・枠線・影つきのパネルをレイヤの絵にする（モダン UI）。 */
    panel(id: number, spec: PanelSpec, page?: Page): void;
    loadImageEx(id: number, path: string, opts: ImageLoadOpts, page?: Page): [number, number];
    clearImage(id: number, page?: Page): void;
    fill(id: number, w: number, h: number, rgba: RGBA, page?: Page): void;
    msgbox(id: number, w: number, h: number, radius: number, rgba: RGBA, page?: Page): void;
    set(id: number, props: LayerProps, page?: Page): void;
    backlay(id?: number): void;
    copy(srcId: number, srcPage: Page, dstId: number, dstPage: Page, copyPosition?: boolean): void;
    blitImage(
      id: number,
      path: string,
      page?: Page,
      dx?: number,
      dy?: number,
      clip?: { x: number; y: number; w: number; h: number },
    ): void;
    move(id: number, args: MoveArgs, page?: Page): void;
    /** 画像の「見える幅」（左端から何画素ぶん合成するか。テキストには効かない）を即座に決める。省略は全幅。 */
    setCrop(id: number, w: number, page?: Page): void;
    /** 画像の「見える幅」を今の値から w へ tween する。位置/不透明度の `move` とは独立。 */
    cropTo(id: number, w: number, timeMs: number, accel?: number, page?: Page): void;
    cancelMove(id: number, page?: Page): void;
    movePath(id: number, points: MoveArgs[], page?: Page, spline?: boolean): void;
    movePathByDistance(
      id: number,
      points: MoveArgs[],
      totalTimeMs: number,
      page?: Page,
      spline?: boolean,
    ): void;
    moveSkip(): void;
    moving(): boolean;
  };

  export const text: {
    style(id: number, style: TextStyle): void;
    print(id: number, segments: Segment[], append: boolean, cps: number): {
      total: number;
      added: number;
    };
    reflow(id: number): void;
    complete(id: number): void;
    isDone(id: number): boolean;
    clear(id: number): void;
    rects(id: number): GlyphRect[];
  };

  export const trans: {
    begin(args: TransArgs): void;
    skip(): void;
    wait(): Promise<void>;
  };

  export const audio: {
    play(
      ch: Channel,
      path: string,
      opts?: {
        loop?: boolean;
        volume?: number;
        fadeInMs?: number;
        startMs?: number;
        paused?: boolean;
      },
    ): void;
    stop(ch: Channel, fadeOutMs?: number): void;
    volume(ch: Channel, v: number): void;
    isPlaying(ch: Channel): boolean;
    position(ch: Channel): number;
    waitEnd(ch: Channel): Promise<void>;
    pause(ch: Channel): void;
    resume(ch: Channel): void;
    fadeTo(ch: Channel, v: number, ms: number, then?: "none" | "stop" | "pause"): void;
    isFading(ch: Channel): boolean;
    isFinished(ch: Channel): boolean;
  };

  export const sys: {
    log(msg: unknown, level?: "info" | "warn" | "error" | "debug"): void;
    now(): number;
    wait(ms: number): Promise<void>;
    quit(): void;
    title(t: string): void;
    readText(rel: string): string;
    exists(rel: string): boolean;
    saveWrite(slot: string, json: string): void;
    saveRead(slot: string): string | null;
    saveList(): { slot: string; json: string }[];
    saveDelete(slot: string): void;
    screenshot(slot: string): void;
    /** 今の画面を 240x135 へ縮小して保持する（メニューを開く直前に呼ぶ）。 */
    thumbCapture(w?: number, h?: number): void;
    /** セーブ一覧のサムネイルを w x h の寸法で別スレッドに先読みする（初回に開くときの復号待ちを避ける）。 */
    prefetchSaveThumbs(w: number, h: number): void;
    /** ゲーム資産の画像を w x h の縮小サムネイルにして別スレッドに先読みする（ギャラリーの初回表示用）。 */
    prefetchImagesScaled(paths: string[], w: number, h: number): void;
    /** 保持中のサムネイルを `<slot>.png` へ書く。 */
    thumbSave(slot: string): void;
    /** 保存日時の表示用文字列（`YYYY/MM/DD HH:MM`）。 */
    dateString(): string;
    quake(ax: number, ay: number, ms: number): void;
    quakeStop(): void;
  };

  export const video: {
    play(layerId: number, path: string, loop?: boolean): void;
    playAt(
      layerId: number,
      path: string,
      loop: boolean,
      frame: number | null,
      positionMs: number | null,
    ): void;
    stop(): void;
    isPlaying(): boolean;
    waitEnd(): Promise<void>;
    /** [openvideo]（W6）: 動画を開くだけで再生は開始しない。 */
    open(layerId: number, path: string, loop?: boolean): void;
    /** [preparevideo]（W6/B4）: open 済み動画の現在の開始フレームを表示する。 */
    prepare(layerId: number, path: string, loop?: boolean): void;
    prepareOpened(layerId: number): void;
    /** [playvideo]（storage= 省略時、W6）: open/prepare 済みの動画の再生を開始する。 */
    playPrepared(layerId: number): void;
    /** [pausevideo]（W6）: 再生中の動画をその場で一時停止する。 */
    pause(): void;
    /** [resumevideo]（W6）: 一時停止からの再開。 */
    resume(): void;
    /** [rewindvideo]（W6）: 先頭へ巻き戻す。 */
    rewind(): void;
    /** 一時停止状態か。 */
    isPaused(): boolean;
    seekFrame(target: number): void;
    seekPosition(positionMs: number): void;
    setPeriodEvent(frame: number): void;
    cancelPeriodEvent(): void;
    setSegmentLoop(start: number, end: number): void;
    cancelSegmentLoop(): void;
    takeEvent(kind: VideoEventKind | "any"): boolean;
    generation(): number;
  };

  export const input: {
    on(handler: (ev: InputEvent) => void): () => void;
    /** ウィンドウを閉じる操作（key:"WindowClose"）の処理を登録する。未登録なら sys.quit()。 */
    onWindowClose(handler: () => void): void;
    waitFor(pred: (ev: InputEvent) => boolean): Promise<InputEvent>;
  };

  export function waitAdvance(): Promise<InputEvent>;
  export function waitClick(): Promise<InputEvent>;
}

declare module "noiria:kag" {
  export interface MessageConfig {
    left: number;
    top: number;
    width: number;
    height: number;
    radius: number;
    color: [number, number, number, number];
    fontSize: number;
    marginX: number;
    marginY: number;
    /** 右/下の余白（未指定は marginX/marginY と同じ）。 */
    marginR?: number;
    marginB?: number;
    /** 窓サイズ一致の背景画像（`/` を含まなければ assets/fg/ 配下）。空/未指定でなし。 */
    frame?: string;
  }

  export type SysButtonId = "qsave" | "qload" | "save" | "load" | "auto" | "skip" | "log" | "menu";

  export interface SystemButtonsOptions {
    items?: SysButtonId[];
    color?: [number, number, number, number];
    textColor?: [number, number, number, number];
  }

  export interface KagOptions {
    scenarioDir?: string;
    charaLayerCount?: number;
    message?: Partial<MessageConfig>;
    linkColor?: [number, number, number, number];
    autoWaitMs?: number;
    /** メッセージ窓の直上・右寄せに常設するシステムボタン列（既定 off）。 */
    systemButtons?: boolean | SystemButtonsOptions;
    /** タイトルの位置。指定するとシステムメニューに「タイトルへ戻る」が出る。 */
    title?: { storage: string; target?: string };
    /** 名前欄プレートの色（color=背景、textColor=文字）。 */
    namePlate?: { color?: [number, number, number, number]; textColor?: [number, number, number, number] };
    /** UI 効果音（storage 名。`/` を含まなければ assets/se/ 配下）。 */
    sound?: { hover?: string; click?: string };
  }

  /** バックログの 1 件（段落ごと）。 */
  export interface BacklogEntry {
    text: string;
    name: string | null;
    voice: string | null;
    rule?: boolean;
  }

  /** 設定メニューの値（音量・cps・オート待ち）。sf.config に永続化される。 */
  export interface SettingsConfig {
    /** BGM 音量 (0..100, 10刻み, 既定 80) */
    bgmVol: number;
    /** SE 音量 (0..100, 10刻み, 既定 80) */
    seVol: number;
    /** ボイス音量 (0..100, 10刻み, 既定 80) */
    voiceVol: number;
    /** 文字送り速度 (cps)。20=遅い/35=普通/60=速い/0=瞬間、既定 35 */
    cps: number;
    /** オート送りの待ち時間 (ms)。800=短/1800=普通/3200=長、既定 1800 */
    autoWaitMs: number;
    /** 未読区間でもスキップを止めない（既定 false） */
    skipUnread: boolean;
    /** メッセージ窓の濃さ (0..100, 10刻み, 既定 100) */
    msgOpacity: number;
  }

  export class KagPlayer {
    constructor(opts?: KagOptions);
    run(startFile: string, startLabel?: string): Promise<void>;
    /** スロットへ保存（成功なら true）。サムネイル・保存時刻・本文の抜粋・履歴も保存される。 */
    save(slot: string): boolean;
    load(slot: string): Promise<boolean>;
    /** Escape でセーブ/ロード/設定メニューを開く（mode 省略時は "save"） */
    openSaveMenu(initialMode?: "save" | "load" | "settings"): Promise<void>;
    /** F5 / F9 と同じ（トースト付き）。 */
    quickSave(): void;
    quickLoad(): Promise<void>;
    /** 画面上部に約 1.2 秒の通知を出す。 */
    toast(text: string): void;
    mode: { skip: boolean; auto: boolean };
    cps: number;
    backlog: BacklogEntry[];
    /** 設定メニューの現在値。init() 時に sf.config から読み込まれる */
    settings: SettingsConfig;
  }

  export function parseScenario(
    source: string,
    file: string,
  ): {
    file: string;
    commands: unknown[];
    labels: Map<string, number>;
  };
}
