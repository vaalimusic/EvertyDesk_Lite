#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsAppUserModelId {
    Launcher,
    Viewer,
}

impl WindowsAppUserModelId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Launcher => "Everty.EvertyDesk.DesktopNext.Launcher",
            Self::Viewer => "Everty.EvertyDesk.DesktopNext.Viewer",
        }
    }
}

#[cfg(windows)]
pub fn set_current_process_app_user_model_id(app_id: WindowsAppUserModelId) {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    let app_id = HSTRING::from(app_id.as_str());
    if let Err(error) = unsafe { SetCurrentProcessExplicitAppUserModelID(&app_id) } {
        eprintln!(
            "[windows-app] failed to set AppUserModelID {}: {error}",
            app_id.to_string_lossy()
        );
    }
}

#[cfg(not(windows))]
pub fn set_current_process_app_user_model_id(_app_id: WindowsAppUserModelId) {}

/// Excludes a window from screen/DXGI capture
/// (`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`, Windows 10 2004+).
///
/// The launcher's own window sits inside the region Desktop Duplication
/// captures while hosting is active. Without this, every repaint of the
/// launcher's own UI looks like "the screen changed" to the change-detector,
/// so the encoder never sees a static frame — continuous re-encode at full
/// rate, i.e. the "100% CPU/GPU while the window is visible, normal while
/// minimized" symptom. This makes any capture (ours or a third party's)
/// render the window as a static black rectangle instead, so the launcher's
/// own repaints can never register as motion again.
///
/// `hwnd` is the raw native window handle value (from
/// `raw_window_handle::Win32WindowHandle::hwnd`), not an iced `window::Id`.
/// Returns `false` (logged, not fatal) on Windows versions before 2004 where
/// this flag doesn't exist, and always on non-Windows platforms.
#[cfg(windows)]
pub fn exclude_window_from_capture(hwnd: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    };

    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    match unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) } {
        Ok(()) => true,
        Err(error) => {
            eprintln!("[windows-app] SetWindowDisplayAffinity failed: {error}");
            false
        }
    }
}

#[cfg(not(windows))]
pub fn exclude_window_from_capture(_hwnd: isize) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_user_model_ids_are_stable_and_namespaced() {
        assert_eq!(
            WindowsAppUserModelId::Launcher.as_str(),
            "Everty.EvertyDesk.DesktopNext.Launcher"
        );
        assert_eq!(
            WindowsAppUserModelId::Viewer.as_str(),
            "Everty.EvertyDesk.DesktopNext.Viewer"
        );
        assert_ne!(
            WindowsAppUserModelId::Launcher.as_str(),
            WindowsAppUserModelId::Viewer.as_str()
        );
    }
}
