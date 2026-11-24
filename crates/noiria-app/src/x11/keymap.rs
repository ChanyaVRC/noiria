//! GetKeyboardMapping の結果を keysym テーブルとして保持し、win32.rs と同じ
//! 特殊キー集合 + Latin-1/Unicode keysym の印字文字へ変換する。IME 非対応
//! （win32.rs の WM_CHAR 最小互換）。

use anyhow::{bail, Result};

pub struct Keymap {
    min_keycode: u8,
    keysyms_per_keycode: u8,
    keysyms: Vec<u32>,
}

impl Keymap {
    /// `keycode` の `column`（0=無シフト、1=シフト）に対応する keysym。
    /// column が keysyms_per_keycode を超える場合は最後の列にクランプする。
    /// 値が 0（NoSymbol）で column>=1 の場合は列 0 にフォールバックする
    /// （X11 の慣例: シフト列が未定義なら無シフト値を使う）。
    pub fn keysym(&self, keycode: u8, column: usize) -> u32 {
        let k = self.keysyms_per_keycode as usize;
        if k == 0 || keycode < self.min_keycode {
            return 0;
        }
        let row = (keycode - self.min_keycode) as usize;
        let base = row * k;
        let col = column.min(k - 1);
        let primary = self.keysyms.get(base + col).copied().unwrap_or(0);
        if primary == 0 && col != 0 {
            self.keysyms.get(base).copied().unwrap_or(0)
        } else {
            primary
        }
    }
}

/// GetKeyboardMapping リプライ（`proto::XConn::send_request_and_wait` が返す
/// 生バイト列、32 バイトヘッダ込み）をパースする。
pub fn parse_reply(min_keycode: u8, reply: &[u8]) -> Result<Keymap> {
    if reply.len() < 32 {
        bail!("GetKeyboardMapping リプライが短すぎます");
    }
    let keysyms_per_keycode = reply[1];
    let reply_len_words = u32::from_le_bytes([reply[4], reply[5], reply[6], reply[7]]) as usize;
    let n = reply_len_words * 4;
    if reply.len() < 32 + n {
        bail!("GetKeyboardMapping リプライのデータ長が不足しています");
    }
    let data = &reply[32..32 + n];
    let mut keysyms = Vec::with_capacity(n / 4);
    for chunk in data.as_chunks::<4>().0 {
        keysyms.push(u32::from_le_bytes(*chunk));
    }
    Ok(Keymap {
        min_keycode,
        keysyms_per_keycode,
        keysyms,
    })
}

const KS_BACKSPACE: u32 = 0xFF08;
const KS_TAB: u32 = 0xFF09;
pub const KS_RETURN: u32 = 0xFF0D;
const KS_ESCAPE: u32 = 0xFF1B;
const KS_CONTROL_L: u32 = 0xFFE3;
const KS_CONTROL_R: u32 = 0xFFE4;
const KS_SPACE: u32 = 0x0020;
const KS_HOME: u32 = 0xFF50;
const KS_LEFT: u32 = 0xFF51;
const KS_UP: u32 = 0xFF52;
const KS_RIGHT: u32 = 0xFF53;
const KS_DOWN: u32 = 0xFF54;
const KS_PAGE_UP: u32 = 0xFF55;
const KS_PAGE_DOWN: u32 = 0xFF56;
const KS_END: u32 = 0xFF57;
const KS_DELETE: u32 = 0xFFFF;
const KS_F1: u32 = 0xFFBE;
const KS_F2: u32 = 0xFFBF;
const KS_F3: u32 = 0xFFC0;
const KS_F4: u32 = 0xFFC1;
const KS_F5: u32 = 0xFFC2;
const KS_F6: u32 = 0xFFC3;
const KS_F7: u32 = 0xFFC4;
const KS_F8: u32 = 0xFFC5;
const KS_F9: u32 = 0xFFC6;
const KS_F10: u32 = 0xFFC7;
pub const KS_F11: u32 = 0xFFC8;
const KS_F12: u32 = 0xFFC9;
const KS_SHIFT_L: u32 = 0xFFE1;
const KS_SHIFT_R: u32 = 0xFFE2;
const UNICODE_KEYSYM_BASE: u32 = 0x0100_0000;

/// keysym → `InputEvent.key` 文字列。win32.rs の WM_KEYDOWN/WM_CHAR 送出集合
/// と同じ結果を返す。非対応 keysym は None。
pub fn to_input_key(keysym: u32) -> Option<String> {
    match keysym {
        KS_BACKSPACE => Some("Backspace".to_string()),
        KS_TAB => Some("Tab".to_string()),
        KS_RETURN => Some("Enter".to_string()),
        KS_ESCAPE => Some("Escape".to_string()),
        KS_CONTROL_L | KS_CONTROL_R => Some("Control".to_string()),
        KS_SHIFT_L | KS_SHIFT_R => Some("Shift".to_string()),
        KS_SPACE => Some(" ".to_string()),
        KS_HOME => Some("Home".to_string()),
        KS_LEFT => Some("ArrowLeft".to_string()),
        KS_UP => Some("ArrowUp".to_string()),
        KS_RIGHT => Some("ArrowRight".to_string()),
        KS_DOWN => Some("ArrowDown".to_string()),
        KS_PAGE_UP => Some("PageUp".to_string()),
        KS_PAGE_DOWN => Some("PageDown".to_string()),
        KS_END => Some("End".to_string()),
        KS_DELETE => Some("Delete".to_string()),
        KS_F1 => Some("F1".to_string()),
        KS_F2 => Some("F2".to_string()),
        KS_F3 => Some("F3".to_string()),
        KS_F4 => Some("F4".to_string()),
        KS_F5 => Some("F5".to_string()),
        KS_F6 => Some("F6".to_string()),
        KS_F7 => Some("F7".to_string()),
        KS_F8 => Some("F8".to_string()),
        KS_F9 => Some("F9".to_string()),
        KS_F10 => Some("F10".to_string()),
        // F11 はホスト（フルスクリーン切替）が消費するためスクリプトへ渡さない。
        KS_F12 => Some("F12".to_string()),
        // Latin-1 印字文字（space は上で処理済みなので 0x21 から）。
        0x0021..=0x00FF => char::from_u32(keysym)
            .filter(|c| !c.is_control())
            .map(|c| c.to_string()),
        // Unicode keysym（0x01000000 + codepoint）。
        k if k >= UNICODE_KEYSYM_BASE && k - UNICODE_KEYSYM_BASE <= 0x10FFFF => {
            char::from_u32(k - UNICODE_KEYSYM_BASE)
                .filter(|c| !c.is_control())
                .map(|c| c.to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GetKeyboardMapping リプライのワイヤバイト列を固定フィクスチャとして
    /// 組み立てる: min_keycode=8, keysyms_per_keycode=2, keycode 8 →
    /// ('a'=0x61, 'A'=0x41)、keycode 9 → (Return=0xFF0D, NoSymbol=0)。
    fn fixture_reply() -> Vec<u8> {
        let keysyms_per_keycode: u8 = 2;
        let entries: [u32; 4] = [0x0061, 0x0041, KS_RETURN, 0];
        let mut reply = vec![0u8; 32];
        reply[0] = 1; // Reply
        reply[1] = keysyms_per_keycode;
        let reply_len_words = (entries.len() * 4 / 4) as u32; // 4 keysyms * 4B / 4 = 4
        reply[4..8].copy_from_slice(&reply_len_words.to_le_bytes());
        for v in entries {
            reply.extend_from_slice(&v.to_le_bytes());
        }
        reply
    }

    #[test]
    fn parse_reply_fixed_wire_bytes() {
        let km = parse_reply(8, &fixture_reply()).unwrap();
        assert_eq!(km.keysym(8, 0), 0x0061);
        assert_eq!(km.keysym(8, 1), 0x0041);
        assert_eq!(km.keysym(9, 0), KS_RETURN);
        // keycode 9 col1 は NoSymbol(0) なので col0 にフォールバック。
        assert_eq!(km.keysym(9, 1), KS_RETURN);
    }

    #[test]
    fn to_input_key_special_set_matches_win32() {
        let cases = [
            (KS_BACKSPACE, "Backspace"),
            (KS_TAB, "Tab"),
            (KS_RETURN, "Enter"),
            (KS_ESCAPE, "Escape"),
            (KS_CONTROL_L, "Control"),
            (KS_CONTROL_R, "Control"),
            (KS_SHIFT_L, "Shift"),
            (KS_SHIFT_R, "Shift"),
            (KS_SPACE, " "),
            (KS_HOME, "Home"),
            (KS_LEFT, "ArrowLeft"),
            (KS_UP, "ArrowUp"),
            (KS_RIGHT, "ArrowRight"),
            (KS_DOWN, "ArrowDown"),
            (KS_PAGE_UP, "PageUp"),
            (KS_PAGE_DOWN, "PageDown"),
            (KS_END, "End"),
            (KS_DELETE, "Delete"),
            (KS_F1, "F1"),
            (KS_F2, "F2"),
            (KS_F3, "F3"),
            (KS_F4, "F4"),
            (KS_F5, "F5"),
            (KS_F6, "F6"),
            (KS_F7, "F7"),
            (KS_F8, "F8"),
            (KS_F9, "F9"),
            (KS_F10, "F10"),
            (KS_F12, "F12"),
        ];
        for (ks, expected) in cases {
            assert_eq!(to_input_key(ks).as_deref(), Some(expected), "ks=0x{ks:X}");
        }
        // F11 はホスト（フルスクリーン切替）が消費するのでスクリプトへは渡さない。
        assert_eq!(to_input_key(KS_F11), None);
    }

    #[test]
    fn to_input_key_printable_latin1_and_unicode() {
        assert_eq!(to_input_key(0x0061).as_deref(), Some("a")); // 'a'
        assert_eq!(
            to_input_key(UNICODE_KEYSYM_BASE + 0x3042).as_deref(),
            Some("あ")
        );
    }
}
