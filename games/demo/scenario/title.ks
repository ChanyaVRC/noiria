; 星待ちの放課後 — タイトル / 共通マクロ / おまけ
;
; レイヤ規約: layer=0 はひなた、layer=1 はしおり。立ち絵は 500x900、top=60。
;   1 人のとき left=390、2 人のとき ひなた left=140 / しおり left=640。
; 変数: f.hinata / f.shiori = 好感度。sf.endHinata / sf.endShiori / sf.endNormal = 到達済みエンド。

*title|タイトル
[title name="星待ちの放課後"]

; ---- 共通マクロ（起動時に必ずここを通るので、ロード後も定義済み） ----
[macro name=np]
[p][cm]
[endmacro]

[macro name=nar]
[name]
[endmacro]

[macro name=me]
[name text=蒼太]
[endmacro]

[macro name=hina]
[image layer=0 storage=%face left=%x|390 top=60]
[name text=ひなた]
[endmacro]

[macro name=shio]
[image layer=1 storage=%face left=%x|390 top=60]
[name text=しおり]
[endmacro]

; 2 人並びのときの表情替え
[macro name=hina2]
[image layer=0 storage=%face left=140 top=60]
[name text=ひなた]
[endmacro]

[macro name=shio2]
[image layer=1 storage=%face left=640 top=60]
[name text=しおり]
[endmacro]

; 立ち絵の登場・退場（クロスフェード）
[macro name=enter]
[backlay]
[image layer=%layer page=back storage=%face left=%x|390 top=60]
[trans method=crossfade time=350]
[wt]
[endmacro]

[macro name=leave]
[backlay]
[freeimage layer=%layer page=back]
[trans method=crossfade time=350]
[wt]
[endmacro]

; 場面転換: 立ち絵を全員下げて背景を差し替える
[macro name=scene]
[cm]
[name]
[backlay]
[image layer=base page=back storage=%bg]
[freeimage layer=0 page=back]
[freeimage layer=1 page=back]
[trans method=%method|crossfade rule=%rule|wipe.png vague=96 time=%time|900]
[wt]
[endmacro]

; イベント CG: ウィンドウを隠して 1 クリック分じっくり見せる
[macro name=showcg]
[cm]
[name]
[backlay]
[image layer=base page=back storage=%storage]
[freeimage layer=0 page=back]
[freeimage layer=1 page=back]
[trans method=universal rule=stars.png vague=128 time=1600]
[wt]
[layopt layer=message0 visible=false]
[waitclick]
[layopt layer=message0 visible=true]
[endmacro]

; ---- タイトル画面 ----
; 専用タイトル画面（main.ts の titleScreen）。項目は START / CONTINUE / GALLERY / CONFIG / EXIT。
*menu|タイトルメニュー
[cm]
[name]
[sysbar visible=false]
[autosave enabled=false]
[history output=false]
[layopt layer=message0 visible=false]
[freeimage layer=0]
[freeimage layer=1]
[image layer=base storage=title.png visible=true]
[playbgm storage=title.wav volume=70]
[titlescreen]

*start_game|ゲーム開始
[cm]
[layopt layer=message0 visible=true]
[sysbar visible=true]
[autosave enabled=true]
[history output=true]
[fadeoutbgm time=500]
[wait time=550]
[eval exp="f.hinata = 0; f.shiori = 0"]
[jump storage=ch1.ks target=*ch1_morning]
