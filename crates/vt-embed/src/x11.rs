use crate::{EmbedError, EmbedRect};
use std::ffi::CString;
use std::os::raw::{c_int, c_long, c_ulong};
use x11_dl::xlib;

pub struct X11Backend {
    xlib: xlib::Xlib,
    display: *mut xlib::Display,
    root: c_ulong,
}

unsafe impl Send for X11Backend {}

impl X11Backend {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let xlib = xlib::Xlib::open().map_err(|e| format!("Failed to load Xlib: {}", e))?;
            let display = (xlib.XOpenDisplay)(std::ptr::null());
            if display.is_null() {
                return Err("Failed to open X display".into());
            }
            let screen = (xlib.XDefaultScreen)(display);
            let root = (xlib.XRootWindow)(display, screen);
            Ok(Self { xlib, display, root })
        }
    }

    pub fn root_window(&self) -> u64 {
        self.root as u64
    }

    pub fn find_window_by_pid(&self, target_pid: u32) -> Result<u64, EmbedError> {
        let windows = self.get_all_windows(self.root);
        for win in windows {
            if let Some(pid) = self.get_window_pid(win) {
                if pid == target_pid {
                    return Ok(win as u64);
                }
            }
        }
        Err(EmbedError::WindowNotFound(target_pid))
    }

    pub fn find_window_by_name(&self, name: &str) -> Result<u64, EmbedError> {
        let windows = self.get_all_windows(self.root);
        for win in windows {
            if let Some(title) = self.get_window_name(win) {
                let title_lower = title.to_lowercase();
                let name_lower = name.to_lowercase();
                if title_lower.contains(&name_lower) {
                    return Ok(win as u64);
                }
            }
        }
        Err(EmbedError::Failed(format!("No window with name '{}'", name)))
    }

    pub fn reparent(
        &self,
        child: u64,
        parent: u64,
        x: i32,
        y: i32,
    ) -> Result<(), EmbedError> {
        unsafe {
            (self.xlib.XReparentWindow)(
                self.display,
                child as c_ulong,
                parent as c_ulong,
                x as c_int,
                y as c_int,
            );
            (self.xlib.XFlush)(self.display);
        }
        Ok(())
    }

    pub fn set_bounds(&self, window: u64, rect: EmbedRect) -> Result<(), EmbedError> {
        unsafe {
            (self.xlib.XMoveResizeWindow)(
                self.display,
                window as c_ulong,
                rect.x as c_int,
                rect.y as c_int,
                rect.width,
                rect.height,
            );
            (self.xlib.XFlush)(self.display);
        }
        Ok(())
    }

    pub fn raise_window(&self, window: u64) -> Result<(), EmbedError> {
        unsafe {
            (self.xlib.XRaiseWindow)(self.display, window as c_ulong);
            (self.xlib.XFlush)(self.display);
        }
        Ok(())
    }

    pub fn get_window_position(&self, window: u64) -> (i32, i32) {
        unsafe {
            let mut x: c_int = 0;
            let mut y: c_int = 0;
            let mut child: c_ulong = 0;
            (self.xlib.XTranslateCoordinates)(
                self.display,
                window as c_ulong,
                self.root,
                0, 0,
                &mut x, &mut y,
                &mut child,
            );
            (x as i32, y as i32)
        }
    }

    pub fn map_window(&self, window: u64) -> Result<(), EmbedError> {
        unsafe {
            (self.xlib.XMapWindow)(self.display, window as c_ulong);
            (self.xlib.XFlush)(self.display);
        }
        Ok(())
    }

    fn get_all_windows(&self, root: c_ulong) -> Vec<c_ulong> {
        let mut result = Vec::new();
        self.collect_windows(root, &mut result);
        result
    }

    fn collect_windows(&self, window: c_ulong, result: &mut Vec<c_ulong>) {
        unsafe {
            let mut root_return: c_ulong = 0;
            let mut parent_return: c_ulong = 0;
            let mut children: *mut c_ulong = std::ptr::null_mut();
            let mut nchildren: u32 = 0;

            let status = (self.xlib.XQueryTree)(
                self.display,
                window,
                &mut root_return,
                &mut parent_return,
                &mut children,
                &mut nchildren,
            );

            if status == 0 || children.is_null() {
                return;
            }

            for i in 0..nchildren as usize {
                let child = *children.add(i);
                result.push(child);
                self.collect_windows(child, result);
            }

            (self.xlib.XFree)(children as *mut _);
        }
    }

    fn get_window_pid(&self, window: c_ulong) -> Option<u32> {
        unsafe {
            let atom_name = CString::new("_NET_WM_PID").unwrap();
            let atom = (self.xlib.XInternAtom)(self.display, atom_name.as_ptr(), 0);
            if atom == 0 {
                return None;
            }

            let mut actual_type: c_ulong = 0;
            let mut actual_format: c_int = 0;
            let mut nitems: c_ulong = 0;
            let mut bytes_after: c_ulong = 0;
            let mut prop: *mut u8 = std::ptr::null_mut();

            let status = (self.xlib.XGetWindowProperty)(
                self.display,
                window,
                atom,
                0,
                1,
                0, // False
                0, // AnyPropertyType
                &mut actual_type,
                &mut actual_format,
                &mut nitems,
                &mut bytes_after,
                &mut prop,
            );

            if status != 0 || prop.is_null() || nitems == 0 {
                if !prop.is_null() {
                    (self.xlib.XFree)(prop as *mut _);
                }
                return None;
            }

            let pid = *(prop as *const u32);
            (self.xlib.XFree)(prop as *mut _);
            Some(pid)
        }
    }

    fn get_window_name(&self, window: c_ulong) -> Option<String> {
        unsafe {
            // Try _NET_WM_NAME first (UTF-8, used by modern apps)
            let atom_name = CString::new("_NET_WM_NAME").unwrap();
            let atom = (self.xlib.XInternAtom)(self.display, atom_name.as_ptr(), 0);
            let utf8_atom_name = CString::new("UTF8_STRING").unwrap();
            let utf8_atom = (self.xlib.XInternAtom)(self.display, utf8_atom_name.as_ptr(), 0);

            let mut actual_type: c_ulong = 0;
            let mut actual_format: c_int = 0;
            let mut nitems: c_ulong = 0;
            let mut bytes_after: c_ulong = 0;
            let mut prop: *mut u8 = std::ptr::null_mut();

            let status = (self.xlib.XGetWindowProperty)(
                self.display, window, atom,
                0, 1024, 0, utf8_atom,
                &mut actual_type, &mut actual_format, &mut nitems,
                &mut bytes_after, &mut prop,
            );

            if status == 0 && !prop.is_null() && nitems > 0 {
                let slice = std::slice::from_raw_parts(prop, nitems as usize);
                let result = String::from_utf8_lossy(slice).into_owned();
                (self.xlib.XFree)(prop as *mut _);
                if !result.is_empty() {
                    return Some(result);
                }
            }
            if !prop.is_null() {
                (self.xlib.XFree)(prop as *mut _);
            }

            // Fallback to WM_NAME
            let mut name: *mut i8 = std::ptr::null_mut();
            let status = (self.xlib.XFetchName)(self.display, window, &mut name);
            if status == 0 || name.is_null() {
                return None;
            }
            let result = std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned();
            (self.xlib.XFree)(name as *mut _);
            Some(result)
        }
    }
}

impl Drop for X11Backend {
    fn drop(&mut self) {
        unsafe {
            (self.xlib.XCloseDisplay)(self.display);
        }
    }
}
