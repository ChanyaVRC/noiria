//! ページトランジション（クロスフェード / ユニバーサル=ルール画像）。
//! 進捗評価は純関数。開始/完了の状態遷移はアプリ側（レンダスレッド）が行う。

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::pixmap::ScrollDir;

#[derive(Clone, Debug)]
pub enum TransMethod {
    Crossfade,
    /// スクリーンサイズに拡縮済みの輝度プレーン + 曖昧さ
    Universal {
        rule: Arc<Vec<u8>>,
        vague: f32,
    },
    /// [trans method=scroll]（W5）: 新ページが `from` の辺からスライドインする。
    /// `stay=`（stayfore|staynone|staybase）は受理するが、このエンジンの合成は常に
    /// 画面全面を覆う不透明フレーム同士なので描画上は区別しない
    /// （`Pixmap::scroll_from` のコメント、`docs/kag-tag-compat.md` 参照）。
    Scroll {
        from: ScrollDir,
    },
}

#[derive(Clone, Debug)]
pub struct Transition {
    pub method: TransMethod,
    pub start: Instant,
    pub duration: Duration,
    /// op_trans_skip で立てる。次フレームで t=1 扱い。
    pub skip: bool,
}

impl Transition {
    pub fn progress(&self, now: Instant) -> f32 {
        if self.skip {
            return 1.0;
        }
        if self.duration.is_zero() {
            return 1.0;
        }
        (now.duration_since(self.start).as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0)
    }

    pub fn is_finished(&self, now: Instant) -> bool {
        self.progress(now) >= 1.0
    }
}

/// ルール画像（グレースケール想定の RGBA straight）をスクリーンサイズの輝度プレーンへ変換。
/// 最近傍でリサイズする（ルール画像は滑らかなグラデーションなので十分）。
pub fn rule_plane_from_rgba(
    src_w: u32,
    src_h: u32,
    rgba: &[u8],
    dst_w: u32,
    dst_h: u32,
) -> Vec<u8> {
    let mut out = Vec::with_capacity((dst_w * dst_h) as usize);
    for y in 0..dst_h {
        let sy = (y as u64 * src_h as u64 / dst_h as u64).min(src_h as u64 - 1) as usize;
        for x in 0..dst_w {
            let sx = (x as u64 * src_w as u64 / dst_w as u64).min(src_w as u64 - 1) as usize;
            let idx = (sy * src_w as usize + sx) * 4;
            // 輝度 = R (グレースケール前提)。カラー画像でも R を使う (krkrZ 同様)
            out.push(rgba[idx]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_clamps() {
        let t = Transition {
            method: TransMethod::Crossfade,
            start: Instant::now(),
            duration: Duration::from_millis(100),
            skip: false,
        };
        let p = t.progress(t.start + Duration::from_millis(50));
        assert!((p - 0.5).abs() < 0.01);
        assert_eq!(t.progress(t.start + Duration::from_secs(10)), 1.0);
    }

    #[test]
    fn skip_forces_completion() {
        let t = Transition {
            method: TransMethod::Crossfade,
            start: Instant::now(),
            duration: Duration::from_secs(100),
            skip: true,
        };
        assert!(t.is_finished(Instant::now()));
    }

    #[test]
    fn rule_resize() {
        // 2x1 のルール (0, 255) を 4x1 に拡大
        let rgba = [0u8, 0, 0, 255, 255, 255, 255, 255];
        let plane = rule_plane_from_rgba(2, 1, &rgba, 4, 1);
        assert_eq!(plane, vec![0, 0, 255, 255]);
    }
}
