//! Pluto — native egui frontend.
//!
//! Native egui migration of the `Frontdoor` web editor. The Frontdoor files
//! remain the visual and behavior reference while the native editing surface
//! is brought to feature parity.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod icons;
mod theme;

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, FALSE, HANDLE},
    System::Threading::CreateMutexW,
    UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE},
};

fn main() -> eframe::Result<()> {
    #[cfg(windows)]
    let _instance_guard = match claim_single_instance() {
        Ok(Some(guard)) => guard,
        Ok(None) => return Ok(()),
        Err(error) => {
            show_startup_error("Pluto could not start", &error);
            return Ok(());
        }
    };
    #[cfg(not(windows))]
    let _instance_guard = ();

    install_startup_panic_hook();
    log_startup_event(&format!(
        "Starting Pluto (pid {}, renderer: OpenGL)",
        std::process::id()
    ));

    let options = eframe::NativeOptions {
        // OpenGL keeps the desktop shell off the heavy wgpu adapter startup
        // path. egui paints the same primitives and colors with either backend.
        renderer: eframe::Renderer::Glow,
        centered: true,
        viewport: egui::ViewportBuilder::default()
            .with_title("Pluto")
            .with_app_id("pluto")
<<<<<<< Updated upstream
=======
            .with_visible(true)
>>>>>>> Stashed changes
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1000.0, 640.0])
            // The web frontend draws its own title bar (`.ed-top`) and its own
            // min/max/close buttons, so the OS decorations are switched off to
            // keep the layout identical.
            .with_decorations(false),
        ..Default::default()
    };

    let result = eframe::run_native(
        "Pluto",
        options,
        Box::new(|cc| {
            log_startup_event("eframe app factory started");
            let app = app::PlutoApp::new(cc);
            log_startup_event("Pluto editor state initialized");
            Ok(Box::new(app))
        }),
    );
    if let Err(error) = &result {
        log_startup_event(&format!("Native window startup failed: {error}"));
        show_startup_error("Pluto could not open", &error.to_string());
    } else {
        log_startup_event("Native window closed normally");
    }
    result
}

fn startup_log_path() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Pluto")
        .join("startup.log")
}

pub(crate) fn log_startup_event(message: &str) {
    use std::io::Write;

    let path = startup_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(log, "{} {message}", startup_timestamp());
    }
}

fn startup_timestamp() -> String {
    // Keep startup diagnostics dependency-free. UNIX time is enough to order
    // attempts and can still be interpreted if the local clock is incorrect.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| format!("[{}]", duration.as_secs()))
        .unwrap_or_else(|_| "[time unavailable]".to_owned())
}

fn install_startup_panic_hook() {
    std::panic::set_hook(Box::new(|panic| {
        log_startup_event(&format!("Unhandled panic: {panic}"));
    }));
}

#[cfg(windows)]
struct SingleInstanceGuard(HANDLE);

#[cfg(windows)]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn claim_single_instance() -> Result<Option<SingleInstanceGuard>, String> {
    let mutex_name: Vec<u16> = "Local\\Pluto.Desktop.Singleton"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), FALSE, mutex_name.as_ptr()) };
    if handle.is_null() {
        let error = unsafe { GetLastError() };
        return Err(format!(
            "Could not create Pluto's single-instance lock (Windows error {error})."
        ));
    }

    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        focus_existing_window();
        unsafe { CloseHandle(handle); }
        return Ok(None);
    }
    Ok(Some(SingleInstanceGuard(handle)))
}

#[cfg(windows)]
fn focus_existing_window() {
    use std::{
        thread,
        time::{Duration, Instant},
    };

    let title: Vec<u16> = "Pluto".encode_utf16().chain(std::iter::once(0)).collect();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if !window.is_null() {
            unsafe {
                ShowWindow(window, SW_RESTORE);
                SetForegroundWindow(window);
            }
            return;
        }
        if Instant::now() >= deadline {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn show_startup_error(title: &str, description: &str) {
    let _ = rfd::MessageDialog::new()
        .set_title(title)
        .set_description(description)
        .set_level(rfd::MessageLevel::Error)
        .show();
}
