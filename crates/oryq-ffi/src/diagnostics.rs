//! Thread-local last-error diagnostics for `oryq_last_error_message()`.

use std::cell::RefCell;
use std::ffi::{c_char, CString};

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

pub fn clear_last_error() {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

pub fn set_last_error(msg: impl Into<String>) {
    let sanitized = msg.into().replace('\0', "\\0");
    let cstr = CString::new(sanitized).unwrap_or_else(|_| {
        CString::new("error message contained interior NUL").expect("static ascii")
    });
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = Some(cstr);
    });
}

pub fn last_error_ptr() -> *const c_char {
    LAST_ERROR.with(|slot| match slot.borrow().as_ref() {
        Some(cstr) => cstr.as_ptr(),
        None => std::ptr::null(),
    })
}
