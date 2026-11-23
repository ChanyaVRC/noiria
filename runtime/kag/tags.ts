// noiria:kag/tags — KAG3 互換タグの実装テーブル。
// それぞれのハンドラは KagPlayer のポリシーメソッドを呼ぶだけの薄い層。

import * as api from "noiria:api";
import * as vars from "noiria:kag/vars";
import { parseAsd } from "noiria:kag/anim";
import type { AsdDef } from "noiria:kag/anim";
import type { Attrs } from "noiria:kag/parser";
import type { KagPlayer, FontState } from "noiria:kag/player";

export type TagHandler = (attrs: Attrs) => void | Promise<void>;

function num(v: string | undefined, dflt: number): number {
  if (v === undefined) return dflt;
  const n = Number(v);
  return Number.isFinite(n) ? n : dflt;
}

function bool(v: string | undefined, dflt: boolean): boolean {
  if (v === undefined) return dflt;
  return v === "true" || v === "1" || v === "yes";
}

function videoSlotIsZero(a: Attrs, tagName: string): boolean {
  const slot = Number(a.slot ?? "0");
  if (Number.isFinite(slot) && slot === 0) return true;
  api.sys.log(`[${tagName}] slot=${a.slot ?? ""} は未対応です（slot=0 のみ）`, "warn");
  return false;
}

function videoFrameAttr(a: Attrs, name: string, tagName: string): number | null {
  const raw = a[name];
  const value = raw === undefined ? NaN : Number(raw);
  if (
    !Number.isFinite(value) ||
    value < 0 ||
    value > 4294967295 ||
    Math.floor(value) !== value
  ) {
    api.sys.log(`[${tagName}] ${name}=${raw ?? ""} は非負の整数フレーム番号ではありません`, "warn");
    return null;
  }
  return value;
}

function videoWaitKind(value: string | undefined): api.VideoEventKind | "any" | null {
  if (value === undefined || value === "") return "any";
  if (value === "loop" || value === "period" || value === "prepare" || value === "segLoop") {
    return value;
  }
  return null;
}

/** [font]/[style]/[deffont]/[defstyle] 共通: 指定された属性だけを target へ部分適用する
 * （未指定の属性は現在値を維持する。KAG の font/style タグの慣習に合わせる）。
 * hex カラーのパースは `p.hexColor`（player.ts のインスタンスメソッド）経由で行う。
 * `noiria:kag/player` を値として import すると player.ts → tags.ts → player.ts の
 * 循環 import になり tsint 実行時に `TypeError` になるため（実機で確認済み、
 * `docs/implementation-notes.md` 参照）、値の import はせず `p` 経由でアクセスする。 */
function applyFontAttrs(
  p: KagPlayer,
  target: FontState,
  a: Attrs,
): void {
  // face="user"（KAG3: ユーザー設定フォント）と空指定は「既定のフォントへ戻す」として扱う。
  if (a.face !== undefined) target.face = a.face === "" || a.face === "user" ? undefined : a.face;
  if (a.size !== undefined) target.size = num(a.size, target.size ?? 0);
  if (a.bold !== undefined) target.bold = bool(a.bold, true);
  if (a.color !== undefined) target.color = p.hexColor(a.color, target.color);
  if (a.edgecolor !== undefined) target.edgeColor = p.hexColor(a.edgecolor, target.edgeColor);
}

function applyLineStyleAttrs(
  p: KagPlayer,
  target: { align: api.TextAlign; lineSpacing: number },
  a: Attrs,
  tagName: string,
): boolean {
  let changed = false;
  if (a.align !== undefined) {
    if (a.align === "left" || a.align === "center" || a.align === "right") {
      target.align = a.align;
      changed = true;
    } else {
      api.sys.log(`[${tagName}] align=${a.align} は不正なため無視します`, "warn");
    }
  }
  if (a.linespacing !== undefined) {
    const lineSpacing = Number(a.linespacing);
    if (!Number.isFinite(lineSpacing)) {
      api.sys.log(`[${tagName}] linespacing=${a.linespacing} は有限値でないため無視します`, "warn");
    } else if (p.messageLineHeight(p.current.index, lineSpacing) !== null) {
      target.lineSpacing = lineSpacing;
      changed = true;
    } else {
      api.sys.log(
        `[${tagName}] linespacing=${a.linespacing} は単精度の有限範囲外のため無視します`,
        "warn",
      );
    }
  }
  return changed;
}

/**
 * [move path=]（W5）: KAG の `path=(x1,y1,o1)(x2,y2,o2)...` を区間ごとの
 * `{x?,y?,opacity?}` 列へパースする。各点の opacity は省略可（3 値目が無ければ
 * undefined のまま、呼び出し側で直前の値を引き継ぐ）。パース不能な括弧やカンマの数が
 * 合わない点は無視する（そのシナリオ作者の
 * typo を握りつぶさないよう、呼び出し側は結果が空配列なら warn する）。純関数（単体テスト対象）。
 */
export function parseMovePath(path: string): { x?: number; y?: number; opacity?: number }[] {
  const out: { x?: number; y?: number; opacity?: number }[] = [];
  const re = /\(([^)]*)\)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(path)) !== null) {
    const parts = m[1].split(",").map((s) => s.trim());
    if (parts.length < 2) continue;
    const x = Number(parts[0]);
    const y = Number(parts[1]);
    if (!Number.isFinite(x) || !Number.isFinite(y)) continue;
    const point: { x?: number; y?: number; opacity?: number } = { x, y };
    if (parts.length >= 3 && parts[2].length > 0) {
      const o = Number(parts[2]);
      if (Number.isFinite(o)) point.opacity = o;
    }
    out.push(point);
  }
  return out;
}

/** [image]/[pimage]（W5）共通: `clip="x,y,w,h"` をパースする。形式不正なら null。 */
function parseClip(v: string | undefined): { x: number; y: number; w: number; h: number } | null {
  if (!v) return null;
  const parts = v.split(",").map((s) => Number(s.trim()));
  if (parts.length !== 4 || parts.some((n) => !Number.isFinite(n))) return null;
  return { x: parts[0], y: parts[1], w: parts[2], h: parts[3] };
}

/** [pimage]（W7）: `clip=` に加え、KAG3 標準の `sx= sy= sw= sh=`（コピー元矩形）を
 * `clip=` の別名として受理する（`clip=` が優先、両方の指定は想定しない）。純関数。 */
export function resolvePimageClip(a: Attrs): { x: number; y: number; w: number; h: number } | null {
  const clip = parseClip(a.clip);
  if (clip) return clip;
  if (a.sx === undefined && a.sy === undefined && a.sw === undefined && a.sh === undefined) return null;
  const w = num(a.sw, 0);
  const h = num(a.sh, 0);
  if (w <= 0 || h <= 0) return null;
  return { x: num(a.sx, 0), y: num(a.sy, 0), w, h };
}

export function makeTags(p: KagPlayer): Record<string, TagHandler> {
  const kindOfLayer = (layerName: string): "bg" | "fg" =>
    layerName === "base" ? "bg" : "fg";

  /** SE 系タグの buf= 属性（既定 0）→ チャンネル名 "se0".."seN"。
   * Rust 側で "se" は "se0" の別名なので、buf 省略時も従来の [playse] と同一チャンネル。 */
  const seCh = (a: Attrs): string => "se" + Math.max(0, Math.floor(num(a.buf, 0)));

  // [hch] は縦中横非対応の平文フォールバック（設計判断、W3）: 初回のみ warn する。
  let warnedHch = false;

  // [hact] はホットスポットアクション非対応の簡易フォールバック（設計判断、W6）: 初回のみ warn する。
  let warnedHact = false;

  // [trans layer=] は対象外（設計判断、W5）: 真のレイヤ単位トランジションはシーングローバル
  // 合成の前提を崩すため、受理して全画面（従来どおり）にフォールバックする。初回のみ warn。
  let warnedTransLayer = false;
  const warnTransLayer = (): void => {
    if (warnedTransLayer) return;
    warnedTransLayer = true;
    api.sys.log("[trans] layer= は対象外のため全画面トランジションにフォールバックします", "warn");
  };

  const handlers: Record<string, TagHandler> = {
    // ---- メッセージ ----
    l: async () => {
      await p.waitAdvance();
    },
    p: async () => {
      await p.waitAdvance();
    },
    r: () => {
      p.newline();
    },
    // cm/ct/er は W3 で分離: cm=全レイヤクリア、ct=cm+カレントを message0/fore へ戻す、
    // er=カレントのみクリア（既定 1 レイヤ構成では 3 つとも同じ見た目になる＝後方互換）。
    cm: () => {
      p.clearAllMessages();
    },
    ct: () => {
      p.clearAllMessages();
      p.current.index = 0;
      p.renderNamePlate();
      p.updateNamePlateVisibility();
    },
    er: () => {
      p.clearMessage();
    },
    delay: (a) => {
      const speed = a.speed;
      if (speed === "nowait") p.cps = 0;
      else if (speed === "user") p.cps = p.settings.cps;
      else p.cps = num(speed, 30) > 0 ? 1000 / num(speed, 30) : 0;
    },
    nowait: () => {
      p.nowait = true;
    },
    endnowait: () => {
      p.nowait = false;
    },
    // ---- 文字装飾 (Wave 1) ----
    // font/style は同じ「現在のフォント状態」(currentFontState) を操作する（設計判断 9）。
    font: (a) => {
      applyFontAttrs(p, p.currentFontState, a);
    },
    resetfont: () => {
      p.currentFontState = { ...p.fontDefaults };
    },
    style: (a) => {
      applyFontAttrs(p, p.currentFontState, a);
      if (applyLineStyleAttrs(p, p.cur.currentLineStyle, a, "style")) {
        p.applyMessageLayerTextStyle(p.current.index);
        p.reflowMessageLayer(p.current.index);
      }
    },
    resetstyle: () => {
      p.currentFontState = { ...p.styleDefaults };
      p.cur.currentLineStyle = { ...p.cur.defaultLineStyle };
      p.applyMessageLayerTextStyle(p.current.index);
      p.reflowMessageLayer(p.current.index);
    },
    deffont: (a) => {
      applyFontAttrs(p, p.fontDefaults, a);
    },
    defstyle: (a) => {
      applyFontAttrs(p, p.styleDefaults, a);
      applyLineStyleAttrs(p, p.cur.defaultLineStyle, a, "defstyle");
    },
    // ルビは次の printText 呼び出し 1 回にだけ乗る（本文セグメント直後にグリフを追加）。
    ruby: (a) => {
      if (a.text) p.pendingRuby = a.text;
    },
    // text= の先頭 1 コードポイントだけを印字するエイリアス（サロゲートペア対応）。
    ch: (a) => {
      const t = a.text;
      if (!t) return;
      const c = t.codePointAt(0);
      if (c === undefined) return;
      p.printText(t.slice(0, c > 0xffff ? 2 : 1), null);
    },
    // x/y（px）は control セグメントとして Rust 側 (assign_lines) に渡し、行内 pen_x /
    // 行番号を直接上書きする（設計判断 3・11）。
    locate: (a) => {
      p.printControl({
        text: "",
        color: null,
        locate: { x: num(a.x, 0), y: a.y !== undefined ? num(a.y, 0) : undefined },
      });
    },
    indent: () => {
      p.printControl({ text: "", color: null, indent: true });
    },
    endindent: () => {
      p.printControl({ text: "", color: null, indent: false });
    },
    history: (a) => {
      p.backlogEnabled = a.output !== "false" && a.output !== "off";
    },
    // [name text=アリス] で名前欄プレートを表示、[name]（text 省略）で非表示。
    // [cm]/[er] では消えず、明示的に変えるまで維持される（KAG の慣習）。
    name: (a) => {
      p.setName(a.text ?? null);
    },

    // ---- 画像レイヤ ----
    image: (a) => {
      const name = a.layer ?? "base";
      const id = p.layers.get(name);
      if (id === undefined) {
        api.sys.log(`不明なレイヤ: ${name}`, "warn");
        return;
      }
      const page = (a.page === "back" ? "back" : "fore") as api.Page;
      const st = p.layerStates.get(name)!;
      if (a.storage) {
        const path = p.resolveStorage(kindOfLayer(name), a.storage);
        // clip=/flipud=/fliplr=/grayscale=（W5 拡張）: いずれか指定されていれば拡張ロード
        // （clip 抽出 → 左右反転 → 上下反転 → グレースケール化の順）、それ以外は従来通り。
        const clip = parseClip(a.clip);
        const flipH = bool(a.fliplr, false);
        const flipV = bool(a.flipud, false);
        const grayscale = bool(a.grayscale, false);
        if (clip || flipH || flipV || grayscale) {
          api.layer.loadImageEx(id, path, { clip: clip ?? undefined, flipH, flipV, grayscale }, page);
        } else {
          api.layer.loadImage(id, path, page);
        }
        st.path = path;
      }
      const x = a.left !== undefined || a.x !== undefined ? num(a.left ?? a.x, 0) : null;
      const y = a.top !== undefined || a.y !== undefined ? num(a.top ?? a.y, 0) : null;
      const props: api.LayerProps = {};
      if (x !== null) {
        props.x = x;
        st.x = x;
      }
      if (y !== null) {
        props.y = y;
        st.y = y;
      }
      if (a.opacity !== undefined) {
        props.opacity = num(a.opacity, 255);
        st.opacity = props.opacity;
      }
      // storage 指定時は既定で表示 (KAG の visible 既定に準拠)
      const vis = a.visible !== undefined ? bool(a.visible, true) : a.storage ? true : undefined;
      if (vis !== undefined) {
        props.visible = vis;
        st.visible = vis;
      }
      api.layer.set(id, props, page);
    },
    freeimage: (a) => {
      const name = a.layer ?? "base";
      const id = p.layers.get(name);
      if (id === undefined) return;
      const page = (a.page === "back" ? "back" : "fore") as api.Page;
      api.layer.clearImage(id, page);
      api.layer.set(id, { visible: false }, page);
      const st = p.layerStates.get(name)!;
      st.path = null;
      st.visible = false;
    },
    backlay: (a) => {
      if (a.layer) {
        const id = p.layers.get(a.layer);
        api.layer.backlay(id);
      } else {
        api.layer.backlay();
      }
    },
    // [pimage layer= page= storage= clip=/sx=sy=sw=sh= dx=/x= dy=/y=]（W5、W7 で
    // sx=/sy=/sw=/sh= を追加）: 画像レイヤに部分画像（clip/sx.. 省略時は全体）を
    // (dx, dy) を左上として重ね描きする。既存内容は消さない（W4 で実装済みの
    // `layer_blit_image` op をそのまま共用）。`sx=/sy=/sw=/sh=`（コピー元矩形）は KAG3
    // 標準の属性名で、`clip=` の別名として受理する（dx/dy は既存どおり。
    // `docs/kag-tag-compat.md`「pimage」備考参照）。
    pimage: (a) => {
      const name = a.layer ?? "0";
      const id = p.layers.get(name);
      if (id === undefined) {
        api.sys.log(`不明なレイヤ: ${name}`, "warn");
        return;
      }
      if (!a.storage) {
        api.sys.log("[pimage] に storage がありません", "error");
        return;
      }
      const path = p.resolveStorage(kindOfLayer(name), a.storage);
      const page = (a.page === "back" ? "back" : "fore") as api.Page;
      const dx = num(a.dx ?? a.x, 0);
      const dy = num(a.dy ?? a.y, 0);
      const clip = resolvePimageClip(a);
      api.layer.blitImage(id, path, page, dx, dy, clip ?? undefined);
    },

    // ---- レイヤアニメーション (move) ----
    move: (a) => {
      const name = a.layer ?? "0";
      const id = p.layers.get(name);
      if (id === undefined) {
        api.sys.log(`不明なレイヤ: ${name}`, "warn");
        return;
      }
      const st = p.layerStates.get(name)!;
      // path=（W5、W7 で時間配分を距離比例に修正）: time= は総時間として
      // native が呼出時の実効位置から各区間へ距離比例配分する。spline=true は現在位置を
      // 先頭点とする自然三次スプライン、accel= は全区間へ一律適用。
      if (a.path) {
        const points = parseMovePath(a.path);
        if (points.length === 0) {
          api.sys.log(`[move] path= の解析に失敗しました: ${a.path}`, "warn");
          return;
        }
        const spline = bool(a.spline, false);
        if (spline && points.length < 2) {
          api.sys.log("[move] spline=true の path= には少なくとも 2 点が必要です", "warn");
          return;
        }
        const totalTimeMs = num(a.time, 1000);
        const accel = a.accel !== undefined ? num(a.accel, 0) : 0;
        const legs: api.MoveArgs[] = points.map((pt) => {
          const leg: api.MoveArgs = { timeMs: 0, accel };
          if (pt.x !== undefined) {
            leg.toX = pt.x;
            st.x = pt.x;
          }
          if (pt.y !== undefined) {
            leg.toY = pt.y;
            st.y = pt.y;
          }
          if (pt.opacity !== undefined) {
            leg.toOpacity = pt.opacity;
            st.opacity = pt.opacity;
          }
          return leg;
        });
        api.layer.movePathByDistance(id, legs, totalTimeMs, "fore", spline);
        return;
      }
      const args: api.MoveArgs = { timeMs: num(a.time, 1000) };
      if (a.x !== undefined) {
        args.toX = num(a.x, st.x);
        st.x = args.toX;
      }
      if (a.y !== undefined) {
        args.toY = num(a.y, st.y);
        st.y = args.toY;
      }
      if (a.opacity !== undefined) {
        args.toOpacity = num(a.opacity, st.opacity);
        st.opacity = args.toOpacity;
      }
      if (a.accel !== undefined) args.accel = num(a.accel, 0);
      api.layer.move(id, args, "fore");
    },
    wm: async () => {
      await p.waitMove();
    },
    // [stopmove]（W5）: 全レイヤの move を目標値（path= の場合は最終区間）で即完了させる。
    // 既存の [wm]/skip モードと同じ実体 (api.layer.moveSkip → Scene::skip_all_tweens)。
    stopmove: () => {
      api.sys.log("[stopmove]");
      api.layer.moveSkip();
    },

    // ---- フレームアニメーション (.asd、Wave 5) ----
    // 実体は anim.ts の AnimManager（player.ts → anim.ts の一方向 import、buttons.ts と
    // 同じパターン）。Rust 側は無変更（既存 layer.loadImage の差し替えループを TS 側で回すだけ）。
    animstart: (a) => {
      const name = a.layer ?? "0";
      const id = p.layers.get(name);
      if (id === undefined) {
        api.sys.log(`不明なレイヤ: ${name}`, "warn");
        return;
      }
      if (!a.storage) {
        api.sys.log("[animstart] に storage がありません", "error");
        return;
      }
      const asdPath = p.resolveStorage(kindOfLayer(name), a.storage);
      let raw: AsdDef;
      try {
        raw = parseAsd(api.sys.readText(asdPath));
      } catch (e) {
        api.sys.log(`[animstart] .asd の読み込みに失敗しました: ${asdPath}: ${e}`, "error");
        return;
      }
      if (raw.cells.length === 0) {
        api.sys.log(`[animstart] .asd にセルがありません: ${asdPath}`, "warn");
        return;
      }
      const def: AsdDef = {
        loop: raw.loop,
        cells: raw.cells.map((c) => ({ path: p.resolveStorage("fg", c.path), wait: c.wait })),
      };
      const page = (a.page === "back" ? "back" : "fore") as api.Page;
      const props: api.LayerProps = { visible: true };
      if (a.left !== undefined || a.x !== undefined) props.x = num(a.left ?? a.x, 0);
      if (a.top !== undefined || a.y !== undefined) props.y = num(a.top ?? a.y, 0);
      api.layer.set(id, props, page);
      p.anim.start(id, page, def);
    },
    animstop: (a) => {
      const name = a.layer ?? "0";
      const id = p.layers.get(name);
      if (id === undefined) return;
      p.anim.stop(id);
    },
    // [wa layer=]: 指定レイヤのアニメーションが止まる（animstop、または loop=false の
    // .asd を最後まで再生し終える）まで待つ。ジャンプ要求ガード付き（p.waitWhile 参照）。
    wa: async (a) => {
      const name = a.layer ?? "0";
      const id = p.layers.get(name);
      if (id === undefined) return;
      await p.waitWhile(() => p.anim.isRunning(id));
    },

    // ---- トランジション ----
    trans: (a) => {
      const method =
        a.method === "universal" ? "universal" : a.method === "scroll" ? "scroll" : "crossfade";
      const args: api.TransArgs = {
        method,
        timeMs: num(a.time, 500),
      };
      if (method === "universal") {
        args.rule = p.resolveStorage("rule", a.rule ?? "wipe.png");
        args.vague = num(a.vague, 64);
      } else if (method === "scroll") {
        // KAG3 の scroll の主要属性（W5）: from=left|right|top|bottom, stay=stayfore|
        // staynone|staybase 相当。stay は受理するが描画上は区別しない（api.TransArgs 参照）。
        args.from = (a.from === "right" || a.from === "top" || a.from === "bottom" ? a.from : "left") as
          | "left"
          | "right"
          | "top"
          | "bottom";
        args.stay = (
          a.stay === "staynone" || a.stay === "staybase" ? a.stay : "stayfore"
        ) as "stayfore" | "staynone" | "staybase";
      }
      // layer=（W5）: 真のレイヤ単位トランジションはシーングローバル合成の前提を崩すため
      // 対象外。受理して全画面（従来どおり）にフォールバックし、初回のみ warn する
      // （`docs/kag-tag-compat.md`「trans」備考参照）。
      if (a.layer !== undefined) warnTransLayer();
      api.trans.begin(args);
    },
    wt: async () => {
      await p.waitTransition();
    },
    // [stoptrans]（W5）: 既存 trans_skip と同義（進行中のトランジションを即完了させる）。
    stoptrans: () => {
      api.sys.log("[stoptrans]");
      api.trans.skip();
    },

    // ---- メッセージウィンドウ ----
    // layer= 省略時はカレントレイヤに作用する（既定 1 レイヤ構成では W2 時点と完全一致）。
    // page= は受理するが、テキストレイヤは常に fore ページのみ描画する実装上の制約により
    // 見た目には影響しない（対応表の備考参照）。
    position: (a) => {
      const idx = p.msgLayerIndex(a.layer);
      if (idx === null) {
        api.sys.log(`[position] 不明なメッセージレイヤ: ${a.layer}`, "warn");
        return;
      }
      const ml = p.msgLayers[idx];
      const margins =
        a.marginl !== undefined || a.margint !== undefined || a.marginr !== undefined || a.marginb !== undefined;
      const resized = a.width !== undefined || a.height !== undefined || margins;
      if (a.left !== undefined) ml.cfg.left = num(a.left, ml.cfg.left);
      if (a.top !== undefined) ml.cfg.top = num(a.top, ml.cfg.top);
      if (a.width !== undefined) ml.cfg.width = num(a.width, ml.cfg.width);
      if (a.height !== undefined) ml.cfg.height = num(a.height, ml.cfg.height);
      // KAG3 標準: color=0xRRGGBB は窓の地色（アルファは opacity= / 既存値のまま）、marginl/t/r/b は本文の余白。
      if (a.color !== undefined) {
        const c = p.hexColor(a.color);
        if (c) ml.cfg.color = [c[0], c[1], c[2], ml.cfg.color[3]];
      }
      if (a.marginl !== undefined) ml.cfg.marginX = num(a.marginl, ml.cfg.marginX);
      if (a.margint !== undefined) ml.cfg.marginY = num(a.margint, ml.cfg.marginY);
      if (a.marginr !== undefined) ml.cfg.marginR = num(a.marginr, ml.cfg.marginR ?? ml.cfg.marginX);
      if (a.marginb !== undefined) ml.cfg.marginB = num(a.marginb, ml.cfg.marginB ?? ml.cfg.marginY);
      // frame=: 窓サイズ一致の背景画像（KAG3 標準の属性名。空文字で解除）。
      if (a.frame !== undefined) ml.cfg.frame = a.frame;
      if (a.opacity !== undefined) {
        ml.cfg.color = [ml.cfg.color[0], ml.cfg.color[1], ml.cfg.color[2], num(a.opacity, ml.cfg.color[3])];
      }
      if (a.visible !== undefined) ml.visible = bool(a.visible, true);
      p.applyMessageLayerConfig(idx);
      if (idx === p.current.index) {
        if (resized && !api.text.isDone(ml.layerId)) p.reflowMessageLayer(idx);
        else p.repaint();
        p.renderNamePlate(); // 名前欄プレートの位置もウィンドウに追従させる
      } else if (resized) {
        p.reflowMessageLayer(idx);
      }
    },

    // ---- 複数メッセージレイヤ (Wave 3) ----
    laycount: (a) => {
      p.setLayCount(num(a.messages, p.msgLayers.length));
    },
    // layer= 省略時は message0。message レイヤ名にマッチしない場合は前景レイヤ (image) 向けの
    // 汎用プロパティ設定にフォールバックする（メッセージレイヤ/前景レイヤ両対応）。
    layopt: (a) => {
      const name = a.layer ?? "message0";
      const idx = p.msgLayerIndex(name);
      if (idx !== null) {
        const ml = p.msgLayers[idx];
        if (a.left !== undefined) ml.cfg.left = num(a.left, ml.cfg.left);
        if (a.top !== undefined) ml.cfg.top = num(a.top, ml.cfg.top);
        if (a.opacity !== undefined) {
          ml.cfg.color = [ml.cfg.color[0], ml.cfg.color[1], ml.cfg.color[2], num(a.opacity, ml.cfg.color[3])];
        }
        if (a.index !== undefined) ml.z = num(a.index, ml.z);
        if (a.visible !== undefined) ml.visible = bool(a.visible, true);
        p.applyMessageLayerConfig(idx);
        if (idx === p.current.index) {
          p.repaint();
          p.renderNamePlate();
        }
        return;
      }
      const id = p.layers.get(name);
      if (id === undefined) {
        api.sys.log(`[layopt] 不明なレイヤ: ${name}`, "warn");
        return;
      }
      const page = (a.page === "back" ? "back" : "fore") as api.Page;
      const props: api.LayerProps = {};
      if (a.left !== undefined) props.x = num(a.left, 0);
      if (a.top !== undefined) props.y = num(a.top, 0);
      if (a.opacity !== undefined) props.opacity = num(a.opacity, 255);
      if (a.visible !== undefined) props.visible = bool(a.visible, true);
      if (a.index !== undefined) props.z = num(a.index, 0);
      api.layer.set(id, props, page);
    },
    // layer= 省略時はカレントの index を返す（player.ts の msgLayerIndex 参照）。
    current: (a) => {
      const idx = p.msgLayerIndex(a.layer);
      if (idx === null) {
        api.sys.log(`[current] 不明なメッセージレイヤ: ${a.layer}`, "warn");
        return;
      }
      p.current.index = idx;
      p.renderNamePlate();
      p.updateNamePlateVisibility();
      // a.page (fore/back) は受理するが、text ops が fore 固定のため見た目には影響しない。
    },
    // 次のクリックで全レイヤの表示が復帰する（右クリック非表示と同じ msgVisible フラグ）。
    hidemessage: () => {
      p.msgVisible = false;
      p.applyGlobalVisibility();
    },
    // [graph storage=...]: メッセージ本文にインライン画像を 1 グリフ扱いで挿入する。
    graph: (a) => {
      if (!a.storage) return;
      p.printImage(p.resolveStorage("fg", a.storage));
    },
    // [ptext layer= page= text= x= y=]: 任意レイヤへ直接テキストを描画する
    // （既存の text ops + W1 の locate 制御トークンを流用。segs ミラーには載せない）。
    ptext: (a) => {
      const idx = p.msgLayerIndex(a.layer);
      const layerId = idx !== null ? p.msgLayers[idx].layerId : p.layers.get(a.layer ?? "");
      if (layerId === undefined) {
        api.sys.log(`[ptext] 不明なレイヤ: ${a.layer ?? ""}`, "warn");
        return;
      }
      const x = num(a.x, 0);
      const y = a.y !== undefined ? num(a.y, 0) : undefined;
      api.text.print(
        layerId,
        [{ text: "", color: null, locate: { x, y } }, { text: a.text ?? "", color: null }],
        true,
        0,
      );
    },
    // 縦中横は非対応のため平文フォールバック（インスタンスごとに初回のみ warn）。
    hch: (a) => {
      if (!warnedHch) {
        warnedHch = true;
        api.sys.log("[hch] 縦中横は非対応のため平文で表示します", "warn");
      }
      if (a.text) p.printText(a.text, null);
    },
    // n 文字分 (= n × 現在 cps) の待ち。
    wc: async (a) => {
      const n = num(a.n, 1);
      const cps = p.cps;
      await p.wait(cps > 0 ? (n / cps) * 1000 : 0);
    },
    // enabled= ch= time=: 指定文字の後に自動ウェイトを入れる（既定オフ）。
    autowc: (a) => {
      if (a.enabled !== undefined) p.autowc.enabled = bool(a.enabled, true);
      if (a.ch !== undefined) p.autowc.ch = a.ch;
      if (a.time !== undefined) p.autowc.time = num(a.time, p.autowc.time);
    },
    // wait 基準時刻の保存のみ（[wait mode=until] が無い現実装では他に参照箇所が無い）。
    resetwait: () => {
      p.waitBaseMs = api.sys.now();
    },
    // srclayer= destlayer= srcpage= destpage= のレイヤ間コピー。
    copylay: (a) => {
      if (!a.srclayer || !a.destlayer) {
        api.sys.log("[copylay] srclayer/destlayer が必要です", "error");
        return;
      }
      const srcIdx = p.msgLayerIndex(a.srclayer);
      const dstIdx = p.msgLayerIndex(a.destlayer);
      const srcId = srcIdx !== null ? p.msgLayers[srcIdx].layerId : p.layers.get(a.srclayer);
      const dstId = dstIdx !== null ? p.msgLayers[dstIdx].layerId : p.layers.get(a.destlayer);
      if (srcId === undefined || dstId === undefined) {
        api.sys.log(`[copylay] 不明なレイヤ: ${a.srclayer} / ${a.destlayer}`, "warn");
        return;
      }
      const srcPage = (a.srcpage === "back" ? "back" : "fore") as api.Page;
      const dstPage = (a.destpage === "back" ? "back" : "fore") as api.Page;
      // W5 で修正した既知バグ（W4 で発見）: コピー先がメッセージレイヤの場合、ネイティブ
      // 位置 (x/y) は TS 側 MessageLayer.cfg が権威的に管理しているため、ここで x/y まで
      // 上書きすると cfg と食い違ったまま（次の position/layopt/ロードまで）残ってしまう。
      // メッセージレイヤ宛のときだけ位置を非コピーにする（内容は従来通りコピーする）。
      const copyPosition = dstIdx === null;
      api.layer.copy(srcId, srcPage, dstId, dstPage, copyPosition);
      if (dstIdx !== null) {
        // メッセージレイヤへのコピーは segs/links ミラーを同期できないためクリアする
        // （ネイティブ側の内容とミラーのズレを防ぐ。次の cm/er まではコピーした内容が
        // そのまま表示され続けるが、ホバー/リンクは効かなくなる）。
        const ml = p.msgLayers[dstIdx];
        ml.segs = [];
        ml.links = [];
        ml.currentText = "";
      }
    },

    // ---- 待ち ----
    wait: async (a) => {
      await p.wait(num(a.time, 0));
    },
    s: async () => {
      // 停止: ジャンプ要求 (リンク選択など) が来るまで待つ
      await p.waitForJump();
    },
    // [titlescreen]: Noiria 拡張。ui="modern" の専用タイトル画面（KagOptions.titleScreen）を出して選択を待つ。
    titlescreen: async () => {
      await p.runTitleScreen();
    },

    // ---- サウンド ----
    playbgm: (a) => {
      if (!a.storage) return;
      const path = p.resolveStorage("bgm", a.storage);
      const volume = num(a.volume, 100) / 100;
      api.audio.play(p.bgmCh, path, {
        loop: bool(a.loop, true),
        volume: p.volFor("bgm", volume),
        fadeInMs: num(a.fadein, 0),
      });
      p.currentBgm = { path, volume, paused: false };
    },
    fadeinbgm: (a) => {
      if (!a.storage) return;
      const path = p.resolveStorage("bgm", a.storage);
      const volume = num(a.volume, 100) / 100;
      api.audio.play(p.bgmCh, path, {
        loop: bool(a.loop, true),
        volume: p.volFor("bgm", volume),
        fadeInMs: num(a.time, 1000),
      });
      p.currentBgm = { path, volume, paused: false };
    },
    stopbgm: () => {
      api.audio.stop(p.bgmCh);
      p.currentBgm = null;
    },
    fadeoutbgm: (a) => {
      api.audio.stop(p.bgmCh, num(a.time, 1000));
      p.currentBgm = null;
    },
    fadebgm: (a) => {
      // W2 拡張: time= 指定でフェード、省略/0 なら従来通り即時変更
      const volume = num(a.volume, 100) / 100;
      const time = num(a.time, 0);
      if (time > 0) {
        api.audio.fadeTo(p.bgmCh, p.volFor("bgm", volume), time, "none");
      } else {
        api.audio.volume(p.bgmCh, p.volFor("bgm", volume));
      }
      if (p.currentBgm) p.currentBgm.volume = volume;
    },
    fadepausebgm: (a) => {
      api.sys.log(`[fadepausebgm] time=${num(a.time, 1000)}`);
      api.audio.fadeTo(p.bgmCh, 0, num(a.time, 1000), "pause");
      if (p.currentBgm) p.currentBgm.paused = true;
    },
    pausebgm: () => {
      api.sys.log("[pausebgm]");
      api.audio.pause(p.bgmCh);
      if (p.currentBgm) p.currentBgm.paused = true;
    },
    resumebgm: () => {
      api.sys.log("[resumebgm]");
      api.audio.resume(p.bgmCh);
      if (p.currentBgm) {
        // fadepausebgm 後は音量が 0 のままなので本来の音量へ戻す
        api.audio.volume(p.bgmCh, p.volFor("bgm", p.currentBgm.volume));
        p.currentBgm.paused = false;
      }
    },
    xchgbgm: (a) => {
      if (!a.storage) return;
      const time = num(a.time, 1000);
      const volume = num(a.volume, 100) / 100;
      const path = p.resolveStorage("bgm", a.storage);
      const oldCh = p.bgmCh;
      const newCh = oldCh === "bgm" ? "bgm2" : "bgm";
      api.sys.log(`[xchgbgm] ${a.storage} (${oldCh} → ${newCh}, ${time}ms)`);
      api.audio.stop(oldCh, time); // 旧 BGM をフェードアウトしつつ…
      api.audio.play(newCh, path, {
        loop: bool(a.loop, true),
        volume: p.volFor("bgm", volume),
        fadeInMs: time, // …新 BGM を同じ時間でフェードイン
      });
      p.bgmCh = newCh;
      p.currentBgm = { path, volume, paused: false };
    },
    bgmopt: (a) => {
      if (a.volume !== undefined) {
        api.sys.log(`[bgmopt] volume=${a.volume}`);
        p.setGlobalVolume("bgm", num(a.volume, 100));
      }
    },
    seopt: (a) => {
      if (a.volume !== undefined) {
        api.sys.log(`[seopt] volume=${a.volume}`);
        p.setGlobalVolume("se", num(a.volume, 100));
      }
    },
    setbgmstop: (a) => {
      api.sys.log(`[setbgmstop] storage=${a.storage ?? ""} target=${a.target ?? ""}`);
      p.setBgmStop(a.storage ?? null, a.target ?? null);
    },
    clearbgmstop: () => {
      api.sys.log("[clearbgmstop]");
      p.clearBgmStop();
    },
    wb: async (a) => {
      await p.waitFade(p.bgmCh, bool(a.canskip, true));
    },
    wl: async (a) => {
      await p.waitBgmEnd(bool(a.canskip, true));
    },
    playse: (a) => {
      if (!a.storage) return;
      const ch = seCh(a);
      const volume = num(a.volume, 100) / 100;
      api.audio.play(ch, p.resolveStorage("se", a.storage), {
        loop: bool(a.loop, false),
        volume: p.volFor("se", volume),
      });
      p.noteSePlay(ch, volume);
    },
    fadeinse: (a) => {
      if (!a.storage) return;
      const ch = seCh(a);
      const volume = num(a.volume, 100) / 100;
      api.audio.play(ch, p.resolveStorage("se", a.storage), {
        loop: bool(a.loop, false),
        volume: p.volFor("se", volume),
        fadeInMs: num(a.time, 1000),
      });
      p.noteSePlay(ch, volume);
    },
    fadeoutse: (a) => {
      const ch = seCh(a);
      api.audio.stop(ch, num(a.time, 1000));
      p.noteSeStop(ch);
    },
    fadese: (a) => {
      const ch = seCh(a);
      const volume = num(a.volume, 100) / 100;
      api.sys.log(`[fadese] buf=${num(a.buf, 0)} volume=${num(a.volume, 100)}`);
      api.audio.fadeTo(ch, p.volFor("se", volume), num(a.time, 1000), "none");
      p.noteSePlay(ch, volume);
    },
    stopse: (a) => {
      const ch = seCh(a);
      api.audio.stop(ch);
      p.noteSeStop(ch);
    },
    ws: async (a) => {
      await api.audio.waitEnd(seCh(a));
    },
    wf: async (a) => {
      await p.waitFade(seCh(a), bool(a.canskip, true));
    },
    // sourceforge 版拡張タグ。Noiria の SE バッファは buf= 指定で動的確保されるため
    // 事前確保は不要 → 受理して no-op（docs/kag-tag-compat.md の備考参照）。
    entersebuf: () => {},
    playvoice: (a) => {
      if (!a.storage) return;
      p.playVoice(p.resolveStorage("voice", a.storage), num(a.volume, 100) / 100);
    },
    stopvoice: () => {
      p.stopVoice();
    },

    // ---- 動画 ----
    video: (a) => {
      if (!videoSlotIsZero(a, "video")) return;
      const name = a.layer ?? "1";
      const id = p.layers.get(name);
      if (id === undefined) {
        api.sys.log(`不明なレイヤ: ${name}`, "warn");
        return;
      }
      let frame: number | null = null;
      let positionMs: number | null = null;
      if (a.frame !== undefined) {
        frame = videoFrameAttr(a, "frame", "video");
        if (frame === null) return;
      } else if (a.position !== undefined) {
        positionMs = Number(a.position);
        if (!Number.isFinite(positionMs) || positionMs < 0) {
          api.sys.log(`[video] position=${a.position} は非負のミリ秒ではありません`, "warn");
          return;
        }
      }
      if (a.left !== undefined || a.top !== undefined) {
        const props: api.LayerProps = {};
        if (a.left !== undefined) props.x = num(a.left, 0);
        if (a.top !== undefined) props.y = num(a.top, 0);
        api.layer.set(id, props, "fore");
        api.layer.set(id, props, "back");
      }
      const visible = bool(a.visible, true);
      api.layer.set(id, { visible }, "fore");
      if (a.storage) {
        const path = p.resolveStorage("video", a.storage);
        const loop = bool(a.loop, false);
        if (frame !== null || positionMs !== null) {
          api.video.playAt(id, path, loop, frame, positionMs);
        } else {
          api.video.play(id, path, loop);
        }
      } else if (frame !== null) {
        api.video.seekFrame(frame);
      } else if (positionMs !== null) {
        api.video.seekPosition(positionMs);
      }
    },
    wv: async (a) => {
      if (!videoSlotIsZero(a, "wv")) return;
      if (p.mode.skip) {
        api.video.stop();
        return;
      }
      await api.video.waitEnd();
    },
    stopvideo: (a) => {
      if (!videoSlotIsZero(a, "stopvideo")) return;
      api.video.stop();
    },

    // ---- 揺れ ----
    quake: (a) => {
      const time = num(a.time, 500);
      api.sys.quake(num(a.hmax, 10), num(a.vmax, 10), time);
      p.lastQuakeMs = time;
    },
    wq: async () => {
      await p.wait(p.lastQuakeMs);
    },
    // [stopquake]（W5）: 画面揺れを即座に停止する。
    stopquake: () => {
      api.sys.log("[stopquake]");
      api.sys.quakeStop();
    },

    // ---- フロー ----
    // store=false（W6）: 着地先ラベルの通過で 1 回だけ自動スナップショット更新を止める
    // （KAG3 準拠、`docs/implementation-notes.md`「2026-07-11 W6」参照）。
    jump: (a) => {
      p.clearMessage();
      if (a.store === "false") p.markStoreFalse();
      p.requestJump(a.storage ?? null, a.target ?? null);
    },
    call: (a) => {
      if (a.store === "false") p.markStoreFalse();
      p.pushCall(a.storage ?? null, a.target ?? null);
    },
    return: () => {
      p.popReturn();
    },
    if: (a) => {
      const cond = !!vars.evalExpr(a.exp ?? "false", p.sys, p);
      if (!cond) p.skipToNextBranch();
    },
    // elsif/else に実行が「素で」到達した = 直前の分岐を実行し終えた、ということ。
    // (偽側からの到達は skipToNextBranch が消費して直接ジャンプするためここには来ない)
    elsif: () => {
      p.skipToEndif();
    },
    else: () => {
      p.skipToEndif();
    },
    endif: () => {
      /* 何もしない */
    },
    macro: (a) => {
      if (!a.name) {
        api.sys.log("[macro] に name がありません", "error");
        return;
      }
      p.captureMacro(a.name);
    },
    endmacro: () => {
      /* captureMacro が消費するためここには来ない */
    },

    // ---- 変数 ----
    eval: (a) => {
      if (a.exp) vars.evalStmt(a.exp, p.sys, p);
    },
    emb: (a) => {
      if (a.exp) p.printText(String(vars.evalExpr(a.exp, p.sys, p)), null);
    },

    // ---- 選択肢 ----
    link: (a) => {
      p.beginLink(a.target ?? null, a.storage ?? null, a.exp ?? null);
    },
    endlink: () => {
      p.endLink();
    },

    // ---- UI・ボタン・クリック制御 (Wave 4) ----
    // button/checkbox/edit の実体は buttons.ts の ButtonManager（player.ts → buttons.ts
    // の一方向 import。tags.ts からは p.buttons 経由でのみ触る）。
    button: (a) => {
      if (!a.graphic) {
        api.sys.log("[button] に graphic がありません", "error");
        return;
      }
      const path = p.resolveStorage("fg", a.graphic);
      api.sys.log(`[button] graphic=${a.graphic} x=${num(a.x, 0)} y=${num(a.y, 0)}`);
      p.buttons.addButton(num(a.x, 0), num(a.y, 0), path, a.target ?? null, a.storage ?? null, a.exp ?? null);
    },
    checkbox: (a) => {
      if (!a.name) {
        api.sys.log("[checkbox] に name がありません", "error");
        return;
      }
      api.sys.log(`[checkbox] name=${a.name}`);
      p.buttons.addCheckbox(num(a.x, 0), num(a.y, 0), a.name);
    },
    edit: (a) => {
      if (!a.name) {
        api.sys.log("[edit] に name がありません", "error");
        return;
      }
      api.sys.log(`[edit] name=${a.name}`);
      p.buttons.addEdit(num(a.x, 0), num(a.y, 0), a.name, num(a.length, 30));
    },
    commit: () => {
      api.sys.log("[commit]");
      p.buttons.commitEdit();
    },
    // [input title= prompt= initial= name=]: 自作テキスト入力モーダル（player.ts の
    // openInputDialog）。name は結果を書き込む変数式（f./sf.、省略時は破棄するだけ）。
    input: async (a) => {
      api.sys.log(`[input] title=${a.title ?? ""}`);
      await p.openInputDialog(a.title ?? "", a.prompt ?? "", a.initial ?? "", a.name ?? null);
      api.sys.log("[input] 終了");
    },
    // [glyph line= fix= left= top=]: クリック待ち記号のカスタム。line 省略で既定（無表示）
    // に戻す。fix=false（既定、W7）はクリック待ち開始時のペン位置へ追従、fix=true は
    // 従来通り固定位置（対応表参照）。
    glyph: (a) => {
      api.sys.log(`[glyph] line=${a.line ?? "(既定)"}`);
      const path = a.line ? p.resolveStorage("fg", a.line) : null;
      p.setGlyph(
        path,
        a.left !== undefined ? num(a.left, 0) : null,
        a.top !== undefined ? num(a.top, 0) : null,
        bool(a.fix, false),
      );
    },
    // [click storage= target=]/[wheel storage= target=]: マウスクリック/ホイールで
    // target へジャンプする常設フック。[cclick]/[cwheel] で解除。
    click: (a) => {
      api.sys.log(`[click] storage=${a.storage ?? ""} target=${a.target ?? ""}`);
      p.setEventClick(a.storage ?? null, a.target ?? null);
    },
    cclick: () => {
      api.sys.log("[cclick]");
      p.clearEventClick();
    },
    wheel: (a) => {
      api.sys.log(`[wheel] storage=${a.storage ?? ""} target=${a.target ?? ""}`);
      p.setEventWheel(a.storage ?? null, a.target ?? null);
    },
    cwheel: () => {
      api.sys.log("[cwheel]");
      p.clearEventWheel();
    },
    // [timeout time= storage= target=]: time ms クリックが無ければジャンプ。[ctimeout] で解除。
    timeout: (a) => {
      api.sys.log(`[timeout] time=${num(a.time, 0)}`);
      p.setTimeoutJump(num(a.time, 0), a.storage ?? null, a.target ?? null);
    },
    ctimeout: () => {
      api.sys.log("[ctimeout]");
      p.clearTimeoutJump();
    },
    // [waitclick]: 左クリックを 1 回待つだけ（文章送りの状態には影響しない）。
    waitclick: async () => {
      api.sys.log("[waitclick] 待機開始");
      await p.waitClickOnly();
      api.sys.log("[waitclick] クリックを受信");
    },
    // [waittrig name=...]: トリガ待ち（発火は `kag.trigger(name)`（KAG3 準拠）または
    // `sys.player.trigger(name)`（後方互換のエイリアス）のどちらでも良い。対応表参照）。
    waittrig: async (a) => {
      if (a.name) {
        api.sys.log(`[waittrig] name=${a.name} 待機開始`);
        await p.waitTrigger(a.name);
        api.sys.log(`[waittrig] name=${a.name} 発火`);
      }
    },
    // [locklink]/[unlocklink]: リンク・ボタン・クリック送りの一括ロック。
    locklink: () => {
      api.sys.log("[locklink]");
      p.linksLocked = true;
      p.buttons.setGlobalLocked(true);
    },
    unlocklink: () => {
      api.sys.log("[unlocklink]");
      p.linksLocked = false;
      p.buttons.setGlobalLocked(false);
    },
    // [clickskip enabled=]: false でクリックによるスキップモード解除を禁止する。
    clickskip: (a) => {
      api.sys.log(`[clickskip] enabled=${bool(a.enabled, true)}`);
      p.clickSkipEnabled = bool(a.enabled, true);
    },
    // [nextskip]: 次の [l]/[p]/[s] までのワンショットスキップ。
    nextskip: () => {
      api.sys.log("[nextskip]");
      p.requestNextSkip();
    },
    // [cancelautomode]: オートモードを解除する（既存の A キートグルと同じフラグ）。
    cancelautomode: () => {
      api.sys.log("[cancelautomode]");
      p.mode.auto = false;
    },
    // [rclick enabled= jump= call= storage= target=]: 右クリック動作のカスタム。
    // 属性なしの [rclick] は既定（全メッセージレイヤ表示トグル）へ戻す。
    rclick: (a) => {
      if (a.enabled !== undefined && !bool(a.enabled, true)) {
        api.sys.log("[rclick] enabled=false");
        p.rclickAction = "disabled";
      } else if (bool(a.jump, false) || bool(a.call, false)) {
        api.sys.log(`[rclick] jump/call storage=${a.storage ?? ""} target=${a.target ?? ""}`);
        p.rclickAction = { call: bool(a.call, false), storage: a.storage ?? null, target: a.target ?? null };
      } else {
        api.sys.log("[rclick] 既定に戻す");
        p.rclickAction = "default";
      }
    },
    // [ask]（sourceforge 拡張）: 自作の「はい/いいえ」ダイアログ（player.ts の
    // askYesNo）。target/storage は「はい」を選んだ場合のみジャンプする。
    ask: async (a) => {
      api.sys.log(`[ask] text=${a.text ?? ""}`);
      const answer = await p.askYesNo(a.text ?? "", a.var ?? null);
      api.sys.log(`[ask] answer=${answer}`);
      if (answer && (a.target || a.storage)) {
        p.clearMessage();
        p.requestJump(a.storage ?? null, a.target ?? null);
      }
    },

    // ---- ストア系 (Wave 6、tempsave/tempload は W7 で KAG3 準拠に修正) ----
    // place=（数値、既定 0）が 0 以上ならディスクの "temp{place}" スロット、負数なら
    // メモリのみの tempSlots を使う（KAG3 で「負の place はメモリ専用スロット」という慣習に
    // 対応。docs/implementation-notes.md「2026-07-12」参照）。
    tempsave: (a) => {
      if (p.guardStore("tempsave")) return;
      const place = num(a.place, 0);
      p.tempSave(place);
      api.sys.log(`[tempsave] place=${place}`);
    },
    tempload: async (a) => {
      if (p.guardStore("tempload")) return;
      const place = num(a.place, 0);
      const ok = await p.tempLoad(place);
      if (!ok) {
        api.sys.log(`[tempload] place=${place} にデータがありません`, "warn");
        return;
      }
      api.sys.log(`[tempload] place=${place}`);
    },
    // 変数 (f) のみのシリアライズ保存（save とは別粒度、対応する restore タグは無い）。
    store: (a) => {
      if (p.guardStore("store")) return;
      const place = num(a.place, 0);
      api.sys.saveWrite(`store${place}`, JSON.stringify(vars.f));
      api.sys.log(`[store] place=${place}`);
    },
    disablestore: (a) => {
      const disabled = bool(a.enabled, true);
      p.setStoreDisabled(disabled);
      api.sys.log(`[disablestore] enabled=${disabled}`);
    },
    // [record]/[goback]: trail（履歴リング、goback 用）への明示記録/消費。
    // startanchor/gotostart（下記）とは独立（W7 で分離、docs/implementation-notes.md
    // 「2026-07-12」参照）。
    record: () => {
      p.pushTrail();
      api.sys.log("[record]");
    },
    goback: async () => {
      if (p.guardStore("goback")) return;
      const ok = await p.goBack();
      if (!ok) api.sys.log("[goback] 記録がありません", "warn");
      else api.sys.log("[goback]");
    },
    // [gotostart]: [startanchor] で記録したアンカーへ戻る（KAG3 準拠、W7 で修正）。
    gotostart: async () => {
      if (p.guardStore("gotostart")) return;
      const ok = await p.goToStart();
      if (!ok) api.sys.log("[gotostart] アンカーが設定されていません", "warn");
      else api.sys.log("[gotostart]");
    },
    // [startanchor enabled=]（既定 true）: 実行時点の完全スナップショットをアンカーへ記録する
    // （KAG3 準拠、W7 で修正。旧実装は「[s] ごとの自動 record の on/off」だった）。
    // enabled=false はアンカーを破棄する。
    startanchor: (a) => {
      const enabled = bool(a.enabled, true);
      p.setAnchor(enabled);
      api.sys.log(`[startanchor] enabled=${enabled}`);
    },
    locksnapshot: () => {
      p.setSnapshotLocked(true);
      api.sys.log("[locksnapshot]");
    },
    unlocksnapshot: () => {
      p.setSnapshotLocked(false);
      api.sys.log("[unlocksnapshot]");
    },
    // [save place= ask=]/[load place= ask=]: 既存の save()/load() 内部関数を直接呼ぶ。
    // ask=true は W4 の askYesNo を流用する。
    save: async (a) => {
      const slot = `save${num(a.place, 1)}`;
      api.sys.log(`[save] place=${num(a.place, 1)} ask=${bool(a.ask, false)}`);
      if (bool(a.ask, false)) {
        const yes = await p.askYesNo("セーブしますか？", null);
        if (!yes) return;
      }
      p.save(slot);
    },
    load: async (a) => {
      const slot = `save${num(a.place, 1)}`;
      api.sys.log(`[load] place=${num(a.place, 1)} ask=${bool(a.ask, false)}`);
      if (bool(a.ask, false)) {
        const yes = await p.askYesNo("ロードしますか？", null);
        if (!yes) return;
      }
      await p.load(slot);
    },
    // [copybookmark src= dest=]/[erasebookmark place=]: save{N} スロット間コピー/削除。
    // sys.saveWrite/saveRead でスロットを直接操作する（save/load タグと同じ save{N} 命名）。
    copybookmark: (a) => {
      if (p.guardStore("copybookmark")) return;
      if (a.src === undefined || a.dest === undefined) {
        api.sys.log("[copybookmark] src/dest が必要です", "error");
        return;
      }
      const srcSlot = `save${num(a.src, 0)}`;
      const destSlot = `save${num(a.dest, 0)}`;
      const json = api.sys.saveRead(srcSlot);
      if (json === null) {
        api.sys.log(`[copybookmark] コピー元にデータがありません: ${srcSlot}`, "warn");
        return;
      }
      api.sys.saveWrite(destSlot, json);
      api.sys.log(`[copybookmark] ${srcSlot} -> ${destSlot}`);
    },
    // 新規 op sys.saveDelete（W7）でスロットファイルを実削除する（KAG3 準拠、真の削除）。
    // 削除後はセーブメニュー上も「（空きスロット）」表示に戻る（旧実装は空 JSON 書き込みの
    // 代替で日付・タイトル空欄のスロットが残り続けていた。docs/implementation-notes.md
    // 「2026-07-12」参照）。
    erasebookmark: (a) => {
      if (p.guardStore("erasebookmark")) return;
      const slot = `save${num(a.place, 0)}`;
      api.sys.saveDelete(slot);
      api.sys.log(`[erasebookmark] ${slot}`);
    },
    clearvar: () => {
      vars.replaceF({});
      api.sys.log("[clearvar]");
    },
    clearsysvar: () => {
      vars.replaceSf({});
      p.resetSysDefaults();
      api.sys.log("[clearsysvar]");
    },

    // ---- スクリプト系 (Wave 6) ----
    // iscript/endscript は parser.ts が生テキストを 1 コマンドとして収集し、player.ts の
    // execCommand が直接 vars.evalStmt を呼ぶ（このハンドラは endscript が単独で出現する
    // 壊れたスクリプト向けの安全網。macro/endmacro と同じ前例、通常は実行時に来ない）。
    endscript: () => {},
    // [ignore exp=]: exp が真ならブロックを [endignore] まで読み飛ばす
    // （skipToNextBranch と同型の if/ignore 入れ子走査、KagPlayer.skipToEndignore 参照）。
    ignore: (a) => {
      if (vars.evalExpr(a.exp ?? "false", p.sys, p)) p.skipToEndignore();
    },
    endignore: () => {
      /* 何もしない（ignore が真でなければそのまま通過する） */
    },
    erasemacro: (a) => {
      if (a.name) {
        p.macros.delete(a.name.toLowerCase());
        api.sys.log(`[erasemacro] name=${a.name}`);
      }
    },
    trace: (a) => {
      if (a.exp !== undefined) api.sys.log(String(vars.evalExpr(a.exp, p.sys, p)));
    },

    // ---- 履歴系 (Wave 6) ----
    hr: () => {
      p.insertBacklogRule();
    },
    hact: (a) => {
      if (!warnedHact) {
        warnedHact = true;
        api.sys.log(
          "[hact] ホットスポットアクションは非対応です。テキストのみバックログへ記録します",
          "warn",
        );
      }
      void a;
      /* [endhact] までの本文はそのまま通常の printText 経路で表示・バックログ記録される */
    },
    endhact: () => {
      /* 何もしない */
    },
    showhistory: async () => {
      await p.openBacklog();
    },

    // ---- 動画拡張 (Wave 6) ----
    // openvideo/preparevideo: ロードのみ（preparevideo は現在の開始フレームを表示する）。
    // playvideo: 再生開始（storage= 指定時は openvideo/preparevideo を経ずに直接再生できる）。
    openvideo: (a) => {
      if (!videoSlotIsZero(a, "openvideo")) return;
      const name = a.layer ?? "1";
      const id = p.layers.get(name);
      if (id === undefined) {
        api.sys.log(`不明なレイヤ: ${name}`, "warn");
        return;
      }
      if (!a.storage) {
        api.sys.log("[openvideo] に storage がありません", "error");
        return;
      }
      api.video.open(id, p.resolveStorage("video", a.storage), bool(a.loop, false));
      api.sys.log(`[openvideo] layer=${name} storage=${a.storage}`);
    },
    preparevideo: (a) => {
      if (!videoSlotIsZero(a, "preparevideo")) return;
      const name = a.layer ?? "1";
      const id = p.layers.get(name);
      if (id === undefined) {
        api.sys.log(`不明なレイヤ: ${name}`, "warn");
        return;
      }
      api.layer.set(id, { visible: true }, "fore");
      if (a.storage) {
        api.video.prepare(id, p.resolveStorage("video", a.storage), bool(a.loop, false));
        api.sys.log(`[preparevideo] layer=${name} storage=${a.storage}`);
      } else {
        api.video.prepareOpened(id);
        api.sys.log(`[preparevideo] layer=${name} storage=(opened)`);
      }
    },
    playvideo: (a) => {
      if (!videoSlotIsZero(a, "playvideo")) return;
      const name = a.layer ?? "1";
      const id = p.layers.get(name);
      if (id === undefined) {
        api.sys.log(`不明なレイヤ: ${name}`, "warn");
        return;
      }
      api.sys.log(`[playvideo] layer=${name}`);
      if (a.storage) {
        api.layer.set(id, { visible: true }, "fore");
        api.video.play(id, p.resolveStorage("video", a.storage), bool(a.loop, false));
      } else {
        api.video.playPrepared(id);
      }
    },
    pausevideo: (a) => {
      if (!videoSlotIsZero(a, "pausevideo")) return;
      api.sys.log("[pausevideo]");
      api.video.pause();
    },
    resumevideo: (a) => {
      if (!videoSlotIsZero(a, "resumevideo")) return;
      api.sys.log("[resumevideo]");
      api.video.resume();
    },
    rewindvideo: (a) => {
      if (!videoSlotIsZero(a, "rewindvideo")) return;
      api.sys.log("[rewindvideo]");
      api.video.rewind();
    },
    videoevent: (a) => {
      if (!videoSlotIsZero(a, "videoevent")) return;
      const frame = videoFrameAttr(a, "frame", "videoevent");
      if (frame === null) return;
      api.video.setPeriodEvent(frame);
    },
    cancelvideoevent: (a) => {
      if (!videoSlotIsZero(a, "cancelvideoevent")) return;
      api.video.cancelPeriodEvent();
    },
    videosegloop: (a) => {
      if (!videoSlotIsZero(a, "videosegloop")) return;
      const start = videoFrameAttr(a, "start", "videosegloop");
      const end = videoFrameAttr(a, "end", "videosegloop");
      if (start === null || end === null) return;
      if (start >= end) {
        api.sys.log(`[videosegloop] start=${start} は end=${end} より小さくなければなりません`, "warn");
        return;
      }
      api.video.setSegmentLoop(start, end);
    },
    cancelvideosegloop: (a) => {
      if (!videoSlotIsZero(a, "cancelvideosegloop")) return;
      api.video.cancelSegmentLoop();
    },
    wp: async (a) => {
      if (!videoSlotIsZero(a, "wp")) return;
      const waitFor = videoWaitKind(a.for);
      if (waitFor === null) {
        api.sys.log(`[wp] for=${a.for ?? ""} は不明な動画イベント種別です`, "warn");
        return;
      }
      const generation = api.video.generation();
      if (api.video.takeEvent(waitFor) || !api.video.isPlaying()) return;
      await p.waitWhile(() => {
        if (api.video.generation() !== generation) return false;
        if (api.video.takeEvent(waitFor)) return false;
        return api.video.isPlaying();
      });
    },

    // ---- システム ----
    // [sysbar visible=] / [autosave enabled=]: Noiria 拡張。タイトル画面などで常設システムボタンを隠す・
    // オートセーブを止める用（ロードすると既定の有効へ戻る）。
    sysbar: (a) => {
      p.setSysBarEnabled(bool(a.visible, true));
    },
    autosave: (a) => {
      p.autoSaveOn = bool(a.enabled, true);
    },
    title: (a) => {
      if (a.name) api.sys.title(a.name);
    },
    cancelskip: () => {
      p.mode.skip = false;
    },
    autowait: (a) => {
      p.autoWaitMs = num(a.time, p.autoWaitMs);
    },
    screenshot: (a) => {
      api.sys.screenshot(a.slot ?? "shot");
    },
    close: () => {
      p.quitGame();
    },
  };

  // ---- 対象外スタブ 10 種 (Wave 6) ----
  // クリッカブルマップ/プラグイン/動画レイヤ等、対応表で「対象外」とした KAG3 標準タグ。
  // warn は 1 回だけ（インスタンスごと）、本文は対応表の備考と一致させる。
  const STUB_REASONS: Record<string, string> = {
    loadplugin: "対象外: Kirikiri プラグイン (.dll) 機構。Noiria は TS の import 機構が上位互換のため対象外です。",
    mapimage: "対象外: クリッカブルマップ画像 (.ma 形式) の読み込み。button/link で代替可能なため対象外です。",
    mapaction: "対象外: クリッカブルマップの領域アクション定義。button/link で代替済みのため対象外です。",
    mapdisable: "対象外: クリッカブルマップの無効化。button/link で代替済みのため対象外です。",
    mappfont: "対象外: プリレンダリング済みフォントデータの管理。自作ラスタライザ (font/raster) のため不要です。",
    videolayer: "対象外: 動画の出力先レイヤ切替。Noiria は video タグの layer= 属性で代替済みです。",
    clearvideolayer: "対象外: videolayer のクリア。同上、代替済みのため対象外です。",
    setbgmlabel: "対象外: BGM のループ区間ラベル設定。自作 OGG/Vorbis デコーダは .sli 等に非対応のため対象外です。",
    clearbgmlabel: "対象外: setbgmlabel の解除。同上、対象外です。",
    cursor: "対象外: マウスカーソル画像の切替。ヘッドレス検証では見えず実利が薄いため対象外です。",
  };
  const warnedStubs = new Set<string>();
  // for-of の const 束縛をクロージャで捕まえるのではなく、forEach のコールバック引数
  // （呼び出しごとに新しい束縛）を使う。tsint 実機検証で for-of + クロージャの組み合わせが
  // 全ハンドラが最終イテレーションの値を共有してしまう問題を確認したため（V8 は正しく
  // イテレーションごとに束縛が分かれる。docs/implementation-notes.md「2026-07-11 W6」参照）。
  Object.entries(STUB_REASONS).forEach(([name, reason]) => {
    handlers[name] = () => {
      if (warnedStubs.has(name)) return;
      warnedStubs.add(name);
      api.sys.log(`[${name}] ${reason}`, "warn");
    };
  });

  return handlers;
}
