//! noiria-export — ゲームディレクトリから Windows/Linux 配布物一式を生成する CLI。
//! 使い方:
//!   noiria-export --game games/sample --out target/export
//!                  [--targets windows,linux] [--bin-windows <exe>] [--bin-linux <bin>]
//!                  [--exe-name <name>] [--no-webp] [--no-flac] [--skip-missing-targets]
//!                  [--exclude <glob>]... [--no-default-excludes] [--no-project-excludes] [--list] [--list-excluded]

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use noiria_tools::export::{
    run_export, run_export_with_policy, ExportOptions, ExportPolicy, Target,
};
use noiria_tools::pack::{print_pack_listing, CommonPackCli};

struct Args {
    game: PathBuf,
    out: PathBuf,
    targets: Vec<Target>,
    bin_windows: Option<PathBuf>,
    bin_linux: Option<PathBuf>,
    exe_name: Option<String>,
    skip_missing_targets: bool,
    common: CommonPackCli,
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
    let mut common = CommonPackCli::default();
    let mut targets: Option<Vec<Target>> = None;
    let mut bin_windows: Option<PathBuf> = None;
    let mut bin_linux: Option<PathBuf> = None;
    let mut exe_name: Option<String> = None;
    let mut skip_missing_targets = false;

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        if !common.try_consume(&argv, &mut i)? {
            let next_val = |flag: &str, i: &mut usize| -> Result<&String> {
                let v = argv
                    .get(*i + 1)
                    .with_context(|| format!("{flag} には値が必要です"))?;
                *i += 1;
                Ok(v)
            };
            match argv[i].as_str() {
                "--targets" => targets = Some(parse_targets(next_val("--targets", &mut i)?)?),
                "--bin-windows" => bin_windows = Some(next_val("--bin-windows", &mut i)?.into()),
                "--bin-linux" => bin_linux = Some(next_val("--bin-linux", &mut i)?.into()),
                "--exe-name" => exe_name = Some(next_val("--exe-name", &mut i)?.clone()),
                "--skip-missing-targets" => skip_missing_targets = true,
                other => bail!("不明な引数です: {other}"),
            }
        }
        i += 1;
    }

    Ok(Args {
        game: common
            .game
            .take()
            .context("--game <ゲームディレクトリ> は必須です")?,
        out: common
            .out
            .take()
            .context("--out <出力先ディレクトリ> は必須です")?,
        targets: targets.unwrap_or_else(|| vec![Target::Windows, Target::Linux]),
        bin_windows,
        bin_linux,
        exe_name,
        skip_missing_targets,
        common,
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
        webp: args.common.webp,
        flac: args.common.flac,
        exclude: args.common.exclude,
        default_excludes: args.common.default_excludes,
        project_excludes: args.common.project_excludes,
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
    print_pack_listing(
        &report.pack_entries,
        &report.pack_excluded,
        args.common.list,
        args.common.list_excluded,
    );
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
