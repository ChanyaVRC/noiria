//! メッセージウィンドウの寸法検証ツール。
//! 既定スタイル (KagPlayer の msgCfg) で N 行のテキストを実レイアウトし、
//! グリフの実際の上下端が箱に収まるかを数値で確認する。
//! 実行: cargo run -p noiria-core --example measure_msgbox

use noiria_core::text::{TextEngine, TextSegment, TextStyle};

fn main() {
    // KagPlayer の computeMsgCfg() と同じ式 (runtime/kag/player.ts):
    //   height = ceil(fontSize × MSG_LINE_HEIGHT × rows) + marginY×2
    //   top    = screenH − MSG_BOTTOM_GAP − height
    let (font_size, line_height, rows) = (26.0f32, 1.6f32, 4i32);
    let (margin_y, bottom_gap, screen_h) = (18i32, 24i32, 720i32);
    let win_h = (font_size * line_height * rows as f32).ceil() as i32 + margin_y * 2;
    let top = screen_h - bottom_gap - win_h;
    println!(
        "計算式: height = ceil({font_size}×{line_height}×{rows}) + {margin_y}×2 = {win_h}px, top = {top}px (下辺 {}px)",
        top + win_h
    );

    let style = TextStyle {
        font_size,
        line_height,
        box_w: 1200 - 60,
        box_h: (win_h - margin_y * 2) as u32,
        outline_width: 2,
        ..Default::default()
    };
    let line_advance = style.font_size * style.line_height;

    let mut engine = TextEngine::new();
    // 実シナリオ相当の 4 行 + 収まらないはずの 5 行目
    let text = "【アリス】\n笑顔にもなれますよ。\n画面を揺らしてみますね。\nボイス（の代わりのビープ）はこちら。\n5行目はカリングされるはず";
    let raster = engine.raster_segments(
        &[TextSegment {
            text: text.into(),
            ..Default::default()
        }],
        &style,
    );

    println!("フォント行送り (fontSize×lineHeight) = {line_advance:.1}px");
    println!(
        "箱の内側高さ boxH = {}px (ウィンドウ {}px - マージン {}px×2)",
        style.box_h, win_h, margin_y
    );
    println!();

    let max_line = raster.glyphs.iter().map(|g| g.line_i).max().unwrap_or(0);
    for line in 0..=max_line {
        let (mut top, mut bottom) = (i32::MAX, i32::MIN);
        let mut count = 0;
        for g in raster.glyphs.iter().filter(|g| g.line_i == line) {
            top = top.min(g.y);
            bottom = bottom.max(g.y + g.pixmap.height as i32);
            count += 1;
        }
        if count == 0 {
            println!("行{line}: (グリフなし = カリング済み)");
            continue;
        }
        let fits = bottom <= style.box_h as i32;
        println!(
            "行{line}: グリフ実寸 y = {top}..{bottom}px  (箱内側 {}px に{}）",
            style.box_h,
            if fits {
                "収まる ✓"
            } else {
                "収まらない ✗"
            }
        );
    }
    println!();
    let total_lines_drawn = raster
        .glyphs
        .iter()
        .map(|g| g.line_i)
        .collect::<std::collections::HashSet<_>>()
        .len();
    println!("描画された行数: {total_lines_drawn} (期待: 4。5行目はカリング)");
}
