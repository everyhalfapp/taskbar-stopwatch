//! 설정 창 — Direct2D 로 직접 그리는 네이티브 창.
//! 창이 열려 있을 때만 존재하고, 닫으면 렌더 타깃과 함께 전부 해제된다.

use std::ffi::c_void;
use std::time::Instant;

use windows::core::*;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Input::Ime::ImmAssociateContext;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::gfx;
use crate::model::{self, Controls, Settings, Theme, Timer};
use crate::timefmt;

pub const CLASS: PCWSTR = w!("TaskbarStopwatchRsSettings");
pub const BASE_W: f32 = 440.0;
pub const BASE_H: f32 = 720.0;
pub const MIN_W: f32 = 380.0;
pub const MIN_H: f32 = 480.0;
pub const T_BLINK: usize = 1;

// ───────────── 클릭 대상 / 명령 ─────────────
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sl { Opacity, Width, Height }

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Flag { Centis, Topmost, AutoPause, Fullscreen, Login }

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Id {
    Toggle, Reset, Adj(i64), Input, Apply,
    Theme(Theme), Mode(bool), Accent(u32), AccentCustom,
    Slider(Sl), Preset(i32, i32), Controls(Controls), Flag(Flag), ResetPos, Scroll,
}

/// 앱에 전달할 명령
#[derive(Clone, Copy, Debug)]
pub enum Cmd {
    Toggle, Reset, Adjust(i64), Set(i64),
    Theme(Theme), Dark(bool), Accent(u32), PickColor,
    Opacity(f32), Size(f32, f32), Controls(Controls), Flag(Flag), ResetPos,
}

// ───────────── 색 ─────────────
type C = D2D1_COLOR_F;
fn rgb(hex: u32) -> C { rgba(hex, 1.0) }
fn rgba(hex: u32, a: f32) -> C {
    C { r: ((hex >> 16) & 255) as f32 / 255.0, g: ((hex >> 8) & 255) as f32 / 255.0, b: (hex & 255) as f32 / 255.0, a }
}
fn alpha(c: C, a: f32) -> C { C { a: c.a * a, ..c } }
fn mix(a: C, b: C, t: f32) -> C {
    C { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }
}
const WHITE: u32 = 0xffffff;
const DANGER: u32 = 0xe5484d;

#[derive(Clone, Copy)]
struct Pal { bg: u32, card: C, card2: C, line: C, fg: C, dim: C, faint: C }
fn palette(light: bool) -> Pal {
    if light {
        Pal { bg: 0xf3f4f7, card: rgb(0xffffff), card2: rgb(0xf0f1f4), line: rgba(0x000000, 0.07), fg: rgb(0x15181e), dim: rgb(0x5d6574), faint: rgb(0x9aa1ad) }
    } else {
        Pal { bg: 0x111318, card: rgb(0x181b22), card2: rgb(0x1f232c), line: rgba(0xffffff, 0.07), fg: rgb(0xeceff4), dim: rgb(0x8b93a3), faint: rgb(0x5c6473) }
    }
}

/// Windows 앱 모드 (라이트/다크)
pub fn system_light() -> bool {
    let mut v: u32 = 0;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut c_void),
            Some(&mut size),
        ) == ERROR_SUCCESS
            && v == 1
    }
}

fn pt(x: f32, y: f32) -> D2D_POINT_2F { D2D_POINT_2F { x, y } }
fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F { D2D_RECT_F { left: l, top: t, right: r, bottom: b } }
fn rr(l: f32, t: f32, w: f32, h: f32, rad: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: rect(l, t, l + w, t + h), radiusX: rad, radiusY: rad }
}
fn contains(r: &D2D_RECT_F, x: f32, y: f32) -> bool { x >= r.left && x < r.right && y >= r.top && y < r.bottom }
fn tr(x: f32, y: f32) -> Matrix3x2 { Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: x, M32: y } }

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq)]
enum Align { Left, Center, Right }

// ───────────── 그리기 도구 ─────────────
pub struct Painter {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    rt: ID2D1RenderTarget,
    fmt: IDWriteTextFormat,
    fmt_wrap: IDWriteTextFormat,
    typo: IDWriteTypography,
    round: ID2D1StrokeStyle,
    brush: ID2D1SolidColorBrush,
    segs: Vec<ID2D1PathGeometry>,
}

struct Txt { l: IDWriteTextLayout, w: f32, h: f32 }

impl Painter {
    pub fn new(d2d: ID2D1Factory, dw: IDWriteFactory, rt: ID2D1RenderTarget) -> Result<Painter> {
        unsafe {
            let mk = |wrap: bool| -> Result<IDWriteTextFormat> {
                let f = dw.CreateTextFormat(w!("Segoe UI Variable Text"), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 13.0, w!("ko-kr"))?;
                f.SetWordWrapping(if wrap { DWRITE_WORD_WRAPPING_WRAP } else { DWRITE_WORD_WRAPPING_NO_WRAP })?;
                Ok(f)
            };
            let fmt = mk(false)?;
            let fmt_wrap = mk(true)?;
            let typo = dw.CreateTypography()?;
            typo.AddFontFeature(DWRITE_FONT_FEATURE { nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, parameter: 1 })?;
            let round = d2d.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND, miterLimit: 10.0, dashStyle: D2D1_DASH_STYLE_SOLID, dashOffset: 0.0,
                },
                None,
            )?;
            let brush = rt.CreateSolidColorBrush(&rgb(WHITE), None)?;
            let mut segs = Vec::new();
            for pts in gfx::seg_points().iter() {
                let g = d2d.CreatePathGeometry()?;
                let sink = g.Open()?;
                sink.BeginFigure(pt(pts[0].0, pts[0].1), D2D1_FIGURE_BEGIN_FILLED);
                let rest: Vec<D2D_POINT_2F> = pts[1..].iter().map(|p| pt(p.0, p.1)).collect();
                sink.AddLines(&rest);
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                sink.Close()?;
                segs.push(g);
            }
            Ok(Painter { d2d, dw, rt, fmt, fmt_wrap, typo, round, brush, segs })
        }
    }

    fn color(&self, c: C) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&c) };
        &self.brush
    }
    fn fill_rr(&self, r: &D2D1_ROUNDED_RECT, c: C) { unsafe { self.rt.FillRoundedRectangle(r, self.color(c)) } }
    fn stroke_rr(&self, r: &D2D1_ROUNDED_RECT, c: C, w: f32) {
        // 선이 픽셀 안쪽에 오도록 반 픽셀 들여서
        let h = w / 2.0;
        let r2 = D2D1_ROUNDED_RECT {
            rect: rect(r.rect.left + h, r.rect.top + h, r.rect.right - h, r.rect.bottom - h),
            radiusX: (r.radiusX - h).max(0.0),
            radiusY: (r.radiusY - h).max(0.0),
        };
        unsafe { self.rt.DrawRoundedRectangle(&r2, self.color(c), w, &self.round) }
    }
    fn circle(&self, x: f32, y: f32, r: f32, c: C) {
        unsafe { self.rt.FillEllipse(&D2D1_ELLIPSE { point: pt(x, y), radiusX: r, radiusY: r }, self.color(c)) }
    }
    fn ring(&self, x: f32, y: f32, r: f32, c: C, w: f32) {
        unsafe { self.rt.DrawEllipse(&D2D1_ELLIPSE { point: pt(x, y), radiusX: r, radiusY: r }, self.color(c), w, &self.round) }
    }
    fn line(&self, a: (f32, f32), b: (f32, f32), c: C, w: f32) {
        unsafe { self.rt.DrawLine(pt(a.0, a.1), pt(b.0, b.1), self.color(c), w, &self.round) }
    }

    fn text_layout(&self, s: &str, size: f32, weight: DWRITE_FONT_WEIGHT, display: bool, wrap: Option<f32>) -> Txt {
        unsafe {
            let wide: Vec<u16> = s.encode_utf16().collect();
            let f = if wrap.is_some() { &self.fmt_wrap } else { &self.fmt };
            let l = self.dw.CreateTextLayout(&wide, f, wrap.unwrap_or(100_000.0), 10_000.0).unwrap();
            let all = DWRITE_TEXT_RANGE { startPosition: 0, length: wide.len() as u32 };
            let _ = l.SetFontSize(size, all);
            let _ = l.SetFontWeight(weight, all);
            if display {
                let _ = l.SetFontFamilyName(w!("Segoe UI Variable Display"), all);
            }
            let _ = l.SetTypography(&self.typo, all);
            let mut m = DWRITE_TEXT_METRICS::default();
            let _ = l.GetMetrics(&mut m);
            Txt { l, w: m.widthIncludingTrailingWhitespace, h: m.height }
        }
    }
    fn draw_txt(&self, t: &Txt, x: f32, y: f32, c: C) {
        unsafe { self.rt.DrawTextLayout(pt(x, y), &t.l, self.color(c), D2D1_DRAW_TEXT_OPTIONS_NONE) }
    }
    /// 글자를 (x 기준 정렬, 세로 가운데 cy) 로 그림. 너비 반환
    fn text(&self, s: &str, size: f32, weight: DWRITE_FONT_WEIGHT, c: C, x: f32, cy: f32, align: Align) -> f32 {
        let t = self.text_layout(s, size, weight, false, None);
        let x = match align { Align::Left => x, Align::Center => x - t.w / 2.0, Align::Right => x - t.w };
        self.draw_txt(&t, x, cy - t.h / 2.0, c);
        t.w
    }
    fn measure(&self, s: &str, size: f32, weight: DWRITE_FONT_WEIGHT) -> f32 {
        self.text_layout(s, size, weight, false, None).w
    }

    fn lcd(&self, s: &str, cx: f32, cy: f32, height: f32, on: C, off: C) {
        let mut glyphs = Vec::new();
        let mut x = 0.0f32;
        for ch in s.chars() {
            if ch == ':' { glyphs.push((None, x)); x += 5.0; } else { glyphs.push((ch.to_digit(10), x)); x += 13.0; }
        }
        let units = x - 3.0 + 4.0;
        let k = height / 20.0;
        let ox = cx - units * k / 2.0;
        let oy = cy - height / 2.0;
        let skew = (-7.0f32).to_radians().tan();
        unsafe {
            for (d, gx) in glyphs {
                // 단위좌표 → 기울이기 → 화면
                let m = Matrix3x2 { M11: k, M12: 0.0, M21: skew * k, M22: k, M31: ox + (gx + 2.4 + 1.0) * k, M32: oy + k };
                self.rt.SetTransform(&m);
                match d {
                    Some(d) => {
                        let on_set = gfx::SEG_ON[d as usize];
                        for (i, g) in self.segs.iter().enumerate() {
                            let lit = on_set.contains(b"abcdefg"[i] as char);
                            self.rt.FillGeometry(g, self.color(if lit { on } else { off }), None);
                        }
                    }
                    None => {
                        self.fill_rr(&rr(1.0, 4.6, 2.0, 2.0, 0.5), on);
                        self.fill_rr(&rr(1.0, 11.4, 2.0, 2.0, 0.5), on);
                    }
                }
            }
        }
    }

    fn rainbow(&self, cx: f32, cy: f32, r: f32) -> Result<()> {
        let cols = [0xff5555, 0xffbb33, 0x44dd66, 0x33bbee, 0x6666ff, 0xdd55ff];
        let n = 36;
        unsafe {
            for i in 0..n {
                let a0 = (i as f32 / n as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                let a1 = ((i + 1) as f32 / n as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2 + 0.02;
                let t = i as f32 / n as f32 * 6.0;
                let (ia, ib) = (t.floor() as usize % 6, (t.floor() as usize + 1) % 6);
                let c = mix(rgb(cols[ia]), rgb(cols[ib]), t.fract());
                let g = self.d2d.CreatePathGeometry()?;
                let sink = g.Open()?;
                sink.BeginFigure(pt(cx, cy), D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLines(&[pt(cx + a0.cos() * r, cy + a0.sin() * r), pt(cx + a1.cos() * r, cy + a1.sin() * r)]);
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                sink.Close()?;
                self.rt.FillGeometry(&g, self.color(c), None);
            }
        }
        Ok(())
    }
}

// ───────────── 화면 상태 ─────────────
pub struct UiState {
    pub light: bool,
    pub view: (f32, f32),
    pub scroll: f32,
    pub content_h: f32,
    pub regions: Vec<(D2D_RECT_F, Id)>,
    pub hover: Option<Id>,
    pub pressed: Option<Id>,
    pub drag_slider: Option<(Sl, f32, f32)>, // 종류, 트랙 왼쪽, 트랙 너비
    pub drag_scroll: Option<(f32, f32)>,     // 시작 마우스 y, 시작 스크롤
    pub input: String,
    pub input_focus: bool,
    pub input_sel: bool,
    pub input_bad: bool,
    pub armed_until: Option<Instant>,
    pub blink0: Instant,
}

impl UiState {
    pub fn new(light: bool, view: (f32, f32)) -> UiState {
        UiState {
            light, view, scroll: 0.0, content_h: 0.0, regions: Vec::new(), hover: None, pressed: None,
            drag_slider: None, drag_scroll: None, input: String::new(), input_focus: false, input_sel: false,
            input_bad: false, armed_until: None, blink0: Instant::now(),
        }
    }
    fn armed(&self) -> bool { self.armed_until.map_or(false, |t| Instant::now() < t) }
    fn max_scroll(&self) -> f32 { (self.content_h - self.view.1).max(0.0) }
    pub fn scroll_by(&mut self, d: f32) { self.scroll = (self.scroll + d).clamp(0.0, self.max_scroll()); }

    pub fn hit(&self, x: f32, y: f32) -> Option<Id> {
        // 스크롤바는 화면 좌표, 나머지는 콘텐츠 좌표
        if self.max_scroll() > 0.0 && x >= self.view.0 - 10.0 { return Some(Id::Scroll); }
        let cy = y + self.scroll;
        self.regions.iter().rev().find(|(r, _)| contains(r, x, cy)).map(|(_, id)| *id)
    }

    fn slider_value(s: &Settings, sl: Sl) -> (f32, f32, f32) {
        match sl {
            Sl::Opacity => ((s.opacity * 100.0).round(), 0.0, 100.0),
            Sl::Width => (s.width, model::MIN_W, model::MAX_W),
            Sl::Height => (s.height, model::MIN_H, model::MAX_H),
        }
    }

    /// 슬라이더 드래그 → 명령
    pub fn slider_cmd(&self, s: &Settings, x: f32) -> Option<Cmd> {
        let (sl, left, width) = self.drag_slider?;
        let (_, lo, hi) = Self::slider_value(s, sl);
        let v = (lo + ((x - left) / width).clamp(0.0, 1.0) * (hi - lo)).round();
        Some(match sl {
            Sl::Opacity => Cmd::Opacity(v / 100.0),
            Sl::Width => Cmd::Size(v, s.height),
            Sl::Height => Cmd::Size(s.width, v),
        })
    }

    pub fn scroll_drag(&mut self, y: f32) {
        if let Some((y0, s0)) = self.drag_scroll {
            let track = self.view.1;
            let ratio = self.content_h / track.max(1.0);
            self.scroll = (s0 + (y - y0) * ratio).clamp(0.0, self.max_scroll());
        }
    }

    /// 마우스 버튼을 누를 때
    pub fn press(&mut self, s: &Settings, x: f32, y: f32) -> Option<Cmd> {
        let id = self.hit(x, y);
        self.pressed = id;
        self.input_focus = id == Some(Id::Input);
        if self.input_focus { self.input_sel = false; self.blink0 = Instant::now(); }
        match id {
            Some(Id::Slider(sl)) => {
                if let Some((r, _)) = self.regions.iter().find(|(_, i)| *i == Id::Slider(sl)) {
                    // 영역은 트랙보다 좌우 4px 넓고, 트랙은 손잡이 반지름(8)만큼 안쪽에서 시작
                    self.drag_slider = Some((sl, r.left + 12.0, (r.right - r.left - 24.0).max(1.0)));
                }
                self.slider_cmd(s, x)
            }
            Some(Id::Scroll) => { self.drag_scroll = Some((y, self.scroll)); None }
            _ => None,
        }
    }

    /// 마우스 버튼을 뗄 때 (같은 대상 위면 클릭)
    pub fn release(&mut self, x: f32, y: f32, timer: &Timer) -> Option<Cmd> {
        let was = self.pressed.take();
        self.drag_slider = None;
        self.drag_scroll = None;
        let id = self.hit(x, y);
        if was.is_none() || was != id { return None; }
        match id? {
            Id::Toggle => Some(Cmd::Toggle),
            Id::Reset => {
                if self.armed() { self.armed_until = None; Some(Cmd::Reset) }
                else { self.armed_until = Some(Instant::now() + std::time::Duration::from_millis(2500)); None }
            }
            Id::Adj(ms) => Some(Cmd::Adjust(ms)),
            Id::Apply => self.submit(timer),
            Id::Theme(t) => Some(Cmd::Theme(t)),
            Id::Mode(d) => Some(Cmd::Dark(d)),
            Id::Accent(c) => Some(Cmd::Accent(c)),
            Id::AccentCustom => Some(Cmd::PickColor),
            Id::Preset(w, h) => Some(Cmd::Size(w as f32, h as f32)),
            Id::Controls(c) => Some(Cmd::Controls(c)),
            Id::Flag(f) => Some(Cmd::Flag(f)),
            Id::ResetPos => Some(Cmd::ResetPos),
            _ => None,
        }
    }

    pub fn submit(&mut self, _t: &Timer) -> Option<Cmd> {
        match timefmt::parse(&self.input) {
            Some(p) => {
                self.input.clear();
                self.input_bad = false;
                Some(if p.relative { Cmd::Adjust(p.ms) } else { Cmd::Set(p.ms) })
            }
            None => { self.input_bad = true; self.input_focus = true; None }
        }
    }

    pub fn key(&mut self, vk: u16, ctrl: bool, t: &Timer) -> (bool, Option<Cmd>) {
        const VK_BACK: u16 = 0x08; const VK_RETURN: u16 = 0x0D; const VK_ESCAPE: u16 = 0x1B;
        const VK_PRIOR: u16 = 0x21; const VK_NEXT: u16 = 0x22; const VK_END: u16 = 0x23; const VK_HOME: u16 = 0x24;
        const VK_UP: u16 = 0x26; const VK_DOWN: u16 = 0x28; const VK_A: u16 = 0x41;
        if self.input_focus {
            self.blink0 = Instant::now();
            match vk {
                VK_RETURN => return (true, self.submit(t)),
                VK_ESCAPE => { self.input_focus = false; return (true, None); }
                VK_BACK => {
                    if self.input_sel { self.input.clear(); self.input_sel = false; } else { self.input.pop(); }
                    self.input_bad = false;
                    return (true, None);
                }
                VK_A if ctrl => { self.input_sel = true; return (true, None); }
                _ => {}
            }
        }
        match vk {
            VK_UP => self.scroll_by(-40.0),
            VK_DOWN => self.scroll_by(40.0),
            VK_PRIOR => self.scroll_by(-self.view.1 * 0.9),
            VK_NEXT => self.scroll_by(self.view.1 * 0.9),
            VK_HOME => self.scroll = 0.0,
            VK_END => self.scroll = self.max_scroll(),
            _ => return (false, None),
        }
        (true, None)
    }

    pub fn char_in(&mut self, ch: char) {
        if !self.input_focus || ch.is_control() { return; }
        if self.input_sel { self.input.clear(); self.input_sel = false; }
        if self.input.chars().count() < 32 { self.input.push(ch); }
        self.input_bad = false;
        self.blink0 = Instant::now();
    }

    pub fn cursor_for(&self, id: Option<Id>) -> PCWSTR {
        match id {
            Some(Id::Input) => IDC_IBEAM,
            Some(Id::Scroll) | None => IDC_ARROW,
            Some(_) => IDC_HAND,
        }
    }

    fn hint(&self, t: &Timer) -> (String, bool) {
        if self.input.trim().is_empty() {
            return ("시:분:초, 시:분, 숫자만 쓰면 분. 앞에 + / − 를 붙이면 현재 시간에서 더하거나 뺍니다.".into(), false);
        }
        match timefmt::parse(&self.input) {
            None => ("형식을 알아볼 수 없어요.".into(), true),
            Some(p) => {
                let target = if p.relative { (t.elapsed() + p.ms).max(0) } else { p.ms };
                (format!("적용하면 → {}", timefmt::format(target, false)), false)
            }
        }
    }
}

// ───────────── 화면 구성 ─────────────
const ACCENTS: [u32; 8] = [0x4f8cff, 0x7c6cff, 0x22c3a6, 0x3ecf5b, 0xffb020, 0xff7a45, 0xff4d6d, 0xe8eaef];
const N: DWRITE_FONT_WEIGHT = DWRITE_FONT_WEIGHT_NORMAL;
const SB: DWRITE_FONT_WEIGHT = DWRITE_FONT_WEIGHT_SEMI_BOLD;

/// 그리기 + 클릭 영역 기록. 콘텐츠 전체 높이를 돌려줌
pub fn paint(p: &Painter, ui: &mut UiState, s: &Settings, t: &Timer) -> Result<()> {
    let pal = palette(ui.light);
    let acc = rgb(s.accent);
    let (vw, vh) = ui.view;
    let mut reg: Vec<(D2D_RECT_F, Id)> = Vec::new();
    let hov = ui.hover;
    let light = ui.light;
    let is_h = |id: Id| hov == Some(id);

    unsafe {
        p.rt.BeginDraw();
        p.rt.SetTransform(&tr(0.0, 0.0));
        p.rt.Clear(Some(&rgb(pal.bg)));
        p.rt.SetTransform(&tr(0.0, -ui.scroll.round()));
    }

    let x0 = 16.0;
    let cw = (vw - 32.0).max(200.0);
    let ix = x0 + 14.0;
    let iw = cw - 28.0;
    let mut y = 10.0;

    let card = |y: f32, h: f32| {
        let r = rr(x0, y, cw, h, 12.0);
        p.fill_rr(&r, pal.card);
        p.stroke_rr(&r, pal.line, 1.0);
    };
    let pill = |reg: &mut Vec<(D2D_RECT_F, Id)>, x: f32, y: f32, w: f32, label: &str, id: Id, kind: u8| {
        // kind: 0 보통, 1 강조, 2 위험
        let hover = hov == Some(id);
        let (bg, fg, border, wt) = match kind {
            1 => (acc, rgb(WHITE), None, SB),
            2 => (rgb(DANGER), rgb(WHITE), None, N),
            _ => (pal.card2, pal.fg, Some(pal.line), N),
        };
        let bg = if hover { mix(bg, rgb(WHITE), if light { 0.35 } else { 0.08 }) } else { bg };
        let r = rr(x, y, w, 32.0, 16.0);
        p.fill_rr(&r, bg);
        if let Some(b) = border { p.stroke_rr(&r, b, 1.0); }
        p.text(label, 13.0, wt, fg, x + w / 2.0, y + 16.0, Align::Center);
        reg.push((rect(x, y, x + w, y + 32.0), id));
    };
    let header = |y: &mut f32, label: &str| {
        *y += 10.0 + 12.0;
        p.text(label, 11.5, SB, pal.dim, x0 + 4.0, *y + 8.0, Align::Left);
        *y += 16.0 + 10.0;
    };
    let field_label = |y: f32, label: &str, out: Option<String>| {
        p.text(label, 12.5, N, pal.fg, ix, y + 8.0, Align::Left);
        if let Some(o) = out {
            let t = p.text_layout(&o, 12.5, N, false, None);
            p.draw_txt(&t, ix + iw - t.w, y + 8.0 - t.h / 2.0, pal.dim);
        }
    };
    let seg = |reg: &mut Vec<(D2D_RECT_F, Id)>, y: f32, items: &[(&str, Id, bool)]| {
        let r = rr(ix, y, iw, 36.0, 10.0);
        p.fill_rr(&r, pal.card2);
        p.stroke_rr(&r, pal.line, 1.0);
        let bw = (iw - 8.0) / items.len() as f32;
        for (i, (label, id, sel)) in items.iter().enumerate() {
            let bx = ix + 4.0 + i as f32 * bw;
            if *sel {
                let b = rr(bx, y + 4.0, bw, 28.0, 7.0);
                p.fill_rr(&rr(bx, y + 5.0, bw, 28.0, 7.0), rgba(0x000000, if light { 0.06 } else { 0.25 }));
                p.fill_rr(&b, pal.card);
                p.stroke_rr(&b, pal.line, 1.0);
            }
            let c = if *sel || hov == Some(*id) { pal.fg } else { pal.dim };
            p.text(label, 12.5, N, c, bx + bw / 2.0, y + 18.0, Align::Center);
            reg.push((rect(bx, y + 4.0, bx + bw, y + 32.0), *id));
        }
    };
    let slider = |reg: &mut Vec<(D2D_RECT_F, Id)>, y: f32, sl: Sl| {
        let (v, lo, hi) = UiState::slider_value(s, sl);
        let f = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
        let cy = y + 10.0;
        let (tl, tw) = (ix + 8.0, iw - 16.0);
        let tx = tl + tw * f;
        p.fill_rr(&rr(ix, cy - 2.5, iw, 5.0, 2.5), pal.card2);
        p.fill_rr(&rr(ix, cy - 2.5, (tx - ix).max(0.0), 5.0, 2.5), acc);
        p.stroke_rr(&rr(ix, cy - 2.5, iw, 5.0, 2.5), pal.line, 1.0);
        p.circle(tx, cy + 1.0, 8.6, rgba(0x000000, 0.35));
        p.circle(tx, cy, 8.0, rgb(WHITE));
        reg.push((rect(ix - 4.0, y - 2.0, ix + iw + 4.0, y + 22.0), Id::Slider(sl)));
    };
    let toggle = |reg: &mut Vec<(D2D_RECT_F, Id)>, y: f32, title: &str, sub: Option<&str>, on: bool, flag: Flag| -> f32 {
        let h = if sub.is_some() { 36.0 } else { 20.0 };
        if let Some(sub) = sub {
            p.text(title, 12.5, N, pal.fg, ix, y + 9.0, Align::Left);
            p.text(sub, 11.5, N, pal.faint, ix, y + 27.0, Align::Left);
        } else {
            p.text(title, 12.5, N, pal.fg, ix, y + 10.0, Align::Left);
        }
        let (tx, ty) = (ix + iw - 36.0, y + h / 2.0 - 10.0);
        let track = rr(tx, ty, 36.0, 20.0, 10.0);
        if on {
            p.fill_rr(&track, acc);
            p.circle(tx + 27.0, ty + 10.0, 7.0, rgb(WHITE));
        } else {
            p.fill_rr(&track, pal.card2);
            p.stroke_rr(&track, pal.line, 1.0);
            p.circle(tx + 11.0, ty + 10.0, 7.0, pal.dim);
        }
        reg.push((rect(ix, y, ix + iw, y + h), Id::Flag(flag)));
        h
    };

    // ══════════ 시간 카드 ══════════
    let (hint, hint_bad) = ui.hint(t);
    let hint_t = p.text_layout(&hint, 11.5, N, false, Some(iw));
    let card_h = 14.0 + 66.0 + 12.0 + 32.0 + 12.0 + 30.0 + 12.0 + 32.0 + 12.0 + hint_t.h + 14.0;
    card(y, card_h);
    let mut cy = y + 14.0;
    {
        let running = t.running();
        let el = t.elapsed();
        let state = if running { "진행 중" } else if el > 0 { "일시정지" } else { "정지" };
        let sc = if running { acc } else { pal.dim };
        let sw = p.measure(state, 11.0, N);
        let total = 7.0 + 6.0 + sw;
        let sx = x0 + cw / 2.0 - total / 2.0;
        let sy = cy + 6.0 + 8.0;
        if running { p.circle(sx + 3.5, sy, 3.5, acc); } else { p.ring(sx + 3.5, sy, 2.75, pal.dim, 1.5); }
        p.text(state, 11.0, N, sc, sx + 13.0, sy, Align::Left);
        let big = p.text_layout(&timefmt::format(el, false), 40.0, SB, true, None);
        p.draw_txt(&big, x0 + cw / 2.0 - big.w / 2.0, cy + 22.0 + 21.0 - big.h / 2.0, pal.fg);
        cy += 66.0 + 12.0;

        // 시작 / 초기화
        let label = if running { "일시정지" } else if el > 0 { "이어서" } else { "시작" };
        let w1 = (p.measure(label, 13.0, SB) + 32.0).max(92.0);
        let armed = ui.armed();
        let rlabel = if armed { "정말 초기화?" } else { "초기화" };
        let w2 = p.measure(rlabel, 13.0, N) + 32.0;
        let bx = x0 + cw / 2.0 - (w1 + 8.0 + w2) / 2.0;
        pill(&mut reg, bx, cy, w1, label, Id::Toggle, 1);
        pill(&mut reg, bx + w1 + 8.0, cy, w2, rlabel, Id::Reset, if armed { 2 } else { 0 });
        cy += 32.0 + 12.0;

        // ± 버튼
        let adj: [(&str, i64); 6] = [("−10분", -600_000), ("−5분", -300_000), ("−1분", -60_000), ("+1분", 60_000), ("+5분", 300_000), ("+10분", 600_000)];
        let bw = (iw - 30.0) / 6.0;
        for (i, (label, ms)) in adj.iter().enumerate() {
            let bx = ix + i as f32 * (bw + 6.0);
            let r = rr(bx, cy, bw, 30.0, 8.0);
            p.fill_rr(&r, pal.card2);
            p.stroke_rr(&r, if is_h(Id::Adj(*ms)) { alpha(acc, 0.6) } else { pal.line }, 1.0);
            p.text(label, 12.0, N, pal.fg, bx + bw / 2.0, cy + 15.0, Align::Center);
            reg.push((rect(bx, cy, bx + bw, cy + 30.0), Id::Adj(*ms)));
        }
        cy += 30.0 + 12.0;

        // 입력 + 적용
        let aw = p.measure("적용", 13.0, N) + 32.0;
        let inw = iw - 8.0 - aw;
        let r = rr(ix, cy, inw, 32.0, 16.0);
        p.fill_rr(&r, pal.card2);
        let border = if ui.input_bad || (hint_bad && !ui.input.is_empty()) { rgb(DANGER) } else if ui.input_focus { acc } else { pal.line };
        p.stroke_rr(&r, border, 1.0);
        reg.push((rect(ix, cy, ix + inw, cy + 32.0), Id::Input));
        unsafe {
            let clip = rect(ix + 10.0, cy, ix + inw - 10.0, cy + 32.0);
            p.rt.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
        }
        if ui.input.is_empty() {
            p.text("예) 2:15:00 · 1:30 · 45 · +20 · -5m", 13.0, N, pal.faint, ix + 12.0, cy + 16.0, Align::Left);
            if ui.input_focus && blink_on(ui) {
                p.line((ix + 12.0, cy + 9.0), (ix + 12.0, cy + 23.0), pal.fg, 1.0);
            }
        } else {
            let tl = p.text_layout(&ui.input, 13.0, N, false, None);
            let room = inw - 24.0;
            let tx = if tl.w > room { ix + 12.0 + room - tl.w } else { ix + 12.0 };
            if ui.input_sel {
                p.fill_rr(&rr(tx - 1.0, cy + 8.0, tl.w + 2.0, 16.0, 2.0), alpha(acc, 0.35));
            }
            p.draw_txt(&tl, tx, cy + 16.0 - tl.h / 2.0, pal.fg);
            if ui.input_focus && !ui.input_sel && blink_on(ui) {
                let cx = tx + tl.w + 1.0;
                p.line((cx, cy + 9.0), (cx, cy + 23.0), pal.fg, 1.0);
            }
        }
        unsafe { p.rt.PopAxisAlignedClip(); }
        pill(&mut reg, ix + inw + 8.0, cy, aw, "적용", Id::Apply, 0);
        cy += 32.0 + 12.0;
        p.draw_txt(&hint_t, ix, cy, if hint_bad { rgb(DANGER) } else { pal.faint });
    }
    y += card_h;

    // ══════════ 테마 ══════════
    header(&mut y, "테마");
    {
        let tw = (cw - 16.0) / 3.0;
        let items = [(Theme::Analog, "아날로그"), (Theme::Digital, "디지털 · 모던"), (Theme::Lcd, "디지털 · LCD")];
        for (i, (th, label)) in items.iter().enumerate() {
            let bx = x0 + i as f32 * (tw + 8.0);
            let sel = s.theme == *th;
            let r = rr(bx, y, tw, 104.0, 12.0);
            p.fill_rr(&r, pal.card);
            p.stroke_rr(&r, if sel { acc } else { pal.line }, 1.5);
            let lc = if sel || is_h(Id::Theme(*th)) { pal.fg } else { pal.dim };
            let lt = p.text_layout(label, 12.0, if sel { SB } else { N }, false, None);
            let ly = y + 104.0 - 10.0 - lt.h;
            p.draw_txt(&lt, bx + tw / 2.0 - lt.w / 2.0, ly, lc);
            let (pcx, pcy) = (bx + tw / 2.0, (y + 14.0 + ly - 8.0) / 2.0);
            match th {
                Theme::Analog => {
                    let k = 48.0 / 100.0;
                    p.ring(pcx, pcy, 44.0 * k, pal.faint, 3.0 * k);
                    for j in 0..12 {
                        let a = (j as f32 * 30.0).to_radians();
                        p.line((pcx + a.sin() * 33.0 * k, pcy - a.cos() * 33.0 * k), (pcx + a.sin() * 39.0 * k, pcy - a.cos() * 39.0 * k), pal.dim, 3.0 * k);
                    }
                    p.line((pcx, pcy + 4.0 * k), (pcx, pcy - 28.0 * k), pal.fg, 5.0 * k);
                    let a = 118f32.to_radians();
                    p.line((pcx - a.sin() * 10.0 * k, pcy + a.cos() * 10.0 * k), (pcx + a.sin() * 38.0 * k, pcy - a.cos() * 38.0 * k), acc, 2.5 * k);
                    p.circle(pcx, pcy, 4.0 * k, acc);
                }
                Theme::Digital => {
                    let a = p.text_layout("01:24", 22.0, SB, true, None);
                    let b = p.text_layout(":36", 22.0, DWRITE_FONT_WEIGHT_MEDIUM, true, None);
                    let x = pcx - (a.w + b.w) / 2.0;
                    p.draw_txt(&a, x, pcy - a.h / 2.0, pal.fg);
                    p.draw_txt(&b, x + a.w, pcy - b.h / 2.0, pal.dim);
                }
                Theme::Lcd => {
                    p.lcd("01:24", pcx, pcy - ui.scroll.round(), 24.0, acc, alpha(pal.fg, 0.07));
                    unsafe { p.rt.SetTransform(&tr(0.0, -ui.scroll.round())); }
                }
            }
            reg.push((rect(bx, y, bx + tw, y + 104.0), Id::Theme(*th)));
        }
        y += 104.0;
    }

    // ══════════ 색상 ══════════
    header(&mut y, "색상");
    {
        let h = 14.0 + 60.0 + 14.0 + 50.0 + 14.0 + 44.0 + 14.0;
        card(y, h);
        let mut cy = y + 14.0;
        field_label(cy, "모드", None);
        seg(&mut reg, cy + 24.0, &[("다크", Id::Mode(true), s.dark), ("라이트", Id::Mode(false), !s.dark)]);
        cy += 60.0 + 14.0;
        field_label(cy, "포인트 색", None);
        let sy = cy + 24.0 + 13.0;
        let known = ACCENTS.contains(&s.accent);
        for (i, c) in ACCENTS.iter().enumerate() {
            let sx = ix + 13.0 + i as f32 * 36.0;
            let r = if is_h(Id::Accent(*c)) { 14.0 } else { 13.0 };
            if s.accent == *c { p.ring(sx, sy, 16.0, pal.fg, 2.0); }
            p.circle(sx, sy, r, rgb(*c));
            reg.push((rect(sx - 13.0, sy - 13.0, sx + 13.0, sy + 13.0), Id::Accent(*c)));
        }
        let sx = ix + 13.0 + 8.0 * 36.0;
        if !known { p.ring(sx, sy, 16.0, pal.fg, 2.0); }
        p.rainbow(sx, sy, if is_h(Id::AccentCustom) { 14.0 } else { 13.0 })?;
        reg.push((rect(sx - 13.0, sy - 13.0, sx + 13.0, sy + 13.0), Id::AccentCustom));
        cy += 50.0 + 14.0;
        field_label(cy, "배경 불투명도", Some(format!("{}%", (s.opacity * 100.0).round())));
        slider(&mut reg, cy + 24.0, Sl::Opacity);
        y += h;
    }

    // ══════════ 크기 ══════════
    header(&mut y, "크기");
    {
        let hint2 = p.text_layout("위젯 오른쪽 끝을 끌어서 너비를, Ctrl + 마우스 휠로 전체 크기를 바꿀 수도 있어요.", 11.5, N, false, Some(iw));
        let h = 14.0 + 30.0 + 14.0 + 44.0 + 14.0 + 44.0 + 14.0 + hint2.h + 14.0;
        card(y, h);
        let mut cy = y + 14.0;
        let presets = [("작게", 132, 30), ("보통", 184, 40), ("넓게", 260, 40), ("크게", 300, 56)];
        let bw = (iw - 18.0) / 4.0;
        for (i, (label, w, hh)) in presets.iter().enumerate() {
            let bx = ix + i as f32 * (bw + 6.0);
            let sel = s.width == *w as f32 && s.height == *hh as f32;
            let r = rr(bx, cy, bw, 30.0, 8.0);
            p.fill_rr(&r, pal.card2);
            p.stroke_rr(&r, if sel { acc } else if is_h(Id::Preset(*w, *hh)) { alpha(acc, 0.6) } else { pal.line }, 1.0);
            p.text(label, 12.0, N, pal.fg, bx + bw / 2.0, cy + 15.0, Align::Center);
            reg.push((rect(bx, cy, bx + bw, cy + 30.0), Id::Preset(*w, *hh)));
        }
        cy += 30.0 + 14.0;
        field_label(cy, "너비", Some(format!("{}px", s.width)));
        slider(&mut reg, cy + 24.0, Sl::Width);
        cy += 44.0 + 14.0;
        field_label(cy, "높이", Some(format!("{}px", s.height)));
        slider(&mut reg, cy + 24.0, Sl::Height);
        cy += 44.0 + 14.0;
        p.draw_txt(&hint2, ix, cy, pal.faint);
        y += h;
    }

    // ══════════ 표시 ══════════
    header(&mut y, "표시");
    {
        let h = 14.0 + 60.0 + 14.0 + 20.0 + 14.0;
        card(y, h);
        let mut cy = y + 14.0;
        field_label(cy, "버튼", None);
        seg(&mut reg, cy + 24.0, &[
            ("항상", Id::Controls(Controls::Always), s.controls == Controls::Always),
            ("마우스 올릴 때", Id::Controls(Controls::Hover), s.controls == Controls::Hover),
            ("숨김", Id::Controls(Controls::Never), s.controls == Controls::Never),
        ]);
        cy += 60.0 + 14.0;
        toggle(&mut reg, cy, "1/100초 표시", None, s.show_centis, Flag::Centis);
        y += h;
    }

    // ══════════ 동작 ══════════
    header(&mut y, "동작");
    {
        let h = 14.0 + 36.0 * 3.0 + 20.0 + 32.0 + 14.0 * 4.0 + 14.0;
        card(y, h);
        let mut cy = y + 14.0;
        cy += toggle(&mut reg, cy, "항상 위에 표시", Some("작업표시줄 위에 고정"), s.always_on_top, Flag::Topmost) + 14.0;
        cy += toggle(&mut reg, cy, "자리 비움 자동 정지", Some("PC 잠금·절전 시 멈추고 돌아오면 이어서"), s.auto_pause, Flag::AutoPause) + 14.0;
        cy += toggle(&mut reg, cy, "전체 화면 앱 실행 중엔 숨기기", Some("게임·발표 중에는 위젯을 잠시 숨김"), s.hide_fullscreen, Flag::Fullscreen) + 14.0;
        cy += toggle(&mut reg, cy, "Windows 시작 시 실행", None, s.open_at_login, Flag::Login) + 14.0;
        let w = p.measure("위젯 위치 초기화", 13.0, N) + 32.0;
        pill(&mut reg, ix + iw - w, cy, w, "위젯 위치 초기화", Id::ResetPos, 0);
        y += h;
    }

    // ══════════ 팁 ══════════
    {
        y += 10.0 + 6.0;
        let tx = x0 + 4.0;
        p.text("위젯 단축 조작", 12.0, DWRITE_FONT_WEIGHT_BOLD, pal.dim, tx, y + 8.0, Align::Left);
        y += 16.0 + 6.0;
        let items: [&[(bool, &str)]; 4] = [
            &[(true, "더블클릭"), (false, "시간 직접 입력 (입력 중"), (true, "↑"), (true, "↓"), (false, "·휠로 1분,"), (true, "Shift"), (false, "10분)")],
            &[(true, "Space"), (false, "시작/정지 ·"), (true, "↑"), (true, "↓"), (false, "±1분")],
            &[(true, "우클릭"), (false, "메뉴 · 끌어서 이동 (작업표시줄 근처에서 자동 정렬)")],
            &[(false, "초기화 버튼은 실수 방지를 위해 두 번 눌러야 해요")],
        ];
        let left = tx + 18.0;
        let right = x0 + cw - 4.0;
        for item in items.iter() {
            p.circle(tx + 8.0, y + 9.0, 2.0, pal.dim);
            let mut x = left;
            // 단어 단위로 줄바꿈
            for (kbd, text) in item.iter() {
                let words: Vec<&str> = if *kbd { vec![*text] } else { text.split(' ').collect() };
                for wd in words {
                    if wd.is_empty() { continue; }
                    let (size, padx) = if *kbd { (11.0, 5.0) } else { (12.0, 0.0) };
                    let tl = p.text_layout(wd, size, N, false, None);
                    let ww = tl.w + padx * 2.0;
                    if x + ww > right && x > left { x = left; y += 20.0; }
                    if *kbd {
                        let r = rr(x, y + 1.0, ww, 17.0, 4.0);
                        p.fill_rr(&r, pal.card2);
                        p.stroke_rr(&r, pal.line, 1.0);
                        p.draw_txt(&tl, x + padx, y + 9.5 - tl.h / 2.0, pal.fg);
                    } else {
                        p.draw_txt(&tl, x, y + 9.0 - tl.h / 2.0, pal.dim);
                    }
                    x += ww + if *kbd { 3.0 } else { 4.0 };
                }
            }
            y += 20.0 + 4.0;
        }
    }
    y += 10.0;
    p.text(concat!("Taskbar Stopwatch v", env!("CARGO_PKG_VERSION"), " · MIT License"), 11.0, N, pal.faint, x0 + cw / 2.0, y + 8.0, Align::Center);
    y += 16.0 + 20.0;
    ui.content_h = y;
    ui.scroll = ui.scroll.clamp(0.0, ui.max_scroll());

    // 스크롤바
    unsafe { p.rt.SetTransform(&tr(0.0, 0.0)); }
    if ui.max_scroll() > 0.0 {
        let th = (vh * vh / ui.content_h).max(30.0);
        let ty = ui.scroll / ui.max_scroll() * (vh - th);
        let c = if is_h(Id::Scroll) || ui.drag_scroll.is_some() { pal.faint } else { mix(pal.line, pal.faint, 0.4) };
        p.fill_rr(&rr(vw - 7.0, ty + 3.0, 4.0, th - 6.0, 2.0), c);
    }

    unsafe {
        let _ = p.rt.EndDraw(None, None);
    }
    ui.regions = reg;
    Ok(())
}

fn blink_on(ui: &UiState) -> bool { (ui.blink0.elapsed().as_millis() / 530) % 2 == 0 }

// ───────────── 실제 창 ─────────────
pub struct SettingsWin {
    pub hwnd: HWND,
    hrt: ID2D1HwndRenderTarget,
    painter: Painter,
    pub ui: UiState,
    pub dpi: u32,
}

impl SettingsWin {
    pub fn scale(&self) -> f32 { self.dpi as f32 / 96.0 }

    /// 창 만들기 (owner 모니터 가운데)
    pub fn create(hinst: HINSTANCE, near: HWND, icon_big: HICON, icon_small: HICON) -> Result<SettingsWin> {
        unsafe {
            let mon = MonitorFromWindow(near, MONITOR_DEFAULTTOPRIMARY);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = windows::Win32::UI::HiDpi::GetDpiForMonitor(mon, windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let k = dx as f32 / 96.0;
            let wa = mi.rcWork;
            let (w, h) = ((BASE_W * k) as i32, ((BASE_H * k) as i32).min(wa.bottom - wa.top - 20));
            let x = wa.left + (wa.right - wa.left - w) / 2;
            let y = wa.top + (wa.bottom - wa.top - h) / 2;
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0), CLASS, w!("스톱워치 설정"), WS_OVERLAPPEDWINDOW,
                x, y, w, h, None, None, hinst, None,
            )?;
            SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(icon_big.0 as isize));
            SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_SMALL as usize), LPARAM(icon_small.0 as isize));
            ImmAssociateContext(hwnd, None);

            let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96);
            let light = system_light();
            apply_titlebar(hwnd, light);

            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: dpi as f32,
                dpiY: dpi as f32,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let hp = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U { width: rc.right.max(1) as u32, height: rc.bottom.max(1) as u32 },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let hrt = d2d.CreateHwndRenderTarget(&props, &hp)?;
            let painter = Painter::new(d2d, dw, hrt.cast()?)?;
            let k = dpi as f32 / 96.0;
            let ui = UiState::new(light, (rc.right as f32 / k, rc.bottom as f32 / k));
            SetTimer(hwnd, T_BLINK, 250, None);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            Ok(SettingsWin { hwnd, hrt, painter, ui, dpi })
        }
    }

    pub fn resized(&mut self) {
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut rc);
            let _ = self.hrt.Resize(&D2D_SIZE_U { width: rc.right.max(1) as u32, height: rc.bottom.max(1) as u32 });
            let k = self.scale();
            self.ui.view = (rc.right as f32 / k, rc.bottom as f32 / k);
            self.ui.scroll_by(0.0);
            self.invalidate();
        }
    }

    pub fn set_dpi(&mut self, dpi: u32) {
        self.dpi = dpi;
        unsafe { self.hrt.SetDpi(dpi as f32, dpi as f32); }
    }

    pub fn set_light(&mut self, light: bool) {
        if self.ui.light != light {
            self.ui.light = light;
            apply_titlebar(self.hwnd, light);
            self.invalidate();
        }
    }

    pub fn invalidate(&self) {
        unsafe { let _ = InvalidateRect(self.hwnd, None, false); }
    }

    pub fn paint(&mut self, s: &Settings, t: &Timer) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(self.hwnd, &mut ps);
            let _ = paint(&self.painter, &mut self.ui, s, t);
            let _ = EndPaint(self.hwnd, &ps);
        }
    }

    /// 마우스 좌표(물리 픽셀) → DIP
    pub fn dip(&self, lp: LPARAM) -> (f32, f32) {
        let x = (lp.0 & 0xffff) as i16 as f32;
        let y = ((lp.0 >> 16) & 0xffff) as i16 as f32;
        (x / self.scale(), y / self.scale())
    }

    /// 주기 타이머: 깜빡임·진행 시간·초기화 확인 해제가 필요할 때만 다시 그림
    pub fn tick(&mut self, running: bool) {
        let armed_expired = self.ui.armed_until.map_or(false, |t| Instant::now() >= t);
        if armed_expired { self.ui.armed_until = None; }
        if running || self.ui.input_focus || armed_expired { self.invalidate(); }
    }
}

fn apply_titlebar(hwnd: HWND, light: bool) {
    unsafe {
        let dark: BOOL = (!light).into();
        let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(20), &dark as *const BOOL as *const c_void, 4);
        let bg = palette(light).bg;
        let cref = COLORREF(((bg & 0xff) << 16) | (bg & 0xff00) | ((bg >> 16) & 0xff));
        let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(35), &cref as *const COLORREF as *const c_void, 4);
    }
}

// ───────────── 개발용: PNG 저장 ─────────────
fn save_png(wic: &IWICImagingFactory, path: &str, bmp: &IWICBitmap, w: u32, h: u32) -> Result<()> {
    unsafe {
        let stream = wic.CreateStream()?;
        let wpath: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        stream.InitializeFromFilename(PCWSTR(wpath.as_ptr()), GENERIC_WRITE.0)?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame: Option<IWICBitmapFrameEncode> = None;
        let mut bag = None;
        enc.CreateNewFrame(&mut frame, &mut bag)?;
        let frame = frame.unwrap();
        frame.Initialize(None)?;
        frame.SetSize(w, h)?;
        let mut pf = GUID_WICPixelFormat32bppPBGRA;
        frame.SetPixelFormat(&mut pf)?;
        frame.WriteSource(bmp, std::ptr::null())?;
        frame.Commit()?;
        enc.Commit()?;
        Ok(())
    }
}

/// BGRA(알파 곱해짐) 픽셀을 PNG 로
pub fn save_pixels(path: &str, w: u32, h: u32, px: &[u8]) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let bmp = wic.CreateBitmapFromMemory(w, h, &GUID_WICPixelFormat32bppPBGRA, w * 4, px)?;
        save_png(&wic, path, &bmp, w, h)
    }
}

// ───────────── 개발용: 설정 화면을 PNG 로 저장 ─────────────
pub fn snapshot(path: &str, light: bool, width: f32, s: &Settings, t: &Timer, input: &str) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0, dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        // 1차: 높이 측정
        let mut ui = UiState::new(light, (width, 4000.0));
        ui.input = input.to_string();
        ui.input_focus = !input.is_empty();
        let (w, mut h) = (width as u32, 4000u32);
        for pass in 0..2 {
            let bmp = wic.CreateBitmap(w, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)?;
            let rt = d2d.CreateWicBitmapRenderTarget(&bmp, &props)?;
            let painter = Painter::new(d2d.clone(), dw.clone(), rt)?;
            ui.view = (width, h as f32);
            paint(&painter, &mut ui, s, t)?;
            if pass == 0 { h = ui.content_h.ceil() as u32; continue; }
            save_png(&wic, path, &bmp, w, h)?;
        }
        Ok(())
    }
}

