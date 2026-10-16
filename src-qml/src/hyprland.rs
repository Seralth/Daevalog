// SPDX-License-Identifier: AGPL-3.0-only

//! Hyprland's socket, for a layer drag.
//!
//! Hyprland moves a layer surface to new margins with its layer animation
//! (`arrangeLayerArray` sets the surface's animated position), and while a
//! button is held it measures the pointer from where that animation is now,
//! not from where the surface is going (`mouseMoveUnified`, through
//! `getSurfaceBoxGlobal`). So during a drag the window coordinates run ahead
//! of the pointer, and a drag measured from them lands past it. The socket
//! gives the pointer on the screen instead, and where the surface was when
//! the drag started.

#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::time::Duration;

/// The layer-shell namespace of the meter window (LayerMeterWindow.qml).
pub const METER_NAMESPACE: &str = "daevalog-meter";

/// A screen position in Hyprland's layout pixels.
pub type Point = (i32, i32);

#[cfg(unix)]
fn socket() -> Option<PathBuf> {
    let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(PathBuf::from(runtime).join("hypr").join(signature).join(".socket.sock"))
}

/// Hyprland runs on Linux only.
#[cfg(not(unix))]
fn request(_command: &str) -> Option<String> {
    None
}

/// Sends one request and reads the reply, which ends when Hyprland closes
/// the connection.
#[cfg(unix)]
fn request(command: &str) -> Option<String> {
    let mut stream = UnixStream::connect(socket()?).ok()?;
    let limit = Some(Duration::from_millis(100));
    stream.set_read_timeout(limit).ok()?;
    stream.set_write_timeout(limit).ok()?;
    stream.write_all(command.as_bytes()).ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    Some(reply)
}

/// Where the pointer is on the screen.
pub fn cursor() -> Option<Point> {
    parse_cursor(&request("/cursorpos")?)
}

/// The top-left corner of this process's layer surface `namespace` on the
/// screen: where Hyprland places it, the end of any animation.
pub fn layer_origin(namespace: &str) -> Option<Point> {
    find_layer(&request("j/layers")?, namespace, std::process::id())
}

/// "x, y".
fn parse_cursor(reply: &str) -> Option<Point> {
    let (x, y) = reply.trim().split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// From `hyprctl -j layers`: monitors, each with levels of layer surfaces.
fn find_layer(reply: &str, namespace: &str, pid: u32) -> Option<Point> {
    let monitors: serde_json::Value = serde_json::from_str(reply).ok()?;
    let layer = monitors
        .as_object()?
        .values()
        .filter_map(|monitor| monitor.get("levels")?.as_object())
        .flat_map(|levels| levels.values())
        .filter_map(serde_json::Value::as_array)
        .flatten()
        .find(|layer| layer["namespace"] == namespace && layer["pid"] == pid)?;
    let at = |key: &str| layer[key].as_f64().map(|value| value.round() as i32);
    Some((at("x")?, at("y")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_reply() {
        assert_eq!(parse_cursor("640, 400"), Some((640, 400)));
        assert_eq!(parse_cursor("-12, 3\n"), Some((-12, 3)));
        assert_eq!(parse_cursor("error: no socket"), None);
    }

    /// Shaped as Hyprland 0.56 writes it: a waybar on level 2, the meter and
    /// the meter's area probes from two processes.
    #[test]
    fn layers_reply() {
        let reply = r#"{
"Virtual-1": {
    "levels": {
        "0": [
                {
                    "address": "0x55d0c0a1b2c0",
                    "x": 0,
                    "y": 30,
                    "w": 1,
                    "h": 770,
                    "alpha": 1,
                    "namespace": "daevalog-area",
                    "pid": 812
                }
        ],
        "1": [],
        "2": [
                {
                    "address": "0x55d0c0a1c3d0",
                    "x": 0,
                    "y": 0,
                    "w": 1280,
                    "h": 30,
                    "alpha": 1,
                    "namespace": "waybar",
                    "pid": 790
                }
        ],
        "3": [
                {
                    "address": "0x55d0c0a1d4e0",
                    "x": 40,
                    "y": 190,
                    "w": 360,
                    "h": 214,
                    "alpha": 1,
                    "namespace": "daevalog-meter",
                    "pid": 700
                },
                {
                    "address": "0x55d0c0a1e5f0",
                    "x": 339.5,
                    "y": 367,
                    "w": 360,
                    "h": 214,
                    "alpha": 1,
                    "namespace": "daevalog-meter",
                    "pid": 812
                }
        ]
    }
}
}"#;
        assert_eq!(find_layer(reply, METER_NAMESPACE, 812), Some((340, 367)));
        assert_eq!(find_layer(reply, METER_NAMESPACE, 700), Some((40, 190)));
        assert_eq!(find_layer(reply, METER_NAMESPACE, 1), None);
        assert_eq!(find_layer("ok", METER_NAMESPACE, 812), None);
    }
}
