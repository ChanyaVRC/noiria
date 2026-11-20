// noiria:kag/buttons — KAG3 の [button]/[checkbox]/[edit] 自作 UI 部品（W4）。
// tags.ts の肥大化を避けるための独立モジュール。import は player.ts → buttons.ts の
// 一方向のみ（buttons.ts は player.ts を import しない。docs/implementation-notes.md
// 「2026-07-11 W4」参照。W1 で発覚した player.ts⇄tags.ts の循環 import 事故と同じ罠を
// 避けるため）。KagPlayer 型には依存せず、`api`/`vars` だけを使う自己完結モジュール。

import * as api from "noiria:api";
import * as vars from "noiria:kag/vars";

export type ButtonVisualState = "normal" | "over" | "pressed";

interface BaseWidget {
  layerId: number;
  x: number;
  y: number;
  w: number;
  h: number;
  /** [locklink] で不活性化されたウィジェット個別のロック（全体の active とは別軸）。 */
  locked: boolean;
}

export interface ButtonWidget extends BaseWidget {
  kind: "button";
  /** 解決済みの画像パス（正常/押下/ホバーが横に 3 分割された 1 枚絵）。 */
  graphic: string;
  /** 1 状態ぶんの幅 (= 画像全体の幅 / 3)。 */
  stateW: number;
  target: string | null;
  storage: string | null;
  exp: string | null;
  state: ButtonVisualState;
}

export interface CheckboxWidget extends BaseWidget {
  kind: "checkbox";
  /** バインド先変数への式（例: "f.flag"）。読み書きは vars.evalExpr/evalStmt 経由。 */
  varExpr: string;
}

export interface EditWidget extends BaseWidget {
  kind: "edit";
  varExpr: string;
  maxLen: number;
  buffer: string;
}

export type Widget = ButtonWidget | CheckboxWidget | EditWidget;

/** クリックがボタンにヒットしたときに呼び出し側 (player.ts) へ返すアクション。
 * checkbox/edit はこのモジュール内で完結するため null（ヒットなし）と区別しない。 */
export interface ClickAction {
  kind: "button";
  target: string | null;
  storage: string | null;
  exp: string | null;
}

/** 3 状態画像の中から state に対応するスライスの clip 矩形を計算する（純関数、単体テスト対象）。 */
export function stateClip(
  stateW: number,
  h: number,
  state: ButtonVisualState,
): { x: number; y: number; w: number; h: number } {
  const idx = state === "normal" ? 0 : state === "over" ? 1 : 2;
  return { x: idx * stateW, y: 0, w: stateW, h };
}

/** (px, py) が矩形 (x, y, w, h) 内かどうか（純関数、ヒットテスト共通ロジック）。 */
export function hitRect(px: number, py: number, x: number, y: number, w: number, h: number): boolean {
  return px >= x && px < x + w && py >= y && py < y + h;
}

/**
 * [edit]/[input] の 1 キー入力を現在のバッファへ適用する（純関数、単体テスト対象）。
 * - 表示可能な単一コードポイントの key（`ev.key.length === 1`）はそのまま追記
 *   （maxLen 到達で無視）。
 * - `"Backspace"` は末尾 1 文字を削除。
 * - それ以外（Enter/Escape/矢印等の名前付きキー、複数文字の key）は無視して現在値を返す。
 * IME 非対応: ネイティブ側 (win32.rs) は WM_CHAR を確定文字ごとに 1 イベントとして
 * 送るのみで、変換候補選択中の未確定文字列は扱わない（docs/kag-tag-compat.md 備考参照）。
 */
export function applyEditKey(buffer: string, key: string, maxLen: number): string {
  if (key === "Backspace") return buffer.length > 0 ? buffer.slice(0, -1) : buffer;
  if (key.length === 1 && buffer.length < maxLen) return buffer + key;
  return buffer;
}

const Z_WIDGET = 400;

/** button/checkbox/edit の一元管理: 登録・描画・ヒットテスト・状態遷移。
 * player.ts が [s]（waitForJump）の開始/終了に合わせて `active` を切り替える
 * （KAG 準拠: 停止中のみ活性）。 */
export class ButtonManager {
  private widgets: Widget[] = [];
  /** [s] 停止中のみ true（KAG 準拠）。false の間はホバー/クリックとも無反応。 */
  active = false;
  /** [locklink]: 全体ロック。true の間はホバー/クリックとも無反応（active とは独立）。 */
  globalLocked = false;
  private editing: EditWidget | null = null;

  private hitEnabled(): boolean {
    return this.active && !this.globalLocked;
  }

  // ---------- 登録 ----------

  /** [button graphic= target= storage= exp= x= y=]。graphic は正常/押下/ホバーが
   * 横に 3 等分された 1 枚絵（解決済みパス）。画像の実寸は一度読み込んで確認し
   * （`api.layer.loadImage` の戻り値）、1 状態ぶんのサイズへキャンバスを縮めてから
   * 通常状態のスライスを blit する（新規の「画像サイズ取得」op を増やさず、既存の
   * loadImage の戻り値を流用する設計判断。docs/implementation-notes.md 参照）。 */
  addButton(
    x: number,
    y: number,
    graphic: string,
    target: string | null,
    storage: string | null,
    exp: string | null,
  ): ButtonWidget {
    const layerId = api.layer.create("image");
    const [totalW, totalH] = api.layer.loadImage(layerId, graphic, "fore");
    const stateW = Math.max(1, Math.floor(totalW / 3));
    const widget: ButtonWidget = {
      kind: "button",
      layerId,
      x,
      y,
      w: stateW,
      h: totalH,
      locked: false,
      graphic,
      stateW,
      target,
      storage,
      exp,
      state: "normal",
    };
    this.widgets.push(widget);
    api.layer.set(layerId, { x, y, z: Z_WIDGET, visible: true }, "fore");
    this.redrawButton(widget);
    return widget;
  }

  private redrawButton(w: ButtonWidget): void {
    api.layer.fill(w.layerId, w.stateW, w.h, [0, 0, 0, 0], "fore");
    api.layer.blitImage(w.layerId, w.graphic, "fore", 0, 0, stateClip(w.stateW, w.h, w.state));
  }

  /** [checkbox name= x= y=]。name は "f.flag" のような変数式。見た目は
   * 自作の角丸矩形 + チェックマーク（text レイヤで msgbox 背景 + "✓" 印字）。 */
  addCheckbox(x: number, y: number, varExpr: string, size = 28): CheckboxWidget {
    const layerId = api.layer.create("text");
    const widget: CheckboxWidget = {
      kind: "checkbox",
      layerId,
      x,
      y,
      w: size,
      h: size,
      locked: false,
      varExpr,
    };
    this.widgets.push(widget);
    api.layer.set(layerId, { x, y, z: Z_WIDGET, visible: true }, "fore");
    this.redrawCheckbox(widget);
    return widget;
  }

  private checkboxChecked(w: CheckboxWidget): boolean {
    try {
      return !!vars.evalExpr(w.varExpr, null);
    } catch {
      return false;
    }
  }

  private redrawCheckbox(w: CheckboxWidget): void {
    const checked = this.checkboxChecked(w);
    const boxColor: api.RGBA = checked ? [90, 200, 140, 235] : [40, 42, 58, 210];
    api.layer.msgbox(w.layerId, w.w, w.h, 4, boxColor, "fore");
    api.text.style(w.layerId, {
      fontSize: Math.floor(w.h * 0.75),
      boxW: w.w,
      boxH: w.h,
      marginX: 0,
      marginY: Math.floor(w.h * 0.08),
      color: [255, 255, 255, 255],
    });
    api.text.print(w.layerId, [{ text: checked ? "✓" : "", color: null }], false, 0);
  }

  /** [edit name= length= x= y=]。メッセージレイヤ内の入力欄相当を、独立した
   * 小さな text レイヤとして表示する（[commit]/Enter で確定するまで自作の
   * オーバーレイとして浮かせる簡易実装。docs/implementation-notes.md 参照）。
   * 既に編集中のフィールドがあれば先に確定してから、このフィールドへ移る。 */
  addEdit(x: number, y: number, varExpr: string, maxLen = 30, w = 260, h = 36): EditWidget {
    if (this.editing) this.commitEdit();
    const layerId = api.layer.create("text");
    const initial = String(this.readVar(varExpr) ?? "");
    const widget: EditWidget = {
      kind: "edit",
      layerId,
      x,
      y,
      w,
      h,
      locked: false,
      varExpr,
      maxLen: Math.max(1, Math.floor(maxLen)),
      buffer: initial.slice(0, maxLen),
    };
    this.widgets.push(widget);
    api.layer.set(layerId, { x, y, z: Z_WIDGET, visible: true }, "fore");
    this.editing = widget;
    this.redrawEdit(widget);
    return widget;
  }

  private readVar(varExpr: string): unknown {
    try {
      return vars.evalExpr(varExpr, null);
    } catch {
      return undefined;
    }
  }

  private redrawEdit(w: EditWidget): void {
    const focused = this.editing === w;
    api.layer.msgbox(w.layerId, w.w, w.h, 6, focused ? [30, 34, 52, 235] : [20, 22, 34, 200], "fore");
    api.text.style(w.layerId, {
      fontSize: Math.floor(w.h * 0.55),
      boxW: w.w - 16,
      boxH: w.h - 10,
      marginX: 8,
      marginY: 5,
      color: [255, 255, 255, 255],
    });
    const shown = focused ? w.buffer + "_" : w.buffer;
    api.text.print(w.layerId, [{ text: shown, color: null }], false, 0);
  }

  /** [commit]: 編集中のフィールドがあれば内容を変数へ確定し、フォーカスを外す。 */
  commitEdit(): void {
    const w = this.editing;
    if (!w) return;
    vars.evalStmt(`${w.varExpr} = ${JSON.stringify(w.buffer)}`, null);
    this.editing = null;
    this.redrawEdit(w);
  }

  /** フォーカス中の編集を破棄する（変数へは書き込まない）。 */
  cancelEdit(): void {
    if (!this.editing) return;
    const w = this.editing;
    this.editing = null;
    this.redrawEdit(w);
  }

  // ---------- 入力 ----------

  /** マウス移動: ボタンのホバー状態を更新する（active/globalLocked/個別 locked を尊重）。 */
  hover(x: number, y: number): void {
    if (!this.hitEnabled()) return;
    for (const w of this.widgets) {
      if (w.kind !== "button" || w.locked) continue;
      const inside = hitRect(x, y, w.x, w.y, w.w, w.h);
      const next: ButtonVisualState = inside ? (w.state === "pressed" ? "pressed" : "over") : "normal";
      if (next !== w.state) {
        w.state = next;
        this.redrawButton(w);
      }
    }
  }

  /** クリック処理。button にヒットした場合のみ ClickAction を返す（ジャンプ/式評価の
   * 実行は player.ts 側の責務: requestJump/vars.evalStmt は KagPlayer の pc/sys に
   * 依存するため、このモジュールからは行わない）。checkbox/edit はここで完結する。 */
  click(x: number, y: number): ClickAction | null {
    if (!this.hitEnabled()) return null;
    for (const w of this.widgets) {
      if (w.locked || !hitRect(x, y, w.x, w.y, w.w, w.h)) continue;
      if (w.kind === "button") {
        w.state = "pressed";
        this.redrawButton(w);
        return { kind: "button", target: w.target, storage: w.storage, exp: w.exp };
      }
      if (w.kind === "checkbox") {
        const next = !this.checkboxChecked(w);
        vars.evalStmt(`${w.varExpr} = ${next}`, null);
        this.redrawCheckbox(w);
        return null;
      }
      if (w.kind === "edit") {
        if (this.editing && this.editing !== w) this.commitEdit();
        this.editing = w;
        this.redrawEdit(w);
        return null;
      }
    }
    return null;
  }

  /** キー入力。フォーカス中の edit フィールドがあれば消費して true を返す。 */
  handleKey(key: string): boolean {
    if (!this.hitEnabled() || !this.editing) return false;
    if (key === "Enter") {
      this.commitEdit();
      return true;
    }
    if (key === "Escape") {
      this.cancelEdit();
      return true;
    }
    const w = this.editing;
    const next = applyEditKey(w.buffer, key, w.maxLen);
    if (next !== w.buffer) {
      w.buffer = next;
      this.redrawEdit(w);
    }
    return true;
  }

  // ---------- ロック (locklink/unlocklink) ----------

  setGlobalLocked(locked: boolean): void {
    this.globalLocked = locked;
  }

  // ---------- 後始末 ----------

  /** 全ウィジェットを解放する（[cm] 等、メッセージ画面を大きくクリアするタイミングで
   * 呼ぶことを想定。個々のシナリオ側の都合に委ねるため tags.ts からのみ呼ばれる）。 */
  clear(): void {
    if (this.editing) this.editing = null;
    for (const w of this.widgets) api.layer.free(w.layerId);
    this.widgets = [];
  }
}
