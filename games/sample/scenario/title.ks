; タイトル画面
*title|タイトル
[title name="Noiria サンプル"]
[cm]
[image layer=base storage=street.png visible=true]
[image layer=0 visible=false]
[playbgm storage=main.wav volume=70]
Noiria サンプル　— ▶ の項目をクリックで選択 —[r]
[link target=*start_game]　▶ はじめから[endlink][r]
[link exp="sys.menu('load')"]　▶ つづきから（ロード）[endlink][r]
（本文はクリックで送り / S=スキップ A=オート H=履歴 Esc=セーブ）[s]

*start_game|ゲーム開始
[cm]
[fadeoutbgm time=400]
[wait time=450]
[jump storage=first.ks target=*start]
