// noiria:kag/mui — モダン UI（KagOptions.ui = "modern"）の部品キット。
// 値の import は noiria:api と noiria:kag/ui のみ（player.ts → m*.ts → mui.ts → ui.ts の一方向。
// 循環 import は tsint で実行時 TypeError になる）。tsint 互換構文のみ（Array.from なし、
// for...of のループ変数をクロージャへ閉じ込めない）。
//
// 部品はすべてレイヤ 1 枚 = 「パネル（グラデ・角丸・枠線・影）+ テキスト」。アニメーションは
// ネイティブの tween（`layer.move`）だけで行い、時間はエンジンの仮想クロックに従う（ヘッドレスで
// V8/tsint がバイト一致する）。

import * as api from "noiria:api";
import { BASE_W, BASE_H, uiScale, sc } from "noiria:kag/ui";
import type { Rect } from "noiria:kag/ui";

export { BASE_W, BASE_H, uiScale, sc };

// ---------- 配色・時間 ----------

/** 夜空の濃紺 + 淡い金 1 色 + 白文字。 */
export const MC = {
  /** 画面を落とす暗幕の色。 */
  veil: [6, 9, 24, 255] as api.RGBA,
  accent: [238, 198, 114, 255] as api.RGBA,
  text: [243, 245, 255, 255] as api.RGBA,
  textSub: [182, 190, 220, 255] as api.RGBA,
  textDim: [128, 137, 172, 255] as api.RGBA,
  /** 細い 1px ライン。 */
  line: [255, 255, 255, 40] as api.RGBA,
  lineStrong: [255, 255, 255, 96] as api.RGBA,
  glassTop: [24, 30, 64, 206] as api.RGBA,
  glassBottom: [12, 16, 40, 222] as api.RGBA,
  glassHoverTop: [40, 48, 92, 226] as api.RGBA,
  glassHoverBottom: [20, 26, 58, 232] as api.RGBA,
};

export const D_FAST = 120;
export const D_NORM = 180;
export const D_SLOW = 240;
/** 画面が開くときのフェード（背景のディム・見出し・中身）。押してから全部が見えるまでを 150ms 前後に収める長さ。 */
export const D_OPEN = 50;
export const D_CLOSE = 50;
/** ページ送り: 退場 → 差し替え → 入場。 */
export const D_SWAP_OUT = 30;
export const D_SWAP_IN = 45;
/** タブ切替で中身が入れ替わるときのフェード（開くときより短い）。 */
export const D_TAB = 60;
export const D_HOVER = 80;
/** `layer.move` の accel: 負 = 減速（出現向き）、正 = 加速（退場向き）。 */
export const EASE_OUT = -2;
export const EASE_IN = 2;
export const EASE_NONE = 0;

export function withAlpha(c: api.RGBA, a: number): api.RGBA {
  return [c[0], c[1], c[2], a];
}

export function sleep(ms: number): Promise<void> {
  return api.sys.wait(ms);
}

export function seg(text: string, color: api.RGBA | null, size?: number, bold?: boolean): api.Segment {
  const s: api.Segment = { text, color };
  if (size !== undefined) s.size = size;
  if (bold === true) s.bold = true;
  return s;
}

/** 文字列の描画幅の見積もり（ヒット判定・配置用）。英大文字 0.62em・英小文字 0.54em・その他の半角 0.5em・
 * 全角などは 1em。 */
export function estimateWidth(text: string, fontSize: number): number {
  let w = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c >= 0x41 && c <= 0x5a) w += fontSize * 0.62;
    else if (c >= 0x61 && c <= 0x7a) w += fontSize * 0.54;
    else if (c < 0x300) w += fontSize * 0.5;
    else w += fontSize;
  }
  return Math.ceil(w);
}

/** 選択肢の `[link]` 文字列から装飾（先頭の空白・全角空白・▶ 類）を落とした表示ラベルにする。純関数。 */
export function cleanChoiceLabel(text: string): string {
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    if (c === " " || c === "　" || c === "\t" || c === "\n" || c === "▶" || c === "▷" || c === "►" || c === ">") i++;
    else break;
  }
  return text.slice(i).replace(/\s+$/, "");
}

// ---------- パネル付きテキストレイヤ ----------

export interface PanelStyle {
  radius?: number;
  top: api.RGBA;
  bottom?: api.RGBA;
  horizontal?: boolean;
  border?: api.RGBA;
  borderW?: number;
  shadow?: { blur: number; dy: number; alpha: number };
  ramp?: number;
}

export interface MBoxOpts {
  /** 背景パネル。省略/null なら文字だけ。 */
  panel?: PanelStyle | null;
  fontSize?: number;
  lineHeight?: number;
  align?: api.TextAlign;
  marginX?: number;
  /** 右余白（折り返し幅の計算用）。省略は marginX と同じ。 */
  marginR?: number;
  marginY?: number;
  color?: api.RGBA;
  outlineWidth?: number;
  outlineColor?: api.RGBA;
  /** true なら位置・表示を fore/back の両ページへ書く（トランジションの swap で消えない常設部品用）。 */
  persist?: boolean;
  visible?: boolean;
  opacity?: number;
  /** appear() の到達先の不透明度（既定 255）。 */
  rest?: number;
  /** 最初に見せる幅（パネルを左端から何画素ぶん）。アンダーラインを左から伸ばす用。省略は全幅。 */
  crop?: number;
}

/** パネル（グラデ・角丸・枠線・影）とテキストを持つレイヤ 1 枚。`rect` はパネル矩形の「静止位置」で、
 * 影のぶん（pad）だけレイヤは外側へはみ出す。 */
export class MBox {
  readonly id: number;
  readonly rect: Rect;
  readonly pad: number;
  /** appear() が最終的に到達する不透明度（ホバー面など普段は 0 のものは 0）。 */
  rest: number;
  private persist: boolean;
  private panel: PanelStyle | null;

  constructor(rect: Rect, z: number, opts: MBoxOpts = {}) {
    this.rect = { x: rect.x, y: rect.y, w: rect.w, h: rect.h };
    this.rest = opts.rest ?? 255;
    this.persist = opts.persist === true;
    this.panel = opts.panel === undefined ? null : opts.panel;
    const sh = this.panel !== null && this.panel.shadow ? this.panel.shadow : null;
    this.pad = sh !== null ? Math.ceil(sh.blur + Math.abs(sh.dy)) + 2 : 0;
    this.id = api.layer.create("text");
    if (this.panel !== null) this.paint();
    const mx = opts.marginX ?? 0;
    const style: api.TextStyle = {
      fontSize: opts.fontSize ?? 20,
      lineHeight: opts.lineHeight ?? 1.3,
      boxW: Math.max(1, rect.w - mx - (opts.marginR ?? mx)),
      boxH: Math.max(1, rect.h - (opts.marginY ?? 0)),
      marginX: this.pad + mx,
      marginY: this.pad + (opts.marginY ?? 0),
      align: opts.align ?? "left",
      color: opts.color ?? MC.text,
      outlineWidth: opts.outlineWidth ?? 0,
    };
    if (opts.outlineColor !== undefined) style.outlineColor = opts.outlineColor;
    api.text.style(this.id, style);
    this.eachPage((page) => {
      api.layer.set(
        this.id,
        {
          x: rect.x - this.pad,
          y: rect.y - this.pad,
          z,
          visible: opts.visible !== false,
          opacity: opts.opacity ?? 255,
        },
        page,
      );
      if (opts.crop !== undefined) api.layer.setCrop(this.id, opts.crop, page);
    });
  }

  private eachPage(f: (page: api.Page) => void): void {
    f("fore");
    if (this.persist) f("back");
  }

  private paint(): void {
    const p = this.panel;
    if (p === null) return;
    this.eachPage((page) => {
      const spec: api.PanelSpec = {
        w: this.rect.w,
        h: this.rect.h,
        radius: p.radius ?? 0,
        top: p.top,
        pad: this.pad,
      };
      if (p.bottom !== undefined) spec.bottom = p.bottom;
      if (p.horizontal === true) spec.horizontal = true;
      if (p.border !== undefined) spec.border = p.border;
      if (p.borderW !== undefined) spec.borderW = p.borderW;
      if (p.shadow !== undefined) spec.shadow = p.shadow;
      if (p.ramp !== undefined) spec.ramp = p.ramp;
      api.layer.panel(this.id, spec, page);
    });
  }

  setPanel(p: PanelStyle): void {
    this.panel = p;
    this.paint();
  }

  /** パネルの幅/高さだけ変えて描き直す（スライダーの塗りなど）。静止位置の x/y は変わらない。 */
  resizePanel(w: number, h: number): void {
    this.rect.w = w;
    this.rect.h = h;
    this.paint();
  }

  setText(segs: api.Segment[]): void {
    api.text.print(this.id, segs, false, 0);
  }

  clearText(): void {
    api.text.clear(this.id);
  }

  /** 進行中のアニメを止めて、絶対位置・不透明度へ即座に置く。 */
  put(x: number, y: number, opacity: number): void {
    this.eachPage((page) => {
      api.layer.cancelMove(this.id, page);
      api.layer.set(this.id, { x: x - this.pad, y: y - this.pad, opacity }, page);
    });
  }

  /** 現在値から (x, y, opacity) へ ms かけて動かす（tween。位置はパネル矩形の座標）。cropW を渡すと
   * 見える幅（パネルの左端から何画素ぶん）も同時に動かす。 */
  go(x: number, y: number, opacity: number, ms: number, accel: number = EASE_OUT, cropW?: number): void {
    if (ms <= 0) {
      this.put(x, y, opacity);
      if (cropW !== undefined) this.setCrop(cropW);
      return;
    }
    this.eachPage((page) => {
      api.layer.move(this.id, { toX: x - this.pad, toY: y - this.pad, toOpacity: opacity, timeMs: ms, accel }, page);
      if (cropW !== undefined) api.layer.cropTo(this.id, cropW + this.pad, ms, accel, page);
    });
  }

  /** 見える幅を即座に決める（影の pad ぶんは含めない = パネル本体の幅）。 */
  setCrop(w: number): void {
    this.eachPage((page) => {
      api.layer.setCrop(this.id, w + this.pad, page);
    });
  }

  show(v: boolean): void {
    this.eachPage((page) => {
      api.layer.set(this.id, { visible: v }, page);
    });
  }

  setZ(z: number): void {
    this.eachPage((page) => {
      api.layer.set(this.id, { z }, page);
    });
  }

  /** 静止位置へフェード + スライドで現れる。dx/dy は出現元のずれ。 */
  appear(dx: number, dy: number, ms: number): void {
    if (ms <= 0) {
      this.put(this.rect.x, this.rect.y, this.rest);
      this.show(true);
      return;
    }
    this.put(this.rect.x + dx, this.rect.y + dy, 0);
    this.show(true);
    this.go(this.rect.x, this.rect.y, this.rest, ms, EASE_OUT);
  }

  /** フェード + スライドで消える（消えたあとの visible は呼び出し側）。 */
  vanish(dx: number, dy: number, ms: number): void {
    this.go(this.rect.x + dx, this.rect.y + dy, 0, ms, EASE_IN);
  }

  free(): void {
    api.layer.free(this.id);
  }
}

/** 画像レイヤ 1 枚（セーブのサムネイル・ギャラリーの CG）。MBox と同じ put/go/show で動かせる。 */
export class MImage {
  readonly id: number;
  readonly rect: Rect;

  constructor(rect: Rect, z: number) {
    this.rect = { x: rect.x, y: rect.y, w: rect.w, h: rect.h };
    this.id = api.layer.create("image");
    api.layer.set(this.id, { x: rect.x, y: rect.y, z, visible: false, opacity: 0 }, "fore");
  }

  /** セーブ領域のサムネイルを rect の寸法へ拡縮して読む。無い/壊れていれば false（画像は空になる）。 */
  loadSaveThumb(slot: string): boolean {
    try {
      api.layer.loadSaveThumb(this.id, slot, "fore", this.rect.w, this.rect.h);
      return true;
    } catch {
      api.layer.clearImage(this.id, "fore");
      return false;
    }
  }

  loadScaled(path: string): boolean {
    try {
      api.layer.loadImageScaled(this.id, path, this.rect.w, this.rect.h, "fore");
      return true;
    } catch (e) {
      api.sys.log(`画像を読めません: ${path}: ${e}`, "warn");
      api.layer.clearImage(this.id, "fore");
      return false;
    }
  }

  clear(): void {
    api.layer.clearImage(this.id, "fore");
  }

  put(x: number, y: number, opacity: number): void {
    api.layer.cancelMove(this.id, "fore");
    api.layer.set(this.id, { x, y, opacity }, "fore");
  }

  go(x: number, y: number, opacity: number, ms: number, accel: number = EASE_OUT): void {
    if (ms <= 0) {
      this.put(x, y, opacity);
      return;
    }
    api.layer.move(this.id, { toX: x, toY: y, toOpacity: opacity, timeMs: ms, accel }, "fore");
  }

  show(v: boolean): void {
    api.layer.set(this.id, { visible: v }, "fore");
  }

  appear(dx: number, dy: number, ms: number): void {
    if (ms <= 0) {
      this.put(this.rect.x, this.rect.y, 255);
      this.show(true);
      return;
    }
    this.put(this.rect.x + dx, this.rect.y + dy, 0);
    this.show(true);
    this.go(this.rect.x, this.rect.y, 255, ms, EASE_OUT);
  }

  vanish(dx: number, dy: number, ms: number): void {
    this.go(this.rect.x + dx, this.rect.y + dy, 0, ms, EASE_IN);
  }

  free(): void {
    api.layer.free(this.id);
  }
}

/** ホバー/フォーカスで枠がアクセント色になるボタン。基本面（文字入り）の上にホバー面（枠 + わずかな
 * 明るさ）を重ね、ホバー面の不透明度を tween するだけで 120ms のイージング付き切替になる。
 * 左端のアクセントバー（任意）はホバーで数 px スライドして現れる。 */
export class MHoverBox {
  readonly base: MBox;
  private over: MBox;
  private bar: MBox | null;
  private barSlide: number;
  private on = false;

  constructor(
    rect: Rect,
    z: number,
    baseOpts: MBoxOpts,
    overPanel: PanelStyle,
    barW: number = 0,
    s: number = 1,
  ) {
    this.base = new MBox(rect, z, baseOpts);
    this.over = new MBox(rect, z + 1, { panel: overPanel, visible: true, opacity: 0, persist: baseOpts.persist });
    this.barSlide = sc(6, s);
    this.bar =
      barW > 0
        ? new MBox({ x: rect.x, y: rect.y, w: barW, h: rect.h }, z + 2, {
            panel: { top: MC.accent, radius: Math.min(2 * s, barW / 2) },
            opacity: 0,
            persist: baseOpts.persist,
          })
        : null;
  }

  get rect(): Rect {
    return this.base.rect;
  }

  get hovered(): boolean {
    return this.on;
  }

  setHover(on: boolean, ms: number = D_FAST): void {
    if (on === this.on) return;
    this.on = on;
    const r = this.base.rect;
    this.over.go(r.x, r.y, on ? 255 : 0, ms, on ? EASE_OUT : EASE_IN);
    if (this.bar !== null) {
      this.bar.go(on ? r.x : r.x - this.barSlide, r.y, on ? 255 : 0, ms, on ? EASE_OUT : EASE_IN);
    }
  }

  /** ホバー状態を tween なしで強制する（選択直後の発光など）。 */
  flash(): void {
    const r = this.base.rect;
    this.on = true;
    this.over.put(r.x, r.y, 255);
    if (this.bar !== null) this.bar.put(r.x, r.y, 255);
  }

  setText(segs: api.Segment[]): void {
    this.base.setText(segs);
  }

  show(v: boolean): void {
    this.base.show(v);
    this.over.show(v);
    if (this.bar !== null) this.bar.show(v);
  }

  /** ベース面だけ動かす/フェードさせる（ホバー面とバーは静止位置の不透明度 0 のまま）。 */
  appear(dx: number, dy: number, ms: number): void {
    const r = this.base.rect;
    this.base.appear(dx, dy, ms);
    this.over.put(r.x, r.y, 0);
    this.over.show(true);
    if (this.bar !== null) {
      this.bar.put(r.x - this.barSlide, r.y, 0);
      this.bar.show(true);
    }
    this.on = false;
  }

  vanish(dx: number, dy: number, ms: number): void {
    this.base.vanish(dx, dy, ms);
    const r = this.base.rect;
    this.over.go(r.x + dx, r.y + dy, 0, ms, EASE_IN);
    if (this.bar !== null) this.bar.go(r.x + dx, r.y + dy, 0, ms, EASE_IN);
  }

  free(): void {
    this.base.free();
    this.over.free();
    if (this.bar !== null) this.bar.free();
  }
}

/** 1 つの画面が作ったレイヤ群をまとめて解放する入れ物（free() を持つものなら何でも）。 */
export class MGroup {
  private items: { free(): void }[] = [];

  add<T extends { free(): void }>(item: T): T {
    this.items.push(item);
    return item;
  }

  freeAll(): void {
    this.items.forEach((it) => it.free());
    this.items = [];
  }

  /** vanish() を持つ部品をまとめてフェードアウトさせる（画面を閉じるとき。解放は呼び出し側）。 */
  vanishAll(dx: number, dy: number, ms: number): void {
    this.items.forEach((it) => {
      const x = it as any;
      if (typeof x.vanish === "function") x.vanish(dx, dy, ms);
    });
  }

  /** appear() を持つ部品（MBox/MImage/MHoverBox）をまとめて、dy だけ下からのフェード + スライドで出す。 */
  appearAll(dx: number, dy: number, ms: number): void {
    this.items.forEach((it) => {
      const x = it as any;
      if (typeof x.appear === "function") x.appear(dx, dy, ms);
    });
  }
}

/** 画面全体を暗く落とす暗幕（縦に少しだけ濃さが変わる）。 */
export function makeVeil(screenW: number, screenH: number, z: number, alphaTop: number, alphaBottom: number): MBox {
  return new MBox({ x: 0, y: 0, w: screenW, h: screenH }, z, {
    panel: { top: withAlpha(MC.veil, alphaTop), bottom: withAlpha(MC.veil, alphaBottom) },
    opacity: 0,
  });
}

/** 画面の見出し: 大きな英字 + 小さな和文ラベル（例: SAVE / セーブ）と、その下の短いアクセントバー。 */
export function addHeading(g: MGroup, z: number, x: number, y: number, en: string, jp: string, ms: number = D_OPEN, s: number = 1): void {
  const bigFs = sc(52, s);
  const smallFs = sc(20, s);
  const slide = sc(14, s);
  const big = new MBox({ x, y, w: sc(560, s), h: sc(70, s) }, z, { fontSize: bigFs, lineHeight: 1.2, color: MC.text, opacity: 0 });
  big.setText([seg(en, MC.text, bigFs, true)]);
  g.add(big);
  const jx = x + Math.ceil(estimateWidth(en, bigFs) * 1.24) + sc(22, s);
  const small = new MBox({ x: jx, y: y + sc(26, s), w: sc(300, s), h: sc(30, s) }, z, { fontSize: smallFs, lineHeight: 1.3, color: MC.textSub, opacity: 0 });
  small.setText([seg(jp, MC.textSub, smallFs)]);
  g.add(small);
  const bar = new MBox({ x: x + sc(3, s), y: y + sc(72, s), w: sc(44, s), h: sc(3, s) }, z, { panel: { top: MC.accent, radius: 1.5 * s }, opacity: 0 });
  g.add(bar);
  [big, small, bar].forEach((b) => {
    if (ms <= 0) {
      b.put(b.rect.x, b.rect.y, 255);
      b.show(true);
      return;
    }
    b.put(b.rect.x - slide, b.rect.y, 0);
    b.show(true);
    b.go(b.rect.x, b.rect.y, 255, ms, EASE_OUT);
  });
}
