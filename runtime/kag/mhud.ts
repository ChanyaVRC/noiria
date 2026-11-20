// noiria:kag/mhud — モダン UI の会話画面まわり（話者名・システムボタン列・クリック待ち記号）と
// 選択肢ボタン画面・トースト・確認ダイアログ。値の import は mui.ts / ui.ts / api のみ
// （player.ts → mhud.ts → mui.ts の一方向）。tsint 互換構文のみ。

import * as api from "noiria:api";
import { UiEvents, inRect } from "noiria:kag/ui";
import type { Rect } from "noiria:kag/ui";
import {
  MBox,
  MGroup,
  MHoverBox,
  MC,
  D_FAST,
  D_NORM,
  D_SLOW,
  D_OPEN,
  D_CLOSE,
  D_HOVER,
  EASE_OUT,
  EASE_IN,
  withAlpha,
  sleep,
  seg,
  makeVeil,
  estimateWidth,
} from "noiria:kag/mui";

// ---------- 会話画面の寸法 ----------

/** 下部の帯（暗幕）の高さと、本文カラムの幅（1 行 40 字 ≒ 22px × 40 = 880。台詞は最長 40 字なので、最後の 1〜2 字と」だけが次の行へ落ちない）。 */
export const BAND_H = 320;
export const COL_W = 880;
export const BAND_FONT = 26;
/** 帯の中の本文の開始位置（話者名の行がその上に入る）・下の余白。 */
export const BAND_TEXT_TOP = 104;
export const BAND_TEXT_BOTTOM = 46;
/** 帯の暗幕が上端から本文位置までに滑らかに濃くなる距離。 */
export const BAND_RAMP = 128;

/** 帯（メッセージ窓）のパネル。alpha が 100% 時の下端の濃さ（窓の濃さ設定は alpha に掛ける）。 */
export function bandPanel(color: api.RGBA): {
  top: api.RGBA;
  bottom: api.RGBA;
  ramp: number;
} {
  return {
    top: [color[0], color[1], color[2], 0],
    bottom: [color[0], color[1], color[2], color[3]],
    ramp: BAND_RAMP,
  };
}

// ---------- 話者名 ----------

/** 本文カラムの左上に置く話者名（アクセント色 + 細いアクセントバー + 右へ消える細線）。独立した箱にしない。 */
export class ModernNameTag {
  private bar: MBox;
  private text: MBox;
  private rule: MBox;
  private shown: string | null = null;
  private visible = false;
  private left: number;
  private bandTop: number;
  private colW: number;
  private textColor: api.RGBA;

  constructor(left: number, bandTop: number, colW: number, textColor: api.RGBA) {
    this.left = left;
    this.bandTop = bandTop;
    this.colW = colW;
    this.textColor = textColor;
    const x = left;
    const y = bandTop + 60;
    this.bar = new MBox({ x, y: y + 3, w: 3, h: 24 }, 105, {
      panel: { top: MC.accent, radius: 1.5 },
      persist: true,
      visible: false,
    });
    this.text = new MBox({ x: x + 16, y, w: 360, h: 32 }, 105, {
      fontSize: 22,
      lineHeight: 1.3,
      color: textColor,
      outlineWidth: 1,
      outlineColor: [0, 0, 0, 120],
      persist: true,
      visible: false,
    });
    this.rule = new MBox({ x: x + 16, y: y + 33, w: 100, h: 1 }, 105, {
      panel: { top: withAlpha(MC.accent, 120), bottom: withAlpha(MC.accent, 0), horizontal: true },
      persist: true,
      visible: false,
    });
  }

  /** 位置の基準（帯の位置・本文カラムの左端）が変わったとき。 */
  relayout(left: number, bandTop: number, colW: number): void {
    if (left === this.left && bandTop === this.bandTop && colW === this.colW) return;
    this.left = left;
    this.bandTop = bandTop;
    this.colW = colW;
    this.shown = null;
  }

  /** 名前を切り替える（null で非表示）。名前が変わったときだけ 160ms で滑り込む。 */
  set(name: string | null, parentVisible: boolean): void {
    this.visible = name !== null;
    if (name === null) {
      this.shown = null;
      this.apply(false);
      return;
    }
    if (name !== this.shown) {
      this.shown = name;
      const x = this.left;
      const y = this.bandTop + 60;
      this.text.setText([seg(name, this.textColor, 22, true)]);
      const w = estimateWidth(name, 22) + 6;
      this.text.rect.w = w;
      const ruleW = Math.max(60, this.colW - 16 - w - 20);
      this.rule.resizePanel(ruleW, 1);
      this.rule.rect.x = x + 16 + w + 12;
      this.rule.put(this.rule.rect.x, y + 15, 0);
      this.bar.put(x - 10, y + 3, 0);
      this.text.put(x + 6, y, 0);
      if (parentVisible) {
        this.apply(true);
        this.bar.go(x, y + 3, 255, D_NORM, EASE_OUT);
        this.text.go(x + 16, y, 255, D_NORM, EASE_OUT);
        this.rule.go(this.rule.rect.x, y + 15, 255, D_SLOW, EASE_OUT);
      }
    } else {
      this.apply(parentVisible);
    }
  }

  private apply(v: boolean): void {
    this.bar.show(v);
    this.text.show(v);
    this.rule.show(v);
  }

  free(): void {
    this.bar.free();
    this.text.free();
    this.rule.free();
  }
}

// ---------- 帯の下端のシステムボタン列 ----------

export type SysButtonKey = "qsave" | "qload" | "save" | "load" | "auto" | "skip" | "log" | "menu";

const SYS_LABEL: Record<SysButtonKey, string> = {
  qsave: "Q.Save",
  qload: "Q.Load",
  save: "Save",
  load: "Load",
  auto: "Auto",
  skip: "Skip",
  log: "Log",
  menu: "Config",
};

const SYS_FONT = 15;
const SYS_GAP = 24;
const SYS_H = 22;

interface SysItem {
  id: SysButtonKey;
  label: string;
  text: MBox | null;
  line: MBox | null;
  rect: Rect;
  active: boolean;
  hover: boolean;
}

/** 帯の下端右寄りに並ぶ小さなテキストボタン列。通常は低コントラスト、ホバーで明るくなり下線が出る、
 * Auto/Skip の ON はアクセント色。`SystemBar`（ui.ts）と同じ面を持つので player は差し替えて使う。 */
export class ModernSystemBar {
  private items: SysItem[];
  private shown = false;
  private band: Rect = { x: 0, y: 0, w: 0, h: 0 };
  private z: number;
  private right = 0;
  private bottom = 0;

  constructor(ids: SysButtonKey[], z: number) {
    this.z = z;
    this.items = ids.map((id) => ({
      id,
      label: SYS_LABEL[id],
      text: null,
      line: null,
      rect: { x: 0, y: 0, w: 0, h: 0 },
      active: false,
      hover: false,
    }));
  }

  /** 帯の位置（上端 windowTop・右端 right は窓の右端）に合わせて並べ直す。ボタン列は帯の下端右寄り。 */
  layout(right: number, windowTop: number): void {
    this.right = right - 32;
    this.bottom = windowTop + BAND_H - 24;
    const widths = this.items.map((it) => estimateWidth(it.label, SYS_FONT) + 2);
    let total = 0;
    widths.forEach((w) => {
      total += w;
    });
    total += SYS_GAP * Math.max(0, this.items.length - 1);
    let x = this.right - total;
    const y = this.bottom - SYS_H;
    this.items.forEach((it, i) => {
      const w = widths[i];
      it.rect = { x: x - 8, y: y - 6, w: w + 16, h: SYS_H + 12 };
      if (it.text === null) {
        it.text = new MBox({ x, y, w: w + 6, h: SYS_H }, this.z, {
          fontSize: SYS_FONT,
          lineHeight: 1.3,
          color: this.colorOf(it),
          outlineWidth: 1,
          outlineColor: [0, 0, 0, 110],
          persist: true,
          visible: this.shown,
        });
        it.line = new MBox({ x, y: y + SYS_H + 1, w, h: 1 }, this.z, {
          panel: { top: MC.accent },
          persist: true,
          visible: this.shown,
          crop: 0,
        });
        this.paint(it);
      } else {
        it.text.rect.x = x;
        it.text.rect.y = y;
        it.text.put(x, y, 255);
        if (it.line !== null) {
          it.line.rect.x = x;
          it.line.rect.y = y + SYS_H + 1;
          it.line.put(x, y + SYS_H + 1, 255);
          it.line.setCrop(it.active || it.hover ? it.line.rect.w : 0);
        }
      }
      x += w + SYS_GAP;
    });
    const first = this.items[0].rect;
    const last = this.items[this.items.length - 1].rect;
    this.band = { x: first.x, y: first.y, w: last.x + last.w - first.x, h: first.h };
  }

  private colorOf(it: SysItem): api.RGBA {
    if (it.active) return MC.accent;
    return it.hover ? MC.text : withAlpha(MC.textSub, 176);
  }

  private paint(it: SysItem): void {
    if (it.text === null) return;
    it.text.setText([seg(it.label, this.colorOf(it), SYS_FONT)]);
    if (it.line !== null) {
      const on = it.active || it.hover;
      const r = it.line.rect;
      it.line.go(r.x, r.y, 255, D_NORM, on ? EASE_OUT : EASE_IN, on ? r.w : 0);
    }
  }

  setVisible(v: boolean): void {
    this.shown = v;
    this.items.forEach((it) => {
      if (it.text !== null) it.text.show(v);
      if (it.line !== null) it.line.show(v);
    });
  }

  setActive(id: SysButtonKey, on: boolean): void {
    this.items.forEach((it) => {
      if (it.id === id && it.active !== on) {
        it.active = on;
        this.paint(it);
      }
    });
  }

  hitButton(x: number, y: number): SysButtonKey | null {
    if (!this.shown) return null;
    for (let i = 0; i < this.items.length; i++) {
      if (inRect(this.items[i].rect, x, y)) return this.items[i].id;
    }
    return null;
  }

  hitBand(x: number, y: number): boolean {
    if (!this.shown || this.items.length === 0) return false;
    return inRect(this.band, x, y);
  }

  hover(x: number, y: number): void {
    const over = this.hitButton(x, y);
    this.items.forEach((it) => {
      const h = it.id === over;
      if (it.hover !== h) {
        it.hover = h;
        this.paint(it);
      }
    });
  }
}

// ---------- クリック待ち記号 ----------

/** 本文の末尾に出る小さな ▼。ゆっくり上下しながら明滅する。位相は「仮想/実時間の絶対時刻」から決めて 32ms
 * ごとに更新する（開始時刻から数えると、エンジン間で 1 tick ずれうる待ちの後でフレームがずれるため）。 */
/** メニュー/バックログを開いている間は、下の ▼ を動かさない（動かすと見えないのに毎周期描き直しになる）。 */
let glyphPaused = false;
export function setGlyphPaused(v: boolean): void {
  glyphPaused = v;
}

export class ModernGlyph {
  private box: MBox;
  private gen = 0;

  constructor(z: number) {
    this.box = new MBox({ x: 0, y: 0, w: 28, h: 28 }, z, {
      fontSize: 16,
      lineHeight: 1.2,
      color: MC.accent,
      align: "center",
      persist: false,
      visible: false,
    });
    this.box.setText([seg("▼", MC.accent, 15)]);
  }

  show(x: number, y: number): void {
    const gen = ++this.gen;
    this.box.put(x, y, 255);
    this.box.show(true);
    void this.loop(gen, x, y);
  }

  private async loop(gen: number, x: number, y: number): Promise<void> {
    const STEP = 32;
    const PERIOD = 1280;
    while (this.gen === gen) {
      const now = api.sys.now();
      const k = Math.floor((now + 2) / STEP);
      const ph = ((k * STEP) % PERIOD) / PERIOD;
      // 三角波を smoothstep でなだらかにする（tsint の Math に cos は無い）
      const tri = ph < 0.5 ? ph * 2 : (1 - ph) * 2;
      const wave = tri * tri * (3 - 2 * tri);
      if (!glyphPaused) this.box.put(x, y + Math.round(wave * 6), Math.round(255 - wave * 165));
      // 次の境界の少し手前で起きる（境界は tick の倍数なので、同じ tick で起きる）
      await sleep(Math.max(1, Math.ceil((k + 1) * STEP - now) - 4));
    }
  }

  hide(): void {
    this.gen++;
    this.box.show(false);
  }

  free(): void {
    this.gen++;
    this.box.free();
  }
}

// ---------- 選択肢ボタン画面 ----------

export interface ChoiceItem {
  label: string;
}

const CH_W = 720;
const CH_H = 64;
const CH_GAP = 16;
const CH_CENTER_Y = 322;

/** `[link]` 群を画面中央に縦に並ぶ横長ボタンとして出す。背景を少し暗くし、ボタンは上から順に遅れて
 * フェード + スライドインする。ホバー/キー操作で枠・左端バーがアクセント色になり、選択時は選ばれた 1 つが
 * 光って他がフェードアウトする。UI の見た目だけを持ち、ジャンプなど意味づけは呼び出し側（player）が行う。 */
export class ChoiceScreen {
  private veil: MBox;
  private buttons: MHoverBox[] = [];
  private focus = -1;
  private visible = true;
  private ready = false;
  private picking = false;
  private onSound: (kind: "hover" | "click") => void;

  constructor(
    screenW: number,
    screenH: number,
    items: ChoiceItem[],
    onSound: (kind: "hover" | "click") => void,
  ) {
    this.onSound = onSound;
    this.veil = makeVeil(screenW, screenH, 90, 84, 118);
    const n = items.length;
    const total = n * CH_H + (n - 1) * CH_GAP;
    const x = Math.floor((screenW - CH_W) / 2);
    const y0 = Math.floor(CH_CENTER_Y - total / 2);
    items.forEach((it, i) => {
      const rect = { x, y: y0 + i * (CH_H + CH_GAP), w: CH_W, h: CH_H };
      const b = new MHoverBox(
        rect,
        150 + i * 3,
        {
          panel: {
            radius: 6,
            top: [28, 35, 74, 238],
            bottom: [14, 19, 46, 244],
            border: [255, 255, 255, 52],
            borderW: 1,
            shadow: { blur: 22, dy: 8, alpha: 0.42 },
          },
          fontSize: 24,
          lineHeight: 1.3,
          align: "center",
          marginY: Math.floor((CH_H - 31) / 2),
          outlineWidth: 0,
          visible: false,
        },
        {
          radius: 6,
          top: [238, 198, 114, 40],
          bottom: [238, 198, 114, 16],
          border: MC.accent,
          borderW: 1.5,
        },
        4,
      );
      b.setText([seg(it.label, MC.text, 24)]);
      this.buttons.push(b);
    });
  }

  /** 出現アニメを始め、最後のボタンが現れ終わるまで待つ。 */
  async show(): Promise<void> {
    this.veil.show(true);
    this.veil.go(0, 0, 255, D_OPEN, EASE_OUT);
    for (let i = 0; i < this.buttons.length; i++) {
      this.buttons[i].appear(0, 14, D_OPEN + 30);
      await sleep(30);
    }
    this.ready = true;
  }

  setVisible(v: boolean): void {
    this.visible = v;
    this.veil.show(v);
    this.buttons.forEach((b) => b.show(v));
  }

  private indexAt(x: number, y: number): number {
    for (let i = 0; i < this.buttons.length; i++) {
      if (inRect(this.buttons[i].rect, x, y)) return i;
    }
    return -1;
  }

  private setFocus(i: number): void {
    if (i === this.focus) return;
    this.focus = i;
    this.buttons.forEach((b, k) => b.setHover(k === i));
    if (i >= 0) this.onSound("hover");
  }

  /** 入力を処理する。戻り値 = このイベントを消費したか、picked = 選ばれた項目（なければ null）。 */
  handle(ev: api.InputEvent): { consumed: boolean; picked: number | null } {
    if (!this.visible || this.picking || !this.ready) return { consumed: false, picked: null };
    if (ev.kind === "move") {
      const i = this.indexAt(ev.x, ev.y);
      if (i >= 0) this.setFocus(i);
      else if (this.focus >= 0 && ev.x >= 0) this.setFocus(-1);
      return { consumed: false, picked: null };
    }
    if (ev.kind === "click" && ev.button === "left") {
      const i = this.indexAt(ev.x, ev.y);
      if (i >= 0) return { consumed: true, picked: i };
      return { consumed: false, picked: null };
    }
    if (ev.kind === "key") {
      const n = this.buttons.length;
      if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
        const dir = ev.key === "ArrowDown" ? 1 : -1;
        const next = this.focus < 0 ? (dir > 0 ? 0 : n - 1) : (this.focus + dir + n) % n;
        this.setFocus(next);
        return { consumed: true, picked: null };
      }
      if ((ev.key === "Enter" || ev.key === " ") && this.focus >= 0) {
        return { consumed: true, picked: this.focus };
      }
    }
    return { consumed: false, picked: null };
  }

  get focused(): number {
    return this.focus;
  }

  /** 選択演出: 選ばれたボタンが光り、他はフェードアウトし、最後に全体が消える。完了まで待つ。 */
  async pick(i: number): Promise<void> {
    this.picking = true;
    this.onSound("click");
    this.buttons.forEach((b, k) => {
      if (k === i) b.flash();
      else b.vanish(0, 8, D_NORM);
    });
    await sleep(D_NORM + 60);
    this.buttons[i].vanish(0, -6, D_NORM);
    this.veil.go(0, 0, 0, D_NORM, EASE_IN);
    await sleep(D_NORM);
  }

  /** `exp=` リンクのように画面を残したまま、押されたことだけ見せる。 */
  async pulse(i: number): Promise<void> {
    this.picking = true;
    this.onSound("click");
    this.buttons[i].flash();
    await sleep(D_NORM);
    this.buttons[i].setHover(false);
    this.focus = -1;
    this.picking = false;
  }

  dispose(): void {
    this.veil.free();
    this.buttons.forEach((b) => b.free());
    this.buttons = [];
  }
}

// ---------- オート/スキップ中のバッジ ----------

/** 画面右上の小さな丸いバッジ（AUTO / SKIP）。出入りはフェード + 数 px のスライド。 */
export class ModernModeBadge {
  private box: MBox | null = null;
  private dot: MBox | null = null;
  private screenW: number;

  constructor(screenW: number) {
    this.screenW = screenW;
  }

  /** label が空なら消す。 */
  set(label: string): void {
    this.free();
    if (label === "") return;
    const w = estimateWidth(label, 14) + 52;
    const x = this.screenW - 48 - w;
    const y = 26;
    const box = new MBox({ x, y, w, h: 32 }, 250, {
      panel: {
        radius: 16,
        top: [28, 35, 74, 226],
        bottom: [14, 19, 46, 232],
        border: withAlpha(MC.accent, 140),
        borderW: 1,
        shadow: { blur: 12, dy: 4, alpha: 0.35 },
      },
      fontSize: 14,
      lineHeight: 1.3,
      marginX: 32,
      marginR: 12,
      marginY: 6,
      opacity: 0,
    });
    box.setText([seg(label, MC.accent, 14, true)]);
    const dot = new MBox({ x: x + 15, y: y + 13, w: 6, h: 6 }, 251, { panel: { top: MC.accent, radius: 3 }, opacity: 0 });
    box.put(x, y - 8, 0);
    box.go(x, y, 255, D_NORM, EASE_OUT);
    dot.put(x + 15, y + 13 - 8, 0);
    dot.go(x + 15, y + 13, 255, D_NORM, EASE_OUT);
    this.box = box;
    this.dot = dot;
  }

  free(): void {
    if (this.box !== null) this.box.free();
    if (this.dot !== null) this.dot.free();
    this.box = null;
    this.dot = null;
  }
}

// ---------- トースト ----------

/** 画面上部中央に短くスライドインする通知。待ち（[wait] 等）を妨げない（自前のタイマーで消える）。 */
export class ModernToast {
  private box: MBox | null = null;
  private dot: MBox | null = null;
  private gen = 0;
  private screenW: number;

  constructor(screenW: number) {
    this.screenW = screenW;
  }

  show(text: string, ms: number = 1400): void {
    this.hide();
    const gen = ++this.gen;
    const w = Math.min(this.screenW - 80, estimateWidth(text, 18) + 64);
    const h = 44;
    const x = Math.floor((this.screenW - w) / 2);
    const y = 28;
    const box = new MBox({ x, y, w, h }, 400, {
      panel: {
        radius: 22,
        top: [28, 35, 74, 236],
        bottom: [14, 19, 46, 240],
        border: withAlpha(MC.accent, 120),
        borderW: 1,
        shadow: { blur: 16, dy: 6, alpha: 0.4 },
      },
      fontSize: 18,
      lineHeight: 1.3,
      align: "center",
      marginX: 18,
      marginR: 8,
      marginY: 10,
      visible: true,
      opacity: 0,
    });
    box.setText([seg(text, MC.text, 18)]);
    const dot = new MBox({ x: x + 20, y: y + 19, w: 6, h: 6 }, 401, {
      panel: { top: MC.accent, radius: 3 },
      opacity: 0,
    });
    this.box = box;
    this.dot = dot;
    box.put(x, y - 18, 0);
    box.go(x, y, 255, D_NORM, EASE_OUT);
    dot.put(x + 20, y + 19 - 18, 0);
    dot.go(x + 20, y + 19, 255, D_NORM, EASE_OUT);
    void this.expire(gen, ms);
  }

  private async expire(gen: number, ms: number): Promise<void> {
    await sleep(ms);
    if (this.gen !== gen) return;
    if (this.box !== null) this.box.go(this.box.rect.x, this.box.rect.y - 10, 0, D_NORM, EASE_IN);
    if (this.dot !== null) this.dot.go(this.dot.rect.x, this.dot.rect.y - 10, 0, D_NORM, EASE_IN);
    await sleep(D_NORM + 20);
    if (this.gen === gen) this.hide();
  }

  hide(): void {
    this.gen++;
    if (this.box !== null) {
      this.box.free();
      this.box = null;
    }
    if (this.dot !== null) {
      this.dot.free();
      this.dot = null;
    }
  }

  get visible(): boolean {
    return this.box !== null;
  }
}

// ---------- 確認ダイアログ ----------

export interface ModernConfirmOpts {
  yesLabel?: string;
  noLabel?: string;
  defaultYes?: boolean;
}

/** 中央の小さなカード + 背景ディム。主ボタン（はい）はアクセント塗り、副ボタンは枠のみ。ボタン以外の
 * 左クリックと Esc は「いいえ」（[ask] の既存方針）。←→↑↓/Tab でフォーカス、Enter/Space で決定。 */
export async function modernConfirm(
  screenW: number,
  screenH: number,
  text: string,
  events: UiEvents,
  opts: ModernConfirmOpts = {},
): Promise<boolean> {
  const g = new MGroup();
  try {
    const w = 560;
    const h = 232;
    const x = Math.floor((screenW - w) / 2);
    const y = Math.floor((screenH - h) / 2);
    const veil = g.add(makeVeil(screenW, screenH, 330, 150, 170));
    const card = g.add(
      new MBox({ x, y, w, h }, 331, {
        panel: {
          radius: 10,
          top: [28, 35, 76, 246],
          bottom: [14, 19, 46, 250],
          border: [255, 255, 255, 56],
          borderW: 1,
          shadow: { blur: 30, dy: 12, alpha: 0.55 },
        },
        fontSize: 22,
        lineHeight: 1.6,
        align: "center",
        marginX: 32,
        marginY: 38,
        opacity: 0,
      }),
    );
    card.setText([seg(text || "?", MC.text, 22)]);
    const bw = 188;
    const bh = 46;
    const by = y + h - bh - 28;
    const yesRect = { x: x + w / 2 - bw - 12, y: by, w: bw, h: bh };
    const noRect = { x: x + w / 2 + 12, y: by, w: bw, h: bh };
    const yes = g.add(
      new MBox(yesRect, 332, {
        panel: { radius: 6, top: MC.accent, bottom: [214, 170, 88, 255] },
        fontSize: 20,
        align: "center",
        marginY: 8,
        opacity: 0,
      }),
    );
    yes.setText([seg(opts.yesLabel ?? "はい", [28, 22, 8, 255], 20, true)]);
    const no = g.add(
      new MBox(noRect, 332, {
        panel: { radius: 6, top: [0, 0, 0, 0], border: MC.lineStrong, borderW: 1 },
        fontSize: 20,
        align: "center",
        marginY: 8,
        opacity: 0,
      }),
    );
    no.setText([seg(opts.noLabel ?? "いいえ", MC.text, 20)]);
    const yesFocus = g.add(
      new MBox(yesRect, 333, {
        panel: { radius: 6, top: [255, 255, 255, 0], border: [255, 255, 255, 230], borderW: 2 },
        opacity: 0,
      }),
    );
    const noFocus = g.add(
      new MBox(noRect, 333, {
        panel: { radius: 6, top: [238, 198, 114, 26], border: MC.accent, borderW: 1.5 },
        opacity: 0,
      }),
    );
    veil.show(true);
    veil.go(0, 0, 255, D_OPEN, EASE_OUT);
    card.put(x, y + 10, 0);
    card.go(x, y, 255, D_OPEN, EASE_OUT);
    yes.put(yesRect.x, yesRect.y + 10, 0);
    yes.go(yesRect.x, yesRect.y, 255, D_OPEN, EASE_OUT);
    no.put(noRect.x, noRect.y + 10, 0);
    no.go(noRect.x, noRect.y, 255, D_OPEN, EASE_OUT);
    let focusYes = opts.defaultYes === true;
    const paint = (): void => {
      yesFocus.go(yesRect.x, yesRect.y, focusYes ? 255 : 0, D_HOVER, EASE_OUT);
      noFocus.go(noRect.x, noRect.y, focusYes ? 0 : 255, D_HOVER, EASE_OUT);
    };
    paint();
    events.clear();
    let result = false;
    while (true) {
      const ev = await events.next();
      if (ev.kind === "move") {
        const onYes = inRect(yesRect, ev.x, ev.y);
        const onNo = inRect(noRect, ev.x, ev.y);
        if ((onYes || onNo) && onYes !== focusYes) {
          focusYes = onYes;
          paint();
        }
      } else if (ev.kind === "click") {
        if (ev.button === "left") {
          result = inRect(yesRect, ev.x, ev.y);
          break;
        }
        if (ev.button === "right") break;
      } else if (ev.kind === "key") {
        const k = ev.key;
        if (k === "Escape") break;
        if (k === "Enter" || k === " ") {
          result = focusYes;
          break;
        }
        if (k === "ArrowLeft" || k === "ArrowRight" || k === "ArrowUp" || k === "ArrowDown" || k === "Tab") {
          focusYes = !focusYes;
          paint();
        }
      }
    }
    card.go(x, y + 6, 0, D_CLOSE, EASE_IN);
    yes.go(yesRect.x, yesRect.y + 6, 0, D_CLOSE, EASE_IN);
    no.go(noRect.x, noRect.y + 6, 0, D_CLOSE, EASE_IN);
    yesFocus.go(yesRect.x, yesRect.y, 0, D_CLOSE, EASE_IN);
    noFocus.go(noRect.x, noRect.y, 0, D_CLOSE, EASE_IN);
    veil.go(0, 0, 0, D_CLOSE, EASE_IN);
    await sleep(D_CLOSE + 10);
    return result;
  } finally {
    g.freeAll();
  }
}
