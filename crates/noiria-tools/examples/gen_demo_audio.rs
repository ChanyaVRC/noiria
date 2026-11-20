//! デモゲームの BGM 5 曲 + SE 3 本だけを書き出す（絵は触らない）。
//! 実行: cargo run -p noiria-tools --release --example gen_demo_audio -- <出力先 assets ディレクトリ>
//! `click.wav` は書かない（採用済み SE のまま）。

use std::path::Path;

use anyhow::Result;
use noiria_tools::synth::{demo, Stereo};
use noiria_tools::wavout::write_wav;

fn stereo(path: &Path, b: &Stereo) -> Result<()> {
    write_wav(path, 2, (0..b.len()).flat_map(|i| [b.l[i], b.r[i]]))
}

fn main() -> Result<()> {
    let out = std::env::args().nth(1).expect("出力先ディレクトリを指定");
    let out = Path::new(&out);
    std::fs::create_dir_all(out.join("bgm"))?;
    std::fs::create_dir_all(out.join("se"))?;
    stereo(&out.join("bgm/title.wav"), &demo::render_title())?;
    stereo(&out.join("bgm/daily.wav"), &demo::render_daily())?;
    stereo(&out.join("bgm/calm.wav"), &demo::render_calm())?;
    stereo(&out.join("bgm/tension.wav"), &demo::render_tension())?;
    stereo(&out.join("bgm/night.wav"), &demo::render_night())?;
    write_wav(
        &out.join("se/chime.wav"),
        1,
        demo::render_chime().into_iter(),
    )?;
    write_wav(
        &out.join("se/sparkle.wav"),
        1,
        demo::render_sparkle().into_iter(),
    )?;
    write_wav(
        &out.join("se/heartbeat.wav"),
        1,
        demo::render_heartbeat().into_iter(),
    )?;
    Ok(())
}
