// Noiria サンプルゲーム — エントリポイント。
// KAG 風シナリオを再生する。TypeScript なので独自の演出コードも書ける。

import { KagPlayer } from "noiria:kag";
import { sys } from "noiria:api";

sys.log("Noiria サンプルを起動します");

const player = new KagPlayer({
  charaLayerCount: 3,
  autoWaitMs: 1800,
});

await player.run("title.ks");
