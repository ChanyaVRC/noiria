// noiria:kag/anim — [animstart]/[animstop]/[wa]（W5）用の最小 .asd アニメ定義パーサと
// レイヤ単位のフレームループ管理。buttons.ts と同じ「自己完結モジュール」パターン
// （`api` だけに依存し、`KagPlayer` 型には依存しない）を踏襲する。import は
// player.ts → anim.ts の一方向のみ（player.ts ⇄ tags.ts の循環 import 事故と同じ罠を
// 避ける、`docs/implementation-notes.md`「2026-07-11 W1」参照）。

import * as api from "noiria:api";

export interface AsdCell {
  /// resolveStorage 適用前の相対パス（.asd に書かれたそのままの文字列）。
  path: string;
  /// このセルを表示し続ける時間 (ms)。
  wait: number;
}

export interface AsdDef {
  loop: boolean;
  cells: AsdCell[];
}

/**
 * .asd アニメ定義の最小サブセットパーサ（W5）。対応する記法:
 *  - `;` 始まりの行 / 空行: コメント・無視
 *  - `loop=true|false`: ループ可否（省略時 true）
 *  - `セル番号,graphic,wait`: セル定義。graphic は resolveStorage 適用前の相対パス
 *    （呼び出し側が "fg" として解決する）、wait は ms。セル番号は識別用のラベルに過ぎず、
 *    再生順は常に記述順（ファイル中に出てきた順）。
 * KAG3 本来の .asd（seg=/module= 等の条件分岐、複数パターン集合の切替）はフルには対応せず、
 * 単一の直列パターン列だけを読む（対応表 `docs/kag-tag-compat.md` の `animstart` 備考参照）。
 * 純関数（単体テスト対象）。
 */
export function parseAsd(src: string): AsdDef {
  let loop = true;
  const cells: AsdCell[] = [];
  for (const rawLine of src.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (line.length === 0 || line.startsWith(";")) continue;
    const loopMatch = /^loop\s*=\s*(true|false)$/i.exec(line);
    if (loopMatch) {
      loop = loopMatch[1].toLowerCase() === "true";
      continue;
    }
    const parts = line.split(",");
    if (parts.length < 3) continue;
    const wait = Number(parts[2].trim());
    cells.push({ path: parts[1].trim(), wait: Number.isFinite(wait) ? wait : 0 });
  }
  return { loop, cells };
}

interface AnimState {
  layerId: number;
  page: api.Page;
  def: AsdDef;
  index: number;
  /// 世代カウンタ: stop()/start() の再入で古いループを自然消滅させる
  /// （setbgmstop/timeout と同じ既存パターン）。
  gen: number;
}

/**
 * [animstart]/[animstop]/[wa]（W5）: レイヤ id ごとに 1 本のフレームループを管理する。
 * `start()` は既存ループを止めてから新しいループを開始する。ループは
 * `api.layer.loadImage` で画像を差し替えつつ `api.sys.wait` する単純なもの（Rust 側は無変更）。
 */
export class AnimManager {
  private anims = new Map<number, AnimState>();
  private gens = new Map<number, number>();

  start(layerId: number, page: api.Page, def: AsdDef): void {
    this.stop(layerId);
    if (def.cells.length === 0) return;
    const gen = (this.gens.get(layerId) ?? 0) + 1;
    this.gens.set(layerId, gen);
    const state: AnimState = { layerId, page, def, index: 0, gen };
    this.anims.set(layerId, state);
    void this.loop(state);
  }

  private async loop(state: AnimState): Promise<void> {
    while (this.anims.get(state.layerId)?.gen === state.gen) {
      const cell = state.def.cells[state.index];
      api.layer.loadImage(state.layerId, cell.path, state.page);
      await api.sys.wait(cell.wait);
      if (this.anims.get(state.layerId)?.gen !== state.gen) return; // stop()/再 start() 済み
      state.index++;
      if (state.index >= state.def.cells.length) {
        if (!state.def.loop) {
          this.anims.delete(state.layerId);
          return;
        }
        state.index = 0;
      }
    }
  }

  /// [animstop]: 指定レイヤのループを止める（既に止まっていれば no-op）。
  stop(layerId: number): void {
    if (this.anims.delete(layerId)) {
      this.gens.set(layerId, (this.gens.get(layerId) ?? 0) + 1);
    }
  }

  /// [wa]: 待ち対象。ループ中（明示 stop されるか、loop=false のセル列を再生し終えるまで）は true。
  isRunning(layerId: number): boolean {
    return this.anims.has(layerId);
  }
}
