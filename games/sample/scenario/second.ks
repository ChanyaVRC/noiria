; Noiria サンプルシナリオ その2 — マクロ・条件分岐・セーブのデモ

*second_start|街にて
[macro name=chat]
[name text=アリス]
[endmacro]

[chat]二つ目のシナリオファイルです。[l][r]
[chat]この名前欄プレートの表示、実はさっきのマクロが担当しています。[p]
[cm]
; ---- W2 音声タグのデモ（一時停止・再開・フェード・SE バッファ） ----
[pausebgm]
[wait time=250]
[resumebgm]
[fadepausebgm time=200]
[wb canskip=false]
[resumebgm]
[fadebgm volume=60 time=300]
[entersebuf buf=2]
[fadeinse storage=click.wav time=150 buf=2]
[wf buf=2]
[fadese volume=40 time=150 buf=2]
[fadeoutse time=150 buf=2]
[setbgmstop storage=second.ks target=*second_start]
[clearbgmstop]
[chat]（BGM の一時停止→再開と、SE バッファのフェードを実演しました）[l][r]
[if exp="(f.affection ?? 0) > 0"]
[chat]さっき「気に入った」と言ってくれましたよね。えへへ。[l][p]
[else]
[chat]まだ様子見でしたよね。がんばります。[l][p]
[endif]
[cm]
[chat]F5 でクイックセーブ、F9 でロードができます。[l][r]
[chat]S でスキップ、A でオートモード、H で履歴が開きますよ。[p]
[cm]
; ---- W4 デモ: input（自作テキスト入力モーダル） ----
; どこをクリックしても現在の内容で確定する設計（--autotest の固定クリック座標でも
; ハングしない。docs/implementation-notes.md 参照）。
[input title="W4 デモ" prompt="お名前を入力してください（クリックで確定）" initial="ゲスト" name="f.demoName"]
[chat]入力欄のデモにお答えいただきありがとうございます（input、W4）。[l][r]
[cm]
[chat]デモはここまでです。最初に戻りますか？[r]
[link target=*restart]　▶ 最初から[endlink][r]
[link target=*bye]　▶ 終了する[endlink][r]
[s]

*restart|最初へ
[cm]
[name]
[fadeoutbgm time=600]
[wait time=300]
[jump storage=title.ks target=*title]

*bye|おわり
[cm]
[chat]遊んでくれてありがとうございました！[l][p]
[name]
[fadeoutbgm time=800]
[wb canskip=false]
[wl canskip=false]
[wait time=900]
[close]
