// noiria:kag/vars — KAG のゲーム変数 (f) / システム変数 (sf) / 一時変数 (tf) と式評価。
// krkrZ 同様、シナリオは信頼されたコードとして全権で評価する。

export const f: Record<string, unknown> = {};
export const sf: Record<string, unknown> = {};
export const tf: Record<string, unknown> = {};

/** f を丸ごと差し替える（ロード時用） */
export function replaceF(data: Record<string, unknown>): void {
  for (const k of Object.keys(f)) delete f[k];
  Object.assign(f, data);
}

export function replaceSf(data: Record<string, unknown>): void {
  for (const k of Object.keys(sf)) delete sf[k];
  Object.assign(sf, data);
}

/** 式として評価して値を返す ([emb exp=...] や &属性値)。
 * `kag`（KAG3 標準グローバル、`sys.player` と同一の KagPlayer インスタンス）を第 3 引数
 * として渡せる（省略可、既定 undefined）。呼び出し側は `sys.player` を渡す
 * （`docs/kag-tag-compat.md`「waittrig」参照。KAG3 の `[eval exp="kag.trigger('x')"]` を
 * そのまま動かすための追加、`sys.player` は後方互換で残す）。 */
export function evalExpr(exp: string, sys: unknown, kag?: unknown): unknown {
  return new Function("f", "sf", "tf", "sys", "kag", `"use strict"; return (${exp});`)(f, sf, tf, sys, kag);
}

/** 文として実行する ([eval exp=...])。kag 引数は evalExpr と同じ。 */
export function evalStmt(exp: string, sys: unknown, kag?: unknown): void {
  new Function("f", "sf", "tf", "sys", "kag", `"use strict"; ${exp};`)(f, sf, tf, sys, kag);
}
