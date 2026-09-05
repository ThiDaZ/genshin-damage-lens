#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::{HWND, POINT, RECT};
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::ClientToScreen;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetClientRect, IsWindowVisible, IsIconic,
};

#[derive(Debug, Clone, Copy)]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[cfg(windows)]
fn to_wide_chars(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub struct WindowFinder;

impl WindowFinder {
    #[cfg(windows)]
    pub fn find_genshin() -> Option<HWND> {
        // Genshin Impact window class is typically UnityWndClass
        let class_name = to_wide_chars("UnityWndClass");

        // Try Global English title
        let title_en = to_wide_chars("Genshin Impact");
        let hwnd = unsafe {
            FindWindowW(
                PCWSTR(class_name.as_ptr()),
                PCWSTR(title_en.as_ptr()),
            )
        };
        if let Ok(h) = hwnd {
            if !h.0.is_null() && unsafe { IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() } {
                return Some(h);
            }
        }

        // Try CN title
        let title_cn = to_wide_chars("原神");
        let hwnd_cn = unsafe {
            FindWindowW(
                PCWSTR(class_name.as_ptr()),
                PCWSTR(title_cn.as_ptr()),
            )
        };
        if let Ok(h) = hwnd_cn {
            if !h.0.is_null() && unsafe { IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() } {
                return Some(h);
            }
        }

        None
    }

    #[cfg(not(windows))]
    pub fn find_genshin() -> Option<()> {
        None
    }

    #[cfg(windows)]
    pub fn get_client_rect(hwnd: HWND) -> Option<WindowRect> {
        unsafe {
            if IsIconic(hwnd).as_bool() {
                return None;
            }

            let mut client_rect = RECT::default();
            if GetClientRect(hwnd, &mut client_rect).is_err() {
                return None;
            }

            let mut pt = POINT {
                x: client_rect.left,
                y: client_rect.top,
            };
            if !ClientToScreen(hwnd, &mut pt).as_bool() {
                return None;
            }

            let width = (client_rect.right - client_rect.left).max(0) as u32;
            let height = (client_rect.bottom - client_rect.top).max(0) as u32;

            // Real Genshin client window is at least 640x480 and positioned on screen
            if width < 640 || height < 480 || pt.x < -1000 || pt.y < -1000 {
                return None;
            }

            Some(WindowRect {
                x: pt.x,
                y: pt.y,
                width,
                height,
            })
        }
    }

    #[cfg(not(windows))]
    pub fn get_client_rect(_: ()) -> Option<WindowRect> {
        None
    }
}
