//! 설정 · 타이머 상태와 저장 (간단한 key=value 텍스트 파일)

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme { Analog, Digital, Lcd }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Controls { Always, Hover, Never }

pub const MIN_W: f32 = 60.0;
pub const MAX_W: f32 = 640.0;
pub const MIN_H: f32 = 22.0;
pub const MAX_H: f32 = 120.0;
pub const DEFAULT_W: f32 = 184.0;
pub const DEFAULT_H: f32 = 40.0;

#[derive(Clone, Debug)]
pub struct Settings {
    pub theme: Theme,
    pub dark: bool,
    pub accent: u32, // 0xRRGGBB
    pub opacity: f32,
    pub width: f32, // DIP
    pub height: f32,
    pub x: Option<i32>, // 물리 픽셀 (화면 좌표)
    pub y: Option<i32>,
    pub controls: Controls,
    pub show_centis: bool,
    pub always_on_top: bool,
    pub auto_pause: bool,
    pub open_at_login: bool,
    pub hide_fullscreen: bool, // 전체 화면 앱(게임·발표) 중엔 숨김
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: Theme::Digital,
            dark: true,
            accent: 0x4f8cff,
            opacity: 0.96,
            width: DEFAULT_W,
            height: DEFAULT_H,
            x: None,
            y: None,
            controls: Controls::Always,
            show_centis: false,
            always_on_top: true,
            auto_pause: true,
            open_at_login: false,
            hide_fullscreen: true,
        }
    }
}

pub fn clamp_size(w: f32, h: f32) -> (f32, f32) {
    let w = if w.is_finite() { w } else { DEFAULT_W };
    let h = if h.is_finite() { h } else { DEFAULT_H };
    (w.round().clamp(MIN_W, MAX_W), h.round().clamp(MIN_H, MAX_H))
}

/// 측정 시간 상한: 9999시간 (값이 비정상적으로 커져 넘치는 일 방지)
pub const MAX_MS: i64 = 9_999 * 3_600_000;

// ───────────── 파일 경로 ─────────────
fn data_dir() -> PathBuf {
    let base = std::env::var_os("SW_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("TaskbarStopwatch"));
    let _ = fs::create_dir_all(&base);
    base
}
fn settings_path() -> PathBuf { data_dir().join("settings.ini") }
fn timer_path() -> PathBuf { data_dir().join("timer.ini") }

fn read_kv(path: &PathBuf) -> Option<HashMap<String, String>> {
    let text = fs::read_to_string(path).ok()?;
    Some(
        text.lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect(),
    )
}

fn hex_color(s: &str) -> Option<u32> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 { return None; }
    u32::from_str_radix(s, 16).ok()
}

impl Settings {
    fn apply_map(&mut self, m: &HashMap<String, String>) {
        let b = |k: &str| m.get(k).map(|v| v == "true" || v == "1");
        // NaN·무한대 같은 값은 무시
        let f = |k: &str| m.get(k).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite());
        if let Some(v) = m.get("theme") {
            self.theme = match v.as_str() { "analog" => Theme::Analog, "lcd" => Theme::Lcd, _ => Theme::Digital };
        }
        if let Some(v) = m.get("mode") { self.dark = v != "light"; }
        if let Some(c) = m.get("accent").and_then(|v| hex_color(v)) { self.accent = c; }
        if let Some(v) = f("opacity") { self.opacity = v.clamp(0.0, 1.0); }
        if let Some(v) = f("width") { self.width = v; }
        if let Some(v) = f("height") { self.height = v; }
        let (w, h) = clamp_size(self.width, self.height);
        self.width = w;
        self.height = h;
        // 화면 좌표로 말이 되는 범위만
        let pos = |k: &str| m.get(k).and_then(|v| v.parse::<i32>().ok()).filter(|v| v.abs() < 100_000);
        self.x = pos("x");
        self.y = pos("y");
        if let Some(v) = m.get("controls") {
            self.controls = match v.as_str() { "hover" => Controls::Hover, "never" => Controls::Never, _ => Controls::Always };
        }
        if let Some(v) = b("showCentis") { self.show_centis = v; }
        if let Some(v) = b("alwaysOnTop") { self.always_on_top = v; }
        if let Some(v) = b("autoPause") { self.auto_pause = v; }
        if let Some(v) = b("openAtLogin") { self.open_at_login = v; }
        if let Some(v) = b("hideFullscreen") { self.hide_fullscreen = v; }
    }

    pub fn load() -> Settings {
        let mut s = Settings::default();
        if let Some(m) = read_kv(&settings_path()) {
            s.apply_map(&m);
        }
        s
    }

    pub fn save(&self) {
        let theme = match self.theme { Theme::Analog => "analog", Theme::Digital => "digital", Theme::Lcd => "lcd" };
        let controls = match self.controls { Controls::Always => "always", Controls::Hover => "hover", Controls::Never => "never" };
        let opt = |v: Option<i32>| v.map(|v| v.to_string()).unwrap_or_default();
        let text = format!(
            "theme={theme}\nmode={}\naccent=#{:06x}\nopacity={}\nwidth={}\nheight={}\nx={}\ny={}\ncontrols={controls}\nshowCentis={}\nalwaysOnTop={}\nautoPause={}\nopenAtLogin={}\nhideFullscreen={}\n",
            if self.dark { "dark" } else { "light" },
            self.accent, self.opacity, self.width, self.height, opt(self.x), opt(self.y),
            self.show_centis, self.always_on_top, self.auto_pause, self.open_at_login, self.hide_fullscreen,
        );
        let _ = fs::write(settings_path(), text);
    }
}

// ───────────── 타이머 ─────────────
#[derive(Clone, Copy, Debug, Default)]
pub struct Timer {
    pub acc: i64,
    pub started_at: Option<i64>,
}

impl Timer {
    pub fn elapsed(&self) -> i64 {
        let run = self.started_at.map(|t| (now_ms() - t).max(0)).unwrap_or(0);
        self.acc.saturating_add(run).clamp(0, MAX_MS)
    }
    pub fn running(&self) -> bool { self.started_at.is_some() }
    pub fn start(&mut self) {
        if self.started_at.is_none() { self.started_at = Some(now_ms()); }
    }
    pub fn pause(&mut self) {
        if self.started_at.is_some() {
            self.acc = self.elapsed();
            self.started_at = None;
        }
    }
    pub fn toggle(&mut self) {
        if self.running() { self.pause() } else { self.start() }
    }
    pub fn reset(&mut self) { *self = Timer::default(); }
    pub fn set(&mut self, ms: i64) {
        self.acc = ms.clamp(0, MAX_MS);
        if self.started_at.is_some() { self.started_at = Some(now_ms()); }
    }
    pub fn adjust(&mut self, delta: i64) { self.set(self.elapsed().saturating_add(delta)); }

    pub fn load() -> Timer {
        let Some(m) = read_kv(&timer_path()) else { return Timer::default() };
        let num = |k: &str| m.get(k).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite()).map(|v| v as i64);
        let acc = num("acc").unwrap_or(0).clamp(0, MAX_MS);
        // 미래 시각이면 지금으로 맞춤
        let started_at = num("startedAt").filter(|v| *v > 0).map(|v| v.min(now_ms()));
        Timer { acc, started_at }
    }

    pub fn save(&self) {
        let text = format!(
            "acc={}\nstartedAt={}\n",
            self.acc,
            self.started_at.map(|v| v.to_string()).unwrap_or_default()
        );
        let _ = fs::write(timer_path(), text);
    }
}
