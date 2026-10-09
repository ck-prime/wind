//! One Windows Graphics Capture session for the primary monitor.
//!
//! `xcap` opens and closes a capture session on every screenshot. Windows draws
//! the capture border when that session starts and removes it when the session
//! closes, which flashes the desktop on each interval. This session stays open,
//! turns the border off before capture starts, and copies one fresh frame when
//! a screenshot is requested. It does not change the display mode, move a
//! window, or inject input.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use wind_core::{emit, Level, LogFields};
use windows::core::Interface;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;

const FRAME_WAIT: Duration = Duration::from_secs(2);

struct Devices {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
}

struct PrimarySession {
    monitor: isize,
    pool: Direct3D11CaptureFramePool,
    session: windows::Graphics::Capture::GraphicsCaptureSession,
    devices: Devices,
}

pub fn capture_primary_rgba() -> Result<(Vec<u8>, u32, u32), String> {
    let mut slot = session_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if slot
        .as_ref()
        .is_some_and(|session| !session.matches_primary())
    {
        *slot = None;
    }
    if slot.is_none() {
        *slot = Some(PrimarySession::open()?);
    }
    let session = slot
        .as_ref()
        .ok_or_else(|| "capture session is not open".to_string())?;
    session.next_frame()
}

fn session_slot() -> &'static Mutex<Option<PrimarySession>> {
    static SLOT: OnceLock<Mutex<Option<PrimarySession>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

impl PrimarySession {
    fn open() -> Result<Self, String> {
        ensure_winrt()?;
        let devices = create_devices()?;
        let monitor = primary_monitor()?;
        let item = capture_item(monitor)?;
        let size = item
            .Size()
            .map_err(|error| format!("capture item size is unavailable: {error}"))?;
        let width = u32::try_from(size.Width).unwrap_or(0);
        let height = u32::try_from(size.Height).unwrap_or(0);
        if width == 0 || height == 0 {
            return Err("the primary display has no pixel size".to_string());
        }
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&devices.dxgi()?) }
            .map_err(|error| format!("Direct3D device bridge failed: {error}"))?;
        let device: IDirect3DDevice = inspectable
            .cast()
            .map_err(|error| format!("IDirect3DDevice cast failed: {error}"))?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|error| format!("capture frame pool failed: {error}"))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|error| format!("capture session failed: {error}"))?;
        let border_disabled = session.SetIsBorderRequired(false).is_ok()
            && session.IsBorderRequired().ok() == Some(false);
        note_border(border_disabled);
        let _ = session.SetIsCursorCaptureEnabled(false);
        session
            .StartCapture()
            .map_err(|error| format!("capture session did not start: {error}"))?;
        Ok(Self {
            monitor: monitor.0 as isize,
            pool,
            session,
            devices,
        })
    }

    fn matches_primary(&self) -> bool {
        primary_monitor()
            .ok()
            .is_some_and(|monitor| monitor.0 as isize == self.monitor)
    }

    fn next_frame(&self) -> Result<(Vec<u8>, u32, u32), String> {
        discard_queued_frames(&self.pool);
        let started = Instant::now();
        loop {
            if let Some(frame) = next_queued_frame(&self.pool) {
                let copied = copy_frame(&frame, &self.devices);
                let _ = frame.Close();
                return copied;
            }
            if started.elapsed() >= FRAME_WAIT {
                return Err("timed out waiting for a capture frame".to_string());
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for PrimarySession {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

fn discard_queued_frames(pool: &Direct3D11CaptureFramePool) {
    for _ in 0..8 {
        let Some(frame) = next_queued_frame(pool) else {
            return;
        };
        let _ = frame.Close();
    }
}

fn next_queued_frame(
    pool: &Direct3D11CaptureFramePool,
) -> Option<windows::Graphics::Capture::Direct3D11CaptureFrame> {
    pool.TryGetNextFrame().ok()
}

fn copy_frame(
    frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
    devices: &Devices,
) -> Result<(Vec<u8>, u32, u32), String> {
    let surface = frame
        .Surface()
        .map_err(|error| format!("capture frame surface failed: {error}"))?;
    let access: IDirect3DDxgiInterfaceAccess = surface
        .cast()
        .map_err(|error| format!("capture surface access failed: {error}"))?;
    let source: ID3D11Texture2D = unsafe { access.GetInterface() }
        .map_err(|error| format!("capture texture is unavailable: {error}"))?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { source.GetDesc(&mut desc) };
    let width = desc.Width;
    let height = desc.Height;
    if width == 0 || height == 0 {
        return Err("captured frame has no pixel size".to_string());
    }
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    desc.MiscFlags = 0;
    let mut staging = None;
    unsafe {
        devices
            .device
            .CreateTexture2D(&desc, None, Some(&mut staging))
    }
    .map_err(|error| format!("staging texture failed: {error}"))?;
    let staging = staging.ok_or_else(|| "staging texture was not created".to_string())?;
    unsafe {
        devices
            .context
            .CopyResource(&staging, &source as &ID3D11Resource);
    }
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        devices
            .context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|error| format!("staging texture map failed: {error}"))?;
    }
    let rgba = copy_bgra_rows(&mapped, width, height);
    unsafe { devices.context.Unmap(&staging, 0) };
    rgba.map(|pixels| (pixels, width, height))
}

fn copy_bgra_rows(
    mapped: &D3D11_MAPPED_SUBRESOURCE,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let row_bytes = (width as usize).saturating_mul(4);
    let pitch = mapped.RowPitch as usize;
    if mapped.pData.is_null() || pitch < row_bytes {
        return Err("captured frame mapping is incomplete".to_string());
    }
    let mut rgba = vec![0u8; row_bytes.saturating_mul(height as usize)];
    unsafe {
        let source = mapped.pData as *const u8;
        for y in 0..height as usize {
            let Some(offset) = y.checked_mul(pitch) else {
                return Err("captured frame mapping is incomplete".to_string());
            };
            let row = std::slice::from_raw_parts(source.add(offset), row_bytes);
            for x in 0..width as usize {
                let input = x * 4;
                let output = (y * width as usize + x) * 4;
                rgba[output] = row[input + 2];
                rgba[output + 1] = row[input + 1];
                rgba[output + 2] = row[input];
                rgba[output + 3] = row[input + 3];
            }
        }
    }
    Ok(rgba)
}

fn capture_item(monitor: HMONITOR) -> Result<GraphicsCaptureItem, String> {
    let interop: IGraphicsCaptureItemInterop =
        windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(|error| format!("capture interop is unavailable: {error}"))?;
    unsafe { interop.CreateForMonitor(monitor) }
        .map_err(|error| format!("monitor capture item failed: {error}"))
}

fn primary_monitor() -> Result<HMONITOR, String> {
    let mut monitors = Vec::new();
    let enumerated = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(monitor_enum),
            LPARAM(&mut monitors as *mut Vec<HMONITOR> as isize),
        )
    };
    if !enumerated.as_bool() {
        let desktop = unsafe { GetDesktopWindow() };
        let fallback = unsafe { MonitorFromWindow(desktop, MONITOR_DEFAULTTOPRIMARY) };
        if fallback.is_invalid() {
            return Err("no primary display is available".to_string());
        }
        return Ok(fallback);
    }
    for monitor in monitors {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..MONITORINFO::default()
        };
        let found = unsafe { GetMonitorInfoW(monitor, &mut info) };
        if found.as_bool() && (info.dwFlags & 1) != 0 {
            return Ok(monitor);
        }
    }
    Err("no primary display is available".to_string())
}

unsafe extern "system" fn monitor_enum(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> windows::core::BOOL {
    let monitors = &mut *(data.0 as *mut Vec<HMONITOR>);
    monitors.push(monitor);
    windows::core::BOOL(1)
}

fn create_devices() -> Result<Devices, String> {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|error| format!("D3D11 device failed: {error}"))?;
    }
    let device = device.ok_or_else(|| "D3D11 device was not created".to_string())?;
    let context = context.ok_or_else(|| "D3D11 context was not created".to_string())?;
    Ok(Devices { device, context })
}

impl Devices {
    fn dxgi(&self) -> Result<IDXGIDevice, String> {
        self.device
            .cast()
            .map_err(|error| format!("DXGI device cast failed: {error}"))
    }
}

fn ensure_winrt() -> Result<(), String> {
    static READY: OnceLock<Result<(), String>> = OnceLock::new();
    READY
        .get_or_init(|| unsafe {
            match RoInitialize(RO_INIT_MULTITHREADED) {
                Ok(()) => Ok(()),
                Err(error) if error.code().0 == 0x8001_0106_u32 as i32 => Ok(()),
                Err(error) => Err(format!("WinRT initialization failed: {error}")),
            }
        })
        .clone()
}

fn note_border(disabled: bool) {
    static NOTED: AtomicBool = AtomicBool::new(false);
    if NOTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let message = if disabled {
        "capture_border_disabled"
    } else {
        "capture_border_required"
    };
    emit(&LogFields {
        level: Level::Info,
        component: "screenshot",
        service_id: Some("screenshot"),
        version: None,
        platform: Some("windows"),
        message,
        error: None,
        state: Some("Running"),
    });
}
