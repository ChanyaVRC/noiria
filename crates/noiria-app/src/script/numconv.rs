//! スクリプトから届く数値を整数へ落とす共通規則（V8 / tsint で同一）。
//!
//! 規則: 非有限（NaN・±Infinity）は 0、有限なら四捨五入（0 から遠い側へ丸める）して対象型の範囲へ飽和。
//! V8 の op 引数（`u32` 直受けは切り捨て・ラップ、serde 構造体は `f64 as T`）と tsint の旧変換
//! （四捨五入 → クランプ）が食い違っていたので、両エンジンとも f64 で受けてここを通す。
//! 時間（ms）だけは `u32::MAX` で打ち切る: `Instant + Duration` は桁あふれで panic するため、
//! u64 いっぱいの tween 長を作らせない（約 49 日。実質無限として十分）。

use std::time::Duration;

use serde::{Deserialize, Deserializer};

fn rounded(n: f64) -> f64 {
    if n.is_finite() {
        n.round()
    } else {
        0.0
    }
}

pub(crate) fn to_u8(n: f64) -> u8 {
    rounded(n) as u8
}

pub(crate) fn to_u32(n: f64) -> u32 {
    rounded(n) as u32
}

pub(crate) fn to_i32(n: f64) -> i32 {
    rounded(n) as i32
}

/// 座標の絶対値の上限。`noiria_core::scene` の tween 補間は `to - from` を i32 で計算するので、
/// 両端がこの範囲なら差が i32 に収まる（範囲外の座標は意味を持たない）。
pub(crate) const COORD_MAX: i32 = (1 << 30) - 1;

/// 位置（x/y）。`to_i32` と同じ規則で、さらに ±`COORD_MAX` へ飽和する。
pub(crate) fn to_coord(n: f64) -> i32 {
    to_i32(n).clamp(-COORD_MAX, COORD_MAX)
}

/// 時間（ms）。負は 0、上限は `u32::MAX`。
pub(crate) fn to_time_ms(n: f64) -> u64 {
    u64::from(to_u32(n))
}

/// `sys.wait` 用。NaN・負は 0、`+Infinity` や巨大値は上限（`u32::MAX` ms）へ打ち切る
/// （`Duration::from_secs_f64` は無限大・約 1.8e22 超で panic する）。
pub(crate) fn wait_duration(ms: f64) -> Duration {
    let ms = if ms.is_nan() { 0.0 } else { ms };
    Duration::from_millis(ms.round().clamp(0.0, u32::MAX as f64) as u64)
}

// ---- serde（V8 の構造体フィールド）: `#[serde(deserialize_with = "...")]` で使う ----

pub(crate) fn de_u32<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    f64::deserialize(d).map(to_u32)
}

pub(crate) fn de_time_ms<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    f64::deserialize(d).map(to_time_ms)
}

pub(crate) fn de_opt_u8<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u8>, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.map(to_u8))
}

#[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
pub(crate) fn de_opt_u32<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.map(to_u32))
}

pub(crate) fn de_opt_coord<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.map(to_coord))
}

pub(crate) fn de_opt_i32<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.map(to_i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_rule_for_fractions_negatives_and_non_finite() {
        assert_eq!(to_i32(10.6), 11);
        assert_eq!(to_i32(-10.6), -11);
        assert_eq!(to_u8(127.5), 128);
        assert_eq!(to_u8(-1.0), 0);
        assert_eq!(to_u8(300.0), 255);
        assert_eq!(to_u32(-1.0), 0);
        assert_eq!(to_u32(1e20), u32::MAX);
        assert_eq!(to_i32(-1e20), i32::MIN);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!((to_u8(bad), to_u32(bad), to_i32(bad), to_time_ms(bad)), (0, 0, 0, 0));
        }
        assert_eq!(to_time_ms(1e30), u64::from(u32::MAX));
        assert_eq!((to_coord(1e20), to_coord(-1e20), to_coord(-10.6)), (COORD_MAX, -COORD_MAX, -11));
    }

    #[test]
    fn wait_duration_never_panics() {
        assert_eq!(wait_duration(f64::NAN), Duration::ZERO);
        assert_eq!(wait_duration(-5.0), Duration::ZERO);
        assert_eq!(wait_duration(1500.4), Duration::from_millis(1500));
        let cap = Duration::from_millis(u64::from(u32::MAX));
        assert_eq!(wait_duration(f64::INFINITY), cap);
        assert_eq!(wait_duration(1e300), cap);
    }
}
