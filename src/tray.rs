//! 알림 영역(트레이) 아이콘

use std::ffi::c_void;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_APP_TRAY: u32 = WM_APP + 1;

/// 스톱워치 모양 아이콘을 코드로 그림 (4x4 슈퍼샘플링)
pub fn make_icon(size: i32, rgb: u32) -> Result<HICON> {
    unsafe {
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let px = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
        let k = size as f32 / 32.0;
        let inside = |x: f32, y: f32| -> bool {
            // 32 단위 설계 좌표
            let (dx, dy) = (x / k - 16.0, y / k - 17.0);
            let r = dx.hypot(dy);
            (r > 10.2 && r < 13.4)                          // 테두리
                || (dx.abs() < 1.5 && dy < 1.0 && dy > -8.0)  // 바늘
                || r < 2.4                                   // 축
                || (dx.abs() < 2.4 && dy > -16.6 && dy < -14.0) // 꼭지
        };
        for y in 0..size {
            for x in 0..size {
                let mut cov = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        if inside(x as f32 + (sx as f32 + 0.5) / 4.0, y as f32 + (sy as f32 + 0.5) / 4.0) { cov += 1; }
                    }
                }
                let i = ((y * size + x) * 4) as usize;
                px[i] = (rgb & 255) as u8;
                px[i + 1] = ((rgb >> 8) & 255) as u8;
                px[i + 2] = ((rgb >> 16) & 255) as u8;
                px[i + 3] = (cov * 255 / 16) as u8;
            }
        }
        let mask = CreateBitmap(size, size, 1, 1, None);
        let icon = CreateIconIndirect(&ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color })?;
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        Ok(icon)
    }
}

pub struct Tray {
    nid: NOTIFYICONDATAW,
    tip: String,
}

impl Tray {
    pub fn new(hwnd: HWND, icon: HICON) -> Tray {
        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: icon,
            ..Default::default()
        };
        let mut t = Tray { nid, tip: String::new() };
        t.add();
        t
    }

    pub fn add(&mut self) {
        unsafe { let _ = Shell_NotifyIconW(NIM_ADD, &self.nid); }
    }

    pub fn set_tip(&mut self, tip: &str) {
        if self.tip == tip { return; }
        self.tip = tip.to_string();
        let wide: Vec<u16> = tip.encode_utf16().take(127).collect();
        self.nid.szTip = [0; 128];
        self.nid.szTip[..wide.len()].copy_from_slice(&wide);
        unsafe { let _ = Shell_NotifyIconW(NIM_MODIFY, &self.nid); }
    }

    pub fn remove(&self) {
        unsafe { let _ = Shell_NotifyIconW(NIM_DELETE, &self.nid); }
    }
}
