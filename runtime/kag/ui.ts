// noiria:kag/ui — システム UI（メニュー/システムボタン/トースト/確認ダイアログ）の共通部品。
// 値の import は noiria:api のみ（player.ts → menu.ts → ui.ts の一方向。循環 import は tsint で
// 実行時 TypeError になる）。tsint 互換構文のみ（Array.from なし、ループ変数をクロージャへ閉じ込めない）。

import * as api from "noiria:api";

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export function inRect(r: Rect, x: number, y: number): boolean {
  return x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
}

/** システム UI の配色（夜空に馴染む濃紺 + 金のアクセント）。 */
export const UI_COLORS = {
  overlay: [8, 10, 26, 255] as api.RGBA,
  panel: [26, 32, 62, 235] as api.RGBA,
  panelHover: [48, 60, 108, 245] as api.RGBA,
  panelEmpty: [18, 22, 44, 225] as api.RGBA,
  tab: [30, 36, 70, 235] as api.RGBA,
  tabActive: [72, 88, 156, 250] as api.RGBA,
  button: [38, 46, 88, 240] as api.RGBA,
  buttonHover: [66, 80, 140, 250] as api.RGBA,
  buttonOn: [214, 164, 52, 250] as api.RGBA,
  danger: [96, 40, 56, 245] as api.RGBA,
  track: [52, 60, 98, 255] as api.RGBA,
  fill: [214, 164, 52, 255] as api.RGBA,
  accent: [255, 216, 120, 255] as api.RGBA,
  text: [236, 240, 255, 255] as api.RGBA,
  dim: [150, 158, 192, 255] as api.RGBA,
  dark: [20, 22, 40, 255] as api.RGBA,
};

export interface BoxOpts {
  radius?: number;
  /** 背景色。省略/null なら背景なし（文字だけ）。 */
  bg?: api.RGBA | null;
  fontSize?: number;
  align?: api.TextAlign;
  marginX?: number;
  /** 右余白（折り返し幅の計算用）。省略は marginX と同じ。 */
  marginR?: number;
  marginY?: number;
  lineHeight?: number;
  color?: api.RGBA;
  outlineWidth?: number;
  /** true なら位置・表示・背景を fore/back の両ページへ書く（トランジションの swap で消えない常設部品用）。 */
  persist?: boolean;
}

/** 背景（角丸矩形）つきテキストレイヤ 1 枚。ボタン・カード・パネルの基本部品。 */
export class UiBox {
  readonly id: number;
  rect: Rect;
  private radius: number;
  private persist: boolean;

  constructor(rect: Rect, z: number, opts: BoxOpts = {}) {
    this.rect = { x: rect.x, y: rect.y, w: rect.w, h: rect.h };
    this.radius = opts.radius ?? 0;
    this.persist = opts.persist === true;
    this.id = api.layer.create("text");
    const bg = opts.bg === undefined ? null : opts.bg;
    if (bg !== null) this.paintBg(bg);
    const fontSize = opts.fontSize ?? 20;
    const lineHeight = opts.lineHeight ?? 1.3;
    const marginX = opts.marginX ?? 0;
    const marginY = opts.marginY ?? 0;
    const style: api.TextStyle = {
      fontSize,
      lineHeight,
      boxW: Math.max(1, rect.w - marginX - (opts.marginR ?? marginX)),
      boxH: Math.max(1, rect.h - marginY),
      marginX,
      marginY,
      align: opts.align ?? "left",
      color: opts.color ?? UI_COLORS.text,
      outlineWidth: opts.outlineWidth ?? 1,
    };
    api.text.style(this.id, style);
    this.eachPage((page) => {
      api.layer.set(this.id, { x: rect.x, y: rect.y, z, visible: true }, page);
    });
  }

  private eachPage(f: (page: api.Page) => void): void {
    f("fore");
    if (this.persist) f("back");
  }

  private paintBg(c: api.RGBA): void {
    this.eachPage((page) => {
      api.layer.msgbox(this.id, this.rect.w, this.rect.h, this.radius, c, page);
    });
  }

  setBg(c: api.RGBA): void {
    this.paintBg(c);
  }

  setText(segs: api.Segment[]): void {
    api.text.print(this.id, segs, false, 0);
  }

  setPos(x: number, y: number): void {
    this.rect.x = x;
    this.rect.y = y;
    this.eachPage((page) => {
      api.layer.set(this.id, { x, y }, page);
    });
  }

  show(v: boolean): void {
    this.eachPage((page) => {
      api.layer.set(this.id, { visible: v }, page);
    });
  }

  setOpacity(o: number): void {
    this.eachPage((page) => {
      api.layer.set(this.id, { opacity: o }, page);
    });
  }

  free(): void {
    api.layer.free(this.id);
  }
}

/** 画像レイヤ 1 枚（セーブのサムネイル表示用）。 */
export class UiImage {
  readonly id: number;

  constructor(x: number, y: number, z: number) {
    this.id = api.layer.create("image");
    api.layer.set(this.id, { x, y, z, visible: true }, "fore");
  }

  /** セーブ領域のサムネイルを読む。無い/壊れていれば false（表示は空になる）。 */
  loadSaveThumb(slot: string): boolean {
    try {
      api.layer.loadSaveThumb(this.id, slot, "fore");
      return true;
    } catch {
      api.layer.clearImage(this.id, "fore");
      return false;
    }
  }

  clear(): void {
    api.layer.clearImage(this.id, "fore");
  }

  free(): void {
    api.layer.free(this.id);
  }
}

/** 1 つの画面が作ったレイヤ群をまとめて解放するための入れ物。 */
export class UiGroup {
  private items: { free(): void }[] = [];

  add<T extends { free(): void }>(item: T): T {
    this.items.push(item);
    return item;
  }

  freeAll(): void {
    for (const it of this.items) it.free();
    this.items = [];
  }
}

/** 入力イベントのキュー。待ちの合間に届いたイベントを取りこぼさない（waitFor は再登録の隙間で落とす）。 */
export class UiEvents {
  private queue: api.InputEvent[] = [];
  private waiter: ((ev: api.InputEvent) => void) | null = null;
  private off: (() => void) | null;

  constructor() {
    this.off = api.input.on((ev) => {
      // キーアップ・quit は UI では使わない。
      if (ev.kind === "keyup" || ev.kind === "quit") return;
      if (this.waiter) {
        const w = this.waiter;
        this.waiter = null;
        w(ev);
      } else {
        this.queue.push(ev);
      }
    });
  }

  next(): Promise<api.InputEvent> {
    const first = this.queue.shift();
    if (first !== undefined) return Promise.resolve(first);
    return new Promise((resolve) => {
      this.waiter = resolve;
    });
  }

  /** 溜まっているイベントを捨てる（確認ダイアログを閉じた直後など）。 */
  clear(): void {
    this.queue = [];
  }

  close(): void {
    if (this.off) this.off();
    this.off = null;
  }
}

// ---------- 確認ダイアログ ----------

export interface ConfirmOpts {
  yesLabel?: string;
  noLabel?: string;
  /** true なら初期フォーカスを「はい」にする（既定は「いいえ」＝誤操作で消さない）。 */
  defaultYes?: boolean;
}

/** 画面中央のはい/いいえモーダル。左クリックはボタン上なら選択、それ以外は「いいえ」で閉じる
 * （[ask] の既定動作どおり。自動入力が行の位置を知らなくてもハングしない）。Esc は「いいえ」。
 * ←→↑↓/Tab でフォーカス移動、Enter/Space で決定。 */
export async function confirmDialog(
  screenW: number,
  screenH: number,
  text: string,
  events: UiEvents,
  opts: ConfirmOpts = {},
): Promise<boolean> {
  const group = new UiGroup();
  try {
    const w = 640;
    const h = 232;
    const left = Math.floor((screenW - w) / 2);
    const top = Math.floor((screenH - h) / 2);
    group.add(new UiBox({ x: 0, y: 0, w: screenW, h: screenH }, 330, { bg: [0, 0, 0, 150] }));
    const panel = group.add(
      new UiBox({ x: left, y: top, w, h }, 331, {
        radius: 14,
        bg: [20, 24, 50, 252],
        fontSize: 24,
        lineHeight: 1.6,
        align: "center",
        marginX: 28,
        marginY: 28,
      }),
    );
    panel.setText([{ text: text || "?", color: UI_COLORS.text }]);
    const btnW = 188;
    const btnH = 52;
    const by = top + h - btnH - 26;
    const yes = group.add(
      new UiBox({ x: left + w / 2 - btnW - 16, y: by, w: btnW, h: btnH }, 332, {
        radius: 10,
        bg: UI_COLORS.button,
        fontSize: 24,
        align: "center",
        marginY: 12,
      }),
    );
    const no = group.add(
      new UiBox({ x: left + w / 2 + 16, y: by, w: btnW, h: btnH }, 332, {
        radius: 10,
        bg: UI_COLORS.button,
        fontSize: 24,
        align: "center",
        marginY: 12,
      }),
    );
    yes.setText([{ text: opts.yesLabel ?? "はい", color: UI_COLORS.accent }]);
    no.setText([{ text: opts.noLabel ?? "いいえ", color: UI_COLORS.text }]);
    let focusYes = opts.defaultYes === true;
    const paint = (): void => {
      yes.setBg(focusYes ? UI_COLORS.tabActive : UI_COLORS.button);
      no.setBg(focusYes ? UI_COLORS.button : UI_COLORS.tabActive);
    };
    paint();
    events.clear();
    while (true) {
      const ev = await events.next();
      if (ev.kind === "move") {
        const onYes = inRect(yes.rect, ev.x, ev.y);
        const onNo = inRect(no.rect, ev.x, ev.y);
        if ((onYes || onNo) && onYes !== focusYes) {
          focusYes = onYes;
          paint();
        }
      } else if (ev.kind === "click") {
        if (ev.button === "left") return inRect(yes.rect, ev.x, ev.y);
        if (ev.button === "right") return false;
      } else if (ev.kind === "key") {
        const k = ev.key;
        if (k === "Escape") return false;
        if (k === "Enter" || k === " ") return focusYes;
        if (k === "ArrowLeft" || k === "ArrowRight" || k === "ArrowUp" || k === "ArrowDown" || k === "Tab") {
          focusYes = !focusYes;
          paint();
        }
      }
    }
  } finally {
    group.freeAll();
  }
}

// ---------- トースト ----------

/** 画面上部中央に一瞬出す通知。待ち（[wait] 等）を妨げない（自前のタイマーで消える）。 */
export class Toast {
  private box: UiBox | null = null;
  private gen = 0;
  private screenW: number;
  private fontSize = 22;

  constructor(screenW: number) {
    this.screenW = screenW;
  }

  show(text: string, ms = 1200, y = 22): void {
    this.hide();
    const gen = ++this.gen;
    const w = Math.min(this.screenW - 80, text.length * this.fontSize + 56);
    const h = 44;
    const box = new UiBox({ x: Math.floor((this.screenW - w) / 2), y, w, h }, 400, {
      radius: 10,
      bg: [18, 22, 48, 236],
      fontSize: this.fontSize,
      align: "center",
      marginY: 8,
    });
    box.setText([{ text, color: UI_COLORS.text }]);
    this.box = box;
    void this.expire(gen, ms);
  }

  private async expire(gen: number, ms: number): Promise<void> {
    await api.sys.wait(ms);
    if (this.gen === gen) this.hide();
  }

  hide(): void {
    this.gen++;
    if (this.box) {
      this.box.free();
      this.box = null;
    }
  }

  /** 表示中のテキストが入っているか（テスト/デバッグ用）。 */
  get visible(): boolean {
    return this.box !== null;
  }
}

// ---------- 常設システムボタン ----------

export type SysButtonId = "qsave" | "qload" | "save" | "load" | "auto" | "skip" | "log" | "menu";

export const SYS_BUTTON_ORDER: SysButtonId[] = ["qsave", "qload", "save", "load", "auto", "skip", "log", "menu"];

export const SYS_BUTTON_LABELS: Record<SysButtonId, string> = {
  qsave: "Q.SAVE",
  qload: "Q.LOAD",
  save: "SAVE",
  load: "LOAD",
  auto: "AUTO",
  skip: "SKIP",
  log: "LOG",
  menu: "MENU",
};

export interface SystemBarStyle {
  items: SysButtonId[];
  color: api.RGBA;
  textColor: api.RGBA;
}

const BAR_BUTTON_H = 28;
const BAR_GAP = 6;
/** メッセージ窓の上辺とバー下端の間隔（バーは窓の内側へ入らない）。 */
const BAR_BOTTOM_GAP = 4;

/** ボタン 1 個の幅（ラベル長に比例、下限あり）。純関数。 */
export function sysButtonWidth(label: string): number {
  return Math.max(48, label.length * 9 + 22);
}

/** 右揃えで並べたときの各ボタン矩形。right は最右ボタンの右端、bottom はバー下端。純関数。 */
export function layoutSystemBar(items: SysButtonId[], right: number, bottom: number): Rect[] {
  const widths = items.map((id) => sysButtonWidth(SYS_BUTTON_LABELS[id]));
  let total = 0;
  widths.forEach((w) => {
    total += w;
  });
  total += BAR_GAP * Math.max(0, items.length - 1);
  let x = right - total;
  const y = bottom - BAR_BUTTON_H;
  return widths.map((w) => {
    const r = { x, y, w, h: BAR_BUTTON_H };
    x += w + BAR_GAP;
    return r;
  });
}

/** メッセージ窓の直上に右寄せで置くテキストボタン列（Q.SAVE/Q.LOAD/SAVE/LOAD/AUTO/SKIP/LOG/MENU）。 */
export class SystemBar {
  private style: SystemBarStyle;
  private z: number;
  private items: { id: SysButtonId; box: UiBox | null; active: boolean; hover: boolean }[] = [];
  private rects: Rect[] = [];
  private shown = false;

  constructor(style: SystemBarStyle, z: number) {
    this.style = style;
    this.z = z;
    this.items = style.items.map((id) => ({ id, box: null, active: false, hover: false }));
  }

  private colorOf(it: { active: boolean; hover: boolean }): api.RGBA {
    if (it.active) return it.hover ? UI_COLORS.accent : UI_COLORS.buttonOn;
    return it.hover ? UI_COLORS.buttonHover : this.style.color;
  }

  /** 窓の上辺 top の直上・右端 right に合わせて置き直す。 */
  layout(right: number, windowTop: number): void {
    this.rects = layoutSystemBar(this.style.items, right, windowTop - BAR_BOTTOM_GAP);
    this.items.forEach((it, i) => {
      const r = this.rects[i];
      if (it.box === null) {
        const box = new UiBox(r, this.z, {
          radius: 6,
          bg: this.colorOf(it),
          fontSize: 15,
          lineHeight: 1.2,
          align: "center",
          marginY: 5,
          outlineWidth: 0,
          persist: true,
        });
        box.setText([{ text: SYS_BUTTON_LABELS[it.id], color: it.active ? UI_COLORS.dark : this.style.textColor }]);
        box.show(this.shown);
        it.box = box;
      } else {
        it.box.setPos(r.x, r.y);
      }
    });
  }

  setVisible(v: boolean): void {
    this.shown = v;
    this.items.forEach((it) => {
      if (it.box !== null) it.box.show(v);
    });
  }

  private repaint(it: { id: SysButtonId; box: UiBox | null; active: boolean; hover: boolean }): void {
    if (it.box === null) return;
    it.box.setBg(this.colorOf(it));
    it.box.setText([{ text: SYS_BUTTON_LABELS[it.id], color: it.active ? UI_COLORS.dark : this.style.textColor }]);
  }

  setActive(id: SysButtonId, on: boolean): void {
    this.items.forEach((it) => {
      if (it.id === id && it.active !== on) {
        it.active = on;
        this.repaint(it);
      }
    });
  }

  /** 表示中のバーのボタン上なら id を返す。 */
  hitButton(x: number, y: number): SysButtonId | null {
    if (!this.shown) return null;
    for (let i = 0; i < this.items.length; i++) {
      if (inRect(this.rects[i], x, y)) return this.items[i].id;
    }
    return null;
  }

  /** バー全体（ボタン間の隙間を含む帯）の上か。本文送りのクリックを奪うかの判定に使う。 */
  hitBand(x: number, y: number): boolean {
    if (!this.shown || this.rects.length === 0) return false;
    const first = this.rects[0];
    const last = this.rects[this.rects.length - 1];
    return inRect({ x: first.x, y: first.y, w: last.x + last.w - first.x, h: first.h }, x, y);
  }

  hover(x: number, y: number): void {
    const over = this.hitButton(x, y);
    this.items.forEach((it) => {
      const h = it.id === over;
      if (it.hover !== h) {
        it.hover = h;
        this.repaint(it);
      }
    });
  }
}
