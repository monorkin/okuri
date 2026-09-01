use std::sync::OnceLock;

use time::OffsetDateTime;

/// The clock this machine keeps, captured before anything else starts.
///
/// `time` refuses to read the local offset once a process has more than one thread, because the
/// answer it gives could already be stale — so it is asked once, first thing in `main`, and
/// remembered. Servers report modification times in UTC, and showing those unconverted puts
/// every file an hour or several out from what the person's own file manager says.
static LOCAL: OnceLock<time::UtcOffset> = OnceLock::new();

/// Called once, from `main`, before any thread is started.
pub fn remember_local_clock() {
    let _ = LOCAL.set(time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC));
}

/// A time as the person reading it keeps time.
pub fn local(at: OffsetDateTime) -> OffsetDateTime {
    at.to_offset(*LOCAL.get().unwrap_or(&time::UtcOffset::UTC))
}

pub fn now() -> OffsetDateTime {
    local(OffsetDateTime::now_utc())
}

/// A file size the way a file manager writes it.
///
/// Powers of 1024 with the short units people actually read, and one decimal place only where
/// it tells you something — "1.4 MB" is useful, "1.0 MB" is noise.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];

    if bytes < 1024 {
        return match bytes {
            1 => "1 byte".to_owned(),
            _ => format!("{bytes} bytes"),
        };
    }

    let mut size = bytes as f64;
    let mut unit = 0;

    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }

    if size < 10.0 && size.fract() >= 0.05 {
        format!("{:.1} {}", size, UNITS[unit])
    } else {
        format!("{:.0} {}", size, UNITS[unit])
    }
}

/// A modification time, in the shape people scan a file list for: a clock time for today,
/// a weekday for this week, and a date beyond that.
pub fn modified(at: OffsetDateTime, now: OffsetDateTime) -> String {
    let elapsed = now - at;

    if at.date() == now.date() {
        format!("{:02}:{:02}", at.hour(), at.minute())
    } else if elapsed.whole_days() < 7 && elapsed.is_positive() {
        at.weekday().to_string()
    } else if at.year() == now.year() {
        format!("{} {}", at.day(), month(at))
    } else {
        format!("{} {} {}", at.day(), month(at), at.year())
    }
}

fn month(at: OffsetDateTime) -> &'static str {
    use time::Month::*;

    match at.month() {
        January => "Jan",
        February => "Feb",
        March => "Mar",
        April => "Apr",
        May => "May",
        June => "Jun",
        July => "Jul",
        August => "Aug",
        September => "Sep",
        October => "Oct",
        November => "Nov",
        December => "Dec",
    }
}


/// A file:// URL as a path, which is what a drop from a file manager hands us.
pub fn path_from_url(url: &str) -> Option<std::path::PathBuf> {
    let path = url.strip_prefix("file://")?;

    // A drop carries percent-encoded bytes, and a file named "my file.txt" is entirely normal.
    let mut decoded = Vec::with_capacity(path.len());
    let mut characters = path.bytes();

    while let Some(byte) = characters.next() {
        if byte == b'%' {
            let digits = [characters.next()?, characters.next()?];
            let text = std::str::from_utf8(&digits).ok()?;

            decoded.push(u8::from_str_radix(text, 16).ok()?);
        } else {
            decoded.push(byte);
        }
    }

    Some(std::path::PathBuf::from(String::from_utf8(decoded).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn sizes_read_the_way_a_file_manager_writes_them() {
        assert_eq!(size(0), "0 bytes");
        assert_eq!(size(1), "1 byte");
        assert_eq!(size(999), "999 bytes");
        assert_eq!(size(1024), "1 KB");
        assert_eq!(size(1536), "1.5 KB");
        assert_eq!(size(250_000), "244 KB");
        assert_eq!(size(1_500_000), "1.4 MB");
        assert_eq!(size(9_000_000_000), "8.4 GB");
    }

    #[test]
    fn times_get_shorter_the_more_recent_they_are() {
        let now = datetime!(2026-08-28 14:30 UTC);

        assert_eq!(modified(datetime!(2026-08-28 09:05 UTC), now), "09:05");
        assert_eq!(modified(datetime!(2026-08-26 09:05 UTC), now), "Wednesday");
        assert_eq!(modified(datetime!(2026-03-02 09:05 UTC), now), "2 Mar");
        assert_eq!(modified(datetime!(2024-12-24 09:05 UTC), now), "24 Dec 2024");
    }

    #[test]
    fn a_file_from_the_future_is_shown_as_a_date_not_a_weekday() {
        let now = datetime!(2026-08-28 14:30 UTC);

        assert_eq!(modified(datetime!(2026-09-04 09:05 UTC), now), "4 Sep");
    }

    #[test]
    fn dropped_urls_become_paths() {
        assert_eq!(
            path_from_url("file:///home/me/notes.txt"),
            Some("/home/me/notes.txt".into())
        );
        assert_eq!(
            path_from_url("file:///home/me/my%20file.txt"),
            Some("/home/me/my file.txt".into())
        );
        assert_eq!(
            path_from_url("file:///home/me/caf%C3%A9.txt"),
            Some("/home/me/café.txt".into())
        );
        assert_eq!(path_from_url("https://example.com/notes.txt"), None);
    }
}
