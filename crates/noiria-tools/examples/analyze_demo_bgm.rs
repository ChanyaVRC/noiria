//! デモ BGM 5 曲（`synth::demo`）をリファレンス曲と同じ指標で並べて比較し、
//! 譜面データの衝突音程・旋律の跳躍なども一覧する。音は再生しない。
//!
//! 実行: cargo run -p noiria-tools --release --example analyze_demo_bgm [-- ラベル=wavパス ...]
//! 追加引数の WAV（旧デモ曲など）は参照用の行として末尾に足す。

use std::path::Path;

use noiria_tools::analysis::{self, Metrics};
use noiria_tools::synth;
use noiria_tools::synth::demo::{self, SongSpec};

fn load_wav(path: &Path) -> anyhow::Result<synth::Stereo> {
    let d = noiria_core::codec::wav::decode(&std::fs::read(path)?)?;
    Ok(analysis::interleaved_to_stereo(
        d.sample_rate,
        d.channels,
        &d.samples,
    ))
}

fn range(rows: &[&Metrics], label: &str, f: impl Fn(&Metrics) -> f32) {
    let v: Vec<f32> = rows.iter().map(|r| f(r)).collect();
    let lo = v.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = v.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    println!("  {label:<16} {lo:>9.2} .. {hi:>9.2}");
}

fn worst_presence(buf: &synth::Stereo) -> f32 {
    use noiria_tools::analysis::{fft, hann_window, Complex};
    let mono: Vec<f32> = buf
        .l
        .iter()
        .zip(buf.r.iter())
        .map(|(a, b)| 0.5 * (a + b))
        .collect();
    let sr = buf.sr as f32;
    let n = 16384usize;
    let win = hann_window(n);
    let mut worst = 0f32;
    let mut pos = 0;
    while pos + n <= mono.len() {
        let mut b: Vec<Complex> = (0..n)
            .map(|i| Complex {
                re: mono[pos + i] * win[i],
                im: 0.0,
            })
            .collect();
        fft(&mut b);
        let (mut tot, mut h) = (0f64, 0f64);
        for (k, c) in b.iter().take(n / 2).enumerate() {
            let f = k as f32 * sr / n as f32;
            let e = (c.re * c.re + c.im * c.im) as f64;
            if f >= 60.0 {
                tot += e;
            }
            if (2000.0..5000.0).contains(&f) {
                h += e;
            }
        }
        worst = worst.max((h / tot.max(1e-12)) as f32);
        pos += n;
    }
    worst
}

fn score_report(name: &str, spec: &SongSpec) {
    let vs = demo::voicings(spec);
    let mut clashes = 0;
    for (ci, _) in spec.segs.iter().enumerate() {
        let (s, e) = spec.seg_beats(ci);
        let mut tones: Vec<i32> = vs[ci].clone();
        tones.push(demo::bass_semis(spec.segs[ci].root));
        tones.extend(demo::arp_tones(spec, &vs, ci));
        for &(b, m, len, _) in &spec.melody {
            if b + len > s + 0.25 && b < e - 0.25 {
                for &t in &tones {
                    if demo::clash(m, t) {
                        println!("  [{name}] clash: seg{ci} beat{b} melody={m} vs tone={t}");
                        clashes += 1;
                    }
                }
            }
        }
    }
    println!("[{name}] melody/chord clashes: {clashes}");
}

fn main() {
    let mut rows: Vec<Metrics> = Vec::new();
    let refs = Path::new(env!("CARGO_MANIFEST_DIR")).join("analysis/refs");
    for (f, label) in [
        ("ambient_relaxing_isaiah658_trim.ogg", "ref:AmbientRelaxing"),
        ("relaxtune1_bgm_trim.ogg", "ref:BgmMusic"),
        ("mystical_town_symphony_trim.ogg", "ref:MysticalTown"),
        ("dark_shrine_loop_qubodup_trim.ogg", "ref:DarkShrine"),
        ("emotional_piano_loop_extenz.wav", "ref:EmotionalPiano"),
    ] {
        let p = refs.join(f);
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        let d = if f.ends_with(".ogg") {
            noiria_core::codec::vorbis::decode(&bytes)
                .map(|d| (d.sample_rate, d.channels, d.samples))
        } else {
            noiria_core::codec::wav::decode(&bytes).map(|d| (d.sample_rate, d.channels, d.samples))
        };
        if let Ok((sr, ch, s)) = d {
            let b = analysis::interleaved_to_stereo(sr, ch, &s);
            println!(
                "[{label}] worst 2-5kHz window ratio = {:.4}",
                worst_presence(&b)
            );
            rows.push(analysis::analyze_track(label, &b));
        }
    }
    let n_ref = rows.len();
    let specs: [(&str, SongSpec); 5] = [
        ("title", demo::title_spec()),
        ("daily", demo::daily_spec()),
        ("calm", demo::calm_spec()),
        ("tension", demo::tension_spec()),
        ("night", demo::night_spec()),
    ];
    for (name, spec) in &specs {
        score_report(name, spec);
        rows.push(analysis::analyze_track(
            &format!("demo:{name}"),
            &demo::render_song(spec),
        ));
    }
    for a in std::env::args().skip(1) {
        if let Some((label, path)) = a.split_once('=') {
            match load_wav(Path::new(path)) {
                Ok(b) => rows.push(analysis::analyze_track(label, &b)),
                Err(e) => eprintln!("[skip] {label}: {e:#}"),
            }
        }
    }
    println!();
    analysis::print_table(&rows);
    let refr: Vec<&Metrics> = rows[..n_ref].iter().collect();
    if !refr.is_empty() {
        println!("\n--- refs (min..max) ---");
        range(&refr, "low%", |m| m.band_low * 100.0);
        range(&refr, "mid%", |m| m.band_mid * 100.0);
        range(&refr, "high%", |m| m.band_high * 100.0);
        range(&refr, "centroid_Hz", |m| m.centroid_mean_hz);
        range(&refr, "centroid_std", |m| m.centroid_std_hz);
        range(&refr, "onset/s", |m| m.onset_density_per_sec);
        range(&refr, "dyn_std_dB", |m| m.dyn_rms_std_db);
        range(&refr, "crest_dB", |m| m.crest_db);
        range(&refr, "L/Rcorr", |m| m.stereo_corr);
        range(&refr, "decay_dB/s", |m| {
            m.decay_slope_db_per_s.unwrap_or(0.0)
        });
        range(&refr, "AM_med", |m| m.am_depth_median);
        range(&refr, "AM_p90", |m| m.am_depth_p90);
    }
}
