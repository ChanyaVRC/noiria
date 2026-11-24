//! `pack_dir` のディレクトリ走査（リンク・重複キー・既定除外）の統合テスト。

use std::path::Path;

use noiria_core::pak::Pak;
use noiria_tools::pack::pack_dir;
use noiria_tools::testutil::TestRoot;

const KEY: [u8; 32] = [9u8; 32];

fn game(root: &TestRoot) -> std::path::PathBuf {
    root.write("game/game.json", br#"{"title":"t","id":"t"}"#);
    root.path().join("game")
}

/// ディレクトリへのリンク（Windows ではジャンクション、unix ではシンボリックリンク）を作る。
fn link_dir(link: &Path, target: &Path) {
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(status.status.success(), "mklink /J failed");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn link_to_outside_directory_is_an_error_not_silently_packed() {
    let root = TestRoot::new("walk_link_outside");
    let game = game(&root);
    root.write("outside/secret.txt", b"not part of the game");
    link_dir(&game.join("linked"), &root.path().join("outside"));

    let err = pack_dir(&game, &KEY, &[], false, false)
        .expect_err("ゲームディレクトリ外を指すリンクは error にする");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("linked") && msg.contains("リンク"),
        "該当パスとリンクであることを示す: {msg}"
    );
}

#[test]
fn link_loop_is_an_error_not_a_stack_overflow() {
    let root = TestRoot::new("walk_link_loop");
    let game = game(&root);
    root.write("game/sub/a.txt", b"a");
    link_dir(&game.join("sub").join("loop"), &game);

    let err = pack_dir(&game, &KEY, &[], false, false).expect_err("循環リンクは error にする");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("loop") && msg.contains("リンク"),
        "リンクだと明示する: {msg}"
    );
}

#[test]
fn junk_and_dot_files_are_excluded_by_default() {
    let root = TestRoot::new("walk_junk");
    let game = game(&root);
    root.write("game/.git/config", b"x");
    root.write("game/.DS_Store", b"x");
    root.write("game/Thumbs.db", b"x");
    root.write("game/desktop.ini", b"x");
    root.write("game/scenario/first.ks~", b"x");
    root.write("game/scenario/first.ks.bak", b"x");
    root.write("game/scenario/first.ks.tmp", b"x");
    root.write("game/scenario/first.ks", b"@title");

    let (bytes, report) = pack_dir(&game, &KEY, &[], false, false).unwrap();
    assert_eq!(report.entry_count, 2, "game.json と first.ks だけ");
    let pak = Pak::open(bytes, &KEY).unwrap();
    assert!(pak.exists("game.json") && pak.exists("scenario/first.ks"));
}

#[cfg(unix)]
#[test]
fn case_only_duplicate_keys_are_an_error() {
    let root = TestRoot::new("walk_case_dup");
    let game = game(&root);
    root.write("game/assets/Room.png", b"a");
    root.write("game/assets/room.png", b"b");
    let err = pack_dir(&game, &KEY, &[], false, false)
        .expect_err("大文字小文字だけが違うキーは error にする");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("Room.png") && msg.contains("room.png"),
        "{msg}"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_file_name_is_an_error() {
    use std::os::unix::ffi::OsStrExt;
    let root = TestRoot::new("walk_non_utf8");
    let game = game(&root);
    let name = std::ffi::OsStr::from_bytes(b"bad_\xff_name.txt");
    std::fs::write(game.join(name), b"x").unwrap();
    let err = pack_dir(&game, &KEY, &[], false, false).expect_err("非 UTF-8 名は error にする");
    assert!(format!("{err:#}").contains("UTF-8"));
}

#[cfg(windows)]
#[test]
fn unpaired_surrogate_file_name_is_an_error() {
    use std::os::windows::ffi::OsStringExt;
    let root = TestRoot::new("walk_non_utf8_win");
    let game = game(&root);
    let name = std::ffi::OsString::from_wide(&[b'b' as u16, 0xD800, b'.' as u16, b't' as u16]);
    std::fs::write(game.join(name), b"x").unwrap();
    let err = pack_dir(&game, &KEY, &[], false, false).expect_err("非 UTF-8 名は error にする");
    assert!(format!("{err:#}").contains("UTF-8"));
}
