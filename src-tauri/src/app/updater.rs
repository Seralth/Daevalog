//! The update prompt, and downloading and starting the installer.

use std::time::Duration;

use tauri::{Emitter, Manager};

use crate::platform;

use super::AppState;

#[tauri::command]
pub(super) async fn show_update_window(
    app: tauri::AppHandle,
    current: String,
    latest: String,
    msi_url: String,
    arch_url: Option<String>,
    deb_url: Option<String>,
    rpm_url: Option<String>,
) -> Result<bool, String> {
    // The manifest names a package per platform (the MSI; the Arch, Debian
    // and RPM packages). Where this install cannot update itself, or the
    // manifest has nothing for it, there is nothing to offer.
    let packages = platform::UpdatePackages {
        msi: &msi_url,
        arch: arch_url.as_deref().unwrap_or(""),
        deb: deb_url.as_deref().unwrap_or(""),
        rpm: rpm_url.as_deref().unwrap_or(""),
    };
    let package_url = platform::updater::package_url(&packages).to_string();
    if !platform::updater::supported() || package_url.is_empty() {
        tracing::info!("Update {} available (running {}); this install updates through its package manager", latest, current);
        return Ok(false);
    }
    let msg = format!("A new update is available!\n\nCurrent: {}\nLatest: {}\n\nDownload and install now?", current, latest);

    let accepted = tokio::task::spawn_blocking(move || {
        platform::dialog::ask_yes_no("Daevalog - Update Available", &msg)
    }).await.unwrap_or(false);

    if accepted {
        // Download and install in background
        let app2 = app.clone();
        let url = package_url;
        tauri::async_runtime::spawn(async move {
            if let Err(e) = download_and_install_update(&app2, &url).await {
                tracing::error!("Update download failed: {}", e);
                // Show error dialog
                let _ = tokio::task::spawn_blocking(move || {
                    platform::dialog::show_error(
                        "Daevalog - Update Error",
                        &format!("Download failed: {}\n\nPlease download manually.", e),
                    );
                }).await;
            }
        });
    }

    Ok(accepted)
}

async fn download_and_install_update(app: &tauri::AppHandle, url: &str) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    use futures_util::StreamExt;

    // Show progress dialog on a blocking thread
    let app_clone = app.clone();
    let url_owned = url.to_string();

    let response = app
        .state::<AppState>()
        .http
        .get(&url_owned)
        .timeout(Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let total_size = response.content_length().unwrap_or(0);
    let file_name = url_owned.rsplit('/').next().unwrap_or("update");
    let msi_path = std::env::temp_dir().join(file_name);

    let mut file = tokio::fs::File::create(&msi_path).await.map_err(|e| e.to_string())?;
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();
    let mut last_pct: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        if total_size > 0 {
            let pct = (downloaded * 100 / total_size).min(100);
            if pct != last_pct {
                last_pct = pct;
                let _ = app_clone.emit("download-progress", pct);
                tracing::info!("Download: {}%", pct);
            }
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);

    tracing::info!("Download complete, launching installer: {}", msi_path.display());

    // Detect current install directory from the running executable's location
    let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let install_dir = current_exe.parent()
        .ok_or("Could not determine install directory")?
        .to_string_lossy()
        .into_owned();
    // Strip a trailing backslash so msiexec doesn't interpret \" as an escape
    let install_dir = install_dir.trim_end_matches('\\').to_string();

    // Launch the installer (msiexec on Windows; see platform::updater).
    platform::updater::run_installer(&msi_path, &install_dir)?;

    // Give installer time to start, then exit
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    app_clone.exit(0);
    Ok(())
}
