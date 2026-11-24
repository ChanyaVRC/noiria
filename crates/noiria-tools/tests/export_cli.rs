use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use noiria_tools::export::{
    run_export, run_export_with_policy, ExportOptions, ExportPolicy, Target,
};
use noiria_tools::testutil::TestRoot;

fn write_game(root: &Path) -> PathBuf {
    let game = root.join("game");
    fs::create_dir_all(&game).unwrap();
    fs::write(
        game.join("game.json"),
        br#"{"title":"Export Test","id":"export-test"}"#,
    )
    .unwrap();
    game
}

fn write_valid_binary(path: &Path) {
    let mut bytes = b"dummy-prefix".to_vec();
    bytes.extend_from_slice(&noiria_core::pak::key_slot_magic());
    bytes.extend_from_slice(&[0; 32]);
    bytes.extend_from_slice(b"dummy-suffix");
    fs::write(path, bytes).unwrap();
}

fn options(game_dir: &Path, out_dir: &Path, bin_windows: &Path, bin_linux: &Path) -> ExportOptions {
    ExportOptions {
        game_dir: game_dir.to_path_buf(),
        out_dir: out_dir.to_path_buf(),
        targets: vec![Target::Windows, Target::Linux],
        bin_windows: Some(bin_windows.to_path_buf()),
        bin_linux: Some(bin_linux.to_path_buf()),
        exe_name: Some("demo".to_string()),
        webp: false,
        flac: false,
        exclude: Vec::new(),
        default_excludes: true,
        project_excludes: true,
    }
}

fn output_file_names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn strict_missing_target_fails_before_creating_outputs() {
    let root = TestRoot::new("strict_missing");
    let game = write_game(root.path());
    let out = root.path().join("out");
    let windows_bin = root.path().join("noiria.exe");
    let missing_linux_bin = root.path().join("missing-noiria");
    write_valid_binary(&windows_bin);

    let error = run_export(&options(&game, &out, &windows_bin, &missing_linux_bin))
        .err()
        .expect("strict export must reject a missing target");

    assert!(format!("{error:#}").contains("linux 向けのビルド済みバイナリが読めません"));
    assert!(!out.exists(), "preflight failure must not create out");
}

#[test]
fn skip_missing_target_exports_present_target_and_cli_reports_partial_success() {
    let root = TestRoot::new("partial");
    let game = write_game(root.path());
    let windows_bin = root.path().join("noiria.exe");
    let missing_linux_bin = root.path().join("missing-noiria");
    write_valid_binary(&windows_bin);

    let api_out = root.path().join("api-out");
    let report = run_export_with_policy(
        &options(&game, &api_out, &windows_bin, &missing_linux_bin),
        ExportPolicy::SkipMissingTargets,
    )
    .expect("one present target should export");

    assert_eq!(report.targets.len(), 1);
    assert_eq!(report.targets[0].target, Target::Windows);
    assert_eq!(report.skipped_targets.len(), 1);
    assert_eq!(report.skipped_targets[0].target, Target::Linux);
    assert_eq!(report.skipped_targets[0].bin_path, missing_linux_bin);
    assert_eq!(
        output_file_names(&api_out.join("windows")),
        ["README.txt", "THIRD-PARTY.txt", "demo.exe", "game.pak"]
            .into_iter()
            .map(str::to_string)
            .collect()
    );
    assert!(!api_out.join("linux").exists());

    let cli_out = root.path().join("cli-out");
    let stale_linux = cli_out.join("linux").join("old-export.txt");
    fs::create_dir_all(stale_linux.parent().unwrap()).unwrap();
    fs::write(&stale_linux, b"keep").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_noiria-export"))
        .arg("--game")
        .arg(&game)
        .arg("--out")
        .arg(&cli_out)
        .arg("--targets")
        .arg("windows,linux")
        .arg("--bin-windows")
        .arg(&windows_bin)
        .arg("--bin-linux")
        .arg(&missing_linux_bin)
        .arg("--exe-name")
        .arg("demo")
        .arg("--no-webp")
        .arg("--no-flac")
        .arg("--skip-missing-targets")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("export 部分完了: 1/2"));
    assert!(stderr.contains("linux 向けのビルド済みバイナリが見つからない"));
    assert!(stderr.contains(&missing_linux_bin.display().to_string()));
    assert!(stderr.contains("既存の出力物は削除していません"));
    assert!(stale_linux.exists());
    assert_eq!(
        output_file_names(&cli_out.join("windows")),
        ["README.txt", "THIRD-PARTY.txt", "demo.exe", "game.pak"]
            .into_iter()
            .map(str::to_string)
            .collect()
    );
}

#[test]
fn skip_missing_targets_rejects_when_all_targets_are_missing() {
    let root = TestRoot::new("all_missing");
    let game = write_game(root.path());
    let out = root.path().join("out");
    let missing_windows_bin = root.path().join("missing-noiria.exe");
    let missing_linux_bin = root.path().join("missing-noiria");

    let error = run_export_with_policy(
        &options(&game, &out, &missing_windows_bin, &missing_linux_bin),
        ExportPolicy::SkipMissingTargets,
    )
    .err()
    .expect("all missing targets must fail");

    assert!(format!("{error:#}").contains("出力可能なターゲットがありません"));
    assert!(!out.exists(), "all-missing failure must not create out");
}

#[test]
fn skip_missing_policy_does_not_skip_an_invalid_key_slot() {
    let root = TestRoot::new("invalid_key_slot");
    let game = write_game(root.path());
    let out = root.path().join("out");
    let windows_bin = root.path().join("noiria.exe");
    let invalid_linux_bin = root.path().join("noiria");
    write_valid_binary(&windows_bin);
    fs::write(&invalid_linux_bin, b"not a noiria binary").unwrap();

    let error = run_export_with_policy(
        &options(&game, &out, &windows_bin, &invalid_linux_bin),
        ExportPolicy::SkipMissingTargets,
    )
    .err()
    .expect("a present but invalid target must fail");

    assert!(format!("{error:#}").contains("KEY_SLOT"));
    assert!(!out.exists(), "invalid preflight must not create out");
}

fn key_in_binary(bytes: &[u8]) -> [u8; 32] {
    let magic = noiria_core::pak::key_slot_magic();
    let pos = bytes
        .windows(magic.len())
        .position(|w| w == magic)
        .expect("KEY_SLOT magic");
    bytes[pos + magic.len()..pos + magic.len() + 32]
        .try_into()
        .unwrap()
}

/// `dir` の exe が持つ鍵で同じフォルダの `game.pak` が開けること（配布物として整合していること）。
fn assert_dir_is_consistent(dir: &Path, exe: &str) {
    let key = key_in_binary(&fs::read(dir.join(exe)).unwrap());
    noiria_core::pak::Pak::open_file(dir.join("game.pak"), &key).unwrap_or_else(|e| {
        panic!(
            "{} の exe と game.pak の鍵が食い違っています: {e:#}",
            dir.display()
        )
    });
}

#[test]
fn reexport_over_unwritable_pak_never_leaves_mismatched_key() {
    let root = TestRoot::new("unwritable_pak");
    let game = write_game(root.path());
    let out = root.path().join("out");
    let windows_bin = root.path().join("noiria.exe");
    let linux_bin = root.path().join("noiria");
    write_valid_binary(&windows_bin);
    write_valid_binary(&linux_bin);
    let opts = options(&game, &out, &windows_bin, &linux_bin);

    run_export(&opts).unwrap();
    assert_dir_is_consistent(&out.join("windows"), "demo.exe");

    let pak_path = out.join("windows").join("game.pak");
    let mut perm = fs::metadata(&pak_path).unwrap().permissions();
    perm.set_readonly(true);
    fs::set_permissions(&pak_path, perm).unwrap();

    let _ = run_export(&opts);

    assert_dir_is_consistent(&out.join("windows"), "demo.exe");
    assert_dir_is_consistent(&out.join("linux"), "demo");
}

#[test]
fn reexport_under_new_exe_name_leaves_no_stale_exe() {
    let root = TestRoot::new("rename_exe");
    let game = write_game(root.path());
    let out = root.path().join("out");
    let windows_bin = root.path().join("noiria.exe");
    let linux_bin = root.path().join("noiria");
    write_valid_binary(&windows_bin);
    write_valid_binary(&linux_bin);
    let mut opts = options(&game, &out, &windows_bin, &linux_bin);
    opts.exe_name = Some("old".to_string());
    run_export(&opts).unwrap();
    opts.exe_name = Some("new".to_string());
    run_export(&opts).unwrap();

    assert_eq!(
        output_file_names(&out.join("windows")),
        ["README.txt", "THIRD-PARTY.txt", "new.exe", "game.pak"]
            .into_iter()
            .map(str::to_string)
            .collect()
    );
}

#[test]
fn out_dir_inside_game_dir_is_not_packed_into_the_pak() {
    let root = TestRoot::new("out_in_game");
    let game = write_game(root.path());
    let out = game.join("export");
    let windows_bin = root.path().join("noiria.exe");
    let linux_bin = root.path().join("noiria");
    write_valid_binary(&windows_bin);
    write_valid_binary(&linux_bin);
    let opts = options(&game, &out, &windows_bin, &linux_bin);

    let first = run_export(&opts).unwrap();
    let second = run_export(&opts).unwrap();

    assert_eq!(first.pack_entry_count, 1);
    assert_eq!(
        second.pack_entry_count, 1,
        "2 回目の export が前回の出力（exe/game.pak）を pak に詰めている"
    );
}

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn write_failure_midway_leaves_previous_output_intact_and_no_work_dirs() {
    let root = TestRoot::new("inject_failure");
    let game = write_game(root.path());
    let out = root.path().join("out");
    let windows_bin = root.path().join("noiria.exe");
    let linux_bin = root.path().join("noiria");
    write_valid_binary(&windows_bin);
    write_valid_binary(&linux_bin);
    let opts = options(&game, &out, &windows_bin, &linux_bin);
    run_export(&opts).unwrap();
    let before_windows = snapshot(&out.join("windows"));
    let before_linux = snapshot(&out.join("linux"));

    let err =
        noiria_tools::export::run_export_with_writer(&opts, ExportPolicy::Strict, &mut |p, b| {
            if p.file_name().is_some_and(|n| n == "game.pak") {
                Err(std::io::Error::other("disk full (injected)"))
            } else {
                fs::write(p, b)
            }
        })
        .err()
        .expect("injected failure must surface");

    assert!(format!("{err:#}").contains("game.pak"));
    assert_eq!(snapshot(&out.join("windows")), before_windows);
    assert_eq!(snapshot(&out.join("linux")), before_linux);
    assert_eq!(
        output_file_names(&out),
        ["linux", "windows"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        "作業ディレクトリ（*.tmp / *.old）が残っている"
    );
    assert_dir_is_consistent(&out.join("windows"), "demo.exe");
}

#[test]
fn stale_work_dirs_from_a_crashed_run_are_cleaned_up() {
    let root = TestRoot::new("stale_tmp");
    let game = write_game(root.path());
    let out = root.path().join("out");
    let windows_bin = root.path().join("noiria.exe");
    let linux_bin = root.path().join("noiria");
    write_valid_binary(&windows_bin);
    write_valid_binary(&linux_bin);
    fs::create_dir_all(out.join("windows.tmp")).unwrap();
    fs::write(out.join("windows.tmp").join("junk"), b"x").unwrap();
    fs::create_dir_all(out.join("windows.old")).unwrap();
    run_export(&options(&game, &out, &windows_bin, &linux_bin)).unwrap();
    run_export(&options(&game, &out, &windows_bin, &linux_bin)).unwrap();
    assert_eq!(
        output_file_names(&out),
        ["linux", "windows"]
            .into_iter()
            .map(str::to_string)
            .collect()
    );
}

#[test]
fn game_json_pack_exclude_flows_through_export_and_cli_reports_the_breakdown() {
    let root = TestRoot::new("export_project_excludes");
    let game = root.path().join("game");
    fs::create_dir_all(game.join("autotest")).unwrap();
    fs::write(
        game.join("game.json"),
        br#"{"title":"Export Test","id":"export-test","pack":{"exclude":["autotest/**"]}}"#,
    )
    .unwrap();
    fs::write(game.join("autotest").join("a.txt"), b"x").unwrap();
    fs::write(game.join(".hidden"), b"x").unwrap();
    fs::write(game.join("scratch.txt"), b"x").unwrap();
    let windows_bin = root.path().join("noiria.exe");
    write_valid_binary(&windows_bin);

    let run = |extra: &[&str]| {
        let out = root.path().join("out");
        let output = Command::new(env!("CARGO_BIN_EXE_noiria-export"))
            .arg("--game")
            .arg(&game)
            .arg("--out")
            .arg(&out)
            .args([
                "--targets",
                "windows",
                "--exe-name",
                "demo",
                "--no-webp",
                "--no-flac",
            ])
            .arg("--bin-windows")
            .arg(&windows_bin)
            .args(["--exclude", "scratch.txt", "--list", "--list-excluded"])
            .args(extra)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    let stdout = run(&[]);
    assert!(stdout.contains("エントリ数 1,"), "{stdout}");
    assert!(
        stdout.contains("除外 3 件（default 1 / project 1 / cli 1 / output 0）"),
        "{stdout}"
    );
    assert!(
        stdout.contains("autotest/a.txt  (project: autotest/**)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("scratch.txt  (cli: scratch.txt)"),
        "{stdout}"
    );
    assert!(stdout.contains(".hidden  (default: .*)"), "{stdout}");

    let stdout = run(&["--no-project-excludes", "--no-default-excludes"]);
    assert!(stdout.contains("エントリ数 3,"), "{stdout}");
    assert!(
        stdout.contains("除外 1 件（default 0 / project 0 / cli 1 / output 0）"),
        "{stdout}"
    );
}
