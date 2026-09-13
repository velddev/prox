use super::{Result, absolute_env};
use std::{
    io,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
    System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ,
        REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegQueryValueExW,
        RegSetValueExW,
    },
    UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    },
};

pub struct Plan {
    directory: PathBuf,
}

impl Plan {
    pub fn new() -> Result<Self> {
        let directory = absolute_env("LOCALAPPDATA")?.join("Programs").join("prox");
        if directory
            .as_os_str()
            .encode_wide()
            .any(|c| c == b';' as u16 || c == 0)
        {
            return Err("install directory cannot contain a semicolon or NUL".into());
        }
        Ok(Self { directory })
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn configure_path(&self) -> Result<()> {
        let environment = wide("Environment");
        let name = wide("Path");
        let mut handle = ptr::null_mut();
        // SAFETY: all strings are NUL-terminated and output pointers reference live storage.
        check(unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                environment.as_ptr(),
                0,
                ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                ptr::null(),
                &mut handle,
                ptr::null_mut(),
            )
        })?;
        let key = Key(handle);
        let (mut value, kind) = read_path(key.0, &name)?;
        let directory = self
            .directory
            .to_str()
            .ok_or("install path must be valid UTF-8")?;
        let present = value.split(';').any(|entry| {
            entry
                .trim()
                .trim_matches('"')
                .trim_end_matches(['\\', '/'])
                .eq_ignore_ascii_case(directory)
        });
        if !present {
            if !value.is_empty() && !value.ends_with(';') {
                value.push(';');
            }
            value.push_str(directory);
            let value = wide(&value);
            let bytes = u32::try_from(value.len() * 2)?;
            // Preserve the original registry value type and unexpanded %VARIABLES%.
            check(unsafe {
                RegSetValueExW(key.0, name.as_ptr(), 0, kind, value.as_ptr().cast(), bytes)
            })?;
        }
        // Notify Explorer so newly launched applications inherit the updated user PATH.
        // Existing shells retain their current environment until restarted.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                environment.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                2000,
                ptr::null_mut(),
            );
        }
        Ok(())
    }
}

fn read_path(key: HKEY, name: &[u16]) -> Result<(String, u32)> {
    let (mut size, mut kind) = (0, 0);
    let result = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null(),
            &mut kind,
            ptr::null_mut(),
            &mut size,
        )
    };
    if result == ERROR_FILE_NOT_FOUND {
        return Ok((String::new(), REG_EXPAND_SZ));
    }
    check(result)?;
    if kind != REG_SZ && kind != REG_EXPAND_SZ {
        return Err("the user PATH registry value is not a string".into());
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
    check(unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null(),
            &mut kind,
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    })?;
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    Ok((String::from_utf16(&buffer[..end])?, kind))
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn check(code: u32) -> io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
