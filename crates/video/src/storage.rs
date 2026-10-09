//! Local video files under the application data directory.
//!
//! The provider writes the encoded bytes. This module only reserves a name
//! and removes a file that was not finalized.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::contract::VideoError;

/// Directory beneath an application data directory.
#[derive(Clone, Debug)]
pub struct VideoStorage {
    directory: PathBuf,
}

impl VideoStorage {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// Reserve a new `.mp4` path. The file is not created.
    pub fn reserve(&self) -> Result<PathBuf, VideoError> {
        if !self.directory.is_absolute() {
            return Err(storage_error(
                "video directory must be an absolute user data path",
            ));
        }
        fs::create_dir_all(&self.directory)
            .map_err(|_| storage_error("video directory could not be created"))?;
        restrict_directory(&self.directory)?;
        let stamp = format_stamp(SystemTime::now());
        for sequence in 0..1000 {
            let path = self.directory.join(file_name(&stamp, sequence));
            if !path.exists() {
                return Ok(path);
            }
        }
        Err(storage_error("video file name collided too many times"))
    }

    /// Remove a file that was not successfully finalized.
    pub fn discard(&self, path: &Path) {
        let _ = fs::remove_file(path);
    }
}

fn restrict_directory(path: &Path) -> Result<(), VideoError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| storage_error("video directory permissions could not be set"))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn storage_error(reason: &str) -> VideoError {
    VideoError::StorageFailed {
        reason: reason.to_string(),
    }
}

fn file_name(stamp: &str, sequence: u32) -> String {
    if sequence == 0 {
        format!("{stamp}.mp4")
    } else {
        format!("{stamp}-{sequence}.mp4")
    }
}

fn format_stamp(time: SystemTime) -> String {
    let millis = time
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let seconds = (millis / 1000) as u64;
    let time_of_day = seconds % 86_400;
    let hour = time_of_day / 3_600;
    let minute = (time_of_day % 3_600) / 60;
    let second = time_of_day % 60;
    let milliseconds = (millis % 1000) as u32;
    let (year, month, day) = civil_from_days(i64::try_from(seconds / 86_400).unwrap_or(0));
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}-{milliseconds:03}")
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    };
    let year = year_of_era as i64 + era * 400 + i64::from(month <= 2);
    (
        i32::try_from(year).unwrap_or(1970),
        month as u32,
        day as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::{format_stamp, VideoStorage};
    use std::fs;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn stamp_uses_utc() {
        assert_eq!(format_stamp(UNIX_EPOCH), "19700101-000000-000");
        assert_eq!(
            format_stamp(UNIX_EPOCH + Duration::from_millis(1_500)),
            "19700101-000001-500"
        );
    }

    #[test]
    fn reserve_does_not_overwrite() {
        let directory =
            std::env::temp_dir().join(format!("wind-video-store-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let storage = VideoStorage::new(directory.clone());
        let first = storage.reserve().expect("reserve");
        fs::write(&first, b"keep").expect("write");
        let second = storage.reserve().expect("second");
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).expect("read"), b"keep");
        assert!(!second.exists());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn relative_directory_is_rejected() {
        let error = VideoStorage::new("videos".into())
            .reserve()
            .expect_err("relative");
        let text = error.to_string();
        assert!(text.contains("absolute"), "{text}");
    }
}
