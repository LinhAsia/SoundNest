use anyhow::{Result, bail};
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: egui::Key,
}

pub fn format_key_name(key: egui::Key) -> &'static str {
    match key {
        egui::Key::ArrowDown => "Down",
        egui::Key::ArrowLeft => "Left",
        egui::Key::ArrowRight => "Right",
        egui::Key::ArrowUp => "Up",
        egui::Key::Escape => "Esc",
        egui::Key::Tab => "Tab",
        egui::Key::Backspace => "Backspace",
        egui::Key::Enter => "Enter",
        egui::Key::Space => "Space",
        egui::Key::Insert => "Insert",
        egui::Key::Delete => "Delete",
        egui::Key::Home => "Home",
        egui::Key::End => "End",
        egui::Key::PageUp => "PageUp",
        egui::Key::PageDown => "PageDown",
        egui::Key::Num0 => "0",
        egui::Key::Num1 => "1",
        egui::Key::Num2 => "2",
        egui::Key::Num3 => "3",
        egui::Key::Num4 => "4",
        egui::Key::Num5 => "5",
        egui::Key::Num6 => "6",
        egui::Key::Num7 => "7",
        egui::Key::Num8 => "8",
        egui::Key::Num9 => "9",
        egui::Key::A => "A",
        egui::Key::B => "B",
        egui::Key::C => "C",
        egui::Key::D => "D",
        egui::Key::E => "E",
        egui::Key::F => "F",
        egui::Key::G => "G",
        egui::Key::H => "H",
        egui::Key::I => "I",
        egui::Key::J => "J",
        egui::Key::K => "K",
        egui::Key::L => "L",
        egui::Key::M => "M",
        egui::Key::N => "N",
        egui::Key::O => "O",
        egui::Key::P => "P",
        egui::Key::Q => "Q",
        egui::Key::R => "R",
        egui::Key::S => "S",
        egui::Key::T => "T",
        egui::Key::U => "U",
        egui::Key::V => "V",
        egui::Key::W => "W",
        egui::Key::X => "X",
        egui::Key::Y => "Y",
        egui::Key::Z => "Z",
        egui::Key::F1 => "F1",
        egui::Key::F2 => "F2",
        egui::Key::F3 => "F3",
        egui::Key::F4 => "F4",
        egui::Key::F5 => "F5",
        egui::Key::F6 => "F6",
        egui::Key::F7 => "F7",
        egui::Key::F8 => "F8",
        egui::Key::F9 => "F9",
        egui::Key::F10 => "F10",
        egui::Key::F11 => "F11",
        egui::Key::F12 => "F12",
        _ => "Key",
    }
}

pub fn parse_key_name(name: &str) -> Option<egui::Key> {
    Some(match name {
        "Down" => egui::Key::ArrowDown,
        "Left" => egui::Key::ArrowLeft,
        "Right" => egui::Key::ArrowRight,
        "Up" => egui::Key::ArrowUp,
        "Esc" => egui::Key::Escape,
        "Tab" => egui::Key::Tab,
        "Backspace" => egui::Key::Backspace,
        "Enter" => egui::Key::Enter,
        "Space" => egui::Key::Space,
        "Insert" => egui::Key::Insert,
        "Delete" => egui::Key::Delete,
        "Home" => egui::Key::Home,
        "End" => egui::Key::End,
        "PageUp" => egui::Key::PageUp,
        "PageDown" => egui::Key::PageDown,
        "0" => egui::Key::Num0,
        "1" => egui::Key::Num1,
        "2" => egui::Key::Num2,
        "3" => egui::Key::Num3,
        "4" => egui::Key::Num4,
        "5" => egui::Key::Num5,
        "6" => egui::Key::Num6,
        "7" => egui::Key::Num7,
        "8" => egui::Key::Num8,
        "9" => egui::Key::Num9,
        "A" => egui::Key::A,
        "B" => egui::Key::B,
        "C" => egui::Key::C,
        "D" => egui::Key::D,
        "E" => egui::Key::E,
        "F" => egui::Key::F,
        "G" => egui::Key::G,
        "H" => egui::Key::H,
        "I" => egui::Key::I,
        "J" => egui::Key::J,
        "K" => egui::Key::K,
        "L" => egui::Key::L,
        "M" => egui::Key::M,
        "N" => egui::Key::N,
        "O" => egui::Key::O,
        "P" => egui::Key::P,
        "Q" => egui::Key::Q,
        "R" => egui::Key::R,
        "S" => egui::Key::S,
        "T" => egui::Key::T,
        "U" => egui::Key::U,
        "V" => egui::Key::V,
        "W" => egui::Key::W,
        "X" => egui::Key::X,
        "Y" => egui::Key::Y,
        "Z" => egui::Key::Z,
        "F1" => egui::Key::F1,
        "F2" => egui::Key::F2,
        "F3" => egui::Key::F3,
        "F4" => egui::Key::F4,
        "F5" => egui::Key::F5,
        "F6" => egui::Key::F6,
        "F7" => egui::Key::F7,
        "F8" => egui::Key::F8,
        "F9" => egui::Key::F9,
        "F10" => egui::Key::F10,
        "F11" => egui::Key::F11,
        "F12" => egui::Key::F12,
        _ => return None,
    })
}

impl Hotkey {
    pub fn to_string(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.win {
            parts.push("Win".to_string());
        }
        parts.push(format_key_name(self.key).to_string());
        parts.join("+")
    }

    pub fn from_string(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();
        if parts.is_empty() {
            return None;
        }
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut win = false;
        let mut base_key = None;

        for part in parts {
            match part.to_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                "win" | "meta" => win = true,
                _ => {
                    if let Some(key) = parse_key_name(part) {
                        base_key = Some(key);
                    }
                }
            }
        }

        base_key.map(|key| Hotkey {
            ctrl,
            alt,
            shift,
            win,
            key,
        })
    }

    #[cfg(windows)]
    pub fn vk_code(&self) -> Result<u32> {
        virtual_key_code(self.key)
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use anyhow::Context;
    use std::collections::HashSet;
    use std::sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    };
    use std::thread::{self, JoinHandle};
    use windows::Win32::{
        Foundation::{LPARAM, LRESULT, WPARAM},
        System::Threading::GetCurrentThreadId,
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
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
        target_hotkeys: Mutex<Vec<Hotkey>>,
        secondary_target_hotkeys: Mutex<Vec<Hotkey>>,
        pressed_indices: Mutex<HashSet<usize>>,
        secondary_pressed_indices: Mutex<HashSet<usize>>,
        trigger_count: AtomicU64,
        secondary_trigger_count: AtomicU64,
        last_error: Mutex<Option<String>>,
        repaint_ctx: Mutex<Option<egui::Context>>,
    }

    pub struct GlobalHotkeyManager {
        state: Arc<HookState>,
        last_seen: u64,
        secondary_last_seen: u64,
        worker: Option<JoinHandle<()>>,
        thread_id: Option<u32>,
        current_keys: Vec<Hotkey>,
        secondary_keys: Vec<Hotkey>,
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
                secondary_last_seen: 0,
                worker: Some(worker),
                thread_id: Some(thread_id),
                current_keys: Vec::new(),
                secondary_keys: Vec::new(),
            }
        }

        pub fn set_hotkeys(&mut self, keys: &[Hotkey]) -> Result<()> {
            if self.current_keys == keys {
                return Ok(());
            }
            self.current_keys = keys.to_vec();
            self.state.pressed_indices.lock().unwrap().clear();
            *self.state.target_hotkeys.lock().unwrap() = keys.to_vec();
            Ok(())
        }

        pub fn set_secondary_hotkeys(&mut self, keys: &[Hotkey]) -> Result<()> {
            if self.secondary_keys == keys {
                return Ok(());
            }
            self.secondary_keys = keys.to_vec();
            self.state.secondary_pressed_indices.lock().unwrap().clear();
            *self.state.secondary_target_hotkeys.lock().unwrap() = keys.to_vec();
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

        pub fn take_secondary_triggered(&mut self) -> bool {
            let current = self.state.secondary_trigger_count.load(Ordering::Relaxed);
            if current == self.secondary_last_seen {
                return false;
            }
            self.secondary_last_seen = current;
            true
        }

        pub fn take_error(&self) -> Option<String> {
            self.state.last_error.lock().unwrap().take()
        }

        fn shutdown(&mut self) {
            self.state.target_hotkeys.lock().unwrap().clear();
            self.state.secondary_target_hotkeys.lock().unwrap().clear();
            self.state.pressed_indices.lock().unwrap().clear();
            self.state.secondary_pressed_indices.lock().unwrap().clear();
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
                let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
                let msg = wparam.0 as u32;
                let is_key_down = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
                let is_key_up = matches!(msg, WM_KEYUP | WM_SYSKEYUP);

                if is_key_down || is_key_up {
                    let target_hotkeys = state.target_hotkeys.lock().unwrap().clone();
                    let secondary_target_hotkeys =
                        state.secondary_target_hotkeys.lock().unwrap().clone();

                    let mut matched_primary = None;
                    let mut matched_secondary = None;

                    // 1. Check primary hotkeys
                    for (idx, hotkey) in target_hotkeys.iter().enumerate() {
                        if let Ok(vk) = hotkey.vk_code() {
                            if vk == info.vkCode {
                                if is_key_down {
                                    if check_modifiers(
                                        hotkey.ctrl,
                                        hotkey.alt,
                                        hotkey.shift,
                                        hotkey.win,
                                    ) {
                                        matched_primary = Some(idx);
                                        break;
                                    }
                                } else {
                                    let pressed =
                                        state.pressed_indices.lock().unwrap().contains(&idx);
                                    if pressed {
                                        matched_primary = Some(idx);
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    // 2. Check secondary hotkeys
                    if matched_primary.is_none() {
                        for (idx, hotkey) in secondary_target_hotkeys.iter().enumerate() {
                            if let Ok(vk) = hotkey.vk_code() {
                                if vk == info.vkCode {
                                    if is_key_down {
                                        if check_modifiers(
                                            hotkey.ctrl,
                                            hotkey.alt,
                                            hotkey.shift,
                                            hotkey.win,
                                        ) {
                                            matched_secondary = Some(idx);
                                            break;
                                        }
                                    } else {
                                        let pressed = state
                                            .secondary_pressed_indices
                                            .lock()
                                            .unwrap()
                                            .contains(&idx);
                                        if pressed {
                                            matched_secondary = Some(idx);
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if let Some(idx) = matched_primary {
                        if is_key_down {
                            state.pressed_indices.lock().unwrap().insert(idx);
                        } else {
                            state.pressed_indices.lock().unwrap().remove(&idx);
                            state.trigger_count.fetch_add(1, Ordering::Relaxed);
                            if let Some(ctx) = state.repaint_ctx.lock().unwrap().clone() {
                                ctx.request_repaint();
                            }
                        }
                        return LRESULT(1); // Swallow
                    }

                    if let Some(idx) = matched_secondary {
                        if is_key_down {
                            state.secondary_pressed_indices.lock().unwrap().insert(idx);
                        } else {
                            state.secondary_pressed_indices.lock().unwrap().remove(&idx);
                            state
                                .secondary_trigger_count
                                .fetch_add(1, Ordering::Relaxed);
                            if let Some(ctx) = state.repaint_ctx.lock().unwrap().clone() {
                                ctx.request_repaint();
                            }
                        }
                        return LRESULT(1); // Swallow
                    }
                }
            }
        }

        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    fn check_modifiers(ctrl: bool, alt: bool, shift: bool, win: bool) -> bool {
        unsafe {
            let sys_ctrl = key_down(VK_CONTROL.0 as i32);
            let sys_alt = key_down(VK_MENU.0 as i32);
            let sys_shift = key_down(VK_SHIFT.0 as i32);
            let sys_win = key_down(VK_LWIN.0 as i32) || key_down(VK_RWIN.0 as i32);

            sys_ctrl == ctrl && sys_alt == alt && sys_shift == shift && sys_win == win
        }
    }

    unsafe fn key_down(vk: i32) -> bool {
        (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
    }

    pub fn physical_hotkey_down(hotkey: Hotkey) -> bool {
        let Ok(vk) = super::virtual_key_code(hotkey.key) else {
            return false;
        };
        unsafe { key_down(vk as i32) && check_modifiers(hotkey.ctrl, hotkey.alt, hotkey.shift, hotkey.win) }
    }
}

#[cfg(windows)]
pub use windows_impl::GlobalHotkeyManager;
#[cfg(windows)]
pub use windows_impl::physical_hotkey_down;

#[cfg(not(windows))]
pub struct GlobalHotkeyManager;

#[cfg(not(windows))]
pub fn physical_hotkey_down(_hotkey: Hotkey) -> bool {
    false
}

#[cfg(not(windows))]
impl GlobalHotkeyManager {
    pub fn new() -> Self {
        Self
    }

    pub fn set_hotkeys(&mut self, _keys: &[Hotkey]) -> Result<()> {
        Ok(())
    }

    pub fn set_repaint_context(&self, _ctx: egui::Context) {}

    pub fn take_triggered(&mut self) -> bool {
        false
    }

    pub fn take_error(&self) -> Option<String> {
        None
    }

    pub fn set_secondary_hotkeys(&mut self, _keys: &[Hotkey]) -> Result<()> {
        Ok(())
    }

    pub fn take_secondary_triggered(&mut self) -> bool {
        false
    }
}

#[cfg(windows)]
fn virtual_key_code(key: egui::Key) -> Result<u32> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
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
