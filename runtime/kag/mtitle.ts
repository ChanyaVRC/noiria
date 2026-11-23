// noiria:kag/mtitle — モダン UI の専用タイトル画面（[titlescreen]）とギャラリー（おまけ）。
// 値の import は mui.ts / ui.ts / api のみ（player.ts → mtitle.ts → mui.ts の一方向）。tsint 互換構文のみ。

import * as api from "noiria:api";
import { UiEvents, inRect } from "noiria:kag/ui";
import type { Rect } from "noiria:kag/ui";
import {
  MBox,
  MGroup,
  MC,
  BASE_W,
  BASE_H,
  uiScale,
  sc,
  D_FAST,
  D_NORM,
  D_OPEN,
  D_SLOW,
  EASE_OUT,
  EASE_IN,
  withAlpha,
  sleep,
  seg,
  estimateWidth,
  addHeading,
  makeVeil,
} from "noiria:kag/mui";

// ---------- 宣言 ----------

/** タイトルのメニュー項目。`action` を持つものは組み込みの動作、持たないものは `target`/`storage`（ジャンプ）か
 * `exp`（式の実行）。`label` は大きな英字、`sub` は小さな和文ラベル。 */
export interface TitleMenuItem {
  label: string;
  sub?: string;
  action?: "continue" | "gallery" | "config" | "exit";
  target?: string;
  storage?: string;
  exp?: string;
}

export interface TitleScreenOptions {
  /** 大きなロゴ文字。 */
  name: string;
  subtitle?: string;
  /** ロゴの上の小さな英字の一行（例: "A SCHOOL ROMANCE"）。 */
  tagline?: string;
  /** 右下の小さな行（バージョン/コピーライト）。 */
  version?: string;
  menu: TitleMenuItem[];
}

/** ギャラリーの 1 枠。sf の `flag` が真なら解放済み（CG サムネイル + 名前）、偽ならロック表示。 */
export interface GalleryEntry {
  flag: string;
  image: string;
  title: string;
}

export interface TitleHost {
  screenW: number;
  screenH: number;
  /** セーブが 1 つでもあるか（CONTINUE の有効/無効）。 */
  hasSave(): boolean;
  openLoad(): Promise<void>;
  openConfig(): Promise<void>;
  openGallery(): Promise<void>;
  confirmExit(): Promise<boolean>;
  quit(): void;
  runExp(exp: string): void;
  /** メニューなどで別のジャンプが要求されたか（ロード成功など）。 */
  jumpPending(): boolean;
  uiSound(kind: "hover" | "click"): void;
}

export interface TitleResult {
  storage: string | null;
  target: string | null;
}

// ---------- タイトル画面 ----------

const ITEM_X = 104;
const ITEM_Y0 = 346;
const ITEM_PITCH = 62;
const Z_TITLE = 30;

interface TItem {
  def: TitleMenuItem;
  text: MBox;
  bar: MBox;
  line: MBox;
  hit: Rect;
  enabled: boolean;
}

export class TitleScreen {
  private host: TitleHost;
  private opts: TitleScreenOptions;
  private g = new MGroup();
  private items: TItem[] = [];
  private focus = -1;
  private s = 1;
  /** 退場でフェードさせるロゴ類（項目以外）。 */
  private fadeAll: MBox[] = [];

  constructor(host: TitleHost, opts: TitleScreenOptions) {
    this.host = host;
    this.opts = opts;
  }

  private itemSegs(it: TItem, hot: boolean): api.Segment[] {
    const main = it.enabled ? (hot ? MC.accent : MC.text) : withAlpha(MC.textDim, 150);
    const sub = it.enabled ? (hot ? withAlpha(MC.accent, 210) : MC.textSub) : withAlpha(MC.textDim, 130);
    const subText = it.def.action === "continue" && !it.enabled ? "NO DATA" : it.def.sub ?? "";
    const segs: api.Segment[] = [seg(it.def.label, main, sc(31, this.s), true)];
    if (subText !== "") segs.push(seg("    " + subText, sub, sc(15, this.s)));
    return segs;
  }

  /** タイトルを出し、項目が選ばれるまで待つ。ジャンプ先を返す（ロードなど別経路のジャンプなら null）。 */
  async run(): Promise<TitleResult | null> {
    const o = this.opts;
    const sw = this.host.screenW;
    const sh = this.host.screenH;
    const s = uiScale(sw, sh);
    this.s = s;
    const ox = Math.floor((sw - sc(BASE_W, s)) / 2);
    const oy = Math.floor((sh - sc(BASE_H, s)) / 2);
    const r = (x: number, y: number, w: number, h: number): Rect => ({
      x: ox + sc(x, s),
      y: oy + sc(y, s),
      w: sc(w, s),
      h: sc(h, s),
    });
    const events = new UiEvents();
    try {
      // 左側の暗い面（文字の可読性）と下端の暗幕
      const footH = sc(140, s);
      const shade = this.g.add(
        new MBox({ x: 0, y: 0, w: ox + sc(860, s), h: sh }, Z_TITLE, {
          panel: { top: [4, 7, 22, 220], bottom: [4, 7, 22, 0], horizontal: true },
          opacity: 0,
        }),
      );
      const foot = this.g.add(
        new MBox({ x: 0, y: sh - footH, w: sw, h: footH }, Z_TITLE, {
          panel: { top: [4, 7, 22, 0], bottom: [4, 7, 22, 170] },
          opacity: 0,
        }),
      );
      this.fadeAll.push(shade);
      this.fadeAll.push(foot);
      shade.show(true);
      foot.show(true);
      shade.go(0, 0, 255, 320, EASE_OUT);
      foot.go(0, sh - footH, 255, 320, EASE_OUT);

      // ロゴ: 小さな英字の一行 → 淡いグロー → 大きなロゴ → 細いサブタイトル
      const logo: MBox[] = [];
      const addText = (rect: Rect, segs: api.Segment[], fontSize: number): MBox => {
        const b = this.g.add(
          new MBox(rect, Z_TITLE + 2, { fontSize, lineHeight: 1.25, opacity: 0, outlineWidth: 0 }),
        );
        b.setText(segs);
        logo.push(b);
        return b;
      };
      const LX = 96;
      const tagFs = sc(15, s);
      if (o.tagline) addText(r(LX + 4, 92, 700, 26), [seg(o.tagline, withAlpha(MC.accent, 230), tagFs)], tagFs);
      const logoFs = sc(80, s);
      const nameW = estimateWidth(o.name, logoFs) + sc(40, s);
      const logoRect = { x: ox + sc(LX, s), y: oy + sc(122, s), w: nameW + sc(40, s), h: sc(110, s) };
      const glow = (width: number, alpha: number): void => {
        const b = this.g.add(
          new MBox(logoRect, Z_TITLE + 1, {
            fontSize: logoFs,
            lineHeight: 1.25,
            opacity: 0,
            outlineWidth: sc(width, s),
          }),
        );
        const sg = seg(o.name, [255, 214, 140, 0], logoFs);
        sg.edgeColor = [255, 206, 120, alpha];
        b.setText([sg]);
        logo.push(b);
      };
      glow(14, 9);
      glow(9, 14);
      glow(5, 22);
      glow(2, 34);
      addText(logoRect, [seg(o.name, [255, 247, 226, 255], logoFs, true)], logoFs);
      const subFs = sc(21, s);
      if (o.subtitle) addText(r(LX + 6, 236, 700, 34), [seg(o.subtitle, MC.textSub, subFs)], subFs);
      const rule = this.g.add(
        new MBox(r(LX + 6, 286, 64, 2), Z_TITLE + 2, {
          panel: { top: MC.accent, radius: 1 * s },
          opacity: 0,
        }),
      );
      logo.push(rule);

      // メニュー項目（左に細い縦線を通し、ホバー中の項目だけアクセント色のバーが重なる）
      const spine = this.g.add(
        new MBox(
          { x: ox + sc(ITEM_X - 21, s), y: oy + sc(ITEM_Y0 + 4, s), w: Math.max(1, sc(1, s)), h: sc((o.menu.length - 1) * ITEM_PITCH + 46, s) },
          Z_TITLE + 2,
          {
            panel: { top: [255, 255, 255, 0], bottom: [255, 255, 255, 0], ramp: 0 },
            opacity: 0,
          },
        ),
      );
      spine.setPanel({ top: [255, 255, 255, 70], bottom: [255, 255, 255, 14] });
      logo.push(spine);
      const hasSave = this.host.hasSave();
      const itemFs = sc(31, s);
      o.menu.forEach((def, i) => {
        const enabled = !(def.action === "continue" && !hasSave);
        const rect = r(ITEM_X, ITEM_Y0 + i * ITEM_PITCH, 460, 54);
        const text = this.g.add(
          new MBox(rect, Z_TITLE + 3, { fontSize: itemFs, lineHeight: 1.3, opacity: 0, outlineWidth: Math.max(1, sc(1, s)), outlineColor: [0, 0, 0, 110] }),
        );
        const bar = this.g.add(
          new MBox({ x: ox + sc(ITEM_X - 22, s), y: rect.y + sc(10, s), w: sc(3, s), h: sc(32, s) }, Z_TITLE + 3, {
            panel: { top: MC.accent, radius: 1.5 * s },
            opacity: 0,
          }),
        );
        const line = this.g.add(
          new MBox({ x: rect.x, y: rect.y + sc(46, s), w: Math.ceil(estimateWidth(def.label, itemFs) * 1.24) + sc(14, s), h: sc(2, s) }, Z_TITLE + 3, {
            panel: { top: MC.accent, radius: 1 * s },
            crop: 0,
            visible: false,
          }),
        );
        const it: TItem = {
          def,
          text,
          bar,
          line,
          hit: { x: ox + sc(ITEM_X - 30, s), y: rect.y - sc(4, s), w: sc(520, s), h: sc(62, s) },
          enabled,
        };
        text.setText(this.itemSegs(it, false));
        this.items.push(it);
      });
      if (o.version) {
        const verFs = sc(14, s);
        const v = this.g.add(
          new MBox({ x: sw - ox - sc(640, s), y: sh - oy - sc(40, s), w: sc(592, s), h: sc(24, s) }, Z_TITLE + 2, {
            fontSize: verFs,
            lineHeight: 1.3,
            align: "right",
            opacity: 0,
          }),
        );
        v.setText([seg(o.version, withAlpha(MC.textSub, 190), verFs)]);
        logo.push(v);
      }

      logo.forEach((b) => this.fadeAll.push(b));
      // 登場: ロゴ → メニューの順にフェードイン
      await sleep(180);
      const logoSlide = sc(16, s);
      for (let i = 0; i < logo.length; i++) {
        logo[i].show(true);
        logo[i].put(logo[i].rect.x - logoSlide, logo[i].rect.y, 0);
        logo[i].go(logo[i].rect.x, logo[i].rect.y, 255, D_SLOW + 80, EASE_OUT);
        if (i === 2 || i === 4) await sleep(70);
      }
      await sleep(D_NORM);
      const itemSlide = sc(22, s);
      for (let i = 0; i < this.items.length; i++) {
        const it = this.items[i];
        it.text.show(true);
        it.line.show(true);
        it.text.put(it.text.rect.x - itemSlide, it.text.rect.y, 0);
        it.text.go(it.text.rect.x, it.text.rect.y, 255, D_SLOW, EASE_OUT);
        await sleep(80);
      }

      let result: TitleResult | null = null;
      let done = false;
      while (!done) {
        const ev = await events.next();
        let pick = -1;
        if (ev.kind === "move") {
          const i = this.hitAt(ev.x, ev.y);
          if (i !== this.focus && (i >= 0 || ev.x >= 0)) this.setFocus(i);
        } else if (ev.kind === "click" && ev.button === "left") {
          pick = this.hitAt(ev.x, ev.y);
        } else if (ev.kind === "key") {
          const n = this.items.length;
          if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
            const dir = ev.key === "ArrowDown" ? 1 : -1;
            let next = this.focus;
            for (let k = 0; k < n; k++) {
              next = next < 0 ? (dir > 0 ? 0 : n - 1) : (next + dir + n) % n;
              if (this.items[next].enabled) break;
            }
            this.setFocus(next);
          } else if ((ev.key === "Enter" || ev.key === " ") && this.focus >= 0) {
            pick = this.focus;
          } else if (ev.key === "Escape") {
            await this.activate({ label: "", action: "config" });
            events.clear();
            if (this.host.jumpPending()) done = true;
          }
        }
        if (pick >= 0 && this.items[pick].enabled) {
          this.host.uiSound("click");
          const def = this.items[pick].def;
          if (def.action === undefined && def.exp === undefined) {
            result = { storage: def.storage ?? null, target: def.target ?? null };
            done = true;
          } else {
            await this.activate(def);
            events.clear();
            if (this.host.jumpPending()) done = true;
          }
        }
      }
      // 退場
      this.fadeOut();
      await sleep(D_NORM + 40);
      return result;
    } finally {
      events.close();
      this.g.freeAll();
    }
  }

  /** 退場: 項目・ロゴ・暗い面をまとめてフェードアウトする。 */
  private fadeOut(): void {
    const shift = sc(14, this.s);
    this.items.forEach((it, i) => {
      it.text.go(it.text.rect.x + (i === this.focus ? shift : 0), it.text.rect.y, 0, D_NORM, EASE_IN);
      it.bar.go(it.bar.rect.x, it.bar.rect.y, 0, D_NORM, EASE_IN);
      it.line.go(it.line.rect.x, it.line.rect.y, 0, D_NORM, EASE_IN);
    });
    this.fadeAll.forEach((b) => b.go(b.rect.x, b.rect.y, 0, D_NORM, EASE_IN));
  }

  private async activate(def: TitleMenuItem): Promise<void> {
    if (def.action === "continue" || def.action === "gallery" || def.action === "config") {
      // 全画面メニューの下でタイトルの文字が透けないよう、開いている間は引っ込める
      this.cover(true);
      try {
        if (def.action === "continue") await this.host.openLoad();
        else if (def.action === "gallery") await this.host.openGallery();
        else await this.host.openConfig();
      } finally {
        this.cover(false);
      }
    } else if (def.action === "exit") {
      if (await this.host.confirmExit()) this.host.quit();
    } else if (def.exp !== undefined) this.host.runExp(def.exp);
  }

  /** メニューを開く間、タイトルの項目・ロゴ・暗い面をフェードアウト/復帰させる。 */
  private cover(on: boolean): void {
    this.setFocus(-1);
    const op = on ? 0 : 255;
    this.items.forEach((it) => {
      it.text.go(it.text.rect.x, it.text.rect.y, op, D_FAST, on ? EASE_IN : EASE_OUT);
      it.bar.go(it.bar.rect.x, it.bar.rect.y, 0, D_FAST, EASE_IN);
      it.line.go(it.line.rect.x, it.line.rect.y, 255, D_FAST, EASE_IN, 0);
    });
    this.fadeAll.forEach((b) => b.go(b.rect.x, b.rect.y, op, D_FAST, on ? EASE_IN : EASE_OUT));
  }

  private hitAt(x: number, y: number): number {
    for (let i = 0; i < this.items.length; i++) {
      if (inRect(this.items[i].hit, x, y)) return i;
    }
    return -1;
  }

  private setFocus(i: number): void {
    if (i >= 0 && !this.items[i].enabled) i = -1;
    if (i === this.focus) return;
    const prev = this.focus;
    this.focus = i;
    if (prev >= 0) this.paintItem(prev, false);
    if (i >= 0) {
      this.paintItem(i, true);
      this.host.uiSound("hover");
    }
  }

  private paintItem(i: number, hot: boolean): void {
    const it = this.items[i];
    const shift = sc(14, this.s);
    it.text.setText(this.itemSegs(it, hot));
    it.text.go(it.text.rect.x + (hot ? shift : 0), it.text.rect.y, 255, D_FAST + 20, hot ? EASE_OUT : EASE_IN);
    it.bar.go(it.bar.rect.x, it.bar.rect.y, hot ? 255 : 0, D_FAST, hot ? EASE_OUT : EASE_IN);
    // アクセントの線が左から伸びる（外れると縮む）。文字と一緒に右へ寄る
    it.line.go(it.line.rect.x + (hot ? shift : 0), it.line.rect.y, 255, D_NORM, hot ? EASE_OUT : EASE_IN, hot ? it.line.rect.w : 0);
  }
}

// ---------- ギャラリー ----------

export interface GalleryHost {
  screenW: number;
  screenH: number;
  unlocked(flag: string): boolean;
  uiSound(kind: "hover" | "click"): void;
}

const G_CARD_W = 360;
const G_CARD_H = 250;
const G_THUMB_W = 336;
const G_THUMB_H = 189;
const G_GAP = 28;
const G_X0 = 72;
const G_Y0 = 140;
const G_PER_PAGE = 6;

/** 解放済みの CG のサムネイルを別スレッドで先に作っておく（ギャラリーを開くたびに待たない）。 */
export function prefetchGalleryThumbs(images: string[], screenW: number = BASE_W, screenH: number = BASE_H): void {
  const s = uiScale(screenW, screenH);
  if (images.length > 0) api.sys.prefetchImagesScaled(images, sc(G_THUMB_W, s), sc(G_THUMB_H, s));
}

/** 「おまけ」: サムネイルのギャラリー画面。解放済みは CG のサムネイル + 名前、未解放は鍵の枠。クリックで全画面表示、
 * もう一度クリックで戻る。Esc/右クリックで閉じる。 */
export class GalleryScreen {
  private host: GalleryHost;
  private entries: GalleryEntry[];
  private g = new MGroup();
  private cards: { rect: Rect; over: MBox; entry: GalleryEntry; open: boolean }[] = [];
  private focus = -1;
  private page = 0;
  private s = 1;
  private ox = 0;
  private oy = 0;

  constructor(host: GalleryHost, entries: GalleryEntry[]) {
    this.host = host;
    this.entries = entries;
  }

  async run(): Promise<void> {
    const sw = this.host.screenW;
    const sh = this.host.screenH;
    const s = uiScale(sw, sh);
    this.s = s;
    this.ox = Math.floor((sw - sc(BASE_W, s)) / 2);
    this.oy = Math.floor((sh - sc(BASE_H, s)) / 2);
    const events = new UiEvents();
    const veil = this.g.add(makeVeil(sw, sh, 310, 250, 253));
    veil.show(true);
    veil.go(0, 0, 255, D_OPEN, EASE_OUT);
    addHeading(this.g, 311, this.ox + sc(72, s), this.oy + sc(40, s), "GALLERY", "おまけ", D_OPEN, s);
    const unlockedCount = this.entries.filter((e) => this.host.unlocked(e.flag)).length;
    const count = this.g.add(
      new MBox({ x: sw - this.ox - sc(72, s) - sc(360, s), y: this.oy + sc(64, s), w: sc(360, s), h: sc(30, s) }, 311, {
        fontSize: sc(17, s),
        lineHeight: 1.3,
        align: "right",
        opacity: 0,
      }),
    );
    count.setText([seg(`${unlockedCount} / ${this.entries.length}`, MC.accent, sc(20, s), true), seg("   解放", MC.textSub, sc(14, s))]);
    count.show(true);
    count.go(count.rect.x, count.rect.y, 255, D_OPEN, EASE_OUT);
    const hintFs = sc(14, s);
    const hint = this.g.add(
      new MBox({ x: this.ox + sc(72, s), y: sh - this.oy - sc(40, s), w: sw - this.ox * 2 - sc(144, s), h: sc(26, s) }, 311, {
        fontSize: hintFs,
        lineHeight: 1.3,
        align: "right",
        opacity: 0,
      }),
    );
    hint.setText([seg("クリックで拡大　←→ 選択　Enter 決定　Esc / 右クリックで戻る", withAlpha(MC.textSub, 190), hintFs)]);
    hint.show(true);
    hint.go(hint.rect.x, hint.rect.y, 255, D_OPEN, EASE_OUT);
    this.buildPage();
    try {
      while (true) {
        const ev = await events.next();
        if (ev.kind === "move") {
          const i = this.cardAt(ev.x, ev.y);
          if (i >= 0) this.setFocus(i);
        } else if (ev.kind === "click") {
          if (ev.button === "right") break;
          if (ev.button === "left") {
            const i = this.cardAt(ev.x, ev.y);
            if (i >= 0) {
              this.setFocus(i);
              if (this.cards[i].open) {
                this.host.uiSound("click");
                await this.view(events, this.cards[i].entry);
                events.clear();
              }
            }
          }
        } else if (ev.kind === "key") {
          const k = ev.key;
          if (k === "Escape") break;
          if (k === "ArrowRight") this.setFocus(Math.min(this.cards.length - 1, this.focus + 1));
          else if (k === "ArrowLeft") this.setFocus(Math.max(0, this.focus - 1));
          else if (k === "ArrowDown") this.setFocus(Math.min(this.cards.length - 1, this.focus + 3));
          else if (k === "ArrowUp") this.setFocus(Math.max(0, this.focus - 3));
          else if ((k === "Enter" || k === " ") && this.focus >= 0 && this.cards[this.focus].open) {
            this.host.uiSound("click");
            await this.view(events, this.cards[this.focus].entry);
            events.clear();
          }
        }
      }
      this.g.freeAll();
    } finally {
      events.close();
      this.g.freeAll();
    }
  }

  private cardAt(x: number, y: number): number {
    for (let i = 0; i < this.cards.length; i++) {
      if (inRect(this.cards[i].rect, x, y)) return i;
    }
    return -1;
  }

  private setFocus(i: number): void {
    if (i === this.focus || i < 0 || i >= this.cards.length) return;
    const prev = this.focus >= 0 ? this.cards[this.focus] : null;
    if (prev) prev.over.go(prev.over.rect.x, prev.over.rect.y, 0, D_FAST, EASE_IN);
    this.focus = i;
    const c = this.cards[i];
    c.over.go(c.over.rect.x, c.over.rect.y, 255, D_FAST, EASE_OUT);
    this.host.uiSound("hover");
  }

  private buildPage(): void {
    const s = this.s;
    const cardW = sc(G_CARD_W, s);
    const cardH = sc(G_CARD_H, s);
    const thumbW = sc(G_THUMB_W, s);
    const thumbH = sc(G_THUMB_H, s);
    const gapX = sc(G_GAP, s);
    const gapY = sc(18, s);
    const x0 = this.ox + sc(G_X0, s);
    const y0 = this.oy + sc(G_Y0, s);
    const pad = sc(12, s);
    const slide = sc(10, s);
    const start = this.page * G_PER_PAGE;
    const list = this.entries.slice(start, start + G_PER_PAGE);
    const reveals: (() => void)[] = [];
    list.forEach((entry, i) => {
      const col = i % 3;
      const row = Math.floor(i / 3);
      const rect = { x: x0 + col * (cardW + gapX), y: y0 + row * (cardH + gapY), w: cardW, h: cardH };
      const open = this.host.unlocked(entry.flag);
      const card = this.g.add(
        new MBox(rect, 312, {
          panel: {
            radius: sc(8, s),
            top: open ? MC.glassTop : [16, 20, 44, 190],
            bottom: open ? MC.glassBottom : [10, 13, 32, 200],
            border: open ? [255, 255, 255, 46] : [255, 255, 255, 22],
            borderW: s,
            shadow: { blur: sc(22, s), dy: sc(8, s), alpha: 0.4 },
          },
          opacity: 0,
        }),
      );
      const num = i + start + 1 < 10 ? `0${i + start + 1}` : `${i + start + 1}`;
      const titleFs = sc(15, s);
      const numFs = sc(13, s);
      const title = this.g.add(
        new MBox({ x: rect.x + pad, y: rect.y + thumbH + sc(16, s), w: rect.w - pad * 2, h: sc(44, s) }, 314, {
          fontSize: titleFs,
          lineHeight: 1.35,
          opacity: 0,
        }),
      );
      const thumbRect = { x: rect.x + pad, y: rect.y + pad, w: thumbW, h: thumbH };
      let thumb: MBox | null = null;
      let img: number | null = null;
      if (open) {
        title.setText([seg(`NO.${num}`, MC.accent, numFs, true), seg("\n", null), seg(entry.title, MC.text, titleFs)]);
        img = api.layer.create("image");
        this.g.add({ free: () => api.layer.free(img!) });
        try {
          api.layer.loadImageScaled(img, entry.image, thumbW, thumbH, "fore");
        } catch (e) {
          api.sys.log(`ギャラリー画像を読めません: ${entry.image}: ${e}`, "warn");
        }
        api.layer.set(img, { x: thumbRect.x, y: thumbRect.y, z: 313, visible: true, opacity: 0 }, "fore");
      } else {
        title.setText([seg(`NO.${num}`, withAlpha(MC.textDim, 200), numFs, true), seg("\n", null), seg("？？？", withAlpha(MC.textDim, 220), titleFs)]);
        thumb = this.g.add(
          new MBox(thumbRect, 313, {
            panel: { radius: sc(4, s), top: [8, 10, 24, 235], bottom: [12, 15, 34, 235], border: [255, 255, 255, 20], borderW: s },
            fontSize: sc(56, s),
            lineHeight: 1.2,
            align: "center",
            marginY: sc(44, s),
            opacity: 0,
          }),
        );
        thumb.setText([seg("？", withAlpha(MC.textDim, 170), sc(52, s)), seg("\nLOCKED", withAlpha(MC.textDim, 150), numFs, true)]);
      }
      const over = this.g.add(
        new MBox(rect, 315, {
          panel: {
            radius: sc(8, s),
            top: [238, 198, 114, open ? 22 : 10],
            bottom: [238, 198, 114, open ? 8 : 4],
            border: open ? MC.accent : withAlpha(MC.accent, 130),
            borderW: 1.5 * s,
          },
          opacity: 0,
        }),
      );
      this.cards.push({ rect, over, entry, open });
      // 出現: 全カードの画像まで読み終えてから一括で（できた順に出すと 1 枚ずつ遅れて見える）
      reveals.push(() => {
        card.appear(0, slide, D_OPEN);
        title.appear(0, slide, D_OPEN);
        if (thumb !== null) thumb.appear(0, slide, D_OPEN);
        if (img !== null) {
          api.layer.set(img, { y: thumbRect.y + slide }, "fore");
          api.layer.move(img, { toY: thumbRect.y, toOpacity: 255, timeMs: D_OPEN, accel: EASE_OUT }, "fore");
        }
        over.show(true);
      });
    });
    reveals.forEach((f) => f());
  }

  /** 全画面表示。クリック/Enter/Esc で戻る。 */
  private async view(events: UiEvents, entry: GalleryEntry): Promise<void> {
    const sw = this.host.screenW;
    const sh = this.host.screenH;
    const s = this.s;
    const v = new MGroup();
    try {
      const black = v.add(makeVeil(sw, sh, 340, 255, 255));
      black.show(true);
      const img = api.layer.create("image");
      v.add({ free: () => api.layer.free(img) });
      try {
        api.layer.loadImage(img, entry.image, "fore");
      } catch (e) {
        api.sys.log(`ギャラリー画像を読めません: ${entry.image}: ${e}`, "warn");
      }
      api.layer.set(img, { x: 0, y: 0, z: 341, visible: true, opacity: 0 }, "fore");
      black.go(0, 0, 255, D_NORM, EASE_OUT);
      api.layer.move(img, { toOpacity: 255, timeMs: D_SLOW + 60, accel: EASE_OUT }, "fore");
      const capH = sc(64, s);
      const capFs = sc(16, s);
      const cap = v.add(
        new MBox({ x: 0, y: sh - capH, w: sw, h: capH }, 342, {
          panel: { top: [0, 0, 0, 0], bottom: [0, 0, 0, 150] },
          fontSize: capFs,
          lineHeight: 1.3,
          align: "center",
          marginY: sc(26, s),
          opacity: 0,
        }),
      );
      cap.setText([seg(entry.title, withAlpha(MC.text, 230), capFs), seg("　　クリックで戻る", withAlpha(MC.textSub, 190), sc(13, s))]);
      cap.show(true);
      await sleep(D_SLOW + 60);
      cap.go(0, sh - capH, 255, D_SLOW, EASE_OUT);
      events.clear();
      while (true) {
        const ev = await events.next();
        if (ev.kind === "click") break;
        if (ev.kind === "key" && (ev.key === "Escape" || ev.key === "Enter" || ev.key === " ")) break;
      }
      api.layer.move(img, { toOpacity: 0, timeMs: D_NORM, accel: EASE_IN }, "fore");
      cap.go(0, sh - capH, 0, D_NORM, EASE_IN);
      black.go(0, 0, 0, D_NORM, EASE_IN);
      await sleep(D_NORM + 20);
    } finally {
      v.freeAll();
    }
  }
}
