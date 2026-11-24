//! 純粋なテキスト折返し・日本語禁則処理・水平整列計算。

use std::collections::{HashMap, HashSet};

use super::TextAlign;

/// [`locate`]/[`indent`]/[`endindent`] タグ用の制御トークン。`assign_lines` 内で
/// pen_x/line_i/indent_x を直接操作するダミー文字として扱う（グリフを持たず、
/// 対応する `Placement` の値は呼び出し側で使われない）。
#[derive(Clone, Copy, Debug)]
pub(super) enum CharControl {
    /// `[locate x= y=]`: pen_x を x へ、y 指定時は line_i も上書きする。
    Locate { pen_x: f32, line_i: Option<usize> },
    /// `[indent]`: 現在の pen_x を以降の折返し行の行頭 x として記憶する。
    IndentSet,
    /// `[endindent]`: 折返し行頭 x を 0 に戻す。
    IndentClear,
}

/// [`assign_lines`] への 1 文字分の入力（実フォントから独立させた最小情報）。
#[derive(Clone, Copy)]
pub(super) struct LineChar {
    pub(super) ch: char,
    pub(super) advance_px: f32,
    pub(super) is_break: bool,
    pub(super) control: Option<CharControl>,
}

/// [`assign_lines`] が 1 文字ごとに返す配置結果。
#[derive(Clone, Copy)]
pub(super) struct Placement {
    pub(super) line_i: usize,
    pub(super) pen_x: f32,
}

/// 折返し＋禁則処理の中核。`chars` と 1:1 対応する `Placement` 列を返す
/// （`'\n'` の要素にもダミーの `Placement` を返すが、呼び出し側では使わない）。
///
/// 実フォントのメトリクス（グリフ形状・ラスタライズ）から独立した純粋関数なので、
/// 禁則処理のロジック単体を合成した advance 値でユニットテストできる。
pub(super) fn assign_lines(chars: &[LineChar], box_w: f32) -> Vec<Placement> {
    let mut out: Vec<Placement> = Vec::with_capacity(chars.len());
    let mut pen_x = 0.0f32;
    let mut line_i = 0usize;
    let mut line_start_idx = 0usize;
    // [indent] で記憶した「以降の折返し行の行頭 x」。[endindent] で 0.0 に戻る。
    let mut indent_x = 0.0f32;

    for &LineChar {
        ch,
        advance_px,
        is_break,
        control,
    } in chars
    {
        if let Some(ctl) = control {
            // [locate]/[indent]/[endindent] 制御トークン。グリフを持たないダミー文字として
            // 1 要素分だけ消費し、pen_x/line_i/indent_x を直接操作する。
            match ctl {
                CharControl::Locate {
                    pen_x: new_pen_x,
                    line_i: new_line_i,
                } => {
                    if let Some(li) = new_line_i {
                        line_i = li;
                    }
                    pen_x = new_pen_x;
                    line_start_idx = out.len() + 1;
                }
                CharControl::IndentSet => indent_x = pen_x,
                CharControl::IndentClear => indent_x = 0.0,
            }
            out.push(Placement { line_i, pen_x });
            continue;
        }

        if is_break {
            line_i += 1;
            pen_x = indent_x;
            line_start_idx = out.len() + 1;
            out.push(Placement { line_i, pen_x });
            continue;
        }

        // 折返しは常にグリフ単位（旧 cosmic-text 実装の Wrap::Glyph に相当）。
        let would_exceed = pen_x > 0.0 && (pen_x + advance_px) > box_w;
        if would_exceed {
            if is_line_start_forbidden(ch) {
                // ぶら下げ: 行頭禁則文字は折り返さず現在行の末尾に置く
                // （box_w を若干超えることを許容する）。
            } else {
                // 道連れ: 現在行の末尾から連続する行末禁則文字（開き括弧等）を数え、
                // それらをまとめて新しい行へ送ってから折り返す。
                let mut pull_from = out.len();
                while pull_from > line_start_idx && is_line_end_forbidden(chars[pull_from - 1].ch) {
                    pull_from -= 1;
                }
                line_i += 1;
                let mut new_pen = indent_x;
                for (i, place) in out.iter_mut().enumerate().skip(pull_from) {
                    place.line_i = line_i;
                    place.pen_x = new_pen;
                    new_pen += chars[i].advance_px;
                }
                pen_x = new_pen;
                line_start_idx = pull_from;
            }
        }

        out.push(Placement { line_i, pen_x });
        pen_x += advance_px;
    }

    out
}

pub(super) fn alignment_offsets(
    chars: &[LineChar],
    placements: &[Placement],
    box_w: f32,
    align: TextAlign,
) -> HashMap<usize, f32> {
    if align == TextAlign::Left {
        return HashMap::new();
    }

    let mut widths = HashMap::<usize, f32>::new();
    let mut located_lines = HashSet::<usize>::new();
    for (ch, place) in chars.iter().zip(placements) {
        if matches!(ch.control, Some(CharControl::Locate { .. })) {
            located_lines.insert(place.line_i);
        }
        if ch.is_break || ch.control.is_some() {
            continue;
        }
        let end_x = place.pen_x + ch.advance_px;
        let width = widths.entry(place.line_i).or_insert(0.0);
        *width = width.max(end_x);
    }

    widths
        .into_iter()
        .map(|(line_i, width)| {
            let remaining = (box_w - width).max(0.0);
            let offset = if located_lines.contains(&line_i) {
                0.0
            } else {
                match align {
                    TextAlign::Left => 0.0,
                    TextAlign::Center => remaining / 2.0,
                    TextAlign::Right => remaining,
                }
            };
            (line_i, offset)
        })
        .collect()
}

/// 行頭禁則文字（この文字の直前で折り返す代わりに前行末へぶら下げる）。
/// JIS X 4051 の全規則ではなく、句読点・閉じ括弧類・小書き仮名・長音符・繰返し記号・
/// 一部の約物という実用上の最小セット。
pub(super) fn is_line_start_forbidden(c: char) -> bool {
    matches!(
        c,
        '。' | '、'
            | '．'
            | ','
            | '，'
            | '.'
            | '」'
            | '』'
            | '】'
            | '｝'
            | '］'
            | '〉'
            | '》'
            | '〕'
            | '’'
            | '”'
            | '〟'
            | '＞'
            | '≫'
            | '）'
            | ')'
            | 'ぁ'
            | 'ぃ'
            | 'ぅ'
            | 'ぇ'
            | 'ぉ'
            | 'っ'
            | 'ゃ'
            | 'ゅ'
            | 'ょ'
            | 'ゎ'
            | 'ァ'
            | 'ィ'
            | 'ゥ'
            | 'ェ'
            | 'ォ'
            | 'ッ'
            | 'ャ'
            | 'ュ'
            | 'ョ'
            | 'ヮ'
            | '゛'
            | '゜'
            | 'ー'
            | '～'
            | '!'
            | '?'
            | '！'
            | '？'
            | ':'
            | ';'
            | '：'
            | '；'
            | '・'
            | '…'
            | '‥'
    )
}

/// 行末禁則文字（この文字が行末に来る場合、続く文字と一緒に次行へ道連れにする）。
pub(super) fn is_line_end_forbidden(c: char) -> bool {
    matches!(
        c,
        '（' | '('
            | '「'
            | '『'
            | '【'
            | '｛'
            | '［'
            | '〈'
            | '《'
            | '〔'
            | '‘'
            | '“'
            | '〝'
            | '＜'
            | '≪'
    )
}
