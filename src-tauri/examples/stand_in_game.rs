//! Play a recording in a window the app takes for the game.
//!
//! ffmpeg -v error -i clip.mp4 -f rawvideo -pix_fmt bgra - |
//!     cargo run --release --example stand_in_game -- 1280 720 30 [x y]
//!
//! The app looks for a window of class `UnityWndClass` titled `Genshin
//! Impact`. This opens one, without a border, at the top-left corner of the
//! primary display (or at `x y`), and shows the frames it reads from standard
//! input at the given rate, pixel for pixel. With the app running next to it
//! the whole live path can be exercised without the game: desktop capture,
//! labeling on the graphics card or the processor, reading, and the panel's
//! footer. The window takes no focus and closes when the recording ends.
//!
//! It is not the game: nothing else loads the machine, so times measured this
//! way say what the live path costs on its own, not next to a fight.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;
    use std::time::{Duration, Instant};
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        GetDC, ReleaseDC, SetDIBitsToDevice, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW,
        SetProcessDPIAware, ShowWindow, TranslateMessage, MSG, PM_REMOVE, SW_SHOWNOACTIVATE,
        WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_POPUP,
    };

    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        return Err("Usage: stand_in_game <width> <height> <fps> [x y]".into());
    }
    let width: i32 = args[0].parse()?;
    let height: i32 = args[1].parse()?;
    let fps: f64 = args[2].parse()?;
    let x: i32 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(0);
    let y: i32 = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(0);

    let window = unsafe {
        // Without this a scaled display would stretch the picture.
        let _ = SetProcessDPIAware();
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            lpszClassName: w!("UnityWndClass"),
            ..Default::default()
        });
        let window = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            w!("UnityWndClass"),
            w!("Genshin Impact"),
            WS_POPUP,
            x,
            y,
            width,
            height,
            None,
            None,
            None,
            None,
        )?;
        let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
        window
    };

    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negative: the first row is the top one, as in the recording.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut frame = vec![0u8; width as usize * height as usize * 4];
    let mut stdin = std::io::stdin().lock();
    let started = Instant::now();
    let mut shown = 0u64;
    while stdin.read_exact(&mut frame).is_ok() {
        unsafe {
            let dc = GetDC(Some(window));
            SetDIBitsToDevice(
                dc,
                0,
                0,
                width as u32,
                height as u32,
                0,
                0,
                0,
                height as u32,
                frame.as_ptr().cast(),
                &info,
                DIB_RGB_COLORS,
            );
            ReleaseDC(Some(window), dc);
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        shown += 1;
        let due = started + Duration::from_secs_f64(shown as f64 / fps);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
    println!(
        "showed {shown} frames in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("stand_in_game needs Windows.");
}
