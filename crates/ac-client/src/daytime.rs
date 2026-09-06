//! What time it is in Dereth.
//!
//! The server tells us its clock in the packets it sends (the TimeSync
//! field); the region file says when the world's clock started and how
//! long a day lasts, so between them the client knows the time of day.
//! The sky, the sun and the ambient light are drawn from it, and the
//! status line can say "Morning".

use crate::Client;

/// Where we are in the day: 0 at midnight, 0.5 at midday.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DayTime {
    pub fraction: f32,
    /// The name the region gives this part of the day ("Morning").
    pub name: &'static str,
    pub is_night: bool,
}

/// The time of day at `server_time` seconds on the world's clock, given
/// when the clock started and how long a day is.
pub fn fraction_of_day(server_time: f64, zero_time_of_year: f64, day_length: f32) -> f32 {
    if !(day_length.is_finite() && day_length > 1.0) {
        return 0.5;
    }
    let since = server_time - zero_time_of_year;
    let days = since / day_length as f64;
    (days - days.floor()) as f32
}

/// The clock time as `HH:MM` of a day that runs midnight to midnight.
pub fn clock(fraction: f32) -> String {
    let t = fraction.rem_euclid(1.0) * 24.0;
    let h = t.floor() as u32 % 24;
    let m = ((t - t.floor()) * 60.0).floor() as u32 % 60;
    format!("{h:02}:{m:02}")
}

impl Client {
    /// The time of day, once the server has told us its clock and the
    /// region file has been read.
    pub fn day_time(&self) -> Option<DayTime> {
        let now = self.session.server_time()?;
        let region = self.assets.region().ok()?;
        let gt = &region.game_time;
        let fraction = fraction_of_day(now, gt.zero_time_of_year, gt.day_length);
        // The region names the parts of the day by where they start.
        let mut name = "";
        let mut is_night = false;
        for t in &gt.times_of_day {
            if t.start <= fraction {
                name = leaked(&t.name);
                is_night = t.is_night;
            }
        }
        if name.is_empty() {
            if let Some(t) = gt.times_of_day.last() {
                name = leaked(&t.name);
                is_night = t.is_night;
            }
        }
        Some(DayTime {
            fraction,
            name,
            is_night,
        })
    }
}

/// The region's names live as long as the process once read; this keeps
/// one copy of each so [`DayTime`] can stay `Copy`.
fn leaked(name: &str) -> &'static str {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let Ok(mut guard) = NAMES.lock() else {
        return "";
    };
    let set = guard.get_or_insert_with(HashSet::new);
    if let Some(s) = set.get(name) {
        return s;
    }
    let s: &'static str = Box::leak(name.to_string().into_boxed_str());
    set.insert(s);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_day_wraps_and_reads_as_a_clock() {
        // A day of 1000 s that started at 0.
        assert_eq!(fraction_of_day(0.0, 0.0, 1000.0), 0.0);
        assert!((fraction_of_day(250.0, 0.0, 1000.0) - 0.25).abs() < 1e-6);
        assert!((fraction_of_day(1250.0, 0.0, 1000.0) - 0.25).abs() < 1e-6);
        // Before the clock started, and a nonsense day length.
        assert!((fraction_of_day(-250.0, 0.0, 1000.0) - 0.75).abs() < 1e-6);
        assert_eq!(fraction_of_day(5.0, 0.0, 0.0), 0.5);
        assert_eq!(clock(0.0), "00:00");
        assert_eq!(clock(0.5), "12:00");
        assert_eq!(clock(0.25), "06:00");
        assert_eq!(clock(1.75), "18:00");
        assert_eq!(leaked("Morning"), "Morning");
        assert_eq!(leaked("Morning").as_ptr(), leaked("Morning").as_ptr());
    }
}
