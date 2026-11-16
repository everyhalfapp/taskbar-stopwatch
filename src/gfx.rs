//! Direct2D(소프트웨어) + DirectWrite 렌더러.
//! 32bpp DIB 에 그린 뒤 UpdateLayeredWindow 로 픽셀 단위 투명 창에 올린다.

use std::ffi::c_void;
use windows::core::*;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;

use crate::model::{Controls, Settings, Theme};
use crate::timefmt;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit { None, Face, Play, Reset, Grip }

// ───────────── 레이아웃 (히트 테스트와 공유) ─────────────
pub struct Layout {
    pub u: f32,
    pub btn: f32,
    pub play: (f32, f32),
    pub reset: (f32, f32),
    pub show_controls: bool,
    pub face_l: f32,
    pub face_r: f32,
    pub grip_x: f32,
    pub edit: (f32, f32, f32, f32),
}

pub fn layout(w: i32, h: i32, scale: f32, s: &Settings, inside: bool) -> Layout {
    let (wf, hf) = (w as f32, h as f32);
    let u = hf / 100.0;
    let btn = (62.0 * u).max(18.0 * scale).min(hf);
    let pad_l = if s.theme == Theme::Analog { 8.0 * u } else { 18.0 * u };
    let pad_r = 14.0 * u;
    let gap = 10.0 * u;
    let show_controls = match s.controls { Controls::Always => true, Controls::Hover => inside, Controls::Never => false };
    let cy = hf / 2.0;
    let reset = (wf - pad_r - btn / 2.0, cy);
    let play = (reset.0 - btn - 2.0 * u, cy);
    let face_r = if s.controls == Controls::Always { play.0 - btn / 2.0 - gap } else { wf - pad_r };
    Layout {
        u, btn, play, reset, show_controls,
        face_l: pad_l,
        face_r: face_r.max(pad_l + 1.0),
        grip_x: wf - 7.0 * scale,
        edit: (8.0 * u, 14.0 * u, wf - 8.0 * u, hf - 14.0 * u),
    }
}

pub fn hit_test(l: &Layout, x: f32, y: f32) -> Hit {
    if x >= l.grip_x { return Hit::Grip; }
    if l.show_controls {
        let r = l.btn / 2.0;
        if (x - l.play.0).hypot(y - l.play.1) <= r { return Hit::Play; }
        if (x - l.reset.0).hypot(y - l.reset.1) <= r { return Hit::Reset; }
    }
    Hit::Face
}

// ───────────── 장면 ─────────────
pub struct EditView<'a> { pub text: &'a str, pub sel_all: bool, pub bad: bool }

pub struct Scene<'a> {
    pub s: &'a Settings,
    pub w: i32,
    pub h: i32,
    pub scale: f32,
    pub ms: i64,
    pub running: bool,
    pub hover: Hit,
    pub inside: bool,
    pub armed: bool,
    pub edit: Option<EditView<'a>>,
}

// ───────────── 색 ─────────────
type C = D2D1_COLOR_F;
fn rgb(hex: u32, a: f32) -> C {
    C { r: ((hex >> 16) & 255) as f32 / 255.0, g: ((hex >> 8) & 255) as f32 / 255.0, b: (hex & 255) as f32 / 255.0, a }
}
fn alpha(c: C, a: f32) -> C { C { a: c.a * a, ..c } }
fn mix(a: C, b: C, t: f32) -> C {
    C { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }
}

struct Pal { bg: u32, fg: C, dim: C, line: C, hover: C, plate: C, tick: C, major: C, ghost: C, lcd: C }
fn palette(dark: bool) -> Pal {
    if dark {
        Pal { bg: 0x11131a, fg: rgb(0xeef1f6, 1.0), dim: rgb(0x8a93a3, 1.0), line: rgb(0xffffff, 0.09), hover: rgb(0xffffff, 0.09),
              plate: rgb(0xffffff, 0.04), tick: rgb(0xffffff, 0.28), major: rgb(0xffffff, 0.7), ghost: rgb(0xffffff, 0.055), lcd: rgb(0x000000, 0.28) }
    } else {
        Pal { bg: 0xf8f9fb, fg: rgb(0x161a21, 1.0), dim: rgb(0x6b7383, 1.0), line: rgb(0x000000, 0.09), hover: rgb(0x000000, 0.06),
              plate: rgb(0x000000, 0.03), tick: rgb(0x000000, 0.25), major: rgb(0x000000, 0.65), ghost: rgb(0x000000, 0.06), lcd: rgb(0x000000, 0.05) }
    }
}

// ───────────── 행렬 (행 벡터: a 먼저, 그다음 b) ─────────────
fn mat(m11: f32, m12: f32, m21: f32, m22: f32, dx: f32, dy: f32) -> Matrix3x2 {
    Matrix3x2 { M11: m11, M12: m12, M21: m21, M22: m22, M31: dx, M32: dy }
}
fn ident() -> Matrix3x2 { mat(1.0, 0.0, 0.0, 1.0, 0.0, 0.0) }
fn tr(x: f32, y: f32) -> Matrix3x2 { mat(1.0, 0.0, 0.0, 1.0, x, y) }
fn sc(s: f32) -> Matrix3x2 { mat(s, 0.0, 0.0, s, 0.0, 0.0) }
fn mul(a: Matrix3x2, b: Matrix3x2) -> Matrix3x2 {
    mat(
        a.M11 * b.M11 + a.M12 * b.M21,
        a.M11 * b.M12 + a.M12 * b.M22,
        a.M21 * b.M11 + a.M22 * b.M21,
        a.M21 * b.M12 + a.M22 * b.M22,
        a.M31 * b.M11 + a.M32 * b.M21 + b.M31,
        a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
    )
}

fn pt(x: f32, y: f32) -> D2D_POINT_2F { D2D_POINT_2F { x, y } }
fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F { D2D_RECT_F { left: l, top: t, right: r, bottom: b } }
fn rrect(l: f32, t: f32, r: f32, b: f32, rad: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: rect(l, t, r, b), radiusX: rad, radiusY: rad }
}
fn ellipse(x: f32, y: f32, r: f32) -> D2D1_ELLIPSE { D2D1_ELLIPSE { point: pt(x, y), radiusX: r, radiusY: r } }

// ───────────── 7세그먼트 형상 (숫자 1칸 = 10 x 18) ─────────────
pub const SEG_ON: [&str; 10] = ["abcdef", "bc", "abged", "abgcd", "fgbc", "afgcd", "afgedc", "abc", "abcdefg", "abcdfg"];

pub fn seg_points() -> [Vec<(f32, f32)>; 7] {
    let t = 1.9f32;
    let g = 0.32f32;
    let (l, r, top, mid, bot) = (t / 2.0, 10.0 - t / 2.0, t / 2.0, 9.0, 18.0 - t / 2.0);
    let hz = |x1: f32, x2: f32, y: f32| vec![(x1, y), (x1 + t / 2.0, y - t / 2.0), (x2 - t / 2.0, y - t / 2.0), (x2, y), (x2 - t / 2.0, y + t / 2.0), (x1 + t / 2.0, y + t / 2.0)];
    let vt = |x: f32, y1: f32, y2: f32| vec![(x, y1), (x + t / 2.0, y1 + t / 2.0), (x + t / 2.0, y2 - t / 2.0), (x, y2), (x - t / 2.0, y2 - t / 2.0), (x - t / 2.0, y1 + t / 2.0)];
    // 순서: a b c d e f g
    [
        hz(l + g, r - g, top),
        vt(r, top + g, mid - g),
        vt(r, mid + g, bot - g),
        hz(l + g, r - g, bot),
        vt(l, mid + g, bot - g),
        vt(l, top + g, mid - g),
        hz(l + g, r - g, mid),
    ]
}

struct Run { text: String, color: C, size: f32, weight: DWRITE_FONT_WEIGHT }

pub struct Gfx {
    _d2d: ID2D1Factory,
    dw: IDWriteFactory,
    rt: ID2D1DCRenderTarget,
    fmt: IDWriteTextFormat,
    typo: IDWriteTypography,
    round: ID2D1StrokeStyle,
    brush: ID2D1SolidColorBrush,
    segs: Vec<ID2D1PathGeometry>,
    play: ID2D1PathGeometry,
    reset_icon: ID2D1PathGeometry,
    pub dc: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    size: (i32, i32),
    bits: *mut c_void,
}

impl Gfx {
    pub fn new() -> Result<Gfx> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE, // GPU 드라이버를 올리지 않아 메모리가 적다
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = d2d.CreateDCRenderTarget(&props)?;
            let fmt = dw.CreateTextFormat(
                w!("Segoe UI Variable Display"),
                None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                100.0,
                w!("ko-kr"),
            )?;
            fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            let typo = dw.CreateTypography()?;
            typo.AddFontFeature(DWRITE_FONT_FEATURE { nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, parameter: 1 })?;
            let round = d2d.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.0,
                },
                None,
            )?;
            let brush = rt.CreateSolidColorBrush(&rgb(0xffffff, 1.0), None)?;

            let poly = |pts: &[(f32, f32)], closed: bool| -> Result<ID2D1PathGeometry> {
                let g = d2d.CreatePathGeometry()?;
                let sink = g.Open()?;
                sink.BeginFigure(pt(pts[0].0, pts[0].1), if closed { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
                let rest: Vec<D2D_POINT_2F> = pts[1..].iter().map(|p| pt(p.0, p.1)).collect();
                sink.AddLines(&rest);
                sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
                sink.Close()?;
                Ok(g)
            };
            let segs = seg_points().iter().map(|p| poly(p, true)).collect::<Result<Vec<_>>>()?;
            // 24x24 아이콘 좌표
            let play = poly(&[(8.6, 6.2), (8.6, 17.8), (18.4, 12.0)], true)?;
            let reset_icon = {
                let g = d2d.CreatePathGeometry()?;
                let sink = g.Open()?;
                sink.BeginFigure(pt(4.5, 12.0), D2D1_FIGURE_BEGIN_HOLLOW);
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: pt(6.7, 6.7),
                    size: D2D_SIZE_F { width: 7.5, height: 7.5 },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
                    arcSize: D2D1_ARC_SIZE_LARGE,
                });
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
                sink.BeginFigure(pt(4.0, 3.8), D2D1_FIGURE_BEGIN_HOLLOW);
                sink.AddLines(&[pt(4.0, 8.4), pt(8.6, 8.4)]);
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
                sink.Close()?;
                g
            };

            let dc = CreateCompatibleDC(None);
            Ok(Gfx { _d2d: d2d, dw, rt, fmt, typo, round, brush, segs, play, reset_icon, dc, dib: HBITMAP::default(), old: HGDIOBJ::default(), size: (0, 0), bits: std::ptr::null_mut() })
        }
    }

    fn ensure_dib(&mut self, w: i32, h: i32) -> Result<()> {
        if self.size == (w, h) && !self.dib.is_invalid() {
            return Ok(());
        }
        unsafe {
            if !self.dib.is_invalid() {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(HGDIOBJ(self.dib.0));
            }
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            self.dib = CreateDIBSection(self.dc, &bi, DIB_RGB_COLORS, &mut bits, None, 0)?;
            self.bits = bits;
            self.old = SelectObject(self.dc, HGDIOBJ(self.dib.0));
        }
        self.size = (w, h);
        Ok(())
    }

    // ── 기본 도형 ──
    fn fill_rr(&self, r: &D2D1_ROUNDED_RECT, c: C) {
        unsafe {
            self.brush.SetColor(&c);
            self.rt.FillRoundedRectangle(r, &self.brush);
        }
    }
    fn stroke_rr(&self, r: &D2D1_ROUNDED_RECT, c: C, w: f32) {
        unsafe {
            self.brush.SetColor(&c);
            self.rt.DrawRoundedRectangle(r, &self.brush, w, &self.round);
        }
    }
    fn fill_circle(&self, x: f32, y: f32, r: f32, c: C) {
        unsafe {
            self.brush.SetColor(&c);
            self.rt.FillEllipse(&ellipse(x, y, r), &self.brush);
        }
    }
    fn stroke_circle(&self, x: f32, y: f32, r: f32, c: C, w: f32) {
        unsafe {
            self.brush.SetColor(&c);
            self.rt.DrawEllipse(&ellipse(x, y, r), &self.brush, w, &self.round);
        }
    }
    fn line(&self, a: (f32, f32), b: (f32, f32), c: C, w: f32) {
        unsafe {
            self.brush.SetColor(&c);
            self.rt.DrawLine(pt(a.0, a.1), pt(b.0, b.1), &self.brush, w, &self.round);
        }
    }
    fn set_tf(&self, m: &Matrix3x2) {
        unsafe { self.rt.SetTransform(m) }
    }

    // ── 텍스트 ──
    /// 여러 색/크기 구간을 한 줄로. 반환: (layout, 너비, 기준선 y)
    fn text(&self, runs: &[Run], fs: f32) -> Result<(IDWriteTextLayout, f32, f32, Vec<ID2D1SolidColorBrush>)> {
        unsafe {
            let all: String = runs.iter().map(|r| r.text.as_str()).collect();
            let wide: Vec<u16> = all.encode_utf16().collect();
            let layout = self.dw.CreateTextLayout(&wide, &self.fmt, 100_000.0, 10_000.0)?;
            let total = DWRITE_TEXT_RANGE { startPosition: 0, length: wide.len() as u32 };
            layout.SetTypography(&self.typo, total)?;
            let mut brushes = Vec::new();
            let mut pos = 0u32;
            for r in runs {
                let len = r.text.encode_utf16().count() as u32;
                let range = DWRITE_TEXT_RANGE { startPosition: pos, length: len };
                layout.SetFontSize(fs * r.size, range)?;
                layout.SetFontWeight(r.weight, range)?;
                let b = self.rt.CreateSolidColorBrush(&r.color, None)?;
                layout.SetDrawingEffect(&b, range)?;
                brushes.push(b);
                pos += len;
            }
            let mut tm = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut tm)?;
            let mut lm = [DWRITE_LINE_METRICS::default(); 1];
            let mut n = 0u32;
            let _ = layout.GetLineMetrics(Some(&mut lm), &mut n);
            Ok((layout, tm.widthIncludingTrailingWhitespace, lm[0].baseline, brushes))
        }
    }

    /// 숫자 기준으로 세로 가운데 맞춰 그리기
    fn draw_text(&self, layout: &IDWriteTextLayout, x: f32, cy: f32, baseline: f32, fs: f32) {
        unsafe {
            self.brush.SetColor(&rgb(0xffffff, 1.0));
            let y = cy - baseline + fs * 0.355;
            self.rt.DrawTextLayout(pt(x, y), layout, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }

    /// 크기에 맞춰 줄인 텍스트 (fs, layout, width, baseline)
    fn fit_text(&self, runs: &[Run], fs0: f32, avail: f32) -> Result<(f32, IDWriteTextLayout, f32, f32, Vec<ID2D1SolidColorBrush>)> {
        let (l, w, b, br) = self.text(runs, fs0)?;
        if w <= avail || w <= 0.0 {
            return Ok((fs0, l, w, b, br));
        }
        let fs = (fs0 * avail / w).max(1.0);
        let (l, w, b, br) = self.text(runs, fs)?;
        Ok((fs, l, w, b, br))
    }

    // ───────────── 메인 렌더 ─────────────
    /// 마지막으로 그린 화면 (BGRA, 알파 곱해진 상태) — 개발용 스냅샷
    pub fn pixels(&self) -> Vec<u8> {
        unsafe {
            let _ = GdiFlush();
            let n = (self.size.0 * self.size.1 * 4) as usize;
            if self.bits.is_null() { return Vec::new(); }
            std::slice::from_raw_parts(self.bits as *const u8, n).to_vec()
        }
    }

    pub fn render(&mut self, sc: &Scene) -> Result<()> {
        let (w, h) = (sc.w, sc.h);
        self.ensure_dib(w, h)?;
        let s = sc.s;
        let p = palette(s.dark);
        let accent = rgb(s.accent, 1.0);
        let (wf, hf) = (w as f32, h as f32);
        let cy = hf / 2.0;
        let l = layout(w, h, sc.scale, s, sc.inside);
        let u = l.u;

        unsafe {
            self.rt.BindDC(self.dc, &RECT { left: 0, top: 0, right: w, bottom: h })?;
            self.rt.BeginDraw();
            self.rt.SetTransform(&ident());
            self.rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            self.rt.Clear(Some(&C { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        }

        // 배경 (완전 투명이면 클릭이 통과하므로 최소 알파 유지)
        let bw = sc.scale.max(1.0);
        let body = rrect(bw / 2.0, bw / 2.0, wf - bw / 2.0, hf - bw / 2.0, (30.0 * u).min(hf / 2.0 - 1.0));
        self.fill_rr(&body, rgb(p.bg, s.opacity.max(0.012)));
        if s.theme == Theme::Lcd {
            self.fill_rr(&body, p.lcd);
        }
        let border = if sc.running { mix(p.line, alpha(accent, 0.5), 0.55) } else { p.line };
        self.stroke_rr(&body, border, bw);

        let text_alpha = if sc.running { 1.0 } else { 0.62 };
        match s.theme {
            Theme::Digital => self.draw_digital(sc, &l, &p, accent, text_alpha)?,
            Theme::Analog => self.draw_analog(sc, &l, &p, accent, text_alpha)?,
            Theme::Lcd => self.draw_lcd(sc, &l, &p, accent),
        }

        // 버튼
        if l.show_controls {
            if s.controls == Controls::Hover {
                let pad = 4.0 * u;
                let r = rrect(l.play.0 - l.btn / 2.0 - pad, cy - l.btn / 2.0 - pad / 2.0, l.reset.0 + l.btn / 2.0 + pad, cy + l.btn / 2.0 + pad / 2.0, hf);
                self.fill_rr(&r, rgb(p.bg, 0.97));
            }
            let k = l.btn * 0.64 / 24.0;
            let icon_tf = |c: (f32, f32)| mul(sc_(k), tr(c.0 - 12.0 * k, c.1 - 12.0 * k));
            // 재생/일시정지
            let hov = sc.hover == Hit::Play;
            if hov { self.fill_circle(l.play.0, l.play.1, l.btn / 2.0, p.hover); }
            unsafe {
                self.brush.SetColor(&p.fg);
                self.set_tf(&icon_tf(l.play));
                if sc.running {
                    self.rt.FillRoundedRectangle(&rrect(6.5, 5.0, 10.5, 19.0, 1.3), &self.brush);
                    self.rt.FillRoundedRectangle(&rrect(13.5, 5.0, 17.5, 19.0, 1.3), &self.brush);
                } else {
                    self.rt.FillGeometry(&self.play, &self.brush, None);
                    self.rt.DrawGeometry(&self.play, &self.brush, 1.6, &self.round);
                }
                self.set_tf(&ident());
            }
            // 초기화
            let (bg, fg) = if sc.armed {
                (Some(rgb(0xe5484d, 1.0)), rgb(0xffffff, 1.0))
            } else if sc.hover == Hit::Reset {
                (Some(p.hover), p.fg)
            } else {
                (None, p.dim)
            };
            if let Some(bg) = bg { self.fill_circle(l.reset.0, l.reset.1, l.btn / 2.0, bg); }
            unsafe {
                self.brush.SetColor(&fg);
                self.set_tf(&icon_tf(l.reset));
                self.rt.DrawGeometry(&self.reset_icon, &self.brush, 2.2, &self.round);
                self.set_tf(&ident());
            }
        }

        // 너비 조절 손잡이 표시
        if sc.inside && sc.edit.is_none() {
            let c = if sc.hover == Hit::Grip { alpha(accent, 0.85) } else { alpha(p.dim, 0.35) };
            let gw = 3.0 * sc.scale;
            let x = wf - 2.0 * sc.scale - gw;
            self.fill_rr(&rrect(x, hf * 0.3, x + gw, hf * 0.7, gw / 2.0), c);
        }

        // 시간 직접 입력
        if let Some(ed) = &sc.edit {
            let (x0, y0, x1, y1) = l.edit;
            let r = rrect(x0, y0, x1, y1, ((y1 - y0) / 2.0).min(22.0 * u));
            self.fill_rr(&r, rgb(p.bg, 1.0));
            self.stroke_rr(&r, if ed.bad { rgb(0xe5484d, 1.0) } else { accent }, 1.5 * sc.scale);
            let fs = hf * 0.39;
            let avail = (x1 - x0) - 16.0 * sc.scale;
            let cx = (x0 + x1) / 2.0;
            if ed.text.is_empty() {
                let runs = [Run { text: "1:30:00 · 45 · +10 · -5m".into(), color: p.dim, size: 0.62, weight: DWRITE_FONT_WEIGHT_NORMAL }];
                let (fs2, lay, tw, base, _b) = self.fit_text(&runs, fs, avail)?;
                self.draw_text(&lay, cx - tw / 2.0, cy, base, fs2 * 0.62);
                self.line((cx - tw / 2.0 - 3.0 * sc.scale, cy - fs * 0.42), (cx - tw / 2.0 - 3.0 * sc.scale, cy + fs * 0.42), accent, 1.5 * sc.scale);
            } else {
                let runs = [Run { text: ed.text.to_string(), color: p.fg, size: 1.0, weight: DWRITE_FONT_WEIGHT_SEMI_BOLD }];
                let (fs2, lay, tw, base, _b) = self.fit_text(&runs, fs, avail)?;
                let x = cx - tw / 2.0;
                if ed.sel_all {
                    self.fill_rr(&rrect(x - 2.0, cy - fs2 * 0.55, x + tw + 2.0, cy + fs2 * 0.55, 3.0 * sc.scale), alpha(accent, 0.32));
                }
                self.draw_text(&lay, x, cy, base, fs2);
                if !ed.sel_all {
                    let cxr = x + tw + 1.5 * sc.scale;
                    self.line((cxr, cy - fs2 * 0.45), (cxr, cy + fs2 * 0.45), accent, 1.5 * sc.scale);
                }
            }
        }

        unsafe { self.rt.EndDraw(None, None)?; }
        Ok(())
    }

    fn time_runs(&self, sc: &Scene, p: &Pal, a: f32, pad_hours: bool) -> Vec<Run> {
        let t = timefmt::parts(sc.ms);
        let fg = alpha(p.fg, a);
        let dim = alpha(p.dim, a);
        let colon = alpha(p.fg, 0.45 * a);
        let semi = DWRITE_FONT_WEIGHT_SEMI_BOLD;
        let med = DWRITE_FONT_WEIGHT_MEDIUM;
        let mut v = vec![
            Run { text: if pad_hours { format!("{:02}", t.h) } else { t.h.to_string() }, color: fg, size: 1.0, weight: semi },
            Run { text: ":".into(), color: colon, size: 1.0, weight: semi },
            Run { text: format!("{:02}", t.m), color: fg, size: 1.0, weight: semi },
            Run { text: ":".into(), color: colon, size: 1.0, weight: semi },
            Run { text: format!("{:02}", t.s), color: dim, size: 1.0, weight: med },
        ];
        if sc.s.show_centis {
            v.push(Run { text: format!("{:02}", t.cs), color: dim, size: 0.62, weight: med });
        }
        v
    }

    fn draw_digital(&self, sc: &Scene, l: &Layout, p: &Pal, accent: C, a: f32) -> Result<()> {
        let hf = sc.h as f32;
        let cy = hf / 2.0;
        let runs = self.time_runs(sc, p, a, true);
        let avail = l.face_r - l.face_l;
        let fs0 = hf * 0.5;
        // 점 + 간격 + 글자 = fs*(0.3+0.38) + 글자폭(fs 비례)
        let (_, _, w0, _, _) = self.fit_text(&runs, fs0, f32::MAX)?;
        let total0 = fs0 * 0.68 + w0;
        let fs = if total0 > avail { (fs0 * avail / total0).max(4.0) } else { fs0 };
        let (lay, tw, base, _b) = self.text(&runs, fs)?;
        let d = fs * 0.3;
        let total = d + fs * 0.38 + tw;
        let x0 = l.face_l + (avail - total) / 2.0;
        if sc.running {
            self.fill_circle(x0 + d / 2.0, cy, d / 2.0, accent);
        } else {
            self.stroke_circle(x0 + d / 2.0, cy, d / 2.0 - 0.75 * sc.scale, p.dim, 1.5 * sc.scale);
        }
        self.draw_text(&lay, x0 + d + fs * 0.38, cy, base, fs);
        Ok(())
    }

    fn draw_analog(&self, sc: &Scene, l: &Layout, p: &Pal, accent: C, a: f32) -> Result<()> {
        let hf = sc.h as f32;
        let cy = hf / 2.0;
        let u = l.u;
        let dia = hf - 16.0 * u;
        let k = dia / 100.0;
        let cx = l.face_l + dia / 2.0;
        // 판
        self.fill_circle(cx, cy, 47.0 * k, p.plate);
        self.stroke_circle(cx, cy, 47.0 * k, p.line, 2.0 * k);
        let at = |r: f32, deg: f32| {
            let t = deg.to_radians();
            (cx + t.sin() * r * k, cy - t.cos() * r * k)
        };
        for i in 0..60 {
            let major = i % 5 == 0;
            let deg = i as f32 * 6.0;
            let r1 = if major { 35.0 } else { 39.0 };
            self.line(at(r1, deg), at(43.0, deg), if major { p.major } else { p.tick }, if major { 3.0 * k } else { 1.6 * k });
        }
        // 바늘
        let sec = sc.ms as f32 / 1000.0;
        let hand = |deg: f32, tail: f32, len: f32, c: C, w: f32| {
            self.line(at(-tail, deg), at(len, deg), c, w * k);
        };
        hand(((sec / 3600.0) % 12.0) * 30.0, 5.0, 22.0, alpha(p.fg, 0.55), 6.0);
        hand(((sec / 60.0) % 60.0) * 6.0, 6.0, 34.0, p.fg, 4.5);
        hand((sec % 60.0) * 6.0, 11.0, 41.0, accent, 2.4);
        self.fill_circle(cx, cy, 4.0 * k, accent);
        self.fill_circle(cx, cy, 1.6 * k, rgb(p.bg, 1.0));

        // 보조 숫자
        let x = l.face_l + dia + 12.0 * u;
        let room = l.face_r - x;
        if room > 8.0 * sc.scale {
            let runs = self.time_runs(sc, p, a, false);
            let (fs, lay, _tw, base, _b) = self.fit_text(&runs, hf * 0.4, room)?;
            if fs >= 8.0 * sc.scale {
                self.draw_text(&lay, x, cy, base, fs);
            }
        }
        Ok(())
    }

    fn draw_lcd(&self, sc: &Scene, l: &Layout, p: &Pal, accent: C) {
        let hf = sc.h as f32;
        let cy = hf / 2.0;
        let text = timefmt::format(sc.ms, sc.s.show_centis);
        // 단위 좌표에서 배치 계산
        enum G { Digit(usize, f32, f32), Colon(f32), Dot(f32) }
        let mut glyphs = Vec::new();
        let (mut x, mut small) = (0.0f32, false);
        for ch in text.chars() {
            match ch {
                ':' => { glyphs.push(G::Colon(x)); x += 5.0; }
                '.' => { glyphs.push(G::Dot(x)); x += 4.0; small = true; }
                c => {
                    let s = if small { 0.62 } else { 1.0 };
                    glyphs.push(G::Digit(c.to_digit(10).unwrap_or(8) as usize, x, s));
                    x += 13.0 * s;
                }
            }
        }
        let units_w = x - 3.0 + 4.0;
        let avail = l.face_r - l.face_l;
        let mut px = hf * 0.5 * 1.08 / 20.0; // 단위 → 픽셀
        if units_w * px > avail { px = avail / units_w; }
        let ox = l.face_l + (avail - units_w * px) / 2.0;
        let oy = cy - 10.0 * px;
        let skew = mat(1.0, 0.0, (-7.0f32).to_radians().tan(), 1.0, 0.0, 0.0);
        let base = mul(mul(mul(tr(2.4, 0.0), skew), tr(1.0, 1.0)), mul(sc_(px), tr(ox, oy)));

        let on_c = if sc.running { accent } else { alpha(accent, 0.55) };
        unsafe {
            for g in &glyphs {
                match *g {
                    G::Digit(d, gx, s) => {
                        self.set_tf(&mul(mul(sc_(s), tr(gx, 18.0 * (1.0 - s))), base));
                        let on = SEG_ON[d.min(9)];
                        for (i, seg) in self.segs.iter().enumerate() {
                            let name = b"abcdefg"[i] as char;
                            self.brush.SetColor(&if on.contains(name) { on_c } else { p.ghost });
                            self.rt.FillGeometry(seg, &self.brush, None);
                        }
                    }
                    G::Colon(gx) => {
                        self.set_tf(&base);
                        self.brush.SetColor(&on_c);
                        self.rt.FillRoundedRectangle(&rrect(gx + 1.0, 4.6, gx + 3.0, 6.6, 0.5), &self.brush);
                        self.rt.FillRoundedRectangle(&rrect(gx + 1.0, 11.4, gx + 3.0, 13.4, 0.5), &self.brush);
                    }
                    G::Dot(gx) => {
                        self.set_tf(&base);
                        self.brush.SetColor(&on_c);
                        self.rt.FillRoundedRectangle(&rrect(gx + 0.6, 16.0, gx + 2.6, 18.0, 0.5), &self.brush);
                    }
                }
            }
            self.set_tf(&ident());
        }
    }
}

fn sc_(s: f32) -> Matrix3x2 { sc(s) }

impl Drop for Gfx {
    fn drop(&mut self) {
        unsafe {
            if !self.dib.is_invalid() {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(HGDIOBJ(self.dib.0));
            }
            let _ = DeleteDC(self.dc);
        }
    }
}

