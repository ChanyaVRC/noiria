// noiria:kag/menu — システムメニュー（セーブ/ロード/設定タブ + タイトルへ戻る/終了）。
// 値の import は noiria:api と noiria:kag/ui のみ（player.ts → menu.ts の一方向）。プレイヤーとのやり取りは
// MenuHost インターフェース越し。tsint 互換構文のみ。

import * as api from "noiria:api";
import { UiBox, UiImage, UiGroup, UiEvents, UI_COLORS, inRect, confirmDialog, BASE_W, BASE_H, uiScale, sc } from "noiria:kag/ui";
import type { Rect } from "noiria:kag/ui";

export type MenuTab = "save" | "load" | "settings";

/** 1 ページのスロット数。 */
export const SLOTS_PER_PAGE = 6;
/** 番号つきスロットのページ数（5 ページ = 30 スロット）。ページ 0 はオート/クイック枠。 */
export const NUMBERED_PAGES = 5;
export const PAGE_COUNT = NUMBERED_PAGES + 1;
export const SPECIAL_SLOTS: string[] = ["auto", "quick"];

/** ページ内のスロット名。ページ 0 は auto/quick、1..5 は save1..save30。範囲外は空。純関数。 */
export function slotsOfPage(page: number): string[] {
  if (page === 0) return [...SPECIAL_SLOTS];
  if (page < 1 || page > NUMBERED_PAGES) return [];
  const out: string[] = [];
  for (let i = 0; i < SLOTS_PER_PAGE; i++) out.push(`save${(page - 1) * SLOTS_PER_PAGE + i + 1}`);
  return out;
}

/** 表示用ラベル。auto → "AUTO"、quick → "QUICK"、saveN → "No.01"。純関数。 */
export function slotLabel(slot: string): string {
  if (slot === "auto") return "AUTO";
  if (slot === "quick") return "QUICK";
  const m = /^save(\d+)$/.exec(slot);
  if (!m) return slot;
  const n = parseInt(m[1], 10);
  return `No.${n < 10 ? "0" : ""}${n}`;
}

/** セーブ一覧に出すメタ情報。旧セーブ（savedAt/excerpt/thumb 無し）は空欄/空表示になる。 */
export interface SlotMeta {
  slot: string;
  used: boolean;
  savedAt: string;
  chapter: string;
  excerpt: string;
  thumb: boolean;
}

export function emptySlotMeta(slot: string): SlotMeta {
  return { slot, used: false, savedAt: "", chapter: "", excerpt: "", thumb: false };
}

/** セーブ JSON からメタ情報を取り出す。壊れた JSON は空きスロット扱い。純関数。 */
export function parseSlotMeta(slot: string, json: string | null): SlotMeta {
  if (json === null || json === undefined) return emptySlotMeta(slot);
  let d: any;
  try {
    d = JSON.parse(json);
  } catch {
    return emptySlotMeta(slot);
  }
  if (d === null || typeof d !== "object") return emptySlotMeta(slot);
  const str = (v: unknown): string => (typeof v === "string" ? v : "");
  return {
    slot,
    used: true,
    savedAt: str(d.savedAt) !== "" ? str(d.savedAt) : str(d.date),
    chapter: str(d.title),
    excerpt: closeExcerpt(str(d.excerpt)),
    thumb: d.thumb === true,
  };
}

const BRACKET_PAIRS: { [open: string]: string } = {
  "「": "」",
  "『": "』",
  "（": "）",
  "(": ")",
  "【": "】",
  "〈": "〉",
  "《": "》",
};

/** 閉じられていない開き括弧に対応する閉じ括弧を、内側のものから順に並べて返す。純関数。 */
export function unclosedBrackets(s: string): string {
  const stack: string[] = [];
  for (let i = 0; i < s.length; i++) {
    const ch = s[i];
    const close = BRACKET_PAIRS[ch];
    if (close !== undefined) stack.push(close);
    else if (stack.length > 0 && stack[stack.length - 1] === ch) stack.pop();
  }
  let out = "";
  for (let i = stack.length - 1; i >= 0; i--) out += stack[i];
  return out;
}

/** 本文の冒頭を 1 行の抜粋にする（改行・連続空白を 1 つの空白へ、max 文字を超えたら … で切る）。切った結果
 * 閉じられなくなった括弧は … の後ろで閉じる（「…」のように。表示は max + 1 + 閉じ括弧の数 文字になる）。純関数。 */
export function makeExcerpt(text: string, max: number): string {
  const flat = text.replace(/\s+/g, " ").trim();
  let n = 0;
  let out = "";
  for (let i = 0; i < flat.length; i++) {
    const c = flat.codePointAt(i)!;
    const ch = c > 0xffff ? flat.slice(i, i + 2) : flat[i];
    if (c > 0xffff) i++;
    if (n >= max) {
      const body = out.endsWith("…") ? out.slice(0, out.length - 1) : out;
      return body + "…" + unclosedBrackets(body);
    }
    out += ch;
    n++;
  }
  return out;
}

/** 保存済みの抜粋が切り詰められたもの（… で終わる）で括弧が閉じていなければ閉じる。旧版が書いた抜粋用。純関数。 */
export function closeExcerpt(ex: string): string {
  return ex.endsWith("…") ? ex + unclosedBrackets(ex.slice(0, ex.length - 1)) : ex;
}

/** スライダーのクリック位置 → 0..100（10 刻み）。track の左端が 0、右端が 100。純関数。 */
export function sliderFromX(x: number, track: Rect): number {
  const t = (x - track.x) / track.w;
  const pct = Math.round(Math.max(0, Math.min(1, t)) * 10) * 10;
  return pct;
}

/** グリッド上の移動。cols 列で count 個のセル。範囲外へ出るなら null（呼び出し側がページ送り等に使う）。純関数。 */
export function moveGrid(sel: number, count: number, cols: number, dx: number, dy: number): number | null {
  const row = Math.floor(sel / cols);
  const col = sel % cols;
  const nc = col + dx;
  const nr = row + dy;
  if (nc < 0 || nc >= cols || nr < 0) return null;
  const next = nr * cols + nc;
  if (next >= count) return null;
  return next;
}

export interface MenuSettings {
  bgmVol: number;
  seVol: number;
  voiceVol: number;
  cps: number;
  autoWaitMs: number;
  skipUnread: boolean;
  msgOpacity: number;
}

export type SettingKey = keyof MenuSettings;

export interface MenuHost {
  screenW: number;
  screenH: number;
  settings: MenuSettings;
  cpsValues: number[];
  cpsLabels: string[];
  autoValues: number[];
  autoLabels: string[];
  hasTitle: boolean;
  /** スロットへ保存（成功なら true）。サムネイルはメニューを開く直前の画面が使われる。 */
  save(slot: string): boolean;
  load(slot: string): Promise<boolean>;
  setSetting(key: SettingKey, value: number | boolean): void;
  resetSettings(): void;
  goTitle(): void;
  quit(): void;
  toast(text: string): void;
  uiSound(kind: "hover" | "click"): void;
  /** 最後に見ていたページ（メニューを開き直したとき同じページから）。 */
  pages: { save: number; load: number };
}

// ---------- レイアウト（1280x720 の設計座標を画面サイズへ比例拡大、異比率は中央寄せ） ----------

const Z = 311;

const CARD_W = 568;
const CARD_H = 146;
const CARD_GAP_X = 24;
const CARD_GAP_Y = 12;
const THUMB_W = 240;
const THUMB_H = 135;

interface Hot {
  key: string;
  rect: Rect;
  box: UiBox | null;
}

interface Card {
  bg: UiBox;
  thumb: UiImage;
  ph: UiBox;
  del: UiBox;
  rect: Rect;
  slot: string | null;
}

interface SettingRow {
  key: SettingKey | "reset";
  label: string;
  kind: "slider" | "choice" | "button";
  choices: { label: string; value: number | boolean }[];
}

export class SystemMenu {
  private host: MenuHost;
  private events: UiEvents | null = null;
  private frame = new UiGroup();
  private content = new UiGroup();
  private tab: MenuTab = "save";
  private s = 1;
  private ox = 0;
  private oy = 0;
  private hots: Hot[] = [];
  private tabBoxes: UiBox[] = [];
  private hotKey: string | null = null;
  private zone: "content" | "footer" = "content";
  private sel = 0;
  private fsel = 0;
  private footer: string[] = [];
  private metas = new Map<string, SlotMeta>();
  private page = 1;
  private cards: Card[] = [];
  private pageBoxes: UiBox[] = [];
  private rows: SettingRow[] = [];
  private rowBgs: UiBox[] = [];
  private rowLabels: UiBox[] = [];
  private rowValueBoxes: UiBox[] = [];
  private rowFills: UiBox[] = [];
  private rowKnobs: UiBox[] = [];
  private trackRects: Rect[] = [];
  private choiceBoxes: UiBox[][] = [];

  constructor(host: MenuHost) {
    this.host = host;
  }

  /** メニューを開いて、閉じられるまで待つ。呼び出し側が uiLock の取得/解放とサムネイルの事前取得を行う。 */
  async run(initial: MenuTab): Promise<void> {
    this.s = uiScale(this.host.screenW, this.host.screenH);
    this.ox = Math.floor((this.host.screenW - sc(BASE_W, this.s)) / 2);
    this.oy = Math.floor((this.host.screenH - sc(BASE_H, this.s)) / 2);
    this.events = new UiEvents();
    try {
      this.loadMetas();
      this.buildFrame();
      this.enterTab(initial);
      while (true) {
        const ev = await this.events.next();
        const r = await this.handle(ev);
        if (r === "close") break;
      }
    } finally {
      this.content.freeAll();
      this.frame.freeAll();
      if (this.events) this.events.close();
      this.events = null;
    }
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
    for (const s of list) {
      if (s.slot === "auto" || s.slot === "quick" || /^save\d+$/.test(s.slot)) {
        this.metas.set(s.slot, parseSlotMeta(s.slot, s.json));
      }
    }
  }

  private metaOf(slot: string): SlotMeta {
    return this.metas.get(slot) ?? emptySlotMeta(slot);
  }

  private reloadMeta(slot: string): void {
    const json = api.sys.saveRead(slot);
    this.metas.set(slot, parseSlotMeta(slot, json));
  }

  // ---------- 骨組み（背景・タブ・閉じる・フッタ） ----------

  private r(x: number, y: number, w: number, h: number): Rect {
    return { x: this.ox + sc(x, this.s), y: this.oy + sc(y, this.s), w: sc(w, this.s), h: sc(h, this.s) };
  }

  private addHot(key: string, rect: Rect, box: UiBox | null): void {
    this.hots.push({ key, rect, box });
  }

  private buildFrame(): void {
    this.hots = [];
    const sw = this.host.screenW;
    const sh = this.host.screenH;
    const s = this.s;
    this.frame.add(new UiBox({ x: 0, y: 0, w: sw, h: sh }, 310, { bg: UI_COLORS.overlay }));
    const tabs: { tab: MenuTab; label: string }[] = [
      { tab: "save", label: "セーブ" },
      { tab: "load", label: "ロード" },
      { tab: "settings", label: "設定" },
    ];
    this.tabBoxes = [];
    tabs.forEach((t, i) => {
      const rect = this.r(60 + i * 160, 22, 150, 46);
      const box = this.frame.add(
        new UiBox(rect, Z, { radius: sc(10, s), bg: UI_COLORS.tab, fontSize: sc(24, s), align: "center", marginY: sc(9, s) }),
      );
      box.setText([{ text: t.label, color: UI_COLORS.text }]);
      this.tabBoxes.push(box);
      this.addHot(`tab:${t.tab}`, rect, box);
    });
    const closeRect = this.r(1100, 22, 120, 46);
    const closeBox = this.frame.add(
      new UiBox(closeRect, Z, { radius: sc(10, s), bg: UI_COLORS.button, fontSize: sc(22, s), align: "center", marginY: sc(10, s) }),
    );
    closeBox.setText([{ text: "閉じる", color: UI_COLORS.text }]);
    this.addHot("close", closeRect, closeBox);

    this.footer = [];
    if (this.host.hasTitle) this.footer.push("title");
    this.footer.push("quit");
    this.footer.forEach((id, i) => {
      const rect = this.r(60 + i * 216, 650, 204, 44);
      const box = this.frame.add(
        new UiBox(rect, Z, { radius: sc(10, s), bg: UI_COLORS.button, fontSize: sc(21, s), align: "center", marginY: sc(8, s) }),
      );
      box.setText([{ text: id === "title" ? "タイトルへ戻る" : "ゲームを終了", color: UI_COLORS.text }]);
      this.addHot(`foot:${id}`, rect, box);
    });

    const hint = this.frame.add(
      new UiBox(this.r(560, 660, 660, 28), Z, { fontSize: sc(16, s), align: "right", lineHeight: 1.2 }),
    );
    hint.setText([
      { text: "↑↓←→ 選択　Enter 決定　Tab タブ切替　PageUp/Down ページ　Esc 閉じる", color: UI_COLORS.dim },
    ]);
  }

  private colorOfHot(h: Hot): api.RGBA {
    const hovered = this.hotKey === h.key;
    if (h.key.startsWith("tab:")) {
      const t = h.key.slice(4);
      if (t === this.tab) return UI_COLORS.tabActive;
      return hovered ? UI_COLORS.buttonHover : UI_COLORS.tab;
    }
    if (h.key.startsWith("foot:")) {
      const focused = this.zone === "footer" && this.footer[this.fsel] === h.key.slice(5);
      return hovered || focused ? UI_COLORS.buttonHover : UI_COLORS.button;
    }
    if (h.key.startsWith("page:")) {
      const p = parseInt(h.key.slice(5), 10);
      if (p === this.page) return UI_COLORS.tabActive;
      return hovered ? UI_COLORS.buttonHover : UI_COLORS.tab;
    }
    if (h.key.startsWith("del:")) return hovered ? [150, 56, 76, 250] : UI_COLORS.danger;
    if (h.key.startsWith("choice:")) {
      const parts = h.key.split(":");
      const row = this.rows[parseInt(parts[1], 10)];
      const ci = parseInt(parts[2], 10);
      const on = this.choiceIsOn(row, ci);
      if (on) return UI_COLORS.tabActive;
      return hovered ? UI_COLORS.buttonHover : UI_COLORS.button;
    }
    return hovered ? UI_COLORS.buttonHover : UI_COLORS.button;
  }

  private repaintHot(key: string | null): void {
    if (key === null) return;
    for (const h of this.hots) {
      if (h.key === key && h.box !== null) h.box.setBg(this.colorOfHot(h));
    }
  }

  private repaintAllHots(): void {
    for (const h of this.hots) {
      if (h.box !== null) h.box.setBg(this.colorOfHot(h));
    }
  }

  // ---------- タブ切替 ----------

  private enterTab(tab: MenuTab): void {
    this.content.freeAll();
    // 骨組みのホットだけ残して中身のホットを捨てる
    this.hots = this.hots.filter((h) => this.isFrameKey(h.key));
    this.tab = tab;
    this.zone = "content";
    this.sel = 0;
    this.fsel = 0;
    this.cards = [];
    this.pageBoxes = [];
    this.rows = [];
    this.rowBgs = [];
    this.rowLabels = [];
    this.rowValueBoxes = [];
    this.rowFills = [];
    this.rowKnobs = [];
    this.trackRects = [];
    this.choiceBoxes = [];
    if (tab === "settings") {
      this.buildSettings();
    } else {
      this.page = tab === "save" ? this.host.pages.save : this.host.pages.load;
      this.buildSlots();
    }
    this.repaintAllHots();
  }

  private isFrameKey(key: string): boolean {
    return key.startsWith("tab:") || key === "close" || key.startsWith("foot:");
  }

  // ---------- セーブ/ロードの一覧 ----------

  private cardRect(i: number): Rect {
    const col = i % 2;
    const row = Math.floor(i / 2);
    return this.r(60 + col * (CARD_W + CARD_GAP_X), 138 + row * (CARD_H + CARD_GAP_Y), CARD_W, CARD_H);
  }

  private buildSlots(): void {
    // ページ選択の列
    const s = this.s;
    const by = 84;
    const pageRad = sc(8, s);
    const pageFs = sc(20, s);
    const pageMy = sc(6, s);
    const prev = this.content.add(
      new UiBox(this.r(60, by, 44, 36), Z, { radius: pageRad, bg: UI_COLORS.button, fontSize: pageFs, align: "center", marginY: pageMy }),
    );
    prev.setText([{ text: "＜", color: UI_COLORS.text }]);
    this.addHot("prev", prev.rect, prev);
    let px = 112;
    for (let p = 0; p < PAGE_COUNT; p++) {
      const w = p === 0 ? 96 : 64;
      const box = this.content.add(
        new UiBox(this.r(px, by, w, 36), Z, { radius: pageRad, bg: UI_COLORS.tab, fontSize: pageFs, align: "center", marginY: pageMy }),
      );
      box.setText([{ text: p === 0 ? "A / Q" : String(p), color: UI_COLORS.text }]);
      this.pageBoxes.push(box);
      this.addHot(`page:${p}`, box.rect, box);
      px += w + 8;
    }
    const next = this.content.add(
      new UiBox(this.r(px, by, 44, 36), Z, { radius: pageRad, bg: UI_COLORS.button, fontSize: pageFs, align: "center", marginY: pageMy }),
    );
    next.setText([{ text: "＞", color: UI_COLORS.text }]);
    this.addHot("next", next.rect, next);

    // カード 6 枚（ページ 0 は先頭 2 枚だけ使う）
    const thumbW = sc(THUMB_W, s);
    const thumbH = sc(THUMB_H, s);
    const imgW = s === 1 ? 0 : thumbW;
    const imgH = s === 1 ? 0 : thumbH;
    for (let i = 0; i < SLOTS_PER_PAGE; i++) {
      const rect = this.cardRect(i);
      const bg = this.content.add(
        new UiBox(rect, Z, {
          radius: sc(10, s),
          bg: UI_COLORS.panel,
          fontSize: sc(18, s),
          lineHeight: 1.28,
          marginX: thumbW + sc(22, s),
          marginR: sc(12, s),
          marginY: sc(9, s),
        }),
      );
      const thumbRect = { x: rect.x + sc(8, s), y: rect.y + sc(5, s), w: thumbW, h: thumbH };
      const thumb = this.content.add(new UiImage(thumbRect.x, thumbRect.y, Z + 1, imgW, imgH));
      const ph = this.content.add(
        new UiBox(thumbRect, Z + 1, {
          radius: sc(4, s),
          bg: [12, 14, 30, 255],
          fontSize: sc(16, s),
          align: "center",
          marginY: sc(56, s),
          color: UI_COLORS.dim,
        }),
      );
      const delRect = { x: rect.x + rect.w - sc(64, s), y: rect.y + sc(8, s), w: sc(56, s), h: sc(26, s) };
      const del = this.content.add(
        new UiBox(delRect, Z + 2, { radius: sc(6, s), bg: UI_COLORS.danger, fontSize: sc(15, s), lineHeight: 1.2, align: "center", marginY: sc(4, s) }),
      );
      del.setText([{ text: "削除", color: UI_COLORS.text }]);
      this.cards.push({ bg, thumb, ph, del, rect, slot: null });
      this.addHot(`card:${i}`, rect, null);
      this.addHot(`del:${i}`, delRect, del);
    }
    this.refreshCards();
    this.refreshPageButtons();
  }

  private refreshPageButtons(): void {
    this.repaintAllHots();
  }

  private slotsHere(): string[] {
    return slotsOfPage(this.page);
  }

  private refreshCards(): void {
    const slots = this.slotsHere();
    this.cards.forEach((c, i) => {
      const slot = i < slots.length ? slots[i] : null;
      c.slot = slot;
      if (slot === null) {
        c.bg.show(false);
        c.thumb.clear();
        c.ph.show(false);
        c.del.show(false);
        return;
      }
      c.bg.show(true);
      this.paintCard(i);
    });
    if (this.sel >= slots.length) this.sel = Math.max(0, slots.length - 1);
    this.paintSelection();
  }

  private paintCard(i: number): void {
    const c = this.cards[i];
    if (c.slot === null) return;
    const m = this.metaOf(c.slot);
    const segs: api.Segment[] = [];
    segs.push({ text: slotLabel(c.slot) + "\n", color: UI_COLORS.accent });
    if (m.used) {
      segs.push({ text: (m.savedAt !== "" ? m.savedAt : "（日時なし）") + "\n", color: UI_COLORS.dim });
      segs.push({ text: (m.chapter !== "" ? m.chapter : "（無題）") + "\n", color: UI_COLORS.text });
      if (m.excerpt !== "") segs.push({ text: m.excerpt, color: UI_COLORS.dim });
    } else {
      segs.push({
        text: c.slot === "auto" ? "（選択肢で自動保存されます）" : "（空きスロット）",
        color: UI_COLORS.dim,
      });
    }
    c.bg.setText(segs);
    const hasThumb = m.used && m.thumb && c.thumb.loadSaveThumb(c.slot);
    if (!hasThumb) c.thumb.clear();
    c.ph.show(!hasThumb);
    if (!hasThumb) c.ph.setText([{ text: m.used ? "NO IMAGE" : "NO DATA", color: UI_COLORS.dim }]);
    c.del.show(m.used);
  }

  private paintSelection(): void {
    this.cards.forEach((c, i) => {
      if (c.slot === null) return;
      const selected = this.zone === "content" && this.sel === i;
      const m = this.metaOf(c.slot);
      c.bg.setBg(selected ? UI_COLORS.panelHover : m.used ? UI_COLORS.panel : UI_COLORS.panelEmpty);
    });
  }

  private setPage(p: number): void {
    const next = Math.max(0, Math.min(PAGE_COUNT - 1, p));
    if (next === this.page) return;
    this.page = next;
    if (this.tab === "save") this.host.pages.save = next;
    else this.host.pages.load = next;
    this.refreshCards();
    this.repaintAllHots();
  }

  private async activateCard(i: number): Promise<"close" | undefined> {
    const slot = this.cards[i].slot;
    if (slot === null) return undefined;
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
        this.paintSelection();
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
    const slot = this.cards[i].slot;
    if (slot === null || !this.metaOf(slot).used) return;
    const ok = await this.ask(`${slotLabel(slot)} を削除しますか？`);
    if (!ok) return;
    api.sys.saveDelete(slot);
    this.metas.set(slot, emptySlotMeta(slot));
    this.paintCard(i);
    this.paintSelection();
  }

  private async ask(text: string): Promise<boolean> {
    const r = await confirmDialog(this.host.screenW, this.host.screenH, text, this.events!);
    this.events!.clear();
    return r;
  }

  // ---------- 設定 ----------

  private choiceIsOn(row: SettingRow, ci: number): boolean {
    if (row.kind !== "choice" || row.key === "reset") return false;
    const cur = this.host.settings[row.key];
    return row.choices[ci].value === cur;
  }

  private buildSettings(): void {
    const h = this.host;
    const stage = (values: number[], labels: string[]): { label: string; value: number | boolean }[] =>
      values.map((v, i) => ({ label: labels[i], value: v }));
    this.rows = [
      { key: "bgmVol", label: "BGM 音量", kind: "slider", choices: [] },
      { key: "seVol", label: "SE 音量", kind: "slider", choices: [] },
      { key: "voiceVol", label: "ボイス音量", kind: "slider", choices: [] },
      { key: "cps", label: "文字速度", kind: "choice", choices: stage(h.cpsValues, h.cpsLabels) },
      { key: "autoWaitMs", label: "オート速度", kind: "choice", choices: stage(h.autoValues, h.autoLabels) },
      {
        key: "skipUnread",
        label: "未読もスキップ",
        kind: "choice",
        choices: [
          { label: "しない", value: false },
          { label: "する", value: true },
        ],
      },
      { key: "msgOpacity", label: "メッセージ窓の濃さ", kind: "slider", choices: [] },
      { key: "reset", label: "初期設定に戻す", kind: "button", choices: [] },
    ];
    const s = this.s;
    const rowRad = sc(10, s);
    const btnRad = sc(8, s);
    const lblFs = sc(22, s);
    const btnFs = sc(20, s);
    const lblMy = sc(12, s);
    const btnMy = sc(8, s);
    const trackRad = sc(6, s);
    const knobW = sc(12, s);
    const knobH = sc(30, s);
    const knobOffY = sc(7, s);
    this.rows.forEach((row, i) => {
      const y = 118 + i * 62;
      const bg = this.content.add(
        new UiBox(this.r(60, y, 1160, 52), Z, { radius: rowRad, bg: [26, 32, 62, 150] }),
      );
      this.rowBgs.push(bg);
      this.addHot(`row:${i}`, this.r(60, y, 1160, 52), null);
      const label = this.content.add(
        new UiBox(this.r(84, y, 300, 52), Z + 1, { fontSize: lblFs, marginY: lblMy }),
      );
      label.setText([{ text: row.label, color: UI_COLORS.text }]);
      this.rowLabels.push(label);
      this.choiceBoxes.push([]);
      this.trackRects.push({ x: 0, y: 0, w: 0, h: 0 });
      if (row.kind === "slider") {
        const dec = this.content.add(
          new UiBox(this.r(400, y + 6, 40, 40), Z + 1, { radius: btnRad, bg: UI_COLORS.button, fontSize: btnFs, align: "center", marginY: btnMy }),
        );
        dec.setText([{ text: "＜", color: UI_COLORS.text }]);
        this.addHot(`arrow:${i}:-1`, dec.rect, dec);
        const track = this.r(462, y + 18, 420, 16);
        this.trackRects[i] = track;
        this.content.add(new UiBox(track, Z + 1, { radius: trackRad, bg: UI_COLORS.track }));
        const fill = this.content.add(new UiBox({ x: track.x, y: track.y, w: track.w, h: track.h }, Z + 2, { radius: trackRad, bg: UI_COLORS.fill }));
        this.rowFills.push(fill);
        const knob = this.content.add(
          new UiBox({ x: track.x, y: track.y - knobOffY, w: knobW, h: knobH }, Z + 3, { radius: sc(5, s), bg: UI_COLORS.accent }),
        );
        this.rowKnobs.push(knob);
        const inc = this.content.add(
          new UiBox(this.r(902, y + 6, 40, 40), Z + 1, { radius: btnRad, bg: UI_COLORS.button, fontSize: btnFs, align: "center", marginY: btnMy }),
        );
        inc.setText([{ text: "＞", color: UI_COLORS.text }]);
        this.addHot(`arrow:${i}:1`, inc.rect, inc);
        const val = this.content.add(new UiBox(this.r(960, y, 120, 52), Z + 1, { fontSize: lblFs, marginY: lblMy }));
        this.rowValueBoxes.push(val);
        this.addHot(`slider:${i}`, { x: track.x - sc(8, s), y: track.y - sc(12, s), w: track.w + sc(16, s), h: track.h + sc(24, s) }, null);
      } else {
        this.rowFills.push(null as unknown as UiBox);
        this.rowKnobs.push(null as unknown as UiBox);
        this.rowValueBoxes.push(null as unknown as UiBox);
        if (row.kind === "choice") {
          const n = row.choices.length;
          const bw = n <= 3 ? 130 : 108;
          row.choices.forEach((c, ci) => {
            const rect = this.r(400 + ci * (bw + 10), y + 6, bw, 40);
            const box = this.content.add(
              new UiBox(rect, Z + 1, { radius: btnRad, bg: UI_COLORS.button, fontSize: btnFs, align: "center", marginY: btnMy }),
            );
            box.setText([{ text: c.label, color: UI_COLORS.text }]);
            this.choiceBoxes[i].push(box);
            this.addHot(`choice:${i}:${ci}`, rect, box);
          });
        } else {
          const rect = this.r(400, y + 6, 240, 40);
          const box = this.content.add(
            new UiBox(rect, Z + 1, { radius: btnRad, bg: UI_COLORS.danger, fontSize: btnFs, align: "center", marginY: btnMy }),
          );
          box.setText([{ text: "初期設定に戻す", color: UI_COLORS.text }]);
          this.addHot("reset", rect, box);
        }
      }
    });
    this.refreshSettings();
  }

  private sliderValue(row: SettingRow): number {
    if (row.key === "reset") return 0;
    return this.host.settings[row.key] as number;
  }

  private refreshSettings(): void {
    const s = this.s;
    const knobW = sc(12, s);
    const knobHalf = Math.floor(knobW / 2);
    const knobOffY = sc(7, s);
    this.rows.forEach((row, i) => {
      if (row.kind === "slider") {
        const v = this.sliderValue(row);
        const track = this.trackRects[i];
        const w = Math.round((track.w * v) / 100);
        const fill = this.rowFills[i];
        if (w > 0) {
          fill.show(true);
          fill.rect.w = w;
          fill.setBg(UI_COLORS.fill);
        } else {
          fill.show(false);
        }
        this.rowKnobs[i].setPos(track.x + Math.max(0, Math.min(track.w - knobW, w - knobHalf)), track.y - knobOffY);
        this.rowValueBoxes[i].setText([{ text: `${v}%`, color: UI_COLORS.accent }]);
      }
      const selected = this.zone === "content" && this.sel === i;
      this.rowBgs[i].setBg(selected ? UI_COLORS.panelHover : [26, 32, 62, 150]);
    });
    this.repaintAllHots();
  }

  /** 設定行の値を delta 段階動かす（スライダーは 10%、選択肢は 1 つ）。端で止まる。 */
  private nudge(i: number, delta: number): void {
    const row = this.rows[i];
    if (row.key === "reset") return;
    if (row.kind === "slider") {
      const cur = this.sliderValue(row);
      const next = Math.max(0, Math.min(100, cur + delta * 10));
      if (next !== cur) this.applySetting(row, next);
    } else if (row.kind === "choice") {
      let idx = 0;
      row.choices.forEach((c, ci) => {
        if (c.value === this.host.settings[row.key as SettingKey]) idx = ci;
      });
      const ni = Math.max(0, Math.min(row.choices.length - 1, idx + delta));
      if (ni !== idx) this.applySetting(row, row.choices[ni].value);
    }
  }

  private applySetting(row: SettingRow, value: number | boolean): void {
    if (row.key === "reset") return;
    this.host.setSetting(row.key, value);
    this.refreshSettings();
  }

  private async resetSettingsFlow(): Promise<void> {
    const ok = await this.ask("設定を初期状態に戻しますか？");
    if (!ok) return;
    this.host.resetSettings();
    this.refreshSettings();
    this.host.toast("初期設定に戻しました");
  }

  // ---------- フッタ（タイトルへ戻る/終了） ----------

  private async activateFooter(id: string): Promise<"close" | undefined> {
    if (id === "title") {
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
    // 後から追加したもの（スライダーの当たり等）を優先
    for (let i = this.hots.length - 1; i >= 0; i--) {
      if (inRect(this.hots[i].rect, x, y)) return this.hots[i];
    }
    return null;
  }

  private async handle(ev: api.InputEvent): Promise<"close" | undefined> {
    if (ev.kind === "move") {
      this.onMove(ev.x, ev.y);
      return undefined;
    }
    if (ev.kind === "wheel") {
      if (this.tab !== "settings") this.setPage(this.page + ((ev.delta ?? 0) > 0 ? -1 : 1));
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
    if (key !== this.hotKey) {
      const prev = this.hotKey;
      this.hotKey = key;
      this.repaintHot(prev);
      this.repaintHot(key);
    }
    if (h === null) return;
    // カード/設定行/フッタにカーソルが乗ったらキーボードのフォーカスも追従させる
    if (h.key.startsWith("card:")) {
      const i = parseInt(h.key.slice(5), 10);
      if (this.cards[i] && this.cards[i].slot !== null && (this.zone !== "content" || this.sel !== i)) {
        this.zone = "content";
        this.sel = i;
        this.host.uiSound("hover");
        this.paintSelection();
        this.repaintAllHots();
      }
    } else if (h.key.startsWith("row:") || h.key.startsWith("slider:") || h.key.startsWith("choice:") || h.key.startsWith("arrow:")) {
      const i = parseInt(h.key.split(":")[1], 10);
      if (this.zone !== "content" || this.sel !== i) {
        this.zone = "content";
        this.sel = i;
        this.host.uiSound("hover");
        this.refreshSettings();
      }
    } else if (h.key.startsWith("foot:")) {
      const idx = this.footer.indexOf(h.key.slice(5));
      if (idx >= 0 && (this.zone !== "footer" || this.fsel !== idx)) {
        this.zone = "footer";
        this.fsel = idx;
        this.host.uiSound("hover");
        this.refreshFocus();
      }
    }
  }

  private refreshFocus(): void {
    if (this.tab === "settings") this.refreshSettings();
    else this.paintSelection();
    this.repaintAllHots();
  }

  private async onClick(x: number, y: number): Promise<"close" | undefined> {
    const h = this.hit(x, y);
    if (h === null) return undefined;
    const k = h.key;
    this.host.uiSound("click");
    if (k === "close") return "close";
    if (k.startsWith("tab:")) {
      const t = k.slice(4) as MenuTab;
      if (t !== this.tab) this.enterTab(t);
      return undefined;
    }
    if (k.startsWith("foot:")) return await this.activateFooter(k.slice(5));
    if (k === "prev") {
      this.setPage(this.page - 1);
      return undefined;
    }
    if (k === "next") {
      this.setPage(this.page + 1);
      return undefined;
    }
    if (k.startsWith("page:")) {
      this.setPage(parseInt(k.slice(5), 10));
      return undefined;
    }
    if (k.startsWith("del:")) {
      await this.deleteCard(parseInt(k.slice(4), 10));
      return undefined;
    }
    if (k.startsWith("card:")) {
      const i = parseInt(k.slice(5), 10);
      this.sel = i;
      this.zone = "content";
      return await this.activateCard(i);
    }
    if (k.startsWith("slider:")) {
      const i = parseInt(k.slice(7), 10);
      this.sel = i;
      this.zone = "content";
      this.applySetting(this.rows[i], sliderFromX(x, this.trackRects[i]));
      return undefined;
    }
    if (k.startsWith("arrow:")) {
      const parts = k.split(":");
      this.sel = parseInt(parts[1], 10);
      this.zone = "content";
      this.nudge(this.sel, parseInt(parts[2], 10));
      return undefined;
    }
    if (k.startsWith("choice:")) {
      const parts = k.split(":");
      const i = parseInt(parts[1], 10);
      this.sel = i;
      this.zone = "content";
      this.applySetting(this.rows[i], this.rows[i].choices[parseInt(parts[2], 10)].value);
      return undefined;
    }
    if (k === "reset") {
      await this.resetSettingsFlow();
      return undefined;
    }
    return undefined;
  }

  private async onKey(k: string): Promise<"close" | undefined> {
    if (k === "Escape") return "close";
    if (k === "Tab") {
      const order: MenuTab[] = ["save", "load", "settings"];
      this.enterTab(order[(order.indexOf(this.tab) + 1) % order.length]);
      return undefined;
    }
    if (this.zone === "footer") return await this.onKeyFooter(k);
    return this.tab === "settings" ? await this.onKeySettings(k) : await this.onKeySlots(k);
  }

  private async onKeyFooter(k: string): Promise<"close" | undefined> {
    if (k === "ArrowLeft" && this.fsel > 0) this.fsel--;
    else if (k === "ArrowRight" && this.fsel < this.footer.length - 1) this.fsel++;
    else if (k === "ArrowUp") {
      this.zone = "content";
      this.sel = this.tab === "settings" ? this.rows.length - 1 : Math.max(0, this.slotsHere().length - 1);
    } else if (k === "Enter" || k === " ") {
      return await this.activateFooter(this.footer[this.fsel]);
    }
    this.refreshFocus();
    return undefined;
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
      this.setPage(this.page - 1);
      return undefined;
    } else if (k === "PageDown") {
      this.setPage(this.page + 1);
      return undefined;
    } else if (k === "Enter" || k === " ") {
      return await this.activateCard(this.sel);
    } else if (k === "Delete") {
      await this.deleteCard(this.sel);
      return undefined;
    } else return undefined;
    const next = moveGrid(this.sel, count, 2, dx, dy);
    if (next !== null) {
      this.sel = next;
      this.host.uiSound("hover");
    } else if (dy > 0) {
      this.zone = "footer";
      this.fsel = Math.min(this.fsel, this.footer.length - 1);
    } else if (dx !== 0) {
      // 左右の端では隣のページへ（行は維持）
      const row = Math.floor(this.sel / 2);
      const np = this.page + dx;
      if (np >= 0 && np < PAGE_COUNT) {
        this.setPage(np);
        const n = this.slotsHere().length;
        this.sel = Math.min(n - 1, row * 2 + (dx > 0 ? 0 : 1));
        if (this.sel < 0) this.sel = 0;
      }
    }
    this.refreshFocus();
    return undefined;
  }

  private async onKeySettings(k: string): Promise<"close" | undefined> {
    if (k === "ArrowUp") {
      if (this.sel > 0) this.sel--;
    } else if (k === "ArrowDown") {
      if (this.sel < this.rows.length - 1) this.sel++;
      else {
        this.zone = "footer";
        this.fsel = 0;
      }
    } else if (k === "ArrowLeft") {
      this.nudge(this.sel, -1);
    } else if (k === "ArrowRight") {
      this.nudge(this.sel, 1);
    } else if (k === "Enter" || k === " ") {
      const row = this.rows[this.sel];
      if (row.kind === "button") {
        await this.resetSettingsFlow();
        return undefined;
      }
      if (row.kind === "choice") {
        // 選択肢は Enter で次へ（端なら先頭へ戻る）
        let idx = 0;
        row.choices.forEach((c, ci) => {
          if (c.value === this.host.settings[row.key as SettingKey]) idx = ci;
        });
        this.applySetting(row, row.choices[(idx + 1) % row.choices.length].value);
        return undefined;
      }
    } else return undefined;
    this.refreshSettings();
    return undefined;
  }
}
