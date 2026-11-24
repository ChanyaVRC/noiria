//! Win32 / X11 のウィンドウ実装が共有する、OS 非依存の入力まわりのロジック。
//! 純粋な状態機械だけを置き、単体テストで挙動を固定する。

#![cfg_attr(windows, allow(dead_code))]

use std::time::{Duration, Instant};

/// ウィンドウの閉じる操作でスクリプトへ送る `kind:"key"` の key 名。
pub(crate) const KEY_WINDOW_CLOSE: &str = "WindowClose";

/// スクリプトが `WindowClose` を受けて終了要求を返すまでホストが待つ猶予。超えたら強制終了
/// （スクリプトのハング・無視でウィンドウが閉じられなくなるのを防ぐ）。
pub(crate) const CLOSE_GRACE: Duration = Duration::from_secs(2);

/// 閉じる操作の猶予タイマー。
#[derive(Default)]
pub(crate) struct CloseGate {
    deadline: Option<Instant>,
}

impl CloseGate {
    /// 閉じる操作を受けた。`script_alive` が偽（スクリプトスレッドが既に終了し入力受信口が
    /// 閉じている）なら待つ意味が無いので `true`（即時に破棄してよい）を返す。
    /// 猶予中の再要求は猶予を延長しない。
    pub(crate) fn request(&mut self, now: Instant, script_alive: bool) -> bool {
        if !script_alive {
            return true;
        }
        self.deadline.get_or_insert(now + CLOSE_GRACE);
        false
    }

    pub(crate) fn expired(&self, now: Instant) -> bool {
        self.deadline.is_some_and(|d| now >= d)
    }

    /// 猶予の残り（メッセージループの待機タイムアウトに畳み込む）。
    pub(crate) fn remaining(&self, now: Instant) -> Option<Duration> {
        self.deadline.map(|d| d.saturating_duration_since(now))
    }
}

/// 押下中のキー（keyup を送るべきもの）の記録。フォーカスを失うとキーアップが届かないため、
/// 失焦時に押しっぱなし分の keyup をまとめて出せるようにする。
#[derive(Default)]
pub(crate) struct KeyTracker {
    held: Vec<String>,
}

impl KeyTracker {
    pub(crate) fn press(&mut self, key: &str) {
        if !self.held.iter().any(|k| k == key) {
            self.held.push(key.to_string());
        }
    }

    /// 押下中だった場合だけ `true`（対応する keydown を送っていない keyup を作らないため）。
    pub(crate) fn release(&mut self, key: &str) -> bool {
        let before = self.held.len();
        self.held.retain(|k| k != key);
        self.held.len() != before
    }

    pub(crate) fn release_all(&mut self) -> Vec<String> {
        std::mem::take(&mut self.held)
    }
}

/// `keyup` を送る対象キーか（修飾キー・決定キー・スペース）。`key` はスクリプトへ渡す表記。
pub(crate) fn is_keyup_tracked(key: &str) -> bool {
    matches!(key, "Control" | "Shift" | "Enter" | " ")
}

/// X11 のキーイベント（簡略表現）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyEv {
    Press { keycode: u8, time: u32 },
    Release { keycode: u8, time: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyAct {
    Down(u8),
    Up(u8),
}

/// X11 コアプロトコルはオートリピートを「同一時刻の KeyRelease + KeyPress の対」で配送する
/// （XKB の detectable auto-repeat を要求しない限り）。この対を検出して両方を捨て、
/// 物理的な初回押下とキーアップだけを通す。
///
/// KeyRelease は対の相手が来るか確定するまで保留する。呼び出し側は、キューが空になったら
/// 少し待って（`has_pending`）それでも後続が無ければ `flush` で確定させること。
#[derive(Default)]
pub(crate) struct RepeatFilter {
    pending: Option<(u8, u32)>,
}

impl RepeatFilter {
    pub(crate) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn feed(&mut self, ev: KeyEv) -> Vec<KeyAct> {
        let mut out = Vec::new();
        match ev {
            KeyEv::Release { keycode, time } => {
                if let Some((k, _)) = self.pending.take() {
                    out.push(KeyAct::Up(k));
                }
                self.pending = Some((keycode, time));
            }
            KeyEv::Press { keycode, time } => match self.pending.take() {
                Some((k, t)) if k == keycode && t == time => {}
                Some((k, _)) => {
                    out.push(KeyAct::Up(k));
                    out.push(KeyAct::Down(keycode));
                }
                None => out.push(KeyAct::Down(keycode)),
            },
        }
        out
    }

    /// 保留中の KeyRelease を本物のキーアップとして確定する。
    pub(crate) fn flush(&mut self) -> Option<KeyAct> {
        self.pending.take().map(|(k, _)| KeyAct::Up(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_gate_forwards_then_expires_after_grace() {
        let t0 = Instant::now();
        let mut g = CloseGate::default();
        assert!(!g.expired(t0));
        assert!(g.remaining(t0).is_none());
        assert!(!g.request(t0, true), "スクリプト生存中は即時破棄しない");
        assert!(!g.expired(t0 + Duration::from_millis(1999)));
        assert!(g.expired(t0 + CLOSE_GRACE));
        assert_eq!(
            g.remaining(t0 + Duration::from_millis(500)),
            Some(Duration::from_millis(1500))
        );
    }

    #[test]
    fn close_gate_dead_script_closes_immediately() {
        let mut g = CloseGate::default();
        assert!(g.request(Instant::now(), false));
    }

    #[test]
    fn close_gate_second_request_does_not_extend_grace() {
        let t0 = Instant::now();
        let mut g = CloseGate::default();
        g.request(t0, true);
        g.request(t0 + Duration::from_millis(1500), true);
        assert!(g.expired(t0 + CLOSE_GRACE));
    }

    #[test]
    fn key_tracker_releases_everything_held() {
        let mut t = KeyTracker::default();
        t.press("Control");
        t.press("Control");
        t.press("Shift");
        assert!(t.release("Shift"));
        assert!(!t.release("Shift"), "押していないキーの release は false");
        assert_eq!(t.release_all(), vec!["Control".to_string()]);
        assert!(t.release_all().is_empty());
    }

    #[test]
    fn keyup_targets() {
        for k in ["Control", "Shift", "Enter", " "] {
            assert!(is_keyup_tracked(k));
        }
        assert!(!is_keyup_tracked("a"));
    }

    #[test]
    fn x11_autorepeat_pairs_are_dropped_but_first_press_and_last_release_pass() {
        let mut f = RepeatFilter::default();
        // 初回押下
        assert_eq!(
            f.feed(KeyEv::Press {
                keycode: 37,
                time: 1000
            }),
            vec![KeyAct::Down(37)]
        );
        // オートリピート 3 回（同時刻の Release+Press 対）
        for t in [1500u32, 1530, 1560] {
            assert!(f
                .feed(KeyEv::Release {
                    keycode: 37,
                    time: t
                })
                .is_empty());
            assert!(f.has_pending());
            assert!(f
                .feed(KeyEv::Press {
                    keycode: 37,
                    time: t
                })
                .is_empty());
            assert!(!f.has_pending());
        }
        // 本物のキーアップ（後続の対が来ないので flush で確定）
        assert!(f
            .feed(KeyEv::Release {
                keycode: 37,
                time: 2000
            })
            .is_empty());
        assert_eq!(f.flush(), Some(KeyAct::Up(37)));
        assert_eq!(f.flush(), None);
    }

    #[test]
    fn x11_release_then_different_key_press_is_not_a_repeat() {
        let mut f = RepeatFilter::default();
        assert!(f
            .feed(KeyEv::Release {
                keycode: 37,
                time: 100
            })
            .is_empty());
        assert_eq!(
            f.feed(KeyEv::Press {
                keycode: 50,
                time: 100
            }),
            vec![KeyAct::Up(37), KeyAct::Down(50)]
        );
    }

    #[test]
    fn x11_release_then_same_key_press_at_different_time_is_a_real_tap() {
        let mut f = RepeatFilter::default();
        assert!(f
            .feed(KeyEv::Release {
                keycode: 37,
                time: 100
            })
            .is_empty());
        assert_eq!(
            f.feed(KeyEv::Press {
                keycode: 37,
                time: 180
            }),
            vec![KeyAct::Up(37), KeyAct::Down(37)]
        );
    }

    #[test]
    fn x11_consecutive_releases_flush_the_earlier_one() {
        let mut f = RepeatFilter::default();
        assert!(f
            .feed(KeyEv::Release {
                keycode: 37,
                time: 100
            })
            .is_empty());
        assert_eq!(
            f.feed(KeyEv::Release {
                keycode: 50,
                time: 101
            }),
            vec![KeyAct::Up(37)]
        );
        assert_eq!(f.flush(), Some(KeyAct::Up(50)));
    }
}
