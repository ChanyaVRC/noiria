# リファレンス BGM 出典一覧

`analyze_bgm.rs`（`crates/noiria-tools/examples/analyze_bgm.rs`）が数値解析にのみ使う
実制作の CC0/パブリックドメイン BGM。**ゲームアセットとしては使用しない**（解析用途限定）。
このディレクトリはリポジトリの git 管理外だが、慣習として `.gitignore` を置いてある。

OGG 系は自作 Vorbis デコーダの逆 MDCT が O(n^2) の素朴な実装
（`crates/noiria-core/src/codec/vorbis/mdct.rs`）であるため、解析の妨げにならない
デコード時間に収めるべく、`tests/vectors_audio/README.md` と同じ手法（Ogg ページ境界での
先頭切り詰め、約230KB≈20〜30秒相当）でトリミング済み。切り詰めても識別/コメント/セット
アップヘッダと先頭の音声パケット群は完全な形で残るため正常にデコードできる。WAV は
デコードが O(n) なのでトリミングしていない（フルサイズのまま）。

| ファイル | 原題 | 作者 | 取得元 | ライセンス | 原寸 | 収録形 |
| --- | --- | --- | --- | --- | --- | --- |
| `ambient_relaxing_isaiah658_trim.ogg` | Ambient Relaxing Loop | isaiah658 | https://opengameart.org/content/ambient-relaxing-loop (file: https://opengameart.org/sites/default/files/Ambient-Loop-isaiah658_0.ogg) | CC0 | 1.3MB | 先頭 約230KB にトリミング |
| `relaxtune1_bgm_trim.ogg` | BGM Music (relaxtune1) | Tozan | https://opengameart.org/content/bgm-music (file: https://opengameart.org/sites/default/files/relaxtune1_0.ogg) | CC0 | 1.1MB | 先頭 約230KB にトリミング |
| `mystical_town_symphony_trim.ogg` | Mystical RPG Maker Town Theme | symphony | https://opengameart.org/content/mystical-rpg-maker-town-theme (file: https://opengameart.org/sites/default/files/symphony%20-%20mystical%20town%2001_1.ogg) | CC0 | 1.8MB | 先頭 約230KB にトリミング |
| `dark_shrine_loop_qubodup_trim.ogg` | Dark Shrine Loop (yd の "Shrine" のリミックス) | qubodup | https://opengameart.org/content/dark-shrine-loop (file: https://opengameart.org/sites/default/files/qubodup-yd-DarkShrineLoop-OpenGameArt.ogg) | CC0 | 678KB | 先頭 約230KB にトリミング |
| `emotional_piano_loop_extenz.wav` | Emotional piano loop (Piano Loop) | extenz | https://opengameart.org/content/emotional-piano-loop (file: https://opengameart.org/sites/default/files/Piano%20Loop.wav) | CC0 | 4.9MB | フルサイズ（WAVはO(n)デコードなのでトリミング不要） |

## 選定方針

- 全て CC0（パブリックドメイン相当、帰属表示不要）。各ページで CC0 バッジを目視確認済み。
- 穏やかな日常系/アンビエント系を優先しつつ、比較の解像度を上げるため用途の異なる曲も
  一部含めた: アンビエントループ2曲、町/RPGテーマ（オーケストラ調）1曲、ダークアンビエント
  1曲、ソロピアノ1曲。いずれも自作3曲（温かい日常曲・静かめの曲・緊張感のある曲）と
  ジャンル的に近い「ゲーム向けループ/BGM」カテゴリ。
- MP3 のみ提供の候補（Relaxing.mp3 / TownTheme.mp3 / Forest_Ambience.mp3 等）は自作
  デコーダで読めないため除外した。
