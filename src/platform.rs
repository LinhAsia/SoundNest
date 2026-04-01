#[cfg(windows)]
mod windows_platform {
    use eframe::Frame;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::HWND,
        Graphics::Dwm::{
            DWMNCRENDERINGPOLICY, DWMNCRP_DISABLED, DWMNCRP_ENABLED, DWMWA_NCRENDERING_POLICY,
            DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT, DWMWCP_ROUND,
            DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
        },
        UI::{
            Controls::MARGINS,
            WindowsAndMessaging::{
                FindWindowW, GWL_EXSTYLE, GWL_STYLE, GetWindowLongW, HWND_TOPMOST,
                SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
                SetWindowLongW, SetWindowPos, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
                WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
            },
        },
    };
    use windows::core::PCWSTR;

    pub fn set_native_window_shadow(frame: &Frame, enabled: bool) {
        let Ok(window_handle) = frame.window_handle() else {
            return;
        };
        let hwnd = match window_handle.as_raw() {
            RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as *mut _),
            _ => return,
        };

        unsafe {
            let policy: DWMNCRENDERINGPOLICY = if enabled {
                DWMNCRP_ENABLED
            } else {
                DWMNCRP_DISABLED
            };
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_NCRENDERING_POLICY,
                &policy as *const _ as *const _,
                std::mem::size_of_val(&policy) as u32,
            );

            let corner = if enabled {
                DWMWCP_ROUND
            } else {
                DWMWCP_DEFAULT
            };
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner as *const _ as *const _,
                std::mem::size_of_val(&corner) as u32,
            );
        }
    }

    pub fn set_overlay_window_native_visuals(
        window_title: &str,
        enabled: bool,
        popup_only: bool,
    ) -> bool {
        let mut title = window_title.encode_utf16().collect::<Vec<_>>();
        title.push(0);

        let Ok(hwnd) = (unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) }) else {
            return false;
        };
        if hwnd.0.is_null() {
            return false;
        }

        unsafe {
            let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let stripped = style
                & !WS_CAPTION.0
                & !WS_THICKFRAME.0
                & !WS_MINIMIZEBOX.0
                & !WS_MAXIMIZEBOX.0
                & !WS_SYSMENU.0;
            let popup_style = if popup_only {
                (stripped & !WS_POPUP.0) | WS_POPUP.0
            } else {
                stripped
            };
            let popup_ex_style = (ex_style & !WS_EX_APPWINDOW.0) | WS_EX_TOOLWINDOW.0;
            let _ = SetWindowLongW(hwnd, GWL_STYLE, popup_style as i32);
            let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, popup_ex_style as i32);

            let margins = MARGINS {
                cxLeftWidth: -1,
                cxRightWidth: -1,
                cyTopHeight: -1,
                cyBottomHeight: -1,
            };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);

            let policy: DWMNCRENDERINGPOLICY = if enabled {
                DWMNCRP_ENABLED
            } else {
                DWMNCRP_DISABLED
            };
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_NCRENDERING_POLICY,
                &policy as *const _ as *const _,
                std::mem::size_of_val(&policy) as u32,
            );

            let corner = if enabled {
                DWMWCP_ROUND
            } else {
                DWMWCP_DEFAULT
            };
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner as *const _ as *const _,
                std::mem::size_of_val(&corner) as u32,
            );

            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
        }

        true
    }
}

#[cfg(windows)]
pub use windows_platform::*;

#[cfg(not(windows))]
pub fn set_native_window_shadow(_frame: &eframe::Frame, _enabled: bool) {}

#[cfg(not(windows))]
pub fn set_overlay_window_native_visuals(
    _window_title: &str,
    _enabled: bool,
    _popup_only: bool,
) -> bool {
    false
}
