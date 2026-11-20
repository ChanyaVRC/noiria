//! Windows (MSVC) 向け: exe 自体のアイコンリソースを埋め込む。
//!
//! `assets/icon.ico`（`crates/noiria-tools` の `gen-icon` バイナリで手続き生成、
//! `cargo run -p noiria-tools --bin gen-icon -- engine`）を読み、その ICONDIR /
//! ICONDIRENTRY を解析して各アイコン実体 (RT_ICON) + アイコングループ
//! (RT_GROUP_ICON) からなる `.res`（32-bit 新形式リソースファイル）を自前で
//! 組み立てる。外部クレート・`rc.exe` は一切使わない（決定論的なバイト列生成）。
//!
//! `.res` は `cargo:rustc-link-arg-bins` で MSVC のリンカ (`link.exe`) へ直接
//! 渡す（`link.exe` は `.res` をそのままリンク入力として受け付ける）。
//! `windows-gnu`/Linux ターゲットでは何もしない（GNU ターゲットは `.res` の
//! 直接リンクに対応する標準経路が無く、本ウェーブのスコープ外。
//! `docs/implementation-notes.md` 2026-07-13 参照）。

use std::env;
use std::fs;
use std::path::Path;

/// ICONDIR (6B) の直後に並ぶ ICONDIRENTRY (16B/個)。
struct IconDirEntry {
    width: u8,
    height: u8,
    color_count: u8,
    reserved: u8,
    planes: u16,
    bit_count: u16,
    data: Vec<u8>,
}

/// `.ico` バイト列を ICONDIR ヘッダの `count` に従って分解する。
fn parse_ico(bytes: &[u8]) -> Vec<IconDirEntry> {
    assert!(bytes.len() >= 6, "icon.ico が短すぎます");
    let reserved = u16::from_le_bytes([bytes[0], bytes[1]]);
    let rtype = u16::from_le_bytes([bytes[2], bytes[3]]);
    assert_eq!(reserved, 0, "icon.ico の ICONDIR.reserved が不正です");
    assert_eq!(
        rtype, 1,
        "icon.ico の ICONDIR.type が icon (1) ではありません"
    );
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;

    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = 6 + i * 16;
        let entry = &bytes[off..off + 16];
        let width = entry[0];
        let height = entry[1];
        let color_count = entry[2];
        let reserved = entry[3];
        let planes = u16::from_le_bytes([entry[4], entry[5]]);
        let bit_count = u16::from_le_bytes([entry[6], entry[7]]);
        let bytes_in_res = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let image_offset =
            u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        let data = bytes[image_offset..image_offset + bytes_in_res].to_vec();
        out.push(IconDirEntry {
            width,
            height,
            color_count,
            reserved,
            planes,
            bit_count,
            data,
        });
    }
    out
}

fn pad4(buf: &mut Vec<u8>) {
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

/// 32-bit `.res` の 1 リソースぶんのヘッダ + データを追記する。
/// Type/Name は常に序数（WORD 0xFFFF + WORD ID）で書く（本ファイルで使うのは
/// RT_ICON(3) / RT_GROUP_ICON(14) のみで文字列名は不要なため、ヘッダ長は
/// 常に 32 バイト固定になる）。
fn push_resource(out: &mut Vec<u8>, rtype: u16, name_id: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // DataSize
    out.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize（序数 Type/Name 固定で常に32）
    out.extend_from_slice(&0xFFFFu16.to_le_bytes()); // Type: 序数
    out.extend_from_slice(&rtype.to_le_bytes());
    out.extend_from_slice(&0xFFFFu16.to_le_bytes()); // Name: 序数
    out.extend_from_slice(&name_id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    out.extend_from_slice(&0x1030u16.to_le_bytes()); // MemoryFlags (MOVEABLE|PURE|DISCARDABLE)
    out.extend_from_slice(&0x0409u16.to_le_bytes()); // LanguageId (en-US)
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    pad4(out);
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

/// RT_ICON（実体）× N + RT_GROUP_ICON（NEWHEADER + RESDIR[N]、`.ico` の
/// ICONDIR/ICONDIRENTRY と同型だが末尾が imageOffset(DWORD) ではなく
/// アイコンリソース ID(WORD) になる）から成る `.res` バイト列を組み立てる。
fn build_res(entries: &[IconDirEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    // 先頭の「空リソース」ヘッダ: 32-bit 新形式 .res であることを示す決まり文句。
    push_resource(&mut out, 0, 0, &[]);

    for (i, e) in entries.iter().enumerate() {
        let icon_id = (i + 1) as u16;
        push_resource(&mut out, RT_ICON, icon_id, &e.data);
    }

    let mut group = Vec::new();
    group.extend_from_slice(&0u16.to_le_bytes()); // reserved
    group.extend_from_slice(&1u16.to_le_bytes()); // type = icon
    group.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (i, e) in entries.iter().enumerate() {
        group.push(e.width);
        group.push(e.height);
        group.push(e.color_count);
        group.push(e.reserved);
        group.extend_from_slice(&e.planes.to_le_bytes());
        group.extend_from_slice(&e.bit_count.to_le_bytes());
        group.extend_from_slice(&(e.data.len() as u32).to_le_bytes()); // bytesInRes
        group.extend_from_slice(&((i + 1) as u16).to_le_bytes()); // 対応する RT_ICON の ID
    }
    push_resource(&mut out, RT_GROUP_ICON, 1, &group);

    out
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        // Linux (`_NET_WM_ICON`) は実行時配線のみで exe リソースの概念が無い。
        // windows-gnu は `.res` 直接リンクの標準経路が無く本ウェーブのスコープ外。
        return;
    }

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR が未設定です");
    let ico_path = Path::new(&manifest_dir).join("assets").join("icon.ico");
    println!("cargo:rerun-if-changed={}", ico_path.display());

    let ico_bytes = fs::read(&ico_path)
        .unwrap_or_else(|e| panic!("icon.ico が読めません（{}）: {e}", ico_path.display()));
    let entries = parse_ico(&ico_bytes);
    assert!(
        !entries.is_empty(),
        "icon.ico にアイコンエントリがありません"
    );
    let res_bytes = build_res(&entries);

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR が未設定です");
    let res_path = Path::new(&out_dir).join("icon.res");
    fs::write(&res_path, &res_bytes).unwrap_or_else(|e| {
        panic!(
            "icon.res の書き出しに失敗しました（{}）: {e}",
            res_path.display()
        )
    });

    println!("cargo:rustc-link-arg-bins={}", res_path.display());
}
