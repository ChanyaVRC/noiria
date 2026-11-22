//! X11 リクエストの直列化とリプライ/イベントのパース。sequence number
//! 照合は「送信ごとに単調増加するカウンタを持ち、リプライの下位16bitと突き合わせる」
//! という最小実装（X11 プロトコルはリプライが要求順に返ることが保証されている
//! ので、これで実用上十分）。

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use anyhow::{bail, Result};
use crossbeam_channel::Sender;

// --- オペコード ---
pub const OP_CREATE_WINDOW: u8 = 1;
pub const OP_MAP_WINDOW: u8 = 8;
pub const OP_CHANGE_PROPERTY: u8 = 18;
/// 本番ではフルスクリーン切替（root へ `_NET_WM_STATE` の ClientMessage）に使う。
/// テストは WM_DELETE_WINDOW や合成キーの自送にも使う。
pub const OP_SEND_EVENT: u8 = 25;
pub const OP_CREATE_GC: u8 = 55;
pub const OP_PUT_IMAGE: u8 = 72;
/// GetImage も同様に検証（テスト②のピクセル読み戻し、テスト⑤のスクリーン
/// ショット取得）専用。`run_windowed` は PutImage で書くだけで読み返さない。
#[cfg(test)]
pub const OP_GET_IMAGE: u8 = 73;
pub const OP_INTERN_ATOM: u8 = 16;
pub const OP_GET_KEYBOARD_MAPPING: u8 = 101;
/// テスト用（他クライアントとして自分のウィンドウを見つける）に追加した
/// opcode。計画の必須オペコード一覧には無いが、④の SendEvent クローズ
/// テストと⑤のスクリーンショット取得に必要なので追加した
/// （詳細は implementation-notes.md 参照）。
#[cfg(test)]
pub const OP_QUERY_TREE: u8 = 15;

// --- 定義済みアトム（InternAtom 不要） ---
pub const XA_STRING: u32 = 31;
pub const XA_ATOM: u32 = 4;
pub const XA_CARDINAL: u32 = 6;
pub const XA_WM_NAME: u32 = 39;
pub const XA_WM_NORMAL_HINTS: u32 = 40;
pub const XA_WM_SIZE_HINTS: u32 = 41;

// --- イベントマスク ---
pub const KEY_PRESS_MASK: u32 = 0x0000_0001;
pub const KEY_RELEASE_MASK: u32 = 0x0000_0002;
pub const BUTTON_PRESS_MASK: u32 = 0x0000_0004;
pub const POINTER_MOTION_MASK: u32 = 0x0000_0040;
pub const EXPOSURE_MASK: u32 = 0x0000_8000;
pub const STRUCTURE_NOTIFY_MASK: u32 = 0x0002_0000;
pub const FOCUS_CHANGE_MASK: u32 = 0x0020_0000;
/// ルートへ送る `_NET_WM_STATE` ClientMessage の宛先マスク（SubstructureRedirect | SubstructureNotify）。
pub const ROOT_WM_REQUEST_MASK: u32 = 0x0010_0000 | 0x0008_0000;

// --- イベントコード ---
pub const EV_KEY_PRESS: u8 = 2;
pub const EV_KEY_RELEASE: u8 = 3;
pub const EV_BUTTON_PRESS: u8 = 4;
pub const EV_MOTION_NOTIFY: u8 = 6;
pub const EV_FOCUS_OUT: u8 = 10;
pub const EV_EXPOSE: u8 = 12;
pub const EV_CONFIGURE_NOTIFY: u8 = 22;
pub const EV_CLIENT_MESSAGE: u8 = 33;

fn pad4(buf: &mut Vec<u8>) {
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

fn set_len_words(req: &mut [u8]) {
    let words = (req.len() / 4) as u16;
    req[2..4].copy_from_slice(&words.to_le_bytes());
}

/// CWBackPixel | CWEventMask の 2 値だけを使う CreateWindow。値リストは
/// 昇順ビットの順で並べる必要がある（back-pixel が event-mask より前）。
#[allow(clippy::too_many_arguments)]
pub fn req_create_window(
    wid: u32,
    parent: u32,
    x: i16,
    y: i16,
    w: u16,
    h: u16,
    depth: u8,
    visual: u32,
    back_pixel: u32,
    event_mask: u32,
) -> Vec<u8> {
    let mut req = Vec::with_capacity(40);
    req.push(OP_CREATE_WINDOW);
    req.push(depth);
    req.extend_from_slice(&0u16.to_le_bytes());
    req.extend_from_slice(&wid.to_le_bytes());
    req.extend_from_slice(&parent.to_le_bytes());
    req.extend_from_slice(&x.to_le_bytes());
    req.extend_from_slice(&y.to_le_bytes());
    req.extend_from_slice(&w.to_le_bytes());
    req.extend_from_slice(&h.to_le_bytes());
    req.extend_from_slice(&0u16.to_le_bytes()); // border-width
    req.extend_from_slice(&1u16.to_le_bytes()); // class = InputOutput
    req.extend_from_slice(&visual.to_le_bytes());
    let value_mask: u32 = 0x0000_0002 /* CWBackPixel */ | 0x0000_0800 /* CWEventMask */;
    req.extend_from_slice(&value_mask.to_le_bytes());
    req.extend_from_slice(&back_pixel.to_le_bytes());
    req.extend_from_slice(&event_mask.to_le_bytes());
    set_len_words(&mut req);
    req
}

pub fn req_map_window(window: u32) -> Vec<u8> {
    let mut req = vec![OP_MAP_WINDOW, 0, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    set_len_words(&mut req);
    req
}

pub fn req_create_gc(cid: u32, drawable: u32) -> Vec<u8> {
    let mut req = vec![OP_CREATE_GC, 0, 0, 0];
    req.extend_from_slice(&cid.to_le_bytes());
    req.extend_from_slice(&drawable.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // value-mask = 0
    set_len_words(&mut req);
    req
}

pub fn req_intern_atom(name: &str, only_if_exists: bool) -> Vec<u8> {
    let mut req = Vec::new();
    req.push(OP_INTERN_ATOM);
    req.push(u8::from(only_if_exists));
    req.extend_from_slice(&0u16.to_le_bytes());
    req.extend_from_slice(&(name.len() as u16).to_le_bytes());
    req.extend_from_slice(&0u16.to_le_bytes());
    req.extend_from_slice(name.as_bytes());
    pad4(&mut req);
    set_len_words(&mut req);
    req
}

pub fn parse_intern_atom_reply(reply: &[u8]) -> Result<u32> {
    if reply.len() < 12 {
        bail!("InternAtom リプライが短すぎます");
    }
    Ok(u32::from_le_bytes([
        reply[8], reply[9], reply[10], reply[11],
    ]))
}

/// `data` は既に `format`（8/16/32bit）単位でパックされたバイト列。
pub fn req_change_property(
    window: u32,
    property: u32,
    prop_type: u32,
    format: u8,
    data: &[u8],
    mode: u8,
) -> Vec<u8> {
    let elem_size = (format as usize / 8).max(1);
    let num_elements = (data.len() / elem_size) as u32;
    let mut req = Vec::with_capacity(24 + data.len());
    req.push(OP_CHANGE_PROPERTY);
    req.push(mode);
    req.extend_from_slice(&0u16.to_le_bytes());
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&property.to_le_bytes());
    req.extend_from_slice(&prop_type.to_le_bytes());
    req.push(format);
    req.extend_from_slice(&[0u8; 3]);
    req.extend_from_slice(&num_elements.to_le_bytes());
    req.extend_from_slice(data);
    pad4(&mut req);
    set_len_words(&mut req);
    req
}

/// ICCCM WM_SIZE_HINTS（18 x CARD32 = 72 バイト）。最小サイズだけ指定する（最大は無制限 =
/// リサイズ・最大化可。他のフィールドは全て 0）。
pub fn wm_size_hints_min(min_w: u32, min_h: u32) -> Vec<u8> {
    const P_MIN_SIZE: u32 = 1 << 4;
    let mut d = Vec::with_capacity(72);
    let mut push = |v: u32| d.extend_from_slice(&v.to_le_bytes());
    push(P_MIN_SIZE); // flags
    push(0); // x (legacy)
    push(0); // y (legacy)
    push(0); // width (legacy)
    push(0); // height (legacy)
    push(min_w); // min_width
    push(min_h); // min_height
    push(0); // max_width
    push(0); // max_height
    push(0); // width_inc
    push(0); // height_inc
    push(0); // min_aspect_num
    push(0); // min_aspect_den
    push(0); // max_aspect_num
    push(0); // max_aspect_den
    push(0); // base_width
    push(0); // base_height
    push(0); // win_gravity
    d
}

/// PutImage 1 バンド分（`format`=2 = ZPixmap 固定）。
pub fn req_put_image_band(
    drawable: u32,
    gc: u32,
    width: u16,
    height: u16,
    dst_y: i16,
    depth: u8,
    data: &[u8],
) -> Vec<u8> {
    let mut req = Vec::with_capacity(24 + data.len());
    req.push(OP_PUT_IMAGE);
    req.push(2); // ZPixmap
    req.extend_from_slice(&0u16.to_le_bytes());
    req.extend_from_slice(&drawable.to_le_bytes());
    req.extend_from_slice(&gc.to_le_bytes());
    req.extend_from_slice(&width.to_le_bytes());
    req.extend_from_slice(&height.to_le_bytes());
    req.extend_from_slice(&0i16.to_le_bytes()); // dst-x
    req.extend_from_slice(&dst_y.to_le_bytes());
    req.push(0); // left-pad
    req.push(depth);
    req.extend_from_slice(&0u16.to_le_bytes()); // unused
    req.extend_from_slice(data);
    pad4(&mut req);
    set_len_words(&mut req);
    req
}

/// フレーム全体（BGRX u32 列）を `max_request_bytes` に収まるよう帯分割して
/// PutImage を連続送出する。
pub fn put_image_banded(
    conn: &mut XConn,
    drawable: u32,
    gc: u32,
    pixels: &[u32],
    img_size: (u32, u32),
    depth: u8,
    max_request_bytes: u32,
) -> Result<()> {
    let (img_w, img_h) = img_size;
    const HEADER: u32 = 24;
    const SAFETY_MARGIN: u32 = 64;
    let bytes_per_row = img_w * 4;
    if bytes_per_row == 0 || img_h == 0 {
        return Ok(());
    }
    let usable = max_request_bytes.saturating_sub(HEADER + SAFETY_MARGIN);
    let rows_per_band = (usable / bytes_per_row).clamp(1, img_h);

    let mut y = 0u32;
    let mut row_bytes = Vec::with_capacity((rows_per_band * bytes_per_row) as usize);
    while y < img_h {
        let band_h = rows_per_band.min(img_h - y);
        let start = (y * img_w) as usize;
        let end = ((y + band_h) * img_w) as usize;
        row_bytes.clear();
        for px in &pixels[start..end] {
            row_bytes.extend_from_slice(&px.to_le_bytes());
        }
        let req = req_put_image_band(
            drawable,
            gc,
            img_w as u16,
            band_h as u16,
            y as i16,
            depth,
            &row_bytes,
        );
        conn.send_request(&req)?;
        y += band_h;
    }
    Ok(())
}

#[cfg(test)]
pub fn req_get_image(drawable: u32, x: i16, y: i16, w: u16, h: u16, plane_mask: u32) -> Vec<u8> {
    let mut req = Vec::with_capacity(20);
    req.push(OP_GET_IMAGE);
    req.push(2); // ZPixmap
    req.extend_from_slice(&5u16.to_le_bytes());
    req.extend_from_slice(&drawable.to_le_bytes());
    req.extend_from_slice(&x.to_le_bytes());
    req.extend_from_slice(&y.to_le_bytes());
    req.extend_from_slice(&w.to_le_bytes());
    req.extend_from_slice(&h.to_le_bytes());
    req.extend_from_slice(&plane_mask.to_le_bytes());
    req
}

/// GetImage リプライ（32 バイトヘッダ + データ）からピクセルデータ部分を返す。
#[cfg(test)]
pub fn parse_get_image_reply(reply: &[u8]) -> Result<&[u8]> {
    if reply.len() < 32 {
        bail!("GetImage リプライが短すぎます");
    }
    Ok(&reply[32..])
}

pub fn req_get_keyboard_mapping(first_keycode: u8, count: u8) -> Vec<u8> {
    vec![OP_GET_KEYBOARD_MAPPING, 0, 2, 0, first_keycode, count, 0, 0]
}

/// GetProperty（テスト用。`_NET_WM_ICON` の読み戻し検証に使う）。
#[cfg(test)]
pub const OP_GET_PROPERTY: u8 = 20;

#[cfg(test)]
pub fn req_get_property(
    window: u32,
    property: u32,
    prop_type: u32,
    long_offset: u32,
    long_length: u32,
) -> Vec<u8> {
    let mut req = Vec::with_capacity(24);
    req.push(OP_GET_PROPERTY);
    req.push(0); // delete = false
    req.extend_from_slice(&0u16.to_le_bytes());
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&property.to_le_bytes());
    req.extend_from_slice(&prop_type.to_le_bytes());
    req.extend_from_slice(&long_offset.to_le_bytes());
    req.extend_from_slice(&long_length.to_le_bytes());
    set_len_words(&mut req);
    req
}

/// GetProperty を送って format=32 の値を `u32` 列として読み戻す
/// （`_NET_WM_ICON` は CARDINAL=32bit 固定なのでこれで十分）。
#[cfg(test)]
pub fn get_property_u32s(
    conn: &mut XConn,
    window: u32,
    property: u32,
    prop_type: u32,
    max_len_words: u32,
) -> Result<Vec<u32>> {
    let req = req_get_property(window, property, prop_type, 0, max_len_words);
    let reply = conn.send_request_and_wait(&req)?;
    if reply.len() < 32 {
        bail!("GetProperty リプライが短すぎます");
    }
    let format = reply[1];
    if format != 32 {
        bail!(
            "GetProperty format が 32 ではありません（format={format}、プロパティ未設定の可能性）"
        );
    }
    let num_items = u32::from_le_bytes([reply[16], reply[17], reply[18], reply[19]]) as usize;
    let need = 32 + num_items * 4;
    if reply.len() < need {
        bail!(
            "GetProperty データ長が不足しています (want {need}, got {})",
            reply.len()
        );
    }
    let mut out = Vec::with_capacity(num_items);
    for chunk in reply[32..need].as_chunks::<4>().0 {
        out.push(u32::from_le_bytes(*chunk));
    }
    Ok(out)
}

#[cfg(test)]
pub fn req_query_tree(window: u32) -> Vec<u8> {
    let mut req = vec![OP_QUERY_TREE, 0, 2, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req
}

#[cfg(test)]
pub fn parse_query_tree_children(reply: &[u8]) -> Result<Vec<u32>> {
    if reply.len() < 32 {
        bail!("QueryTree リプライが短すぎます");
    }
    let num_children = u16::from_le_bytes([reply[16], reply[17]]) as usize;
    let need = 32 + num_children * 4;
    if reply.len() < need {
        bail!("QueryTree リプライのデータ長が不足しています");
    }
    let mut out = Vec::with_capacity(num_children);
    for chunk in reply[32..need].as_chunks::<4>().0 {
        out.push(u32::from_le_bytes(*chunk));
    }
    Ok(out)
}

/// GetImage して BGRX u32 列として返す（テスト用ヘルパ）。
#[cfg(test)]
pub fn get_image_pixels(
    conn: &mut XConn,
    drawable: u32,
    x: i16,
    y: i16,
    w: u16,
    h: u16,
) -> Result<Vec<u32>> {
    let req = req_get_image(drawable, x, y, w, h, 0xFFFF_FFFF);
    let reply = conn.send_request_and_wait(&req)?;
    let data = parse_get_image_reply(&reply)?;
    let need = (w as usize) * (h as usize) * 4;
    if data.len() < need {
        bail!(
            "GetImage データ長が不足しています (want {need}, got {})",
            data.len()
        );
    }
    let mut out = Vec::with_capacity((w as usize) * (h as usize));
    for chunk in data[..need].as_chunks::<4>().0 {
        out.push(u32::from_le_bytes(*chunk));
    }
    Ok(out)
}

pub fn req_send_event(
    destination: u32,
    propagate: bool,
    event_mask: u32,
    event_bytes: &[u8; 32],
) -> Vec<u8> {
    let mut req = Vec::with_capacity(44);
    req.push(OP_SEND_EVENT);
    req.push(u8::from(propagate));
    req.extend_from_slice(&11u16.to_le_bytes());
    req.extend_from_slice(&destination.to_le_bytes());
    req.extend_from_slice(&event_mask.to_le_bytes());
    req.extend_from_slice(event_bytes);
    req
}

/// ClientMessage（format=32）のワイヤバイト列を組み立てる。`data` は 5 個の CARD32。
pub fn build_client_message(window: u32, message_type: u32, data: [u32; 5]) -> [u8; 32] {
    let mut ev = [0u8; 32];
    ev[0] = EV_CLIENT_MESSAGE;
    ev[1] = 32;
    ev[4..8].copy_from_slice(&window.to_le_bytes());
    ev[8..12].copy_from_slice(&message_type.to_le_bytes());
    for (i, v) in data.iter().enumerate() {
        ev[12 + i * 4..16 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    ev
}

/// `data[0]` だけを使う ClientMessage（テスト④が WM_DELETE_WINDOW を自送するのに使う）。
#[cfg(test)]
pub fn build_client_message32(window: u32, message_type: u32, data0: u32) -> [u8; 32] {
    build_client_message(window, message_type, [data0, 0, 0, 0, 0])
}

// --- パケット読み取り ---

pub enum Packet {
    Error {
        code: u8,
        seq: u16,
        bad_value: u32,
        minor_opcode: u16,
        major_opcode: u8,
    },
    Reply {
        seq: u16,
        data: Vec<u8>,
    },
    Event {
        code: u8,
        data: [u8; 32],
    },
}

pub fn read_packet(r: &mut impl Read) -> Result<Packet> {
    let mut buf = [0u8; 32];
    r.read_exact(&mut buf)?;
    match buf[0] {
        0 => {
            let code = buf[1];
            let seq = u16::from_le_bytes([buf[2], buf[3]]);
            let bad_value = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
            let minor_opcode = u16::from_le_bytes([buf[8], buf[9]]);
            let major_opcode = buf[10];
            Ok(Packet::Error {
                code,
                seq,
                bad_value,
                minor_opcode,
                major_opcode,
            })
        }
        1 => {
            let seq = u16::from_le_bytes([buf[2], buf[3]]);
            let reply_len_words = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
            let mut data = buf.to_vec();
            if reply_len_words > 0 {
                let mut extra = vec![0u8; reply_len_words * 4];
                r.read_exact(&mut extra)?;
                data.extend_from_slice(&extra);
            }
            Ok(Packet::Reply { seq, data })
        }
        code => Ok(Packet::Event {
            code: code & 0x7F,
            data: buf,
        }),
    }
}

/// デコード済みイベント（`window.rs` のメインループ/リーダースレッドが使う）。
#[derive(Debug, Clone)]
pub enum XEvent {
    KeyPress {
        keycode: u8,
        state: u16,
        time: u32,
    },
    KeyRelease {
        keycode: u8,
        time: u32,
    },
    ButtonPress {
        button: u8,
        event_x: i16,
        event_y: i16,
    },
    MotionNotify {
        event_x: i16,
        event_y: i16,
    },
    Expose,
    FocusOut,
    ConfigureNotify {
        width: u16,
        height: u16,
    },
    ClientMessage {
        message_type: u32,
        data0: u32,
    },
    Other,
}

pub fn decode_event(code: u8, data: &[u8; 32]) -> XEvent {
    match code {
        EV_KEY_PRESS | EV_KEY_RELEASE => {
            let keycode = data[1];
            let time = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
            let state = u16::from_le_bytes([data[28], data[29]]);
            if code == EV_KEY_PRESS {
                XEvent::KeyPress {
                    keycode,
                    state,
                    time,
                }
            } else {
                XEvent::KeyRelease { keycode, time }
            }
        }
        EV_BUTTON_PRESS => {
            let button = data[1];
            let event_x = i16::from_le_bytes([data[24], data[25]]);
            let event_y = i16::from_le_bytes([data[26], data[27]]);
            XEvent::ButtonPress {
                button,
                event_x,
                event_y,
            }
        }
        EV_MOTION_NOTIFY => {
            let event_x = i16::from_le_bytes([data[24], data[25]]);
            let event_y = i16::from_le_bytes([data[26], data[27]]);
            XEvent::MotionNotify { event_x, event_y }
        }
        EV_EXPOSE => XEvent::Expose,
        EV_FOCUS_OUT => XEvent::FocusOut,
        EV_CONFIGURE_NOTIFY => XEvent::ConfigureNotify {
            width: u16::from_le_bytes([data[20], data[21]]),
            height: u16::from_le_bytes([data[22], data[23]]),
        },
        EV_CLIENT_MESSAGE => {
            let message_type = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
            let data0 = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);
            XEvent::ClientMessage {
                message_type,
                data0,
            }
        }
        _ => XEvent::Other,
    }
}

/// 書き込み専用ハンドル + sequence/resource-id カウンタ。セットアップ完了後は
/// このハンドルの `stream` は書き込み専用として使い、読み取りは別スレッド
/// （`reader_loop`）がクローンしたハンドルで行う。
pub struct XConn {
    pub stream: UnixStream,
    next_seq: u16,
    resource_id_base: u32,
    resource_id_mask: u32,
    next_counter: u32,
}

impl XConn {
    pub fn new(stream: UnixStream, resource_id_base: u32, resource_id_mask: u32) -> Self {
        Self {
            stream,
            next_seq: 1,
            resource_id_base,
            resource_id_mask,
            next_counter: 0,
        }
    }

    /// resource-id-mask は実サーバでは連続下位ビットである前提（Xorg/Xvfb は
    /// 常にそう）の最小実装。`base | (counter & mask)`。
    pub fn alloc_id(&mut self) -> u32 {
        let id = self.resource_id_base | (self.next_counter & self.resource_id_mask);
        self.next_counter += 1;
        id
    }

    /// リプライを期待しないリクエストを送る（sequence だけ進める）。
    pub fn send_request(&mut self, bytes: &[u8]) -> Result<u16> {
        self.stream.write_all(bytes)?;
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        Ok(seq)
    }

    /// リクエストを送り、対応する Reply（またはそれ以前に届いた Error）を
    /// 待つ。セットアップフェーズ（リーダースレッド起動前）専用。
    /// 想定外の Event が割り込んできた場合はログに残して読み飛ばす
    /// （このフェーズでは Expose 等が飛んでくる可能性は低いが、
    /// 万一発生しても致命傷にしないための最小限のフォールバック）。
    pub fn send_request_and_wait(&mut self, bytes: &[u8]) -> Result<Vec<u8>> {
        let want_seq = self.send_request(bytes)?;
        loop {
            match read_packet(&mut self.stream)? {
                Packet::Reply { seq, data } => {
                    if seq == want_seq {
                        return Ok(data);
                    }
                    log::warn!("X11: 想定外の reply seq={seq}（期待 {want_seq}）を読み飛ばします");
                }
                Packet::Error {
                    code,
                    seq,
                    bad_value,
                    minor_opcode,
                    major_opcode,
                } => {
                    bail!(
                        "X11 エラー応答: code={code} seq={seq} bad_value={bad_value} \
                         minor_opcode={minor_opcode} major_opcode={major_opcode}"
                    );
                }
                Packet::Event { code, .. } => {
                    log::debug!("X11: セットアップ中にイベント code={code} を受信、破棄します");
                }
            }
        }
    }
}

/// X ソケット専用リーダースレッドの本体。セットアップ完了後に呼ばれ、
/// 以降は Event のみを想定する（Reply は来ない設計）。Error（type=0）は
/// ログして継続する（致命的な接続断ではないため）。ソケット切断や
/// チャンネル送信失敗でループを抜ける。
pub fn reader_loop(mut stream: UnixStream, tx: Sender<XEvent>) {
    loop {
        match read_packet(&mut stream) {
            Ok(Packet::Event { code, data }) => {
                let ev = decode_event(code, &data);
                if tx.send(ev).is_err() {
                    break;
                }
            }
            Ok(Packet::Error {
                code,
                seq,
                bad_value,
                minor_opcode,
                major_opcode,
            }) => {
                log::warn!(
                    "X11 エラー応答（実行継続）: code={code} seq={seq} bad_value={bad_value} \
                     minor_opcode={minor_opcode} major_opcode={major_opcode}"
                );
            }
            Ok(Packet::Reply { seq, .. }) => {
                log::warn!("X11: メインループ中に想定外の reply seq={seq} を受信、無視します");
            }
            Err(e) => {
                log::info!("X11: 接続が切断されました（{e}）");
                break;
            }
        }
    }
}

#[cfg(test)]
mod decode_tests {
    use super::*;

    #[test]
    fn decodes_key_events_with_time_and_state() {
        let mut d = [0u8; 32];
        d[1] = 37;
        d[4..8].copy_from_slice(&123_456u32.to_le_bytes());
        d[28..30].copy_from_slice(&0x8u16.to_le_bytes());
        assert!(matches!(
            decode_event(EV_KEY_PRESS, &d),
            XEvent::KeyPress { keycode: 37, state: 0x8, time: 123_456 }
        ));
        assert!(matches!(
            decode_event(EV_KEY_RELEASE, &d),
            XEvent::KeyRelease { keycode: 37, time: 123_456 }
        ));
    }

    #[test]
    fn decodes_configure_notify_size_and_focus_out() {
        let mut d = [0u8; 32];
        d[20..22].copy_from_slice(&1920u16.to_le_bytes());
        d[22..24].copy_from_slice(&1080u16.to_le_bytes());
        assert!(matches!(
            decode_event(EV_CONFIGURE_NOTIFY, &d),
            XEvent::ConfigureNotify { width: 1920, height: 1080 }
        ));
        assert!(matches!(decode_event(EV_FOCUS_OUT, &d), XEvent::FocusOut));
    }

    #[test]
    fn client_message_builder_places_all_five_data_words() {
        let ev = build_client_message(0x11, 0x22, [2, 0xAA, 0, 1, 0]);
        assert_eq!(ev[0], EV_CLIENT_MESSAGE);
        assert_eq!(ev[1], 32);
        assert_eq!(u32::from_le_bytes(ev[4..8].try_into().unwrap()), 0x11);
        assert_eq!(u32::from_le_bytes(ev[8..12].try_into().unwrap()), 0x22);
        assert_eq!(u32::from_le_bytes(ev[12..16].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(ev[16..20].try_into().unwrap()), 0xAA);
        assert_eq!(u32::from_le_bytes(ev[24..28].try_into().unwrap()), 1);
    }
}
