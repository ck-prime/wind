//! Local screenshot files under the application data directory.
//!
//! Files are user-scoped. Logs do not include the file name or the pixels.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::contract::{ImageFormat, Screenshot, ScreenshotError, PNG_SIGNATURE};

/// Where one capture was written. The path is for the caller. It is not logged.
pub struct StoredScreenshot {
    pub captured_at: SystemTime,
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    pub byte_len: u64,
    pub path: PathBuf,
}

impl std::fmt::Debug for StoredScreenshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredScreenshot")
            .field("captured_at", &self.captured_at)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("byte_len", &self.byte_len)
            .finish()
    }
}

/// Directory beneath an application data directory.
#[derive(Clone, Debug)]
pub struct ScreenshotStorage {
    directory: PathBuf,
}

impl ScreenshotStorage {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// Write `image` once. An existing file is left unchanged.
    pub fn store(&self, image: Screenshot) -> Result<StoredScreenshot, ScreenshotError> {
        validate_png(&image)?;
        if !self.directory.is_absolute() {
            return Err(storage_error(
                "screenshot directory must be an absolute user data path",
            ));
        }
        fs::create_dir_all(&self.directory)
            .map_err(|_| storage_error("screenshot directory could not be created"))?;
        restrict_directory(&self.directory)?;
        let stamp = format_stamp(image.captured_at);
        let byte_len = u64::try_from(image.data.len()).unwrap_or(u64::MAX);
        for sequence in 0..1000 {
            let name = file_name(&stamp, sequence);
            let path = self.directory.join(name);
            match write_new_file(&path, &image.data) {
                Ok(()) => {
                    return Ok(StoredScreenshot {
                        captured_at: image.captured_at,
                        width: image.width,
                        height: image.height,
                        format: image.format,
                        byte_len,
                        path,
                    });
                }
                Err(WriteError::Exists) => continue,
                Err(WriteError::Failed) => {
                    remove_incomplete(&path);
                    return Err(storage_error("screenshot file could not be written"));
                }
            }
        }
        Err(storage_error(
            "screenshot file name collided too many times",
        ))
    }
}

fn validate_png(image: &Screenshot) -> Result<(), ScreenshotError> {
    if image.format != ImageFormat::Png {
        return Err(ScreenshotError::EncodingFailed {
            reason: "screenshot format is not png".to_string(),
        });
    }
    if image.width == 0 || image.height == 0 {
        return Err(ScreenshotError::EncodingFailed {
            reason: "screenshot dimensions are empty".to_string(),
        });
    }
    if image.data.len() < PNG_SIGNATURE.len() || !image.data.starts_with(PNG_SIGNATURE) {
        return Err(ScreenshotError::EncodingFailed {
            reason: "screenshot bytes are not a png".to_string(),
        });
    }
    Ok(())
}

enum WriteError {
    Exists,
    Failed,
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(WriteError::Exists);
        }
        Err(_) => return Err(WriteError::Failed),
    };
    if file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .is_err()
    {
        drop(file);
        remove_incomplete(path);
        return Err(WriteError::Failed);
    }
    Ok(())
}

fn remove_incomplete(path: &Path) {
    let _ = fs::remove_file(path);
}

fn restrict_directory(path: &Path) -> Result<(), ScreenshotError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| storage_error("screenshot directory permissions could not be set"))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn storage_error(reason: &str) -> ScreenshotError {
    ScreenshotError::StorageFailed {
        reason: reason.to_string(),
    }
}

fn file_name(stamp: &str, sequence: u32) -> String {
    if sequence == 0 {
        format!("{stamp}.png")
    } else {
        format!("{stamp}-{sequence}.png")
    }
}

pub(crate) fn format_stamp(time: SystemTime) -> String {
    let millis = time
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let seconds = (millis / 1000) as u64;
    let milliseconds = (millis % 1000) as u32;
    let days = seconds / 86_400;
    let time_of_day = seconds % 86_400;
    let hour = time_of_day / 3_600;
    let minute = (time_of_day % 3_600) / 60;
    let second = time_of_day % 60;
    let (year, month, day) = civil_from_days(i64::try_from(days).unwrap_or(0));
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}-{milliseconds:03}")
}

/// Howard Hinnant's civil-from-days conversion, shifted to the Unix epoch.
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
    use super::{format_stamp, ScreenshotStorage};
    use crate::contract::{sample_screenshot, ImageFormat, ScreenshotError, PNG_SIGNATURE};
    use std::fs;
    use std::time::{Duration, UNIX_EPOCH};

    fn storage(label: &str) -> (ScreenshotStorage, std::path::PathBuf) {
        let directory =
            std::env::temp_dir().join(format!("wind-screenshot-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        (ScreenshotStorage::new(directory.clone()), directory)
    }

    #[test]
    fn stamp_uses_utc_and_milliseconds() {
        assert_eq!(format_stamp(UNIX_EPOCH), "19700101-000000-000");
        assert_eq!(
            format_stamp(UNIX_EPOCH + Duration::from_millis(1)),
            "19700101-000000-001"
        );
        assert_eq!(
            format_stamp(UNIX_EPOCH + Duration::from_millis(1_500)),
            "19700101-000001-500"
        );
    }

    #[test]
    fn store_writes_png_without_replacing_an_existing_file() {
        let (storage, directory) = storage("store");
        let first = storage.store(sample_screenshot()).expect("first store");
        let marker = b"existing";
        fs::write(&first.path, marker).expect("marker");
        let second = storage.store(sample_screenshot()).expect("second store");
        assert_ne!(first.path, second.path);
        assert_eq!(fs::read(&first.path).expect("marker bytes"), marker);
        assert!(second.path.starts_with(&directory));
        assert!(second.path.extension().is_some_and(|ext| ext == "png"));
        let bytes = fs::read(&second.path).expect("png");
        assert!(bytes.starts_with(PNG_SIGNATURE));
        assert_eq!(second.width, 1);
        assert_eq!(second.height, 1);
        assert_eq!(second.format, ImageFormat::Png);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn invalid_png_creates_no_file() {
        let (storage, directory) = storage("invalid");
        let mut image = sample_screenshot();
        image.data = b"not a png".to_vec();
        let error = storage.store(image).expect_err("encoding");
        assert!(matches!(error, ScreenshotError::EncodingFailed { .. }));
        assert!(!directory.exists());
    }

    #[test]
    fn unavailable_destination_leaves_no_partial_file() {
        let parent =
            std::env::temp_dir().join(format!("wind-screenshot-file-{}", std::process::id()));
        let _ = fs::remove_file(&parent);
        fs::write(&parent, b"not a directory").expect("file");
        let storage = ScreenshotStorage::new(parent.clone());
        let error = storage.store(sample_screenshot()).expect_err("storage");
        assert!(matches!(error, ScreenshotError::StorageFailed { .. }));
        assert_eq!(fs::read(&parent).expect("original"), b"not a directory");
        let _ = fs::remove_file(&parent);
    }

    #[test]
    fn relative_directory_is_rejected() {
        let storage = ScreenshotStorage::new(std::path::PathBuf::from("screenshots"));
        let error = storage.store(sample_screenshot()).expect_err("relative");
        assert!(matches!(error, ScreenshotError::StorageFailed { .. }));
    }

    #[test]
    fn incomplete_file_is_removed() {
        let directory =
            std::env::temp_dir().join(format!("wind-screenshot-partial-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("dir");
        let partial = directory.join("partial.png");
        fs::write(&partial, b"partial").expect("partial");
        super::remove_incomplete(&partial);
        assert!(!partial.exists());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn zero_dimensions_are_rejected() {
        let (storage, directory) = storage("dimensions");
        let mut image = sample_screenshot();
        image.width = 0;
        let error = storage.store(image).expect_err("dimensions");
        assert!(matches!(error, ScreenshotError::EncodingFailed { .. }));
        assert!(!directory.exists());
    }
}
