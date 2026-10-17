//! Pasting an image into an agent. Claude and Codex read the clipboard themselves
//! on their image key, but from inside the daemon they may not reach it: on Windows
//! the daemon is a detached process, and in WSL Windows' clipboard is behind
//! interop that ends with the terminal that started the daemon. The TUI runs where
//! you are, so it reads the image, saves it, and pastes the file's path, which both
//! agents attach as an image.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::Engine;

/// Whether `keys` is an agent's image-paste key: Ctrl-V (Claude on macOS and Linux,
/// Codex), Alt-V (Claude on Windows and WSL), or an empty paste, which is what a
/// terminal pastes from a clipboard holding only an image.
pub fn is_image_key(keys: &[u8]) -> bool {
    matches!(keys, b"\x16" | b"\x1bv" | b"\x1b[200~\x1b[201~")
}

/// Saves the clipboard's image as a PNG in `dir`; `None` when it holds none.
pub fn save_image(dir: &Path) -> Option<PathBuf> {
    valkyrie_proto::pastes::save(dir, ".png", &read_png()?).ok()
}

/// What to type for `path`: quoted if it has a space, since Codex reads an unquoted
/// one as text (Windows home folders can have spaces), and as a paste when the
/// program takes bracketed paste, so it's read as one.
pub fn paste_text(path: &Path, bracketed: bool) -> Vec<u8> {
    let path = path.display().to_string();
    let path = if path.contains(char::is_whitespace) {
        format!("\"{path}\"")
    } else {
        path
    };
    match bracketed {
        true => format!("\x1b[200~{path}\x1b[201~").into_bytes(),
        false => path.into_bytes(),
    }
}

fn read_png() -> Option<Vec<u8>> {
    // WSL's own clipboard is WSLg's, which doesn't carry Windows' images.
    if crate::links::is_wsl()
        && let Some(png) = windows_png()
    {
        return Some(png);
    }
    let image = arboard::Clipboard::new().ok()?.get_image().ok()?;
    let rgba = image::RgbaImage::from_raw(
        image.width as u32,
        image.height as u32,
        image.bytes.into_owned(),
    )?;
    let mut png = std::io::Cursor::new(Vec::new());
    rgba.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(png.into_inner())
}

/// Windows' clipboard image from WSL, through PowerShell, as Claude reads it there.
fn windows_png() -> Option<Vec<u8>> {
    const SCRIPT: &str = "Add-Type -AssemblyName System.Windows.Forms; \
        $i = [System.Windows.Forms.Clipboard]::GetImage(); if ($null -eq $i) { exit 1 }; \
        $ms = New-Object System.IO.MemoryStream; \
        $i.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png); \
        [Convert]::ToBase64String($ms.ToArray())";
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Sta", "-Command", SCRIPT])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    base64::engine::general_purpose::STANDARD
        .decode(out.stdout.trim_ascii())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_keys() {
        assert!(is_image_key(b"\x16"));
        assert!(is_image_key(b"\x1bv"));
        assert!(is_image_key(b"\x1b[200~\x1b[201~"));
        assert!(!is_image_key(b"v"));
        assert!(!is_image_key(b"\x1b[200~x\x1b[201~"));
    }

    #[test]
    fn paste_quotes_spaces() {
        let plain = Path::new("/s/pastes/1.png");
        assert_eq!(paste_text(plain, false), b"/s/pastes/1.png");
        assert_eq!(
            paste_text(plain, true),
            b"\x1b[200~/s/pastes/1.png\x1b[201~"
        );
        let spaced = Path::new("/home/A B/1.png");
        assert_eq!(paste_text(spaced, false), b"\"/home/A B/1.png\"");
    }
}
