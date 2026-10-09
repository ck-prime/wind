//! Media Foundation H.264 MP4 sink.
//!
//! Frames are BGRA, top-down, and even in both dimensions. One sink writer
//! owns the file. `finalize` is the only path that reports success.

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFMediaType, IMFSinkWriter, MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer,
    MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Video, MFStartup,
    MFTranscodeContainerType_MPEG4, MFVideoFormat_H264, MFVideoFormat_RGB32,
    MFVideoInterlace_Progressive, MFSTARTUP_NOSOCKET, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_TRANSCODE_CONTAINERTYPE, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

pub(crate) struct H264Sink {
    writer: IMFSinkWriter,
    stream: u32,
    width: u32,
    height: u32,
    frame_ticks: i64,
    frames: u64,
    finalized: bool,
}

impl H264Sink {
    pub(crate) fn create(
        path: &Path,
        width: u32,
        height: u32,
        frames_per_second: u32,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err("H.264 frame size must be even and non-zero".to_string());
        }
        let fps = frames_per_second.max(1);
        startup()?;
        let mut wide: Vec<u16> =
            std::os::windows::ffi::OsStrExt::encode_wide(path.as_os_str()).collect();
        wide.push(0);
        unsafe {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 1).map_err(|error| error.to_string())?;
            let attributes = attributes
                .ok_or_else(|| "media foundation attributes were not created".to_string())?;
            attributes
                .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
                .map_err(|error| error.to_string())?;
            let writer = MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), None, &attributes)
                .map_err(|error| error.to_string())?;
            let output = MFCreateMediaType().map_err(|error| error.to_string())?;
            set_output_type(&output, width, height, fps)?;
            let stream = writer
                .AddStream(&output)
                .map_err(|error| error.to_string())?;
            let input = MFCreateMediaType().map_err(|error| error.to_string())?;
            set_input_type(&input, width, height, fps)?;
            writer
                .SetInputMediaType(stream, &input, None)
                .map_err(|error| error.to_string())?;
            writer.BeginWriting().map_err(|error| error.to_string())?;
            Ok(Self {
                writer,
                stream,
                width,
                height,
                frame_ticks: 10_000_000 / i64::from(fps),
                frames: 0,
                finalized: false,
            })
        }
    }

    pub(crate) fn write_bgra(&mut self, pixels: &[u8]) -> Result<(), String> {
        let expected = (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4);
        if pixels.len() < expected {
            return Err("frame is shorter than the encoder size".to_string());
        }
        unsafe {
            let buffer =
                MFCreateMemoryBuffer(expected as u32).map_err(|error| error.to_string())?;
            let mut data = std::ptr::null_mut();
            buffer
                .Lock(&mut data, None, None)
                .map_err(|error| error.to_string())?;
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), data.cast(), expected);
            buffer
                .SetCurrentLength(expected as u32)
                .map_err(|error| error.to_string())?;
            buffer.Unlock().map_err(|error| error.to_string())?;
            let sample = MFCreateSample().map_err(|error| error.to_string())?;
            sample
                .AddBuffer(&buffer)
                .map_err(|error| error.to_string())?;
            let time = self.frames as i64 * self.frame_ticks;
            sample
                .SetSampleTime(time)
                .map_err(|error| error.to_string())?;
            sample
                .SetSampleDuration(self.frame_ticks)
                .map_err(|error| error.to_string())?;
            self.writer
                .WriteSample(self.stream, &sample)
                .map_err(|error| error.to_string())?;
        }
        self.frames = self.frames.saturating_add(1);
        Ok(())
    }

    pub(crate) fn frames(&self) -> u64 {
        self.frames
    }

    pub(crate) fn finalize(mut self) -> Result<(), String> {
        unsafe {
            self.writer.Finalize().map_err(|error| error.to_string())?;
        }
        self.finalized = true;
        Ok(())
    }
}

impl Drop for H264Sink {
    fn drop(&mut self) {
        if !self.finalized {
            unsafe {
                let _ = self.writer.Finalize();
            }
        }
    }
}

fn set_output_type(output: &IMFMediaType, width: u32, height: u32, fps: u32) -> Result<(), String> {
    unsafe {
        output
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|error| error.to_string())?;
        output
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
            .map_err(|error| error.to_string())?;
        output
            .SetUINT32(&MF_MT_AVG_BITRATE, 1_500_000)
            .map_err(|error| error.to_string())?;
        output
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|error| error.to_string())?;
        output
            .SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))
            .map_err(|error| error.to_string())?;
        output
            .SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))
            .map_err(|error| error.to_string())?;
        output
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn set_input_type(input: &IMFMediaType, width: u32, height: u32, fps: u32) -> Result<(), String> {
    unsafe {
        input
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|error| error.to_string())?;
        input
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
            .map_err(|error| error.to_string())?;
        input
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|error| error.to_string())?;
        input
            .SetUINT32(&MF_MT_DEFAULT_STRIDE, width.saturating_mul(4))
            .map_err(|error| error.to_string())?;
        input
            .SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))
            .map_err(|error| error.to_string())?;
        input
            .SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))
            .map_err(|error| error.to_string())?;
        input
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn pack(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

fn startup() -> Result<(), String> {
    static STARTED: std::sync::Mutex<Option<Result<(), String>>> = std::sync::Mutex::new(None);
    let mut slot = STARTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = slot.as_ref() {
        return existing.clone();
    }
    let outcome = unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|error| error.to_string())
    };
    *slot = Some(outcome.clone());
    outcome
}
