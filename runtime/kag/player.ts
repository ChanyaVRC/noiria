// noiria:kag/player — KAG3 風シナリオプレイヤー。
// krkrZ の KAG3 (Conductor/MainWindow) に相当する実行ループとゲームプレイポリシーを持つ。

import * as api from "noiria:api";
import { parseScenario, Scenario, Command, Attrs } from "noiria:kag/parser";
import * as vars from "noiria:kag/vars";
import { makeTags, TagHandler } from "noiria:kag/tags";
import { ButtonManager, applyEditKey } from "noiria:kag/buttons";
import { AnimManager } from "noiria:kag/anim";
import { SystemMenu, makeExcerpt } from "noiria:kag/menu";
import type { MenuHost, SettingKey } from "noiria:kag/menu";
import { UiBox, UiEvents, UI_COLORS, SystemBar, Toast, confirmDialog, SYS_BUTTON_ORDER } from "noiria:kag/ui";
import type { SysButtonId } from "noiria:kag/ui";
import {
  ModernNameTag,
  ModernSystemBar,
  ModernGlyph,
  ModernToast,
  ModernModeBadge,
  ChoiceScreen,
  modernConfirm,
  BAND_H,
  COL_W,
  BAND_FONT,
  BAND_TEXT_TOP,
  BAND_TEXT_BOTTOM,
  bandPanel,
  setGlyphPaused,
} from "noiria:kag/mhud";
import { cleanChoiceLabel, MC } from "noiria:kag/mui";
import { TitleScreen, GalleryScreen, prefetchGalleryThumbs } from "noiria:kag/mtitle";
import type { TitleScreenOptions, GalleryEntry, TitleHost } from "noiria:kag/mtitle";
import { ModernSystemMenu, ModernBacklog, prefetchSlotThumbs } from "noiria:kag/mmenu";

function utf8Len(s: string): number {
  let n = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s.codePointAt(i)!;
    if (c > 0xffff) i++;
    n += c <= 0x7f ? 1 : c <= 0x7ff ? 2 : c <= 0xffff ? 3 : 4;
  }
  return n;
}

/** Unicode コードポイント数（サロゲートペアを 1 文字と数える）。tsint に無い Array.from は使わない。 */
function codePointCount(s: string): number {
  let n = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s.codePointAt(i)!;
    if (c > 0xffff) i++;
    n++;
  }
  return n;
}

/**
 * `"0xRRGGBB"` 形式の 16 進数カラー文字列を RGBA（アルファ 255 固定）にパースする。
 * `Number("0x...")` の hex 文字列パースと `>>`/`&` ビット演算は tsint 側で対応済みと
 * 実装前に確認済み（`docs/implementation-notes.md` 参照）なので、手書き hex パーサへの
 * 切替は不要だった。パース不能（`v` が undefined、または `Number()` が NaN を返す）なら
 * `dflt` を返す。
 */
function parseHexColor(v: string | undefined, dflt?: api.RGBA): api.RGBA | undefined {
  if (v === undefined) return dflt;
  const n = Number(v);
  if (!Number.isFinite(n)) return dflt;
  const r = (n >> 16) & 0xff;
  const g = (n >> 8) & 0xff;
  const b = n & 0xff;
  return [r, g, b, 255];
}

/** 設定メニューで変更・永続化する値。sf.config に JSON でそのまま入る。 */
export interface SettingsConfig {
  bgmVol: number;
  seVol: number;
  voiceVol: number;
  cps: number;
  autoWaitMs: number;
  /** true なら既読スキップ中も未読区間で止まらない。 */
  skipUnread: boolean;
  /** メッセージ窓の濃さ (0..100, 10刻み, 既定 100)。窓色のアルファに掛ける割合。 */
  msgOpacity: number;
}

/** 文字送り速度 (cps) の 4 段階。UI 表示順（遅い→普通→速い→瞬間）。 */
export const CPS_STAGES: number[] = [20, 35, 60, 0];
export const CPS_LABELS: string[] = ["遅い", "普通", "速い", "瞬間"];

/** オート送り待ち時間 (ms) の 3 段階。 */
export const AUTO_STAGES: number[] = [800, 1800, 3200];
export const AUTO_LABELS: string[] = ["短", "普通", "長"];

const DEFAULT_SETTINGS: SettingsConfig = {
  bgmVol: 80,
  seVol: 80,
  voiceVol: 80,
  cps: 35,
  autoWaitMs: 1800,
  skipUnread: false,
  msgOpacity: 100,
};

/** 実効音量 = タグの volume (0..1) × 設定の音量割合 (0..100)。純関数（単体テスト対象）。 */
export function effectiveVolume(tagVolume: number, pct: number): number {
  return tagVolume * (pct / 100);
}

/** オート送りの待ち時間（全文を表示し終えてから数える）: 基本待ち（設定の 3 段階）+ 文字数 × 係数。係数は基本待ちの
 * 1/40（短 20ms / 普通 45ms / 長 80ms）で、加算分は基本待ちの 2 倍までに抑える。固定だと、長い台詞は読み終える前に、
 * 短い台詞は読み終えてからも長く待つことになる。純関数。 */
export function autoWaitFor(base: number, chars: number): number {
  return base + Math.min(chars * (base / 40), base * 2);
}

/** 音量設定値 (0..100) を範囲内にクランプする。 */
export function clampVolPct(v: number): number {
  return Math.max(0, Math.min(100, v));
}

/**
 * 段階配列 (cps/autoWaitMs 用) 内で dir 方向へ 1 段階だけ移動する（dir は -1 か 1 を渡す）。
 * 端では止まる（ラップしない）。現在値が配列に無ければ先頭扱い。純関数（単体テスト対象）。
 * dir を number 型にしているのは、tsint パーサが型位置の負数リテラル (`1 | -1`) を
 * 未対応のため（コーパステスト tsint_can_parse_all_embedded_runtime_modules 参照）。
 */
export function stepStage(stages: number[], current: number, dir: number): number {
  let idx = stages.indexOf(current);
  if (idx < 0) idx = 0;
  idx = Math.max(0, Math.min(stages.length - 1, idx + dir));
  return stages[idx];
}

export type MenuMode = "save" | "load" | "settings";

/** メニュー見出しクリックでのモード循環: セーブ → ロード → 設定 → セーブ。 */
export function nextMenuMode(m: MenuMode): MenuMode {
  return m === "save" ? "load" : m === "load" ? "settings" : "save";
}

export interface MessageConfig {
  left: number;
  top: number;
  width: number;
  height: number;
  radius: number;
  color: api.RGBA;
  fontSize: number;
  marginX: number;
  marginY: number;
  /** 表示行数。height を明示しない場合、ここからウィンドウ高さを計算する */
  rows: number;
  /** 本文の既定フォント family。未指定ならエンジン既定（同梱フォントがあればそれ）。 */
  fontFamily?: string;
  /** 右/下の余白。未指定は marginX/marginY と同じ（左右・上下対称）。KAG の marginr/marginb。 */
  marginR?: number;
  marginB?: number;
  /** 窓サイズ一致の背景画像（storage。`/` を含まなければ assets/fg/ 配下）。空/未指定でなし。
   * 指定中は窓色の矩形は描かず、画像が窓の背景になる（不透明度設定は画像へ掛かる）。 */
  frame?: string;
}

const TRANSPARENT: api.RGBA = [0, 0, 0, 0];

/** メッセージウィンドウの行送り係数。Rust 側 TextStyle の既定 (1.6) と一致させること。 */
const MSG_LINE_HEIGHT = 1.6;
const F32_MAX = 3.4028234663852886e38;

function isFiniteF32(value: number): boolean {
  return Number.isFinite(value) && Math.abs(value) <= F32_MAX;
}
/** 画面下端からメッセージウィンドウ下辺までの隙間 (px) */
const MSG_BOTTOM_GAP = 24;

export interface LineStyle {
  align: api.TextAlign;
  lineSpacing: number;
}

const DEFAULT_LINE_STYLE: LineStyle = {
  align: "left",
  lineSpacing: 0,
};

function normalizeLineStyle(style: LineStyle | undefined): LineStyle {
  const align =
    style && (style.align === "center" || style.align === "right") ? style.align : "left";
  const lineSpacing = style && isFiniteF32(style.lineSpacing) ? style.lineSpacing : 0;
  return { align, lineSpacing };
}

// ---------- 名前欄プレート ----------

export interface NamePlateConfig {
  left: number;
  top: number;
  width: number;
  height: number;
  radius: number;
  color: api.RGBA;
  fontSize: number;
  marginX: number;
  marginY: number;
}

/** 名前欄のフォントサイズ = 本文フォントサイズ × この係数（本文よりひと回り小さく）。 */
const NAME_PLATE_FONT_RATIO = 0.82;
/** メッセージウィンドウ上端から名前欄下端までの隙間 (px)。ウィンドウに少し重ねる。 */
const NAME_PLATE_OVERLAP = 10;
/** 名前欄の左右マージン・上下マージン (px)。 */
const NAME_PLATE_MARGIN_X = 14;
const NAME_PLATE_MARGIN_Y = 6;
/** 名前欄の幅の下限・上限 (px)。名前の文字数に応じて可変にしつつ極端な幅を防ぐ。 */
const NAME_PLATE_MIN_WIDTH = 88;
const NAME_PLATE_MAX_WIDTH = 220;

/**
 * 名前欄プレートの幅を名前の文字数とフォントサイズから計算する。
 * 幅 = 左右マージン×2 + 文字数×フォントサイズ（等幅近似）を [MIN, MAX] にクランプ。
 * 純関数（単体テスト対象）。
 */
export function namePlateWidth(name: string, fontSize: number, marginX: number): number {
  const chars = codePointCount(name);
  const w = marginX * 2 + chars * fontSize;
  return Math.max(NAME_PLATE_MIN_WIDTH, Math.min(NAME_PLATE_MAX_WIDTH, Math.ceil(w)));
}

/**
 * 名前欄プレートの寸法・位置をメッセージウィンドウ設定と現在の名前から計算する。
 * 高さ = ceil(フォントサイズ × 行送り係数) + 上下マージン×2（1 行分）。
 * 位置はメッセージウィンドウの少し上、左端 + 16px に置き、下端を隙間ぶんだけ
 * ウィンドウへ重ねる（top = msg.top − 高さ − 隙間）。色はウィンドウ色を少し明るくする。
 * 純関数（単体テスト対象）。
 */
export function computeNamePlateCfg(
  msg: MessageConfig,
  name: string,
  style?: { color?: api.RGBA },
): NamePlateConfig {
  const fontSize = Math.round(msg.fontSize * NAME_PLATE_FONT_RATIO);
  const height = Math.ceil(fontSize * MSG_LINE_HEIGHT) + NAME_PLATE_MARGIN_Y * 2;
  const width = namePlateWidth(name, fontSize, NAME_PLATE_MARGIN_X);
  return {
    left: msg.left + 16,
    top: msg.top - height + NAME_PLATE_OVERLAP,
    width,
    height,
    radius: Math.round(msg.radius * 0.7),
    color: (style && style.color) || [
      Math.min(255, msg.color[0] + 28),
      Math.min(255, msg.color[1] + 28),
      Math.min(255, msg.color[2] + 40),
      Math.min(255, msg.color[3] + 30),
    ],
    fontSize,
    marginX: NAME_PLATE_MARGIN_X,
    marginY: NAME_PLATE_MARGIN_Y,
  };
}

/** 常設システムボタン列の詳細指定。省略した項目は既定（8 個すべて・濃紺の地に白文字）。 */
export interface SystemButtonsOptions {
  items?: SysButtonId[];
  color?: api.RGBA;
  textColor?: api.RGBA;
}

export interface KagOptions {
  scenarioDir?: string;
  charaLayerCount?: number;
  message?: Partial<MessageConfig>;
  linkColor?: api.RGBA;
  autoWaitMs?: number;
  /** true（または詳細指定）でメッセージ窓の直上右寄せに Q.SAVE/Q.LOAD/SAVE/LOAD/AUTO/SKIP/LOG/MENU を常設する。既定 off。 */
  systemButtons?: boolean | SystemButtonsOptions;
  /** タイトルの位置。指定するとシステムメニューに「タイトルへ戻る」が出る。 */
  title?: { storage: string; target?: string };
  /** 名前欄プレートの色（color=プレート背景、textColor=名前の文字色）。 */
  namePlate?: { color?: api.RGBA; textColor?: api.RGBA };
  /** UI 効果音（storage 名。`/` を含まなければ assets/se/ 配下）。hover は未指定なら鳴らさない。
   * click 未指定は従来どおり click.wav。 */
  sound?: { hover?: string; click?: string };
  /** プレイヤー向け UI の見た目。"classic"（既定）は従来の箱型窓、"modern" は市販 ADV 風（下部の全幅の帯・
   * 中央寄せの本文カラム・専用のタイトル/メニュー/ギャラリー画面）。 */
  ui?: "classic" | "modern";
  /** `[s]` で止まった時点の `[link]` 群の見せ方。"inline"（本文中の ▶ 行）/ "buttons"（画面中央の縦並びボタン）。
   * 既定は ui="modern" なら "buttons"、それ以外は "inline"。 */
  choices?: { style?: "inline" | "buttons" };
  /** ui="modern" の `[titlescreen]` タグが出す専用タイトル画面の宣言。 */
  titleScreen?: TitleScreenOptions;
  /** 「おまけ」ギャラリー（タイトルの GALLERY 項目）。sf の flag が真なら解放済み。 */
  gallery?: GalleryEntry[];
}

/** バックログの 1 件（段落ごと）。 */
export interface BacklogEntry {
  text: string;
  /** 記録時の [name] の値。 */
  name: string | null;
  /** その段落で鳴らしたボイスの解決済みパス（クリックで再生）。 */
  voice: string | null;
  /** [hr] の区切り線。 */
  rule?: boolean;
}

/** セーブデータへ保存するバックログの件数（直近 N 件）。 */
const BACKLOG_SAVE_LIMIT = 30;
const BACKLOG_LIMIT = 200;

/** [font]/[style]/[deffont]/[defstyle] が操作する文字装飾の状態。face は [font face=] の family 名。 */
export interface FontState {
  size?: number;
  bold?: boolean;
  color?: api.RGBA;
  edgeColor?: api.RGBA;
  face?: string;
}

interface LayerState {
  path: string | null;
  x: number;
  y: number;
  z: number;
  opacity: number;
  visible: boolean;
}

/** 常設システムボタン列の面（classic の SystemBar と modern の ModernSystemBar が共通に持つ）。 */
interface SysBarLike {
  layout(right: number, windowTop: number): void;
  setVisible(v: boolean): void;
  setActive(id: SysButtonId, on: boolean): void;
  hitButton(x: number, y: number): SysButtonId | null;
  hitBand(x: number, y: number): boolean;
  hover(x: number, y: number): void;
}

interface Link {
  target: string | null;
  storage: string | null;
  /** ジャンプの代わりに実行する式 ([link exp=...]) */
  exp: string | null;
  start: number; // currentText の UTF-8 バイト開始
  end: number;
  /** segs 配列内の範囲（ホバー時の色替え用） */
  segStart: number;
  segEnd: number;
}

interface ActiveLink {
  target: string | null;
  storage: string | null;
  exp: string | null;
  start: number;
  segStart: number;
}

/** メッセージレイヤ 1 枚ぶんの状態（W3: 複数メッセージレイヤ）。
 * 既定状態（[laycount]/[current] 未使用）では msgLayers は要素 1 個（message0）のみで、
 * 挙動・見た目は W2 時点と完全一致する（cur ゲッタが常に msgLayers[0] を指すため）。 */
class MessageLayer {
  layerId: number;
  cfg: MessageConfig;
  currentLineStyle: LineStyle = { ...DEFAULT_LINE_STYLE };
  defaultLineStyle: LineStyle = { ...DEFAULT_LINE_STYLE };
  /** レイヤの重ね順（[layopt index=]）。既定 100（旧実装の固定値を踏襲）。 */
  z = 100;
  /** このレイヤ自身の表示可否（[layopt visible=]）。[hidemessage]/右クリックの
   * 一時非表示 (KagPlayer.msgVisible) とは独立で、両方の論理積で実際の表示が決まる。 */
  visible = true;
  segs: api.Segment[] = [];
  links: Link[] = [];
  activeLink: ActiveLink | null = null;
  hoveredLink = -1;
  currentText = "";
  /** 名前欄プレートに表示中の名前（null なら非表示）。[current] でレイヤを切り替えると
   * 名前欄プレートもこのレイヤの値に追従する。 */
  nameLabel: string | null = null;
  /** cfg.frame の背景画像レイヤ（frame 未使用の間は null）と、読み込み済みのパス。 */
  frameLayer: number | null = null;
  framePath: string | null = null;

  constructor(layerId: number, cfg: MessageConfig) {
    this.layerId = layerId;
    this.cfg = cfg;
  }
}

/** フック呼び出し（[rclick call=] 等）で退避するメッセージレイヤ 1 枚ぶんの表示内容。 */
interface MessageContent {
  segs: api.Segment[];
  text: string;
  links: Link[];
}

/** [laycount messages=N] の上限。KAG3 に厳密な上限は無いが、ネイティブ側テキストレイヤを
 * 無制限に確保しないよう実装上の上限を設ける（対応表に明記）。 */
const MAX_MSG_LAYERS = 8;

interface SaveData {
  version: number;
  /** index（W6 で追加）: 定義されていればラベル境界ではない任意位置への厳密な復帰
   * （[tempsave]/[record] 等）。未定義なら従来通りラベルからの再生（[save]/onLabel 自動記録）。 */
  pos: { file: string; label: string | null; index?: number };
  f: Record<string, unknown>;
  layers: ({ name: string } & LayerState)[];
  /** paused/positionMs が無い旧セーブは false/0ms として復元する。 */
  bgm: { path: string; volume: number; paused?: boolean; positionMs?: number } | null;
  /** 名前欄プレートに表示中の名前（null なら非表示）。カレントレイヤの nameLabel の写し
   * （v1 互換 + 表示用）。 */
  name: string | null;
  date: string;
  title: string;
  /** v2 (W3) で追加: 各メッセージレイヤの状態。v1 セーブには無い（migrateSaveV1toV2 が
   * 空配列で補完する）。 */
  msgLayers?: {
    cfg: MessageConfig;
    visible: boolean;
    nameLabel: string | null;
    currentLineStyle?: LineStyle;
    defaultLineStyle?: LineStyle;
  }[];
  /** v2 (W3) で追加: カレントレイヤの index。 */
  current?: number;
  /** W6 で追加: [tempsave]/[record] 等の完全スナップショットのみ持つ、カレントレイヤの
   * 表示内容（`pos.index` とセットで使う）。 */
  curSegs?: api.Segment[];
  curText?: string;
  /** W7 で追加: [record]/[goback] の履歴リング（trail）を [save]/F5 の永続化に含める
   * （`docs/implementation-notes.md`「2026-07-12」参照）。`makeSnapshot()`/`makeFullSnapshot()`
   * 自体はこのフィールドを設定しない（trail エントリ自身が trail を持たないようにするため。
   * save() が実際にディスクへ書き出す直前にだけ付加する）。旧セーブ（このフィールドが無い）は
   * ロード時に空配列で補完する（後方互換）。上限は KagPlayer.TRAIL_LIMIT（30 件）のまま。 */
  trail?: SaveData[];
  /** B2 で追加: 保存時刻（`YYYY/MM/DD HH:MM`）・本文の抜粋・サムネイルの有無・保存時点の履歴
   * （直近 BACKLOG_SAVE_LIMIT 件）。旧セーブには無く、一覧では空欄/空表示、ロード時の履歴は空で補完する。 */
  savedAt?: string;
  excerpt?: string;
  thumb?: boolean;
  backlog?: BacklogEntry[];
}

/** セーブ JSON の `backlog` を検証して履歴へ戻す（旧セーブ・不正な形は空/読み飛ばし）。純関数。 */
export function restoreBacklog(raw: unknown): BacklogEntry[] {
  const out: BacklogEntry[] = [];
  if (!Array.isArray(raw)) return out;
  for (const e of raw) {
    if (e === null || typeof e !== "object" || typeof (e as any).text !== "string") continue;
    const x = e as any;
    const entry: BacklogEntry = {
      text: x.text,
      name: typeof x.name === "string" ? x.name : null,
      voice: typeof x.voice === "string" ? x.voice : null,
    };
    if (x.rule === true) entry.rule = true;
    out.push(entry);
  }
  return out.slice(-BACKLOG_LIMIT);
}

/** v1 セーブデータ（`msgLayers`/`current` が欠損している可能性がある）を v2 形式へ
 * 補完する純関数（単体テスト対象）。v1 は「laycount 機能が存在しなかった」ため
 * 保存済みのレイヤ位置情報が無く、`msgLayers` は空配列のまま返す（呼び出し側 `load()` は
 * 空配列の場合、現在実行中のレイヤ構成に触れず `name` フィールドのみで名前欄を復元する
 * 旧来の経路にフォールバックする）。既に v2 以上のデータはそのまま返す。 */
export function migrateSaveV1toV2(data: SaveData): SaveData {
  if (data.version >= 2) return data;
  return {
    ...data,
    version: 2,
    msgLayers: data.msgLayers ?? [],
    current: data.current ?? 0,
  };
}

/**
 * [ignore]/[endignore]（W6）の入れ子走査。commands の startIndex から、[if]/[ignore] で
 * depth++、[endif]/[endignore] で depth-- し、depth が 0 からもうひとつ閉じた地点の
 * 「その次」の index を返す（if/ignore を同じ depth に含める skipToNextBranch/skipToEndif と
 * 同型の走査。閉じタグが見つからなければ commands.length）。純関数（単体テスト対象）。
 */
export function scanIgnoreBlock(commands: Command[], startIndex: number): number {
  let depth = 0;
  let i = startIndex;
  while (i < commands.length) {
    const c = commands[i];
    i++;
    if (c.type !== "tag") continue;
    if (c.name === "if" || c.name === "ignore") depth++;
    else if (c.name === "endif" || c.name === "endignore") {
      if (depth === 0) return i;
      depth--;
    }
  }
  return commands.length;
}

export class KagPlayer {
  api = api;
  vars = vars;

  screenW = 1280;
  screenH = 720;
  scenarioDir: string;
  charaLayerCount: number;
  private msgOverrides: Partial<MessageConfig>;
  linkColor: api.RGBA;
  autoWaitMs: number;
  /** 設定メニューの値（音量・cps・オート待ち等）。sf.config に永続化される。 */
  settings: SettingsConfig;
  /** 「初期設定に戻す」の戻し先（ゲームが autoWaitMs を指定していればそれを反映）。 */
  private defaultSettings: SettingsConfig;
  private titleOpt: { storage: string; target: string | null } | null;
  private namePlateOpt: { color?: api.RGBA; textColor?: api.RGBA };
  private soundOpt: { hover?: string; click?: string };
  private sysBarOpt: { items: SysButtonId[]; color: api.RGBA; textColor: api.RGBA } | null;
  private sysBar: SysBarLike | null = null;
  /** ui="modern" か。会話画面の帯・話者名・システムボタン列・選択肢ボタン・各メニューの描画が変わる。 */
  readonly modern: boolean;
  private choiceButtons: boolean;
  private titleScreenOpt: TitleScreenOptions | null;
  private galleryOpt: GalleryEntry[];
  private modernName: ModernNameTag | null = null;
  private modernGlyph: ModernGlyph | null = null;
  private modernToast: ModernToast | null = null;
  private modeBadge: ModernModeBadge | null = null;
  /** 選択肢ボタン画面（`[s]` の停止中だけ存在）。見た目だけを持ち、ジャンプなどの意味づけは player が行う。 */
  private choiceUi: ChoiceScreen | null = null;
  private choiceLinks: Link[] = [];
  /** 選択肢ボタンの出現を待っている間（本文の文字送り中）のクリックで文字送りを完了させる。 */
  private choiceWaiting = false;
  /** lockUi の入れ子の深さ（タイトル画面が開いたままメニューを開くため）。 */
  private uiDepth = 0;
  private glyphGen = 0;
  /** [sysbar visible=] で切り替える常設ボタンの表示可否（ロードで既定の表示へ戻る）。 */
  private sysBarOn = true;
  /** [autosave enabled=] で切り替えるオートセーブ可否（ロードで既定の有効へ戻る）。 */
  autoSaveOn = true;
  private toastUi: Toast | null = null;
  /** メニューを開く直前に撮ったサムネイルをセーブに使うか（メニュー上のセーブにメニューが写らないように）。 */
  private thumbPending = false;
  private modeWaiters: (() => void)[] = [];
  /** MenuHost: メニューを開き直したとき同じページから始めるための記憶（0 = オート/クイック枠）。 */
  pages = { save: 1, load: 0 };
  readonly cpsValues: number[] = CPS_STAGES;
  readonly cpsLabels: string[] = CPS_LABELS;
  readonly autoValues: number[] = AUTO_STAGES;
  readonly autoLabels: string[] = AUTO_LABELS;

  /** 論理名 ("base" | "0".."N" | "message") → レイヤ id */
  layers = new Map<string, number>();
  layerStates = new Map<string, LayerState>();
  /** メッセージレイヤ (W3: 複数メッセージレイヤ)。既定は要素 1 個（message0）。 */
  msgLayers: MessageLayer[] = [];
  /** 以後の文章表示・cm 以外のメッセージ操作の対象 ([current layer=.. page=..])。 */
  current = { index: 0 };
  /** [autowc enabled= ch= time=]: 指定文字の後に自動ウェイトを入れる（既定オフ）。 */
  autowc: { enabled: boolean; ch: string; time: number } = { enabled: false, ch: "", time: 0 };
  /** [resetwait]: wait 基準時刻の保存のみ（[wait mode=until] が無い現実装では他に
   * 参照箇所が無い。docs/kag-tag-compat.md 参照）。 */
  waitBaseMs = 0;

  // ---- ストア系 (W6, W7 で tempsave/tempload をディスク対応に修正) ----
  /** [tempsave]/[tempload]: place が負数のときだけ使うメモリ上のみのスナップショットスロット
   * （place をキーにする）。place が 0 以上ならディスクの `"temp{place}"` スロット
   * （sys.saveWrite/saveRead、通常セーブ "save{N}" とは別プレフィックス）を使う（KAG3 の
   * 「負の place はメモリ専用スロット」慣習に対応、W7。docs/implementation-notes.md
   * 「2026-07-12」参照）。 */
  tempSlots = new Map<number, SaveData>();
  /** [record]/[goback]/[gotostart]/[startanchor]: 巻き戻し履歴リング（KAG の sf.trail 相当の
   * 概念だが、実装はメモリ上のみで sf には永続化しない。理由は implementation-notes 参照）。 */
  private trail: SaveData[] = [];
  /** trail の保持件数の上限（実装上の上限、MAX_MSG_LAYERS と同じ位置づけ）。 */
  private static readonly TRAIL_LIMIT = 30;
  /** [startanchor]/[gotostart]（W7 で KAG3 準拠に修正）: `[startanchor]`（既定 enabled=true）
   * 実行時点の完全スナップショットを保持する専用アンカースロット。`[gotostart]` はここへ
   * 復帰する（`trail` とは独立、消費しない）。`enabled=false` はアンカーを破棄する。
   * `docs/implementation-notes.md`「2026-07-12」参照。 */
  private anchor: SaveData | null = null;
  /** [disablestore enabled=]: save/load/tempsave/tempload/store/copybookmark/erasebookmark/
   * goback/gotostart を一時的に禁止する。 */
  private storeDisabled = false;
  /** [locksnapshot]/[unlocksnapshot]: onLabel() の自動スナップショット更新を禁止する。 */
  private snapshotLocked = false;
  /** [jump]/[call] の store=false 属性: 次に onLabel() を通過する 1 回だけ自動スナップショット
   * 更新をスキップする（優先順位は implementation-notes 参照）。 */
  private suppressNextAutoSnapshot = false;

  /** カレントメッセージレイヤ。既定状態（laycount 未使用）では常に msgLayers[0]
   * （旧実装の単一 msgId 相当）を指す。 */
  get cur(): MessageLayer {
    return this.msgLayers[this.current.index];
  }

  private scenarios = new Map<string, Scenario>();
  pc = { file: "", index: 0 };
  /** section: 呼び出し時に読み進め中だったラベル区間（戻ったとき再開する）。restore: 右クリック等の
   * フック呼び出し時点のメッセージ内容（サブルーチンが消していても戻ったとき復元する）。 */
  private callStack: {
    file: string;
    index: number;
    section: string | null;
    restore?: MessageContent[];
  }[] = [];
  running = false;
  private jumpRequest: { file: string; label: string | null; isCall: boolean } | null = null;

  cps = 35;
  nowait = false;
  mode = { skip: false, auto: false };
  private uiLock = false;
  /** uiLock（メニュー/バックログ/モーダル表示中）の経過ぶんを wait・オート・[timeout] から除くための
   * 累計。pausedSince は表示中のみ非 null。pauseCount は表示開始のたびに増える。 */
  private pausedMs = 0;
  private pausedSince: number | null = null;
  private pauseCount = 0;
  private resumeWaiters: (() => void)[] = [];
  /** 最後に sys スロットへ書いた JSON（変化したときだけ書くための dirty 判定）。 */
  private sysSavedJson: string | null = null;
  /** 読み進め中のラベル区間 (`file*label`)。区間を最後まで通過した時点で既読にする。 */
  private readSection: string | null = null;
  /** Ctrl 押下中スキップ: 押下中か、押下前からスキップが入っていたか（S キーのトグルと併用するため）。 */
  private ctrlHeld = false;
  private skipBeforeCtrl = false;
  /** [playvoice] で鳴らしたボイスがまだ停止されていない可能性があるか（次へ送る操作で止める）。 */
  private voiceActive = false;
  private currentVoiceVolume: number | null = null;
  private seVolumes = new Map<string, number>();

  backlogEnabled = true;
  backlog: BacklogEntry[] = [];
  /** ラベル通過時点（snapshot と同時）の履歴。セーブはラベルから再生し直すので、その時点の履歴を保存する。 */
  private snapshotBacklog: BacklogEntry[] = [];
  private paragraph = "";
  /** 現在の段落の最初のテキストを出した時点の [name]、その段落で鳴らしたボイス（履歴の 1 件に記録する）。 */
  private paragraphName: string | null = null;
  private paragraphVoice: string | null = null;
  /** [font]/[style] タグで積んだ「現在のフォント状態」。printText がセグメントへ反映する。
   * [cm]/[er]/clearMessage() ではリセットしない（[name] の既存の慣習に合わせる）。 */
  currentFontState: FontState = {};
  /** [resetfont] が戻る先のデフォルト値（[deffont] で更新）。 */
  fontDefaults: FontState = {};
  /** [resetstyle] が戻る先のデフォルト値（[defstyle] で更新）。 */
  styleDefaults: FontState = {};
  /** [ruby text=...] で予約したルビ文字列。次の printText 呼び出しで 1 回だけ消費される。 */
  pendingRuby: string | null = null;
  hoverColor: api.RGBA = [255, 255, 255, 255];
  /** [hidemessage]/右クリックによる全レイヤ一時非表示。次のクリックで復帰する。
   * [hidemessage] タグから直接書き換えるため公開。 */
  msgVisible = true;
  /** [button]/[checkbox]/[edit] の管理（W4）。import は player.ts → buttons.ts の
   * 一方向のみ（docs/implementation-notes.md 参照）。active は [s] 停止中のみ true。 */
  buttons = new ButtonManager();
  /// [animstart]/[animstop]/[wa] の管理（W5）。buttons と同じ「player.ts → anim.ts の
  /// 一方向 import」の自己完結モジュール（`docs/implementation-notes.md`「2026-07-11 W5」参照）。
  anim = new AnimManager();
  /** [locklink]/[unlocklink]: リンク・ボタン・クリック送りの一括ロック。 */
  linksLocked = false;
  /** [clickskip enabled=]: false でクリックによるスキップモード解除を禁止する
   * （既定 true = 従来通りクリックでスキップ解除できる）。 */
  clickSkipEnabled = true;
  /** [nextskip]: 次の [l]/[p]/[s] の待ちを消費したら自動的に mode.skip を解除する
   * ワンショットフラグ。 */
  private nextSkipOneShot = false;
  /** [click]/[cclick] [wheel]/[cwheel]: マウス入力の常設ジャンプフック。 */
  eventJumps: {
    click: { storage: string | null; target: string | null } | null;
    wheel: { storage: string | null; target: string | null } | null;
  } = { click: null, wheel: null };
  /** [timeout]/[ctimeout]: クリック待ち（waitAdvance/waitForJump）中のみ発火するジャンプ予約
   * （KAG3 準拠、W7 で修正。`docs/implementation-notes.md`「2026-07-12」参照）。 */
  private timeoutJump: { storage: string | null; target: string | null } | null = null;
  private timeoutGen = 0;
  /** 満了したが（クリック待ち外だったため）まだ発火していないタイマー。次に
   * waitAdvance()/waitForJump() へ入った時点で即発火する。 */
  private timeoutExpired = false;
  /** 現在 waitAdvance()/waitForJump()（[l]/[p]/[s] のクリック待ち）の中かどうか。
   * [timeout] の発火可否の判定に使う。 */
  private inAdvanceWait = false;
  /** [rclick]: 右クリック動作のカスタム。既定 ("default") は既存の全レイヤ表示
   * トグル、"disabled" は無効化、オブジェクト指定でジャンプ/コールへ差し替える。 */
  rclickAction:
    | "default"
    | "disabled"
    | { call: boolean; storage: string | null; target: string | null } = "default";
  /** [waittrig name=...]: 名前ごとの待機者リスト。[eval exp="kag.trigger('x')"]（KAG3 準拠、
   * evalExpr/evalStmt の kag 引数経由）または後方互換の
   * [eval exp="sys.player.trigger('x')"] から発火する。 */
  private triggerWaiters = new Map<string, (() => void)[]>();
  /** [glyph line= fix= left= top=]: クリック待ち記号のカスタム画像。line 未指定
   * (path=null) の既定状態では何も表示しない（元々このエンジンに専用の待ち記号表示が
   * 無かったため、既定の挙動＝無表示を維持する。docs/implementation-notes.md 参照）。
   * fix=false（既定、W7 で実装、KAG3 準拠）はクリック待ち開始時のペン位置（最終グリフの
   * 右隣）へ追従する。fix=true は従来通り固定位置（left=/top= 省略時はメッセージウィンドウ
   * 右下）。 */
  private glyphCfg: { path: string | null; left: number | null; top: number | null; fix: boolean } = {
    path: null,
    left: null,
    top: null,
    fix: false,
  };
  private glyphLayer: number | null = null;
  /** 名前欄プレート（物理的には 1 枚だけ再利用し、カレントレイヤの nameLabel/cfg に
   * 追従して再描画する）。 */
  private nameLayer: number | null = null;
  private pendingOffs = new Set<() => void>();
  lastQuakeMs = 0;
  currentBgm: { path: string; volume: number; paused: boolean } | null = null;
  /** BGM の再生チャンネル。[xchgbgm] のクロスフェードで "bgm" ⇄ "bgm2" を切り替える。
   * セーブ対象外（ロード時は "bgm" に戻す）。 */
  bgmCh: string = "bgm";
  /** [setbgmstop] の予約（BGM 自然終端でジャンプ）。[clearbgmstop] で解除。 */
  bgmStopJump: { storage: string | null; target: string | null } | null = null;
  private bgmStopGen = 0;
  private currentLabel: { file: string; label: string | null; display: string | null } = {
    file: "",
    label: null,
    display: null,
  };
  private snapshot: SaveData | null = null;

  macros = new Map<string, Command[]>();
  macroArgs: Attrs[] = [];

  private tags: Record<string, TagHandler>;
  private interruptResolvers: ((v: "int") => void)[] = [];
  private warnedTags = new Set<string>();
  private backlogLayer: number | null = null;
  private backlogHeader: UiBox | null = null;
  private backlogBar: UiBox | null = null;
  private backlogThumb: UiBox | null = null;
  private backlogOffset = 0;
  private indicatorLayer: number | null = null;

  constructor(opts: KagOptions = {}) {
    this.scenarioDir = opts.scenarioDir ?? "scenario/";
    this.charaLayerCount = opts.charaLayerCount ?? 3;
    this.linkColor = opts.linkColor ?? [255, 216, 120, 255];
    this.autoWaitMs = opts.autoWaitMs ?? 1800;
    this.defaultSettings = { ...DEFAULT_SETTINGS, autoWaitMs: this.autoWaitMs };
    this.settings = { ...this.defaultSettings };
    this.titleOpt = opts.title ? { storage: opts.title.storage, target: opts.title.target ?? null } : null;
    this.modern = opts.ui === "modern";
    this.choiceButtons = (opts.choices?.style ?? (this.modern ? "buttons" : "inline")) === "buttons";
    this.titleScreenOpt = opts.titleScreen ?? null;
    this.galleryOpt = opts.gallery ?? [];
    this.namePlateOpt = opts.namePlate ?? {};
    this.soundOpt = opts.sound ?? {};
    const sb = opts.systemButtons;
    if (sb === true || (sb !== undefined && sb !== false && typeof sb === "object")) {
      const o = sb === true ? {} : sb;
      this.sysBarOpt = {
        items: o.items ?? [...SYS_BUTTON_ORDER],
        color: o.color ?? [30, 38, 74, 220],
        textColor: o.textColor ?? UI_COLORS.text,
      };
    } else {
      this.sysBarOpt = null;
    }
    this.msgOverrides = opts.message ?? {};
    this.tags = makeTags(this);
  }

  /**
   * メッセージウィンドウ寸法をフォントメトリクスと画面サイズから計算する。
   * 高さ = ceil(fontSize × 行送り係数 × 行数) + 上下マージン。
   * グリフの実寸（ディセンダ・縁取り込み）が行送り×行数の中に収まることは
   * `cargo run -p noiria-core --example measure_msgbox` で実測検証済み。
   * ユーザーが opts.message で明示した値は常に優先される。
   */
  private computeMsgCfg(): MessageConfig {
    const o = this.msgOverrides;
    if (this.modern) {
      return {
        left: o.left ?? 0,
        top: o.top ?? this.screenH - (o.height ?? BAND_H),
        width: o.width ?? this.screenW,
        height: o.height ?? BAND_H,
        radius: 0,
        color: o.color ?? [6, 9, 24, 236],
        fontSize: o.fontSize ?? BAND_FONT,
        marginX: o.marginX ?? Math.floor((this.screenW - COL_W) / 2),
        marginY: o.marginY ?? BAND_TEXT_TOP,
        rows: o.rows ?? 4,
        fontFamily: o.fontFamily,
        marginR: o.marginR,
        marginB: o.marginB ?? BAND_TEXT_BOTTOM,
        frame: o.frame,
      };
    }
    const fontSize = o.fontSize ?? 26;
    const marginY = o.marginY ?? 18;
    const rows = o.rows ?? 4;
    const height = o.height ?? Math.ceil(fontSize * MSG_LINE_HEIGHT * rows) + marginY * 2;
    const left = o.left ?? 40;
    return {
      left,
      top: o.top ?? this.screenH - MSG_BOTTOM_GAP - height,
      width: o.width ?? this.screenW - left * 2,
      height,
      radius: o.radius ?? 14,
      color: o.color ?? [10, 12, 30, 195],
      fontSize,
      marginX: o.marginX ?? 30,
      marginY,
      rows,
      fontFamily: o.fontFamily,
      marginR: o.marginR,
      marginB: o.marginB,
      frame: o.frame,
    };
  }

  // ---------- 初期化 ----------

  private init(): void {
    try {
      const cfg = JSON.parse(api.sys.readText("game.json"));
      this.screenW = cfg.width ?? 1280;
      this.screenH = cfg.height ?? 720;
    } catch {
      /* 既定値のまま */
    }

    // base
    const base = api.layer.create("image");
    api.layer.set(base, { z: 0, visible: false }, "fore");
    api.layer.set(base, { z: 0, visible: false }, "back");
    this.layers.set("base", base);
    this.layerStates.set("base", { path: null, x: 0, y: 0, z: 0, opacity: 255, visible: false });

    // キャラレイヤ
    for (let i = 0; i < this.charaLayerCount; i++) {
      const id = api.layer.create("image");
      api.layer.set(id, { z: 10 + i, visible: false }, "fore");
      api.layer.set(id, { z: 10 + i, visible: false }, "back");
      this.layers.set(String(i), id);
      this.layerStates.set(String(i), {
        path: null,
        x: 0,
        y: 0,
        z: 10 + i,
        opacity: 255,
        visible: false,
      });
    }

    // メッセージレイヤ (W3: 複数メッセージレイヤ。既定は message0 の 1 枚のみで、
    // 挙動・見た目は W2 時点と完全一致する)
    const msgLayerId = api.layer.create("text");
    const ml0 = new MessageLayer(msgLayerId, this.computeMsgCfg());
    this.msgLayers = [ml0];
    this.current = { index: 0 };
    this.applyMessageLayerConfig(0);
    this.layers.set("message", msgLayerId);
    this.layers.set("message0", msgLayerId);

    // システム変数
    // 壊れた sys.json は読み出し側 (savedata.rs) が .bak へ退避して null を返す。ここへ届くのは
    // JSON として妥当なものだけだが、オブジェクトでない値（null/配列等）は採用しない。
    const sysJson = api.sys.saveRead("sys");
    if (sysJson) {
      try {
        const parsed = JSON.parse(sysJson);
        if (parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)) {
          vars.replaceSf(parsed);
        } else {
          api.sys.log("sys.json がオブジェクトではないため無視します", "error");
        }
      } catch (e) {
        api.sys.log(`sys.json を読み込めませんでした（既定値で続行）: ${e}`, "error");
      }
    }
    if (!vars.sf.readLabels) vars.sf.readLabels = {};
    this.loadSettings();
    if (this.settings.msgOpacity !== 100) this.refreshMessageWindows();

    if (this.modern) {
      this.modernGlyph = new ModernGlyph(200);
      const m0 = this.msgLayers[0].cfg;
      this.modernName = new ModernNameTag(
        m0.left + m0.marginX,
        m0.top,
        m0.width - m0.marginX - (m0.marginR ?? m0.marginX),
        this.namePlateOpt.textColor ?? MC.accent,
      );
    }
    if (this.sysBarOpt !== null) {
      this.sysBar = this.modern
        ? new ModernSystemBar(this.sysBarOpt.items, 106)
        : new SystemBar(this.sysBarOpt, 106);
      this.layoutSysBar();
    }

    this.installGlobalHandlers();
  }

  /** sf.config から設定値を読み込み（無ければ既定値のまま）、cps/autoWaitMs へ反映する。 */
  private loadSettings(): void {
    const raw = vars.sf.config as Partial<SettingsConfig> | undefined;
    if (raw && typeof raw === "object") {
      if (typeof raw.bgmVol === "number") this.settings.bgmVol = clampVolPct(raw.bgmVol);
      if (typeof raw.seVol === "number") this.settings.seVol = clampVolPct(raw.seVol);
      if (typeof raw.voiceVol === "number") this.settings.voiceVol = clampVolPct(raw.voiceVol);
      if (typeof raw.cps === "number" && CPS_STAGES.indexOf(raw.cps) >= 0) this.settings.cps = raw.cps;
      if (typeof raw.autoWaitMs === "number" && AUTO_STAGES.indexOf(raw.autoWaitMs) >= 0) {
        this.settings.autoWaitMs = raw.autoWaitMs;
      }
      if (typeof raw.skipUnread === "boolean") this.settings.skipUnread = raw.skipUnread;
      if (typeof raw.msgOpacity === "number") this.settings.msgOpacity = clampVolPct(raw.msgOpacity);
    }
    vars.sf.config = this.settings; // 以後 saveSys() で自動的にシリアライズされる
    this.applySettings();
  }

  /** settings の値を実行時状態 (cps/autoWaitMs) へ反映する。 */
  private applySettings(): void {
    this.cps = this.settings.cps;
    this.autoWaitMs = this.settings.autoWaitMs;
  }

  /** タグの volume (0..1) に設定の音量割合を掛けた実効音量を返す。 */
  volFor(kind: "bgm" | "se" | "voice", tagVolume: number): number {
    const pct =
      kind === "bgm" ? this.settings.bgmVol : kind === "se" ? this.settings.seVol : this.settings.voiceVol;
    return effectiveVolume(tagVolume, pct);
  }

  /** 指定インデックスのメッセージレイヤへ cfg/z/visible をネイティブ側へ反映する
   * （旧 applyMessageConfig の複数レイヤ対応版）。呼び出し順序（msgbox → {x,y} →
   * text.style → {z,visible}）は旧実装の op 呼び出し列と完全に一致させてある
   * （tsint リポジトリの kag_full.rs op ログ回帰テストが呼び出し順に依存するため）。 */
  applyMessageLayerConfig(idx: number): void {
    const ml = this.msgLayers[idx];
    const m = ml.cfg;
    const framed = this.prepareFrame(ml);
    if (this.modern && !framed) {
      api.layer.panel(ml.layerId, { w: m.width, h: m.height, ...bandPanel(this.windowColor(m.color)) }, "fore");
    } else {
      api.layer.msgbox(ml.layerId, m.width, m.height, m.radius, framed ? TRANSPARENT : this.windowColor(m.color), "fore");
    }
    api.layer.set(ml.layerId, { x: m.left, y: m.top }, "fore");
    this.applyMessageLayerTextStyle(idx);
    api.layer.set(ml.layerId, { z: ml.z, visible: this.msgVisible && ml.visible }, "fore");
    if (framed) this.placeFrame(ml);
    if (idx === 0 && this.sysBar !== null) this.layoutSysBar();
  }

  /** メッセージ窓の色に「窓の濃さ」設定（%）を掛けたもの。100% は色をそのまま返す（既定の ops 列を保つ）。 */
  private windowColor(c: api.RGBA): api.RGBA {
    const pct = this.settings.msgOpacity;
    if (pct >= 100) return c;
    return [c[0], c[1], c[2], Math.round((c[3] * pct) / 100)];
  }

  /** cfg.frame の背景画像レイヤを用意する（frame 未使用なら何も呼ばず false）。読み込めなければ frame を無効扱いにする。 */
  private prepareFrame(ml: MessageLayer): boolean {
    const f = ml.cfg.frame;
    if (!f) {
      if (ml.frameLayer !== null) {
        api.layer.free(ml.frameLayer);
        ml.frameLayer = null;
        ml.framePath = null;
      }
      return false;
    }
    const path = this.resolveStorage("fg", f);
    if (ml.frameLayer === null) {
      ml.frameLayer = api.layer.create("image");
      ml.framePath = null;
    }
    if (ml.framePath !== path) {
      try {
        api.layer.loadImage(ml.frameLayer, path, "fore");
        ml.framePath = path;
      } catch (e) {
        api.sys.log(`メッセージ窓の frame を読み込めません: ${path}: ${e}`, "error");
        api.layer.free(ml.frameLayer);
        ml.frameLayer = null;
        ml.framePath = null;
        return false;
      }
    }
    return true;
  }

  private placeFrame(ml: MessageLayer): void {
    if (ml.frameLayer === null) return;
    api.layer.set(
      ml.frameLayer,
      {
        x: ml.cfg.left,
        y: ml.cfg.top,
        z: ml.z - 1,
        visible: this.msgVisible && ml.visible,
        opacity: Math.round((255 * this.settings.msgOpacity) / 100),
      },
      "fore",
    );
  }

  /** 窓の濃さ設定が変わったとき、全メッセージ窓と名前欄を描き直す。 */
  refreshMessageWindows(): void {
    for (let i = 0; i < this.msgLayers.length; i++) this.applyMessageLayerConfig(i);
    this.renderNamePlate();
  }

  applyMessageLayerTextStyle(idx: number): void {
    const ml = this.msgLayers[idx];
    const m = ml.cfg;
    const lineHeight = this.messageLineHeight(idx, ml.currentLineStyle.lineSpacing);
    if (lineHeight === null) {
      api.sys.log(
        `[style] linespacing=${ml.currentLineStyle.lineSpacing} から有限な lineHeight を算出できないため無視します`,
        "warn",
      );
      return;
    }
    const style: api.TextStyle = {
      fontSize: m.fontSize,
      lineHeight,
      boxW: m.width - m.marginX - (m.marginR ?? m.marginX),
      boxH: m.height - m.marginY - (m.marginB ?? m.marginY),
      marginX: m.marginX,
      marginY: m.marginY,
      align: ml.currentLineStyle.align,
    };
    if (m.fontFamily !== undefined) style.fontFamily = m.fontFamily;
    api.text.style(ml.layerId, style);
  }

  messageLineHeight(idx: number, lineSpacing: number): number | null {
    if (!isFiniteF32(lineSpacing)) return null;
    const fontSize = this.msgLayers[idx].cfg.fontSize;
    const lineHeight =
      lineSpacing === 0
        ? MSG_LINE_HEIGHT
        : Math.max(1, fontSize * MSG_LINE_HEIGHT + lineSpacing) / fontSize;
    return isFiniteF32(lineHeight) ? lineHeight : null;
  }

  reflowMessageLayer(idx: number): void {
    const ml = this.msgLayers[idx];
    if (ml.segs.length > 0) api.text.reflow(ml.layerId);
  }

  /** msgVisible（[hidemessage]/右クリックの一時非表示）と各レイヤ自身の visible の
   * 論理積を全メッセージレイヤへ適用する。[hidemessage] タグからも呼ばれるため公開。 */
  applyGlobalVisibility(): void {
    for (const ml of this.msgLayers) {
      api.layer.set(ml.layerId, { visible: this.msgVisible && ml.visible }, "fore");
      if (ml.frameLayer !== null) {
        api.layer.set(ml.frameLayer, { visible: this.msgVisible && ml.visible }, "fore");
      }
    }
    this.updateNamePlateVisibility();
    this.updateSysBarVisibility();
    if (this.choiceUi !== null) this.choiceUi.setVisible(this.msgVisible);
    if (this.modernGlyph !== null && !this.msgVisible) this.modernGlyph.hide();
  }

  /** "message0".."message{N-1}"（"message" は "message0" の別名）→ index。
   * 存在しない/範囲外なら null。layer= 省略時はカレントの index を返す。 */
  msgLayerIndex(name: string | undefined): number | null {
    if (name === undefined) return this.current.index;
    const m = /^message(\d*)$/.exec(name);
    if (!m) return null;
    const idx = m[1] === "" ? 0 : parseInt(m[1], 10);
    return idx >= 0 && idx < this.msgLayers.length ? idx : null;
  }

  /** [laycount messages=N]: メッセージレイヤ数を変更する（[1, MAX_MSG_LAYERS] にクランプ）。
   * 増分は既定寸法の新規レイヤを追加、減分は末尾から解放する。カレントが範囲外になったら
   * message0 に戻す。 */
  setLayCount(n: number): void {
    const count = Math.max(1, Math.min(MAX_MSG_LAYERS, Math.floor(n) || 1));
    if (count > this.msgLayers.length) {
      for (let i = this.msgLayers.length; i < count; i++) {
        const layerId = api.layer.create("text");
        const ml = new MessageLayer(layerId, this.computeMsgCfg());
        this.msgLayers.push(ml);
        this.layers.set(`message${i}`, layerId);
        this.applyMessageLayerConfig(i);
      }
    } else if (count < this.msgLayers.length) {
      while (this.msgLayers.length > count) {
        const i = this.msgLayers.length - 1;
        if (this.msgLayers[i].frameLayer !== null) api.layer.free(this.msgLayers[i].frameLayer!);
        api.layer.free(this.msgLayers[i].layerId);
        this.layers.delete(`message${i}`);
        this.msgLayers.pop();
      }
      if (this.current.index >= count) this.current.index = 0;
    }
  }

  // ---------- 名前欄プレート ----------

  private ensureNameLayer(): number {
    if (this.nameLayer === null) {
      this.nameLayer = api.layer.create("text");
      api.layer.set(this.nameLayer, { z: 105, visible: false }, "fore");
    }
    return this.nameLayer;
  }

  /**
   * [name text=...] / [name] のハンドラ本体。text が非空なら名前欄を設定して表示、
   * 省略時は非表示にする（[cm]/[er] では呼ばれないため、明示的に変えるまで維持される）。
   * 名前はカレントレイヤに紐づく（[current] でレイヤを切り替えると表示も追従する）。
   */
  setName(text: string | null): void {
    this.cur.nameLabel = text && text.length > 0 ? text : null;
    if (this.cur.nameLabel !== null || this.modernName !== null) this.renderNamePlate();
    this.updateNamePlateVisibility();
  }

  /** カレントレイヤの名前・メッセージウィンドウ寸法から名前欄プレートを再描画する
   * （[position]/[current]/[layopt] 変更時にも使う）。物理的な名前欄レイヤは 1 枚だけ
   * 再利用し、カレントレイヤの nameLabel/cfg に追従させる。 */
  renderNamePlate(): void {
    if (this.modernName !== null) {
      const m = this.cur.cfg;
      this.modernName.relayout(m.left + m.marginX, m.top, m.width - m.marginX - (m.marginR ?? m.marginX));
      this.modernName.set(this.cur.nameLabel, this.msgVisible && this.cur.visible);
      return;
    }
    if (this.cur.nameLabel === null) return;
    const id = this.ensureNameLayer();
    const cfg = computeNamePlateCfg(this.cur.cfg, this.cur.nameLabel, this.namePlateOpt);
    api.layer.msgbox(id, cfg.width, cfg.height, cfg.radius, this.windowColor(cfg.color), "fore");
    api.layer.set(id, { x: cfg.left, y: cfg.top }, "fore");
    const plateStyle: api.TextStyle = {
      fontSize: cfg.fontSize,
      lineHeight: MSG_LINE_HEIGHT,
      boxW: cfg.width - cfg.marginX * 2,
      boxH: cfg.height - cfg.marginY * 2,
      marginX: cfg.marginX,
      marginY: cfg.marginY,
    };
    if (this.cur.cfg.fontFamily !== undefined) plateStyle.fontFamily = this.cur.cfg.fontFamily;
    api.text.style(id, plateStyle);
    api.text.print(id, [{ text: this.cur.nameLabel, color: this.namePlateOpt.textColor ?? null }], false, 0);
  }

  /** 名前欄の表示可否を反映する（名前が設定されている & カレントレイヤ・メッセージ
   * ウィンドウ全体が表示中のときのみ表示）。[current]/[ct] からも呼ばれるため公開。 */
  updateNamePlateVisibility(): void {
    if (this.modernName !== null) {
      this.renderNamePlate();
      return;
    }
    if (this.nameLayer === null) return;
    const visible = this.msgVisible && this.cur.visible && this.cur.nameLabel !== null;
    api.layer.set(this.nameLayer, { visible }, "fore");
  }

  private installGlobalHandlers(): void {
    // ウィンドウを閉じる操作はどの状態でも（メニュー表示中・[wait] 中・トランジション待ち中でも）
    // 入力ポンプが直接ここへ届ける。sf を保存してから終了する。
    api.input.onWindowClose(() => {
      this.saveSys();
      api.sys.quit();
    });
    api.input.on((ev) => {
      if (ev.kind === "keyup") {
        if (ev.key === "Control" && this.ctrlHeld) {
          this.ctrlHeld = false;
          if (!this.skipBeforeCtrl) this.mode.skip = false;
        }
        return;
      }
      if (ev.kind === "key") {
        if (this.uiLock) return;
        const k = ev.key;
        // フォーカス中の [edit]/[input] があれば入力を横取りする（キーボード
        // ショートカットより優先。W4）。
        if (k !== undefined && this.buttons.handleKey(k)) return;
        if (this.choiceUi !== null && !this.linksLocked) {
          const r = this.choiceUi.handle(ev);
          if (r.picked !== null) {
            void this.pickChoice(r.picked);
            return;
          }
          if (r.consumed) return;
        }
        if (k === "Control") {
          // 押している間だけスキップ（S キーのトグルとは独立。オートリピートは重ねて押下扱いにしない）。
          if (!this.ctrlHeld) {
            this.ctrlHeld = true;
            this.skipBeforeCtrl = this.mode.skip;
          }
          this.mode.skip = true;
          // 押した瞬間に今のクリック待ちを抜ける（押しっぱなしで次々に送るため）。
          if (this.inAdvanceWait) this.fireInterrupt();
        } else if (k === "s" || k === "S") this.mode.skip = !this.mode.skip;
        else if (k === "a" || k === "A") this.mode.auto = !this.mode.auto;
        else if (k === "F5") this.quickSave();
        else if (k === "F9") void this.quickLoad();
        else if (k === "h" || k === "H" || k === "PageUp") void this.openBacklog();
        else if (k === "Escape") void this.openSaveMenu();
        else if (k === "ArrowDown") this.focusLink(1);
        else if (k === "ArrowUp") this.focusLink(-1);
        else if (k === "Enter" && this.linkFocused()) this.activateLink(this.cur.hoveredLink);
      } else if (ev.kind === "wheel") {
        if (this.uiLock) return;
        // [wheel]/[cwheel] 常設フック（W4）: 設定されていれば通常のホイール履歴表示より優先。
        // フックは常にジャンプ（KAG3 に call 形式は無い）。[jump] タグと同じくメッセージを消す。
        if (this.eventJumps.wheel) {
          const j = this.eventJumps.wheel;
          this.clearMessage();
          this.requestJump(j.storage, j.target);
          return;
        }
        if ((ev.delta ?? 0) > 0) void this.openBacklog();
      } else if (ev.kind === "move") {
        if (this.choiceUi !== null && !this.uiLock && !this.linksLocked) this.choiceUi.handle(ev);
        else this.updateHover(ev.x, ev.y);
        this.buttons.hover(ev.x, ev.y);
        if (!this.uiLock && this.sysBar !== null) this.sysBar.hover(ev.x, ev.y);
      } else if (ev.kind === "click" && ev.button === "right") {
        if (this.uiLock) return;
        // [rclick]（W4）: 既定 ("default") は KAG 標準の全メッセージレイヤ表示切り替え
        // （[hidemessage] と同じ msgVisible フラグ）。"disabled" は無効化、それ以外は
        // ジャンプ/コールへ差し替える。
        if (this.rclickAction === "disabled") return;
        if (this.rclickAction !== "default") {
          const a = this.rclickAction;
          if (a.call) {
            // call はメッセージを消さない。サブルーチンが消しても、戻ったとき元の表示と待ちを復元する。
            this.callFromHook(a.storage, a.target);
          } else {
            this.clearMessage();
            this.requestJump(a.storage, a.target);
          }
          return;
        }
        this.msgVisible = !this.msgVisible;
        this.applyGlobalVisibility();
      } else if (ev.kind === "click" && ev.button === "left") {
        // 常設システムボタン: クリック待ち中・文字送り中・[s] 中のいずれでも反応し、本文送りにはしない。
        if (!this.uiLock && this.sysBar !== null) {
          const id = this.sysBar.hitButton(ev.x, ev.y);
          if (id !== null) {
            this.onSysButton(id);
            return;
          }
        }
        // 選択肢ボタン: ボタン上のクリックは選択、文字送り中のクリックは文字送りの完了。
        if (!this.uiLock && this.choiceUi !== null && !this.linksLocked) {
          const r = this.choiceUi.handle(ev);
          if (r.picked !== null) {
            void this.pickChoice(r.picked);
            return;
          }
          if (r.consumed) return;
        }
        if (!this.uiLock && this.choiceWaiting) {
          api.text.complete(this.cur.layerId);
          return;
        }
        // [click]/[cclick] 常設フック（W4）: 設定されていれば最優先でジャンプする。
        if (!this.uiLock && this.eventJumps.click) {
          const j = this.eventJumps.click;
          this.clearMessage();
          this.requestJump(j.storage, j.target);
          return;
        }
        // [button]/[checkbox]/[edit] のヒットテスト（[s] 停止中のみ活性、W4）。
        if (!this.uiLock) {
          const action = this.buttons.click(ev.x, ev.y);
          if (action) {
            this.clearMessage();
            if (action.exp) {
              try {
                vars.evalStmt(action.exp, this.sys, this);
              } catch (e) {
                api.sys.log(`[button] exp の実行エラー: ${e}`, "error");
              }
            } else {
              this.requestJump(action.storage, action.target);
            }
            return;
          }
        }
        // [clickskip enabled=false] の間はクリックによるスキップ解除を行わない。
        if (this.mode.skip && this.clickSkipEnabled) this.mode.skip = false;
        if (!this.msgVisible) {
          // 消えている全レイヤをクリックで復帰（名前欄プレートも連動）
          this.msgVisible = true;
          this.applyGlobalVisibility();
          return;
        }
        // [locklink] の間はリンククリックも無効化する。
        if (!this.uiLock && this.cur.links.length > 0 && !this.linksLocked && !this.choiceButtons) {
          this.tryClickLink(ev.x, ev.y);
        }
      }
    });
  }

  /** [rclick call=true] 等のマウスフック呼び出し。メッセージは消さず、戻り先を待ち（[l]/[p]/[s]）
   * の位置に取る（マクロ内の待ちは位置を再現できないため待ちの直後）。戻ったときは popReturn が
   * 退避した表示内容を復元する。 */
  private callFromHook(storage: string | null, target: string | null): void {
    const rewait = this.inAdvanceWait && this.macroArgs.length === 0;
    this.callStack.push({
      file: this.pc.file,
      index: rewait ? this.pc.index - 1 : this.pc.index,
      section: this.readSection,
      restore: this.captureMessages(),
    });
    this.readSection = null;
    this.requestJump(storage, target, true);
  }

  private captureMessages(): MessageContent[] {
    return this.msgLayers.map((ml) => ({ segs: [...ml.segs], text: ml.currentText, links: [...ml.links] }));
  }

  private restoreMessages(saved: MessageContent[]): void {
    saved.forEach((c, i) => {
      const ml = this.msgLayers[i];
      if (!ml || (ml.currentText === c.text && ml.segs.length === c.segs.length)) return;
      this.clearLayerContent(ml);
      if (c.segs.length === 0) return;
      api.text.print(ml.layerId, c.segs, false, 0);
      ml.segs = [...c.segs];
      ml.currentText = c.text;
      ml.links = [...c.links];
    });
  }

  // ---------- シナリオ管理 ----------

  private scenario(file: string): Scenario {
    let sc = this.scenarios.get(file);
    if (!sc) {
      const src = api.sys.readText(
        file.includes("/") ? file : this.scenarioDir + file,
      );
      sc = parseScenario(src, file);
      this.scenarios.set(file, sc);
    }
    return sc;
  }

  resolveStorage(kind: "bg" | "fg" | "bgm" | "se" | "voice" | "rule" | "video", storage: string): string {
    if (storage.includes("/")) return storage;
    return `assets/${kind}/${storage}`;
  }

  // ---------- 実行ループ ----------

  async run(startFile: string, startLabel?: string): Promise<void> {
    this.init();
    this.pc = { file: startFile.replace(/^scenario\//, ""), index: 0 };
    if (startLabel) {
      try {
        const sc = this.scenario(this.pc.file);
        this.pc.index = sc.labels.get(startLabel.replace(/^\*/, "")) ?? 0;
      } catch {
        /* 読めないシナリオは下の実行ループが画面表示つきで報告する */
      }
    }
    this.running = true;
    if (this.modern) {
      prefetchSlotThumbs();
      const sf = vars.sf as Record<string, unknown>;
      prefetchGalleryThumbs(this.galleryOpt.filter((e) => !!sf[e.flag]).map((e) => e.image));
    }
    void this.modeIndicatorLoop();

    while (this.running) {
      // メニュー/バックログ表示中はシナリオを進めない（閉じたら再開）。
      if (this.uiLock) await this.waitResumed();

      if (this.jumpRequest) {
        const req = this.jumpRequest;
        this.jumpRequest = null;
        this.cur.links = [];
        let sc: Scenario;
        try {
          sc = this.scenario(req.file);
        } catch (e) {
          api.sys.log(`シナリオを読み込めません: ${req.file}: ${e}`, "error");
          this.abandonCall(req);
          await this.haltWithMessage(`シナリオ '${req.file}' が見つかりません`);
          continue;
        }
        const idx = req.label !== null ? sc.labels.get(req.label) : 0;
        if (idx === undefined) {
          this.abandonCall(req);
          await this.haltWithMessage(`ラベル '*${req.label}' が見つかりません (${req.file})`);
          continue;
        }
        if (req.file !== this.pc.file) {
          // 別シナリオへ移る = 今のラベル区間を最後まで読み終えた。シナリオ遷移の節目で sf も保存する。
          this.finishSection();
          this.saveSys();
        }
        this.pc = { file: req.file, index: idx ?? 0 };
        continue;
      }

      let sc: Scenario;
      try {
        sc = this.scenario(this.pc.file);
      } catch (e) {
        api.sys.log(`シナリオを読み込めません: ${this.pc.file}: ${e}`, "error");
        await this.haltWithMessage(`シナリオ '${this.pc.file}' が見つかりません`);
        continue;
      }
      if (this.pc.index >= sc.commands.length) {
        // ファイル終端: 次の指示 (ジャンプ) を待つ
        this.finishSection();
        this.saveSys();
        await this.idleUntilJump();
        continue;
      }

      const cmd = sc.commands[this.pc.index++];
      try {
        await this.execCommand(cmd);
      } catch (e) {
        api.sys.log(`${this.pc.file}:${cmd.line} 実行エラー: ${(e as Error)?.stack ?? e}`, "error");
      }
    }
  }

  /** 失敗した [call]（フック呼び出し含む）が積んだ戻り先を捨てる。 */
  private abandonCall(req: { isCall: boolean }): void {
    if (!req.isCall) return;
    const e = this.callStack.pop();
    if (e) this.readSection = e.section;
  }

  /** 欠落コンテンツ（存在しないシナリオ/ラベル）に当たったとき、落下実行せずメッセージ窓に理由を
   * 出してクリック待ち→その位置で停止する（[s] 相当。メニューからのロード等でジャンプ要求が来れば
   * 再開する）。 */
  private async haltWithMessage(msg: string): Promise<void> {
    api.sys.log(msg, "error");
    this.mode.skip = false;
    this.mode.auto = false;
    this.clearAllMessages();
    this.msgVisible = true;
    this.cur.visible = true;
    this.applyGlobalVisibility();
    const seg: api.Segment = { text: msg, color: null };
    api.text.print(this.cur.layerId, [seg], false, 0);
    this.cur.segs.push(seg);
    this.cur.currentText += msg;
    this.paragraph += msg;
    await this.waitAdvance();
    await this.waitForJump();
  }

  async execCommand(cmd: Command): Promise<void> {
    switch (cmd.type) {
      case "label":
        this.onLabel(cmd.name, cmd.display);
        return;
      case "iscript":
        // [iscript]〜[endscript]（W6）: 生テキストを TS として評価する（TJS 互換はしない）。
        // tsint でも Function コンストラクタが実行時コンパイルに対応済みのため、既存の
        // vars.evalStmt（eval タグ等と同じ経路）をそのまま使う（対応表・実装ノート参照）。
        vars.evalStmt(cmd.code, this.sys, this);
        return;
      case "text":
        if (this.autowc.enabled && this.autowc.ch) {
          await this.printTextWithAutoWait(cmd.text);
        } else {
          this.printText(cmd.text, null);
        }
        return;
      case "tag": {
        const attrs = this.resolveAttrs(cmd.attrs);
        if (this.macros.has(cmd.name)) {
          await this.execMacro(cmd.name, attrs);
          return;
        }
        const handler = this.tags[cmd.name];
        if (!handler) {
          if (!this.warnedTags.has(cmd.name)) {
            this.warnedTags.add(cmd.name);
            api.sys.log(`未対応タグ: [${cmd.name}]`, "warn");
          }
          return;
        }
        await handler(attrs);
        return;
      }
    }
  }

  /** &式 と %マクロ引数 を解決した属性のコピーを返す */
  resolveAttrs(attrs: Attrs): Attrs {
    const out: Attrs = {};
    for (const [k, v] of Object.entries(attrs)) {
      if (v.startsWith("&")) {
        out[k] = String(vars.evalExpr(v.slice(1), this.sysObject(), this));
      } else if (v.startsWith("%")) {
        const body = v.slice(1);
        const bar = body.indexOf("|");
        const name = bar >= 0 ? body.slice(0, bar) : body;
        const dflt = bar >= 0 ? body.slice(bar + 1) : "";
        const top = this.macroArgs[this.macroArgs.length - 1] ?? {};
        out[k] = top[name] ?? dflt;
      } else {
        out[k] = v;
      }
    }
    return out;
  }

  private async execMacro(name: string, args: Attrs): Promise<void> {
    if (this.macroArgs.length > 16) {
      api.sys.log(`マクロの入れ子が深すぎます: ${name}`, "error");
      return;
    }
    const body = this.macros.get(name)!;
    this.macroArgs.push(args);
    try {
      for (const cmd of body) {
        if (this.uiLock) await this.waitResumed();
        await this.execCommand(cmd);
      }
    } finally {
      this.macroArgs.pop();
    }
  }

  /** 今のラベル区間を読了済みにする（次のラベル到達・別シナリオへの jump・終端・[return] で呼ぶ）。
   * 途中までしか読んでいない区間（ロード/終了で抜けた場合）は呼ばれないので既読にならない。 */
  private finishSection(): void {
    if (this.readSection === null) return;
    const labels = (vars.sf.readLabels ?? (vars.sf.readLabels = {})) as Record<string, boolean>;
    labels[this.readSection] = true;
    this.readSection = null;
  }

  private onLabel(name: string, display: string | null): void {
    this.finishSection();
    const key = `${this.pc.file}*${name}`;
    const read = (vars.sf.readLabels as Record<string, boolean>)[key] === true;
    // 既読スキップ中に未読区間へ入ったら止まる（設定で「未読もスキップ」にしていれば止まらない）。
    if (this.mode.skip && !read && !this.settings.skipUnread) this.mode.skip = false;
    this.readSection = key;
    this.saveSys();
    this.currentLabel = { file: this.pc.file, label: name, display };
    // W6: locksnapshot（永続、最優先）→ store=false による 1 回限りの抑制、の順で
    // 自動スナップショット更新を止める。disablestore はこの自動更新自体には関与せず、
    // save()/load() 等の「実行そのもの」を別レイヤでガードする（両者は独立に併用できる。
    // docs/implementation-notes.md「2026-07-11 W6」参照）。
    if (this.snapshotLocked) {
      /* 既存の snapshot を維持する */
    } else if (this.suppressNextAutoSnapshot) {
      this.suppressNextAutoSnapshot = false;
    } else {
      this.snapshot = this.makeSnapshot();
      this.snapshotBacklog = this.backlog.slice(-BACKLOG_SAVE_LIMIT);
    }
  }

  // ---------- テキスト ----------

  printText(t: string, colorOverride: api.RGBA | null): void {
    const cur = this.cur;
    // 選択肢ボタン方式では [link] 内の文字は窓に見せない（レイアウトと履歴にだけ残る）。
    const hidden = this.choiceButtons && cur.activeLink !== null;
    const color = hidden
      ? TRANSPARENT
      : (cur.activeLink ? this.linkColor : colorOverride) ?? this.currentFontState.color ?? null;
    const cps = hidden || this.nowait || this.mode.skip ? 0 : this.cps;
    const seg: api.Segment = { text: t, color };
    if (hidden) seg.edgeColor = TRANSPARENT;
    if (this.currentFontState.size !== undefined) seg.size = this.currentFontState.size;
    if (this.currentFontState.bold !== undefined) seg.bold = this.currentFontState.bold;
    if (this.currentFontState.edgeColor !== undefined) seg.edgeColor = this.currentFontState.edgeColor;
    if (this.currentFontState.face !== undefined) seg.fontFamily = this.currentFontState.face;
    if (this.pendingRuby !== null) {
      seg.ruby = this.pendingRuby;
      this.pendingRuby = null;
    }
    api.text.print(cur.layerId, [seg], true, cps);
    cur.segs.push(seg);
    cur.currentText += t;
    if (this.paragraph.trim() === "") this.paragraphName = cur.nameLabel;
    this.paragraph += t;
  }

  /** [autowc]: 指定文字 (ch) の後ろで自動的に time ms 待つよう分割して printText する
   * （Rust の reveal は無変更、TS 側で分割 print + sys.wait するだけ）。
   * `wait()` は中断 (jumpRequest) されても例外を投げず素通りする（KagPlayer 全体の
   * 既存方針）ため、複数回 await するこのループでは各待ちの後に jumpRequest を
   * 明示チェックして早期 return する。チェックせずに残りを印字し続けると、
   * セーブ/ロードや選択肢クリックによるジャンプで clearAllMessages() 済みのレイヤへ
   * 旧シナリオ位置のテキストが紛れ込む（実機で確認済みの不具合、修正済み）。 */
  private async printTextWithAutoWait(t: string): Promise<void> {
    const ch = this.autowc.ch;
    let buf = "";
    for (const c of t) {
      buf += c;
      if (c === ch) {
        this.printText(buf, null);
        buf = "";
        await this.wait(this.autowc.time);
        if (this.jumpRequest !== null) return;
      }
    }
    if (buf.length > 0) this.printText(buf, null);
  }

  /** [graph storage=...]: 解決済みパスを 1 グリフ扱いのインライン画像として現在の
   * カレントレイヤへ挿入する（設計判断、W3。docs/implementation-notes.md 参照）。 */
  printImage(path: string): void {
    const cur = this.cur;
    const cps = this.nowait || this.mode.skip ? 0 : this.cps;
    const seg: api.Segment = { text: "", color: null, inlineImage: path };
    api.text.print(cur.layerId, [seg], true, cps);
    cur.segs.push(seg);
  }

  /** [locate]/[indent]/[endindent] 用: グリフを持たない制御専用セグメントを送る。
   * 文字送り (cps) やバックログには影響しない（`text: ""` のため実質何もしない）。 */
  printControl(seg: api.Segment): void {
    const cur = this.cur;
    api.text.print(cur.layerId, [seg], true, 0);
    cur.segs.push(seg);
  }

  /** [font]/[style]/[deffont]/[defstyle] 用: `"0xRRGGBB"` を RGBA へパースする。
   * tags.ts が `noiria:kag/player` の値をインポートすると player.ts → tags.ts → player.ts の
   * 循環importになり、tsint（V8 と異なり import を静的な live binding ではなく評価順に
   * 解決する）では未評価時に `undefined` を掴んで `TypeError` になる（実機で確認済み）。
   * そのためモジュールレベル関数を export せず、インスタンスメソッド経由で公開する。 */
  hexColor(v: string | undefined, dflt?: api.RGBA): api.RGBA | undefined {
    return parseHexColor(v, dflt);
  }

  newline(): void {
    const cur = this.cur;
    const seg = { text: "\n", color: null };
    api.text.print(cur.layerId, [seg], true, 0);
    cur.segs.push(seg);
    cur.currentText += "\n";
    this.paragraph += "\n";
  }

  /** [er]: カレントレイヤのみクリアする（旧 clearMessage 相当。既定 1 レイヤ構成では
   * [cm]/[ct] と同一の見た目になる）。 */
  clearMessage(): void {
    this.flushBacklog();
    this.clearLayerContent(this.cur);
  }

  /** [cm]: 全メッセージレイヤをクリアする（カレントは変更しない）。 */
  clearAllMessages(): void {
    this.flushBacklog();
    for (const ml of this.msgLayers) this.clearLayerContent(ml);
  }

  private clearLayerContent(ml: MessageLayer): void {
    api.text.clear(ml.layerId);
    ml.currentText = "";
    ml.segs = [];
    ml.links = [];
    ml.activeLink = null;
    ml.hoveredLink = -1;
  }

  /** 現在の断片を（ホバー色を反映して）描き直す。文字送りはしない。カレントレイヤ対象。 */
  repaint(): void {
    const cur = this.cur;
    if (cur.segs.length === 0) return;
    const hovered = cur.links[cur.hoveredLink];
    const painted = cur.segs.map((s, i) =>
      hovered && i >= hovered.segStart && i < hovered.segEnd
        ? { text: s.text, color: this.hoverColor }
        : s,
    );
    api.text.print(cur.layerId, painted, false, 0);
  }

  /** [hr]（W6）からも呼ぶため公開（現在の段落をバックログへ確定する）。 */
  flushBacklog(): void {
    const p = this.paragraph.trim();
    if (this.backlogEnabled && p.length > 0) {
      this.pushBacklog({ text: p, name: this.paragraphName, voice: this.paragraphVoice });
    }
    this.paragraph = "";
    this.paragraphName = null;
    this.paragraphVoice = null;
  }

  private pushBacklog(e: BacklogEntry): void {
    this.backlog.push(e);
    if (this.backlog.length > BACKLOG_LIMIT) this.backlog.shift();
  }

  /** [hr]: バックログへ区切り線を挿入する（現在の段落を確定してから追加する）。 */
  insertBacklogRule(): void {
    this.flushBacklog();
    if (this.backlogEnabled) this.pushBacklog({ text: "──────────", name: null, voice: null, rule: true });
  }

  // ---------- 待ち合わせ (WaitController 相当) ----------

  private interruptible<T>(p: Promise<T>): Promise<T | "int"> {
    return Promise.race([
      p,
      new Promise<"int">((res) => this.interruptResolvers.push(res)),
    ]);
  }

  private fireInterrupt(): void {
    const rs = this.interruptResolvers;
    this.interruptResolvers = [];
    for (const r of rs) r("int");
    // 中断された入力待ちのハンドラを掃除（リーク防止）
    for (const off of this.pendingOffs) off();
    this.pendingOffs.clear();
  }

  private waitAdvanceEvent(): Promise<api.InputEvent> {
    return new Promise((resolve) => {
      const off = api.input.on((e) => {
        if (this.uiLock) return;
        if (
          // [locklink]: クリック送りも対象（Enter/Space キーは引き続き有効）。
          (e.kind === "click" &&
            e.button === "left" &&
            this.msgVisible &&
            !this.linksLocked &&
            !this.barBlocks(e.x, e.y)) ||
          (e.kind === "key" && ((e.key === "Enter" && !this.linkFocused()) || e.key === " "))
        ) {
          off();
          this.pendingOffs.delete(off);
          resolve(e);
        }
      });
      this.pendingOffs.add(off);
    });
  }

  /** [l]/[p] の待ち。クリック 1 回目はリベール完了、2 回目で送り。カレントレイヤ対象。
   * [nextskip] のワンショット消費と [glyph] のクリック待ち記号表示を外側で共通に扱う。
   * [timeout] の発火可否判定用に、この間 inAdvanceWait を true にする。 */
  async waitAdvance(): Promise<void> {
    this.saveSys();
    this.inAdvanceWait = true;
    this.checkTimeoutOnWaitEnter();
    try {
      await this.waitAdvanceInner();
    } finally {
      this.inAdvanceWait = false;
    }
    this.glyphGen++;
    this.setGlyphVisible(false);
    this.consumeNextSkip();
  }

  private async waitAdvanceInner(): Promise<void> {
    // checkTimeoutOnWaitEnter() がこの待ちへ入った時点で満了済みタイマーを発火させて
    // jumpRequest を立てている可能性がある（[timeout] 準拠、W7）。その場合は待たずに戻る。
    if (this.jumpRequest !== null) return;
    // システムボタンで AUTO/SKIP を切り替えたときだけ待ちをやり直す（キー操作の挙動は従来どおり）。
    while ((await this.waitAdvanceStep()) === "retry") {
      if (this.jumpRequest !== null) return;
    }
  }

  private async waitAdvanceStep(): Promise<"done" | "retry"> {
    this.modeWaiters = [];
    if (this.mode.skip) {
      api.text.complete(this.cur.layerId);
      this.stopVoiceOnAdvance();
      await this.sleepActive(40, false);
      return "done";
    }
    if (this.mode.auto) {
      while (!api.text.isDone(this.cur.layerId)) {
        if ((await this.sleepActive(50)) === "int") return "done";
      }
      // ボイス再生中はボイス終了まで待ってからオート待ち時間を数える。待ち中のクリックはオート解除+送り。
      const r = await this.interruptible(
        Promise.race([
          this.autoDelay(),
          this.waitAdvanceEvent().then(() => "click" as const),
          this.modeWake(),
        ]),
      );
      if (r === "int") return "done";
      if (r === "mode") return "retry";
      if (r === "click") this.mode.auto = false;
      this.stopVoiceOnAdvance();
      return "done";
    }
    while (true) {
      if (api.text.isDone(this.cur.layerId)) this.setGlyphVisible(true);
      else if (this.modernGlyph !== null) void this.watchGlyph();
      const r = await this.interruptible(
        Promise.race([this.waitAdvanceEvent().then(() => "click" as const), this.modeWake()]),
      );
      if (r === "int") return "done";
      if (r === "mode") {
        if (this.mode.skip || this.mode.auto) return "retry";
        continue;
      }
      if (!api.text.isDone(this.cur.layerId)) {
        this.setGlyphVisible(false);
        api.text.complete(this.cur.layerId);
        continue;
      }
      this.stopVoiceOnAdvance();
      return "done";
    }
  }

  /** modern のクリック待ち記号: 待ちの途中で文字送りが終わったときにも出す（classic の [glyph] は待ちへ入った
   * 時点で文字送りが終わっているときだけ出る）。 */
  private async watchGlyph(): Promise<void> {
    const gen = ++this.glyphGen;
    while (this.inAdvanceWait && gen === this.glyphGen && !api.text.isDone(this.cur.layerId)) {
      await api.sys.wait(40);
    }
    if (this.inAdvanceWait && gen === this.glyphGen) this.setGlyphVisible(true);
  }

  /** システムボタンでモードが切り替わったら解決する（待ち中の waitAdvanceStep が状態を見直す）。 */
  private modeWake(): Promise<"mode"> {
    return new Promise((resolve) => {
      this.modeWaiters.push(() => resolve("mode"));
    });
  }

  private wakeModeWaiters(): void {
    const ws = this.modeWaiters;
    this.modeWaiters = [];
    for (const w of ws) w();
  }

  /** オート送りの待ち: ボイスの終了を待ってから autoWaitFor(autoWaitMs, 文字数)（メニュー表示中は数えない）。 */
  private async autoDelay(): Promise<"timeout" | "int"> {
    await api.audio.waitEnd("voice");
    const chars = this.cur.currentText.replace(/\s/g, "").length;
    return (await this.sleepActive(autoWaitFor(this.autoWaitMs, chars))) === "int" ? "int" : "timeout";
  }

  /** 次の行/ページへ送る操作で、再生中のボイスを止める。 */
  private stopVoiceOnAdvance(): void {
    if (!this.voiceActive) return;
    this.voiceActive = false;
    this.currentVoiceVolume = null;
    api.audio.stop("voice");
  }

  /** [playvoice]: 前のボイスを止めて鳴らす。 */
  playVoice(path: string, tagVolume = 1): void {
    api.audio.stop("voice");
    api.audio.play("voice", path, { volume: this.volFor("voice", tagVolume) });
    this.voiceActive = true;
    this.currentVoiceVolume = tagVolume;
    this.paragraphVoice = path;
  }

  /** [stopvoice] */
  stopVoice(): void {
    this.voiceActive = false;
    this.currentVoiceVolume = null;
    api.audio.stop("voice");
  }

  noteSePlay(ch: string, tagVolume: number): void {
    this.seVolumes.set(ch, tagVolume);
  }

  noteSeStop(ch: string): void {
    this.seVolumes.delete(ch);
  }

  syncSeVolumes(): void {
    for (const [ch, tagVol] of [...this.seVolumes.entries()]) {
      api.audio.volume(ch, this.volFor("se", tagVol));
    }
  }

  syncVoiceVolume(): void {
    if (this.voiceActive && this.currentVoiceVolume !== null) {
      api.audio.volume("voice", this.volFor("voice", this.currentVoiceVolume));
    }
  }

  private syncBgmVolume(): void {
    if (this.currentBgm) {
      api.audio.volume(this.bgmCh, this.volFor("bgm", this.currentBgm.volume));
    }
    const otherCh = this.bgmCh === "bgm" ? "bgm2" : "bgm";
    if (this.settings.bgmVol === 0 && api.audio.isFading(otherCh)) {
      api.audio.volume(otherCh, 0);
    }
  }

  stopAllSe(): void {
    api.audio.stop("se");
    for (const [ch] of [...this.seVolumes.entries()]) {
      // "se" と "se0" は audio.rs 上で同一スロット (CH_SE) の別名。二重停止を避ける。
      if (ch !== "se" && ch !== "se0") api.audio.stop(ch);
    }
    this.seVolumes.clear();
  }

  // ---------- 一時停止（メニュー/バックログ/モーダル表示中） ----------

  private lockUi(): void {
    this.uiDepth++;
    if (this.uiLock) return;
    this.uiLock = true;
    setGlyphPaused(true);
    this.pauseCount++;
    this.pausedSince = api.sys.now();
  }

  private unlockUi(): void {
    if (this.uiDepth > 0) this.uiDepth--;
    if (this.uiDepth > 0 || !this.uiLock) return;
    this.uiLock = false;
    setGlyphPaused(false);
    if (this.pausedSince !== null) {
      this.pausedMs += api.sys.now() - this.pausedSince;
      this.pausedSince = null;
    }
    const waiters = this.resumeWaiters;
    this.resumeWaiters = [];
    for (const w of waiters) w();
  }

  private pausedTotal(): number {
    return this.pausedMs + (this.pausedSince !== null ? api.sys.now() - this.pausedSince : 0);
  }

  /** uiLock が解けるまで待つ（表示中でなければ即座に解決）。 */
  private waitResumed(): Promise<void> {
    if (!this.uiLock) return Promise.resolve();
    return new Promise<void>((resolve) => {
      this.resumeWaiters.push(resolve);
    });
  }

  /** ms の待ち。uiLock 中の時間は数えない（途中で表示が出入りしても残りを待ち直す）。
   * canInterrupt（既定 true）ならジャンプ要求で打ち切り "int" を返す。表示が一度も出なかった
   * 通常経路では api.sys.wait を 1 回呼ぶだけで、従来と同じ ops 列・タイミングになる。 */
  private async sleepActive(ms: number, canInterrupt = true): Promise<"done" | "int"> {
    let remaining = ms;
    while (true) {
      if (this.uiLock) {
        const r = await (canInterrupt ? this.interruptible(this.waitResumed()) : this.waitResumed());
        if (r === "int") return "int";
      }
      const count0 = this.pauseCount;
      const t0 = api.sys.now();
      const paused0 = this.pausedTotal();
      const wake = api.sys.wait(remaining);
      const r = await (canInterrupt ? this.interruptible(wake) : wake);
      if (r === "int") return "int";
      if (this.pauseCount === count0 && !this.uiLock) return "done";
      remaining -= api.sys.now() - t0 - (this.pausedTotal() - paused0);
      if (remaining <= 1) return "done";
    }
  }

  /** スキップ考慮の時間待ち */
  async wait(ms: number): Promise<void> {
    if (this.mode.skip) ms = Math.min(ms, 30);
    await this.sleepActive(ms);
  }

  /** [wa]（W5）等、外部モジュール（tags.ts）から使うループ待ち専用ヘルパ:
   * `cond()` が真の間 `stepMs` ずつ待ち続ける。各待ちの後にジャンプ要求（セーブ/ロード等）が
   * 来ていれば直ちに中断する（`printTextWithAutoWait` と同じ既存ルール、複数 await するタグは
   * 各 wait 後に `jumpRequest` をチェックする。`jumpRequest` は private フィールドのため、
   * tags.ts など外部からは直接参照できずこのメソッド経由になる）。 */
  async waitWhile(cond: () => boolean, stepMs = 30): Promise<void> {
    while (cond()) {
      await this.wait(stepMs);
      if (this.jumpRequest !== null) return;
    }
  }

  async waitTransition(): Promise<void> {
    if (this.mode.skip) api.trans.skip();
    await api.trans.wait();
  }

  /** [wm] 用: 進行中の move (レイヤアニメーション) が全て完了するまで待つ。
   * スキップモード時は即座に完了させて返る（ネイティブの通知待ちが不要なため）。 */
  async waitMove(): Promise<void> {
    if (this.mode.skip) {
      api.layer.moveSkip();
      return;
    }
    while (api.layer.moving()) {
      const r = await this.interruptible(api.sys.wait(30));
      if (r === "int") return;
    }
  }

  /** [wb]/[wf] 用: チャンネルのフェード完了待ち。canskip=true でスキップ中は即抜け。
   * --autotest（音声スレッド不在）では isFading が常に false のため自然に即抜けする。 */
  async waitFade(ch: string, canskip: boolean): Promise<void> {
    if (canskip && this.mode.skip) return;
    // FadeTo/Play コマンドがオーディオスレッドへ届き状態が同期されるまでの猶予
    await api.sys.wait(60);
    while (api.audio.isFading(ch)) {
      if (canskip && this.mode.skip) return;
      const r = await this.interruptible(api.sys.wait(30));
      if (r === "int") return;
    }
  }

  /** [wl] 用: BGM の演奏終了（非ループ再生の自然終端）待ち。
   * finished だけでなく「そもそも再生中でない」場合も完了扱いにする。
   * --autotest では音声スレッドが起動せず finished が永遠に立たないため、
   * isPlaying=false（既定値）で即抜けるこのフォールバックが無いとハングする。 */
  async waitBgmEnd(canskip: boolean): Promise<void> {
    if (canskip && this.mode.skip) return;
    await api.sys.wait(60);
    while (api.audio.isPlaying(this.bgmCh) && !api.audio.isFinished(this.bgmCh)) {
      if (canskip && this.mode.skip) return;
      const r = await this.interruptible(api.sys.wait(30));
      if (r === "int") return;
    }
  }

  /** [setbgmstop]: BGM の演奏終了（自然終端）時のジャンプを予約する。 */
  setBgmStop(storage: string | null, target: string | null): void {
    this.bgmStopJump = { storage, target };
    const gen = ++this.bgmStopGen;
    void this.watchBgmStop(gen);
  }

  /** [clearbgmstop]: setbgmstop の予約を解除する。 */
  clearBgmStop(): void {
    this.bgmStopJump = null;
    this.bgmStopGen++;
  }

  /** setbgmstop の監視ループ（waitEnd と同じポーリング方式）。世代カウンタで
   * clearbgmstop / 再予約時に自然消滅する。 */
  private async watchBgmStop(gen: number): Promise<void> {
    while (this.running && this.bgmStopJump !== null && this.bgmStopGen === gen) {
      if (api.audio.isFinished(this.bgmCh)) {
        const j = this.bgmStopJump;
        this.bgmStopJump = null;
        this.requestJump(j.storage, j.target);
        return;
      }
      await api.sys.wait(120);
    }
  }

  // ---------- UI 部品 (button/checkbox/edit) の状態制御 (W4) ----------

  /** [glyph line= fix= left= top=]: クリック待ち記号を設定する。path=null で無表示に戻す。
   * fix=false（既定、W7 で実装）はクリック待ち開始時のペン位置へ追従する。fix=true は
   * 従来通り固定位置（left=/top= 省略時はメッセージウィンドウ右下）として扱う
   * （docs/implementation-notes.md 参照）。 */
  setGlyph(path: string | null, left: number | null, top: number | null, fix: boolean): void {
    this.glyphCfg = { path, left, top, fix };
    if (this.glyphLayer !== null) {
      if (path) api.layer.loadImage(this.glyphLayer, path, "fore");
      else {
        api.layer.clearImage(this.glyphLayer, "fore");
        api.layer.set(this.glyphLayer, { visible: false }, "fore");
      }
    }
  }

  private ensureGlyphLayer(): number {
    if (this.glyphLayer === null) {
      this.glyphLayer = api.layer.create("image");
      if (this.glyphCfg.path) api.layer.loadImage(this.glyphLayer, this.glyphCfg.path, "fore");
      api.layer.set(this.glyphLayer, { z: 200, visible: false }, "fore");
    }
    return this.glyphLayer;
  }

  /** クリック待ち記号の表示位置を更新する（`setGlyphVisible(true)` から呼ぶ）。
   * fix=true は固定位置（left=/top= 省略時はメッセージウィンドウ右下）。fix=false（既定）は
   * `api.text.rects` の最終グリフ矩形からペン位置を計算し、その右隣に配置する
   * （「ペン位置追従」、W7）。rects が空（本文なし）の場合はメッセージウィンドウ左上
   * マージン位置へフォールバックする。 */
  private updateGlyphPosition(id: number): void {
    let x: number;
    let y: number;
    if (this.glyphCfg.fix) {
      x = this.glyphCfg.left ?? this.cur.cfg.left + this.cur.cfg.width - 40;
      y = this.glyphCfg.top ?? this.cur.cfg.top + this.cur.cfg.height - 40;
    } else {
      const rects = api.text.rects(this.cur.layerId);
      const originX = this.cur.cfg.left + this.cur.cfg.marginX;
      const originY = this.cur.cfg.top + this.cur.cfg.marginY;
      if (rects.length === 0) {
        x = originX;
        y = originY;
      } else {
        const last = rects[rects.length - 1];
        x = originX + last.x + last.w;
        y = originY + last.y;
      }
    }
    api.layer.set(id, { x, y }, "fore");
  }

  /** クリック待ち記号の表示切り替え（[glyph line=] が設定されていない間は no-op）。
   * 表示する際（visible=true）は、その時点のペン位置/固定位置で毎回位置を再計算する
   * （fix=false のペン追従は「クリック待ち開始時」に計算する設計、W7）。 */
  private setGlyphVisible(visible: boolean): void {
    if (this.modernGlyph !== null && !this.glyphCfg.path) {
      if (!visible) this.modernGlyph.hide();
      else if (this.msgVisible && this.cur.visible) {
        const rects = api.text.rects(this.cur.layerId);
        const m = this.cur.cfg;
        const originX = m.left + m.marginX;
        const originY = m.top + m.marginY;
        const last = rects.length > 0 ? rects[rects.length - 1] : null;
        const lineH = Math.round(m.fontSize * 1.6);
        const x = last === null ? originX : originX + last.x + last.w + 4;
        const y = last === null ? originY : originY + last.y + Math.round((lineH - 28) / 2) - 18;
        this.modernGlyph.show(x, y);
      }
      return;
    }
    if (!this.glyphCfg.path) return;
    const id = this.ensureGlyphLayer();
    if (visible) this.updateGlyphPosition(id);
    api.layer.set(id, { visible }, "fore");
  }

  /** [nextskip] のワンショット消費: [l]/[p]/[s] の待ちが 1 回終わるたびに呼ぶ。 */
  private consumeNextSkip(): void {
    if (this.nextSkipOneShot) {
      this.nextSkipOneShot = false;
      this.mode.skip = false;
    }
  }

  /** [nextskip]: 次の [l]/[p]/[s] までスキップモードへ入るワンショット指定。 */
  requestNextSkip(): void {
    this.mode.skip = true;
    this.nextSkipOneShot = true;
  }

  /** [waitclick]: 左クリックを 1 回待つだけ（文章送りの完了状態やモードには影響しない）。 */
  async waitClickOnly(): Promise<void> {
    const r = await this.interruptible(
      api.input.waitFor((e) => e.kind === "click" && e.button === "left" && !this.barBlocks(e.x, e.y)),
    );
    void r;
  }

  /** [waittrig name=...]: 指定名のトリガが `kag.trigger('x')`（KAG3 準拠）または
   * `sys.player.trigger('x')`（後方互換）から発火されるまで待つ。 */
  async waitTrigger(name: string): Promise<void> {
    const p = new Promise<void>((resolve) => {
      const list = this.triggerWaiters.get(name) ?? [];
      list.push(resolve);
      this.triggerWaiters.set(name, list);
    });
    const r = await this.interruptible(p);
    void r;
  }

  /** `[eval exp="kag.trigger('name')"]`（KAG3 準拠）または
   * `[eval exp="sys.player.trigger('name')"]`（後方互換）から呼ぶ想定のトリガ発火。 */
  trigger(name: string): void {
    const waiters = this.triggerWaiters.get(name);
    if (!waiters) return;
    this.triggerWaiters.delete(name);
    for (const w of waiters) w();
  }

  /** [click]/[wheel] 常設フックの設定/解除（[cclick]/[cwheel]）。 */
  setEventClick(storage: string | null, target: string | null): void {
    this.eventJumps.click = { storage, target };
  }
  clearEventClick(): void {
    this.eventJumps.click = null;
  }
  setEventWheel(storage: string | null, target: string | null): void {
    this.eventJumps.wheel = { storage, target };
  }
  clearEventWheel(): void {
    this.eventJumps.wheel = null;
  }

  /** [timeout time= storage= target=]: ms 経過後、**クリック待ち（waitAdvance/waitForJump）中
   * でなければジャンプしない** KAG3 準拠のタイマー（W7 で修正、`docs/implementation-notes.md`
   * 「2026-07-12」参照）。満了時にクリック待ち中でなければ「満了済み」を記録するだけにし、
   * 次に waitAdvance()/waitForJump() へ入った時点で即発火する。[timeout] は 1 回発火型の
   * ため、発火したら自動的に解除する（[ctimeout] を待たない）。クリックで待ちが解決された
   * 場合はタイマーを解除しない（次の待ちで再び満了していれば発火する）。 */
  setTimeoutJump(ms: number, storage: string | null, target: string | null): void {
    this.timeoutJump = { storage, target };
    this.timeoutExpired = false;
    const gen = ++this.timeoutGen;
    void this.watchTimeout(gen, ms);
  }
  clearTimeoutJump(): void {
    this.timeoutJump = null;
    this.timeoutExpired = false;
    this.timeoutGen++;
  }
  private async watchTimeout(gen: number, ms: number): Promise<void> {
    await this.sleepActive(ms, false);
    if (!this.running || this.timeoutGen !== gen || this.timeoutJump === null) return;
    if (this.inAdvanceWait) this.fireTimeoutJump();
    else this.timeoutExpired = true;
  }
  /** 満了済みタイマーを実際に発火する（1 回発火型のため自動解除する）。 */
  private fireTimeoutJump(): void {
    const j = this.timeoutJump;
    if (!j) return;
    this.timeoutJump = null;
    this.timeoutExpired = false;
    this.timeoutGen++;
    this.clearMessage();
    this.requestJump(j.storage, j.target);
  }
  /** waitAdvance()/waitForJump() の入口で呼ぶ: クリック待ち外で満了していたタイマーが
   * あれば、この待ちへ入った時点で即発火する。 */
  private checkTimeoutOnWaitEnter(): void {
    if (this.timeoutExpired && this.timeoutJump !== null) this.fireTimeoutJump();
  }

  /** [ask]（sourceforge 拡張）: 自作の「はい/いいえ」ダイアログ。var が非 null なら
   * 結果 (true/false) を式として書き込む（例: "f.answer"）。ボタン以外の左クリック・Esc は
   * 「いいえ」で閉じる（行の位置を知らない自動入力でもハングしないよう [input] と同じ方針、
   * docs/implementation-notes.md 参照）。 */
  async askYesNo(text: string, varExpr: string | null): Promise<boolean> {
    if (this.uiLock) return false;
    this.lockUi();
    const events = new UiEvents();
    let answer = false;
    try {
      answer = this.modern
        ? await modernConfirm(this.screenW, this.screenH, text || "?", events)
        : await confirmDialog(this.screenW, this.screenH, text || "?", events);
    } finally {
      events.close();
      this.unlockUi();
    }
    if (varExpr) vars.evalStmt(`${varExpr} = ${answer}`, this.sys, this);
    return answer;
  }

  /** [input title= prompt= initial= name=]: 自作テキスト入力モーダル。name は
   * 結果を書き込む変数式（省略時は入力を破棄するだけ）。クリックでも「確定」扱いに
   * している（ヘッドレス環境では文字キーが届かない検証区間があるため。
   * docs/implementation-notes.md 参照）。 */
  async openInputDialog(title: string, prompt: string, initial: string, varExpr: string | null): Promise<void> {
    if (this.uiLock) return;
    this.lockUi();
    try {
      const w = 640;
      const h = 220;
      const left = Math.floor((this.screenW - w) / 2);
      const top = Math.floor((this.screenH - h) / 2);
      const id = api.layer.create("text");
      api.layer.set(id, { x: left, y: top, z: 320, visible: true }, "fore");
      api.text.style(id, {
        fontSize: 22,
        boxW: w - 48,
        boxH: h - 32,
        marginX: 24,
        marginY: 16,
        lineHeight: 1.6,
      });
      const accent: api.RGBA = [255, 216, 120, 255];
      let buffer = initial;
      const redraw = (): void => {
        api.layer.msgbox(id, w, h, 12, [12, 12, 28, 240], "fore");
        api.text.print(
          id,
          [
            { text: title ? title + "\n" : "", color: accent },
            { text: (prompt ? prompt + "\n\n" : "\n"), color: null },
            { text: buffer + "_", color: null },
            { text: "\n\n(クリック/Enter で確定、Esc で取消)", color: [160, 160, 180, 255] },
          ],
          false,
          0,
        );
      };
      redraw();

      let confirmed = true;
      while (true) {
        const ev = await api.input.waitFor(
          (e) =>
            (e.kind === "click" && e.button === "left") ||
            (e.kind === "key" &&
              (e.key === "Enter" || e.key === "Escape" || e.key === "Backspace" || (e.key ?? "").length === 1)),
        );
        if (ev.kind === "click" || ev.key === "Enter") break;
        if (ev.key === "Escape") {
          confirmed = false;
          break;
        }
        buffer = applyEditKey(buffer, ev.key ?? "", 60);
        redraw();
      }
      api.layer.free(id);
      if (confirmed && varExpr) vars.evalStmt(`${varExpr} = ${JSON.stringify(buffer)}`, this.sys, this);
    } finally {
      this.unlockUi();
    }
  }

  /** [bgmopt]/[seopt]: グローバル音量（設定値, 0..100）を更新する。sf.config へ
   * 永続化される settings に「素の %」だけを書き、実効音量は volFor() が一元的に
   * 掛けるため二重掛けにならない。BGM は再生中なら即座に反映する。 */
  setGlobalVolume(kind: "bgm" | "se" | "voice", volPct: number): void {
    const v = clampVolPct(volPct);
    if (kind === "bgm") {
      this.settings.bgmVol = v;
      this.syncBgmVolume();
    } else if (kind === "se") {
      this.settings.seVol = v;
      this.syncSeVolumes();
    } else {
      this.settings.voiceVol = v;
      this.syncVoiceVolume();
    }
  }

  private async idleUntilJump(): Promise<void> {
    while (this.running && !this.jumpRequest) {
      await api.sys.wait(60);
    }
  }

  /** [s] タグ用: ジャンプ要求（リンク選択・ロード等）が来るまで停止する。
   * button/checkbox/edit が活性化するのはこの停止中のみ（KAG 準拠、W4）。
   * [timeout] の発火可否判定用に、この間 inAdvanceWait を true にする（W7）。 */
  async waitForJump(): Promise<void> {
    this.saveSys();
    this.autoSaveAtChoice();
    this.inAdvanceWait = true;
    this.checkTimeoutOnWaitEnter();
    this.buttons.active = true;
    try {
      if (this.choiceButtons) await this.startChoices();
      await this.idleUntilJump();
    } finally {
      this.stopChoices();
      this.buttons.active = false;
      this.inAdvanceWait = false;
    }
    this.consumeNextSkip();
  }

  // ---------- 選択肢ボタン画面（choices.style = "buttons"） ----------

  /** `[s]` で止まった時点のリンク群を画面中央のボタンとして出す。リンクが無ければ何もしない。本文の
   * 文字送りが終わるのを待ってから（待つ間のクリックは文字送りの完了になる）、出現アニメを再生する。 */
  private async startChoices(): Promise<void> {
    const links: Link[] = [];
    const labels: { label: string }[] = [];
    this.msgLayers.forEach((ml) => {
      ml.links.forEach((l) => {
        let t = "";
        for (let i = l.segStart; i < l.segEnd && i < ml.segs.length; i++) t += ml.segs[i].text;
        const label = cleanChoiceLabel(t);
        if (label === "") return;
        links.push(l);
        labels.push({ label });
      });
    });
    if (links.length === 0) return;
    this.choiceWaiting = true;
    try {
      while (!api.text.isDone(this.cur.layerId)) {
        await api.sys.wait(30);
        if (this.jumpRequest !== null) return;
      }
    } finally {
      this.choiceWaiting = false;
    }
    if (this.jumpRequest !== null) return;
    this.choiceLinks = links;
    const ui = new ChoiceScreen(this.screenW, this.screenH, labels, (k) => this.playUiSound(k));
    this.choiceUi = ui;
    ui.setVisible(this.msgVisible);
    await ui.show();
  }

  private stopChoices(): void {
    if (this.choiceUi !== null) {
      this.choiceUi.dispose();
      this.choiceUi = null;
    }
    this.choiceLinks = [];
    this.choiceWaiting = false;
  }

  /** 選ばれた選択肢を実行する（演出 → `exp=` なら式を実行して画面を残す / それ以外はジャンプ）。 */
  private async pickChoice(i: number): Promise<void> {
    const ui = this.choiceUi;
    const link = this.choiceLinks[i];
    if (ui === null || link === undefined) return;
    if (link.exp) {
      await ui.pulse(i);
      try {
        vars.evalStmt(link.exp, this.sys, this);
      } catch (e) {
        api.sys.log(`link exp の実行エラー: ${e}`, "error");
      }
      return;
    }
    await ui.pick(i);
    if (this.choiceUi !== ui) return;
    this.clearMessage();
    this.requestJump(link.storage, link.target);
  }

  // ---------- ジャンプ/コール ----------

  requestJump(storage: string | null, target: string | null, isCall = false): void {
    this.jumpRequest = {
      file: storage ?? this.pc.file,
      label: target ? target.replace(/^\*/, "") : null,
      isCall,
    };
    this.fireInterrupt();
  }

  pushCall(storage: string | null, target: string | null): void {
    this.callStack.push({ file: this.pc.file, index: this.pc.index, section: this.readSection });
    this.readSection = null;
    this.requestJump(storage, target, true);
  }

  popReturn(): void {
    const ret = this.callStack.pop();
    if (!ret) {
      api.sys.log("[return] コールスタックが空です", "error");
      return;
    }
    // サブルーチンの区間はここで読了、呼び出し元の区間（まだ途中）を再開する。
    this.finishSection();
    this.readSection = ret.section;
    this.jumpRequest = null;
    this.pc = { file: ret.file, index: ret.index };
    if (ret.restore) this.restoreMessages(ret.restore);
  }

  // ---------- if/elsif/else ----------

  /**
   * 条件が偽だったとき: 同レベルの次の分岐を探す。
   * elsif は条件をここで評価し、真なら直後から実行を継続する。
   * else なら直後から継続、endif なら抜ける。
   */
  skipToNextBranch(): void {
    const sc = this.scenario(this.pc.file);
    let depth = 0;
    while (this.pc.index < sc.commands.length) {
      const c = sc.commands[this.pc.index++];
      if (c.type !== "tag") continue;
      if (c.name === "if") {
        depth++;
      } else if (c.name === "endif") {
        if (depth === 0) return;
        depth--;
      } else if (depth === 0 && c.name === "elsif") {
        const attrs = this.resolveAttrs(c.attrs);
        if (vars.evalExpr(attrs.exp ?? "false", this.sys, this)) return; // この分岐を実行
      } else if (depth === 0 && c.name === "else") {
        return; // else 分岐を実行
      }
    }
  }

  /** 実行済み分岐の終わりで endif まで飛ぶ */
  skipToEndif(): void {
    const sc = this.scenario(this.pc.file);
    let depth = 0;
    while (this.pc.index < sc.commands.length) {
      const c = sc.commands[this.pc.index];
      this.pc.index++;
      if (c.type === "tag") {
        if (c.name === "if") depth++;
        else if (c.name === "endif") {
          if (depth === 0) return;
          depth--;
        }
      }
    }
  }

  /** [ignore exp=]（W6）: exp が真ならブロックを [endignore] まで読み飛ばす。
   * 実体は純関数 scanIgnoreBlock（if/ignore を同じ depth に含める入れ子走査）。 */
  skipToEndignore(): void {
    const sc = this.scenario(this.pc.file);
    this.pc.index = scanIgnoreBlock(sc.commands, this.pc.index);
  }

  /** [macro name=...] から [endmacro] までを取り込み、pc を先へ進める */
  captureMacro(name: string): void {
    const sc = this.scenario(this.pc.file);
    const body: Command[] = [];
    while (this.pc.index < sc.commands.length) {
      const c = sc.commands[this.pc.index++];
      if (c.type === "tag" && c.name === "endmacro") {
        this.macros.set(name.toLowerCase(), body);
        return;
      }
      body.push(c);
    }
    api.sys.log(`[endmacro] が見つかりません: ${name}`, "error");
  }

  // ---------- リンク（選択肢） ----------

  beginLink(target: string | null, storage: string | null, exp: string | null = null): void {
    const cur = this.cur;
    cur.activeLink = {
      target,
      storage,
      exp,
      start: utf8Len(cur.currentText),
      segStart: cur.segs.length,
    };
  }

  endLink(): void {
    const cur = this.cur;
    if (!cur.activeLink) return;
    cur.links.push({
      target: cur.activeLink.target,
      storage: cur.activeLink.storage,
      exp: cur.activeLink.exp,
      start: cur.activeLink.start,
      end: utf8Len(cur.currentText),
      segStart: cur.activeLink.segStart,
      segEnd: cur.segs.length,
    });
    cur.activeLink = null;
  }

  /** 座標に当たるリンクの index。なければ -1。ヒットテストの原点はカレントレイヤの cfg。 */
  private linkAt(x: number, y: number): number {
    const cur = this.cur;
    if (cur.links.length === 0) return -1;
    const rects = api.text.rects(cur.layerId);
    const originX = cur.cfg.left + cur.cfg.marginX;
    const originY = cur.cfg.top + cur.cfg.marginY;
    for (let i = 0; i < cur.links.length; i++) {
      const link = cur.links[i];
      for (const r of rects) {
        if (r.byte_start < link.start || r.byte_start >= link.end) continue;
        const rx = originX + r.x;
        const ry = originY + r.y;
        if (x >= rx - 4 && x <= rx + r.w + 4 && y >= ry - 4 && y <= ry + r.h + 4) {
          return i;
        }
      }
    }
    return -1;
  }

  private updateHover(x: number, y: number): void {
    const cur = this.cur;
    if (this.uiLock || cur.links.length === 0 || !api.text.isDone(cur.layerId)) return;
    const idx = this.linkAt(x, y);
    if (idx !== cur.hoveredLink) {
      cur.hoveredLink = idx;
      this.repaint();
      if (idx >= 0) this.playUiSound("hover");
    }
  }

  /** キーボード（↑↓）で選択中のリンクがあるか（マウスのホバーも同じ状態を使う）。 */
  private linkFocused(): boolean {
    const cur = this.cur;
    return !this.linksLocked && cur.links.length > 0 && cur.hoveredLink >= 0 && cur.hoveredLink < cur.links.length;
  }

  /** ↑↓ で選択肢のフォーカスを動かす（端はループ。未選択なら ↓ で先頭・↑ で末尾）。 */
  private focusLink(dir: number): void {
    const cur = this.cur;
    if (this.choiceButtons || cur.links.length === 0 || this.linksLocked || !api.text.isDone(cur.layerId)) return;
    const n = cur.links.length;
    const idx = cur.hoveredLink < 0 || cur.hoveredLink >= n ? (dir > 0 ? 0 : n - 1) : (cur.hoveredLink + dir + n) % n;
    cur.hoveredLink = idx;
    this.repaint();
    this.playUiSound("hover");
  }

  /** UI 効果音。hover は sound.hover の指定があるときだけ鳴らす。click は従来どおり click.wav が既定。 */
  playUiSound(kind: "hover" | "click"): void {
    const name = kind === "hover" ? this.soundOpt.hover : this.soundOpt.click ?? "click.wav";
    if (!name) return;
    api.audio.play("se", this.resolveStorage("se", name), { volume: this.volFor("se", 1) });
  }

  /** MenuHost: メニュー内の操作音（クリックは sound.click の指定があるときだけ。従来メニューは無音）。 */
  uiSound(kind: "hover" | "click"): void {
    const name = kind === "hover" ? this.soundOpt.hover : this.soundOpt.click;
    if (!name) return;
    this.playUiSound(kind);
  }

  private tryClickLink(x: number, y: number): void {
    const idx = this.linkAt(x, y);
    if (idx < 0) return;
    this.activateLink(idx);
  }

  private activateLink(idx: number): void {
    const link = this.cur.links[idx];
    if (!link) return;
    this.playUiSound("click");
    if (link.exp) {
      // ジャンプせずに式を実行（メニューを開く等）。リンクは残す。
      try {
        vars.evalStmt(link.exp, this.sys, this);
      } catch (e) {
        api.sys.log(`link exp の実行エラー: ${e}`, "error");
      }
      return;
    }
    const target = link.target;
    const storage = link.storage;
    this.clearMessage();
    this.requestJump(storage, target);
  }

  // ---------- セーブ/ロード ----------

  private sysObject(): Record<string, unknown> {
    return {
      save: (slot: string) => this.save(slot),
      load: (slot: string) => this.load(slot),
      menu: (mode?: string) =>
        void this.openSaveMenu(mode === "load" ? "load" : mode === "settings" ? "settings" : "save"),
      backlog: () => void this.openBacklog(),
      gallery: () => {
        if (!this.uiLock) void this.runGallery();
      },
      quit: () => this.quitGame(),
      player: this,
    };
  }

  get sys(): Record<string, unknown> {
    return this.sysObject();
  }

  makeSnapshot(): SaveData {
    return {
      version: 2,
      pos: { file: this.currentLabel.file, label: this.currentLabel.label },
      f: JSON.parse(JSON.stringify(vars.f)),
      layers: [...this.layerStates.entries()].map(([name, s]) => ({ name, ...s })),
      bgm: this.currentBgm
        ? { ...this.currentBgm, positionMs: api.audio.position(this.bgmCh) }
        : null,
      name: this.cur.nameLabel,
      date: new Date().toLocaleString(),
      title: this.currentLabel.display ?? this.currentLabel.label ?? "",
      msgLayers: this.msgLayers.map((ml) => ({
        cfg: { ...ml.cfg },
        visible: ml.visible,
        nameLabel: ml.nameLabel,
        currentLineStyle: { ...ml.currentLineStyle },
        defaultLineStyle: { ...ml.defaultLineStyle },
      })),
      current: this.current.index,
    };
  }

  /** [tempsave]/[record] 等（W6）: makeSnapshot() に加えて、正確な再開位置
   * (`pos.index`) とカレントレイヤの表示内容 (`curSegs`/`curText`) を含む完全
   * スナップショットを作る。通常の makeSnapshot()（onLabel の自動キャプチャ・[save] が
   * 使う）はラベル境界の記録のみで、ロード時はシナリオをラベルから再生することで表示内容を
   * 再現する（既存の後方互換動作）。tempsave/record はラベル境界とは限らない任意の位置で
   * 呼ばれるため、pos.index による厳密な再開と curSegs/curText による表示内容の直接復元が
   * 必要（`docs/implementation-notes.md`「2026-07-11 W6」参照）。 */
  makeFullSnapshot(): SaveData {
    const base = this.makeSnapshot();
    return {
      ...base,
      pos: { ...base.pos, index: this.pc.index },
      curSegs: [...this.cur.segs],
      curText: this.cur.currentText,
    };
  }

  /** スロットへ保存する（成功なら true）。サムネイルは、メニュー経由ならメニューを開く直前の画面、
   * それ以外（F5・[save]・オートセーブ）は今の画面。保存時刻・本文の抜粋・履歴（保存時点の直近 N 件）も入れる。 */
  save(slot: string): boolean {
    if (this.guardStore("save")) return false;
    if (!this.snapshot) {
      api.sys.log("セーブポイント（ラベル）をまだ通過していません", "warn");
      return false;
    }
    let thumb = false;
    try {
      if (!this.thumbPending) this.captureThumb();
      api.sys.thumbSave(slot);
      thumb = true;
    } catch {
      /* サムネイル失敗は無視（一覧では画像なし表示になる） */
    }
    let savedAt = "";
    try {
      savedAt = api.sys.dateString();
    } catch {
      /* 日時が取れなければ空（一覧は snapshot の date を表示する） */
    }
    // trail は makeSnapshot()/makeFullSnapshot() 自体には含めない（trail エントリが自身の
    // trail を持たないようにするため）。実際にディスクへ書き出す直前にだけ付加する
    // （W7、docs/implementation-notes.md「2026-07-12」参照）。
    const data: SaveData = {
      ...this.snapshot,
      trail: this.trail,
      savedAt,
      excerpt: this.recentExcerpt(),
      thumb,
      backlog: this.snapshotBacklog,
    };
    api.sys.saveWrite(slot, JSON.stringify(data));
    api.sys.log(`セーブしました: ${slot}`);
    return true;
  }

  /** 今の画面をセーブのサムネイルとして撮る。モダン UI は大きいカード用に大きめ（384x216）に撮る。 */
  private captureThumb(): void {
    if (this.modern) api.sys.thumbCapture(384, 216);
    else api.sys.thumbCapture();
  }

  /** セーブ一覧に出す「直近の本文」。選択肢の画面では選択肢以外の直前の段落、それ以外は表示中の本文。 */
  private recentExcerpt(): string {
    let text = "";
    if (this.cur.links.length === 0) text = this.cur.currentText.trim();
    if (text === "") {
      for (let i = this.backlog.length - 1; i >= 0; i--) {
        if (!this.backlog[i].rule) {
          text = this.backlog[i].text;
          break;
        }
      }
    }
    return makeExcerpt(text, 34);
  }

  /** [s] で選択肢が出ているとき、専用スロット "auto" へ自動保存する（失敗してもゲームは止めない）。 */
  private autoSaveAtChoice(): void {
    if (!this.autoSaveOn || this.storeDisabled || this.snapshot === null) return;
    if (!this.msgLayers.some((ml) => ml.links.length > 0)) return;
    try {
      this.save("auto");
    } catch (e) {
      api.sys.log(`オートセーブに失敗しました: ${e}`, "warn");
    }
  }

  quickSave(): void {
    this.toast(this.save("quick") ? "クイックセーブしました" : "クイックセーブできません");
  }

  async quickLoad(): Promise<void> {
    this.toast((await this.load("quick")) ? "クイックロードしました" : "クイックセーブがありません");
  }

  /** 画面上部に約 1.2 秒だけ出す通知（待ちを妨げない）。 */
  toast(text: string): void {
    if (this.modern) {
      if (this.modernToast === null) this.modernToast = new ModernToast(this.screenW);
      this.modernToast.show(text);
      return;
    }
    if (this.toastUi === null) this.toastUi = new Toast(this.screenW);
    // メニュー/履歴の表示中は上端のタブと重ならない位置（カードとフッタの間）に出す。
    this.toastUi.show(text, 1200, this.uiLock ? 606 : 22);
  }

  async load(slot: string): Promise<boolean> {
    if (this.guardStore("load")) return false;
    const json = api.sys.saveRead(slot);
    if (!json) {
      api.sys.log(`セーブデータがありません: ${slot}`, "warn");
      return false;
    }
    let data: SaveData;
    try {
      data = migrateSaveV1toV2(JSON.parse(json) as SaveData);
    } catch (e) {
      api.sys.log(`セーブデータが壊れています: ${slot}: ${e}`, "error");
      return false;
    }
    await this.applySnapshot(data, true);
    api.sys.log(`ロードしました: ${slot}`);
    return true;
  }

  /** [tempload]/[goback]/[gotostart]（W6）が使う、ディスク I/O を伴わないスナップショット
   * 適用の実体。[load] もこれへ委譲する（`pos.index` の有無で「厳密な再開」と「ラベルからの
   * 再生」を切り替える。docs/implementation-notes.md「2026-07-11 W6」参照）。
   * `restoreTrail`: true なら `data.trail`（無ければ空配列、旧セーブとの後方互換）で
   * `this.trail` を丸ごと差し替える（[load] 専用）。false（goback/gotostart/tempload）では
   * `this.trail` に触れない（goback は呼び出し前に自分で pop 済みのため、ここで空配列へ
   * 巻き戻すと trail が消えてしまう。W7、docs/implementation-notes.md「2026-07-12」参照）。 */
  async applySnapshot(data: SaveData, restoreTrail: boolean): Promise<void> {
    data = migrateSaveV1toV2(data);
    this.mode.skip = false;
    this.mode.auto = false;
    // 読み途中の区間は既読にしない（ロードで抜けた）。
    this.readSection = null;
    this.stopVoice();
    this.stopAllSe();
    this.clearAllMessages();

    // レイヤ復元
    for (const l of data.layers) {
      const id = this.layers.get(l.name);
      if (id === undefined) continue;
      const state: LayerState = {
        path: l.path,
        x: l.x,
        y: l.y,
        z: l.z,
        opacity: l.opacity,
        visible: l.visible,
      };
      this.layerStates.set(l.name, state);
      for (const page of ["fore", "back"] as const) {
        api.layer.cancelMove(id, page);
        if (l.path) api.layer.loadImage(id, l.path, page);
        else api.layer.clearImage(id, page);
        api.layer.set(
          id,
          { x: l.x, y: l.y, z: l.z, opacity: l.opacity, visible: l.visible },
          page,
        );
      }
    }

    // メッセージレイヤ構成 + 名前欄プレート (v2)。v1 セーブ（migrateSaveV1toV2 が
    // msgLayers を空配列で補完したもの）は実行中のレイヤ構成に触れず、行スタイルを
    // left/0 に補完して名前欄を復元する（後方互換）。
    if (data.msgLayers && data.msgLayers.length > 0) {
      this.setLayCount(data.msgLayers.length);
      for (let i = 0; i < data.msgLayers.length; i++) {
        const saved = data.msgLayers[i];
        const ml = this.msgLayers[i];
        ml.cfg = { ...ml.cfg, ...saved.cfg };
        ml.visible = saved.visible;
        ml.nameLabel = saved.nameLabel;
        ml.currentLineStyle = normalizeLineStyle(saved.currentLineStyle);
        ml.defaultLineStyle = normalizeLineStyle(saved.defaultLineStyle);
        this.applyMessageLayerConfig(i);
      }
      this.current.index = Math.min(Math.max(data.current ?? 0, 0), this.msgLayers.length - 1);
      this.renderNamePlate();
      this.updateNamePlateVisibility();
    } else {
      for (let i = 0; i < this.msgLayers.length; i++) {
        const ml = this.msgLayers[i];
        ml.currentLineStyle = { ...DEFAULT_LINE_STYLE };
        ml.defaultLineStyle = { ...DEFAULT_LINE_STYLE };
        this.applyMessageLayerTextStyle(i);
      }
      this.setName(data.name ?? null);
    }

    // W6: カレントレイヤの表示内容を復元する（tempsave/record 等、pos.index 付き
    // スナップショットのみが持つ）。無ければ従来通り clearAllMessages() 済みの空状態のまま、
    // シナリオのラベル再生で表示が再現される（既存動作）。
    if (data.curSegs && data.curSegs.length > 0) {
      this.cur.segs = data.curSegs;
      this.cur.currentText = data.curText ?? "";
      api.text.print(this.cur.layerId, data.curSegs, false, 0);
    }

    // W7: [load] のみ trail を丸ごと差し替える（旧セーブ/未定義は空配列で補完）。
    // goback/gotostart/tempload（restoreTrail=false）は this.trail に一切触れない。
    if (restoreTrail) {
      this.trail = data.trail ?? [];
      // 履歴もセーブ時点のものへ戻す（旧セーブは空）。ラベルから再生し直すぶんはこれから積まれる。
      this.backlog = restoreBacklog(data.backlog);
      this.snapshotBacklog = this.backlog.slice(-BACKLOG_SAVE_LIMIT);
      this.paragraph = "";
      this.paragraphName = null;
      this.paragraphVoice = null;
      // ロードしたゲーム進行は常設ボタン・オートセーブが有効な状態から始まる（タイトル用の無効化を持ち越さない）。
      this.sysBarOn = true;
      this.autoSaveOn = true;
      this.backlogEnabled = true;
      this.updateSysBarVisibility();
    }

    // 変数と BGM
    vars.replaceF(data.f ?? {});
    // xchgbgm で旧チャンネル ("bgm2") が残っていても止め、チャンネルを既定へ戻す
    if (this.bgmCh !== "bgm") api.audio.stop(this.bgmCh);
    this.bgmCh = "bgm";
    this.clearBgmStop();
    if (data.bgm) {
      // paused は v1 セーブに無いフィールド → false 補完
      const paused = data.bgm.paused === true;
      const savedPositionMs = data.bgm.positionMs;
      const startMs =
        typeof savedPositionMs === "number" &&
        Number.isFinite(savedPositionMs) &&
        savedPositionMs >= 0
          ? savedPositionMs
          : 0;
      this.currentBgm = { path: data.bgm.path, volume: data.bgm.volume, paused };
      api.audio.play("bgm", data.bgm.path, {
        loop: true,
        volume: this.volFor("bgm", data.bgm.volume),
        fadeInMs: paused ? 0 : 300,
        startMs,
        paused,
      });
    } else {
      api.audio.stop("bgm", 300);
      this.currentBgm = null;
    }

    // W6: pos.index があれば直接ジャンプ（ラベル再生ではなく厳密な再開点）、
    // なければ従来通りラベルへのジャンプ要求（scenario replay-from-label、既存動作）。
    if (data.pos.index !== undefined) {
      this.jumpRequest = null;
      this.pc = { file: data.pos.file, index: data.pos.index };
    } else {
      this.requestJump(data.pos.file, data.pos.label);
    }
  }

  // ---- ストア系ヘルパ (W6) ----

  /** セーブ/ロード系 W6 タグの入口で共通に呼ぶガード。[disablestore] 中なら warn して
   * true を返す（呼び出し側は true が返れば処理を中断する）。save()/load() 自身にも
   * かけているため、F5/F9/メニュー経由の従来のセーブ/ロードも disablestore 中は禁止される。 */
  guardStore(tagName: string): boolean {
    if (this.storeDisabled) {
      api.sys.log(`[${tagName}] disablestore 中のため実行しません`, "warn");
      return true;
    }
    return false;
  }

  /** [disablestore enabled=]（既定 true）。 */
  setStoreDisabled(disabled: boolean): void {
    this.storeDisabled = disabled;
  }

  /** [tempsave place=]: place>=0 はディスクの `"temp{place}"` スロットへ、負数はメモリのみの
   * tempSlots へ保存する（W7、KAG3 準拠。docs/implementation-notes.md「2026-07-12」参照）。 */
  tempSave(place: number): void {
    const data = this.makeFullSnapshot();
    if (place >= 0) {
      api.sys.saveWrite(`temp${place}`, JSON.stringify(data));
    } else {
      this.tempSlots.set(place, data);
    }
  }

  /** [tempload place=]: place>=0 はディスクから、負数はメモリから復元する。データが無ければ
   * false（呼び出し側で warn する）。 */
  async tempLoad(place: number): Promise<boolean> {
    let data: SaveData | undefined;
    if (place >= 0) {
      const json = api.sys.saveRead(`temp${place}`);
      if (json) {
        try {
          data = JSON.parse(json) as SaveData;
        } catch {
          data = undefined;
        }
      }
    } else {
      data = this.tempSlots.get(place);
    }
    if (!data) return false;
    await this.applySnapshot(data, false);
    return true;
  }

  /** [locksnapshot]/[unlocksnapshot]: onLabel() の自動スナップショット更新を止める/戻す。 */
  setSnapshotLocked(locked: boolean): void {
    this.snapshotLocked = locked;
  }

  /** [jump store=false]/[call store=false]: 次に onLabel() を通過する 1 回だけ
   * 自動スナップショット更新をスキップする。 */
  markStoreFalse(): void {
    this.suppressNextAutoSnapshot = true;
  }

  /** [startanchor enabled=]（既定 true）: 実行時点の完全スナップショットをアンカースロットへ
   * 記録する（KAG3 準拠、W7 で修正）。enabled=false はアンカーを破棄する。 */
  setAnchor(enabled: boolean): void {
    this.anchor = enabled ? this.makeFullSnapshot() : null;
  }

  /** [record]: 現在位置を trail へ積む（明示記録）。上限 TRAIL_LIMIT を超えたら古い方から破棄。 */
  pushTrail(): void {
    this.trail.push(this.makeFullSnapshot());
    if (this.trail.length > KagPlayer.TRAIL_LIMIT) this.trail.shift();
  }

  /** [goback]: 直前の trail エントリを消費して戻る。記録が無ければ false。 */
  async goBack(): Promise<boolean> {
    const snap = this.trail.pop();
    if (!snap) return false;
    await this.applySnapshot(snap, false);
    return true;
  }

  /** [gotostart]: [startanchor] で記録したアンカーへ戻る（消費しない）。KAG3 準拠（W7 で
   * 修正、旧実装は trail[0] を使っていた。`docs/implementation-notes.md`「2026-07-12」参照）。
   * アンカーが未設定なら false。 */
  async goToStart(): Promise<boolean> {
    if (!this.anchor) return false;
    await this.applySnapshot(this.anchor, false);
    return true;
  }

  /** [clearsysvar]: sf を全消去した直後に呼ぶ。readLabels の再初期化と設定値の既定復帰、
   * trail のクリアを行う（設計判断、W6。docs/implementation-notes.md 参照）。 */
  resetSysDefaults(): void {
    vars.sf.readLabels = {};
    this.settings = { ...DEFAULT_SETTINGS };
    vars.sf.config = this.settings;
    this.applySettings();
    this.trail = [];
    this.anchor = null;
  }

  /** sf（システム変数）が最後の保存から変わっていれば sys スロットへ書く（変化が無ければ何もしない）。
   * ラベル通過・クリック待ち/[s] への突入・シナリオ遷移・終了・ウィンドウを閉じる操作で呼ぶ。 */
  saveSys(): void {
    let json: string;
    try {
      json = JSON.stringify(vars.sf);
    } catch (e) {
      api.sys.log(`sf をシリアライズできません: ${e}`, "error");
      return;
    }
    if (json === this.sysSavedJson) return;
    try {
      api.sys.saveWrite("sys", json);
      this.sysSavedJson = json;
    } catch (e) {
      api.sys.log(`sys の保存に失敗しました: ${e}`, "error");
    }
  }

  /** ゲーム側からの正規の終了（[close]・sys.quit()）。読み終えた区間を既読にして sf を保存してから終える。 */
  quitGame(): void {
    this.finishSection();
    this.saveSys();
    api.sys.quit();
  }

  // ---------- 常設システムボタン ----------

  private layoutSysBar(): void {
    if (this.sysBar === null) return;
    const m = this.msgLayers[0].cfg;
    this.sysBar.layout(m.left + m.width - 16, m.top);
    this.updateSysBarVisibility();
  }

  private updateSysBarVisibility(): void {
    if (this.sysBar === null) return;
    this.sysBar.setVisible(this.sysBarOn && this.msgVisible && this.msgLayers[0].visible);
  }

  /** [sysbar visible=]: 常設システムボタンの表示を切り替える（タイトル画面などで隠す用）。 */
  setSysBarEnabled(on: boolean): void {
    this.sysBarOn = on;
    this.updateSysBarVisibility();
  }

  /** 常設ボタンの上のクリックか（本文送りにしない判定）。 */
  private barBlocks(x: number, y: number): boolean {
    return this.sysBar !== null && this.sysBar.hitBand(x, y);
  }

  private onSysButton(id: SysButtonId): void {
    switch (id) {
      case "qsave":
        this.quickSave();
        break;
      case "qload":
        void this.quickLoad();
        break;
      case "save":
        void this.openSaveMenu("save");
        break;
      case "load":
        void this.openSaveMenu("load");
        break;
      case "auto":
        this.mode.auto = !this.mode.auto;
        this.afterModeToggle();
        break;
      case "skip":
        this.mode.skip = !this.mode.skip;
        this.afterModeToggle();
        break;
      case "log":
        void this.openBacklog();
        break;
      case "menu":
        void this.openSaveMenu("settings");
        break;
    }
  }

  private afterModeToggle(): void {
    this.syncSysBar();
    this.wakeModeWaiters();
  }

  private syncSysBar(): void {
    if (this.sysBar === null) return;
    this.sysBar.setActive("auto", this.mode.auto);
    this.sysBar.setActive("skip", this.mode.skip);
  }

  // ---------- バックログ UI ----------

  private static readonly BACKLOG_FONT = 22;
  private static readonly BACKLOG_LINE = 1.5;
  private static readonly BACKLOG_TOP = 64;
  private static readonly BACKLOG_LEFT = 80;

  async openBacklog(): Promise<void> {
    if (this.uiLock) return;
    this.flushBacklogPreview();
    if (this.backlog.length === 0) return;
    if (this.modern) {
      this.lockUi();
      try {
        await new ModernBacklog({
          screenW: this.screenW,
          screenH: this.screenH,
          entries: this.backlog,
          playVoice: (path: string) => {
            api.audio.stop("voice");
            api.audio.play("voice", path, { volume: this.volFor("voice", 1) });
          },
          stopVoice: () => {
            api.audio.stop("voice");
            this.voiceActive = false;
            this.currentVoiceVolume = null;
          },
        }).run();
      } finally {
        this.unlockUi();
      }
      return;
    }
    this.lockUi();
    const events = new UiEvents();
    let voiceStarted = false;
    try {
      this.ensureBacklogLayers();
      this.backlogOffset = 0;
      let view = this.renderBacklog();
      api.layer.set(this.backlogLayer!, { visible: true }, "fore");
      this.setBacklogChrome(true);

      while (true) {
        const ev = await events.next();
        if (ev.kind === "wheel") {
          this.backlogOffset = this.clampBacklogOffset(this.backlogOffset + ((ev.delta ?? 0) > 0 ? 1 : -1));
          view = this.renderBacklog();
          continue;
        }
        if (ev.kind === "key") {
          const k = ev.key;
          if (k === "Escape" || k === "h" || k === "H") break;
          let step = 0;
          if (k === "ArrowUp") step = 1;
          else if (k === "ArrowDown") step = -1;
          else if (k === "PageUp") step = Math.max(1, view.count - 1);
          else if (k === "PageDown") step = -Math.max(1, view.count - 1);
          if (step !== 0) {
            this.backlogOffset = this.clampBacklogOffset(this.backlogOffset + step);
            view = this.renderBacklog();
          }
          continue;
        }
        if (ev.kind === "click") {
          if (ev.button === "left") {
            // ボイスつきの行はクリックで再生（閉じない）。それ以外のクリックは閉じる。
            const entry = this.backlogEntryAt(view, ev.x, ev.y);
            if (entry !== null && entry.voice) {
              api.audio.stop("voice");
              api.audio.play("voice", entry.voice, { volume: this.volFor("voice", 1) });
              voiceStarted = true;
              continue;
            }
            break;
          }
          if (ev.button === "right") break;
        }
      }
      api.layer.set(this.backlogLayer!, { visible: false }, "fore");
      this.setBacklogChrome(false);
      if (voiceStarted) {
        api.audio.stop("voice");
        this.voiceActive = false;
        this.currentVoiceVolume = null;
      }
    } finally {
      events.close();
      this.unlockUi();
    }
  }

  /** 履歴の本文オーバーレイ（最初に作る = 従来どおり最初に確保されるレイヤ）と、見出し・スクロールバー。 */
  private ensureBacklogLayers(): void {
    if (this.backlogLayer === null) {
      this.backlogLayer = this.createOverlay([6, 8, 18, 250], 300, {
        fontSize: KagPlayer.BACKLOG_FONT,
        lineHeight: KagPlayer.BACKLOG_LINE,
        boxW: this.screenW - KagPlayer.BACKLOG_LEFT * 2 - 20,
        boxH: this.screenH - KagPlayer.BACKLOG_TOP - 30,
        marginX: KagPlayer.BACKLOG_LEFT,
        marginY: KagPlayer.BACKLOG_TOP,
      });
    }
    if (this.backlogHeader === null) {
      this.backlogHeader = new UiBox({ x: 0, y: 14, w: this.screenW, h: 40 }, 301, {
        fontSize: 20,
        lineHeight: 1.2,
        marginX: KagPlayer.BACKLOG_LEFT,
        marginY: 8,
        color: UI_COLORS.accent,
      });
      this.backlogBar = new UiBox(
        { x: this.screenW - 34, y: KagPlayer.BACKLOG_TOP, w: 8, h: this.screenH - KagPlayer.BACKLOG_TOP - 40 },
        301,
        { radius: 4, bg: UI_COLORS.track },
      );
      this.backlogThumb = new UiBox({ x: this.screenW - 34, y: KagPlayer.BACKLOG_TOP, w: 8, h: 24 }, 302, {
        radius: 4,
        bg: UI_COLORS.fill,
      });
      this.setBacklogChrome(false);
    }
  }

  private setBacklogChrome(visible: boolean): void {
    if (this.backlogHeader) this.backlogHeader.show(visible);
    if (this.backlogBar) this.backlogBar.show(visible);
    if (this.backlogThumb) this.backlogThumb.show(visible);
  }

  private clampBacklogOffset(v: number): number {
    return Math.max(0, Math.min(Math.max(0, this.backlog.length - 1), v));
  }

  /** 履歴 1 件のおおよその行数（名前行 + 本文の折り返し + 区切りの空行）。収まるかの目安で、最終判定は実測。 */
  private backlogEntryLines(e: BacklogEntry): number {
    const perLine = Math.max(
      10,
      Math.floor((this.screenW - KagPlayer.BACKLOG_LEFT * 2 - 40) / KagPlayer.BACKLOG_FONT) - 2,
    );
    let n = e.name ? 1 : 0;
    for (const line of e.text.split("\n")) n += Math.max(1, Math.ceil(codePointCount(line) / perLine));
    return n + 1;
  }

  /** 末尾が backlogOffset 件手前の履歴から、画面に収まる件数ぶんを描く。描いた範囲と各件のバイト範囲を返す。 */
  private renderBacklog(): {
    first: number;
    count: number;
    ranges: { start: number; end: number; entry: BacklogEntry }[];
  } {
    const layer = this.backlogLayer!;
    const total = this.backlog.length;
    const end = total - this.backlogOffset;
    const lineAdvance = KagPlayer.BACKLOG_FONT * KagPlayer.BACKLOG_LINE;
    const capacity = Math.floor((this.screenH - KagPlayer.BACKLOG_TOP - 30) / lineAdvance);
    let first = end;
    let used = 0;
    while (first > 0) {
      const lines = this.backlogEntryLines(this.backlog[first - 1]);
      if (used + lines > capacity && first < end) break;
      used += lines;
      first--;
    }
    while (true) {
      const segs: api.Segment[] = [];
      const ranges: { start: number; end: number; entry: BacklogEntry }[] = [];
      let bytes = 0;
      for (let i = first; i < end; i++) {
        const e = this.backlog[i];
        const start = bytes;
        if (e.name) {
          const head = e.name + (e.voice ? "  ♪" : "") + "\n";
          segs.push({ text: head, color: UI_COLORS.accent });
          bytes += utf8Len(head);
        } else if (e.voice) {
          segs.push({ text: "♪\n", color: UI_COLORS.accent });
          bytes += utf8Len("♪\n");
        }
        const body = e.text + "\n\n";
        segs.push({ text: body, color: e.rule ? UI_COLORS.dim : null });
        bytes += utf8Len(body);
        ranges.push({ start, end: bytes, entry: e });
      }
      api.text.print(layer, segs, false, 0);
      // 末尾（最新）の行が切り捨てられていたら、最古の 1 件を外して描き直す。
      const rects = api.text.rects(layer);
      const lastEnd = rects.length > 0 ? rects[rects.length - 1].byte_end : 0;
      const lastBody = ranges.length > 0 ? ranges[ranges.length - 1].end - 2 : 0;
      if (lastEnd >= lastBody || end - first <= 1) {
        this.updateBacklogChrome(first, end, total);
        return { first, count: end - first, ranges };
      }
      first++;
    }
  }

  private updateBacklogChrome(first: number, end: number, total: number): void {
    if (this.backlogHeader) {
      this.backlogHeader.setText([
        { text: `履歴　${end} / ${total}`, color: UI_COLORS.accent },
        {
          text: "　　ホイール・↑↓・PageUp/Down でスクロール　♪ の行はクリックでボイス再生　Esc・右クリックで閉じる",
          color: UI_COLORS.dim,
        },
      ]);
    }
    if (this.backlogBar && this.backlogThumb) {
      const track = this.backlogBar.rect;
      const h = Math.max(24, Math.round((track.h * (end - first)) / Math.max(1, total)));
      const y = track.y + Math.round(((track.h - h) * first) / Math.max(1, total - (end - first)));
      this.backlogThumb.rect.h = h;
      this.backlogThumb.setBg(UI_COLORS.fill);
      this.backlogThumb.setPos(track.x, y);
    }
  }

  /** 履歴の本文上のクリック位置が属する履歴 1 件（グリフ矩形のバイト位置で引く）。 */
  private backlogEntryAt(
    view: { ranges: { start: number; end: number; entry: BacklogEntry }[] },
    x: number,
    y: number,
  ): BacklogEntry | null {
    if (this.backlogLayer === null) return null;
    const rects = api.text.rects(this.backlogLayer);
    const ox = KagPlayer.BACKLOG_LEFT;
    const oy = KagPlayer.BACKLOG_TOP;
    for (const r of rects) {
      if (x < ox + r.x - 6 || x > ox + r.x + r.w + 6 || y < oy + r.y - 6 || y > oy + r.y + r.h + 6) continue;
      for (const g of view.ranges) {
        if (r.byte_start >= g.start && r.byte_start < g.end) return g.entry;
      }
    }
    return null;
  }

  /** 現在表示中の段落もプレビューとして拾う（クリア前でも履歴に見えるように） */
  private flushBacklogPreview(): void {
    const p = this.paragraph.trim();
    const last = this.backlog[this.backlog.length - 1];
    if (p.length > 0 && (last === undefined || last.text !== p)) {
      this.pushBacklog({ text: p, name: this.paragraphName, voice: this.paragraphVoice });
      this.paragraph = "";
      this.paragraphName = null;
      this.paragraphVoice = null;
    }
  }

  // ---------- モードインジケータ ----------

  /** 画面右上にスキップ/オート状態を出す（150ms ポーリング）。常設ボタンの AUTO/SKIP の強調も同期する。 */
  private async modeIndicatorLoop(): Promise<void> {
    let shown = "";
    while (this.running) {
      const cur = this.mode.skip ? "▶▶ スキップ中" : this.mode.auto ? "▷ オート" : "";
      if (cur !== shown && this.modern) {
        shown = cur;
        if (this.modeBadge === null) this.modeBadge = new ModernModeBadge(this.screenW);
        this.modeBadge.set(this.mode.skip ? "SKIP" : this.mode.auto ? "AUTO" : "");
      } else if (cur !== shown) {
        shown = cur;
        if (this.indicatorLayer === null) {
          this.indicatorLayer = api.layer.create("text");
          api.text.style(this.indicatorLayer, {
            fontSize: 22,
            boxW: 240,
            boxH: 40,
            marginX: 0,
            marginY: 0,
            color: [255, 216, 120, 255],
            outlineWidth: 2,
          });
          api.layer.set(
            this.indicatorLayer,
            { x: this.screenW - 250, y: 14, z: 250, visible: false },
            "fore",
          );
        }
        if (cur) {
          api.text.print(this.indicatorLayer, [{ text: cur, color: null }], false, 0);
          api.layer.set(this.indicatorLayer, { visible: true }, "fore");
        } else {
          api.layer.set(this.indicatorLayer, { visible: false }, "fore");
        }
      }
      this.syncSysBar();
      await api.sys.wait(150);
    }
  }

  // ---------- オーバーレイ UI 共通 ----------

  /** 全画面の半透明テキストオーバーレイを作る（バックログ用） */
  private createOverlay(bg: api.RGBA, z: number, style: api.TextStyle): number {
    const id = api.layer.create("text");
    api.layer.msgbox(id, this.screenW, this.screenH, 0, bg, "fore");
    api.text.style(id, style);
    api.layer.set(id, { z, visible: false }, "fore");
    return id;
  }

  // ---------- システムメニュー（セーブ/ロード/設定） ----------

  /** セーブ/ロード/設定タブのメニューを開く（Esc・右クリック・閉じるで抜ける）。メニューを開く
   * 直前の画面をサムネイルとして取り、メニュー上のセーブにメニューが写り込まないようにする。 */
  async openSaveMenu(initialMode: MenuMode = "save"): Promise<void> {
    if (this.uiLock) return;
    await this.runSaveMenu(initialMode);
  }

  /** メニュー本体（uiLock の確認をしない。タイトル画面が lockUi したまま開くときもこちらを使う）。 */
  private async runSaveMenu(initialMode: MenuMode): Promise<void> {
    api.sys.log("セーブメニューを開きます");
    this.lockUi();
    try {
      try {
        this.captureThumb();
        this.thumbPending = true;
      } catch {
        this.thumbPending = false;
      }
      if (this.modern) await new ModernSystemMenu(this as unknown as MenuHost).run(initialMode);
      else await new SystemMenu(this as unknown as MenuHost).run(initialMode);
    } finally {
      this.thumbPending = false;
      this.unlockUi();
    }
  }

  // ---------- 専用タイトル画面・ギャラリー（ui = "modern"） ----------

  /** `[titlescreen]`: 宣言（KagOptions.titleScreen）どおりの専用タイトル画面を出し、項目が選ばれるまで
   * 止まる（`[s]` と同じくジャンプ要求で次へ進む）。ui="modern" 以外では使えない（`[s]` として動く）。 */
  async runTitleScreen(): Promise<void> {
    const opt = this.titleScreenOpt;
    if (!this.modern || opt === null) {
      api.sys.log("[titlescreen] は ui=\"modern\" と KagOptions.titleScreen の指定が必要です（[s] として動きます）", "warn");
      await this.waitForJump();
      return;
    }
    this.saveSys();
    let result: { storage: string | null; target: string | null } | null = null;
    this.lockUi();
    try {
      result = await new TitleScreen(this.titleHost(), opt).run();
    } finally {
      this.unlockUi();
    }
    if (result !== null) this.requestJump(result.storage, result.target);
  }

  private hasAnySave(): boolean {
    try {
      return api.sys.saveList().some((e) => e.slot === "auto" || e.slot === "quick" || /^save\d+$/.test(e.slot));
    } catch {
      return false;
    }
  }

  private titleHost(): TitleHost {
    return {
      screenW: this.screenW,
      screenH: this.screenH,
      hasSave: () => this.hasAnySave(),
      openLoad: () => this.runSaveMenu("load"),
      openConfig: () => this.runSaveMenu("settings"),
      openGallery: () => this.runGallery(),
      confirmExit: async () => {
        const events = new UiEvents();
        try {
          return await modernConfirm(this.screenW, this.screenH, "ゲームを終了しますか？", events);
        } finally {
          events.close();
        }
      },
      quit: () => this.quitGame(),
      runExp: (exp: string) => {
        try {
          vars.evalStmt(exp, this.sys, this);
        } catch (e) {
          api.sys.log(`title exp の実行エラー: ${e}`, "error");
        }
      },
      jumpPending: () => this.jumpRequest !== null,
      uiSound: (k: "hover" | "click") => this.uiSound(k),
    };
  }

  /** ギャラリー（KagOptions.gallery）を開く。タイトル画面からも `sys.gallery()` からも使う。 */
  async runGallery(): Promise<void> {
    if (this.galleryOpt.length === 0) {
      this.toast("ギャラリーに登録された項目がありません");
      return;
    }
    const sf = vars.sf as Record<string, unknown>;
    this.lockUi();
    try {
      await new GalleryScreen(
        {
          screenW: this.screenW,
          screenH: this.screenH,
          unlocked: (flag: string) => !!sf[flag],
          uiSound: (k: "hover" | "click") => this.uiSound(k),
        },
        this.galleryOpt,
      ).run();
    } finally {
      this.unlockUi();
    }
  }

  get hasTitle(): boolean {
    return this.titleOpt !== null;
  }

  /** MenuHost: 設定値を 1 つ変更し、即時反映して永続化する。 */
  setSetting(key: SettingKey, value: number | boolean): void {
    const s = this.settings as unknown as Record<string, number | boolean>;
    s[key] = value;
    switch (key) {
      case "bgmVol":
        this.syncBgmVolume();
        break;
      case "seVol":
        this.syncSeVolumes();
        // 試し鳴り（ヘッドレス実行では音声スレッドが無いため実質無音）
        this.playUiSound("click");
        break;
      case "voiceVol":
        this.syncVoiceVolume();
        break;
      case "cps":
        this.cps = this.settings.cps;
        break;
      case "autoWaitMs":
        this.autoWaitMs = this.settings.autoWaitMs;
        break;
      case "msgOpacity":
        this.refreshMessageWindows();
        break;
      default:
        break;
    }
    this.saveSys();
  }

  /** MenuHost: 設定を初期状態（ゲームの既定）へ戻す。 */
  resetSettings(): void {
    this.settings = { ...this.defaultSettings };
    vars.sf.config = this.settings;
    this.applySettings();
    this.syncBgmVolume();
    this.syncSeVolumes();
    this.syncVoiceVolume();
    this.refreshMessageWindows();
    this.saveSys();
  }

  /** MenuHost: タイトルへ戻る（KagOptions.title の位置へジャンプ）。メニューを閉じた後にシナリオが再開する。 */
  goTitle(): void {
    const t = this.titleOpt;
    if (t === null) return;
    this.mode.skip = false;
    this.mode.auto = false;
    this.readSection = null;
    this.callStack = [];
    this.stopVoice();
    this.stopAllSe();
    this.clearAllMessages();
    this.requestJump(t.storage, t.target);
  }

  /** MenuHost: ゲームを終了する（読み終えた区間を既読にして sf を保存してから）。 */
  quit(): void {
    this.quitGame();
  }
}
