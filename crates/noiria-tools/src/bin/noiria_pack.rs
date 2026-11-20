//! noiria-pack — ゲームディレクトリを NPK1 暗号化 pak にまとめる CLI。
//! 使い方:
//!   noiria-pack --game games/sample --out target/sample.pak [--key <64桁hex>] [--exclude <glob>]... [--no-default-excludes] [--no-project-excludes]
//!                [--list] [--list-excluded] [--no-webp] [--no-flac]
//! `--exclude` は glob（`*` `**` `?`。`/` なしはファイル名/ディレクトリ名、`/` ありは相対パス全体に照合）。
//! 既定でドットファイル/ディレクトリと `Thumbs.db` `desktop.ini` `*~` `*.bak` `*.tmp`、および
//! `--out` の pak 自身（`--game` 内にある場合）を除外する（`--no-default-excludes` で既定除外を切る）。
//! `game.json` の `"pack": {"exclude": ["<glob>", ...]}` も除外に加わる（既定 + game.json + `--exclude` の和集合。
//! `--no-project-excludes` で game.json の指定だけを無視）。`game.json` と `main` のファイルは除外できない（エラー）。
//! `--list` で pak に入ったエントリの一覧（パス・元サイズ・変換）、`--list-excluded` で除外されたパスと規則を表示する。
//! `--key` 省略時は `noiria_core::pak::dev_key()`（開発用固定鍵、非秘密）を使う。
//! `--no-webp` を指定しない限り、`.png` エントリは WebP ロスレスへ変換して元 PNG より
//! 小さければ差し替える（W8）。`--no-flac` を指定しない限り、`.wav` エントリは
//! wav-flac (pak codec=2) へ可逆変換して元 WAV より小さければ差し替える（W9）。

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use noiria_tools::pack::{excluded_summary, pack_dir_with, PackOptions};

struct Args {
    game: PathBuf,
    out: PathBuf,
    key_hex: Option<String>,
    exclude: Vec<String>,
    default_excludes: bool,
    project_excludes: bool,
    list: bool,
    list_excluded: bool,
    webp: bool,
    flac: bool,
}

fn parse_args() -> Result<Args> {
    let mut game: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut key_hex: Option<String> = None;
    let mut exclude: Vec<String> = Vec::new();
    let mut default_excludes = true;
    let mut project_excludes = true;
    let mut list = false;
    let mut list_excluded = false;
    let mut webp = true;
    let mut flac = true;

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--game" => {
                game = Some(argv.get(i + 1).context("--game には値が必要です")?.into());
                i += 1;
            }
            "--out" => {
                out = Some(argv.get(i + 1).context("--out には値が必要です")?.into());
                i += 1;
            }
            "--key" => {
                key_hex = Some(argv.get(i + 1).context("--key には値が必要です")?.clone());
                i += 1;
            }
            "--exclude" => {
                exclude.push(
                    argv.get(i + 1)
                        .context("--exclude には値が必要です")?
                        .clone(),
                );
                i += 1;
            }
            "--no-default-excludes" => {
                default_excludes = false;
            }
            "--no-project-excludes" => {
                project_excludes = false;
            }
            "--list-excluded" => {
                list_excluded = true;
            }
            "--list" => {
                list = true;
            }
            "--no-webp" => {
                webp = false;
            }
            "--no-flac" => {
                flac = false;
            }
            other => bail!("不明な引数です: {other}"),
        }
        i += 1;
    }

    Ok(Args {
        game: game.context("--game <ゲームディレクトリ> は必須です")?,
        out: out.context("--out <出力先.pak> は必須です")?,
        key_hex,
        exclude,
        default_excludes,
        project_excludes,
        list,
        list_excluded,
        webp,
        flac,
    })
}

fn parse_hex_key(hex: &str) -> Result<[u8; 32]> {
    // ASCII 16 進数字だけを許す（`from_str_radix` は "+1" を受理し、非 ASCII を含むと
    // バイト境界スライスで panic するため先に検査する）。
    anyhow::ensure!(
        hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "--key は 64 桁の16進数（0-9a-fA-F、32バイト）である必要があります"
    );
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)?;
    }
    Ok(key)
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let key = match &args.key_hex {
        Some(hex) => parse_hex_key(hex)?,
        None => noiria_core::pak::dev_key(),
    };

    println!("pack 開始: {}", args.game.display());
    let mut opts = PackOptions::new(args.webp, args.flac);
    opts.exclude = args.exclude.clone();
    opts.default_excludes = args.default_excludes;
    opts.project_excludes = args.project_excludes;
    opts.skip_paths = vec![args.out.clone()];
    let packed = pack_dir_with(&args.game, &key, &opts)
        .with_context(|| format!("pack に失敗しました: {}", args.game.display()))?;
    for w in &packed.warnings {
        eprintln!("警告: {w}");
    }
    let (bytes, report) = (&packed.pak, &packed.report);

    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.out, bytes)
        .with_context(|| format!("pak の書き出しに失敗しました: {}", args.out.display()))?;

    let ratio = if report.total_plain_len > 0 {
        report.total_pak_len as f64 / report.total_plain_len as f64 * 100.0
    } else {
        0.0
    };
    println!("エントリ数: {}", report.entry_count);
    println!(
        "codec: stored={}, deflate={}",
        report.codec_stored_count, report.codec_deflate_count
    );
    println!("元サイズ: {} バイト", report.total_plain_len);
    println!(
        "pak サイズ: {} バイト ({:.1}%)",
        report.total_pak_len, ratio
    );
    println!(
        "webp 変換: {} 件, 削減 {} バイト{}",
        report.webp_converted_count,
        report.webp_bytes_saved,
        if args.webp {
            ""
        } else {
            "（--no-webp 指定のため変換なし）"
        }
    );
    println!(
        "flac 変換: {} 件, 削減 {} バイト{}",
        report.flac_converted_count,
        report.flac_bytes_saved,
        if args.flac {
            ""
        } else {
            "（--no-flac 指定のため変換なし）"
        }
    );
    if args.list {
        for e in &packed.entries {
            println!("  {}  {} バイト{}", e.path, e.plain_len, e.conversion.map(|c| format!("  [{c}]")).unwrap_or_default());
        }
    }
    println!("{}", excluded_summary(&packed.excluded));
    if args.list_excluded {
        for e in &packed.excluded {
            println!("  除外: {}", e.describe());
        }
    }
    println!("書き出し完了: {}", args.out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_hex_key;

    #[test]
    fn hex_key_accepts_exactly_64_hex_digits() {
        let key = parse_hex_key(&"0aF".repeat(21).chars().chain("b".chars()).collect::<String>()).unwrap();
        assert_eq!(key[0], 0x0a);
        assert!(parse_hex_key(&"a".repeat(63)).is_err());
    }

    #[test]
    fn hex_key_rejects_sign_and_non_ascii_without_panicking() {
        assert!(parse_hex_key(&format!("+1{}", "0".repeat(62))).is_err());
        let non_ascii = format!("a{}a", "é".repeat(31));
        assert_eq!(non_ascii.len(), 64);
        assert!(parse_hex_key(&non_ascii).is_err());
    }
}
