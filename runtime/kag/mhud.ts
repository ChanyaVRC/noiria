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
  BASE_H,
  uiScale,
  sc,
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

export interface BandMetrics {
  s: number;
  bandH: number;
  colW: number;
  bandFont: number;
  textTop: number;
  textBottom: number;
  ramp: number;
}

export function bandMetrics(screenW: number, screenH: number): BandMetrics {
  const s = uiScale(screenW, screenH);
  return {
    s,
    bandH: sc(BAND_H, s),
    colW: sc(COL_W, s),
    bandFont: sc(BAND_FONT, s),
    textTop: sc(BAND_TEXT_TOP, s),
    textBottom: sc(BAND_TEXT_BOTTOM, s),
    ramp: sc(BAND_RAMP, s),
  };
}

/** 帯（メッセージ窓）のパネル。alpha が 100% 時の下端の濃さ（窓の濃さ設定は alpha に掛ける）。 */
export function bandPanel(color: api.RGBA, ramp: number = BAND_RAMP): {
  top: api.RGBA;
  bottom: api.RGBA;
  ramp: number;
} {
  return {
    top: [color[0], color[1], color[2], 0],
    bottom: [color[0], color[1], color[2], color[3]],
    ramp,
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
  private s: number;

  constructor(left: number, bandTop: number, colW: number, textColor: api.RGBA, s: number = 1) {
    this.left = left;
    this.bandTop = bandTop;
    this.colW = colW;
    this.textColor = textColor;
    this.s = s;
    const x = left;
    const y = bandTop + sc(60, s);
    const ruleH = Math.max(1, sc(1, s));
    this.bar = new MBox({ x, y: y + sc(3, s), w: sc(3, s), h: sc(24, s) }, 105, {
      panel: { top: MC.accent, radius: 1.5 * s },
      persist: true,
      visible: false,
    });
    this.text = new MBox({ x: x + sc(16, s), y, w: sc(360, s), h: sc(32, s) }, 105, {
      fontSize: sc(22, s),
      lineHeight: 1.3,
      color: textColor,
      outlineWidth: Math.max(1, sc(1, s)),
      outlineColor: [0, 0, 0, 120],
      persist: true,
      visible: false,
    });
    this.rule = new MBox({ x: x + sc(16, s), y: y + sc(33, s), w: sc(100, s), h: ruleH }, 105, {
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
      const s = this.s;
      const x = this.left;
      const y = this.bandTop + sc(60, s);
      const fs = sc(22, s);
      const ruleH = Math.max(1, sc(1, s));
      this.text.setText([seg(name, this.textColor, fs, true)]);
      const w = estimateWidth(name, fs) + sc(6, s);
      this.text.rect.w = w;
      const ruleW = Math.max(sc(60, s), this.colW - sc(16, s) - w - sc(20, s));
      this.rule.resizePanel(ruleW, ruleH);
      this.rule.rect.x = x + sc(16, s) + w + sc(12, s);
      this.rule.put(this.rule.rect.x, y + sc(15, s), 0);
      this.bar.put(x - sc(10, s), y + sc(3, s), 0);
      this.text.put(x + sc(6, s), y, 0);
      if (parentVisible) {
        this.apply(true);
        this.bar.go(x, y + sc(3, s), 255, D_NORM, EASE_OUT);
        this.text.go(x + sc(16, s), y, 255, D_NORM, EASE_OUT);
        this.rule.go(this.rule.rect.x, y + sc(15, s), 255, D_SLOW, EASE_OUT);
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
  private s: number;
  private right = 0;
  private bottom = 0;

  constructor(ids: SysButtonKey[], z: number, s: number = 1) {
    this.z = z;
    this.s = s;
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
    const s = this.s;
    const sysFont = sc(SYS_FONT, s);
    const sysGap = sc(SYS_GAP, s);
    const sysH = sc(SYS_H, s);
    const lineH = Math.max(1, sc(1, s));
    this.right = right - sc(32, s);
    this.bottom = windowTop + sc(BAND_H, s) - sc(24, s);
    const widths = this.items.map((it) => Math.ceil(estimateWidth(it.label, sysFont) * 1.12) + sc(4, s));
    let total = 0;
    widths.forEach((w) => {
      total += w;
    });
    total += sysGap * Math.max(0, this.items.length - 1);
    let x = this.right - total;
    const y = this.bottom - sysH;
    this.items.forEach((it, i) => {
      const w = widths[i];
      it.rect = { x: x - sc(8, s), y: y - sc(6, s), w: w + sc(16, s), h: sysH + sc(12, s) };
      if (it.text === null) {
        it.text = new MBox({ x, y, w: w + sc(6, s), h: sysH }, this.z, {
          fontSize: sysFont,
          lineHeight: 1.3,
          color: this.colorOf(it),
          outlineWidth: Math.max(1, sc(1, s)),
          outlineColor: [0, 0, 0, 110],
          persist: true,
          visible: this.shown,
        });
        it.line = new MBox({ x, y: y + sysH + sc(1, s), w, h: lineH }, this.z, {
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
          it.line.rect.y = y + sysH + sc(1, s);
          it.line.put(x, y + sysH + sc(1, s), 255);
          it.line.setCrop(it.active || it.hover ? it.line.rect.w : 0);
        }
      }
      x += w + sysGap;
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
    it.text.setText([seg(it.label, this.colorOf(it), sc(SYS_FONT, this.s))]);
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
  private s: number;

  constructor(z: number, s: number = 1) {
    this.s = s;
    const size = sc(28, s);
    this.box = new MBox({ x: 0, y: 0, w: size, h: size }, z, {
      fontSize: sc(16, s),
      lineHeight: 1.2,
      color: MC.accent,
      align: "center",
      persist: false,
      visible: false,
    });
    this.box.setText([seg("▼", MC.accent, sc(15, s))]);
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
    const amp = sc(6, this.s);
    while (this.gen === gen) {
      const now = api.sys.now();
      const k = Math.floor((now + 2) / STEP);
      const ph = ((k * STEP) % PERIOD) / PERIOD;
      // 三角波を smoothstep でなだらかにする（tsint の Math に cos は無い）
      const tri = ph < 0.5 ? ph * 2 : (1 - ph) * 2;
      const wave = tri * tri * (3 - 2 * tri);
      if (!glyphPaused) this.box.put(x, y + Math.round(wave * amp), Math.round(255 - wave * 165));
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
  private s: number;
  private onSound: (kind: "hover" | "click") => void;

  constructor(
    screenW: number,
    screenH: number,
    items: ChoiceItem[],
    onSound: (kind: "hover" | "click") => void,
  ) {
    this.onSound = onSound;
    const s = uiScale(screenW, screenH);
    this.s = s;
    this.veil = makeVeil(screenW, screenH, 90, 84, 118);
    const chW = sc(CH_W, s);
    const chH = sc(CH_H, s);
    const chGap = sc(CH_GAP, s);
    const oy = Math.floor((screenH - sc(BASE_H, s)) / 2);
    const centerY = oy + sc(CH_CENTER_Y, s);
    const fs = sc(24, s);
    const n = items.length;
    const total = n * chH + (n - 1) * chGap;
    const x = Math.floor((screenW - chW) / 2);
    const y0 = Math.floor(centerY - total / 2);
    items.forEach((it, i) => {
      const rect = { x, y: y0 + i * (chH + chGap), w: chW, h: chH };
      const b = new MHoverBox(
        rect,
        150 + i * 3,
        {
          panel: {
            radius: sc(6, s),
            top: [28, 35, 74, 238],
            bottom: [14, 19, 46, 244],
            border: [255, 255, 255, 52],
            borderW: s,
            shadow: { blur: sc(22, s), dy: sc(8, s), alpha: 0.42 },
          },
          fontSize: fs,
          lineHeight: 1.3,
          align: "center",
          marginY: Math.floor((chH - sc(31, s)) / 2),
          outlineWidth: 0,
          visible: false,
        },
        {
          radius: sc(6, s),
          top: [238, 198, 114, 40],
          bottom: [238, 198, 114, 16],
          border: MC.accent,
          borderW: 1.5 * s,
        },
        sc(4, s),
        s,
      );
      b.setText([seg(it.label, MC.text, fs)]);
      this.buttons.push(b);
    });
  }

  /** 出現アニメを始め、最後のボタンが現れ終わるまで待つ。 */
  async show(): Promise<void> {
    this.veil.show(true);
    this.veil.go(0, 0, 255, D_OPEN, EASE_OUT);
    const dy = sc(14, this.s);
    for (let i = 0; i < this.buttons.length; i++) {
      this.buttons[i].appear(0, dy, D_OPEN + 30);
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
    const dyOther = sc(8, this.s);
    this.buttons.forEach((b, k) => {
      if (k === i) b.flash();
      else b.vanish(0, dyOther, D_NORM);
    });
    await sleep(D_NORM + 60);
    this.buttons[i].vanish(0, -sc(6, this.s), D_NORM);
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
  private s: number;

  constructor(screenW: number, screenH: number = BASE_H) {
    this.screenW = screenW;
    this.s = uiScale(screenW, screenH);
  }

  /** label が空なら消す。 */
  set(label: string): void {
    this.free();
    if (label === "") return;
    const s = this.s;
    const fs = sc(14, s);
    const w = Math.ceil(estimateWidth(label, fs) * 1.25) + sc(56, s);
    const h = sc(32, s);
    const x = this.screenW - sc(48, s) - w;
    const y = sc(26, s);
    const box = new MBox({ x, y, w, h }, 250, {
      panel: {
        radius: sc(16, s),
        top: [28, 35, 74, 226],
        bottom: [14, 19, 46, 232],
        border: withAlpha(MC.accent, 140),
        borderW: s,
        shadow: { blur: sc(12, s), dy: sc(4, s), alpha: 0.35 },
      },
      fontSize: fs,
      lineHeight: 1.3,
      marginX: sc(32, s),
      marginR: sc(12, s),
      marginY: sc(6, s),
      opacity: 0,
    });
    box.setText([seg(label, MC.accent, fs, true)]);
    const dx = sc(15, s);
    const dy = sc(13, s);
    const dotSize = sc(6, s);
    const slide = sc(8, s);
    const dot = new MBox({ x: x + dx, y: y + dy, w: dotSize, h: dotSize }, 251, { panel: { top: MC.accent, radius: sc(3, s) }, opacity: 0 });
    box.put(x, y - slide, 0);
    box.go(x, y, 255, D_NORM, EASE_OUT);
    dot.put(x + dx, y + dy - slide, 0);
    dot.go(x + dx, y + dy, 255, D_NORM, EASE_OUT);
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
  private s: number;

  constructor(screenW: number, screenH: number = BASE_H) {
    this.screenW = screenW;
    this.s = uiScale(screenW, screenH);
  }

  show(text: string, ms: number = 1400): void {
    this.hide();
    const gen = ++this.gen;
    const s = this.s;
    const fs = sc(18, s);
    const w = Math.min(this.screenW - sc(80, s), estimateWidth(text, fs) + sc(64, s));
    const h = sc(44, s);
    const x = Math.floor((this.screenW - w) / 2);
    const y = sc(28, s);
    const box = new MBox({ x, y, w, h }, 400, {
      panel: {
        radius: sc(22, s),
        top: [28, 35, 74, 236],
        bottom: [14, 19, 46, 240],
        border: withAlpha(MC.accent, 120),
        borderW: s,
        shadow: { blur: sc(16, s), dy: sc(6, s), alpha: 0.4 },
      },
      fontSize: fs,
      lineHeight: 1.3,
      align: "center",
      marginX: sc(18, s),
      marginR: sc(8, s),
      marginY: sc(10, s),
      visible: true,
      opacity: 0,
    });
    box.setText([seg(text, MC.text, fs)]);
    const dx = sc(20, s);
    const dy = sc(19, s);
    const dotSize = sc(6, s);
    const slide = sc(18, s);
    const dot = new MBox({ x: x + dx, y: y + dy, w: dotSize, h: dotSize }, 401, {
      panel: { top: MC.accent, radius: sc(3, s) },
      opacity: 0,
    });
    this.box = box;
    this.dot = dot;
    box.put(x, y - slide, 0);
    box.go(x, y, 255, D_NORM, EASE_OUT);
    dot.put(x + dx, y + dy - slide, 0);
    dot.go(x + dx, y + dy, 255, D_NORM, EASE_OUT);
    void this.expire(gen, ms);
  }

  private async expire(gen: number, ms: number): Promise<void> {
    await sleep(ms);
    if (this.gen !== gen) return;
    const out = sc(10, this.s);
    if (this.box !== null) this.box.go(this.box.rect.x, this.box.rect.y - out, 0, D_NORM, EASE_IN);
    if (this.dot !== null) this.dot.go(this.dot.rect.x, this.dot.rect.y - out, 0, D_NORM, EASE_IN);
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
  const s = uiScale(screenW, screenH);
  try {
    const w = sc(560, s);
    const h = sc(232, s);
    const x = Math.floor((screenW - w) / 2);
    const y = Math.floor((screenH - h) / 2);
    const cardFs = sc(22, s);
    const btnFs = sc(20, s);
    const btnRad = sc(6, s);
    const veil = g.add(makeVeil(screenW, screenH, 330, 150, 170));
    const card = g.add(
      new MBox({ x, y, w, h }, 331, {
        panel: {
          radius: sc(10, s),
          top: [28, 35, 76, 246],
          bottom: [14, 19, 46, 250],
          border: [255, 255, 255, 56],
          borderW: s,
          shadow: { blur: sc(30, s), dy: sc(12, s), alpha: 0.55 },
        },
        fontSize: cardFs,
        lineHeight: 1.6,
        align: "center",
        marginX: sc(32, s),
        marginY: sc(38, s),
        opacity: 0,
      }),
    );
    card.setText([seg(text || "?", MC.text, cardFs)]);
    const bw = sc(188, s);
    const bh = sc(46, s);
    const gap = sc(12, s);
    const by = y + h - bh - sc(28, s);
    const cx = x + Math.floor(w / 2);
    const yesRect = { x: cx - bw - gap, y: by, w: bw, h: bh };
    const noRect = { x: cx + gap, y: by, w: bw, h: bh };
    const yes = g.add(
      new MBox(yesRect, 332, {
        panel: { radius: btnRad, top: MC.accent, bottom: [214, 170, 88, 255] },
        fontSize: btnFs,
        align: "center",
        marginY: sc(8, s),
        opacity: 0,
      }),
    );
    yes.setText([seg(opts.yesLabel ?? "はい", [28, 22, 8, 255], btnFs, true)]);
    const no = g.add(
      new MBox(noRect, 332, {
        panel: { radius: btnRad, top: [0, 0, 0, 0], border: MC.lineStrong, borderW: s },
        fontSize: btnFs,
        align: "center",
        marginY: sc(8, s),
        opacity: 0,
      }),
    );
    no.setText([seg(opts.noLabel ?? "いいえ", MC.text, btnFs)]);
    const yesFocus = g.add(
      new MBox(yesRect, 333, {
        panel: { radius: btnRad, top: [255, 255, 255, 0], border: [255, 255, 255, 230], borderW: 2 * s },
        opacity: 0,
      }),
    );
    const noFocus = g.add(
      new MBox(noRect, 333, {
        panel: { radius: btnRad, top: [238, 198, 114, 26], border: MC.accent, borderW: 1.5 * s },
        opacity: 0,
      }),
    );
    const slideIn = sc(10, s);
    const slideOut = sc(6, s);
    veil.show(true);
    veil.go(0, 0, 255, D_OPEN, EASE_OUT);
    card.put(x, y + slideIn, 0);
    card.go(x, y, 255, D_OPEN, EASE_OUT);
    yes.put(yesRect.x, yesRect.y + slideIn, 0);
    yes.go(yesRect.x, yesRect.y, 255, D_OPEN, EASE_OUT);
    no.put(noRect.x, noRect.y + slideIn, 0);
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
    card.go(x, y + slideOut, 0, D_CLOSE, EASE_IN);
    yes.go(yesRect.x, yesRect.y + slideOut, 0, D_CLOSE, EASE_IN);
    no.go(noRect.x, noRect.y + slideOut, 0, D_CLOSE, EASE_IN);
    yesFocus.go(yesRect.x, yesRect.y, 0, D_CLOSE, EASE_IN);
    noFocus.go(noRect.x, noRect.y, 0, D_CLOSE, EASE_IN);
    veil.go(0, 0, 0, D_CLOSE, EASE_IN);
    await sleep(D_CLOSE + 10);
    return result;
  } finally {
    g.freeAll();
  }
}
