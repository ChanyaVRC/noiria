//! リファレンスBGM（実制作の CC0/PD 楽曲、`analysis/refs/` 配下）と自作3曲を
//! 同じ数値指標で解析し、表形式で比較出力する。
//!
//! 音の再生は一切行わない（数値解析のみ）。リファレンス音源はゲームアセットとしては
//! 使わない（解析専用、出典は `analysis/refs/SOURCES.md` を参照）。
//!
//! 実行: cargo run -p noiria-tools --release --example analyze_bgm
//!
//! `analysis/refs/` はリポジトリの git 管理外の作業ディレクトリで、ライセンス上
//! コミットしないため（`analysis/.gitignore` 参照）、環境によってはファイルが
//! 存在しない。その場合はリファレンス行を skip し、自作3曲だけの表を出す
//! （`docs/bgm-design-notes.md` にある取得手順で再取得できる）。

use std::path::{Path, PathBuf};
use std::time::Instant;

use noiria_tools::analysis::{self, Metrics};
use noiria_tools::synth;

fn refs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("analysis/refs")
}

/// (ファイル名, 表示名) のリスト。`SOURCES.md` に出典/ライセンスを記録済み。
const REF_FILES: &[(&str, &str)] = &[
    (
        "ambient_relaxing_isaiah658_trim.ogg",
        "ref:AmbientRelaxing(isaiah658)",
    ),
    ("relaxtune1_bgm_trim.ogg", "ref:BgmMusic(Tozan)"),
    (
        "mystical_town_symphony_trim.ogg",
        "ref:MysticalTown(symphony)",
    ),
    (
        "dark_shrine_loop_qubodup_trim.ogg",
        "ref:DarkShrineLoop(qubodup)",
    ),
    (
        "emotional_piano_loop_extenz.wav",
        "ref:EmotionalPiano(extenz)",
    ),
];

fn load_ref(path: &Path) -> anyhow::Result<synth::Stereo> {
    let bytes = std::fs::read(path)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (sr, channels, samples) = match ext.as_str() {
        "ogg" => {
            let d = noiria_core::codec::vorbis::decode(&bytes)?;
            (d.sample_rate, d.channels, d.samples)
        }
        "wav" => {
            let d = noiria_core::codec::wav::decode(&bytes)?;
            (d.sample_rate, d.channels, d.samples)
        }
        other => anyhow::bail!("未対応の拡張子: {other}"),
    };
    Ok(analysis::interleaved_to_stereo(sr, channels, &samples))
}

fn main() {
    let mut rows: Vec<Metrics> = Vec::new();

    let dir = refs_dir();
    for (fname, label) in REF_FILES {
        let path = dir.join(fname);
        if !path.exists() {
            eprintln!(
                "[skip] リファレンスが見つかりません: {} (SOURCES.md の取得手順を参照)",
                path.display()
            );
            continue;
        }
        let t0 = Instant::now();
        match load_ref(&path) {
            Ok(buf) => {
                let m = analysis::analyze_track(label, &buf);
                eprintln!(
                    "[decode] {label}: {:.2}s (デコード+解析 {:.2?})",
                    m.secs,
                    t0.elapsed()
                );
                rows.push(m);
            }
            Err(e) => eprintln!("[skip] {label} のデコードに失敗: {e:#}"),
        }
    }

    type Render = fn() -> synth::Stereo;
    let own: [(&str, Render); 3] = [
        ("own:main.wav", synth::render_main),
        ("own:alt_calm.wav", synth::render_alt_calm),
        ("own:alt_tension.wav", synth::render_alt_tension),
    ];
    for (label, render) in own {
        let buf = render();
        rows.push(analysis::analyze_track(label, &buf));
    }

    println!();
    analysis::print_table(&rows);

    // リファレンス群のレンジ（自作3曲を除く）を要約し、乖離を見やすくする。
    let ref_rows: Vec<&Metrics> = rows.iter().filter(|r| r.name.starts_with("ref:")).collect();
    if !ref_rows.is_empty() {
        println!();
        println!("--- リファレンス群のレンジ (n={}) ---", ref_rows.len());
        print_range("low%", ref_rows.iter().map(|r| r.band_low * 100.0));
        print_range("mid%", ref_rows.iter().map(|r| r.band_mid * 100.0));
        print_range("high%", ref_rows.iter().map(|r| r.band_high * 100.0));
        print_range("centroid_Hz", ref_rows.iter().map(|r| r.centroid_mean_hz));
        print_range(
            "centroid_std_Hz",
            ref_rows.iter().map(|r| r.centroid_std_hz),
        );
        print_range("onset/s", ref_rows.iter().map(|r| r.onset_density_per_sec));
        print_range("dyn_rms_std_dB", ref_rows.iter().map(|r| r.dyn_rms_std_db));
        print_range("crest_dB", ref_rows.iter().map(|r| r.crest_db));
        print_range("L/Rcorr", ref_rows.iter().map(|r| r.stereo_corr));
        print_range("AM_med", ref_rows.iter().map(|r| r.am_depth_median));
        print_range("AM_p90", ref_rows.iter().map(|r| r.am_depth_p90));
    } else {
        println!("(リファレンスが1曲もロードできなかったため、レンジ要約はスキップ)");
    }
}

fn print_range(label: &str, vals: impl Iterator<Item = f32>) {
    let vals: Vec<f32> = vals.collect();
    if vals.is_empty() {
        return;
    }
    let lo = vals.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = vals.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let m = vals.iter().sum::<f32>() / vals.len() as f32;
    println!("  {label:<16} {lo:>9.2} .. {hi:>9.2}  (avg {m:>9.2})");
}
