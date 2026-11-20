//! W6 統合テスト: `games/sample` を実際に NPK1 pak へパックし、`GameDir::open` 経由で
//! ディレクトリ版と pak 版が完全に同じバイト列を返すこと、および pak の生バイト列に
//! 既知の平文（ファイル名・シナリオ本文）が含まれていないこと（秘匿性）を検証する。
//! `games/sample` はリポジトリに同梱の実ゲームなので存在しない環境は無い想定だが、
//! 念のため無ければスキップする（他の統合テストと同じ防御的パターン）。
//!
//! W8: 「バイト列が完全一致」のテスト (`*_byte_for_byte`/`*_hides_filenames_and_content`)
//! は `pack_dir(..., webp=false, flac=false)` で WebP/wav-flac 変換を無効にして呼ぶ
//! （`read_bytes` は生の格納バイト列を返す API なので、PNG→WebP 変換が有効だと画像エントリの
//! 生バイトは一致しなくなるのが期待通りの挙動のため）。WebP 変換を有効にした場合の等価性は
//! 別テスト `packed_sample_game_with_webp_matches_directory_by_decoded_pixels` で、
//! 生バイトではなく `load_image_rgba` のデコード結果（ピクセル）が一致することを検証する
//! （ロスレスなので完全一致するはず。`docs/implementation-notes.md` 2026-07-12 W8 参照）。
//!
//! W9: wav-flac (codec=2) はデコード時に**元 WAV バイト列を完全復元する**ため、webp とは
//! 違い `read_bytes` の生バイト比較でも変換 ON/OFF に関わらず等しくなるはず。それを直接
//! 検証するのが `packed_sample_game_with_flac_matches_directory_backend_byte_for_byte`
//! （`docs/implementation-notes.md` 2026-07-12 配布対応 W9 参照）。

use std::path::{Path, PathBuf};

use noiria_core::assets::GameDir;
use noiria_tools::pack::pack_dir;
use noiria_tools::testutil::TestRoot;

fn sample_game_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/sample")
}

fn collect_rel_paths(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap();
                let rel_str: String = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push(rel_str);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn packed_sample_game_matches_directory_backend_byte_for_byte() {
    let game_dir = sample_game_dir();
    if !game_dir.exists() {
        eprintln!(
            "[skip] games/sample が見つかりません: {}",
            game_dir.display()
        );
        return;
    }

    let key = noiria_core::pak::dev_key();
    let (pak_bytes, report) = pack_dir(&game_dir, &key, &[], false, false).expect("pack_dir");
    assert!(
        report.entry_count > 10,
        "エントリ数が少なすぎます: {}",
        report.entry_count
    );

    let tmp = TestRoot::new("pak_roundtrip");
    let tmp_pak = tmp.write("game.pak", &pak_bytes);

    let (dir_backend, dir_config) = GameDir::open(&game_dir).expect("GameDir::open (dir)");
    let (pak_backend, pak_config) = GameDir::open(&tmp_pak).expect("GameDir::open (pak)");
    assert_eq!(dir_config.title, pak_config.title);
    assert_eq!(dir_config.id, pak_config.id);

    let rel_paths = collect_rel_paths(&game_dir);
    assert!(
        rel_paths.len() >= report.entry_count,
        "パック対象より多いファイルが見つかった"
    );

    for rel in &rel_paths {
        let from_dir = dir_backend
            .read_bytes(rel)
            .unwrap_or_else(|e| panic!("dir backend read failed {rel}: {e:#}"));
        let from_pak = pak_backend
            .read_bytes(rel)
            .unwrap_or_else(|e| panic!("pak backend read failed {rel}: {e:#}"));
        assert_eq!(from_dir, from_pak, "内容が不一致: {rel}");
    }

}

/// W8: WebP 変換を有効にした場合、画像エントリは生バイトこそ変わるが
/// `load_image_rgba`（エンジンが実際に使う API）のデコード結果はディレクトリ版と
/// pak 版で完全に一致すること（ロスレスなので当然の帰結）を確認する。
/// 画像以外のエントリは従来通り生バイトが一致することも合わせて確認する。
#[test]
fn packed_sample_game_with_webp_matches_directory_by_decoded_pixels() {
    let game_dir = sample_game_dir();
    if !game_dir.exists() {
        eprintln!(
            "[skip] games/sample が見つかりません: {}",
            game_dir.display()
        );
        return;
    }

    let key = noiria_core::pak::dev_key();
    let (pak_bytes, report) = pack_dir(&game_dir, &key, &[], true, false).expect("pack_dir");
    assert!(
        report.webp_converted_count > 0,
        "sample の PNG は WebP の方が小さくなるはず"
    );

    let tmp = TestRoot::new("pak_roundtrip");
    let tmp_pak = tmp.write("game.pak", &pak_bytes);

    let (dir_backend, _) = GameDir::open(&game_dir).expect("GameDir::open (dir)");
    let (pak_backend, _) = GameDir::open(&tmp_pak).expect("GameDir::open (pak)");

    let mut image_checked = 0usize;
    for rel in &collect_rel_paths(&game_dir) {
        if rel.to_ascii_lowercase().ends_with(".png") {
            let (dw, dh, drgba) = dir_backend
                .load_image_rgba(rel)
                .unwrap_or_else(|e| panic!("dir 画像読込失敗 {rel}: {e:#}"));
            let (pw, ph, prgba) = pak_backend
                .load_image_rgba(rel)
                .unwrap_or_else(|e| panic!("pak 画像読込失敗 {rel}: {e:#}"));
            assert_eq!((dw, dh), (pw, ph), "サイズ不一致: {rel}");
            assert_eq!(drgba, prgba, "デコード結果（ピクセル）が不一致: {rel}");
            image_checked += 1;
        } else {
            let from_dir = dir_backend
                .read_bytes(rel)
                .unwrap_or_else(|e| panic!("dir backend read failed {rel}: {e:#}"));
            let from_pak = pak_backend
                .read_bytes(rel)
                .unwrap_or_else(|e| panic!("pak backend read failed {rel}: {e:#}"));
            assert_eq!(from_dir, from_pak, "内容が不一致: {rel}");
        }
    }
    assert!(
        image_checked > 0,
        "PNG が 1 つも見つかりませんでした（sample アセット構成が変わった?）"
    );

}

/// W9: wav-flac 変換を有効にした場合、`read_bytes`（エンジンが実際に使う生バイト API）が
/// ディレクトリ版と pak 版で**生バイトのまま**完全一致することを確認する。webp と異なり
/// wav-flac は `pak::Pak::read` が復号時に元 WAV バイト列を完全復元する設計のため、
/// `load_image_rgba` のような「デコード結果の比較」ではなく単純な生バイト比較で十分。
#[test]
fn packed_sample_game_with_flac_matches_directory_backend_byte_for_byte() {
    let game_dir = sample_game_dir();
    if !game_dir.exists() {
        eprintln!(
            "[skip] games/sample が見つかりません: {}",
            game_dir.display()
        );
        return;
    }

    let key = noiria_core::pak::dev_key();
    let (pak_bytes, report) = pack_dir(&game_dir, &key, &[], false, true).expect("pack_dir");
    assert!(
        report.flac_converted_count > 0,
        "sample の WAV は wav-flac の方が小さくなるはず"
    );

    let tmp = TestRoot::new("pak_roundtrip");
    let tmp_pak = tmp.write("game.pak", &pak_bytes);

    let (dir_backend, _) = GameDir::open(&game_dir).expect("GameDir::open (dir)");
    let (pak_backend, _) = GameDir::open(&tmp_pak).expect("GameDir::open (pak)");

    let mut wav_checked = 0usize;
    for rel in &collect_rel_paths(&game_dir) {
        let from_dir = dir_backend
            .read_bytes(rel)
            .unwrap_or_else(|e| panic!("dir backend read failed {rel}: {e:#}"));
        let from_pak = pak_backend
            .read_bytes(rel)
            .unwrap_or_else(|e| panic!("pak backend read failed {rel}: {e:#}"));
        assert_eq!(from_dir, from_pak, "内容が不一致: {rel}");
        if rel.to_ascii_lowercase().ends_with(".wav") {
            wav_checked += 1;
        }
    }
    assert!(
        wav_checked > 0,
        "WAV が 1 つも見つかりませんでした（sample アセット構成が変わった?）"
    );

}

#[test]
fn packed_sample_game_hides_filenames_and_content() {
    let game_dir = sample_game_dir();
    if !game_dir.exists() {
        eprintln!(
            "[skip] games/sample が見つかりません: {}",
            game_dir.display()
        );
        return;
    }

    let key = noiria_core::pak::dev_key();
    let (pak_bytes, _report) = pack_dir(&game_dir, &key, &[], false, false).expect("pack_dir");

    for needle in [
        &b"first.ks"[..],
        &b"alice_normal.png"[..],
        &b"scenario/first.ks"[..],
        &b"fg/alice_normal.png"[..],
    ] {
        assert!(
            !pak_bytes.windows(needle.len()).any(|w| w == needle),
            "pak の生バイト列に平文が含まれている: {:?}",
            String::from_utf8_lossy(needle)
        );
    }

    // シナリオ本文の一部（first.ks 内の既知の日本語断片）も平文で見えないことを確認する。
    let first_ks = std::fs::read(game_dir.join("scenario/first.ks")).expect("first.ks");
    // 十分に長い（衝突しにくい）部分文字列を探す: 先頭付近の非空行から 20 バイト程度切り出す。
    if let Some(chunk) = first_ks
        .windows(24)
        .find(|w| w.iter().all(|&b| b != 0) && !w.iter().all(|&b| b.is_ascii_whitespace()))
    {
        assert!(
            !pak_bytes.windows(chunk.len()).any(|w| w == chunk),
            "pak の生バイト列にシナリオ本文の断片が平文で含まれている"
        );
    }
}
