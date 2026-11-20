//! DISPLAY 解析、Unix ドメインソケット接続、Xauthority 検索、X11 セットアップ
//! ハンドシェイク。TCP（ホスト名付き DISPLAY）は明確なエラーとし非対応。

use std::env;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

/// パース済み DISPLAY（ローカル X11 のみサポート）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplaySpec {
    pub display: u32,
    pub screen: u32,
}

/// DISPLAY 文字列 (`:N` / `:N.S` / `unix:N` / `unix:N.S`) を解析する。
/// ホスト名付き形式 (`host:N`, TCP 接続が要る) は明確なエラーにする。
pub fn parse_display(s: &str) -> Result<DisplaySpec> {
    let rest = if let Some(r) = s.strip_prefix("unix:") {
        r
    } else if let Some(r) = s.strip_prefix(':') {
        r
    } else {
        bail!(
            "DISPLAY '{s}' はホスト名付き（TCP 接続が必要）の形式のため非対応です。\
             ローカル ':N' または 'unix:N' 形式のみサポートします"
        );
    };
    let (display_str, screen_str) = match rest.split_once('.') {
        Some((d, s)) => (d, s),
        None => (rest, "0"),
    };
    let display = display_str
        .parse::<u32>()
        .with_context(|| format!("DISPLAY '{s}' のディスプレイ番号を解析できません"))?;
    let screen = screen_str
        .parse::<u32>()
        .with_context(|| format!("DISPLAY '{s}' のスクリーン番号を解析できません"))?;
    Ok(DisplaySpec { display, screen })
}

/// 実行時に使う DISPLAY 文字列を決定する。`NOIRIA_X11_TEST_DISPLAY`（テスト用
/// 上書き、Xvfb 等の隔離ディスプレイを指す）が設定されていればそれを優先し、
/// 通常はユーザーのデスクトップ DISPLAY を汚さない。未設定時は標準の `DISPLAY`
/// を使う。
pub fn resolve_display_env() -> Result<String> {
    if let Ok(v) = env::var("NOIRIA_X11_TEST_DISPLAY") {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    env::var("DISPLAY").context("環境変数 DISPLAY が設定されていません")
}

/// `/tmp/.X11-unix/X{display}` へ接続する。失敗時は同じディスプレイ番号の
/// abstract socket（Linux 固有、`\0/tmp/.X11-unix/X{display}`）を試す。
pub fn connect_socket(display: u32) -> Result<UnixStream> {
    let path = format!("/tmp/.X11-unix/X{display}");
    match UnixStream::connect(&path) {
        Ok(s) => return Ok(s),
        Err(e) => {
            log::debug!(
                "X11: unix socket {path} への接続に失敗（{e}）、abstract socket を試行します"
            );
        }
    }
    connect_abstract(display).with_context(|| {
        format!("X11 サーバへの接続に失敗しました（display={display}）。unix socket と abstract socket の両方を試行済み")
    })
}

#[cfg(target_os = "linux")]
fn connect_abstract(display: u32) -> Result<UnixStream> {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::SocketAddr;
    let name = format!("/tmp/.X11-unix/X{display}");
    let addr = SocketAddr::from_abstract_name(name.as_bytes())
        .context("abstract socket アドレスの構築に失敗しました")?;
    UnixStream::connect_addr(&addr).context("abstract socket への接続に失敗しました")
}

#[cfg(not(target_os = "linux"))]
fn connect_abstract(_display: u32) -> Result<UnixStream> {
    bail!("abstract socket は Linux 専用です")
}

const FAMILY_LOCAL: u16 = 256;
const FAMILY_WILD: u16 = 65535;

fn xauthority_path() -> Option<PathBuf> {
    if let Ok(p) = env::var("XAUTHORITY") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    let home = env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".Xauthority"))
}

fn local_hostname() -> String {
    if let Ok(h) = env::var("HOSTNAME") {
        if !h.is_empty() {
            return h;
        }
    }
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn read_u16be(r: &mut &[u8]) -> Option<u16> {
    if r.len() < 2 {
        return None;
    }
    let v = u16::from_be_bytes([r[0], r[1]]);
    *r = &r[2..];
    Some(v)
}

fn read_chunk(r: &mut &[u8], n: usize) -> Option<Vec<u8>> {
    if r.len() < n {
        return None;
    }
    let v = r[..n].to_vec();
    *r = &r[n..];
    Some(v)
}

struct XauthEntry {
    family: u16,
    address: Vec<u8>,
    display: Vec<u8>,
    name: Vec<u8>,
    data: Vec<u8>,
}

fn parse_one_entry(r: &mut &[u8]) -> Option<XauthEntry> {
    let family = read_u16be(r)?;
    let addr_len = read_u16be(r)? as usize;
    let address = read_chunk(r, addr_len)?;
    let disp_len = read_u16be(r)? as usize;
    let display = read_chunk(r, disp_len)?;
    let name_len = read_u16be(r)? as usize;
    let name = read_chunk(r, name_len)?;
    let data_len = read_u16be(r)? as usize;
    let data = read_chunk(r, data_len)?;
    Some(XauthEntry {
        family,
        address,
        display,
        name,
        data,
    })
}

/// `.Xauthority` から display に対応する MIT-MAGIC-COOKIE-1 を探す。見つから
/// なければ `None`（呼び出し側は空 auth でハンドシェイクを試みる）。
pub fn find_mit_cookie(display: u32) -> Option<(Vec<u8>, Vec<u8>)> {
    let path = xauthority_path()?;
    let data = std::fs::read(&path).ok()?;
    let mut r: &[u8] = &data;
    let hostname = local_hostname();
    let display_str = display.to_string();
    while !r.is_empty() {
        let Some(entry) = parse_one_entry(&mut r) else {
            break;
        };
        let host_matches = entry.family == FAMILY_WILD
            || (entry.family == FAMILY_LOCAL
                && (entry.address.is_empty() || entry.address == hostname.as_bytes()));
        let display_matches = entry.display.is_empty() || entry.display == display_str.as_bytes();
        if host_matches && display_matches && entry.name == b"MIT-MAGIC-COOKIE-1" {
            return Some((entry.name, entry.data));
        }
    }
    None
}

/// セットアップハンドシェイク成功時に得られる情報。
pub struct Setup {
    pub root: u32,
    /// 先頭スクリーンの寸法（px, mm）。初期ウィンドウサイズの決定（DPI 推定と収まる上限）に使う。
    pub screen_px: (u16, u16),
    pub screen_mm: (u16, u16),
    pub root_visual: u32,
    pub depth: u8,
    pub resource_id_base: u32,
    pub resource_id_mask: u32,
    /// バイト単位（ワイヤ上は 4 バイト単位で送られてくるので 4 倍済み）。
    pub max_request_length: u32,
    pub min_keycode: u8,
    pub max_keycode: u8,
}

fn pad4(buf: &mut Vec<u8>) {
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

fn read_u8(p: &mut &[u8]) -> Result<u8> {
    if p.is_empty() {
        bail!("X11 セットアップ応答が短すぎます");
    }
    let v = p[0];
    *p = &p[1..];
    Ok(v)
}
fn read_u16le(p: &mut &[u8]) -> Result<u16> {
    if p.len() < 2 {
        bail!("X11 セットアップ応答が短すぎます");
    }
    let v = u16::from_le_bytes([p[0], p[1]]);
    *p = &p[2..];
    Ok(v)
}
fn read_u32le(p: &mut &[u8]) -> Result<u32> {
    if p.len() < 4 {
        bail!("X11 セットアップ応答が短すぎます");
    }
    let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
    *p = &p[4..];
    Ok(v)
}
fn skip(p: &mut &[u8], n: usize) -> Result<()> {
    if p.len() < n {
        bail!("X11 セットアップ応答が短すぎます");
    }
    *p = &p[n..];
    Ok(())
}

const TRUE_COLOR: u8 = 4;
const EXPECTED_RED_MASK: u32 = 0x00FF_0000;

/// クライアント接続バイトオーダー = 'l' (LSBFirst)。以降このコネクション上の
/// 全リクエスト/リプライ/イベントは常にリトルエンディアンで符号化する
/// （サーバがどちらのネイティブバイト順でも、接続時に指定した順で変換して
/// 返してくる。X11 プロトコルの規定どおり）。
const BYTE_ORDER_LSB: u8 = b'l';

/// セットアップハンドシェイクを行い、TrueColor visual（depth 24/32、
/// red-mask==0xFF0000）を検証して返す。想定外の visual / image-byte-order は
/// 明確なエラーにする（バイトスワップは初版非対応）。
pub fn handshake(stream: &mut UnixStream, auth_name: &[u8], auth_data: &[u8]) -> Result<Setup> {
    let mut req = Vec::new();
    req.push(BYTE_ORDER_LSB);
    req.push(0);
    req.extend_from_slice(&11u16.to_le_bytes());
    req.extend_from_slice(&0u16.to_le_bytes());
    req.extend_from_slice(&(auth_name.len() as u16).to_le_bytes());
    req.extend_from_slice(&(auth_data.len() as u16).to_le_bytes());
    req.extend_from_slice(&0u16.to_le_bytes());
    req.extend_from_slice(auth_name);
    pad4(&mut req);
    req.extend_from_slice(auth_data);
    pad4(&mut req);
    stream
        .write_all(&req)
        .context("X11 セットアップリクエストの送信に失敗")?;

    let mut head = [0u8; 8];
    stream
        .read_exact(&mut head)
        .context("X11 セットアップ応答ヘッダの受信に失敗")?;
    let status = head[0];
    let reason_len = head[1] as usize;
    let add_len_words = u16::from_le_bytes([head[6], head[7]]) as usize;
    let mut rest = vec![0u8; add_len_words * 4];
    stream
        .read_exact(&mut rest)
        .context("X11 セットアップ応答本体の受信に失敗")?;

    match status {
        0 => {
            let reason = String::from_utf8_lossy(&rest[..reason_len.min(rest.len())]);
            bail!("X11 サーバへの接続を拒否されました: {reason}");
        }
        2 => bail!(
            "X11 サーバが追加認証 (Authenticate) を要求しました。\
             MIT-MAGIC-COOKIE-1 以外の認証方式は非対応です"
        ),
        1 => {}
        other => bail!("未知の X11 セットアップ応答ステータスです: {other}"),
    }

    let mut p: &[u8] = &rest;
    let _release_number = read_u32le(&mut p)?;
    let resource_id_base = read_u32le(&mut p)?;
    let resource_id_mask = read_u32le(&mut p)?;
    let _motion_buffer_size = read_u32le(&mut p)?;
    let vendor_len = read_u16le(&mut p)? as usize;
    let max_request_length_words = read_u16le(&mut p)? as u32;
    let num_screens = read_u8(&mut p)?;
    let num_formats = read_u8(&mut p)?;
    let image_byte_order = read_u8(&mut p)?;
    let _bitmap_bit_order = read_u8(&mut p)?;
    let _bitmap_scanline_unit = read_u8(&mut p)?;
    let _bitmap_scanline_pad = read_u8(&mut p)?;
    let min_keycode = read_u8(&mut p)?;
    let max_keycode = read_u8(&mut p)?;
    skip(&mut p, 4)?;
    skip(&mut p, vendor_len)?;
    // vendor 文字列は 4 バイト境界へパディングされる。
    let vendor_pad = (4 - vendor_len % 4) % 4;
    skip(&mut p, vendor_pad)?;
    for _ in 0..num_formats {
        skip(&mut p, 8)?; // FORMAT は 8 バイト固定
    }

    if image_byte_order != 0 {
        bail!(
            "この X サーバの image-byte-order は MSBFirst です（非対応、値={image_byte_order}）。\
             バイトスワップは初版非対応のためエラーとします"
        );
    }

    let mut chosen: Option<(u32, u8)> = None;
    let mut root = 0u32;
    let mut screen_px = (0u16, 0u16);
    let mut screen_mm = (0u16, 0u16);
    for i in 0..num_screens {
        let s_root = read_u32le(&mut p)?;
        skip(&mut p, 4 * 4)?; // default-colormap, white-pixel, black-pixel, current-input-masks
        let (w_px, h_px) = (read_u16le(&mut p)?, read_u16le(&mut p)?);
        let (w_mm, h_mm) = (read_u16le(&mut p)?, read_u16le(&mut p)?);
        skip(&mut p, 2 * 2)?; // min/max installed maps (2x CARD16)
        let _root_visual = read_u32le(&mut p)?;
        skip(&mut p, 2)?; // backing-stores, save-unders
        let _root_depth = read_u8(&mut p)?;
        let num_depths = read_u8(&mut p)?;
        if i == 0 {
            root = s_root;
            screen_px = (w_px, h_px);
            screen_mm = (w_mm, h_mm);
        }
        for _ in 0..num_depths {
            let depth_val = read_u8(&mut p)?;
            skip(&mut p, 1)?;
            let num_visuals = read_u16le(&mut p)?;
            skip(&mut p, 4)?;
            for _ in 0..num_visuals {
                let visual_id = read_u32le(&mut p)?;
                let class = read_u8(&mut p)?;
                skip(&mut p, 1)?; // bits-per-rgb-value
                skip(&mut p, 2)?; // colormap-entries
                let red_mask = read_u32le(&mut p)?;
                skip(&mut p, 4 + 4 + 4)?; // green-mask, blue-mask, pad
                if i == 0
                    && chosen.is_none()
                    && class == TRUE_COLOR
                    && (depth_val == 24 || depth_val == 32)
                    && red_mask == EXPECTED_RED_MASK
                {
                    chosen = Some((visual_id, depth_val));
                }
            }
        }
    }

    let (root_visual, depth) = chosen.context(
        "TrueColor visual (depth 24 または 32、red-mask==0xFF0000) が見つかりませんでした。\
         この X サーバの visual 構成には対応していません（スウィズル対応は初版スコープ外）",
    )?;

    Ok(Setup {
        root,
        screen_px,
        screen_mm,
        root_visual,
        depth,
        resource_id_base,
        resource_id_mask,
        max_request_length: max_request_length_words * 4,
        min_keycode,
        max_keycode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_display_local_no_screen() {
        let d = parse_display(":0").unwrap();
        assert_eq!(
            d,
            DisplaySpec {
                display: 0,
                screen: 0
            }
        );
    }

    #[test]
    fn parse_display_local_with_screen() {
        let d = parse_display(":1.2").unwrap();
        assert_eq!(
            d,
            DisplaySpec {
                display: 1,
                screen: 2
            }
        );
    }

    #[test]
    fn parse_display_unix_prefix() {
        let d = parse_display("unix:99").unwrap();
        assert_eq!(
            d,
            DisplaySpec {
                display: 99,
                screen: 0
            }
        );
        let d2 = parse_display("unix:3.1").unwrap();
        assert_eq!(
            d2,
            DisplaySpec {
                display: 3,
                screen: 1
            }
        );
    }

    #[test]
    fn parse_display_rejects_tcp_host() {
        assert!(parse_display("myhost:0").is_err());
        assert!(parse_display("192.168.1.1:0").is_err());
    }

    #[test]
    fn parse_display_rejects_garbage_number() {
        assert!(parse_display(":abc").is_err());
    }
}
