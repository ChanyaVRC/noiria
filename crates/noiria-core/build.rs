// alsa_out.rs は libasound を dlopen で実行時ロードする（リンク時依存なし、
// OS 境界 API 扱い。詳細は alsa_out.rs 冒頭コメント参照）。
//
// ただし dlopen/dlsym/dlclose/dlerror 自体のシンボルはリンク時に解決する必要が
// あり、glibc 2.34 未満の Linux ではこれらは libc.so ではなく libdl.so 側に
// ある（2.34 以降は libc に統合済み。ただし libdl.so.2 は後方互換のための
// 空のスタブとして引き続き存在し、リンクしても実害はない）。念のため
// Linux + glibc ターゲットでのみ明示的に `-ldl` をリンクする
// （musl/macOS は対象外。libasound 自体を使わない環境なので必要になったら
// 個別に判断する）。
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "linux" && target_env == "gnu" {
        println!("cargo:rustc-link-lib=dl");
    }
}
