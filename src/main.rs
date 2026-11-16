//! Taskbar Stopwatch — 작업표시줄 위에 띄우는 작업 시간 스톱워치 (Win32 + Direct2D)
#![windows_subsystem = "windows"]

mod gfx;
mod model;
mod timefmt;
mod settings_ui;
mod tray;

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::System::RemoteDesktop::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::Dialogs::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::Ime::ImmAssociateContext;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::SHQueryUserNotificationState;
use windows::Win32::UI::WindowsAndMessaging::*;

use gfx::{Gfx, Hit};
use model::{Controls, Settings, Theme, Timer};
use settings_ui::{Cmd, Flag, SettingsWin};
use tray::{Tray, WM_APP_TRAY};

const CLASS: PCWSTR = w!("TaskbarStopwatchRs");
const WM_APP_SHOW: u32 = WM_APP + 2;

const T_TICK: usize = 1;
const T_TOP: usize = 2;
const T_ARM: usize = 3;
const T_SAVE: usize = 4;
const T_TRAY: usize = 5;
const T_BAD: usize = 6;

const MIN: i64 = 60_000;

static WM_TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

// ───────────── 앱 상태 ─────────────
enum Press {
    None,
    Button(Hit),
    Drag { sx: i32, sy: i32, wx: i32, wy: i32, moved: bool },
    Resize { sx: i32, w0: i32 },
}

struct Edit { text: String, orig: String, sel_all: bool, bad: bool }

struct App {
    hwnd: HWND,
    s: Settings,
    t: Timer,
    gfx: Gfx,
    tray: Tray,
    dpi: u32,
    pos: (i32, i32),
    size: (i32, i32),
    inside: bool,
    hover: Hit,
    press: Press,
    armed: bool,
    edit: Option<Edit>,
    auto_paused: bool,
    visible: bool,
    fs_hidden: bool,
    hinst: HINSTANCE,
    icons: (HICON, HICON),
    sw: Option<SettingsWin>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// 재진입(모달 루프 안에서 오는 메시지) 시엔 None 을 돌려주고 건너뜀
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|c| c.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }
fn lo(v: isize) -> i32 { (v & 0xffff) as i16 as i32 }
fn hi(v: isize) -> i32 { ((v >> 16) & 0xffff) as i16 as i32 }

// ───────────── 모니터 / 작업표시줄 ─────────────
fn monitor_info(m: HMONITOR) -> MONITORINFO {
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe { let _ = GetMonitorInfoW(m, &mut mi); }
    mi
}
fn monitor_dpi(m: HMONITOR) -> u32 {
    let (mut x, mut y) = (96u32, 96u32);
    unsafe { let _ = GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut x, &mut y); }
    x
}

#[derive(Clone, Copy, PartialEq)]
enum Edge { Bottom, Top, Left, Right }
fn taskbar_rect(mi: &MONITORINFO) -> Option<(Edge, RECT)> {
    let (m, w) = (mi.rcMonitor, mi.rcWork);
    if w.bottom < m.bottom { return Some((Edge::Bottom, RECT { left: m.left, top: w.bottom, right: m.right, bottom: m.bottom })); }
    if w.top > m.top { return Some((Edge::Top, RECT { left: m.left, top: m.top, right: m.right, bottom: w.top })); }
    if w.left > m.left { return Some((Edge::Left, RECT { left: m.left, top: m.top, right: w.left, bottom: m.bottom })); }
    if w.right < m.right { return Some((Edge::Right, RECT { left: w.right, top: m.top, right: m.right, bottom: m.bottom })); }
    None
}

/// 기본 작업표시줄의 알림 영역(트레이) 왼쪽 x 좌표
fn tray_left() -> Option<i32> {
    unsafe {
        let bar = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
        let notify = FindWindowExW(bar, None, w!("TrayNotifyWnd"), None).ok()?;
        let mut r = RECT::default();
        GetWindowRect(notify, &mut r).ok()?;
        (r.right > r.left).then_some(r.left)
    }
}

/// 기본 위치: 주 모니터 작업표시줄의 트레이 왼편 (반환: x, y, dpi)
fn default_pos(s: &Settings) -> (i32, i32, u32) {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let dpi = monitor_dpi(mon);
        let k = dpi as f32 / 96.0;
        let (w, h) = ((s.width * k).round() as i32, (s.height * k).round() as i32);
        let mi = monitor_info(mon);
        if let Some((edge, tb)) = taskbar_rect(&mi) {
            if matches!(edge, Edge::Bottom | Edge::Top) && tb.bottom - tb.top >= h {
                let right = match tray_left() {
                    Some(x) if x > tb.left + w && x <= tb.right => x,
                    _ => tb.right - (330.0 * k) as i32,
                };
                return (right - w - (8.0 * k) as i32, tb.top + (tb.bottom - tb.top - h) / 2, dpi);
            }
        }
        let wa = mi.rcWork;
        (wa.right - w - (16.0 * k) as i32, wa.bottom - h - (16.0 * k) as i32, dpi)
    }
}

fn point_on_screen(x: i32, y: i32) -> bool {
    unsafe { !MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONULL).is_invalid() }
}

// ───────────── 시작 프로그램 등록 ─────────────
fn set_login_item(enable: bool) {
    let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let name = w!("TaskbarStopwatch");
    unsafe {
        if enable {
            let Ok(exe) = std::env::current_exe() else { return };
            let val = wide(&format!("\"{}\"", exe.display()));
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, key, name, REG_SZ.0, Some(val.as_ptr() as *const _), (val.len() * 2) as u32);
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, key, name);
        }
    }
}

// ───────────── App 동작 ─────────────
impl App {
    fn scale(&self) -> f32 { self.dpi as f32 / 96.0 }
    fn phys(&self) -> (i32, i32) {
        let k = self.scale();
        ((self.s.width * k).round() as i32, (self.s.height * k).round() as i32)
    }
    fn layout(&self) -> gfx::Layout { gfx::layout(self.size.0, self.size.1, self.scale(), &self.s, self.inside) }

    fn render(&mut self) {
        let edit = self.edit.as_ref().map(|e| gfx::EditView { text: &e.text, sel_all: e.sel_all, bad: e.bad });
        let scene = gfx::Scene {
            s: &self.s,
            w: self.size.0,
            h: self.size.1,
            scale: self.scale(),
            ms: self.t.elapsed(),
            running: self.t.running(),
            hover: self.hover,
            inside: self.inside,
            armed: self.armed,
            edit,
        };
        if self.gfx.render(&scene).is_err() { return; }
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        unsafe {
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&POINT { x: self.pos.0, y: self.pos.1 }),
                Some(&SIZE { cx: self.size.0, cy: self.size.1 }),
                self.gfx.dc,
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }
    }

    /// 다음 다시 그리기 예약: 정지 중엔 타이머 없음, 디지털은 초 경계에만
    fn schedule(&mut self) {
        unsafe {
            let _ = KillTimer(self.hwnd, T_TICK);
            if !self.t.running() || !self.visible { return; }
            let ms = if self.s.theme == Theme::Analog || self.s.show_centis {
                33
            } else {
                (1000 - self.t.elapsed().rem_euclid(1000) + 8) as u32
            };
            SetTimer(self.hwnd, T_TICK, ms, None);
        }
    }

    fn refresh(&mut self) {
        if let Some(sw) = &self.sw { sw.invalidate(); }
        self.render();
        self.schedule();
        self.update_tip();
    }

    fn update_tip(&mut self) {
        let tip = format!(
            "작업 스톱워치 · {}{}",
            timefmt::format_edit(self.t.elapsed()),
            if self.t.running() { " (진행 중)" } else { " (정지)" }
        );
        self.tray.set_tip(&tip);
    }

    fn timer_changed(&mut self) {
        self.t.save();
        self.refresh();
    }

    fn save_soon(&self) {
        unsafe { SetTimer(self.hwnd, T_SAVE, 300, None); }
    }

    fn settings_changed(&mut self) {
        self.save_soon();
        self.refresh();
    }

    fn set_size_dip(&mut self, w: f32, h: f32) {
        let (w, h) = model::clamp_size(w, h);
        self.s.width = w;
        self.s.height = h;
        let old_h = self.size.1;
        self.size = self.phys();
        self.pos.1 += (old_h - self.size.1) / 2; // 세로 중심 유지
        self.s.x = Some(self.pos.0);
        self.s.y = Some(self.pos.1);
        self.settings_changed();
    }

    fn apply_topmost(&self) {
        unsafe {
            let after = if self.s.always_on_top { HWND_TOPMOST } else { HWND_NOTOPMOST };
            let _ = SetWindowPos(self.hwnd, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn set_visible(&mut self, v: bool) {
        self.visible = v;
        self.fs_hidden = false;
        unsafe { let _ = ShowWindow(self.hwnd, if v { SW_SHOWNOACTIVATE } else { SW_HIDE }); }
        if v { self.apply_topmost(); }
        self.refresh();
    }

    /// 같은 모니터에서 전체 화면 앱(게임·발표)이 돌고 있으면 잠시 숨김
    fn check_fullscreen(&mut self) {
        let busy = self.s.hide_fullscreen
            && unsafe { SHQueryUserNotificationState() }.map(|q| matches!(q.0, 2..=4)).unwrap_or(false)
            && unsafe {
                let fg = GetForegroundWindow();
                fg != self.hwnd && MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) == MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST)
            };
        if busy && self.visible && !self.fs_hidden {
            self.fs_hidden = true;
            unsafe { let _ = ShowWindow(self.hwnd, SW_HIDE); }
        } else if !busy && self.fs_hidden {
            self.fs_hidden = false;
            if self.visible {
                unsafe { let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE); }
                self.apply_topmost();
                self.render();
            }
        }
    }

    /// 다른 DPI 모니터로 옮겨졌으면 크기 다시 계산
    fn check_dpi(&mut self) {
        let d = unsafe { GetDpiForWindow(self.hwnd) };
        if d != 0 && d != self.dpi {
            self.dpi = d;
            self.size = self.phys();
        }
    }

    /// 작업표시줄 근처면 가운데로 붙임
    fn snap(&mut self) {
        let (w, h) = self.size;
        let (mut x, mut y) = self.pos;
        unsafe {
            let mon = MonitorFromPoint(POINT { x: x + w / 2, y: y + h / 2 }, MONITOR_DEFAULTTONEAREST);
            if let Some((edge, tb)) = taskbar_rect(&monitor_info(mon)) {
                let snap = (14.0 * self.scale()) as i32;
                match edge {
                    Edge::Bottom | Edge::Top if h <= tb.bottom - tb.top => {
                        let cy = y + h / 2;
                        if cy > tb.top - snap && cy < tb.bottom + snap { y = tb.top + (tb.bottom - tb.top - h) / 2; }
                    }
                    Edge::Left | Edge::Right if w <= tb.right - tb.left => {
                        let cx = x + w / 2;
                        if cx > tb.left - snap && cx < tb.right + snap { x = tb.left + (tb.right - tb.left - w) / 2; }
                    }
                    _ => {}
                }
            }
        }
        self.pos = (x, y);
    }

    fn reset_position(&mut self) {
        let (x, y, dpi) = default_pos(&self.s);
        self.dpi = dpi;
        self.size = self.phys();
        self.pos = (x, y);
        self.s.x = Some(x);
        self.s.y = Some(y);
        self.settings_changed();
    }

    // ── 편집 ──
    fn open_edit(&mut self) {
        if self.edit.is_some() { return; }
        let text = timefmt::format_edit(self.t.elapsed());
        self.edit = Some(Edit { orig: text.clone(), text, sel_all: true, bad: false });
        unsafe {
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(self.hwnd);
        }
        self.render();
    }

    fn close_edit(&mut self, apply: bool) {
        let Some(e) = self.edit.take() else { return };
        if apply && e.text != e.orig {
            if let Some(p) = timefmt::parse(&e.text) {
                if p.relative { self.t.adjust(p.ms) } else { self.t.set(p.ms) }
                self.timer_changed();
                return;
            }
        }
        self.render();
    }

    fn edit_step(&mut self, up: bool, step: i64) {
        let Some(e) = self.edit.as_mut() else { return };
        if let Some(p) = timefmt::parse(&e.text) {
            if !p.relative {
                e.text = timefmt::format_edit((p.ms + if up { step } else { -step }).max(0));
                e.sel_all = false;
            }
        }
        self.render();
    }

    fn edit_key(&mut self, vk: u16) -> bool {
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        match VIRTUAL_KEY(vk) {
            VK_RETURN => {
                let e = self.edit.as_mut().unwrap();
                if e.text != e.orig && timefmt::parse(&e.text).is_none() {
                    e.bad = true;
                    unsafe { SetTimer(self.hwnd, T_BAD, 400, None); }
                    self.render();
                } else {
                    self.close_edit(true);
                }
            }
            VK_ESCAPE => self.close_edit(false),
            VK_UP => self.edit_step(true, if shift { 10 * MIN } else { MIN }),
            VK_DOWN => self.edit_step(false, if shift { 10 * MIN } else { MIN }),
            VK_BACK => {
                let e = self.edit.as_mut().unwrap();
                if e.sel_all { e.text.clear(); e.sel_all = false; } else { e.text.pop(); }
                e.bad = false;
                self.render();
            }
            VK_DELETE => {
                let e = self.edit.as_mut().unwrap();
                e.text.clear();
                e.sel_all = false;
                self.render();
            }
            VK_A if ctrl => {
                self.edit.as_mut().unwrap().sel_all = true;
                self.render();
            }
            _ => return false,
        }
        true
    }

    fn edit_char(&mut self, ch: char) {
        let Some(e) = self.edit.as_mut() else { return };
        let ok = ch.is_ascii_digit() || ":+-.hmsHMS".contains(ch) || "시분초".contains(ch);
        if !ok { return; }
        if e.sel_all { e.text.clear(); e.sel_all = false; }
        if e.text.chars().count() < 18 { e.text.push(ch.to_ascii_lowercase()); }
        e.bad = false;
        self.render();
    }

    // ── 자리 비움 ──
    fn away(&mut self) {
        if self.s.auto_pause && self.t.running() {
            self.t.pause();
            self.auto_paused = true;
            self.timer_changed();
        }
    }
    fn back(&mut self) {
        if self.s.auto_pause && self.auto_paused {
            self.auto_paused = false;
            self.t.start();
            self.timer_changed();
        }
    }

    fn shutdown_save(&mut self) {
        self.t.pause(); // 꺼져 있던 시간이 작업 시간으로 잡히지 않도록
        self.t.save();
        self.s.save();
    }

    // ── 설정 창 ──
    fn open_settings(&mut self) {
        if let Some(sw) = &self.sw {
            unsafe {
                if IsIconic(sw.hwnd).as_bool() { let _ = ShowWindow(sw.hwnd, SW_RESTORE); }
                let _ = SetForegroundWindow(sw.hwnd);
            }
            return;
        }
        match SettingsWin::create(self.hinst, self.hwnd, self.icons.0, self.icons.1) {
            Ok(sw) => self.sw = Some(sw),
            Err(e) => unsafe {
                let msg = wide(&format!("설정 창을 열 수 없어요: {e}"));
                MessageBoxW(None, PCWSTR(msg.as_ptr()), w!("스톱워치"), MB_ICONERROR);
            },
        }
    }

    fn apply_cmd(&mut self, c: Cmd) {
        match c {
            Cmd::Toggle => { self.t.toggle(); self.auto_paused = false; self.timer_changed(); }
            Cmd::Reset => { self.t.reset(); self.auto_paused = false; self.timer_changed(); }
            Cmd::Adjust(d) => { self.t.adjust(d); self.timer_changed(); }
            Cmd::Set(ms) => { self.t.set(ms); self.timer_changed(); }
            Cmd::Theme(th) => { self.s.theme = th; self.settings_changed(); }
            Cmd::Dark(d) => { self.s.dark = d; self.settings_changed(); }
            Cmd::Accent(c) => { self.s.accent = c; self.settings_changed(); }
            Cmd::Opacity(v) => { self.s.opacity = v.clamp(0.0, 1.0); self.settings_changed(); }
            Cmd::Size(w, h) => {
                if (w - self.s.width).abs() >= 0.5 || (h - self.s.height).abs() >= 0.5 { self.set_size_dip(w, h); }
            }
            Cmd::Controls(c) => { self.s.controls = c; self.settings_changed(); }
            Cmd::Flag(f) => {
                match f {
                    Flag::Centis => self.s.show_centis = !self.s.show_centis,
                    Flag::Topmost => { self.s.always_on_top = !self.s.always_on_top; self.apply_topmost(); }
                    Flag::AutoPause => self.s.auto_pause = !self.s.auto_pause,
                    Flag::Fullscreen => { self.s.hide_fullscreen = !self.s.hide_fullscreen; self.check_fullscreen(); }
                    Flag::Login => { self.s.open_at_login = !self.s.open_at_login; set_login_item(self.s.open_at_login); }
                }
                self.settings_changed();
            }
            Cmd::ResetPos => self.reset_position(),
            Cmd::PickColor => {}
        }
    }

    // ── 메뉴 ──
    fn build_menu(&self) -> HMENU {
        unsafe {
            let m = CreatePopupMenu().unwrap_or_default();
            let item = |menu: HMENU, id: u32, text: &str, checked: bool| {
                let w = wide(text);
                let flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
                let _ = AppendMenuW(menu, flags, id as usize, PCWSTR(w.as_ptr()));
            };
            let sep = |menu: HMENU| { let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()); };
            let sub = |menu: HMENU, text: &str, child: HMENU| {
                let w = wide(text);
                let _ = AppendMenuW(menu, MF_POPUP, child.0 as usize, PCWSTR(w.as_ptr()));
            };
            let radio = |menu: HMENU, first: u32, last: u32, sel: u32| {
                let _ = CheckMenuRadioItem(menu, first, last, sel, MF_BYCOMMAND.0);
            };

            item(m, ID_TOGGLE, if self.t.running() { "일시정지" } else { "시작" }, false);
            item(m, ID_RESET, "초기화…", false);
            sep(m);

            let adj = CreatePopupMenu().unwrap_or_default();
            item(adj, ID_EDIT, "직접 입력…\tEnter", false);
            sep(adj);
            for (i, (label, _)) in ADJUST.iter().enumerate() {
                if i == 5 { sep(adj); }
                item(adj, ID_ADJ + i as u32, label, false);
            }
            sub(m, "시간 조정", adj);

            let th = CreatePopupMenu().unwrap_or_default();
            item(th, ID_THEME, "아날로그", false);
            item(th, ID_THEME + 1, "디지털 · 모던", false);
            item(th, ID_THEME + 2, "디지털 · LCD", false);
            radio(th, ID_THEME, ID_THEME + 2, ID_THEME + match self.s.theme { Theme::Analog => 0, Theme::Digital => 1, Theme::Lcd => 2 });
            sep(th);
            item(th, ID_MODE, "다크", false);
            item(th, ID_MODE + 1, "라이트", false);
            radio(th, ID_MODE, ID_MODE + 1, if self.s.dark { ID_MODE } else { ID_MODE + 1 });
            sub(m, "테마", th);

            let col = CreatePopupMenu().unwrap_or_default();
            for (i, (name, _)) in ACCENTS.iter().enumerate() {
                item(col, ID_ACCENT + i as u32, name, false);
            }
            item(col, ID_ACCENT + ACCENTS.len() as u32, "직접 고르기…", false);
            let sel = ACCENTS.iter().position(|a| a.1 == self.s.accent).unwrap_or(ACCENTS.len());
            radio(col, ID_ACCENT, ID_ACCENT + ACCENTS.len() as u32, ID_ACCENT + sel as u32);
            sep(col);
            let op = CreatePopupMenu().unwrap_or_default();
            for (i, v) in OPACITY.iter().enumerate() {
                item(op, ID_OPACITY + i as u32, &format!("{}%", (v * 100.0).round()), false);
            }
            let near = OPACITY.iter().enumerate().min_by(|a, b| (a.1 - self.s.opacity).abs().total_cmp(&(b.1 - self.s.opacity).abs())).map(|x| x.0).unwrap_or(0);
            radio(op, ID_OPACITY, ID_OPACITY + OPACITY.len() as u32 - 1, ID_OPACITY + near as u32);
            sub(col, "배경 불투명도", op);
            sub(m, "색상", col);

            let sz = CreatePopupMenu().unwrap_or_default();
            for (i, (name, w, h)) in SIZES.iter().enumerate() {
                item(sz, ID_SIZE + i as u32, &format!("{name}\t{w}×{h}"), *w == self.s.width && *h == self.s.height);
            }
            sep(sz);
            item(sz, ID_SIZE_W_UP, "너비 넓히기\tShift+휠", false);
            item(sz, ID_SIZE_W_DOWN, "너비 좁히기", false);
            item(sz, ID_SIZE_UP, "전체 크게\tCtrl+휠", false);
            item(sz, ID_SIZE_DOWN, "전체 작게", false);
            sub(m, "크기", sz);

            let disp = CreatePopupMenu().unwrap_or_default();
            item(disp, ID_CONTROLS, "버튼 항상 표시", false);
            item(disp, ID_CONTROLS + 1, "버튼은 마우스 올릴 때만", false);
            item(disp, ID_CONTROLS + 2, "버튼 숨김", false);
            radio(disp, ID_CONTROLS, ID_CONTROLS + 2, ID_CONTROLS + match self.s.controls { Controls::Always => 0, Controls::Hover => 1, Controls::Never => 2 });
            sep(disp);
            item(disp, ID_CENTIS, "1/100초 표시", self.s.show_centis);
            sub(m, "표시", disp);

            sep(m);
            item(m, ID_TOPMOST, "항상 위에 표시", self.s.always_on_top);
            item(m, ID_AUTOPAUSE, "자리 비움 자동 정지 (잠금·절전)", self.s.auto_pause);
            item(m, ID_FULLSCREEN, "전체 화면 앱 실행 중엔 숨기기", self.s.hide_fullscreen);
            item(m, ID_LOGIN, "Windows 시작 시 실행", self.s.open_at_login);
            sep(m);
            item(m, ID_SETTINGS, "설정…", false);
            item(m, ID_RESETPOS, "위치 초기화", false);
            item(m, ID_HIDE, if self.visible { "숨기기" } else { "보이기" }, false);
            item(m, ID_QUIT, "종료", false);
            m
        }
    }

    fn command(&mut self, id: u32) {
        match id {
            ID_TOGGLE => { self.t.toggle(); self.auto_paused = false; self.timer_changed(); }
            ID_EDIT => { self.set_visible(true); self.open_edit(); }
            i if (ID_ADJ..ID_ADJ + ADJUST.len() as u32).contains(&i) => {
                self.t.adjust(ADJUST[(i - ID_ADJ) as usize].1 * MIN);
                self.timer_changed();
            }
            i if (ID_THEME..ID_THEME + 3).contains(&i) => {
                self.s.theme = [Theme::Analog, Theme::Digital, Theme::Lcd][(i - ID_THEME) as usize];
                self.settings_changed();
            }
            ID_MODE => { self.s.dark = true; self.settings_changed(); }
            i if i == ID_MODE + 1 => { self.s.dark = false; self.settings_changed(); }
            i if (ID_ACCENT..ID_ACCENT + ACCENTS.len() as u32).contains(&i) => {
                self.s.accent = ACCENTS[(i - ID_ACCENT) as usize].1;
                self.settings_changed();
            }
            i if (ID_OPACITY..ID_OPACITY + OPACITY.len() as u32).contains(&i) => {
                self.s.opacity = OPACITY[(i - ID_OPACITY) as usize];
                self.settings_changed();
            }
            i if (ID_SIZE..ID_SIZE + SIZES.len() as u32).contains(&i) => {
                let (_, w, h) = SIZES[(i - ID_SIZE) as usize];
                self.set_size_dip(w, h);
            }
            ID_SIZE_W_UP => self.set_size_dip(self.s.width + 20.0, self.s.height),
            ID_SIZE_W_DOWN => self.set_size_dip(self.s.width - 20.0, self.s.height),
            ID_SIZE_UP => self.set_size_dip(self.s.width * 1.1, self.s.height * 1.1),
            ID_SIZE_DOWN => self.set_size_dip(self.s.width / 1.1, self.s.height / 1.1),
            i if (ID_CONTROLS..ID_CONTROLS + 3).contains(&i) => {
                self.s.controls = [Controls::Always, Controls::Hover, Controls::Never][(i - ID_CONTROLS) as usize];
                self.settings_changed();
            }
            ID_CENTIS => { self.s.show_centis = !self.s.show_centis; self.settings_changed(); }
            ID_TOPMOST => { self.s.always_on_top = !self.s.always_on_top; self.apply_topmost(); self.settings_changed(); }
            ID_AUTOPAUSE => { self.s.auto_pause = !self.s.auto_pause; self.settings_changed(); }
            ID_LOGIN => { self.s.open_at_login = !self.s.open_at_login; set_login_item(self.s.open_at_login); self.settings_changed(); }
            ID_FULLSCREEN => { self.s.hide_fullscreen = !self.s.hide_fullscreen; self.check_fullscreen(); self.settings_changed(); }
            ID_SETTINGS => self.open_settings(),
            ID_RESETPOS => self.reset_position(),
            ID_HIDE => self.set_visible(!self.visible),
            _ => {}
        }
    }
}

// 메뉴 ID
const ID_TOGGLE: u32 = 1;
const ID_RESET: u32 = 2;
const ID_EDIT: u32 = 3;
const ID_ADJ: u32 = 10;
const ID_THEME: u32 = 30;
const ID_MODE: u32 = 35;
const ID_ACCENT: u32 = 40;
const ID_OPACITY: u32 = 60;
const ID_SIZE: u32 = 70;
const ID_SIZE_W_UP: u32 = 76;
const ID_SIZE_W_DOWN: u32 = 77;
const ID_SIZE_UP: u32 = 78;
const ID_SIZE_DOWN: u32 = 79;
const ID_CONTROLS: u32 = 80;
const ID_CENTIS: u32 = 83;
const ID_TOPMOST: u32 = 90;
const ID_AUTOPAUSE: u32 = 91;
const ID_LOGIN: u32 = 92;
const ID_FULLSCREEN: u32 = 93;
const ID_RESETPOS: u32 = 95;
const ID_SETTINGS: u32 = 97;
const ID_HIDE: u32 = 96;
const ID_QUIT: u32 = 99;

const ADJUST: [(&str, i64); 10] = [
    ("+1분", 1), ("+5분", 5), ("+10분", 10), ("+30분", 30), ("+1시간", 60),
    ("−1분", -1), ("−5분", -5), ("−10분", -10), ("−30분", -30), ("−1시간", -60),
];
const ACCENTS: [(&str, u32); 8] = [
    ("블루", 0x4f8cff), ("퍼플", 0x7c6cff), ("민트", 0x22c3a6), ("그린", 0x3ecf5b),
    ("앰버", 0xffb020), ("오렌지", 0xff7a45), ("핑크", 0xff4d6d), ("화이트", 0xe8eaef),
];
const OPACITY: [f32; 6] = [1.0, 0.96, 0.9, 0.8, 0.7, 0.5];
const SIZES: [(&str, f32, f32); 4] = [("작게", 132.0, 30.0), ("보통", 184.0, 40.0), ("넓게", 260.0, 40.0), ("크게", 300.0, 56.0)];

// ───────────── 모달 작업 (App 빌림 밖에서 실행) ─────────────
fn show_menu(hwnd: HWND, x: i32, y: i32) {
    let Some(menu) = with_app(|a| a.build_menu()) else { return };
    let cmd = unsafe {
        let _ = SetForegroundWindow(hwnd);
        let c = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, x, y, 0, hwnd, None);
        let _ = DestroyMenu(menu);
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        c.0 as u32
    };
    match cmd {
        0 => {}
        ID_RESET => confirm_reset(hwnd),
        ID_QUIT => quit(hwnd),
        i if i == ID_ACCENT + ACCENTS.len() as u32 => pick_color(hwnd),
        i => { with_app(|a| a.command(i)); }
    }
}

fn confirm_reset(hwnd: HWND) {
    let elapsed = with_app(|a| a.t.elapsed()).unwrap_or(0);
    if elapsed > 0 {
        let msg = wide(&format!("측정한 시간({})을 0으로 초기화할까요?", timefmt::format_edit(elapsed)));
        let r = unsafe { MessageBoxW(hwnd, PCWSTR(msg.as_ptr()), w!("스톱워치 초기화"), MB_YESNO | MB_ICONQUESTION | MB_TOPMOST) };
        if r != IDYES { return; }
    }
    with_app(|a| { a.t.reset(); a.auto_paused = false; a.timer_changed(); });
}

fn pick_color(hwnd: HWND) {
    static mut CUSTOM: [COLORREF; 16] = [COLORREF(0x00ffffff); 16];
    let cur = with_app(|a| a.s.accent).unwrap_or(0x4f8cff);
    let bgr = ((cur & 0xff) << 16) | (cur & 0xff00) | ((cur >> 16) & 0xff);
    unsafe {
        let mut cc = CHOOSECOLORW {
            lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: hwnd,
            rgbResult: COLORREF(bgr),
            lpCustColors: std::ptr::addr_of_mut!(CUSTOM) as *mut COLORREF,
            Flags: CC_FULLOPEN | CC_RGBINIT,
            ..Default::default()
        };
        if ChooseColorW(&mut cc).as_bool() {
            let c = cc.rgbResult.0;
            let rgb = ((c & 0xff) << 16) | (c & 0xff00) | ((c >> 16) & 0xff);
            with_app(|a| { a.s.accent = rgb; a.settings_changed(); });
        }
    }
}

fn quit(hwnd: HWND) {
    close_settings();
    with_app(|a| { a.shutdown_save(); a.tray.remove(); });
    unsafe { let _ = DestroyWindow(hwnd); }
}

fn close_settings() {
    if let Some(Some(sw)) = with_app(|a| a.sw.take()) {
        let h = sw.hwnd;
        drop(sw); // 렌더 타깃 먼저 해제
        unsafe { let _ = DestroyWindow(h); }
    }
}

/// 설정 창 메시지
extern "system" fn settings_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_CLOSE {
            close_settings();
            return LRESULT(0);
        }
        // 재진입(앱이 다른 처리 중)이거나 아직 연결 전이면 기본 처리
        let mine = with_app(|a| a.sw.as_ref().map(|s| s.hwnd == hwnd).unwrap_or(false)).unwrap_or(false);
        if !mine {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let mut pick = false;
        let handled: Option<LRESULT> = with_app(|a| {
            let mut cmd: Option<Cmd> = None;
            let r = {
                let App { sw, s, t, .. } = &mut *a;
                let w = sw.as_mut().unwrap();
                match msg {
                    WM_PAINT => { w.paint(s, t); Some(LRESULT(0)) }
                    WM_ERASEBKGND => Some(LRESULT(1)),
                    WM_SIZE => { w.resized(); Some(LRESULT(0)) }
                    WM_GETMINMAXINFO => {
                        let mm = &mut *(lp.0 as *mut MINMAXINFO);
                        let k = w.scale();
                        mm.ptMinTrackSize = POINT { x: (settings_ui::MIN_W * k) as i32, y: (settings_ui::MIN_H * k) as i32 };
                        Some(LRESULT(0))
                    }
                    WM_DPICHANGED => {
                        let r = *(lp.0 as *const RECT);
                        w.set_dpi((wp.0 & 0xffff) as u32);
                        let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                        w.resized();
                        Some(LRESULT(0))
                    }
                    WM_SETTINGCHANGE => { w.set_light(settings_ui::system_light()); None }
                    WM_TIMER => { w.tick(t.running()); Some(LRESULT(0)) }
                    WM_MOUSEMOVE => {
                        let (x, y) = w.dip(lp);
                        if w.ui.drag_slider.is_some() {
                            cmd = w.ui.slider_cmd(s, x);
                        } else if w.ui.drag_scroll.is_some() {
                            w.ui.scroll_drag(y);
                            w.invalidate();
                        } else {
                            let h = w.ui.hit(x, y);
                            if h != w.ui.hover {
                                w.ui.hover = h;
                                let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                                let _ = TrackMouseEvent(&mut tme);
                                w.invalidate();
                            }
                        }
                        Some(LRESULT(0))
                    }
                    0x02A3 /* WM_MOUSELEAVE */ => { w.ui.hover = None; w.invalidate(); Some(LRESULT(0)) }
                    WM_SETCURSOR if (lp.0 & 0xffff) as u32 == HTCLIENT => {
                        let id = if w.ui.drag_slider.is_some() { w.ui.pressed } else { w.ui.hover };
                        if let Ok(c) = LoadCursorW(None, w.ui.cursor_for(id)) { SetCursor(c); }
                        Some(LRESULT(1))
                    }
                    WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                        let (x, y) = w.dip(lp);
                        SetCapture(hwnd);
                        cmd = w.ui.press(s, x, y);
                        if msg == WM_LBUTTONDBLCLK && w.ui.pressed == Some(settings_ui::Id::Input) { w.ui.input_sel = true; }
                        w.invalidate();
                        Some(LRESULT(0))
                    }
                    WM_LBUTTONUP => {
                        let (x, y) = w.dip(lp);
                        cmd = w.ui.release(x, y, t);
                        w.invalidate();
                        Some(LRESULT(0))
                    }
                    WM_MOUSEWHEEL => {
                        let delta = ((wp.0 >> 16) & 0xffff) as i16 as f32;
                        w.ui.scroll_by(-delta / 120.0 * 48.0);
                        w.invalidate();
                        Some(LRESULT(0))
                    }
                    WM_KEYDOWN => {
                        let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                        let (h, c) = w.ui.key(wp.0 as u16, ctrl, t);
                        cmd = c;
                        if h { w.invalidate(); Some(LRESULT(0)) } else { None }
                    }
                    WM_CHAR => {
                        if let Some(ch) = char::from_u32(wp.0 as u32) { w.ui.char_in(ch); }
                        w.invalidate();
                        Some(LRESULT(0))
                    }
                    WM_KILLFOCUS => { w.ui.input_focus = false; w.invalidate(); None }
                    _ => None,
                }
            };
            if let Some(c) = cmd {
                if matches!(c, Cmd::PickColor) { pick = true; } else { a.apply_cmd(c); }
            }
            r
        })
        .flatten();
        if msg == WM_LBUTTONUP { let _ = ReleaseCapture(); }
        if pick { pick_color(hwnd); }
        if let Some(r) = handled { return r; }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

// ───────────── 메시지 처리 ─────────────
extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_TASKBAR_CREATED.load(Ordering::Relaxed) && msg != 0 {
            with_app(|a| a.tray.add());
            return LRESULT(0);
        }
        match msg {
            WM_MOUSEACTIVATE => {
                // 클릭해도 다른 앱의 포커스를 빼앗지 않음 (편집할 때만 활성화)
                let editing = with_app(|a| a.edit.is_some()).unwrap_or(false);
                return LRESULT(if editing { 1 } else { 3 }); // MA_ACTIVATE / MA_NOACTIVATE
            }
            WM_SETCURSOR => {
                if (lp.0 & 0xffff) as u32 == HTCLIENT {
                    let id = with_app(|a| match (&a.press, a.hover) {
                        (Press::Resize { .. }, _) | (_, Hit::Grip) => IDC_SIZEWE,
                        (Press::Drag { moved: true, .. }, _) => IDC_SIZEALL,
                        (_, Hit::Play | Hit::Reset) => IDC_HAND,
                        _ => IDC_ARROW,
                    })
                    .unwrap_or(IDC_ARROW);
                    if let Ok(c) = LoadCursorW(None, id) { SetCursor(c); }
                    return LRESULT(1);
                }
            }
            WM_MOUSEMOVE => {
                let (x, y) = (lo(lp.0), hi(lp.0));
                with_app(|a| {
                    if !a.inside {
                        a.inside = true;
                        let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                        let _ = TrackMouseEvent(&mut tme);
                        a.render();
                    }
                    match a.press {
                        Press::Drag { sx, sy, wx, wy, ref mut moved } => {
                            let mut c = POINT::default();
                            let _ = GetCursorPos(&mut c);
                            let (dx, dy) = (c.x - sx, c.y - sy);
                            if *moved || dx.abs() + dy.abs() > 4 {
                                *moved = true;
                                a.pos = (wx + dx, wy + dy);
                                let _ = SetWindowPos(hwnd, None, a.pos.0, a.pos.1, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                            }
                        }
                        Press::Resize { sx, w0 } => {
                            let mut c = POINT::default();
                            let _ = GetCursorPos(&mut c);
                            let w = (w0 + c.x - sx) as f32 / a.scale();
                            if (w - a.s.width).abs() >= 1.0 { a.set_size_dip(w, a.s.height); }
                        }
                        _ => {
                            let h = gfx::hit_test(&a.layout(), x as f32, y as f32);
                            if h != a.hover { a.hover = h; a.render(); }
                        }
                    }
                });
                return LRESULT(0);
            }
            0x02A3 /* WM_MOUSELEAVE */ => {
                with_app(|a| { a.inside = false; a.hover = Hit::None; a.render(); });
                return LRESULT(0);
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let (x, y) = (lo(lp.0), hi(lp.0));
                with_app(|a| {
                    if a.edit.is_some() {
                        let (x0, y0, x1, y1) = a.layout().edit;
                        let inside_box = (x as f32) >= x0 && (x as f32) <= x1 && (y as f32) >= y0 && (y as f32) <= y1;
                        if !inside_box {
                            let ok = a.edit.as_ref().map(|e| timefmt::parse(&e.text).is_some()).unwrap_or(false);
                            a.close_edit(ok);
                        }
                        return;
                    }
                    let hit = gfx::hit_test(&a.layout(), x as f32, y as f32);
                    if msg == WM_LBUTTONDBLCLK && hit == Hit::Face {
                        a.open_edit();
                        return;
                    }
                    let mut c = POINT::default();
                    let _ = GetCursorPos(&mut c);
                    a.press = match hit {
                        Hit::Grip => Press::Resize { sx: c.x, w0: a.size.0 },
                        Hit::Play | Hit::Reset => Press::Button(hit),
                        _ => Press::Drag { sx: c.x, sy: c.y, wx: a.pos.0, wy: a.pos.1, moved: false },
                    };
                    SetCapture(hwnd);
                });
                return LRESULT(0);
            }
            WM_LBUTTONUP => {
                let (x, y) = (lo(lp.0), hi(lp.0));
                // ReleaseCapture 는 WM_CAPTURECHANGED 를 바로 보내므로, 눌림 상태를 먼저 꺼내 처리한 뒤 해제
                with_app(|a| {
                    let press = std::mem::replace(&mut a.press, Press::None);
                    match press {
                        Press::Button(b) => {
                            if gfx::hit_test(&a.layout(), x as f32, y as f32) == b {
                                match b {
                                    Hit::Play => { a.t.toggle(); a.auto_paused = false; a.timer_changed(); }
                                    Hit::Reset => {
                                        if a.armed {
                                            a.armed = false;
                                            let _ = KillTimer(hwnd, T_ARM);
                                            a.t.reset();
                                            a.auto_paused = false;
                                            a.timer_changed();
                                        } else {
                                            a.armed = true;
                                            SetTimer(hwnd, T_ARM, 2200, None);
                                            a.render();
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Press::Drag { moved: true, .. } => {
                            a.check_dpi();
                            a.snap();
                            a.s.x = Some(a.pos.0);
                            a.s.y = Some(a.pos.1);
                            a.settings_changed();
                        }
                        Press::Resize { .. } => a.render(),
                        _ => {}
                    }
                });
                let _ = ReleaseCapture();
                return LRESULT(0);
            }
            WM_CAPTURECHANGED => {
                with_app(|a| {
                    if let Press::Drag { moved: true, .. } = a.press {
                        a.s.x = Some(a.pos.0);
                        a.s.y = Some(a.pos.1);
                        a.save_soon();
                    }
                    a.press = Press::None;
                });
                return LRESULT(0);
            }
            WM_RBUTTONUP => {
                let mut c = POINT::default();
                let _ = GetCursorPos(&mut c);
                let editing = with_app(|a| a.edit.is_some()).unwrap_or(false);
                if !editing { show_menu(hwnd, c.x, c.y); }
                return LRESULT(0);
            }
            WM_MOUSEWHEEL => {
                let delta = hi(wp.0 as isize);
                let keys = (wp.0 & 0xffff) as u32;
                with_app(|a| {
                    let up = delta > 0;
                    if a.edit.is_some() {
                        a.edit_step(up, if keys & 0x4 != 0 { 10 * MIN } else { MIN });
                    } else if keys & 0x8 != 0 {
                        let f = if up { 1.06 } else { 1.0 / 1.06 };
                        a.set_size_dip(a.s.width * f, a.s.height * f);
                    } else if keys & 0x4 != 0 {
                        let f = if up { 1.06 } else { 1.0 / 1.06 };
                        a.set_size_dip(a.s.width * f, a.s.height);
                    }
                });
                return LRESULT(0);
            }
            WM_KEYDOWN => {
                let vk = wp.0 as u16;
                let handled = with_app(|a| {
                    if a.edit.is_some() { return a.edit_key(vk); }
                    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                    match VIRTUAL_KEY(vk) {
                        VK_SPACE => { a.t.toggle(); a.timer_changed(); }
                        VK_RETURN | VK_F2 => a.open_edit(),
                        VK_UP => { a.t.adjust(if shift { 10 * MIN } else { MIN }); a.timer_changed(); }
                        VK_DOWN => { a.t.adjust(if shift { -10 * MIN } else { -MIN }); a.timer_changed(); }
                        _ => return false,
                    }
                    true
                })
                .unwrap_or(false);
                if handled { return LRESULT(0); }
            }
            WM_CHAR => {
                if let Some(ch) = char::from_u32(wp.0 as u32) {
                    if !ch.is_control() { with_app(|a| a.edit_char(ch)); }
                }
                return LRESULT(0);
            }
            WM_KILLFOCUS => {
                with_app(|a| {
                    if let Some(e) = &a.edit {
                        let ok = timefmt::parse(&e.text).is_some();
                        a.close_edit(ok);
                    }
                });
            }
            WM_TIMER => {
                let id = wp.0;
                with_app(|a| match id {
                    T_TICK => { a.render(); a.schedule(); }
                    T_TOP => {
                        a.check_fullscreen();
                        if a.s.always_on_top && a.visible && !a.fs_hidden && matches!(a.press, Press::None) { a.apply_topmost(); }
                    }
                    T_ARM => { let _ = KillTimer(hwnd, T_ARM); a.armed = false; a.render(); }
                    T_SAVE => { let _ = KillTimer(hwnd, T_SAVE); a.s.save(); }
                    T_TRAY => { if a.t.running() { a.update_tip(); } }
                    T_BAD => { let _ = KillTimer(hwnd, T_BAD); if let Some(e) = a.edit.as_mut() { e.bad = false; } a.render(); }
                    _ => {}
                });
                return LRESULT(0);
            }
            WM_APP_TRAY => {
                match (lp.0 & 0xffff) as u32 {
                    WM_LBUTTONUP => { with_app(|a| a.set_visible(!a.visible)); }
                    WM_RBUTTONUP => {
                        let mut c = POINT::default();
                        let _ = GetCursorPos(&mut c);
                        show_menu(hwnd, c.x, c.y);
                    }
                    _ => {}
                }
                return LRESULT(0);
            }
            WM_APP_SHOW => {
                with_app(|a| { a.set_visible(true); a.open_settings(); });
                return LRESULT(0);
            }
            0x02B1 /* WM_WTSSESSION_CHANGE */ => {
                match wp.0 as u32 {
                    0x7 => { with_app(|a| a.away()); }  // WTS_SESSION_LOCK
                    0x8 => { with_app(|a| a.back()); }  // WTS_SESSION_UNLOCK
                    _ => {}
                }
                return LRESULT(0);
            }
            WM_POWERBROADCAST => {
                match wp.0 as u32 {
                    0x4 => { with_app(|a| a.away()); }          // PBT_APMSUSPEND
                    0x7 | 0x12 => { with_app(|a| a.back()); }   // PBT_APMRESUMESUSPEND / AUTOMATIC
                    _ => {}
                }
                return LRESULT(1);
            }
            WM_QUERYENDSESSION => return LRESULT(1),
            WM_ENDSESSION => {
                if wp.0 != 0 { with_app(|a| a.shutdown_save()); }
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let r = *(lp.0 as *const RECT);
                with_app(|a| {
                    a.dpi = (wp.0 & 0xffff) as u32;
                    a.size = a.phys();
                    if matches!(a.press, Press::None) { a.pos = (r.left, r.top); }
                    a.render();
                });
                return LRESULT(0);
            }
            WM_DISPLAYCHANGE => {
                with_app(|a| {
                    let (w, h) = a.size;
                    if !point_on_screen(a.pos.0 + w / 2, a.pos.1 + h / 2) { a.reset_position(); }
                });
            }
            WM_CLOSE => { quit(hwnd); return LRESULT(0); }
            WM_DESTROY => {
                let _ = WTSUnRegisterSessionNotification(hwnd);
                PostQuitMessage(0);
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        // 개발용: StopWatch.exe --snapshot-settings out.png [light] [width] [input]
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).map(|a| a == "--snapshot-settings").unwrap_or(false) {
            let path = args.get(2).cloned().unwrap_or_else(|| "settings.png".into());
            let light = args.get(3).map(|v| v == "light").unwrap_or(false);
            let width = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(settings_ui::BASE_W);
            let input = args.get(5).cloned().unwrap_or_default();
            return settings_ui::snapshot(&path, light, width, &Settings::load(), &Timer::load(), &input);
        }
        // 개발용: --snapshot-widget out.png <analog|digital|lcd> <dark|light> [배율] [accent hex] [cs]
        if args.get(1).map(|a| a == "--snapshot-widget").unwrap_or(false) {
            let path = args.get(2).cloned().unwrap_or_else(|| "widget.png".into());
            let mut s = Settings::default();
            s.theme = match args.get(3).map(|v| v.as_str()) { Some("analog") => Theme::Analog, Some("lcd") => Theme::Lcd, _ => Theme::Digital };
            s.dark = args.get(4).map(|v| v != "light").unwrap_or(true);
            let k: f32 = args.get(5).and_then(|v| v.parse().ok()).unwrap_or(2.0);
            if let Some(c) = args.get(6).and_then(|v| u32::from_str_radix(v.trim_start_matches('#'), 16).ok()) { s.accent = c; }
            s.show_centis = args.get(7).map(|v| v == "cs").unwrap_or(false);
            if s.theme == Theme::Analog { s.width = 210.0; }
            let (w, h) = ((s.width * k).round() as i32, (s.height * k).round() as i32);
            let mut g = Gfx::new()?;
            g.render(&gfx::Scene {
                s: &s, w, h, scale: k, ms: 3_724_560, running: true,
                hover: Hit::None, inside: false, armed: false, edit: None,
            })?;
            return settings_ui::save_pixels(&path, w as u32, h as u32, &g.pixels());
        }

        // 단일 인스턴스: 이미 떠 있으면 그 위젯을 보이게 하고 종료
        let _mutex = CreateMutexW(None, true, w!("Local\\TaskbarStopwatchRs"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(h) = FindWindowW(CLASS, None) {
                let _ = PostMessageW(h, WM_APP_SHOW, WPARAM(0), LPARAM(0));
            }
            return Ok(());
        }

        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let wc2 = WNDCLASSW {
            style: CS_DBLCLKS | CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(settings_proc),
            hInstance: hinst,
            lpszClassName: settings_ui::CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&wc2);
        WM_TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        let s = Settings::load();
        let t = Timer::load();

        // 위치/DPI 결정
        let (x, y, dpi) = match (s.x, s.y) {
            (Some(x), Some(y)) if point_on_screen(x + 10, y + 10) => {
                let mon = MonitorFromPoint(POINT { x: x + 10, y: y + 10 }, MONITOR_DEFAULTTONEAREST);
                (x, y, monitor_dpi(mon))
            }
            _ => default_pos(&s),
        };
        let k = dpi as f32 / 96.0;
        let (w, h) = ((s.width * k).round() as i32, (s.height * k).round() as i32);

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | if s.always_on_top { WS_EX_TOPMOST } else { WINDOW_EX_STYLE(0) },
            CLASS,
            w!("작업 스톱워치"),
            WS_POPUP,
            x, y, w, h,
            None, None, hinst, None,
        )?;
        ImmAssociateContext(hwnd, None); // 한글 IME 끄기 (숫자/단위 입력용)
        let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);

        let icon = tray::make_icon(GetSystemMetrics(SM_CXSMICON).max(16) * 2, 0x4f8cff)?;
        // 창 아이콘: exe 에 넣은 앱 아이콘 (없으면 그려서 사용)
        let res_icon = |size: i32| -> Option<HICON> {
            LoadImageW(hinst, PCWSTR(1 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR).ok().map(|h| HICON(h.0))
        };
        let icon_big = match res_icon(GetSystemMetrics(SM_CXICON)) { Some(i) => i, None => tray::make_icon(32, 0x4f8cff)? };
        let icon_small = res_icon(GetSystemMetrics(SM_CXSMICON)).unwrap_or(icon);
        let tray = Tray::new(hwnd, icon);
        if s.open_at_login { set_login_item(true); } // 실행 파일 경로가 바뀌었을 수 있으니 갱신

        let mut app = App {
            hwnd, s, t, gfx: Gfx::new()?, tray,
            dpi, pos: (x, y), size: (w, h),
            inside: false, hover: Hit::None, press: Press::None,
            armed: false, edit: None, auto_paused: false, visible: true, fs_hidden: false,
            hinst, icons: (icon_big, icon_small), sw: None,
        };
        app.check_dpi();
        app.refresh();
        app.check_fullscreen(); // 게임 중에 켜져도 화면에 튀어나오지 않게
        if !app.fs_hidden {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            app.apply_topmost();
        }
        APP.with(|c| *c.borrow_mut() = Some(app));

        SetTimer(hwnd, T_TOP, 1500, None);
        SetTimer(hwnd, T_TRAY, 1000, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|c| c.borrow_mut().take());
        Ok(())
    }
}
