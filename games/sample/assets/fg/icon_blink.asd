; W5 デモ用アニメ定義（[animstart]/[wa]/[animstop] 対応表の「最小サブセット」記法）。
; 対応する記法:
;   - `;` 始まりの行 / 空行はコメント・無視
;   - `loop=true|false`（省略時 true）
;   - `セル番号,graphic,wait`（graphic は fg アセットからの相対パス、wait は ms）。
;     セル番号は識別用のラベルに過ぎず、再生順は常にこのファイルに書かれた順。
; seg=/module= 等の条件分岐を伴うフル .asd 構文は対象外（docs/kag-tag-compat.md 参照）。
loop=false
0,icon_blink_a.png,350
1,icon_blink_b.png,200
2,icon_blink_a.png,150
