use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ptr::addr_of_mut;

use std::default::Default;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::Sender;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{FALSE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayDevicesW, EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromWindow, DEVMODEW,
    DISPLAY_DEVICEW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, ENUM_CURRENT_SETTINGS, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT,
    RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK, RIDEV_REMOVE, RID_INPUT, RIM_TYPEMOUSE,
};

use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
    GetDesktopWindow, GetForegroundWindow, GetMessageW, GetShellWindow, GetWindowRect,
    PostThreadMessageW, RegisterClassW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    EDD_GET_DEVICE_INTERFACE_NAME, HHOOK, HOOKPROC, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
    LLKHF_INJECTED, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WINDOW_STYLE,
    WM_DISPLAYCHANGE, WM_INPUT, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WM_USER, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW, WNDPROC,
};

use input_event::{
    scancode::{self, Linux},
    Event, KeyboardEvent, PointerEvent, BTN_BACK, BTN_FORWARD, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT,
};

use super::{display_util, CaptureEvent, Position};

pub(crate) struct EventThread {
    request_buffer: Arc<Mutex<Vec<ClientUpdate>>>,
    thread: Option<thread::JoinHandle<()>>,
    thread_id: u32,
}

impl EventThread {
    pub(crate) fn new(event_tx: Sender<(Position, CaptureEvent)>) -> std::io::Result<Self> {
        let request_buffer = Default::default();
        let (thread, thread_id) = start(event_tx, Arc::clone(&request_buffer))?;
        Ok(Self {
            request_buffer,
            thread: Some(thread),
            thread_id,
        })
    }

    pub(crate) fn release_capture(&self) {
        self.signal(RequestType::Release);
    }

    pub(crate) fn create(&self, pos: Position) {
        self.client_update(ClientUpdate::Create(pos));
    }

    pub(crate) fn destroy(&self, pos: Position) {
        self.client_update(ClientUpdate::Destroy(pos));
    }

    pub(crate) fn set_target(&self, bounds: [i32; 4]) {
        self.client_update(ClientUpdate::Target(bounds));
    }

    fn exit(&self) -> bool {
        self.signal(RequestType::Exit)
    }

    fn client_update(&self, request: ClientUpdate) {
        {
            let mut requests = self.request_buffer.lock().unwrap();
            requests.push(request);
        }
        self.signal(RequestType::ClientUpdate);
    }

    fn signal(&self, event_type: RequestType) -> bool {
        let id = self.thread_id;
        if unsafe { PostThreadMessageW(id, WM_USER, WPARAM(event_type as usize), LPARAM(0)) }
            .is_err()
        {
            OVERFLOW.store(true, Ordering::Release);
            return false;
        }
        true
    }
}

impl Drop for EventThread {
    fn drop(&mut self) {
        let posted = self.exit();
        match self.thread.take() {
            Some(thread) if posted || thread.is_finished() => {
                let _ = thread.join();
            }
            _ => {}
        }
    }
}

enum RequestType {
    ClientUpdate = 0,
    Release = 1,
    Exit = 2,
}

enum ClientUpdate {
    Create(Position),
    Destroy(Position),
    Target([i32; 4]),
}

pub(super) static FULLSCREEN_BLOCKED: AtomicBool = AtomicBool::new(false);
pub(super) static OVERFLOW: AtomicBool = AtomicBool::new(false);
fn blocking_send_event(pos: Position, event: CaptureEvent) {
    let _ = try_send_event(pos, event);
}

fn try_send_event(
    pos: Position,
    event: CaptureEvent,
) -> Result<(), TrySendError<(Position, CaptureEvent)>> {
    let result = EVENT_TX.with_borrow_mut(|tx| tx.as_mut().unwrap().try_send((pos, event)));
    if result.is_err() {
        OVERFLOW.store(true, Ordering::Release);
        ACTIVE_CLIENT.take();
    }
    result
}

thread_local! {
    /// all configured clients
    static CLIENTS: RefCell<HashSet<Position>> = RefCell::new(HashSet::new());
    /// currently active client
    static ACTIVE_CLIENT: Cell<Option<Position>> = const { Cell::new(None) };
    /// input event channel
    static EVENT_TX: RefCell<Option<Sender<(Position, CaptureEvent)>>> = const { RefCell::new(None) };
    /// displays and generation counter
    static DISPLAYS: RefCell<(Vec<RECT>, i32)> = const { RefCell::new((Vec::new(), 0)) };
    static TARGET: Cell<Option<[i32; 4]>> = const { Cell::new(None) };
}

fn get_msg() -> Option<MSG> {
    unsafe {
        let mut msg = std::mem::zeroed();
        let ret = GetMessageW(addr_of_mut!(msg), None, 0, 0);
        match ret.0 {
            0 => None,
            x if x > 0 => Some(msg),
            _ => {
                OVERFLOW.store(true, Ordering::Release);
                None
            }
        }
    }
}

struct Hook(HHOOK);
impl Drop for Hook {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
    }
}

fn start(
    event_tx: Sender<(Position, CaptureEvent)>,
    request_buffer: Arc<Mutex<Vec<ClientUpdate>>>,
) -> std::io::Result<(thread::JoinHandle<()>, u32)> {
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let msg_thread = thread::spawn(move || {
        if let Err(error) = start_routine(ready_tx.clone(), event_tx, request_buffer) {
            let _ = ready_tx.send(Err(std::io::Error::other(error.to_string())));
        }
    });
    let id = ready_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| std::io::Error::other("Windows capture thread did not initialize"))??;
    Ok((msg_thread, id))
}

fn start_routine(
    ready: std::sync::mpsc::SyncSender<std::io::Result<u32>>,
    event_tx: Sender<(Position, CaptureEvent)>,
    request_buffer: Arc<Mutex<Vec<ClientUpdate>>>,
) -> windows::core::Result<()> {
    EVENT_TX.replace(Some(event_tx));
    let mouse_proc: HOOKPROC = Some(mouse_proc);
    let kybrd_proc: HOOKPROC = Some(kybrd_proc);
    let window_proc: WNDPROC = Some(window_proc);

    /* register hooks */
    let _mouse_hook = Hook(unsafe { SetWindowsHookExW(WH_MOUSE_LL, mouse_proc, None, 0)? });
    let _keyboard_hook = Hook(unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, kybrd_proc, None, 0)? });

    let instance = unsafe { GetModuleHandleW(None)? };
    let instance = instance.into();
    let window_class: WNDCLASSW = WNDCLASSW {
        lpfnWndProc: window_proc,
        hInstance: instance,
        lpszClassName: w!("lan-mouse-message-window-class"),
        ..Default::default()
    };

    static WINDOW_CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);
    if WINDOW_CLASS_REGISTERED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        /* register window class if not yet done so */
        unsafe {
            let ret = RegisterClassW(&window_class);
            if ret == 0 {
                return Err(windows::core::Error::from_win32());
            }
        }
    }

    /* window is used ro receive WM_DISPLAYCHANGE messages */
    let window = unsafe {
        CreateWindowExW(
            Default::default(),
            w!("lan-mouse-message-window-class"),
            w!("lan-mouse-msg-window"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )?
    };
    unsafe {
        RegisterRawInputDevices(
            &[RAWINPUTDEVICE {
                usUsagePage: 1,
                usUsage: 2,
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: window,
            }],
            std::mem::size_of::<RAWINPUTDEVICE>() as u32,
        )?;
    }

    // Only report readiness after hooks and the message queue exist.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));

    /* run message loop */
    while let Some(msg) = get_msg() {
        // mouse / keybrd proc do not actually return a message
        if msg.hwnd.0.is_null() {
            /* messages sent via PostThreadMessage */
            match msg.wParam.0 {
                x if x == RequestType::Exit as usize => break,
                x if x == RequestType::Release as usize => {
                    ACTIVE_CLIENT.take();
                }
                x if x == RequestType::ClientUpdate as usize => {
                    let requests = {
                        let mut res = vec![];
                        let mut requests = request_buffer.lock().unwrap();
                        for request in requests.drain(..) {
                            res.push(request);
                        }
                        res
                    };

                    for request in requests {
                        update_clients(request)
                    }
                }
                _ => {}
            }
        } else {
            /* other messages for window_procs */
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    unsafe {
        let _ = RegisterRawInputDevices(
            &[RAWINPUTDEVICE {
                usUsagePage: 1,
                usUsage: 2,
                dwFlags: RIDEV_REMOVE,
                ..Default::default()
            }],
            std::mem::size_of::<RAWINPUTDEVICE>() as u32,
        );
        let _ = DestroyWindow(window);
    }
    Ok(())
}

unsafe extern "system" fn mouse_proc(ncode: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if ncode < 0 || (*(lparam.0 as *const MSLLHOOKSTRUCT)).flags & LLMHF_INJECTED != 0 {
        return CallNextHookEx(None, ncode, wparam, lparam);
    }
    /* no client was active */
    let Some(pos) = ACTIVE_CLIENT.get() else {
        return CallNextHookEx(None, ncode, wparam, lparam);
    };

    // Motion comes from Raw Input so it continues past the desktop edge.
    if wparam.0 == WM_MOUSEMOVE as usize {
        return LRESULT(1);
    }

    /* convert to lan-mouse event */
    let Some(pointer_event) = to_mouse_event(wparam, lparam) else {
        return LRESULT(1);
    };

    /* notify mainthread (drop events if sending too fast) */
    if let Err(e) = try_send_event(pos, CaptureEvent::Input(Event::Pointer(pointer_event))) {
        log::warn!("e: {e}");
    }

    /* don't pass event to applications */
    LRESULT(1)
}

unsafe extern "system" fn kybrd_proc(ncode: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if ncode < 0
        || (*(lparam.0 as *const KBDLLHOOKSTRUCT))
            .flags
            .contains(LLKHF_INJECTED)
    {
        return CallNextHookEx(None, ncode, wparam, lparam);
    }
    /* get active client if any */
    let Some(client) = ACTIVE_CLIENT.get() else {
        return CallNextHookEx(None, ncode, wparam, lparam);
    };

    /* convert to key event */
    let Some(key_event) = to_key_event(wparam, lparam) else {
        return LRESULT(1);
    };

    if let Err(e) = try_send_event(client, CaptureEvent::Input(Event::Keyboard(key_event))) {
        log::warn!("e: {e}");
    }

    /* don't pass event to applications */
    LRESULT(1)
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    uint: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if uint == WM_INPUT {
        raw_mouse_motion(lparam);
    } else if uint == WM_DISPLAYCHANGE {
        log::debug!("display resolution changed");
        DISPLAY_RESOLUTION_GENERATION.fetch_add(1, Ordering::Release);
    }
    DefWindowProcW(hwnd, uint, wparam, lparam)
}

pub(crate) fn fullscreen_focused() -> bool {
    unsafe {
        let window = GetForegroundWindow();
        if window.0.is_null() || window == GetDesktopWindow() || window == GetShellWindow() {
            return false;
        }
        let mut rect = RECT::default();
        let mut monitor = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetWindowRect(window, &mut rect).is_ok()
            && GetMonitorInfoW(
                MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
            .as_bool()
            && display_util::covers_monitor(&rect, &monitor.rcMonitor)
    }
}

fn raw_mouse_motion(lparam: LPARAM) {
    // Check before activation so games never lose their local mouse/keyboard hooks.
    if fullscreen_focused() {
        if ACTIVE_CLIENT.take().is_some() {
            FULLSCREEN_BLOCKED.store(true, Ordering::Release);
        }
        return;
    }

    let mut raw = RAWINPUT::default();
    let mut size = std::mem::size_of::<RAWINPUT>() as u32;
    let bytes = unsafe {
        GetRawInputData(
            HRAWINPUT(lparam.0 as *mut _),
            RID_INPUT,
            Some((&mut raw as *mut RAWINPUT).cast()),
            &mut size,
            std::mem::size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if bytes == u32::MAX {
        ACTIVE_CLIENT.take();
        OVERFLOW.store(true, Ordering::Release);
        return;
    }
    if raw.header.dwType != RIM_TYPEMOUSE.0 {
        return;
    }
    let mouse = unsafe { raw.data.mouse };
    if mouse.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 != 0 || (mouse.lLastX == 0 && mouse.lLastY == 0) {
        return;
    }
    let pos = if let Some(pos) = ACTIVE_CLIENT.get() {
        pos
    } else {
        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_err() {
            return;
        }
        let point = (point.x, point.y);
        if !TARGET.get().is_none_or(|[x, y, w, h]| {
            point.0 >= x && point.0 < x + w && point.1 >= y && point.1 < y + h
        }) {
            return;
        }
        let entered = DISPLAYS.with_borrow_mut(|(displays, generation)| {
            update_display_regions(displays, generation);
            display_util::entered_barrier(point, (mouse.lLastX, mouse.lLastY), displays)
        });
        let Some(pos) = entered else {
            return;
        };
        if !CLIENTS.with_borrow(|clients| clients.contains(&pos)) {
            return;
        }
        ACTIVE_CLIENT.set(Some(pos));
        blocking_send_event(
            pos,
            CaptureEvent::Begin {
                point: [point.0 as f64, point.1 as f64],
            },
        );
        pos
    };
    blocking_send_event(
        pos,
        CaptureEvent::Input(Event::Pointer(PointerEvent::Motion {
            time: 0,
            dx: mouse.lLastX as f64,
            dy: mouse.lLastY as f64,
        })),
    );
}

static DISPLAY_RESOLUTION_GENERATION: AtomicI32 = AtomicI32::new(1);

fn update_display_regions(displays: &mut Vec<RECT>, generation: &mut i32) {
    let global_generation = DISPLAY_RESOLUTION_GENERATION.load(Ordering::Acquire);
    if *generation != global_generation {
        enumerate_displays(displays);
        log::debug!("displays: {displays:?}");
        *generation = global_generation;
    }
}

fn enumerate_displays(display_rects: &mut Vec<RECT>) {
    display_rects.clear();
    unsafe {
        let mut devices = vec![];
        for i in 0.. {
            let mut device: DISPLAY_DEVICEW = std::mem::zeroed();
            device.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
            let ret = EnumDisplayDevicesW(None, i, &mut device, EDD_GET_DEVICE_INTERFACE_NAME);
            if ret == FALSE {
                break;
            }
            if device
                .StateFlags
                .contains(DISPLAY_DEVICE_ATTACHED_TO_DESKTOP)
            {
                devices.push(device.DeviceName);
            }
        }
        for device in devices {
            let mut dev_mode: DEVMODEW = std::mem::zeroed();
            dev_mode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
            let ret = EnumDisplaySettingsW(
                PCWSTR::from_raw(&device as *const _),
                ENUM_CURRENT_SETTINGS,
                &mut dev_mode,
            );
            if ret == FALSE {
                log::warn!("no display mode");
                continue;
            }

            let pos = dev_mode.Anonymous1.Anonymous2.dmPosition;
            let (x, y) = (pos.x, pos.y);
            let (width, height) = (dev_mode.dmPelsWidth, dev_mode.dmPelsHeight);

            display_rects.push(RECT {
                left: x,
                right: x + width as i32,
                top: y,
                bottom: y + height as i32,
            });
        }
    }
}

fn update_clients(request: ClientUpdate) {
    match request {
        ClientUpdate::Create(pos) => {
            CLIENTS.with_borrow_mut(|clients| clients.insert(pos));
        }
        ClientUpdate::Destroy(pos) => {
            if let Some(active_pos) = ACTIVE_CLIENT.get() {
                if pos == active_pos {
                    let _ = ACTIVE_CLIENT.take();
                }
            }
            CLIENTS.with_borrow_mut(|clients| clients.remove(&pos));
        }
        ClientUpdate::Target(bounds) => TARGET.set(Some(bounds)),
    }
}

fn to_key_event(wparam: WPARAM, lparam: LPARAM) -> Option<KeyboardEvent> {
    let kybrdllhookstruct: KBDLLHOOKSTRUCT = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let mut scan_code = kybrdllhookstruct.scanCode;
    if kybrdllhookstruct.flags.contains(LLKHF_EXTENDED) {
        scan_code |= 0xE000;
    }
    let Ok(win_scan_code) = scancode::Windows::try_from(scan_code) else {
        return None;
    };
    let Ok(linux_scan_code): Result<Linux, ()> = win_scan_code.try_into() else {
        return None;
    };
    let scan_code = linux_scan_code as u32;
    match wparam {
        WPARAM(p) if p == WM_KEYDOWN as usize => Some(KeyboardEvent::Key {
            time: 0,
            key: scan_code,
            state: 1,
        }),
        WPARAM(p) if p == WM_KEYUP as usize => Some(KeyboardEvent::Key {
            time: 0,
            key: scan_code,
            state: 0,
        }),
        WPARAM(p) if p == WM_SYSKEYDOWN as usize => Some(KeyboardEvent::Key {
            time: 0,
            key: scan_code,
            state: 1,
        }),
        WPARAM(p) if p == WM_SYSKEYUP as usize => Some(KeyboardEvent::Key {
            time: 0,
            key: scan_code,
            state: 0,
        }),
        _ => None,
    }
}

fn to_mouse_event(wparam: WPARAM, lparam: LPARAM) -> Option<PointerEvent> {
    let mouse_low_level: MSLLHOOKSTRUCT = unsafe { *(lparam.0 as *const MSLLHOOKSTRUCT) };
    match wparam {
        WPARAM(p) if p == WM_LBUTTONDOWN as usize => Some(PointerEvent::Button {
            time: 0,
            button: BTN_LEFT,
            state: 1,
        }),
        WPARAM(p) if p == WM_MBUTTONDOWN as usize => Some(PointerEvent::Button {
            time: 0,
            button: BTN_MIDDLE,
            state: 1,
        }),
        WPARAM(p) if p == WM_RBUTTONDOWN as usize => Some(PointerEvent::Button {
            time: 0,
            button: BTN_RIGHT,
            state: 1,
        }),
        WPARAM(p) if p == WM_LBUTTONUP as usize => Some(PointerEvent::Button {
            time: 0,
            button: BTN_LEFT,
            state: 0,
        }),
        WPARAM(p) if p == WM_MBUTTONUP as usize => Some(PointerEvent::Button {
            time: 0,
            button: BTN_MIDDLE,
            state: 0,
        }),
        WPARAM(p) if p == WM_RBUTTONUP as usize => Some(PointerEvent::Button {
            time: 0,
            button: BTN_RIGHT,
            state: 0,
        }),
        WPARAM(p) if p == WM_MOUSEWHEEL as usize => Some(PointerEvent::AxisDiscrete120 {
            axis: 0,
            value: -(mouse_low_level.mouseData as i32 >> 16),
        }),
        WPARAM(p) if p == WM_XBUTTONDOWN as usize || p == WM_XBUTTONUP as usize => {
            let hb = mouse_low_level.mouseData >> 16;
            let button = match hb {
                1 => BTN_BACK,
                2 => BTN_FORWARD,
                _ => {
                    log::warn!("unknown mouse button");
                    return None;
                }
            };
            Some(PointerEvent::Button {
                time: 0,
                button,
                state: if p == WM_XBUTTONDOWN as usize { 1 } else { 0 },
            })
        }
        WPARAM(p) if p == WM_MOUSEHWHEEL as usize => Some(PointerEvent::AxisDiscrete120 {
            axis: 1, // Horizontal
            value: mouse_low_level.mouseData as i32 >> 16,
        }),
        _ => None,
    }
}
