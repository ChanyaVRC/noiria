//! 自作 OGG/Vorbis デコーダ (`codec::ogg` + `codec::vorbis`) の回帰スナップショットテスト。
//!
//! V1 では rodio (symphonia-codec-vorbis バックエンド) を参照デコーダとして実時間で
//! クロスチェックしていた（相関 1.0・最大誤差 1e-7 オーダーで一致することを確認済み）。
//! V2 で rodio を依存から完全削除したため、そのクロスチェック実行結果の要約を
//! 定数として焼き込み、以後は「自作デコーダの出力がこの参照値から劣化していないか」
//! を検知するリグレッションテストに切り替えた。
//!
//! `REF_*` 定数は `crates/noiria-core/examples/vorbis_snapshot.rs`（rodio を一時的に
//! dev-dependency として追加して実行した使い捨てスクリプト。既に削除済み）の出力を
//! そのまま貼り付けたもの。再生成する場合の手順:
//! 1. `crates/noiria-core/Cargo.toml` の `[dev-dependencies]` に
//!    `rodio = { version = "0.22", default-features = false, features = ["vorbis"] }` を追加
//! 2. 同じ内容の `examples/vorbis_snapshot.rs` を復元（本コミット以前の履歴、または本ファイル
//!    冒頭のコメントを参照して再実装）して `cargo run -p noiria-core --example vorbis_snapshot`
//! 3. 出力された `REF_*` 定数をこのファイルへ貼り付け、手順 1 の dev-dependency を削除
//!
//! スナップショットが「1 サンプルずつの厳密値」ではなく「1 秒ごとのチャンク RMS」なのは、
//! 自作デコーダと rodio とで先頭プライミング等による数サンプル程度のオフセット差が
//! あっても安定して比較できるようにするため（劣化検知が目的で、ビット完全一致の検証は
//! 既に上記の V1 クロスチェックで実施済み）。
//!
//! テストベクタは `tests/vectors_audio/`（取得元・取得手順は同ディレクトリの README 参照）。
//! ファイルが存在しない場合はテストを skip する (取得不能環境向け)。

use std::path::{Path, PathBuf};

fn vectors_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors_audio")
}

struct RefSnapshot {
    channels: usize,
    sample_rate: usize,
    frame_count: usize,
    overall_rms: f64,
    /// 1 秒 (sample_rate フレーム) ごとのチャンク RMS。末尾チャンクは半端でもよい。
    chunk_rms: &'static [f64],
}

// 生成手順はファイル冒頭のコメント参照。
const REF_MONO1_A_BOY2: RefSnapshot = RefSnapshot {
    channels: 1,
    sample_rate: 44100,
    frame_count: 93632,
    overall_rms: 0.0079116908,
    chunk_rms: &[0.0113515588, 0.0020000547, 0.0005817880],
};

const REF_MONO2_ABASHED_EN: RefSnapshot = RefSnapshot {
    channels: 1,
    sample_rate: 44100,
    frame_count: 73792,
    overall_rms: 0.1113771245,
    chunk_rms: &[0.1410911597, 0.0355351908],
};

const REF_STEREO1_KALMYKIA_ANTHEM_TRIMMED: RefSnapshot = RefSnapshot {
    channels: 2,
    sample_rate: 48000,
    frame_count: 1302720,
    overall_rms: 0.2488453227,
    chunk_rms: &[
        0.1664270134,
        0.2444056866,
        0.1783903002,
        0.1901257011,
        0.2485343542,
        0.2497593505,
        0.2642646022,
        0.2973643271,
        0.2650931459,
        0.2525709461,
        0.2581687680,
        0.2564876857,
        0.2515201119,
        0.2433971333,
        0.2429598317,
        0.2393359735,
        0.2603843817,
        0.2757258364,
        0.2651878983,
        0.2340516104,
        0.2358693025,
        0.2350157496,
        0.2661660936,
        0.2668629190,
        0.2598893183,
        0.2814532984,
        0.2457664150,
        0.2459473225,
    ],
};

fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum_sq / samples.len() as f64).sqrt()
}

/// 相対誤差 (許容値は呼び出し側が渡す)。両方 ~0 に近い場合は差を絶対値で見る。
fn approx_eq_rel(a: f64, b: f64, rel_tol: f64, abs_tol: f64) -> bool {
    (a - b).abs() <= abs_tol.max(rel_tol * a.abs().max(b.abs()))
}

fn check_snapshot(name: &str, r: &RefSnapshot) {
    let path = vectors_dir().join(name);
    if !path.exists() {
        eprintln!("skip: テストベクタが見つかりません: {}", path.display());
        return;
    }
    let bytes = std::fs::read(&path).expect("テストベクタの読み込みに失敗");

    let ours = match noiria_core::codec::vorbis::decode(&bytes) {
        Ok(d) => d,
        Err(e) => panic!("自作デコーダが失敗しました ({name}): {e:#}"),
    };

    assert_eq!(
        ours.channels as usize, r.channels,
        "チャンネル数不一致 ({name})"
    );
    assert_eq!(
        ours.sample_rate as usize, r.sample_rate,
        "サンプルレート不一致 ({name})"
    );

    let channels = r.channels;
    let our_frame_count = ours.samples.len() / channels;
    assert!(our_frame_count > 0, "デコード結果が空です ({name})");

    // フレーム数の差は数千フレーム程度を許容 (MDCT の先頭プライミング/末尾トリミング差。
    // V1 クロスチェック時の許容値 (4096) を踏襲)。
    let frame_diff = our_frame_count.abs_diff(r.frame_count);
    assert!(
        frame_diff <= 4096,
        "フレーム数が参照スナップショットと大きく食い違っています ({name}): \
         ours={our_frame_count} ref={}",
        r.frame_count
    );

    let overall_rms = rms(&ours.samples);
    assert!(
        approx_eq_rel(overall_rms, r.overall_rms, 0.05, 1e-6),
        "全体 RMS が参照スナップショットと食い違っています ({name}): \
         ours={overall_rms:.8} ref={:.8}",
        r.overall_rms
    );

    let mut frame = 0usize;
    let mut chunk_idx = 0usize;
    while frame < our_frame_count && chunk_idx < r.chunk_rms.len() {
        let end_frame = (frame + r.sample_rate).min(our_frame_count);
        let start = frame * channels;
        let end = end_frame * channels;
        let chunk = rms(&ours.samples[start..end]);
        let expected = r.chunk_rms[chunk_idx];
        assert!(
            approx_eq_rel(chunk, expected, 0.08, 1e-6),
            "チャンク {chunk_idx} (約 {chunk_idx}〜{}秒) の RMS が参照スナップショットと \
             食い違っています ({name}): ours={chunk:.8} ref={expected:.8}",
            chunk_idx + 1
        );
        frame = end_frame;
        chunk_idx += 1;
    }
    // 参照側のチャンク数の大半をこちらでも再現できていること（末尾の 1 チャンク程度の
    // 差はフレーム数許容差から生じうるので許す）。
    assert!(
        chunk_idx + 1 >= r.chunk_rms.len(),
        "チャンク数が参照スナップショットより大きく少ないです ({name}): \
         compared={chunk_idx} ref_total={}",
        r.chunk_rms.len()
    );

    eprintln!(
        "[vorbis snapshot] {name}: ch={channels} rate={} our_frames={our_frame_count} \
         ref_frames={} overall_rms={overall_rms:.8} (ref={:.8})",
        r.sample_rate, r.frame_count, r.overall_rms
    );
}

#[test]
fn snapshot_mono_a_boy2() {
    check_snapshot("mono1_a_boy2.ogg", &REF_MONO1_A_BOY2);
}

#[test]
fn snapshot_mono_abashed_en() {
    check_snapshot("mono2_abashed_en.ogg", &REF_MONO2_ABASHED_EN);
}

#[test]
fn snapshot_stereo_kalmykia_anthem() {
    check_snapshot(
        "stereo1_kalmykia_anthem_trimmed.ogg",
        &REF_STEREO1_KALMYKIA_ANTHEM_TRIMMED,
    );
}
