// noiria:kag/mmenu — モダン UI のメニュー画面（セーブ/ロード/設定）とバックログ。
// 値の import は api / ui.ts / menu.ts（純関数のみ）/ mui.ts / mhud.ts（確認ダイアログ）だけ
// （player.ts → mmenu.ts の一方向）。tsint 互換構文のみ（Array.from なし、for...of のループ変数を
// クロージャへ閉じ込めない、パラメータプロパティなし）。

import * as api from "noiria:api";
import { UiEvents, inRect } from "noiria:kag/ui";
import type { Rect } from "noiria:kag/ui";
import {
  slotsOfPage,
  slotLabel,
  parseSlotMeta,
  emptySlotMeta,
  sliderFromX,
  moveGrid,
  PAGE_COUNT,
  unclosedBrackets,
} from "noiria:kag/menu";
import type { MenuHost, MenuTab, SlotMeta, SettingKey } from "noiria:kag/menu";
import {
  MBox,
  MImage,
  MGroup,
  MC,
  D_FAST,
  D_NORM,
  D_SLOW,
  D_OPEN,
  D_CLOSE,
  D_SWAP_OUT,
  D_SWAP_IN,
  D_TAB,
  D_HOVER,
  EASE_OUT,
  EASE_IN,
  withAlpha,
  sleep,
  seg,
  estimateWidth,
  addHeading,
  makeVeil,
} from "noiria:kag/mui";
import { modernConfirm } from "noiria:kag/mhud";

// ---------- レイアウト（1280x720 の設計座標。画面が違えば中央寄せ） ----------

const DESIGN_W = 1280;
const DESIGN_H = 720;
const Z0 = 311;

const GRID_X = 72;
const GRID_Y = 164;
const CARD_W = 360;
const CARD_H = 262;
const GAP_X = 28;
const GAP_Y = 12;
const THUMB_W = 336;
const THUMB_H = 189;
/** カード下段の文字（章名・抜粋）に使える幅。レイヤの幅 332 に余裕を持たせる。 */
const CARD_TEXT_W = 324;

/** 幅 maxW に収まるよう末尾を … で切る（estimateWidth の見積もり。収まるならそのまま）。純関数。 */
export function fitWidth(text: string, size: number, maxW: number): string {
  if (estimateWidth(text, size) <= maxW) return text;
  let k = text.length - 1;
  while (k > 0 && estimateWidth(text.slice(0, k) + "…", size) > maxW) k--;
  return text.slice(0, k) + "…";
}

/** 抜粋を幅 maxW に収める。切ったときは … の後ろで、閉じられなくなった括弧を閉じる（閉じ括弧の分も幅に含める）。純関数。 */
export function fitExcerpt(text: string, size: number, maxW: number): string {
  const flat = text.replace(/\s+/g, " ").trim();
  if (estimateWidth(flat, size) <= maxW) return flat;
  let k = flat.length - 1;
  while (k > 0) {
    const body = flat.slice(0, k);
    const shown = (body.endsWith("…") ? body.slice(0, body.length - 1) : body) + "…" + unclosedBrackets(body);
    if (estimateWidth(shown, size) <= maxW) return shown;
    k--;
  }
  return "…";
}

/** セーブ一覧の絵を別スレッドで先に復号しておく（開くたびに待たない）。 */
export function prefetchSlotThumbs(): void {
  api.sys.prefetchSaveThumbs(THUMB_W, THUMB_H);
}

interface Hot {
  key: string;
  rect: Rect;
}

interface CardView {
  rect: Rect;
  bg: MBox;
  over: MBox;
  img: MImage;
  empty: MBox;
  scrim: MBox;
  label: MBox;
  date: MBox;
  chap: MBox;
  exc: MBox;
  del: MBox;
  slot: string | null;
  used: boolean;
  hasThumb: boolean;
  lifted: boolean;
}

interface SettingRow {
  key: SettingKey | "reset" | "title" | "quit";
  label: string;
  kind: "slider" | "choice" | "button";
  choices: { label: string; value: number | boolean }[];
  col: number;
  /** 行の左上（画面座標）。 */
  rect: Rect;
}

interface RowView {
  focusBg: MBox;
  bar: MBox;
  fill: MBox | null;
  knob: MBox | null;
  value: MBox | null;
  pill: MBox | null;
  segs: MBox[];
  track: Rect;
  seg: Rect;
}

export class ModernSystemMenu {
  private host: MenuHost;
  private events: UiEvents | null = null;
  private chrome = new MGroup();
  /** appearAll で一括して現れるもの（ページ送り・ヒント・設定の行）。 */
  private content = new MGroup();
  /** 見出し・カード・ページ送りの下線: 現れ方を個別に決めるので appearAll の対象にしない。 */
  private fixed = new MGroup();
  /** 見出しが現れるフェードの長さ（開くとき D_OPEN、タブ切替は 0）。中身（カード・設定の行）は常に即時。 */
  private revealMs = D_OPEN;
  /** セーブ一覧を現し終えたか（それまでは下線などを不透明度 0 で置く）。 */
  private slotsLive = false;
  private tab: MenuTab = "save";
  private ox = 0;
  private oy = 0;
  private hots: Hot[] = [];
  private metas = new Map<string, SlotMeta>();
  private page = 1;
  private sel = -1;
  private busy = false;
  // クロームの部品
  private navBoxes: MBox[] = [];
  private navRects: Rect[] = [];
  private navLine: MBox | null = null;
  private closeBox: MBox | null = null;
  private closeRect: Rect = { x: 0, y: 0, w: 0, h: 0 };
  private hoverKey: string | null = null;
  // スロット一覧
  private cards: CardView[] = [];
  private pagerBoxes: MBox[] = [];
  private pagerRects: Rect[] = [];
  private pagerLine: MBox | null = null;
  // 設定
  private rows: SettingRow[] = [];
  private rowViews: RowView[] = [];

  constructor(host: MenuHost) {
    this.host = host;
  }

  async run(initial: MenuTab): Promise<void> {
    this.ox = Math.floor((this.host.screenW - DESIGN_W) / 2);
    this.oy = Math.floor((this.host.screenH - DESIGN_H) / 2);
    this.events = new UiEvents();
    try {
      this.buildChrome();
      this.loadMetas();
      this.enterTab(initial, false);
      while (true) {
        const ev = await this.events.next();
        const r = await this.handle(ev);
        if (r === "close") break;
      }
      await this.closeAnim();
    } finally {
      this.content.freeAll();
      this.fixed.freeAll();
      this.chrome.freeAll();
      if (this.events) this.events.close();
      this.events = null;
    }
  }

  private r(x: number, y: number, w: number, h: number): Rect {
    return { x: this.ox + x, y: this.oy + y, w, h };
  }

  // ---------- データ ----------

  private loadMetas(): void {
    this.metas = new Map<string, SlotMeta>();
    let list: { slot: string; json: string }[] = [];
    try {
      list = api.sys.saveList();
    } catch (e) {
      api.sys.log(`セーブ一覧を読めません: ${e}`, "error");
    }
    list.forEach((s) => {
      if (s.slot === "auto" || s.slot === "quick" || /^save\d+$/.test(s.slot)) {
        this.metas.set(s.slot, parseSlotMeta(s.slot, s.json));
      }
    });
  }

  private metaOf(slot: string): SlotMeta {
    return this.metas.get(slot) ?? emptySlotMeta(slot);
  }

  private reloadMeta(slot: string): void {
    this.metas.set(slot, parseSlotMeta(slot, api.sys.saveRead(slot)));
  }

  // ---------- 骨組み（暗幕・ナビ・閉じる・ヒント） ----------

  private buildChrome(): void {
    const sw = this.host.screenW;
    const sh = this.host.screenH;
    const veil = this.chrome.add(makeVeil(sw, sh, 310, 250, 253));
    veil.show(true);
    veil.go(0, 0, 255, D_OPEN, EASE_OUT);

    const tabs: { tab: MenuTab; label: string }[] = [
      { tab: "save", label: "SAVE" },
      { tab: "load", label: "LOAD" },
      { tab: "settings", label: "CONFIG" },
    ];
    const FS = 17;
    const gap = 38;
    const closeW = estimateWidth("CLOSE  ×", FS) + 8;
    const widths = tabs.map((t) => estimateWidth(t.label, FS) + 6);
    let total = closeW + 54;
    widths.forEach((w) => {
      total += w + gap;
    });
    let x = 1208 - total;
    this.navBoxes = [];
    this.navRects = [];
    tabs.forEach((t, i) => {
      const rect = this.r(x, 60, widths[i], 28);
      const b = this.chrome.add(new MBox(rect, Z0 + 1, { fontSize: FS, lineHeight: 1.3, visible: false, opacity: 0 }));
      this.navBoxes.push(b);
      this.navRects.push(rect);
      this.hots.push({ key: `tab:${t.tab}`, rect: { x: rect.x - 12, y: rect.y - 10, w: rect.w + 24, h: rect.h + 22 } });
      x += widths[i] + gap;
    });
    this.navLine = this.chrome.add(
      new MBox({ x: this.navRects[0].x, y: this.navRects[0].y + 30, w: this.navRects[0].w, h: 2 }, Z0 + 1, {
        panel: { top: MC.accent, radius: 1 },
        visible: false,
        opacity: 0,
      }),
    );
    // 区切りの縦線 + 閉じる
    const sepX = x + 8;
    const sep = this.chrome.add(
      new MBox(this.r(sepX, 62, 1, 24), Z0 + 1, { panel: { top: MC.lineStrong }, visible: false, opacity: 0 }),
    );
    this.closeRect = this.r(sepX + 28, 60, closeW, 28);
    this.closeBox = this.chrome.add(
      new MBox(this.closeRect, Z0 + 1, { fontSize: FS, lineHeight: 1.3, visible: false, opacity: 0 }),
    );
    this.hots.push({
      key: "close",
      rect: { x: this.closeRect.x - 12, y: this.closeRect.y - 10, w: this.closeRect.w + 24, h: this.closeRect.h + 22 },
    });
    [sep, this.closeBox].forEach((b) => b.appear(0, -8, D_OPEN));
    this.navBoxes.forEach((b) => b.appear(0, -8, D_OPEN));
    this.repaintNav();
  }

  private repaintNav(): void {
    const labels = ["SAVE", "LOAD", "CONFIG"];
    const tabs: MenuTab[] = ["save", "load", "settings"];
    this.navBoxes.forEach((b, i) => {
      const active = tabs[i] === this.tab;
      const hot = this.hoverKey === `tab:${tabs[i]}`;
      b.setText([seg(labels[i], active ? MC.accent : hot ? MC.text : MC.textSub, 17, true)]);
    });
    if (this.closeBox !== null) {
      const hot = this.hoverKey === "close";
      this.closeBox.setText([seg("CLOSE  ×", hot ? MC.text : MC.textSub, 17, true)]);
    }
    const idx = tabs.indexOf(this.tab);
    if (this.navLine !== null && idx >= 0) {
      const nr = this.navRects[idx];
      this.navLine.resizePanel(nr.w, 2);
      this.navLine.go(nr.x, nr.y + 30, 255, D_FAST, EASE_OUT);
    }
  }

  private async closeAnim(): Promise<void> {
    this.content.vanishAll(0, 6, D_CLOSE);
    this.fixed.vanishAll(0, 6, D_CLOSE);
    this.chrome.vanishAll(0, 0, D_CLOSE);
    await sleep(D_CLOSE + 10);
  }

  // ---------- タブ切替 ----------

  private enterTab(tab: MenuTab, animate: boolean): void {
    this.revealMs = animate ? 0 : D_OPEN;
    this.slotsLive = false;
    this.content.freeAll();
    this.fixed.freeAll();
    this.hots = this.hots.filter((h) => h.key.startsWith("tab:") || h.key === "close");
    this.tab = tab;
    this.sel = -1;
    this.cards = [];
    this.pagerBoxes = [];
    this.pagerRects = [];
    this.pagerLine = null;
    this.rows = [];
    this.rowViews = [];
    this.repaintNav();
    if (tab === "settings") {
      addHeading(this.fixed, Z0, this.ox + 72, this.oy + 40, "CONFIG", "設定", this.revealMs);
      this.buildSettings();
    } else {
      addHeading(this.fixed, Z0, this.ox + 72, this.oy + 40, tab === "save" ? "SAVE" : "LOAD", tab === "save" ? "セーブ" : "ロード", this.revealMs);
      this.page = tab === "save" ? this.host.pages.save : this.host.pages.load;
      this.buildSlots();
    }
  }

  // ---------- セーブ/ロードの一覧 ----------

  private cardRect(i: number): Rect {
    const col = i % 3;
    const row = Math.floor(i / 3);
    return this.r(GRID_X + col * (CARD_W + GAP_X), GRID_Y + row * (CARD_H + GAP_Y), CARD_W, CARD_H);
  }

  private pagerLabel(p: number): string {
    return p === 0 ? "A/Q" : String(p);
  }

  private buildSlots(): void {
    // ページ切替: ‹ A/Q 1 2 3 4 5 ›
    const y = 126;
    const FS = 17;
    let x = 72;
    const mk = (key: string, text: string, w: number): void => {
      const rect = this.r(x, y, w, 26);
      const b = this.content.add(new MBox(rect, Z0 + 1, { fontSize: FS, lineHeight: 1.3, visible: false, opacity: 0, align: "center" }));
      b.setText([seg(text, MC.textSub, FS, true)]);
      this.pagerBoxes.push(b);
      this.pagerRects.push(rect);
      this.hots.push({ key, rect: { x: rect.x - 8, y: rect.y - 6, w: rect.w + 16, h: rect.h + 14 } });
      x += w + 14;
    };
    mk("prev", "‹", 18);
    for (let p = 0; p < PAGE_COUNT; p++) mk(`page:${p}`, this.pagerLabel(p), p === 0 ? 42 : 24);
    mk("next", "›", 18);
    this.pagerLine = this.fixed.add(
      new MBox({ x: 0, y: this.oy + y + 28, w: 24, h: 2 }, Z0 + 1, { panel: { top: MC.accent, radius: 1 }, visible: false, opacity: 0, rest: 255 }),
    );
    const hint = this.content.add(
      new MBox(this.r(560, y + 4, 648, 22), Z0 + 1, { fontSize: 13, lineHeight: 1.3, align: "right", visible: false, opacity: 0 }),
    );
    hint.setText([seg("←→↑↓ 選択　Enter 決定　PageUp/Down ページ　Del 削除　Tab タブ　Esc 閉じる", withAlpha(MC.textSub, 170), 13)]);

    for (let i = 0; i < 6; i++) this.buildCard(i);
    // 中身は全部できてから一括で現す（できた順に見えると、枠だけ・サムネイルだけが先に出て「重い」画面に見える）
    this.refreshCards();
    this.repaintPager(false);
    const pr = this.pagerRects[this.page + 1];
    if (this.pagerLine !== null) this.pagerLine.go(pr.x, pr.y + 28, 255, 0, EASE_OUT);
    this.slotsLive = true;
    this.content.appearAll(0, 0, 0);
    this.cards.forEach((c) => this.revealCard(c, 0, 0));
  }

  /** 使うレイヤだけを現す（サムネイルがあれば画像、無ければ EMPTY/NO IMAGE の面。ホバー面と × は 0 のまま）。 */
  private revealCard(c: CardView, dy: number, ms: number): void {
    if (c.slot === null) return;
    c.bg.appear(0, dy, ms);
    c.label.appear(0, dy, ms);
    c.date.appear(0, dy, ms);
    c.chap.appear(0, dy, ms);
    c.exc.appear(0, dy, ms);
    if (c.used) c.scrim.appear(0, dy, ms);
    if (c.hasThumb) c.img.appear(0, dy, ms);
    else c.empty.appear(0, dy, ms);
  }

  /** 表示後にセーブ/削除で中身が変わったカードの、条件付きの面（サムネイル・暗幕・EMPTY）の不透明度を合わせる
   * （画像は不透明度 0 で作るので、visible だけ立てても見えない）。 */
  private settleCard(c: CardView): void {
    const op = (on: boolean): number => (on ? 255 : 0);
    c.scrim.put(c.scrim.rect.x, c.scrim.rect.y, op(c.used));
    c.img.put(c.img.rect.x, c.img.rect.y, op(c.hasThumb));
    c.empty.put(c.empty.rect.x, c.empty.rect.y, op(!c.hasThumb));
  }

  private placeCard(c: CardView, dx: number, opacity: number): void {
    c.bg.put(c.rect.x + dx, c.rect.y, opacity);
    c.label.put(c.label.rect.x + dx, c.label.rect.y, opacity);
    c.date.put(c.date.rect.x + dx, c.date.rect.y, opacity);
    c.chap.put(c.chap.rect.x + dx, c.chap.rect.y, opacity);
    c.exc.put(c.exc.rect.x + dx, c.exc.rect.y, opacity);
    c.empty.put(c.empty.rect.x + dx, c.empty.rect.y, opacity);
    c.scrim.put(c.scrim.rect.x + dx, c.scrim.rect.y, opacity);
    c.img.put(c.img.rect.x + dx, c.img.rect.y, opacity);
  }

  private buildCard(i: number): void {
    const rect = this.cardRect(i);
    const bg = this.fixed.add(
      new MBox(rect, Z0 + 1, {
        panel: {
          radius: 8,
          top: MC.glassTop,
          bottom: MC.glassBottom,
          border: [255, 255, 255, 44],
          borderW: 1,
          shadow: { blur: 22, dy: 8, alpha: 0.38 },
        },
        visible: false,
        opacity: 0,
      }),
    );
    const thumbRect = { x: rect.x + 12, y: rect.y + 12, w: THUMB_W, h: THUMB_H };
    const img = this.fixed.add(new MImage(thumbRect, Z0 + 2));
    const empty = this.fixed.add(
      new MBox(thumbRect, Z0 + 2, {
        panel: { radius: 4, top: [8, 10, 24, 170], bottom: [12, 15, 34, 170], border: [255, 255, 255, 12], borderW: 1 },
        fontSize: 15,
        lineHeight: 1.3,
        align: "center",
        marginY: 82,
        visible: false,
        opacity: 0,
      }),
    );
    const scrim = this.fixed.add(
      new MBox({ x: thumbRect.x, y: thumbRect.y + THUMB_H - 40, w: THUMB_W, h: 40 }, Z0 + 3, {
        panel: { radius: 0, top: [0, 0, 0, 0], bottom: [0, 0, 0, 150] },
        visible: false,
        opacity: 0,
      }),
    );
    const label = this.fixed.add(
      new MBox({ x: thumbRect.x + 8, y: thumbRect.y + 8, w: 64, h: 22 }, Z0 + 4, {
        panel: { radius: 4, top: [8, 10, 24, 205], border: [255, 255, 255, 26], borderW: 1 },
        fontSize: 13,
        lineHeight: 1.3,
        align: "center",
        marginY: 2,
        visible: false,
        opacity: 0,
      }),
    );
    const date = this.fixed.add(
      new MBox({ x: thumbRect.x + THUMB_W - 10 - 190, y: thumbRect.y + THUMB_H - 25, w: 190, h: 20 }, Z0 + 4, {
        fontSize: 12,
        lineHeight: 1.3,
        align: "right",
        visible: false,
        opacity: 0,
      }),
    );
    const chap = this.fixed.add(
      new MBox({ x: rect.x + 14, y: rect.y + 12 + THUMB_H + 10, w: rect.w - 28, h: 24 }, Z0 + 3, { fontSize: 17, lineHeight: 1.3, visible: false, opacity: 0 }),
    );
    const exc = this.fixed.add(
      new MBox({ x: rect.x + 14, y: rect.y + 12 + THUMB_H + 34, w: rect.w - 28, h: 20 }, Z0 + 3, { fontSize: 13, lineHeight: 1.3, visible: false, opacity: 0 }),
    );
    const over = this.fixed.add(
      new MBox(rect, Z0 + 4, {
        panel: {
          radius: 8,
          top: [238, 198, 114, 22],
          bottom: [238, 198, 114, 8],
          border: MC.accent,
          borderW: 1.5,
          shadow: { blur: 26, dy: 10, alpha: 0.5 },
        },
        visible: false,
        opacity: 0,
        rest: 0,
      }),
    );
    const del = this.fixed.add(
      new MBox({ x: thumbRect.x + THUMB_W - 34, y: thumbRect.y + 8, w: 26, h: 26 }, Z0 + 5, {
        panel: { radius: 13, top: [10, 12, 28, 220], border: [255, 255, 255, 90], borderW: 1 },
        fontSize: 18,
        lineHeight: 1.1,
        align: "center",
        marginY: 3,
        visible: false,
        opacity: 0,
        rest: 0,
      }),
    );
    del.setText([seg("×", MC.text, 18)]);
    this.cards.push({ rect, bg, over, img, empty, scrim, label, date, chap, exc, del, slot: null, used: false, hasThumb: false, lifted: false });
    this.hots.push({ key: `card:${i}`, rect });
    this.hots.push({ key: `del:${i}`, rect: { x: thumbRect.x + THUMB_W - 38, y: thumbRect.y + 4, w: 34, h: 34 } });
  }

  private slotsHere(): string[] {
    return slotsOfPage(this.page);
  }

  private visibleCard(c: CardView, v: boolean): void {
    c.bg.show(v);
    c.scrim.show(v && c.used);
    c.label.show(v);
    c.date.show(v);
    c.chap.show(v);
    c.exc.show(v);
    c.over.show(v);
    c.del.show(v && c.used);
    c.empty.show(v && !c.hasThumb);
    c.img.show(v && c.hasThumb);
  }

  private refreshCards(): void {
    const slots = this.slotsHere();
    this.cards.forEach((c, i) => {
      c.slot = i < slots.length ? slots[i] : null;
      c.lifted = false;
      if (c.slot === null) {
        c.used = false;
        c.hasThumb = false;
        this.visibleCard(c, false);
        return;
      }
      this.paintCard(i);
    });
    if (this.sel >= slots.length) this.sel = Math.max(-1, slots.length - 1);
  }

  private paintCard(i: number): void {
    const c = this.cards[i];
    if (c.slot === null) return;
    const m = this.metaOf(c.slot);
    c.used = m.used;
    c.label.setText([seg(slotLabel(c.slot), m.used ? MC.accent : withAlpha(MC.textSub, 220), 13, true)]);
    c.date.setText([seg(m.used ? (m.savedAt !== "" ? m.savedAt : "") : "", MC.text, 12)]);
    if (m.used) {
      c.chap.setText([seg(m.chapter !== "" ? fitWidth(m.chapter, 17, CARD_TEXT_W) : "（無題）", MC.text, 17)]);
      c.exc.setText([seg(fitExcerpt(m.excerpt, 13, CARD_TEXT_W), withAlpha(MC.textSub, 200), 13)]);
    } else {
      c.chap.setText([seg(c.slot === "auto" ? "選択肢で自動保存されます" : "空きスロット", withAlpha(MC.textDim, 200), 15)]);
      c.exc.clearText();
    }
    c.hasThumb = m.used && m.thumb && c.img.loadSaveThumb(c.slot);
    if (!c.hasThumb) c.img.clear();
    c.empty.setText([
      seg(m.used ? "NO IMAGE" : c.slot === "auto" ? "AUTO" : "EMPTY", withAlpha(MC.textDim, 190), 15, true),
    ]);
    c.bg.setPanel({
      radius: 8,
      top: m.used ? MC.glassTop : [18, 22, 48, 96],
      bottom: m.used ? MC.glassBottom : [10, 13, 32, 110],
      border: m.used ? [255, 255, 255, 44] : [255, 255, 255, 14],
      borderW: 1,
      shadow: m.used ? { blur: 22, dy: 8, alpha: 0.38 } : { blur: 12, dy: 4, alpha: 0.18 },
    });
    this.visibleCard(c, true);
  }

  /** カードの面（枠・文字・サムネイル）を dx だけずらした位置へ opacity まで ms で動かす。 */
  private moveCard(c: CardView, dx: number, ms: number, opacity: number, accel: number): void {
    c.bg.go(c.rect.x + dx, c.rect.y, opacity, ms, accel);
    c.label.go(c.label.rect.x + dx, c.label.rect.y, opacity, ms, accel);
    c.date.go(c.date.rect.x + dx, c.date.rect.y, opacity, ms, accel);
    c.chap.go(c.chap.rect.x + dx, c.chap.rect.y, opacity, ms, accel);
    c.exc.go(c.exc.rect.x + dx, c.exc.rect.y, opacity, ms, accel);
    c.empty.go(c.empty.rect.x + dx, c.empty.rect.y, opacity, ms, accel);
    c.scrim.go(c.scrim.rect.x + dx, c.scrim.rect.y, opacity, ms, accel);
    c.img.go(c.img.rect.x + dx, c.img.rect.y, opacity, ms, accel);
  }

  /** 選択カードは動かさず、アクセント色の枠・薄い色味・濃い影だけを重ねる（位置が 1 枚だけずれて見えないように）。 */
  private setLift(i: number, on: boolean): void {
    const c = this.cards[i];
    if (c === undefined || c.slot === null || c.lifted === on) return;
    c.lifted = on;
    const ease = on ? EASE_OUT : EASE_IN;
    c.over.go(c.rect.x, c.rect.y, on ? 255 : 0, D_HOVER, ease);
    c.del.go(c.del.rect.x, c.del.rect.y, on && c.used ? 255 : 0, D_HOVER, ease);
  }

  private setSel(i: number): void {
    if (i === this.sel) return;
    if (this.sel >= 0) this.setLift(this.sel, false);
    this.sel = i;
    if (i >= 0) {
      this.setLift(i, true);
      this.host.uiSound("hover");
    }
  }

  private repaintPager(animate: boolean): void {
    this.pagerBoxes.forEach((b, i) => {
      // 0=prev, 1..PAGE_COUNT=ページ, 最後=next
      let active = false;
      let key = "";
      if (i === 0) key = "prev";
      else if (i === this.pagerBoxes.length - 1) key = "next";
      else {
        key = `page:${i - 1}`;
        active = i - 1 === this.page;
      }
      const hot = this.hoverKey === key;
      const label = i === 0 ? "‹" : i === this.pagerBoxes.length - 1 ? "›" : this.pagerLabel(i - 1);
      b.setText([seg(label, active ? MC.accent : hot ? MC.text : MC.textSub, 17, true)]);
    });
    if (this.pagerLine !== null) {
      const r = this.pagerRects[this.page + 1];
      this.pagerLine.resizePanel(r.w, 2);
      if (animate) this.pagerLine.go(r.x, r.y + 28, 255, D_OPEN, EASE_OUT);
      else this.pagerLine.put(r.x, r.y + 28, this.slotsLive ? 255 : 0);
      this.pagerLine.show(true);
    }
  }

  /** ページ送りは即時（旧ページの退場も入場の動きも無しに、同じフレームで入れ替える）。 */
  private async setPage(p: number): Promise<void> {
    const next = Math.max(0, Math.min(PAGE_COUNT - 1, p));
    if (next === this.page) return;
    if (this.sel >= 0) this.setLift(this.sel, false);
    this.page = next;
    if (this.tab === "save") this.host.pages.save = next;
    else this.host.pages.load = next;
    this.refreshCards();
    this.cards.forEach((c) => {
      if (c.slot !== null) this.placeCard(c, 0, 255);
    });
    this.repaintPager(false);
  }

  private async activateCard(i: number): Promise<"close" | undefined> {
    const c = this.cards[i];
    if (c === undefined || c.slot === null) return undefined;
    const slot = c.slot;
    const meta = this.metaOf(slot);
    if (this.tab === "save") {
      if (slot === "auto") {
        this.host.toast("オートセーブ枠には手動で保存できません");
        return undefined;
      }
      if (meta.used) {
        const ok = await this.ask(`${slotLabel(slot)} を上書きしますか？`);
        if (!ok) return undefined;
      }
      if (this.host.save(slot)) {
        this.reloadMeta(slot);
        this.paintCard(i);
        this.settleCard(c);
        c.lifted = false;
        this.setLift(i, true);
        this.host.toast("セーブしました");
      } else {
        this.host.toast("セーブできませんでした");
      }
      return undefined;
    }
    if (!meta.used) return undefined;
    const ok = await this.ask(`${slotLabel(slot)} をロードしますか？\n現在の進行状況は失われます`);
    if (!ok) return undefined;
    const loaded = await this.host.load(slot);
    if (loaded) return "close";
    this.host.toast("ロードできませんでした");
    return undefined;
  }

  private async deleteCard(i: number): Promise<void> {
    const c = this.cards[i];
    if (c === undefined || c.slot === null || !this.metaOf(c.slot).used) return;
    const slot = c.slot;
    const ok = await this.ask(`${slotLabel(slot)} を削除しますか？`);
    if (!ok) return;
    api.sys.saveDelete(slot);
    this.metas.set(slot, emptySlotMeta(slot));
    this.paintCard(i);
    this.settleCard(c);
    c.lifted = false;
    this.setLift(i, true);
  }

  private async ask(text: string): Promise<boolean> {
    const r = await modernConfirm(this.host.screenW, this.host.screenH, text, this.events!);
    this.events!.clear();
    return r;
  }

  // ---------- 設定 ----------

  private buildSettings(): void {
    const h = this.host;
    const stage = (values: number[], labels: string[]): { label: string; value: number | boolean }[] =>
      values.map((v, i) => ({ label: labels[i], value: v }));
    const COL_X = [72, 688];
    const ROW_H = 60;
    const TOP = 196;
    const mkRow = (col: number, idx: number, key: SettingRow["key"], label: string, kind: SettingRow["kind"], choices: SettingRow["choices"], yOff: number): void => {
      this.rows.push({
        key,
        label,
        kind,
        choices,
        col,
        rect: this.r(COL_X[col] - 14, TOP + yOff + idx * ROW_H, 548, ROW_H - 6),
      });
    };
    // 左: SOUND / SYSTEM、右: TEXT
    mkRow(0, 0, "bgmVol", "BGM 音量", "slider", [], 0);
    mkRow(0, 1, "seVol", "SE 音量", "slider", [], 0);
    mkRow(0, 2, "voiceVol", "ボイス音量", "slider", [], 0);
    mkRow(1, 0, "cps", "文字の速さ", "choice", stage(h.cpsValues, h.cpsLabels), 0);
    mkRow(1, 1, "autoWaitMs", "オートの待ち", "choice", stage(h.autoValues, h.autoLabels), 0);
    mkRow(1, 2, "skipUnread", "未読もスキップ", "choice", [{ label: "しない", value: false }, { label: "する", value: true }], 0);
    mkRow(1, 3, "msgOpacity", "メッセージ窓の濃さ", "slider", [], 0);
    const SYS_TOP = 3 * ROW_H + 62;
    mkRow(0, 0, "reset", "初期設定に戻す", "button", [], SYS_TOP);
    if (h.hasTitle) mkRow(0, 1, "title", "タイトルへ戻る", "button", [], SYS_TOP);
    mkRow(0, h.hasTitle ? 2 : 1, "quit", "ゲームを終了", "button", [], SYS_TOP);

    // グループ見出しと細線
    const group = (col: number, y: number, en: string, jp: string): void => {
      const x = COL_X[col];
      const t = this.content.add(new MBox(this.r(x, y, 520, 22), Z0, { fontSize: 14, lineHeight: 1.3, visible: false, opacity: 0 }));
      t.setText([seg(en, MC.accent, 14, true), seg("   " + jp, MC.textSub, 13)]);
      this.content.add(new MBox(this.r(x, y + 28, 520, 1), Z0, { panel: { top: MC.line }, visible: false, opacity: 0 }));
    };
    group(0, 150, "SOUND", "サウンド");
    group(1, 150, "TEXT", "テキスト");
    group(0, TOP + SYS_TOP - 46, "SYSTEM", "システム");

    this.rows.forEach((row, i) => {
      this.hots.push({ key: `row:${i}`, rect: row.rect });
      this.rowViews.push(this.buildRow(row, i));
    });
    // 値の文字を先に入れてから一括で現す。appear は静止位置（knob は左端）へ戻すので、現した直後にもう一度
    // 値へ合わせて動かす
    this.refreshSettings(true);
    this.content.appearAll(0, 0, 0);
    this.refreshSettings(true);
  }

  private buildRow(row: SettingRow, i: number): RowView {
    const x0 = row.rect.x + 14;
    const y0 = row.rect.y;
    const rh = row.rect.h;
    const focusBg = this.content.add(
      new MBox(row.rect, Z0, {
        panel: { radius: 6, top: [255, 255, 255, 14], bottom: [255, 255, 255, 6] },
        visible: false,
        opacity: 0,
        rest: 0,
      }),
    );
    const bar = this.content.add(
      new MBox({ x: row.rect.x, y: y0 + 8, w: 3, h: rh - 16 }, Z0 + 1, {
        panel: { top: MC.accent, radius: 1.5 },
        visible: false,
        opacity: 0,
        rest: 0,
      }),
    );
    const lbl = this.content.add(
      new MBox({ x: x0, y: y0 + 14, w: 200, h: 28 }, Z0 + 1, { fontSize: 18, lineHeight: 1.3, visible: false, opacity: 0 }),
    );
    lbl.setText([seg(row.label, row.kind === "button" ? MC.text : MC.text, 18)]);
    const view: RowView = {
      focusBg,
      bar,
      fill: null,
      knob: null,
      value: null,
      pill: null,
      segs: [],
      track: { x: 0, y: 0, w: 0, h: 0 },
      seg: { x: 0, y: 0, w: 0, h: 0 },
    };
    if (row.kind === "slider") {
      const track = { x: x0 + 214, y: y0 + 26, w: 224, h: 4 };
      view.track = track;
      this.content.add(
        new MBox(track, Z0 + 1, { panel: { top: [255, 255, 255, 46], radius: 2 }, visible: false, opacity: 0 }),
      );
      view.fill = this.content.add(
        new MBox(track, Z0 + 2, { panel: { top: MC.accent, radius: 2 }, visible: false, opacity: 0, crop: 0 }),
      );
      view.knob = this.content.add(
        new MBox({ x: track.x, y: track.y - 6, w: 16, h: 16 }, Z0 + 3, {
          panel: { radius: 8, top: [250, 252, 255, 255], border: MC.accent, borderW: 2, shadow: { blur: 6, dy: 1, alpha: 0.4 } },
          visible: false,
          opacity: 0,
        }),
      );
      view.value = this.content.add(
        new MBox({ x: x0 + 458, y: y0 + 14, w: 60, h: 28 }, Z0 + 1, { fontSize: 17, lineHeight: 1.3, align: "right", visible: false, opacity: 0 }),
      );
      this.hots.push({ key: `slider:${i}`, rect: { x: track.x - 10, y: track.y - 14, w: track.w + 20, h: track.h + 28 } });
    } else if (row.kind === "choice") {
      const n = row.choices.length;
      const cw = 340;
      const seg0 = { x: x0 + 178, y: y0 + 11, w: cw, h: 34 };
      view.seg = seg0;
      this.content.add(
        new MBox(seg0, Z0 + 1, {
          panel: { radius: 6, top: [255, 255, 255, 12], border: [255, 255, 255, 44], borderW: 1 },
          visible: false,
          opacity: 0,
        }),
      );
      const sw = Math.floor(cw / n);
      view.pill = this.content.add(
        new MBox({ x: seg0.x + 2, y: seg0.y + 2, w: sw - 4, h: 30 }, Z0 + 2, {
          panel: { radius: 5, top: [238, 198, 114, 42], bottom: [238, 198, 114, 26], border: MC.accent, borderW: 1 },
          visible: false,
          opacity: 0,
        }),
      );
      row.choices.forEach((c, ci) => {
        const b = this.content.add(
          new MBox({ x: seg0.x + ci * sw, y: seg0.y, w: sw, h: 34 }, Z0 + 3, {
            fontSize: 16,
            lineHeight: 1.3,
            align: "center",
            marginY: 6,
            visible: false,
            opacity: 0,
          }),
        );
        b.setText([seg(c.label, MC.textSub, 16)]);
        view.segs.push(b);
        this.hots.push({ key: `choice:${i}:${ci}`, rect: { x: seg0.x + ci * sw, y: seg0.y, w: sw, h: 34 } });
      });
    } else {
      const chev = this.content.add(
        new MBox({ x: row.rect.x + row.rect.w - 40, y: y0 + 12, w: 24, h: 28 }, Z0 + 1, {
          fontSize: 22,
          lineHeight: 1.1,
          align: "right",
          visible: false,
          opacity: 0,
        }),
      );
      chev.setText([seg("›", MC.textSub, 24)]);
    }
    return view;
  }

  private sliderValue(row: SettingRow): number {
    if (row.kind !== "slider") return 0;
    return this.host.settings[row.key as SettingKey] as number;
  }

  private choiceIndex(row: SettingRow): number {
    let idx = 0;
    row.choices.forEach((c, ci) => {
      if (c.value === this.host.settings[row.key as SettingKey]) idx = ci;
    });
    return idx;
  }

  private refreshSettings(instant: boolean = false): void {
    this.rows.forEach((row, i) => {
      const v = this.rowViews[i];
      if (row.kind === "slider" && v.fill !== null && v.knob !== null && v.value !== null) {
        const val = this.sliderValue(row);
        const w = Math.round((v.track.w * val) / 100);
        v.fill.show(true);
        v.fill.go(v.track.x, v.track.y, 255, instant ? 0 : D_FAST + 40, EASE_OUT, w);
        v.knob.go(v.track.x + w - 8, v.track.y - 6, 255, instant ? 0 : D_FAST + 40, EASE_OUT);
        v.value.setText([seg(`${val}`, MC.accent, 17, true), seg("%", MC.textSub, 13)]);
      } else if (row.kind === "choice" && v.pill !== null) {
        const idx = this.choiceIndex(row);
        const sw = Math.floor(v.seg.w / row.choices.length);
        v.pill.go(v.seg.x + idx * sw + 2, v.seg.y + 2, 255, instant ? 0 : D_NORM, EASE_OUT);
        v.segs.forEach((b, ci) => {
          b.setText([seg(row.choices[ci].label, ci === idx ? MC.accent : MC.textSub, 16, ci === idx)]);
        });
      }
      const on = this.sel === i;
      v.focusBg.go(v.focusBg.rect.x, v.focusBg.rect.y, on ? 255 : 0, D_FAST, on ? EASE_OUT : EASE_IN);
      v.bar.go(v.bar.rect.x, v.bar.rect.y, on ? 255 : 0, D_FAST, on ? EASE_OUT : EASE_IN);
    });
  }

  private setRowSel(i: number): void {
    if (i === this.sel) return;
    this.sel = i;
    if (i >= 0) this.host.uiSound("hover");
    this.refreshSettings();
  }

  private nudge(i: number, delta: number): void {
    const row = this.rows[i];
    if (row.kind === "slider") {
      const cur = this.sliderValue(row);
      const next = Math.max(0, Math.min(100, cur + delta * 10));
      if (next !== cur) this.applySetting(row, next);
    } else if (row.kind === "choice") {
      const idx = this.choiceIndex(row);
      const ni = Math.max(0, Math.min(row.choices.length - 1, idx + delta));
      if (ni !== idx) this.applySetting(row, row.choices[ni].value);
    }
  }

  private applySetting(row: SettingRow, value: number | boolean): void {
    this.host.setSetting(row.key as SettingKey, value);
    this.refreshSettings();
  }

  private async activateButton(key: SettingRow["key"]): Promise<"close" | undefined> {
    if (key === "reset") {
      const ok = await this.ask("設定を初期状態に戻しますか？");
      if (!ok) return undefined;
      this.host.resetSettings();
      this.refreshSettings();
      this.host.toast("初期設定に戻しました");
      return undefined;
    }
    if (key === "title") {
      const ok = await this.ask("タイトルへ戻りますか？\n保存していない進行状況は失われます");
      if (!ok) return undefined;
      this.host.goTitle();
      return "close";
    }
    const ok = await this.ask("ゲームを終了しますか？\n保存していない進行状況は失われます");
    if (!ok) return undefined;
    this.host.quit();
    return "close";
  }

  // ---------- 入力 ----------

  private hit(x: number, y: number): Hot | null {
    for (let i = this.hots.length - 1; i >= 0; i--) {
      const h = this.hots[i];
      if (h.key.startsWith("del:")) {
        const ci = parseInt(h.key.slice(4), 10);
        const c = this.cards[ci];
        if (c === undefined || c.slot === null || !c.used || c.lifted === false) continue;
      }
      if (inRect(h.rect, x, y)) return h;
    }
    return null;
  }

  private async handle(ev: api.InputEvent): Promise<"close" | undefined> {
    if (this.busy) {
      if (ev.kind === "click" || ev.kind === "key") return undefined;
    }
    if (ev.kind === "move") {
      this.onMove(ev.x, ev.y);
      return undefined;
    }
    if (ev.kind === "wheel") {
      if (this.tab !== "settings") await this.setPage(this.page + ((ev.delta ?? 0) > 0 ? -1 : 1));
      return undefined;
    }
    if (ev.kind === "click") {
      if (ev.button === "right") return "close";
      if (ev.button === "left") return await this.onClick(ev.x, ev.y);
      return undefined;
    }
    if (ev.kind === "key") return await this.onKey(ev.key ?? "");
    return undefined;
  }

  private onMove(x: number, y: number): void {
    const h = this.hit(x, y);
    const key = h === null ? null : h.key;
    if (key !== this.hoverKey) {
      this.hoverKey = key;
      this.repaintNav();
      if (this.tab !== "settings") this.repaintPager(false);
    }
    if (h === null) return;
    if (h.key.startsWith("card:")) {
      const i = parseInt(h.key.slice(5), 10);
      if (this.cards[i] && this.cards[i].slot !== null) this.setSel(i);
    } else if (h.key.startsWith("row:") || h.key.startsWith("slider:") || h.key.startsWith("choice:")) {
      this.setRowSel(parseInt(h.key.split(":")[1], 10));
    }
  }

  private async onClick(x: number, y: number): Promise<"close" | undefined> {
    const h = this.hit(x, y);
    if (h === null) return undefined;
    const k = h.key;
    this.host.uiSound("click");
    if (k === "close") return "close";
    if (k.startsWith("tab:")) {
      const t = k.slice(4) as MenuTab;
      if (t !== this.tab) this.enterTab(t, true);
      return undefined;
    }
    if (k === "prev") {
      await this.setPage(this.page - 1);
      return undefined;
    }
    if (k === "next") {
      await this.setPage(this.page + 1);
      return undefined;
    }
    if (k.startsWith("page:")) {
      await this.setPage(parseInt(k.slice(5), 10));
      return undefined;
    }
    if (k.startsWith("del:")) {
      await this.deleteCard(parseInt(k.slice(4), 10));
      return undefined;
    }
    if (k.startsWith("card:")) {
      const i = parseInt(k.slice(5), 10);
      this.setSel(i);
      return await this.activateCard(i);
    }
    if (k.startsWith("slider:")) {
      const i = parseInt(k.slice(7), 10);
      this.sel = i;
      this.applySetting(this.rows[i], sliderFromX(x, this.rowViews[i].track));
      return undefined;
    }
    if (k.startsWith("choice:")) {
      const parts = k.split(":");
      const i = parseInt(parts[1], 10);
      this.sel = i;
      this.applySetting(this.rows[i], this.rows[i].choices[parseInt(parts[2], 10)].value);
      return undefined;
    }
    if (k.startsWith("row:")) {
      const i = parseInt(k.slice(4), 10);
      this.sel = i;
      if (this.rows[i].kind === "button") return await this.activateButton(this.rows[i].key);
      this.refreshSettings();
    }
    return undefined;
  }

  private async onKey(k: string): Promise<"close" | undefined> {
    if (k === "Escape") return "close";
    if (k === "Tab") {
      const order: MenuTab[] = ["save", "load", "settings"];
      this.enterTab(order[(order.indexOf(this.tab) + 1) % order.length], true);
      return undefined;
    }
    return this.tab === "settings" ? await this.onKeySettings(k) : await this.onKeySlots(k);
  }

  private async onKeySlots(k: string): Promise<"close" | undefined> {
    const count = this.slotsHere().length;
    let dx = 0;
    let dy = 0;
    if (k === "ArrowLeft") dx = -1;
    else if (k === "ArrowRight") dx = 1;
    else if (k === "ArrowUp") dy = -1;
    else if (k === "ArrowDown") dy = 1;
    else if (k === "PageUp") {
      await this.setPage(this.page - 1);
      return undefined;
    } else if (k === "PageDown") {
      await this.setPage(this.page + 1);
      return undefined;
    } else if (k === "Enter" || k === " ") {
      if (this.sel < 0) this.setSel(0);
      return await this.activateCard(this.sel);
    } else if (k === "Delete") {
      await this.deleteCard(this.sel);
      return undefined;
    } else return undefined;
    if (this.sel < 0) {
      this.setSel(0);
      return undefined;
    }
    const next = moveGrid(this.sel, count, 3, dx, dy);
    if (next !== null) {
      this.setSel(next);
    } else if (dx !== 0) {
      const row = Math.floor(this.sel / 3);
      const np = this.page + dx;
      if (np >= 0 && np < PAGE_COUNT) {
        await this.setPage(np);
        const n = this.slotsHere().length;
        this.setSel(Math.max(0, Math.min(n - 1, row * 3 + (dx > 0 ? 0 : 2))));
      }
    }
    return undefined;
  }

  private async onKeySettings(k: string): Promise<"close" | undefined> {
    const n = this.rows.length;
    if (k === "ArrowUp") {
      this.setRowSel(this.sel <= 0 ? 0 : this.sel - 1);
    } else if (k === "ArrowDown") {
      this.setRowSel(this.sel < 0 ? 0 : Math.min(n - 1, this.sel + 1));
    } else if (k === "ArrowLeft") {
      if (this.sel >= 0) this.nudge(this.sel, -1);
    } else if (k === "ArrowRight") {
      if (this.sel >= 0) this.nudge(this.sel, 1);
    } else if (k === "Enter" || k === " ") {
      if (this.sel < 0) return undefined;
      const row = this.rows[this.sel];
      if (row.kind === "button") return await this.activateButton(row.key);
      if (row.kind === "choice") {
        this.applySetting(row, row.choices[(this.choiceIndex(row) + 1) % row.choices.length].value);
      }
    }
    return undefined;
  }
}

// ---------- バックログ ----------

export interface BacklogHost {
  screenW: number;
  screenH: number;
  entries: { text: string; name: string | null; voice: string | null; rule?: boolean }[];
  playVoice(path: string): void;
  stopVoice(): void;
}

function utf8Len(s: string): number {
  let n = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c < 0x80) n += 1;
    else if (c < 0x800) n += 2;
    else if (c >= 0xd800 && c <= 0xdbff) {
      n += 4;
      i++;
    } else n += 3;
  }
  return n;
}

const LOG_X = 72;
const LOG_TOP = 136;
const LOG_BOTTOM_PAD = 52;
const LOG_NAME_W = 190;
/** 開いたときに組む履歴の件数（全件を組むと 200 件で 200ms を超える）と、上へ読み進めたときに足す件数。 */
const LOG_FIRST = 40;
const LOG_MORE = 60;
/** 先頭からこれだけ（px）まで近づいたら古い履歴を足す。 */
const LOG_EXTEND_AT = 400;

/** 履歴: 話者名（アクセント色）を左カラム、本文を右カラムに置いた 2 カラム。行間に細い区切り。
 * 全履歴を 1 枚のテキストレイヤに組み、レイヤの y を tween で動かして滑らかにスクロールする
 * （上下は不透明な帯で隠す）。 */
export class ModernBacklog {
  private host: BacklogHost;
  private g = new MGroup();
  private layer = 0;
  private scroll = 0;
  private maxScroll = 0;
  private viewH = 0;
  private starts: { start: number; end: number; entry: BacklogHost["entries"][0] }[] = [];
  private thumb: MBox | null = null;
  private thumbY = 0;
  private voiceStarted = false;
  private colW = 0;
  /** 組んである最古の履歴の番号（それより古いものは、上へ読み進めたときに足す）。 */
  private first = 0;

  constructor(host: BacklogHost) {
    this.host = host;
  }

  async run(): Promise<void> {
    const sw = this.host.screenW;
    const sh = this.host.screenH;
    const events = new UiEvents();
    this.viewH = sh - LOG_TOP - LOG_BOTTOM_PAD;
    try {
      // 暗幕と見出しを先に出し、重い組版（本文・覆い・スクロールバー）は見えない状態で作ってから一括で現す
      const veil = this.g.add(makeVeil(sw, sh, 300, 255, 255));
      veil.show(true);
      veil.go(0, 0, 255, D_OPEN, EASE_OUT);
      addHeading(this.g, 303, 72, 40, "LOG", "履歴");
      const info = this.g.add(
        new MBox({ x: sw - 72 - 560, y: 62, w: 560, h: 30 }, 303, { fontSize: 14, lineHeight: 1.3, align: "right", visible: false, opacity: 0 }),
      );
      info.setText([
        seg(`${this.host.entries.length}`, MC.accent, 20, true),
        seg("   ホイール・↑↓・PageUp/Down でスクロール　♪ はクリックで再生　Esc / 右クリックで閉じる", withAlpha(MC.textSub, 190), 13),
      ]);
      info.appear(0, -8, D_OPEN);
      // 本文レイヤ（最新の LOG_FIRST 件だけを組む。上へ読み進めたら足す）
      this.colW = sw - LOG_X * 2 - 30;
      this.first = Math.max(0, this.host.entries.length - LOG_FIRST);
      const layer = api.layer.create("text");
      this.layer = layer;
      this.g.add({ free: () => api.layer.free(layer) });
      api.text.style(layer, {
        fontSize: 20,
        lineHeight: 1.75,
        boxW: this.colW,
        boxH: 40000,
        marginX: 0,
        marginY: 0,
        color: MC.text,
        outlineWidth: 0,
      });
      this.layout();
      api.layer.set(layer, { x: LOG_X, y: LOG_TOP, z: 301, visible: true, opacity: 0 }, "fore");
      this.scrollTo(this.maxScroll, false);
      // 上下の覆い（本文のはみ出しを隠し、縁は滑らかに溶ける）
      const top = this.g.add(
        new MBox({ x: 0, y: 0, w: sw, h: LOG_TOP + 26 }, 302, {
          panel: { top: [6, 9, 24, 255], bottom: [6, 9, 24, 0], ramp: 0 },
          visible: false,
        }),
      );
      const solidTop = this.g.add(new MBox({ x: 0, y: 0, w: sw, h: LOG_TOP - 14 }, 302, { panel: { top: [6, 9, 24, 255] }, visible: false }));
      const bottom = this.g.add(
        new MBox({ x: 0, y: sh - LOG_BOTTOM_PAD - 18, w: sw, h: LOG_BOTTOM_PAD + 18 }, 302, {
          panel: { top: [6, 9, 24, 0], bottom: [6, 9, 24, 255], ramp: 28 },
          visible: false,
        }),
      );
      [top, solidTop, bottom].forEach((b) => b.show(true));
      // スクロールバー
      const bar = this.g.add(
        new MBox({ x: sw - 54, y: LOG_TOP, w: 2, h: this.viewH }, 303, { panel: { top: [255, 255, 255, 26], radius: 1 }, visible: false, opacity: 0 }),
      );
      this.thumb = this.g.add(
        new MBox({ x: sw - 55, y: LOG_TOP, w: 4, h: 40 }, 304, { panel: { top: withAlpha(MC.accent, 190), radius: 2 }, visible: false, opacity: 0 }),
      );
      this.updateThumb(false);
      api.layer.move(layer, { toOpacity: 255, timeMs: D_OPEN, accel: EASE_OUT }, "fore");
      bar.show(true);
      bar.go(bar.rect.x, bar.rect.y, 255, D_OPEN, EASE_OUT);
      this.thumb.put(this.thumb.rect.x, this.thumbY, 0);
      this.thumb.show(true);
      this.thumb.go(this.thumb.rect.x, this.thumbY, 255, D_OPEN, EASE_OUT);
      while (true) {
        const ev = await events.next();
        if (ev.kind === "wheel") {
          this.scrollTo(this.scroll + ((ev.delta ?? 0) > 0 ? -96 : 96), true);
        } else if (ev.kind === "key") {
          const k = ev.key;
          if (k === "Escape" || k === "h" || k === "H") break;
          if (k === "ArrowUp") this.scrollTo(this.scroll - 72, true);
          else if (k === "ArrowDown") this.scrollTo(this.scroll + 72, true);
          else if (k === "PageUp") this.scrollTo(this.scroll - (this.viewH - 60), true);
          else if (k === "PageDown") this.scrollTo(this.scroll + (this.viewH - 60), true);
          else if (k === "Home") this.scrollTo(0, true);
          else if (k === "End") this.scrollTo(this.maxScroll, true);
        } else if (ev.kind === "click") {
          if (ev.button === "right") break;
          if (ev.button === "left") {
            const entry = this.entryAt(ev.x, ev.y);
            if (entry !== null && entry.voice) {
              this.host.playVoice(entry.voice);
              this.voiceStarted = true;
              continue;
            }
            break;
          }
        }
      }
      if (this.voiceStarted) this.host.stopVoice();
      api.layer.move(layer, { toOpacity: 0, timeMs: D_CLOSE, accel: EASE_IN }, "fore");
      this.g.vanishAll(0, 0, D_CLOSE);
      veil.go(0, 0, 0, D_CLOSE, EASE_IN);
      await sleep(D_CLOSE + 10);
    } finally {
      events.close();
      this.g.freeAll();
    }
  }

  /** `first` 番目以降の履歴を 2 カラムで組む。名前は左（アクセント色）、本文は右カラムにインデントして折り返す。 */
  private layout(): void {
    const colW = this.colW;
    const segs: api.Segment[] = [];
    let bytes = 0;
    const push = (s: api.Segment): void => {
      segs.push(s);
      bytes += utf8Len(s.text);
    };
    const rule = (): void => {
      // for 文より while（tsint は本体が単文の for + 文字列連結で TypeError になる）
      const n = Math.floor(colW / 9);
      let line = "";
      let k = 0;
      while (k < n) {
        line += "\u2500";
        k++;
      }
      push(seg(line + "\n", [255, 255, 255, 34], 9));
    };
    this.starts = [];
    const shown = this.host.entries.slice(this.first);
    shown.forEach((e, idx) => {
      if (idx > 0) rule();
      const start = bytes;
      if (e.rule) {
        push(seg("\n", null));
        return;
      }
      const head = (e.name ? e.name : "") + (e.voice ? (e.name ? "  ♪" : "♪") : "");
      push(seg(head, MC.accent, 18, true));
      push({ text: "", color: null, locate: { x: LOG_NAME_W } });
      push({ text: "", color: null, indent: true });
      push(seg(e.text, MC.text, 20));
      push({ text: "", color: null, indent: false });
      push(seg("\n", null));
      this.starts.push({ start, end: bytes, entry: e });
    });
    api.text.print(this.layer, segs, false, 0);
    const rects = api.text.rects(this.layer);
    const bottom = rects.length > 0 ? rects[rects.length - 1].y + rects[rects.length - 1].h : 0;
    this.maxScroll = Math.max(0, bottom + 24 - this.viewH);
  }

  /** 組んである履歴の先頭近くまで読み進めたら、さらに古い履歴を足して組み直す。レイヤの上に足した高さだけ
   * scroll をずらすので、見えている位置は動かない。足した高さを返す。 */
  private extendOlder(): number {
    if (this.first === 0) return 0;
    const before = this.maxScroll;
    this.first = Math.max(0, this.first - LOG_MORE);
    this.layout();
    const added = this.maxScroll - before;
    this.scroll += added;
    api.layer.cancelMove(this.layer, "fore");
    api.layer.set(this.layer, { y: LOG_TOP - this.scroll }, "fore");
    return added;
  }

  private scrollTo(v: number, animate: boolean): void {
    let target = v;
    if (v === 0) {
      while (this.first > 0) this.extendOlder();
    } else if (target < LOG_EXTEND_AT && this.first > 0) {
      target += this.extendOlder();
    }
    this.scroll = Math.max(0, Math.min(this.maxScroll, target));
    const y = LOG_TOP - this.scroll;
    if (animate) api.layer.move(this.layer, { toY: y, toOpacity: 255, timeMs: 150, accel: EASE_OUT }, "fore");
    else api.layer.set(this.layer, { y }, "fore");
    this.updateThumb(animate);
  }

  private updateThumb(animate: boolean): void {
    if (this.thumb === null) return;
    const total = this.maxScroll + this.viewH;
    const h = Math.max(36, Math.round((this.viewH * this.viewH) / Math.max(1, total)));
    const y = LOG_TOP + Math.round(((this.viewH - h) * this.scroll) / Math.max(1, this.maxScroll));
    this.thumbY = y;
    this.thumb.resizePanel(4, h);
    if (animate) this.thumb.go(this.thumb.rect.x, y, 255, 150, EASE_OUT);
    else this.thumb.put(this.thumb.rect.x, y, 255);
  }

  /** 画面上の座標が属する履歴 1 件（グリフ矩形のバイト位置で引く）。 */
  private entryAt(x: number, y: number): BacklogHost["entries"][0] | null {
    const rects = api.text.rects(this.layer);
    const ox = LOG_X;
    const oy = LOG_TOP - this.scroll;
    for (let i = 0; i < rects.length; i++) {
      const r = rects[i];
      if (x < ox + r.x - 6 || x > ox + r.x + r.w + 6 || y < oy + r.y - 6 || y > oy + r.y + r.h + 6) continue;
      for (let k = 0; k < this.starts.length; k++) {
        const g = this.starts[k];
        if (r.byte_start >= g.start && r.byte_start < g.end) return g.entry;
      }
    }
    return null;
  }
}
