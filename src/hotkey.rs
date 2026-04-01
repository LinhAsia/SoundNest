use anyhow::{Result, bail};
use eframe::egui;

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use anyhow::Context;
    use std::sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    };
    use std::thread::{self, JoinHandle};
    use windows::Win32::{
        Foundation::{LPARAM, LRESULT, WPARAM},
        System::Threading::GetCurrentThreadId,
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, VK_0, VK_1, VK_2, VK_3, VK_4, VK_5, VK_6, VK_7, VK_8, VK_9, VK_A,
                VK_B, VK_BACK, VK_C, VK_CONTROL, VK_D, VK_DELETE, VK_DOWN, VK_E, VK_END, VK_ESCAPE,
                VK_F, VK_F1, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_F10,
                VK_F11, VK_F12, VK_G, VK_H, VK_HOME, VK_I, VK_INSERT, VK_J, VK_K, VK_L, VK_LEFT,
                VK_LWIN, VK_M, VK_MENU, VK_N, VK_NEXT, VK_O, VK_P, VK_PRIOR, VK_Q, VK_R, VK_RETURN,
                VK_RIGHT, VK_RWIN, VK_S, VK_SHIFT, VK_SPACE, VK_T, VK_TAB, VK_U, VK_UP, VK_V, VK_W,
                VK_X, VK_Y, VK_Z,
            },
            WindowsAndMessaging::{
                CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG,
                PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
                WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
            },
        },
    };

    static HOTKEY_STATE: OnceLock<Arc<HookState>> = OnceLock::new();

    #[derive(Default)]
    struct HookState {
        target_vk: AtomicU32,
        trigger_count: AtomicU64,
        pressed: AtomicBool,
        last_error: Mutex<Option<String>>,
        repaint_ctx: Mutex<Option<egui::Context>>,
    }

    pub struct GlobalHotkeyManager {
        state: Arc<HookState>,
        last_seen: u64,
        worker: Option<JoinHandle<()>>,
        thread_id: Option<u32>,
        current_key: Option<egui::Key>,
    }

    impl GlobalHotkeyManager {
        pub fn new() -> Self {
            let state = HOTKEY_STATE
                .get_or_init(|| Arc::new(HookState::default()))
                .clone();
            let (tx, rx) = mpsc::channel();
            let worker_state = Arc::clone(&state);

            let worker = thread::Builder::new()
                .name("record-hotkey".to_owned())
                .spawn(move || unsafe {
                    let thread_id = GetCurrentThreadId();
                    let hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0)
                    {
                        Ok(hook) => hook,
                        Err(error) => {
                            *worker_state.last_error.lock().unwrap() =
                                Some(format!("Trigger key unavailable: {error}"));
                            let _ = tx.send(thread_id);
                            return;
                        }
                    };

                    let _ = tx.send(thread_id);

                    let mut message = MSG::default();
                    loop {
                        let status = GetMessageW(&mut message, None, 0, 0);
                        if status.0 == -1 {
                            *worker_state.last_error.lock().unwrap() =
                                Some("Trigger key message loop failed".to_owned());
                            break;
                        }
                        if status.0 == 0 {
                            break;
                        }
                        let _ = TranslateMessage(&message);
                        let _ = DispatchMessageW(&message);
                    }

                    let _ = UnhookWindowsHookEx(hook);
                })
                .expect("unable to start trigger key worker");

            let thread_id = rx
                .recv()
                .context("trigger key worker did not start")
                .unwrap_or_default();

            Self {
                state,
                last_seen: 0,
                worker: Some(worker),
                thread_id: Some(thread_id),
                current_key: None,
            }
        }

        pub fn set_hotkey(&mut self, key: Option<egui::Key>) -> Result<()> {
            if self.current_key == key {
                return Ok(());
            }
            self.current_key = key;
            self.state.pressed.store(false, Ordering::Relaxed);
            self.state.target_vk.store(
                key.map(virtual_key_code).transpose()?.unwrap_or(0),
                Ordering::Relaxed,
            );
            Ok(())
        }

        pub fn set_repaint_context(&self, ctx: egui::Context) {
            *self.state.repaint_ctx.lock().unwrap() = Some(ctx);
        }

        pub fn take_triggered(&mut self) -> bool {
            let current = self.state.trigger_count.load(Ordering::Relaxed);
            if current == self.last_seen {
                return false;
            }
            self.last_seen = current;
            true
        }

        pub fn take_error(&self) -> Option<String> {
            self.state.last_error.lock().unwrap().take()
        }

        fn shutdown(&mut self) {
            self.state.target_vk.store(0, Ordering::Relaxed);
            self.state.pressed.store(false, Ordering::Relaxed);
            if let Some(thread_id) = self.thread_id.take() {
                unsafe {
                    let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
                }
            }
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    impl Drop for GlobalHotkeyManager {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 && lparam.0 != 0 {
            let state = HOTKEY_STATE.get().cloned();
            if let Some(state) = state {
                let target_vk = state.target_vk.load(Ordering::Relaxed);
                if target_vk != 0 {
                    let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
                    if info.vkCode == target_vk {
                        if modifier_down() {
                            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
                        }
                        match wparam.0 as u32 {
                            WM_KEYDOWN | WM_SYSKEYDOWN => {
                                state.pressed.store(true, Ordering::Relaxed);
                                return LRESULT(1);
                            }
                            WM_KEYUP | WM_SYSKEYUP => {
                                if state.pressed.swap(false, Ordering::Relaxed) {
                                    state.trigger_count.fetch_add(1, Ordering::Relaxed);
                                    if let Some(ctx) = state.repaint_ctx.lock().unwrap().clone() {
                                        ctx.request_repaint();
                                    }
                                }
                                return LRESULT(1);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    fn modifier_down() -> bool {
        unsafe {
            key_down(VK_MENU.0 as i32)
                || key_down(VK_CONTROL.0 as i32)
                || key_down(VK_SHIFT.0 as i32)
                || key_down(VK_LWIN.0 as i32)
                || key_down(VK_RWIN.0 as i32)
        }
    }

    unsafe fn key_down(vk: i32) -> bool {
        (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
    }

    fn virtual_key_code(key: egui::Key) -> Result<u32> {
        Ok(match key {
            egui::Key::ArrowDown => VK_DOWN.0 as u32,
            egui::Key::ArrowLeft => VK_LEFT.0 as u32,
            egui::Key::ArrowRight => VK_RIGHT.0 as u32,
            egui::Key::ArrowUp => VK_UP.0 as u32,
            egui::Key::Escape => VK_ESCAPE.0 as u32,
            egui::Key::Tab => VK_TAB.0 as u32,
            egui::Key::Backspace => VK_BACK.0 as u32,
            egui::Key::Enter => VK_RETURN.0 as u32,
            egui::Key::Space => VK_SPACE.0 as u32,
            egui::Key::Insert => VK_INSERT.0 as u32,
            egui::Key::Delete => VK_DELETE.0 as u32,
            egui::Key::Home => VK_HOME.0 as u32,
            egui::Key::End => VK_END.0 as u32,
            egui::Key::PageUp => VK_PRIOR.0 as u32,
            egui::Key::PageDown => VK_NEXT.0 as u32,
            egui::Key::Num0 => VK_0.0 as u32,
            egui::Key::Num1 => VK_1.0 as u32,
            egui::Key::Num2 => VK_2.0 as u32,
            egui::Key::Num3 => VK_3.0 as u32,
            egui::Key::Num4 => VK_4.0 as u32,
            egui::Key::Num5 => VK_5.0 as u32,
            egui::Key::Num6 => VK_6.0 as u32,
            egui::Key::Num7 => VK_7.0 as u32,
            egui::Key::Num8 => VK_8.0 as u32,
            egui::Key::Num9 => VK_9.0 as u32,
            egui::Key::A => VK_A.0 as u32,
            egui::Key::B => VK_B.0 as u32,
            egui::Key::C => VK_C.0 as u32,
            egui::Key::D => VK_D.0 as u32,
            egui::Key::E => VK_E.0 as u32,
            egui::Key::F => VK_F.0 as u32,
            egui::Key::G => VK_G.0 as u32,
            egui::Key::H => VK_H.0 as u32,
            egui::Key::I => VK_I.0 as u32,
            egui::Key::J => VK_J.0 as u32,
            egui::Key::K => VK_K.0 as u32,
            egui::Key::L => VK_L.0 as u32,
            egui::Key::M => VK_M.0 as u32,
            egui::Key::N => VK_N.0 as u32,
            egui::Key::O => VK_O.0 as u32,
            egui::Key::P => VK_P.0 as u32,
            egui::Key::Q => VK_Q.0 as u32,
            egui::Key::R => VK_R.0 as u32,
            egui::Key::S => VK_S.0 as u32,
            egui::Key::T => VK_T.0 as u32,
            egui::Key::U => VK_U.0 as u32,
            egui::Key::V => VK_V.0 as u32,
            egui::Key::W => VK_W.0 as u32,
            egui::Key::X => VK_X.0 as u32,
            egui::Key::Y => VK_Y.0 as u32,
            egui::Key::Z => VK_Z.0 as u32,
            egui::Key::F1 => VK_F1.0 as u32,
            egui::Key::F2 => VK_F2.0 as u32,
            egui::Key::F3 => VK_F3.0 as u32,
            egui::Key::F4 => VK_F4.0 as u32,
            egui::Key::F5 => VK_F5.0 as u32,
            egui::Key::F6 => VK_F6.0 as u32,
            egui::Key::F7 => VK_F7.0 as u32,
            egui::Key::F8 => VK_F8.0 as u32,
            egui::Key::F9 => VK_F9.0 as u32,
            egui::Key::F10 => VK_F10.0 as u32,
            egui::Key::F11 => VK_F11.0 as u32,
            egui::Key::F12 => VK_F12.0 as u32,
            _ => bail!("Unsupported trigger key"),
        })
    }
}

#[cfg(windows)]
pub use windows_impl::GlobalHotkeyManager;

#[cfg(not(windows))]
pub struct GlobalHotkeyManager;

#[cfg(not(windows))]
impl GlobalHotkeyManager {
    pub fn new() -> Self {
        Self
    }

    pub fn set_hotkey(&mut self, _key: Option<egui::Key>) -> Result<()> {
        Ok(())
    }

    pub fn set_repaint_context(&self, _ctx: egui::Context) {}

    pub fn take_triggered(&mut self) -> bool {
        false
    }

    pub fn take_error(&self) -> Option<String> {
        None
    }
}
