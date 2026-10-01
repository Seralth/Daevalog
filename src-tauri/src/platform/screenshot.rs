//! Screenshots of the meter's own windows: to the clipboard, and optionally to
//! a PNG file.
//!
//! The page measures what to capture with `getBoundingClientRect`, which is in
//! CSS pixels relative to the window's client area. The screen is in physical
//! pixels, so the rect is scaled by the page's `devicePixelRatio` (Windows
//! display scaling, times any page zoom) and offset by where the client area
//! sits on screen. Skipping either crops or shifts the image at 125%/150%
//! scaling, which is what players reported.

/// A rectangle on screen, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl ScreenRect {
    /// The smallest rectangle covering both.
    pub fn union(self, other: ScreenRect) -> ScreenRect {
        let left = self.left.min(other.left);
        let top = self.top.min(other.top);
        let right = (self.left + self.width).max(other.left + other.width);
        let bottom = (self.top + self.height).max(other.top + other.height);
        ScreenRect { left, top, width: right - left, height: bottom - top }
    }
}

/// A CSS-pixel rect inside a window's client area, as screen pixels, given the
/// client area's on-screen origin.
pub fn css_rect_to_screen(origin: (i32, i32), x: f64, y: f64, w: f64, h: f64, scale: f64) -> ScreenRect {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let left = origin.0 + (x * scale).round() as i32;
    let top = origin.1 + (y * scale).round() as i32;
    let right = origin.0 + ((x + w) * scale).round() as i32;
    let bottom = origin.1 + ((y + h) * scale).round() as i32;
    ScreenRect { left, top, width: (right - left).max(1), height: (bottom - top).max(1) }
}

/// Encode top-down RGBA rows as a PNG. Hand-rolled because the only thing
/// needed is "write these pixels", and flate2 is already a dependency.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    use std::io::Write;

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }

    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in rgba.chunks_exact(stride).take(height as usize) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = z.write_all(&raw);
    let idat = z.finish().unwrap_or_default();

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, deflate, no filter set, no interlace

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(windows)]
pub use win::*;

#[cfg(windows)]
mod win {
    use super::{encode_png, ScreenRect};
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::DataExchange::*;

    /// Where a window's client area (what the page draws into) sits on screen.
    pub fn client_origin(hwnd_raw: isize) -> (i32, i32) {
        let mut point = POINT { x: 0, y: 0 };
        unsafe {
            let _ = ClientToScreen(HWND(hwnd_raw as *mut _), &mut point);
        }
        (point.x, point.y)
    }

    /// A window's whole client area on screen.
    pub fn client_rect(hwnd_raw: isize) -> ScreenRect {
        let hwnd = HWND(hwnd_raw as *mut _);
        let mut rect = RECT::default();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect);
        }
        let (left, top) = client_origin(hwnd_raw);
        ScreenRect { left, top, width: rect.right - rect.left, height: rect.bottom - rect.top }
    }

    /// Copy `rect` of the screen, put it on the clipboard (owned by `owner`),
    /// and optionally write it to `png_path`. Returns (clipboard ok, file ok).
    pub fn capture(owner_raw: isize, rect: ScreenRect, png_path: Option<&std::path::Path>) -> (bool, bool) {
        if rect.width <= 0 || rect.height <= 0 {
            return (false, false);
        }
        unsafe {
            let hdc_screen = GetDC(None);
            let hdc_mem = CreateCompatibleDC(Some(hdc_screen));
            let hbm = CreateCompatibleBitmap(hdc_screen, rect.width, rect.height);
            let old = SelectObject(hdc_mem, hbm.into());
            let copied = BitBlt(
                hdc_mem, 0, 0, rect.width, rect.height,
                Some(hdc_screen), rect.left, rect.top, SRCCOPY,
            )
            .is_ok();
            SelectObject(hdc_mem, old);

            let mut file_ok = false;
            if copied {
                if let Some(path) = png_path {
                    file_ok = write_png(hdc_mem, hbm, rect, path);
                }
            }
            let _ = DeleteDC(hdc_mem);
            ReleaseDC(None, hdc_screen);

            let mut clipboard_ok = false;
            if copied && OpenClipboard(Some(HWND(owner_raw as *mut _))).is_ok() {
                let _ = EmptyClipboard();
                // CF_BITMAP = 2. On success the clipboard owns the bitmap.
                clipboard_ok = SetClipboardData(2, Some(HANDLE(hbm.0 as *mut _))).is_ok();
                let _ = CloseClipboard();
            }
            if !clipboard_ok {
                let _ = DeleteObject(hbm.into());
            }
            (clipboard_ok, file_ok)
        }
    }

    unsafe fn write_png(hdc: HDC, hbm: HBITMAP, rect: ScreenRect, path: &std::path::Path) -> bool {
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = rect.width;
        info.bmiHeader.biHeight = -rect.height; // negative: top-down rows
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB.0;
        let mut pixels = vec![0u8; rect.width as usize * rect.height as usize * 4];
        let rows = unsafe {
            GetDIBits(
                hdc, hbm, 0, rect.height as u32,
                Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS,
            )
        };
        if rows != rect.height {
            return false;
        }
        // BGRx -> RGBA, opaque: screen captures carry no meaningful alpha.
        for px in pixels.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        let png = encode_png(rect.width as u32, rect.height as u32, &pixels);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, png).is_ok()
    }

    /// `Pictures\A2Tools DPS Meter`, the default place screenshots are saved.
    pub fn default_folder() -> Option<std::path::PathBuf> {
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{FOLDERID_Pictures, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
        unsafe {
            let raw = SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None).ok()?;
            let path = raw.to_string().ok();
            CoTaskMemFree(Some(raw.0 as *const _));
            Some(std::path::PathBuf::from(path?).join("A2Tools DPS Meter"))
        }
    }

    /// The standard Windows folder picker, owned by `owner`. Blocks until the
    /// player chooses or cancels; run it off the UI thread.
    pub fn pick_folder(owner_raw: isize, start_in: Option<&str>) -> Option<String> {
        use windows::core::HSTRING;
        use windows::Win32::System::Com::*;
        use windows::Win32::UI::Shell::*;
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let result = (|| -> Option<String> {
                let dialog: IFileOpenDialog =
                    CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
                let options = dialog.GetOptions().ok()?;
                dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
                if let Some(start) = start_in.filter(|s| !s.is_empty()) {
                    if let Ok(item) =
                        SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(start), None)
                    {
                        let _ = dialog.SetFolder(&item);
                    }
                }
                dialog.Show(Some(HWND(owner_raw as *mut _))).ok()?; // Err when cancelled
                let item = dialog.GetResult().ok()?;
                let raw = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
                let path = raw.to_string().ok();
                CoTaskMemFree(Some(raw.0 as *const _));
                path
            })();
            CoUninitialize();
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_css_pixels_to_the_screen() {
        // A 300x200 CSS-pixel panel at (10, 20) in a window whose client area
        // starts at (1000, 500), on a 150% display.
        let r = css_rect_to_screen((1000, 500), 10.0, 20.0, 300.0, 200.0, 1.5);
        assert_eq!(r, ScreenRect { left: 1015, top: 530, width: 450, height: 300 });
        // At 100% it is a plain offset.
        let r = css_rect_to_screen((1000, 500), 10.0, 20.0, 300.0, 200.0, 1.0);
        assert_eq!(r, ScreenRect { left: 1010, top: 520, width: 300, height: 200 });
    }

    #[test]
    fn union_covers_both_windows() {
        let meter = ScreenRect { left: 100, top: 100, width: 400, height: 300 };
        let details = ScreenRect { left: 520, top: 80, width: 800, height: 600 };
        assert_eq!(meter.union(details), ScreenRect { left: 100, top: 80, width: 1220, height: 600 });
    }

    #[test]
    fn png_is_well_formed() {
        let png = encode_png(2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        // CRC of an empty IEND chunk is a fixed, well-known value.
        assert_eq!(&png[png.len() - 4..], &[0xAE, 0x42, 0x60, 0x82]);
    }
}
