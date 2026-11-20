// noiria:kag/mtitle — モダン UI の専用タイトル画面（[titlescreen]）とギャラリー（おまけ）。
// 値の import は mui.ts / ui.ts / api のみ（player.ts → mtitle.ts → mui.ts の一方向）。tsint 互換構文のみ。

import * as api from "noiria:api";
import { UiEvents, inRect } from "noiria:kag/ui";
import type { Rect } from "noiria:kag/ui";
import {
  MBox,
  MGroup,
  MC,
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
    const segs: api.Segment[] = [seg(it.def.label, main, 31, true)];
    if (subText !== "") segs.push(seg("    " + subText, sub, 15));
    return segs;
  }

  /** タイトルを出し、項目が選ばれるまで待つ。ジャンプ先を返す（ロードなど別経路のジャンプなら null）。 */
  async run(): Promise<TitleResult | null> {
    const o = this.opts;
    const sw = this.host.screenW;
    const sh = this.host.screenH;
    const events = new UiEvents();
    try {
      // 左側の暗い面（文字の可読性）と下端の暗幕
      const shade = this.g.add(
        new MBox({ x: 0, y: 0, w: 860, h: sh }, Z_TITLE, {
          panel: { top: [4, 7, 22, 220], bottom: [4, 7, 22, 0], horizontal: true },
          opacity: 0,
        }),
      );
      const foot = this.g.add(
        new MBox({ x: 0, y: sh - 140, w: sw, h: 140 }, Z_TITLE, {
          panel: { top: [4, 7, 22, 0], bottom: [4, 7, 22, 170] },
          opacity: 0,
        }),
      );
      this.fadeAll.push(shade);
      this.fadeAll.push(foot);
      shade.show(true);
      foot.show(true);
      shade.go(0, 0, 255, 320, EASE_OUT);
      foot.go(0, sh - 140, 255, 320, EASE_OUT);

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
      if (o.tagline) addText({ x: LX + 4, y: 92, w: 700, h: 26 }, [seg(o.tagline, withAlpha(MC.accent, 230), 15)], 15);
      const nameW = estimateWidth(o.name, 80) + 40;
      const glow = (width: number, alpha: number): void => {
        const b = this.g.add(
          new MBox({ x: LX, y: 122, w: nameW + 40, h: 110 }, Z_TITLE + 1, {
            fontSize: 80,
            lineHeight: 1.25,
            opacity: 0,
            outlineWidth: width,
          }),
        );
        const s = seg(o.name, [255, 214, 140, 0], 80);
        s.edgeColor = [255, 206, 120, alpha];
        b.setText([s]);
        logo.push(b);
      };
      glow(14, 9);
      glow(9, 14);
      glow(5, 22);
      glow(2, 34);
      addText({ x: LX, y: 122, w: nameW + 40, h: 110 }, [seg(o.name, [255, 247, 226, 255], 80, true)], 80);
      if (o.subtitle) addText({ x: LX + 6, y: 236, w: 700, h: 34 }, [seg(o.subtitle, MC.textSub, 21)], 21);
      const rule = this.g.add(
        new MBox({ x: LX + 6, y: 286, w: 64, h: 2 }, Z_TITLE + 2, {
          panel: { top: MC.accent, radius: 1 },
          opacity: 0,
        }),
      );
      logo.push(rule);

      // メニュー項目（左に細い縦線を通し、ホバー中の項目だけアクセント色のバーが重なる）
      const spine = this.g.add(
        new MBox({ x: ITEM_X - 21, y: ITEM_Y0 + 4, w: 1, h: (o.menu.length - 1) * ITEM_PITCH + 46 }, Z_TITLE + 2, {
          panel: { top: [255, 255, 255, 0], bottom: [255, 255, 255, 0], ramp: 0 },
          opacity: 0,
        }),
      );
      spine.setPanel({ top: [255, 255, 255, 70], bottom: [255, 255, 255, 14] });
      logo.push(spine);
      const hasSave = this.host.hasSave();
      o.menu.forEach((def, i) => {
        const enabled = !(def.action === "continue" && !hasSave);
        const rect = { x: ITEM_X, y: ITEM_Y0 + i * ITEM_PITCH, w: 460, h: 54 };
        const text = this.g.add(
          new MBox(rect, Z_TITLE + 3, { fontSize: 31, lineHeight: 1.3, opacity: 0, outlineWidth: 1, outlineColor: [0, 0, 0, 110] }),
        );
        const bar = this.g.add(
          new MBox({ x: ITEM_X - 22, y: rect.y + 10, w: 3, h: 32 }, Z_TITLE + 3, {
            panel: { top: MC.accent, radius: 1.5 },
            opacity: 0,
          }),
        );
        const line = this.g.add(
          new MBox({ x: ITEM_X, y: rect.y + 46, w: estimateWidth(def.label, 31) + 14, h: 2 }, Z_TITLE + 3, {
            panel: { top: MC.accent, radius: 1 },
            crop: 0,
            visible: false,
          }),
        );
        const it: TItem = { def, text, bar, line, hit: { x: ITEM_X - 30, y: rect.y - 4, w: 520, h: 62 }, enabled };
        text.setText(this.itemSegs(it, false));
        this.items.push(it);
      });
      if (o.version) {
        const v = this.g.add(
          new MBox({ x: sw - 640, y: sh - 40, w: 592, h: 24 }, Z_TITLE + 2, {
            fontSize: 14,
            lineHeight: 1.3,
            align: "right",
            opacity: 0,
          }),
        );
        v.setText([seg(o.version, withAlpha(MC.textSub, 190), 14)]);
        logo.push(v);
      }

      logo.forEach((b) => this.fadeAll.push(b));
      // 登場: ロゴ → メニューの順にフェードイン
      await sleep(180);
      for (let i = 0; i < logo.length; i++) {
        logo[i].show(true);
        logo[i].put(logo[i].rect.x - 16, logo[i].rect.y, 0);
        logo[i].go(logo[i].rect.x, logo[i].rect.y, 255, D_SLOW + 80, EASE_OUT);
        if (i === 2 || i === 4) await sleep(70);
      }
      await sleep(D_NORM);
      for (let i = 0; i < this.items.length; i++) {
        const it = this.items[i];
        it.text.show(true);
        it.line.show(true);
        it.text.put(it.text.rect.x - 22, it.text.rect.y, 0);
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
    this.items.forEach((it, i) => {
      it.text.go(it.text.rect.x + (i === this.focus ? 14 : 0), it.text.rect.y, 0, D_NORM, EASE_IN);
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
    it.text.setText(this.itemSegs(it, hot));
    it.text.go(it.text.rect.x + (hot ? 14 : 0), it.text.rect.y, 255, D_FAST + 20, hot ? EASE_OUT : EASE_IN);
    it.bar.go(it.bar.rect.x, it.bar.rect.y, hot ? 255 : 0, D_FAST, hot ? EASE_OUT : EASE_IN);
    // アクセントの線が左から伸びる（外れると縮む）。文字と一緒に右へ寄る
    it.line.go(it.line.rect.x + (hot ? 14 : 0), it.line.rect.y, 255, D_NORM, hot ? EASE_OUT : EASE_IN, hot ? it.line.rect.w : 0);
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
const G_CARD_H = 262;
const G_THUMB_W = 336;
const G_THUMB_H = 189;
const G_GAP = 28;
const G_X0 = 72;
const G_Y0 = 176;
const G_PER_PAGE = 6;

/** 解放済みの CG のサムネイルを別スレッドで先に作っておく（ギャラリーを開くたびに待たない）。 */
export function prefetchGalleryThumbs(images: string[]): void {
  if (images.length > 0) api.sys.prefetchImagesScaled(images, G_THUMB_W, G_THUMB_H);
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

  constructor(host: GalleryHost, entries: GalleryEntry[]) {
    this.host = host;
    this.entries = entries;
  }

  async run(): Promise<void> {
    const sw = this.host.screenW;
    const sh = this.host.screenH;
    const events = new UiEvents();
    const veil = this.g.add(makeVeil(sw, sh, 310, 250, 253));
    veil.show(true);
    veil.go(0, 0, 255, D_OPEN, EASE_OUT);
    addHeading(this.g, 311, 72, 40, "GALLERY", "おまけ");
    const unlockedCount = this.entries.filter((e) => this.host.unlocked(e.flag)).length;
    const count = this.g.add(
      new MBox({ x: sw - 72 - 360, y: 64, w: 360, h: 30 }, 311, { fontSize: 17, lineHeight: 1.3, align: "right", opacity: 0 }),
    );
    count.setText([seg(`${unlockedCount} / ${this.entries.length}`, MC.accent, 20, true), seg("   解放", MC.textSub, 14)]);
    count.show(true);
    count.go(count.rect.x, count.rect.y, 255, D_OPEN, EASE_OUT);
    const hint = this.g.add(
      new MBox({ x: 72, y: sh - 44, w: sw - 144, h: 26 }, 311, { fontSize: 14, lineHeight: 1.3, align: "right", opacity: 0 }),
    );
    hint.setText([seg("クリックで拡大　←→ 選択　Enter 決定　Esc / 右クリックで戻る", withAlpha(MC.textSub, 190), 14)]);
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
    const start = this.page * G_PER_PAGE;
    const list = this.entries.slice(start, start + G_PER_PAGE);
    const reveals: (() => void)[] = [];
    list.forEach((entry, i) => {
      const col = i % 3;
      const row = Math.floor(i / 3);
      const rect = { x: G_X0 + col * (G_CARD_W + G_GAP), y: G_Y0 + row * (G_CARD_H + 24), w: G_CARD_W, h: G_CARD_H };
      const open = this.host.unlocked(entry.flag);
      const card = this.g.add(
        new MBox(rect, 312, {
          panel: {
            radius: 8,
            top: open ? MC.glassTop : [16, 20, 44, 190],
            bottom: open ? MC.glassBottom : [10, 13, 32, 200],
            border: open ? [255, 255, 255, 46] : [255, 255, 255, 22],
            borderW: 1,
            shadow: { blur: 22, dy: 8, alpha: 0.4 },
          },
          opacity: 0,
        }),
      );
      const num = i + start + 1 < 10 ? `0${i + start + 1}` : `${i + start + 1}`;
      const title = this.g.add(
        new MBox({ x: rect.x + 12, y: rect.y + G_THUMB_H + 20, w: rect.w - 24, h: 54 }, 314, {
          fontSize: 17,
          lineHeight: 1.35,
          opacity: 0,
        }),
      );
      const thumbRect = { x: rect.x + 12, y: rect.y + 12, w: G_THUMB_W, h: G_THUMB_H };
      let thumb: MBox | null = null;
      let img: number | null = null;
      if (open) {
        title.setText([seg(`NO.${num}`, MC.accent, 13, true), seg("\n", null), seg(entry.title, MC.text, 17)]);
        img = api.layer.create("image");
        this.g.add({ free: () => api.layer.free(img!) });
        try {
          api.layer.loadImageScaled(img, entry.image, G_THUMB_W, G_THUMB_H, "fore");
        } catch (e) {
          api.sys.log(`ギャラリー画像を読めません: ${entry.image}: ${e}`, "warn");
        }
        api.layer.set(img, { x: thumbRect.x, y: thumbRect.y, z: 313, visible: true, opacity: 0 }, "fore");
      } else {
        title.setText([seg(`NO.${num}`, withAlpha(MC.textDim, 200), 13, true), seg("\n", null), seg("？？？", withAlpha(MC.textDim, 220), 17)]);
        thumb = this.g.add(
          new MBox(thumbRect, 313, {
            panel: { radius: 4, top: [8, 10, 24, 235], bottom: [12, 15, 34, 235], border: [255, 255, 255, 20], borderW: 1 },
            fontSize: 56,
            lineHeight: 1.2,
            align: "center",
            marginY: 44,
            opacity: 0,
          }),
        );
        thumb.setText([seg("？", withAlpha(MC.textDim, 170), 52), seg("\nLOCKED", withAlpha(MC.textDim, 150), 13, true)]);
      }
      const over = this.g.add(
        new MBox(rect, 315, {
          panel: {
            radius: 8,
            top: [238, 198, 114, open ? 22 : 10],
            bottom: [238, 198, 114, open ? 8 : 4],
            border: open ? MC.accent : withAlpha(MC.accent, 130),
            borderW: 1.5,
          },
          opacity: 0,
        }),
      );
      this.cards.push({ rect, over, entry, open });
      // 出現: 全カードの画像まで読み終えてから一括で（できた順に出すと 1 枚ずつ遅れて見える）
      reveals.push(() => {
        card.appear(0, 10, D_OPEN);
        title.appear(0, 10, D_OPEN);
        if (thumb !== null) thumb.appear(0, 10, D_OPEN);
        if (img !== null) {
          api.layer.set(img, { y: thumbRect.y + 10 }, "fore");
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
      const cap = v.add(
        new MBox({ x: 0, y: sh - 64, w: sw, h: 64 }, 342, {
          panel: { top: [0, 0, 0, 0], bottom: [0, 0, 0, 150] },
          fontSize: 16,
          lineHeight: 1.3,
          align: "center",
          marginY: 26,
          opacity: 0,
        }),
      );
      cap.setText([seg(entry.title, withAlpha(MC.text, 230), 16), seg("　　クリックで戻る", withAlpha(MC.textSub, 190), 13)]);
      cap.show(true);
      await sleep(D_SLOW + 60);
      cap.go(0, sh - 64, 255, D_SLOW, EASE_OUT);
      events.clear();
      while (true) {
        const ev = await events.next();
        if (ev.kind === "click") break;
        if (ev.kind === "key" && (ev.key === "Escape" || ev.key === "Enter" || ev.key === " ")) break;
      }
      api.layer.move(img, { toOpacity: 0, timeMs: D_NORM, accel: EASE_IN }, "fore");
      cap.go(0, sh - 64, 0, D_NORM, EASE_IN);
      black.go(0, 0, 0, D_NORM, EASE_IN);
      await sleep(D_NORM + 20);
    } finally {
      v.freeAll();
    }
  }
}
