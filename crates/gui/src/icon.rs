// SPDX-License-Identifier: MIT OR Apache-2.0
//! Apply the same rendered kea to the native window and its Windows taskbar button.
use tauri::ipc::{InvokeBody, Request};

const SIZE: u32 = 64;

/// Synchronous Tauri commands run on the UI thread, where the window and Shell COM live.
#[tauri::command]
pub(crate) fn set_accent_icon(
    window: tauri::WebviewWindow,
    request: Request<'_>,
) -> Result<(), String> {
    let InvokeBody::Raw(rgba) = request.body() else {
        return Err("Expected RGBA icon pixels".into());
    };
    if rgba.len() != (SIZE * SIZE * 4) as usize {
        return Err("Expected a 64 by 64 icon".into());
    }
    window
        .set_icon(tauri::image::Image::new(rgba, SIZE, SIZE))
        .map_err(|e| e.to_string())?;
    #[cfg(windows)]
    taskbar::update(&window, rgba).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(windows)]
mod taskbar {
    use super::SIZE;
    use std::{
        error::Error,
        ffi::OsString,
        fs,
        os::windows::ffi::OsStringExt,
        path::{Path, PathBuf},
    };
    use tauri::Manager;
    use windows::{
        Win32::{
            Foundation::{LPARAM, WPARAM},
            System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READWRITE},
            UI::{
                Shell::{IShellLinkW, SHCNE_UPDATEITEM, SHCNF_PATHW, SHChangeNotify, ShellLink},
                WindowsAndMessaging::{ICON_BIG, ICON_SMALL, SendMessageW, WM_GETICON, WM_SETICON},
            },
        },
        core::{HSTRING, Interface},
    };

    fn path_from_buffer(buf: &[u16]) -> PathBuf {
        OsString::from_wide(&buf[..buf.iter().position(|&n| n == 0).unwrap_or(buf.len())]).into()
    }

    #[allow(unsafe_code)]
    pub(super) fn update(window: &tauri::WebviewWindow, rgba: &[u8]) -> Result<(), Box<dyn Error>> {
        let hwnd = window.hwnd()?;
        // Tauri sets and owns ICON_SMALL. Windows also asks for ICON_BIG for
        // the running taskbar button. Both slots borrow the same window-owned
        // HICON, updated together on the UI thread. Neither call takes ownership.
        unsafe {
            let icon = SendMessageW(hwnd, WM_GETICON, Some(WPARAM(ICON_SMALL as usize)), None);
            if icon.0 != 0 {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(ICON_BIG as usize)),
                    Some(LPARAM(icon.0)),
                );
            }
        }
        if window.label() == "main" {
            let link = window
                .path()
                .data_dir()?
                .join("Microsoft/Internet Explorer/Quick Launch/User Pinned/TaskBar/Familiar.lnk");
            if link.is_file() {
                update_pin(&link, &window.path().app_local_data_dir()?, rgba)?;
            }
        }
        Ok(())
    }

    #[allow(unsafe_code)]
    fn update_pin(shortcut: &Path, dir: &Path, rgba: &[u8]) -> Result<(), Box<dyn Error>> {
        let name = HSTRING::from(shortcut.as_os_str());
        // All COM calls are on the apartment used by the WebView. The string
        // and writable buffers live through each synchronous Shell call.
        let (link, file, current) = unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let file: IPersistFile = link.cast()?;
            file.Load(&name, STGM_READWRITE)?;
            let mut buf = vec![0; 32768];
            link.GetPath(&mut buf, std::ptr::null_mut(), 0)?;
            if fs::canonicalize(path_from_buffer(&buf))?
                != fs::canonicalize(std::env::current_exe()?)?
            {
                return Ok(());
            }
            let mut index = 0;
            link.GetIconLocation(&mut buf, &mut index)?;
            (link, file, path_from_buffer(&buf))
        };

        let image = ico::IconImage::from_rgba_data(SIZE, SIZE, rgba.to_vec());
        let mut icons = ico::IconDir::new(ico::ResourceType::Icon);
        icons.add_entry(ico::IconDirEntry::encode_as_png(&image)?);
        let mut encoded = Vec::new();
        icons.write(&mut encoded)?;
        // Explorer caches by path. Alternate two owned files so it sees a
        // new icon and the shortcut always points at a complete file.
        let first = dir.join("accent-a.ico");
        let second = dir.join("accent-b.ico");
        if (current == first || current == second) && fs::read(&current).is_ok_and(|b| b == encoded)
        {
            return Ok(());
        }
        let next = if current == first { second } else { first };
        fs::create_dir_all(dir)?;
        fs::write(&next, encoded)?;
        // Change just the icon location of this executable's existing pin.
        unsafe {
            link.SetIconLocation(&HSTRING::from(next.as_os_str()), 0)?;
            file.Save(&name, true)?;
            SHChangeNotify(
                SHCNE_UPDATEITEM,
                SHCNF_PATHW,
                Some(name.as_ptr().cast()),
                None,
            );
        }
        Ok(())
    }
}
