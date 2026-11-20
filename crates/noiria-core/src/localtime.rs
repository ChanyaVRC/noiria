//! 現在のローカル日時の文字列化（セーブ一覧の保存日時表示用）。
//!
//! JS の `Date` はエンジンごとに挙動が違う（tsint は getter が無く `toLocaleString` の
//! 書式も V8 と異なる）ため、日時は Rust 側で `YYYY/MM/DD HH:MM` に整形して渡す。
//! ローカルタイムゾーンの解決だけ OS API を直接呼ぶ（OS 境界。外部クレートは使わない）。

/// ローカルの年月日時分。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl LocalTime {
    /// `2026/11/19 07:05` 形式（ゼロ埋め、分まで）。
    pub fn format(&self) -> String {
        format!(
            "{:04}/{:02}/{:02} {:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }
}

#[cfg(windows)]
pub fn now_local() -> Option<LocalTime> {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(out: *mut SystemTime);
    }
    let mut st = SystemTime::default();
    // SAFETY: `st` は SYSTEMTIME と同一レイアウト（`repr(C)`、u16 が 8 個）の書き込み先で、
    // GetLocalTime はそこへ書くだけ。引数のポインタはこの呼び出しの間だけ有効であればよい。
    unsafe { GetLocalTime(&mut st) };
    Some(LocalTime {
        year: st.year as i32,
        month: st.month as u32,
        day: st.day as u32,
        hour: st.hour as u32,
        minute: st.minute as u32,
    })
}

#[cfg(unix)]
pub fn now_local() -> Option<LocalTime> {
    use std::os::raw::{c_char, c_int, c_long};
    #[repr(C)]
    struct Tm {
        tm_sec: c_int,
        tm_min: c_int,
        tm_hour: c_int,
        tm_mday: c_int,
        tm_mon: c_int,
        tm_year: c_int,
        tm_wday: c_int,
        tm_yday: c_int,
        tm_isdst: c_int,
        tm_gmtoff: c_long,
        tm_zone: *const c_char,
    }
    // C の time_t。64bit の Unix は常に i64 (= c_long)。32bit は glibc が c_long（i32）、
    // musl（1.2 以降）だけ 32bit でも i64。固定の i64 だと 32bit glibc で引数・戻り値の幅がずれる。
    #[cfg(all(target_pointer_width = "32", not(target_env = "musl")))]
    type TimeT = c_long;
    #[cfg(any(target_pointer_width = "64", target_env = "musl"))]
    type TimeT = i64;
    extern "C" {
        fn time(t: *mut TimeT) -> TimeT;
        fn localtime_r(t: *const TimeT, out: *mut Tm) -> *mut Tm;
    }
    let mut tm = Tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null(),
    };
    // SAFETY: time(NULL) は引数を書き込まない。`t` は有効なローカル変数で、localtime_r は
    // `&t` を読み、glibc/musl の `struct tm` と同一レイアウト（`repr(C)`）の `tm` へ書く
    // だけ（スレッドセーフな _r 版なので内部の静的領域を共有しない）。戻り値 NULL は失敗。
    let ok = unsafe {
        let t = time(std::ptr::null_mut());
        !localtime_r(&t, &mut tm).is_null()
    };
    ok.then_some(LocalTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
    })
}

#[cfg(not(any(windows, unix)))]
pub fn now_local() -> Option<LocalTime> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_zero_pads() {
        let t = LocalTime {
            year: 2026,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
        };
        assert_eq!(t.format(), "2026/01/02 03:04");
    }

    #[test]
    fn now_local_is_plausible() {
        let t = now_local().expect("OS からローカル時刻を取れる");
        assert!((2024..2200).contains(&t.year), "{t:?}");
        assert!((1..=12).contains(&t.month) && (1..=31).contains(&t.day), "{t:?}");
        assert!(t.hour < 24 && t.minute < 60, "{t:?}");
    }
}
