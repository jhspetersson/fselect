use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use chrono::{NaiveDate, TimeZone};

fn fselect(query: &str) -> (Option<i32>, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_fselect"))
        .args(["--nocolor", query])
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run fselect");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn local_time(hour: u32, minute: u32, second: u32) -> SystemTime {
    let naive = NaiveDate::from_ymd_opt(2024, 6, 1).unwrap().and_hms_opt(hour, minute, second).unwrap();
    SystemTime::from(chrono::Local.from_local_datetime(&naive).single().unwrap())
}

fn timed_files(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (file, time) in [("midnight.txt", local_time(0, 0, 0)), ("early.txt", local_time(1, 12, 0)), ("noon.txt", local_time(12, 0, 0))] {
        let path = dir.join(file);
        fs::write(&path, "x").unwrap();
        fs::File::options().write(true).open(&path).unwrap().set_modified(time).unwrap();
    }
    dir
}

fn names(root: &Path, condition: &str) -> (Option<i32>, String, String) {
    fselect(&format!("select name from {} where {condition} order by name", root.display()))
}

#[test]
fn explicit_midnight_matches_an_instant_and_a_bare_date_spans_the_day() {
    let dir = timed_files("fselect_it_datetime_midnight");

    let explicit = names(&dir, "modified = '1 June 2024 00:00:00'");
    let after = names(&dir, "modified > '1 June 2024 00:00:00' and modified < '2 June 2024'");
    let whole_day = names(&dir, "modified = '1 June 2024'");
    let iso = names(&dir, "modified = '2024-06-01 00:00:00'");
    let _ = fs::remove_dir_all(&dir);

    for result in [&explicit, &after, &whole_day, &iso] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(explicit.1, "midnight.txt\n");
    assert_eq!(after.1, "early.txt\nnoon.txt\n");
    assert_eq!(whole_day.1, "early.txt\nmidnight.txt\nnoon.txt\n");
    assert_eq!(iso.1, "midnight.txt\n");
}

#[test]
fn twelve_hour_times_follow_the_clock() {
    let dir = timed_files("fselect_it_datetime_twelve_hour");

    let twelve_am = names(&dir, "modified = '1 June 2024 12 am'");
    let twelve_pm = names(&dir, "modified = '1 June 2024 12 pm'");
    let twelve_pm_seconds = names(&dir, "modified = '1 June 2024 12:00:00 pm'");
    let one_twelve_am = names(&dir, "modified = '1 June 2024 1:12 am'");
    let one_twelve_pm = names(&dir, "modified = '1 June 2024 1:12 pm'");
    let with_seconds = names(&dir, "modified = '1 June 2024 1:12:00 am'");
    let _ = fs::remove_dir_all(&dir);

    for result in [&twelve_am, &twelve_pm, &twelve_pm_seconds, &one_twelve_am, &one_twelve_pm, &with_seconds] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(twelve_am.1, "midnight.txt\n");
    assert_eq!(twelve_pm.1, "noon.txt\n");
    assert_eq!(twelve_pm_seconds.1, "noon.txt\n");
    assert_eq!(one_twelve_am.1, "early.txt\n", "minutes after another hour are kept");
    assert_eq!(one_twelve_pm.1, "");
    assert_eq!(with_seconds.1, "early.txt\n");
}

fn two_directory_tiff() -> Vec<u8> {
    fn ifd(entries: &[(u16, &[u8])], next: u32, data: &mut Vec<u8>, data_base: u32) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, value) in entries {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&2u16.to_le_bytes());
            out.extend_from_slice(&(value.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data_base + data.len() as u32).to_le_bytes());
            data.extend_from_slice(value);
        }
        out.extend_from_slice(&next.to_le_bytes());
        out
    }
    let primary: &[(u16, &[u8])] = &[(0x010E, b"main\0"), (0x0132, b"2024:06:01 12:00:00\0")];
    let thumbnail: &[(u16, &[u8])] = &[(0x010E, b"thumb\0"), (0x0132, b"2020:01:01 00:00:00\0")];
    let ifd_len = |n: usize| 2 + 12 * n + 4;
    let first = 8u32;
    let second = first + ifd_len(primary.len()) as u32;
    let data_base = second + ifd_len(thumbnail.len()) as u32;
    let mut data = Vec::new();
    let ifd0 = ifd(primary, second, &mut data, data_base);
    let ifd1 = ifd(thumbnail, 0, &mut data, data_base);
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II\x2a\x00");
    tiff.extend_from_slice(&first.to_le_bytes());
    tiff.extend(ifd0);
    tiff.extend(ifd1);
    tiff.extend(data);
    tiff
}

#[test]
fn exif_fields_come_from_the_primary_image_only() {
    let dir = std::env::temp_dir().join("fselect_it_datetime_exif_thumbnail");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("photo.tif"), two_directory_tiff()).unwrap();
    let root = dir.display();

    let shown = fselect(&format!("select exif_desc, exif_datetime from {root}"));
    let june = names(&dir, "exif_datetime = '2024-06-01'");
    let january = names(&dir, "exif_datetime = '2020-01-01'");
    let _ = fs::remove_dir_all(&dir);

    for result in [&shown, &june, &january] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(shown.1, "main\t2024-06-01 12:00:00\n");
    assert_eq!(june.1, "photo.tif\n");
    assert_eq!(january.1, "", "the thumbnail's date must not leak into the search");
}

fn days_ago_at_noon(days: i64) -> SystemTime {
    let day = chrono::Local::now().date_naive() - chrono::Duration::days(days);
    SystemTime::from(chrono::Local.from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap()).single().unwrap())
}

#[test]
fn relative_days_count_back_from_today() {
    let dir = std::env::temp_dir().join("fselect_it_datetime_relative");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (file, time) in [("now.txt", SystemTime::now()), ("yesterday.txt", days_ago_at_noon(1)), ("old.txt", days_ago_at_noon(3))] {
        let path = dir.join(file);
        fs::write(&path, "x").unwrap();
        fs::File::options().write(true).open(&path).unwrap().set_modified(time).unwrap();
    }

    let today = names(&dir, "modified = today");
    let yesterday = names(&dir, "modified = yesterday");
    let minus_three = names(&dir, "modified = -3");
    let recent = names(&dir, "modified >= -1");
    let older = names(&dir, "modified < '2 days ago'");
    let _ = fs::remove_dir_all(&dir);

    for result in [&today, &yesterday, &minus_three, &recent, &older] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(today.1, "now.txt\n");
    assert_eq!(yesterday.1, "yesterday.txt\n");
    assert_eq!(minus_three.1, "old.txt\n");
    assert_eq!(recent.1, "now.txt\nyesterday.txt\n");
    assert_eq!(older.1, "old.txt\n");
}

#[test]
fn date_functions_extract_and_shift_parts() {
    let dir = timed_files("fselect_it_datetime_functions");
    let root = dir.display();

    let parts = fselect(&format!(
        "select year(modified), month(modified), day(modified), dow(modified), dayname(modified), doy(modified), extract('hour', modified), extract('quarter', modified) from {root} where name = noon.txt"
    ));
    let arithmetic = fselect(&format!(
        "select date_diff(modified, '2024-01-01'), date_diff('2024-01-01', modified), date_trunc('month', modified), last_day(modified), date_add(modified, 30), date_sub(modified, 1) from {root} where name = noon.txt"
    ));
    let filtered = names(&dir, "year(modified) = 2024 and extract('hour', modified) >= 1");
    let truncated = names(&dir, "date_trunc('year', modified) = '2024-01-01'");
    let _ = fs::remove_dir_all(&dir);

    for result in [&parts, &arithmetic, &filtered, &truncated] {
        assert_eq!(result.0, Some(0), "{}", result.2);
    }
    assert_eq!(parts.1, "2024\t6\t1\t7\tSaturday\t153\t12\t2\n");
    assert_eq!(arithmetic.1, "152\t-152\t2024-06-01 00:00:00\t2024-06-30\t2024-07-01 12:00:00\t2024-05-31 12:00:00\n");
    assert_eq!(filtered.1, "early.txt\nnoon.txt\n");
    assert_eq!(truncated.1, "early.txt\nmidnight.txt\nnoon.txt\n");
}
