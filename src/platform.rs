#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragGhostKind {
    Sound,
    Folder,
}

#[derive(Clone, Debug)]
pub struct DragGhostSpec {
    pub kind: DragGhostKind,
    pub waveform: Vec<f32>,
    pub dark_theme: bool,
}

#[cfg(windows)]
mod windows_platform {
    use crate::platform::{DragGhostKind, DragGhostSpec};
    use anyhow::{Context, Result, bail};
    use eframe::Frame;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::thread;
    use windows::Win32::{
        Foundation::{
            COLORREF, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND,
            POINT, S_OK, SIZE,
        },
        Graphics::Dwm::{
            DWMNCRENDERINGPOLICY, DWMNCRP_DISABLED, DWMNCRP_ENABLED, DWMWA_NCRENDERING_POLICY,
            DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT, DWMWCP_ROUND,
            DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
        },
        Graphics::Gdi::{
            AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
            CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
            ScreenToClient, SelectObject,
        },
        System::{
            Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
            Ole::{DROPEFFECT_COPY, IDropSource, IDropSource_Impl, OleInitialize, OleUninitialize},
            SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS},
            Threading::Sleep,
        },
        UI::{
            Controls::MARGINS,
            Shell::{
                CIDLData_CreateFromIDArray, CLSID_DragDropHelper, IDragSourceHelper, ILClone,
                ILCreateFromPathW, ILFindLastID, ILFree, ILRemoveLastID, SHDRAGIMAGE, SHDoDragDrop,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, FindWindowW, GWL_EXSTYLE, GWL_STYLE, GetCursorPos,
                GetWindowLongW, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW, LoadCursorW, SW_MINIMIZE,
                SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
                SWP_NOSIZE, SetCursor, SetWindowLongW, SetWindowPos, ShowWindow, ULW_ALPHA,
                UpdateLayeredWindow, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
                WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP,
                WS_SYSMENU, WS_THICKFRAME,
            },
        },
    };
    use windows::core::{PCWSTR, implement, w};

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

            let corner = DWMWCP_ROUND;
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

    pub fn hide_native_window_by_title(title: &str) {
        let mut utf16 = title.encode_utf16().collect::<Vec<_>>();
        utf16.push(0);
        let hwnd = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(utf16.as_ptr())) };
        if let Ok(hwnd) = hwnd {
            if !hwnd.0.is_null() {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_MINIMIZE);
                }
            }
        }
    }

    #[allow(dead_code)]
    pub fn set_overlay_window_native_visuals(
        window_title: &str,
        enabled: bool,
        popup_only: bool,
    ) -> bool {
        let mut utf16 = window_title.encode_utf16().collect::<Vec<_>>();
        utf16.push(0);
        let Ok(hwnd) = (unsafe { FindWindowW(PCWSTR::null(), PCWSTR(utf16.as_ptr())) }) else {
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

    fn create_drag_ghost_bitmap(
        spec: &DragGhostSpec,
    ) -> Result<(windows::Win32::Graphics::Gdi::HBITMAP, i32, i32)> {
        let width = 164i32;
        let height = 176i32;
        let mut bitmap_info = BITMAPINFO::default();
        bitmap_info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };

        let mut bits = ptr::null_mut();
        let hbitmap = unsafe {
            CreateDIBSection(None, &bitmap_info, DIB_RGB_COLORS, &mut bits, None, 0)
                .context("unable to create drag ghost bitmap")?
        };

        let width_usize = width as usize;
        let height_usize = height as usize;
        let mut pixels = vec![0u8; width_usize * height_usize * 4];
        draw_drag_ghost_pixels(&mut pixels, width_usize, height_usize, spec);
        unsafe {
            ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
        }
        Ok((hbitmap, width, height))
    }

    fn premul(color: [u8; 4]) -> [u8; 4] {
        let alpha = color[3] as u16;
        [
            ((color[0] as u16 * alpha + 127) / 255) as u8,
            ((color[1] as u16 * alpha + 127) / 255) as u8,
            ((color[2] as u16 * alpha + 127) / 255) as u8,
            color[3],
        ]
    }

    fn set_pixel(buffer: &mut [u8], width: usize, x: usize, y: usize, color: [u8; 4]) {
        let index = (y * width + x) * 4;
        if index + 3 >= buffer.len() {
            return;
        }
        buffer[index] = color[2];
        buffer[index + 1] = color[1];
        buffer[index + 2] = color[0];
        buffer[index + 3] = color[3];
    }

    fn fill_round_rect(
        buffer: &mut [u8],
        width: usize,
        height: usize,
        x: f32,
        y: f32,
        rect_w: f32,
        rect_h: f32,
        radius: f32,
        color: [u8; 4],
    ) {
        let color = premul(color);
        let left = x.max(0.0) as usize;
        let top = y.max(0.0) as usize;
        let right = (x + rect_w).min(width as f32) as usize;
        let bottom = (y + rect_h).min(height as f32) as usize;
        let r = radius.min(rect_h * 0.5).min(rect_w * 0.5);

        for py in top..bottom {
            for px in left..right {
                let fx = px as f32 + 0.5;
                let fy = py as f32 + 0.5;
                let inside_core = fx >= x + r && fx <= x + rect_w - r;
                let inside_side = fy >= y + r && fy <= y + rect_h - r;
                let tl_dx = fx - (x + r);
                let tr_dx = fx - (x + rect_w - r);
                let ty = fy - (y + r);
                let by = fy - (y + rect_h - r);
                let inside_corner = tl_dx * tl_dx + ty * ty <= r * r
                    || tr_dx * tr_dx + ty * ty <= r * r
                    || tl_dx * tl_dx + by * by <= r * r
                    || tr_dx * tr_dx + by * by <= r * r;
                if (inside_core && fy >= y && fy <= y + rect_h)
                    || (inside_side && fx >= x && fx <= x + rect_w)
                    || inside_corner
                {
                    set_pixel(buffer, width, px, py, color);
                }
            }
        }
    }

    fn draw_wave_bars(
        buffer: &mut [u8],
        width: usize,
        height: usize,
        x: f32,
        y: f32,
        rect_w: f32,
        rect_h: f32,
        waveform: &[f32],
        color: [u8; 4],
    ) {
        if waveform.is_empty() {
            return;
        }
        let bar_count = waveform.len().min(28).max(8);
        let step = rect_w / bar_count as f32;
        let color = premul(color);
        for index in 0..bar_count {
            let source_index = index * waveform.len() / bar_count;
            let level = waveform
                .get(source_index)
                .copied()
                .unwrap_or(0.25)
                .clamp(0.06, 1.0);
            let bar_h = (rect_h * (0.18 + level * 0.72)).clamp(6.0, rect_h);
            let left = (x + index as f32 * step + step * 0.22).max(0.0) as usize;
            let right = (x + index as f32 * step + step * 0.78).min(width as f32) as usize;
            let top = (y + (rect_h - bar_h) * 0.5).max(0.0) as usize;
            let bottom = (top as f32 + bar_h).min(height as f32) as usize;
            for py in top..bottom {
                for px in left..right {
                    set_pixel(buffer, width, px, py, color);
                }
            }
        }
    }

    fn draw_folder_icon(
        buffer: &mut [u8],
        width: usize,
        height: usize,
        x: f32,
        y: f32,
        rect_w: f32,
        rect_h: f32,
        body: [u8; 4],
        tab: [u8; 4],
    ) {
        fill_round_rect(
            buffer,
            width,
            height,
            x + rect_w * 0.08,
            y + rect_h * 0.22,
            rect_w * 0.42,
            rect_h * 0.2,
            8.0,
            tab,
        );
        fill_round_rect(
            buffer,
            width,
            height,
            x,
            y + rect_h * 0.34,
            rect_w,
            rect_h * 0.48,
            12.0,
            body,
        );
    }

    fn draw_drag_ghost_pixels(
        buffer: &mut [u8],
        width: usize,
        height: usize,
        spec: &DragGhostSpec,
    ) {
        let (card_fill, panel_fill, stroke, wave) = if spec.dark_theme {
            (
                [29, 24, 35, 242],
                [22, 18, 27, 250],
                [227, 82, 149, 210],
                [255, 218, 234, 245],
            )
        } else {
            (
                [248, 242, 246, 244],
                [255, 255, 255, 248],
                [227, 82, 149, 190],
                [214, 51, 132, 245],
            )
        };

        fill_round_rect(
            buffer,
            width,
            height,
            8.0,
            10.0,
            148.0,
            156.0,
            28.0,
            [86, 43, 67, 34],
        );
        fill_round_rect(
            buffer, width, height, 0.0, 0.0, 164.0, 176.0, 28.0, card_fill,
        );
        fill_round_rect(
            buffer,
            width,
            height,
            0.0,
            0.0,
            164.0,
            176.0,
            28.0,
            [0, 0, 0, 0],
        );

        for inset in 0..2usize {
            fill_round_rect(
                buffer,
                width,
                height,
                inset as f32,
                inset as f32,
                164.0 - inset as f32 * 2.0,
                176.0 - inset as f32 * 2.0,
                28.0,
                [
                    stroke[0],
                    stroke[1],
                    stroke[2],
                    if inset == 0 { 84 } else { 40 },
                ],
            );
        }

        fill_round_rect(
            buffer,
            width,
            height,
            14.0,
            18.0,
            74.0,
            10.0,
            5.0,
            [stroke[0], stroke[1], stroke[2], 232],
        );
        match spec.kind {
            DragGhostKind::Sound => {
                fill_round_rect(
                    buffer, width, height, 14.0, 48.0, 136.0, 62.0, 18.0, panel_fill,
                );
                draw_wave_bars(
                    buffer,
                    width,
                    height,
                    22.0,
                    58.0,
                    120.0,
                    42.0,
                    &spec.waveform,
                    wave,
                );
            }
            DragGhostKind::Folder => {
                fill_round_rect(
                    buffer,
                    width,
                    height,
                    14.0,
                    48.0,
                    136.0,
                    62.0,
                    18.0,
                    [29, 20, 14, 252],
                );
                fill_round_rect(
                    buffer,
                    width,
                    height,
                    16.0,
                    50.0,
                    132.0,
                    58.0,
                    16.0,
                    [68, 37, 16, 236],
                );
                draw_folder_icon(
                    buffer,
                    width,
                    height,
                    24.0,
                    54.0,
                    46.0,
                    34.0,
                    [255, 156, 58, 248],
                    [255, 210, 118, 248],
                );
                draw_wave_bars(
                    buffer,
                    width,
                    height,
                    78.0,
                    58.0,
                    54.0,
                    20.0,
                    &spec.waveform,
                    [255, 230, 202, 245],
                );
                draw_wave_bars(
                    buffer,
                    width,
                    height,
                    78.0,
                    82.0,
                    42.0,
                    10.0,
                    &spec.waveform,
                    [255, 156, 58, 225],
                );
            }
        }
        fill_round_rect(
            buffer,
            width,
            height,
            14.0,
            126.0,
            38.0,
            28.0,
            14.0,
            [255, 255, 255, 18],
        );
        fill_round_rect(
            buffer,
            width,
            height,
            60.0,
            126.0,
            38.0,
            28.0,
            14.0,
            [stroke[0], stroke[1], stroke[2], 230],
        );
        fill_round_rect(
            buffer,
            width,
            height,
            14.0,
            118.0,
            44.0,
            4.0,
            2.0,
            [255, 255, 255, 108],
        );
    }

    fn initialize_transparent_drag_image(
        data_object: &windows::Win32::System::Com::IDataObject,
    ) -> Result<()> {
        let mut bitmap_info = BITMAPINFO::default();
        bitmap_info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: 1,
            biHeight: -1,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };

        let mut bits = ptr::null_mut();
        let bitmap = unsafe {
            CreateDIBSection(None, &bitmap_info, DIB_RGB_COLORS, &mut bits, None, 0)
                .context("unable to create transparent drag image")?
        };
        unsafe {
            ptr::write_bytes(bits.cast::<u8>(), 0, 4);
        }

        let drag_helper: IDragSourceHelper = unsafe {
            CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER)
                .context("unable to create drag source helper")?
        };
        let drag_image = SHDRAGIMAGE {
            sizeDragImage: SIZE { cx: 1, cy: 1 },
            ptOffset: POINT { x: 0, y: 0 },
            hbmpDragImage: bitmap,
            crColorKey: COLORREF(0),
        };
        unsafe {
            let result = drag_helper.InitializeFromBitmap(&drag_image, data_object);
            let _ = DeleteObject(bitmap.into());
            result.context("unable to initialize transparent drag image")?;
        }
        Ok(())
    }

    struct DragOverlayGuard {
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }

    impl Drop for DragOverlayGuard {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    fn start_drag_overlay(spec: &DragGhostSpec) -> Option<DragOverlayGuard> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let spec = spec.clone();
        let worker = thread::Builder::new()
            .name("sound-drag-overlay".to_owned())
            .spawn(move || unsafe {
                let Ok((bitmap, width, height)) = create_drag_ghost_bitmap(&spec) else {
                    return;
                };
                let mem_dc = CreateCompatibleDC(None);
                if mem_dc.0.is_null() {
                    let _ = DeleteObject(bitmap.into());
                    return;
                }
                let old_bitmap = SelectObject(mem_dc, bitmap.into());
                let hwnd = match CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                    w!("STATIC"),
                    w!(""),
                    WS_POPUP,
                    0,
                    0,
                    width,
                    height,
                    None,
                    None,
                    None,
                    None,
                ) {
                    Ok(hwnd) => hwnd,
                    Err(_) => {
                        if !old_bitmap.0.is_null() {
                            let _ = SelectObject(mem_dc, old_bitmap);
                        }
                        let _ = DeleteDC(mem_dc);
                        let _ = DeleteObject(bitmap.into());
                        return;
                    }
                };
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

                let source_origin = POINT { x: 0, y: 0 };
                let bitmap_size = SIZE {
                    cx: width,
                    cy: height,
                };
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };

                while !stop_for_thread.load(Ordering::Relaxed) {
                    let mut cursor = POINT::default();
                    if GetCursorPos(&mut cursor).is_ok() {
                        let position = POINT {
                            x: cursor.x + 18,
                            y: cursor.y - 18,
                        };
                        let _ = UpdateLayeredWindow(
                            hwnd,
                            None,
                            Some(&position),
                            Some(&bitmap_size),
                            Some(mem_dc),
                            Some(&source_origin),
                            COLORREF(0),
                            Some(&blend),
                            ULW_ALPHA,
                        );
                    }
                    Sleep(8);
                }

                let _ = DestroyWindow(hwnd);
                if !old_bitmap.0.is_null() {
                    let _ = SelectObject(mem_dc, old_bitmap);
                }
                let _ = DeleteDC(mem_dc);
                let _ = DeleteObject(bitmap.into());
            })
            .ok()?;

        Some(DragOverlayGuard {
            stop,
            worker: Some(worker),
        })
    }

    pub fn drag_file_out(path: &Path, ghost: Option<&DragGhostSpec>) -> Result<()> {
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
                let _ = initialize_transparent_drag_image(&data_object);
                let _overlay_guard = ghost.and_then(start_drag_overlay);
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

    #[allow(dead_code)]
    pub fn cursor_screen_position() -> Option<eframe::egui::Pos2> {
        unsafe {
            let mut cursor = POINT::default();
            if GetCursorPos(&mut cursor).is_ok() {
                Some(eframe::egui::pos2(cursor.x as f32, cursor.y as f32))
            } else {
                None
            }
        }
    }

    pub fn cursor_window_position(window_title: &str) -> Option<eframe::egui::Pos2> {
        let mut utf16 = window_title.encode_utf16().collect::<Vec<_>>();
        utf16.push(0);
        let hwnd = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(utf16.as_ptr())) }.ok()?;
        if hwnd.0.is_null() {
            return None;
        }

        unsafe {
            let mut cursor = POINT::default();
            if GetCursorPos(&mut cursor).is_err() {
                return None;
            }
            if !ScreenToClient(hwnd, &mut cursor).as_bool() {
                return None;
            }
            Some(eframe::egui::pos2(cursor.x as f32, cursor.y as f32))
        }
    }
}
#[cfg(windows)]
pub use windows_platform::*;

#[cfg(not(windows))]
pub fn set_native_window_shadow(_frame: &eframe::Frame, _enabled: bool) {}

#[cfg(not(windows))]
pub fn set_native_window_topmost(_frame: &eframe::Frame, _enabled: bool) {}

#[cfg(not(windows))]
pub fn show_native_window(_frame: &eframe::Frame) {}

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn set_overlay_window_native_visuals(
    _window_title: &str,
    _enabled: bool,
    _popup_only: bool,
) -> bool {
    false
}

#[cfg(not(windows))]
pub fn drag_file_out(
    _path: &std::path::Path,
    _ghost: Option<&DragGhostSpec>,
) -> anyhow::Result<()> {
    anyhow::bail!("Drag out is only available on Windows")
}

#[cfg(not(windows))]
pub fn cursor_screen_position() -> Option<eframe::egui::Pos2> {
    None
}

#[cfg(not(windows))]
pub fn cursor_window_position(_window_title: &str) -> Option<eframe::egui::Pos2> {
    None
}
