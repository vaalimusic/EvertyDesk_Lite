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
