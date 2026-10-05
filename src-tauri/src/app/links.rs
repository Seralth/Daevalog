//! Opening links and the meter's own folders in the desktop's apps.

use std::path::{Path, PathBuf};

use tauri_plugin_opener::OpenerExt;

use super::data_folder;

/// What may be handed to the desktop's opener.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    Link(String),
    Folder(PathBuf),
}

/// An https link, or a folder inside the meter's data folder. Anything else
/// (other schemes, files, paths outside the folder) is refused.
fn allowed(target: &str, data_dir: &Path) -> Option<Target> {
    if let Ok(url) = reqwest::Url::parse(target) {
        if url.scheme() == "https" && url.host_str().is_some_and(|h| !h.is_empty()) {
            return Some(Target::Link(url.into()));
        }
    }
    let path = Path::new(target);
    if !path.is_absolute() {
        return None;
    }
    let path = data_folder::inside(path, data_dir)?;
    path.is_dir().then_some(Target::Folder(path))
}

pub(crate) fn open(app: &tauri::AppHandle, data_dir: &Path, target: &str) {
    let opened = match allowed(target, data_dir) {
        Some(Target::Link(url)) => app.opener().open_url(url, None::<&str>),
        Some(Target::Folder(path)) => app.opener().open_path(path.to_string_lossy(), None::<&str>),
        None => {
            let shown: String = target.chars().take(200).collect();
            tracing::warn!("Not opening {shown:?}: only https links and the meter's own folders open");
            return;
        }
    };
    if let Err(e) = opened {
        tracing::warn!("Could not open a link or folder: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_links_and_the_data_folder_open() {
        let base = std::env::temp_dir().join(format!("a2t-links-{}", std::process::id()));
        let data = base.join("data");
        let preview = data.join("share-preview");
        std::fs::create_dir_all(&preview).unwrap();
        std::fs::create_dir_all(base.join("other")).unwrap();
        std::fs::write(data.join("settings.json"), b"{}").unwrap();
        let path = |p: &Path| p.to_string_lossy().into_owned();

        assert_eq!(
            allowed("https://a2tools.app/logs/1", &data),
            Some(Target::Link("https://a2tools.app/logs/1".into()))
        );
        assert!(matches!(allowed("https://ko-fi.com/x", &data), Some(Target::Link(_))));
        for refused in [
            "http://a2tools.app/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "mailto:a@b.c",
            "smb://host/share",
            "https://",
            "share-preview",
            "",
        ] {
            assert_eq!(allowed(refused, &data), None, "{refused}");
        }

        let canonical = preview.canonicalize().unwrap();
        assert_eq!(allowed(&path(&preview), &data), Some(Target::Folder(canonical)));
        assert!(matches!(allowed(&path(&data), &data), Some(Target::Folder(_))));
        // Files, folders outside, a climb out, and paths that do not exist.
        assert_eq!(allowed(&path(&data.join("settings.json")), &data), None);
        assert_eq!(allowed(&path(&base.join("other")), &data), None);
        assert_eq!(allowed(&path(&preview.join("..").join("..").join("other")), &data), None);
        assert_eq!(allowed(&path(&data.join("missing")), &data), None);

        let _ = std::fs::remove_dir_all(&base);
    }
}
