//! noiria-export — ゲームディレクトリから Windows/Linux 配布物一式を生成する CLI。
//! 使い方:
//!   noiria-export --game games/sample --out target/export
//!                  [--targets windows,linux] [--bin-windows <exe>] [--bin-linux <bin>]
//!                  [--exe-name <name>] [--no-webp] [--no-flac] [--skip-missing-targets]
//!                  [--exclude <glob>]... [--no-default-excludes] [--no-project-excludes] [--list] [--list-excluded]
//! `--exclude`/`--no-default-excludes`/`--no-project-excludes`/`--list`/`--list-excluded` と game.json の `pack.exclude` は noiria-pack と同じ（`--out` ディレクトリが
//! `--game` 内にあれば自動的に pak から外れる）。出力は `<out>/<target>.tmp` に全部書いてから
//! `<out>/<target>` と入れ替える（失敗しても旧出力は壊れない）。
//! `--targets` 省略時は windows,linux 両方。`--bin-*` 省略時は `target/dist/` 配下の
//! 既定ファイル名（`noiria.exe`/`noiria`）を探す（先に dist プロファイルでビルドしておく
//! 必要がある。`docs/distribution.md` 参照）。`--exe-name` 省略時は game.json の title を
//! 安全な文字だけへ正規化した名前を使う。`--no-webp` を指定しない限り、pack 時に
//! `.png` を WebP ロスレスへ自動変換する（元 PNG より小さい場合のみ採用、W8）。
//! `--no-flac` を指定しない限り、pack 時に `.wav` を wav-flac (pak codec=2) へ可逆変換する
//! （元 WAV より小さい場合のみ採用、W9）。

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use noiria_tools::export::{
    run_export, run_export_with_policy, ExportOptions, ExportPolicy, Target,
};
use noiria_tools::pack::excluded_summary;

struct Args {
    game: PathBuf,
    out: PathBuf,
    targets: Vec<Target>,
    bin_windows: Option<PathBuf>,
    bin_linux: Option<PathBuf>,
    exe_name: Option<String>,
    webp: bool,
    flac: bool,
    skip_missing_targets: bool,
    exclude: Vec<String>,
    default_excludes: bool,
    project_excludes: bool,
    list: bool,
    list_excluded: bool,
}

fn parse_targets(spec: &str) -> Result<Vec<Target>> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        match part.trim() {
            "windows" => out.push(Target::Windows),
            "linux" => out.push(Target::Linux),
            "" => {}
            other => bail!("--targets に不明な値です: {other}（windows/linux のみ対応）"),
        }
    }
    if out.is_empty() {
        bail!("--targets が空です");
    }
    Ok(out)
}

fn parse_args() -> Result<Args> {
    let mut game: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut targets: Option<Vec<Target>> = None;
    let mut bin_windows: Option<PathBuf> = None;
    let mut bin_linux: Option<PathBuf> = None;
    let mut exe_name: Option<String> = None;
    let mut webp = true;
    let mut flac = true;
    let mut skip_missing_targets = false;
    let mut exclude: Vec<String> = Vec::new();
    let mut default_excludes = true;
    let mut project_excludes = true;
    let mut list = false;
    let mut list_excluded = false;

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
            "--targets" => {
                let spec = argv.get(i + 1).context("--targets には値が必要です")?;
                targets = Some(parse_targets(spec)?);
                i += 1;
            }
            "--bin-windows" => {
                bin_windows = Some(
                    argv.get(i + 1)
                        .context("--bin-windows には値が必要です")?
                        .into(),
                );
                i += 1;
            }
            "--bin-linux" => {
                bin_linux = Some(
                    argv.get(i + 1)
                        .context("--bin-linux には値が必要です")?
                        .into(),
                );
                i += 1;
            }
            "--exe-name" => {
                exe_name = Some(
                    argv.get(i + 1)
                        .context("--exe-name には値が必要です")?
                        .clone(),
                );
                i += 1;
            }
            "--no-webp" => {
                webp = false;
            }
            "--no-flac" => {
                flac = false;
            }
            "--skip-missing-targets" => {
                skip_missing_targets = true;
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
            other => bail!("不明な引数です: {other}"),
        }
        i += 1;
    }

    Ok(Args {
        game: game.context("--game <ゲームディレクトリ> は必須です")?,
        out: out.context("--out <出力先ディレクトリ> は必須です")?,
        targets: targets.unwrap_or_else(|| vec![Target::Windows, Target::Linux]),
        bin_windows,
        bin_linux,
        exe_name,
        webp,
        flac,
        skip_missing_targets,
        exclude,
        default_excludes,
        project_excludes,
        list,
        list_excluded,
    })
}

fn main() -> Result<()> {
    let args = parse_args()?;

    println!("export 開始: {}", args.game.display());
    let requested_target_count = args.targets.len();
    let opts = ExportOptions {
        game_dir: args.game,
        out_dir: args.out,
        targets: args.targets,
        bin_windows: args.bin_windows,
        bin_linux: args.bin_linux,
        exe_name: args.exe_name,
        webp: args.webp,
        flac: args.flac,
        exclude: args.exclude,
        default_excludes: args.default_excludes,
        project_excludes: args.project_excludes,
    };
    let report = if args.skip_missing_targets {
        run_export_with_policy(&opts, ExportPolicy::SkipMissingTargets)
    } else {
        run_export(&opts)
    }?;

    for w in &report.pack_warnings {
        eprintln!("警告: {w}");
    }
    for rel in &report.mp4_warnings {
        println!(
            "警告: pak 非対応のアセットです（Media Foundation は pak 内から再生できません）: {rel}"
        );
    }

    for skipped in &report.skipped_targets {
        eprintln!(
            "警告: {} 向けのビルド済みバイナリが見つからないためスキップしました: {}\
             （既存の出力物は削除していません）",
            skipped.target.name(),
            skipped.bin_path.display()
        );
    }

    println!("exe 名: {}", report.exe_name);
    println!(
        "pack: エントリ数 {}, 元サイズ {} バイト, pak サイズ {} バイト",
        report.pack_entry_count, report.pack_total_plain_len, report.pack_total_pak_len
    );
    println!(
        "webp 変換: {} 件, 削減 {} バイト",
        report.webp_converted_count, report.webp_bytes_saved
    );
    println!(
        "flac 変換: {} 件, 削減 {} バイト",
        report.flac_converted_count, report.flac_bytes_saved
    );
    if args.list {
        for e in &report.pack_entries {
            println!(
                "  {}  {} バイト{}",
                e.path,
                e.plain_len,
                e.conversion.map(|c| format!("  [{c}]")).unwrap_or_default()
            );
        }
    }
    println!("{}", excluded_summary(&report.pack_excluded));
    if args.list_excluded {
        for e in &report.pack_excluded {
            println!("  除外: {}", e.describe());
        }
    }
    for t in &report.targets {
        println!(
            "{}: {} ({} バイト)",
            t.target.name(),
            t.bin_path.display(),
            t.bin_size
        );
    }
    if report.skipped_targets.is_empty() {
        println!("export 完了");
    } else {
        println!(
            "export 部分完了: {}/{}",
            report.targets.len(),
            requested_target_count
        );
    }
    Ok(())
}
