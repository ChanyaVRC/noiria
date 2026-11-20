//! W9 統合テスト: FLAC 風サブセットコーデックの encode (`noiria_tools::flac_enc::try_encode`)
//! → decode (`noiria_core::codec::flac_subset::decode_wav`) の round-trip をバイト完全一致で
//! 検証する。エンコーダ (noiria-tools) とデコーダ (noiria-core) は意図的に別クレートで
//! 独立実装している（`docs/implementation-notes.md` 2026-07-12 配布対応 W9 参照）ため、
//! この統合テストが唯一「両方を同時に呼べる」場所になる。
//!
//! 内容:
//! 1. `games/sample` 実アセットの WAV 5 ファイル全部の round-trip
//! 2. エッジケース（奇数長 data、追加チャンク付き、8/24bit フォールバック、極短 PCM、無音、
//!    フルスケール、1ch/2ch）
//! 3. シード固定の擬似乱数 PCM（+ 正弦波・ランプ・矩形波等の合成波形）数百本の round-trip
//!    プロパティテスト

use std::path::Path;

use noiria_core::codec::flac_subset::decode_wav;
use noiria_tools::flac_enc::try_encode;

/// wav.rs/pack.rs のテストと同型の最小 WAV バイト列組み立てヘルパー（16bit PCM 固定）。
fn build_wav16(channels: u16, rate: u32, body: &[u8]) -> Vec<u8> {
    let block_align = channels * 2;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + body.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
    v.extend_from_slice(&block_align.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(body.len() as u32).to_le_bytes());
    v.extend_from_slice(body);
    v
}

fn samples_to_body(samples: &[i16]) -> Vec<u8> {
    let mut body = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        body.extend_from_slice(&s.to_le_bytes());
    }
    body
}

/// encode→decode し、`Some` の場合は元バイト列と完全一致することを assert する。
/// `None`（フォールバック）は許容し、呼び出し側が意図に応じて追加でチェックする。
fn assert_roundtrip_if_encoded(wav: &[u8]) -> bool {
    match try_encode(wav) {
        Some(blob) => {
            let restored = decode_wav(&blob).expect("decode_wav が失敗した（round-tripバグ）");
            assert_eq!(restored, wav, "round-trip でバイト列が一致しない");
            true
        }
        None => false,
    }
}

// ---------------------------------------------------------------------
// 1. games/sample 実アセットの WAV 5 ファイル
// ---------------------------------------------------------------------

#[test]
fn sample_game_wav_files_round_trip() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/sample/assets");
    if !root.exists() {
        eprintln!(
            "[skip] games/sample/assets が見つかりません: {}",
            root.display()
        );
        return;
    }
    let files = [
        "bgm/main.wav",
        "bgm/alt_calm.wav",
        "bgm/alt_tension.wav",
        "se/click.wav",
        "voice/alice_001.wav",
    ];
    let mut checked = 0;
    for rel in files {
        let path = root.join(rel);
        if !path.exists() {
            eprintln!("[skip] {rel} が見つかりません");
            continue;
        }
        let wav = std::fs::read(&path).unwrap();
        let blob = try_encode(&wav)
            .unwrap_or_else(|| panic!("{rel}: 実アセットの16bit WAVはflac化できるはず"));
        let restored = decode_wav(&blob).unwrap_or_else(|e| panic!("{rel}: decode失敗: {e:#}"));
        assert_eq!(restored, wav, "{rel}: round-trip バイト不一致");
        assert!(
            blob.len() < wav.len(),
            "{rel}: 圧縮されていない ({} -> {})",
            wav.len(),
            blob.len()
        );
        // サイズ計測ログ（`docs/implementation-notes.md` 2026-07-12 配布対応 W9 参照）:
        // `cargo test sample_game_wav_files_round_trip -- --nocapture` で圧縮率を確認できる。
        eprintln!(
            "[flac] {rel}: {} -> {} ({:.1}%)",
            wav.len(),
            blob.len(),
            blob.len() as f64 / wav.len() as f64 * 100.0
        );
        checked += 1;
    }
    assert!(checked > 0, "サンプル WAV が1つも見つかりませんでした");
}

#[test]
fn bgm_decode_speed_is_well_under_target() {
    // 目安: BGM 7.5MB級の pak 読出し(decode_wav)が <200ms/曲（起動・曲切替をブロックしない）。
    // CI 環境差によるフレーキーさを避けるため assert のしきい値は緩め(1000ms)に取り、
    // 実測値はログに出力して `docs/implementation-notes.md` 2026-07-13 の計測表に転記する
    // （`cargo test bgm_decode_speed_is_well_under_target --release -- --nocapture` で確認）。
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/sample/assets/bgm");
    if !root.exists() {
        eprintln!("[skip] games/sample/assets/bgm が見つかりません");
        return;
    }
    for rel in ["main.wav", "alt_calm.wav", "alt_tension.wav"] {
        let path = root.join(rel);
        if !path.exists() {
            continue;
        }
        let wav = std::fs::read(&path).unwrap();
        let blob = try_encode(&wav).unwrap_or_else(|| panic!("{rel}: flac化できるはず"));
        let start = std::time::Instant::now();
        let restored = decode_wav(&blob).unwrap();
        let elapsed = start.elapsed();
        assert_eq!(restored, wav, "{rel}: decode結果が元WAVと不一致");
        eprintln!(
            "[decode speed] {rel}: {:.1}ms ({} bytes blob)",
            elapsed.as_secs_f64() * 1000.0,
            blob.len()
        );
        assert!(
            elapsed.as_millis() < 1000,
            "{rel}: decode に {}ms かかった（目安200msを大幅に超過、性能劣化の可能性）",
            elapsed.as_millis()
        );
    }
}

// ---------------------------------------------------------------------
// 2. エッジケース
// ---------------------------------------------------------------------

#[test]
fn edge_case_odd_length_data_falls_back() {
    let body = vec![0u8; 1001]; // 16bit mono には割り切れない奇数長
    let wav = build_wav16(1, 44100, &body);
    assert!(
        try_encode(&wav).is_none(),
        "奇数長 data は非対応としてフォールバックするはず"
    );
}

#[test]
fn edge_case_extra_chunk_before_and_after_data_round_trips() {
    // LIST チャンクを data の前に、cue チャンク相当のダミーを data の後ろに挟む。
    let samples: Vec<i16> = (0..2000)
        .map(|i| ((i as f64 * 0.1).sin() * 20000.0) as i16)
        .collect();
    let body = samples_to_body(&samples);

    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    let list_payload = b"INFOISFTNoiriaTest\0"; // 適当なダミー内容（偶数長にはしない=パディング検証も兼ねる）
    let list_len = list_payload.len() as u32;
    let data_len = body.len() as u32;
    let extra_len = 8u32; // ダミーの追加チャンク
    let riff_len =
        4 + (8 + 16) + (8 + list_len + (list_len & 1)) + (8 + data_len) + (8 + extra_len);
    v.extend_from_slice(&riff_len.to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // mono
    v.extend_from_slice(&44100u32.to_le_bytes());
    v.extend_from_slice(&(44100u32 * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"LIST");
    v.extend_from_slice(&list_len.to_le_bytes());
    v.extend_from_slice(list_payload);
    if list_len % 2 == 1 {
        v.push(0);
    }
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    v.extend_from_slice(&body);
    v.extend_from_slice(b"tail");
    v.extend_from_slice(&extra_len.to_le_bytes());
    v.extend_from_slice(&[0xABu8; 8]);

    let encoded = assert_roundtrip_if_encoded(&v);
    assert!(encoded, "追加チャンク付きサイン波は flac 化できるはず");
}

#[test]
fn edge_case_8bit_and_24bit_fall_back() {
    let body8 = vec![128u8; 2000];
    let mut wav8 = Vec::new();
    wav8.extend_from_slice(b"RIFF");
    wav8.extend_from_slice(&(36 + body8.len() as u32).to_le_bytes());
    wav8.extend_from_slice(b"WAVE");
    wav8.extend_from_slice(b"fmt ");
    wav8.extend_from_slice(&16u32.to_le_bytes());
    wav8.extend_from_slice(&1u16.to_le_bytes());
    wav8.extend_from_slice(&1u16.to_le_bytes());
    wav8.extend_from_slice(&44100u32.to_le_bytes());
    wav8.extend_from_slice(&44100u32.to_le_bytes());
    wav8.extend_from_slice(&1u16.to_le_bytes());
    wav8.extend_from_slice(&8u16.to_le_bytes());
    wav8.extend_from_slice(b"data");
    wav8.extend_from_slice(&(body8.len() as u32).to_le_bytes());
    wav8.extend_from_slice(&body8);
    assert!(
        try_encode(&wav8).is_none(),
        "8bit PCM は非対応としてフォールバックするはず"
    );

    let mut body24 = Vec::new();
    for _ in 0..500 {
        body24.extend_from_slice(&[0, 0, 1]);
    }
    let mut wav24 = Vec::new();
    wav24.extend_from_slice(b"RIFF");
    wav24.extend_from_slice(&(36 + body24.len() as u32).to_le_bytes());
    wav24.extend_from_slice(b"WAVE");
    wav24.extend_from_slice(b"fmt ");
    wav24.extend_from_slice(&16u32.to_le_bytes());
    wav24.extend_from_slice(&1u16.to_le_bytes());
    wav24.extend_from_slice(&1u16.to_le_bytes());
    wav24.extend_from_slice(&44100u32.to_le_bytes());
    wav24.extend_from_slice(&(44100 * 3u32).to_le_bytes());
    wav24.extend_from_slice(&3u16.to_le_bytes());
    wav24.extend_from_slice(&24u16.to_le_bytes());
    wav24.extend_from_slice(b"data");
    wav24.extend_from_slice(&(body24.len() as u32).to_le_bytes());
    wav24.extend_from_slice(&body24);
    assert!(
        try_encode(&wav24).is_none(),
        "24bit PCM は非対応としてフォールバックするはず"
    );
}

#[test]
fn edge_case_very_short_pcm_round_trips() {
    // 数十サンプルの短い無音（確実に圧縮できる内容にして、実際に flac 経路を通す）。
    let samples: Vec<i16> = vec![0; 40];
    let wav = build_wav16(1, 44100, &samples_to_body(&samples));
    let encoded = assert_roundtrip_if_encoded(&wav);
    assert!(encoded, "短い無音は flac 化できるはず");

    // 単一サンプルのみ（さらに極端なケース）: 縮む/縮まないは問わず round-trip 一致だけ確認。
    let one_sample = vec![1234i16];
    let wav1 = build_wav16(1, 44100, &samples_to_body(&one_sample));
    assert_roundtrip_if_encoded(&wav1); // Some/None どちらでもパニックしないことを確認
}

#[test]
fn edge_case_silence_round_trips() {
    let samples = vec![0i16; 44100 * 2]; // 2秒 mono
    let wav = build_wav16(1, 44100, &samples_to_body(&samples));
    assert!(
        assert_roundtrip_if_encoded(&wav),
        "無音は flac 化できるはず"
    );
}

#[test]
fn edge_case_full_scale_round_trips() {
    // フルスケール正弦波（最大振幅）1秒ぶん、2ch。
    let n = 44100;
    let mut samples = Vec::with_capacity(n * 2);
    for i in 0..n {
        let v = ((i as f64 * 0.05).sin() * (i16::MAX as f64)) as i16;
        samples.push(v);
        samples.push(-v); // 右chは逆位相
    }
    let wav = build_wav16(2, 44100, &samples_to_body(&samples));
    assert!(
        assert_roundtrip_if_encoded(&wav),
        "フルスケール正弦波は flac 化できるはず"
    );
}

#[test]
fn edge_case_mono_round_trips() {
    let samples: Vec<i16> = (0..5000).map(|i| ((i % 300) as i16) - 150).collect();
    let wav = build_wav16(1, 22050, &samples_to_body(&samples));
    assert!(
        assert_roundtrip_if_encoded(&wav),
        "モノラルのランプ波形は flac 化できるはず"
    );
}

#[test]
fn edge_case_smooth_multitone_signal_exercises_lpc_path_round_trips() {
    // シンセ BGM を模した滑らかな複合正弦波。`noiria_tools::flac_enc` の単体テスト
    // (`high_order_correlated_signal_picks_lpc_over_fixed`) で同じ形状の信号が実際に
    // LPC 候補（次数1-8）を選ぶことを確認済み — このテストはその LPC 経路を通った
    // ブロックを含む blob が decode 側でバイト完全一致することを保証する
    // （`docs/implementation-notes.md` 2026-07-13 wav-flac LPC 拡張 参照）。
    let n = 6000;
    let samples: Vec<i16> = (0..n)
        .map(|i| {
            let t = i as f64;
            (5000.0 * (0.017 * t).sin() + 3000.0 * (0.0031 * t).sin()) as i16
        })
        .collect();
    let wav = build_wav16(1, 44100, &samples_to_body(&samples));
    assert!(
        assert_roundtrip_if_encoded(&wav),
        "滑らかな複合正弦波は flac 化できるはず"
    );
}

// ---------------------------------------------------------------------
// 3. プロパティテスト（シード固定の疑似乱数 + 合成波形、数百本）
// ---------------------------------------------------------------------

/// 外部 crate 非依存の最小 PRNG（splitmix64）。テスト専用、暗号用途ではない。
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    fn next_i16(&mut self) -> i16 {
        (self.next_u64() as u16) as i16
    }
    fn next_range(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next_u64() as usize) % (hi - lo + 1)
    }
}

#[derive(Clone, Copy)]
enum Waveform {
    WhiteNoise,
    RandomWalk,
    Sine,
    Ramp,
    Square,
    Silence,
}

fn gen_samples(rng: &mut Rng, kind: Waveform, n: usize, amp: f64) -> Vec<i16> {
    match kind {
        Waveform::WhiteNoise => (0..n).map(|_| rng.next_i16()).collect(),
        Waveform::RandomWalk => {
            let mut v = 0i32;
            (0..n)
                .map(|_| {
                    let step = (rng.next_i16() as i32) / 4096; // 小さめの歩幅で相関を持たせる
                    v = (v + step).clamp(i16::MIN as i32, i16::MAX as i32);
                    v as i16
                })
                .collect()
        }
        Waveform::Sine => {
            let freq = 0.01 + (rng.next_u64() % 100) as f64 * 0.001;
            (0..n)
                .map(|i| ((i as f64 * freq).sin() * amp) as i16)
                .collect()
        }
        Waveform::Ramp => {
            let period = rng.next_range(10, 500) as i64;
            (0..n)
                .map(|i| {
                    let phase = (i as i64) % period;
                    let frac = phase as f64 / period as f64;
                    ((frac * 2.0 - 1.0) * amp) as i16
                })
                .collect()
        }
        Waveform::Square => {
            let period = rng.next_range(4, 200);
            (0..n)
                .map(|i| {
                    if (i / period.max(1)).is_multiple_of(2) {
                        amp as i16
                    } else {
                        -(amp as i16)
                    }
                })
                .collect()
        }
        Waveform::Silence => vec![0i16; n],
    }
}

#[test]
fn property_random_and_synthetic_pcm_round_trip() {
    let mut rng = Rng(0x00C0_FFEE_1234_5678);
    let waveforms = [
        Waveform::WhiteNoise,
        Waveform::RandomWalk,
        Waveform::Sine,
        Waveform::Ramp,
        Waveform::Square,
        Waveform::Silence,
    ];

    let mut total = 0usize;
    let mut encoded_count = 0usize;
    for _ in 0..300 {
        let kind = waveforms[rng.next_range(0, waveforms.len() - 1)];
        let channels = if rng.next_range(0, 1) == 0 {
            1u16
        } else {
            2u16
        };
        let n_frames = rng.next_range(1, 6000);
        let amp = 1000.0 + (rng.next_u64() % 31000) as f64;
        let rate = [8000u32, 22050, 44100][rng.next_range(0, 2)];

        let n_samples = n_frames * channels as usize;
        let samples = gen_samples(&mut rng, kind, n_samples, amp);
        let wav = build_wav16(channels, rate, &samples_to_body(&samples));

        total += 1;
        if assert_roundtrip_if_encoded(&wav) {
            encoded_count += 1;
        }
    }

    // 合成波形の大半（無音・正弦波・ランプ・矩形波・相関のあるランダムウォーク）は
    // 圧縮が効いて Some になるはず。白色雑音は非圧縮的なので None もあり得る
    // （コンテナヘッダのオーバーヘッド分だけ元より大きくなるため、正しいフォールバック）。
    assert!(
        encoded_count > total / 3,
        "圧縮が適用されたケースが少なすぎる（実装に問題がある可能性）: {encoded_count}/{total}"
    );
    eprintln!("property test: {encoded_count}/{total} 本が flac 化され round-trip 確認済み");
}

#[test]
fn property_stereo_channel_interleave_is_preserved() {
    // L/R が独立に符号化されても、デコード後のインターリーブ順（LRLR...）が
    // 元と一致することを明示的に検証する（チャンネル取り違えバグの検出用）。
    let mut rng = Rng(42);
    for _ in 0..50 {
        let n_frames = rng.next_range(10, 2000);
        let mut samples = Vec::with_capacity(n_frames * 2);
        for _ in 0..n_frames {
            samples.push(rng.next_i16() / 8); // 左（やや相関を持たせて圧縮されやすくする）
            samples.push(rng.next_i16() / 8); // 右
        }
        let wav = build_wav16(2, 44100, &samples_to_body(&samples));
        if let Some(blob) = try_encode(&wav) {
            let restored = decode_wav(&blob).unwrap();
            assert_eq!(restored, wav, "ステレオのインターリーブ順が壊れている");
        }
    }
}
