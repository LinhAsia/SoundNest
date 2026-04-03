#[cfg(windows)]
mod windows_platform {
    use anyhow::{Context, Result, bail};
    use eframe::Frame;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::Win32::{
        Foundation::{
            DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, S_OK,
        },
        Graphics::Dwm::{
            DWMNCRENDERINGPOLICY, DWMNCRP_DISABLED, DWMNCRP_ENABLED, DWMWA_NCRENDERING_POLICY,
            DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT, DWMWCP_ROUND,
            DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
        },
        System::{
            Ole::{DROPEFFECT_COPY, IDropSource, IDropSource_Impl, OleInitialize, OleUninitialize},
            SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS},
        },
        UI::{
            Controls::MARGINS,
            Shell::{
                CIDLData_CreateFromIDArray, ILClone, ILCreateFromPathW, ILFindLastID, ILFree,
                ILRemoveLastID, SHDoDragDrop,
            },
            WindowsAndMessaging::{
                FindWindowW, GWL_EXSTYLE, GWL_STYLE, GetWindowLongW, HWND_NOTOPMOST, HWND_TOPMOST,
                IDC_ARROW, LoadCursorW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
                SWP_NOOWNERZORDER, SWP_NOSIZE, SetCursor, SetWindowLongW, SetWindowPos, WS_CAPTION,
                WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP,
                WS_SYSMENU, WS_THICKFRAME,
            },
        },
    };
    use windows::core::{PCWSTR, implement};

    #[implement(IDropSource)]
    struct FileDropSource;

    #[allow(non_snake_case)]
    impl IDropSource_Impl for FileDropSource_Impl {
        fn QueryContinueDrag(
            &self,
            fescapepressed: windows_core::BOOL,
            grfkeystate: MODIFIERKEYS_FLAGS,
        ) -> windows_core::HRESULT {
            if fescapepressed.as_bool() {
                DRAGDROP_S_CANCEL
            } else if (grfkeystate & MK_LBUTTON).0 == 0 {
                DRAGDROP_S_DROP
            } else {
                S_OK
            }
        }

        fn GiveFeedback(
            &self,
            _dweffect: windows::Win32::System::Ole::DROPEFFECT,
        ) -> windows_core::HRESULT {
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }

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

    pub fn set_native_window_topmost(frame: &Frame, enabled: bool) {
        let Ok(window_handle) = frame.window_handle() else {
            return;
        };
        let hwnd = match window_handle.as_raw() {
            RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as *mut _),
            _ => return,
        };

        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(if enabled {
                    HWND_TOPMOST
                } else {
                    HWND_NOTOPMOST
                }),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
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

    pub fn drag_file_out(path: &Path) -> Result<()> {
        if !path.exists() {
            bail!("exported sound file is missing");
        }

        let mut wide_path = path.as_os_str().encode_wide().collect::<Vec<_>>();
        wide_path.push(0);

        struct OleGuard;
        impl Drop for OleGuard {
            fn drop(&mut self) {
                unsafe {
                    OleUninitialize();
                }
            }
        }

        struct PidlGuard(*const windows::Win32::UI::Shell::Common::ITEMIDLIST);
        impl Drop for PidlGuard {
            fn drop(&mut self) {
                if !self.0.is_null() {
                    unsafe {
                        ILFree(Some(self.0));
                    }
                }
            }
        }

        unsafe {
            OleInitialize(None).context("unable to initialize OLE drag session")?;
            let _ole_guard = OleGuard;
            let drag_result = (|| -> Result<()> {
                let full_pidl = ILCreateFromPathW(PCWSTR(wide_path.as_ptr()));
                if full_pidl.is_null() {
                    bail!("unable to create shell drag path");
                }
                let _full_pidl_guard = PidlGuard(full_pidl);

                let parent_pidl = ILClone(full_pidl);
                let child_pidl = ILClone(ILFindLastID(full_pidl));
                if parent_pidl.is_null() || child_pidl.is_null() {
                    bail!("unable to create shell drag data");
                }
                let _parent_pidl_guard = PidlGuard(parent_pidl);
                let _child_pidl_guard = PidlGuard(child_pidl);

                let _ = ILRemoveLastID(Some(parent_pidl));
                let child_items = [child_pidl as *const _];
                let data_object =
                    CIDLData_CreateFromIDArray(parent_pidl as *const _, Some(&child_items))
                        .context("unable to build drag payload")?;
                let drop_source: IDropSource = FileDropSource.into();
                let _ = SHDoDragDrop(None, &data_object, &drop_source, DROPEFFECT_COPY)
                    .context("unable to start drag and drop")?;
                Ok(())
            })();
            if let Ok(cursor) = LoadCursorW(None, IDC_ARROW) {
                let _ = SetCursor(Some(cursor));
            }
            drag_result?;
        }

        Ok(())
    }
}

#[cfg(windows)]
pub use windows_platform::*;

#[cfg(not(windows))]
pub fn set_native_window_shadow(_frame: &eframe::Frame, _enabled: bool) {}

#[cfg(not(windows))]
pub fn set_native_window_topmost(_frame: &eframe::Frame, _enabled: bool) {}

#[cfg(not(windows))]
pub fn set_overlay_window_native_visuals(
    _window_title: &str,
    _enabled: bool,
    _popup_only: bool,
) -> bool {
    false
}

#[cfg(not(windows))]
pub fn drag_file_out(_path: &std::path::Path) -> anyhow::Result<()> {
    anyhow::bail!("Drag out is only available on Windows")
}
