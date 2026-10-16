//! Screenshots of the meter: PNG encoding, and saving without overwriting.

/// Write `png` to `wanted`, or beside it as `name (2).png` and so on when
/// that file exists: never over a file already there. The path written.
pub fn write_new_png(wanted: &std::path::Path, png: &[u8]) -> Option<std::path::PathBuf> {
    use std::io::Write;

    let dir = wanted.parent()?;
    let _ = std::fs::create_dir_all(dir);
    let stem = wanted.file_stem()?.to_string_lossy().into_owned();
    for n in 1..=999 {
        let path = if n == 1 { wanted.to_path_buf() } else { dir.join(format!("{stem} ({n}).png")) };
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if file.write_all(png).is_ok() {
                    return Some(path);
                }
                let _ = std::fs::remove_file(&path);
                return None;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// Length, type, data, then the CRC-32 of type and data.
fn put_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    png.extend((data.len() as u32).to_be_bytes().into_iter().chain(*kind));
    png.extend(data);
    png.extend(crc.sum().to_be_bytes());
}

fn zlib_default(raw: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(raw).and_then(|_| encoder.finish()).unwrap_or_default()
}

/// A PNG of `height` rows of `width` RGBA pixels, unfiltered, compressed at
/// zlib's default level. Bytes past the last whole row are left out.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let stride = 4 * width as usize;
    let rows_given = if stride == 0 { height as usize } else { rgba.len() / stride };
    let mut scanlines = Vec::new();
    for y in 0..rows_given.min(height as usize) {
        scanlines.push(0); // no filter
        scanlines.extend_from_slice(&rgba[y * stride..(y + 1) * stride]);
    }

    // Bit depth 8, colour type 6 (RGBA), then compression, filter and
    // interlace methods, all 0.
    let mut ihdr = [0u8; 13];
    ihdr[..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8..].copy_from_slice(&[8, 6, 0, 0, 0]);

    let mut png = SIGNATURE.to_vec();
    put_chunk(&mut png, b"IHDR", &ihdr);
    put_chunk(&mut png, b"IDAT", &zlib_default(&scanlines));
    put_chunk(&mut png, b"IEND", &[]);
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_has_signature_header_and_end() {
        let png = encode_png(2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]);
        assert!(png.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]));
        assert_eq!(png.get(12..16), Some(&b"IHDR"[..]));
        assert!(png.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
    }

    #[test]
    fn one_red_pixel_matches_the_baseline() {
        let want = "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c63f8cfc0f01f\
                    00050001ff89993d1d0000000049454e44ae426082";
        assert_eq!(crate::built::to_hex(&encode_png(1, 1, &[255, 0, 0, 255])), want);
    }

    #[test]
    fn rows_past_the_height_are_ignored() {
        let pixel = [1, 2, 3, 4];
        assert_eq!(encode_png(1, 1, &pixel.repeat(2)), encode_png(1, 1, &pixel));
    }

    #[test]
    fn screenshots_never_replace_a_file() {
        let dir = std::env::temp_dir().join(format!("daevalog-png-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("old scratch folder");
        }
        let wanted = dir.join("shot.png");
        let written = [write_new_png(&wanted, b"one"), write_new_png(&wanted, b"two")];
        assert_eq!(written, [Some(wanted.clone()), Some(dir.join("shot (2).png"))]);
        let contents: Vec<Vec<u8>> = written.iter().flatten().map(|p| std::fs::read(p).unwrap()).collect();
        assert_eq!(contents, [b"one".to_vec(), b"two".to_vec()]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
