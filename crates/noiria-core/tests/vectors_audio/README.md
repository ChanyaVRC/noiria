# Vorbis デコーダ テストベクタ

`codec::ogg` / `codec::vorbis` の自作デコーダを検証するための実 OGG/Vorbis ファイル。
`crates/noiria-core/tests/vorbis_crosscheck.rs` が使用する。

V1 では参照デコーダ (rodio 0.22。バックエンドは `symphonia-codec-vorbis` +
`symphonia-format-ogg`) を実時間でクロスチェックしていた（相関 1.0・最大誤差 1e-7
オーダーで一致することを確認済み）。V2 で rodio を依存から完全削除したため、現在は
そのクロスチェック結果を要約したスナップショット定数に対する回帰テストに切り替えている
（詳細・再生成手順は `vorbis_crosscheck.rs` 冒頭のコメント参照）。

このディレクトリは noiria リポジトリ自体が git 管理外のため `.gitignore` の心配はないが、
再取得手順を明記しておく（ファイルが失われた場合、下記コマンドで再取得できる。取得できない
場合はクロスチェックテストは `eprintln!` で skip 通知を出して継続する設計）。

## ファイル一覧

| ファイル | チャンネル | サンプルレート | 取得元 | ライセンス |
| --- | --- | --- | --- | --- |
| `mono1_a_boy2.ogg` | mono (1ch) | 44100Hz | [File:A boy2.ogg](https://commons.wikimedia.org/wiki/File:A_boy2.ogg) (Wikimedia Commons) | 出典ページ参照 (音声発音サンプル) |
| `mono2_abashed_en.ogg` | mono (1ch) | 44100Hz | [File:Abashed-en.ogg](https://commons.wikimedia.org/wiki/File:Abashed-en.ogg) (Wikimedia Commons) | 出典ページ参照 (英語発音サンプル、Wiktionary 由来) |
| `stereo1_kalmykia_anthem_trimmed.ogg` | stereo (2ch) | 48000Hz | [File:Kalmykia anthem (short instrumental).ogg](https://commons.wikimedia.org/wiki/File:Kalmykia_anthem_(short_instrumental).ogg) (Wikimedia Commons) を Ogg ページ境界で先頭 約260KB に切り詰めたもの | 出典ページ参照 (オーケストラ演奏、パブリックドメイン/CC 表記は出典ページ参照) |

## 再取得手順

```bash
# mono サンプル (そのまま使用)
curl -sL -o mono1_a_boy2.ogg \
  "https://upload.wikimedia.org/wikipedia/commons/7/73/A_boy2.ogg"
curl -sL -o mono2_abashed_en.ogg \
  "https://upload.wikimedia.org/wikipedia/commons/f/f6/Abashed-en.ogg"

# stereo サンプル (フルサイズ ~670KB をダウンロードし、Ogg ページ境界で
# 約260KB に切り詰める。数百 KB 以下という方針とデコード時間 (自作 iMDCT は
# O(n^2) の素朴な実装) の両方を抑えるため)
curl -sL -o /tmp/kalmykia_full.ogg \
  "https://upload.wikimedia.org/wikipedia/commons/b/b9/Kalmykia_anthem_%28short_instrumental%29.ogg"
python3 - <<'EOF'
with open('/tmp/kalmykia_full.ogg', 'rb') as f:
    data = f.read()

def iter_pages(data):
    pos = 0
    while pos < len(data):
        assert data[pos:pos+4] == b'OggS'
        seg_count = data[pos+26]
        seg_table = data[pos+27:pos+27+seg_count]
        total = 27 + seg_count + sum(seg_table)
        yield pos, total
        pos += total

target = 260 * 1024
end = 0
for pos, total in iter_pages(data):
    if end + total > target and end > 0:
        break
    end += total

with open('stereo1_kalmykia_anthem_trimmed.ogg', 'wb') as f:
    f.write(data[:end])
EOF
```

## 備考

- いずれも Xiph 公式サンプル (media.xiph.org) はディレクトリリスティングが提供されておらず
  直接の小容量 Vorbis サンプルを特定できなかったため、Wikimedia Commons の CC0/PD 系
  音声ファイルから選定した。
- stereo ファイルは Ogg ページ単位で末尾を切り詰めているだけなので、識別/コメント/セットアップ
  ヘッダおよび先頭の音声パケット群は完全な形で残っており、正常にデコードできる
  (最後のパケットが半端に切れる場合はデコーダ側がパケット終端 (EOP) を「切り詰められた
  ものとして採用する」設計になっている)。
- テストが対象ファイルを見つけられない場合 (削除された・再取得に失敗した等) は
  `tests/vorbis_crosscheck.rs` がテストを skip する (`eprintln!` で通知し、即 return)。
