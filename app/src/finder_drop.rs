//! Folders dropped from Finder onto the window. The page has drag and drop of its own (pads,
//! the Arranger), so the window's own drop handling stays off; here only drags that carry files
//! from outside are taken, at the web view's drag methods, and every other drag goes on to the
//! page as before.

use objc2::ffi::NSUInteger;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{class, msg_send, sel};
use std::ffi::{c_char, c_void, CStr};
use std::sync::OnceLock;

const COPY: NSUInteger = 1;

static ON_DROP: OnceLock<Box<dyn Fn(Vec<String>) + Send + Sync>> = OnceLock::new();
static ENTERED: OnceLock<Imp> = OnceLock::new();
static UPDATED: OnceLock<Imp> = OnceLock::new();
static PERFORM: OnceLock<Imp> = OnceLock::new();

/// The paths of the files a drag carries; empty for a drag made inside the page.
unsafe fn file_paths(info: *mut AnyObject) -> Vec<String> {
    let mut out = vec![];
    unsafe {
        let pb: *mut AnyObject = msg_send![info, draggingPasteboard];
        if pb.is_null() {
            return out;
        }
        let classes: *mut AnyObject = msg_send![class!(NSArray), arrayWithObject: class!(NSURL)];
        let items: *mut AnyObject = msg_send![pb, readObjectsForClasses: classes, options: std::ptr::null_mut::<AnyObject>()];
        if items.is_null() {
            return out;
        }
        let n: NSUInteger = msg_send![items, count];
        for i in 0..n {
            let url: *mut AnyObject = msg_send![items, objectAtIndex: i];
            let is_file: Bool = msg_send![url, isFileURL];
            if !is_file.as_bool() {
                continue;
            }
            let path: *mut AnyObject = msg_send![url, path];
            if path.is_null() {
                continue;
            }
            let s: *const c_char = msg_send![path, UTF8String];
            if !s.is_null() {
                out.push(CStr::from_ptr(s).to_string_lossy().into_owned());
            }
        }
    }
    out
}

type OpFn = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> NSUInteger;
type BoolFn = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> Bool;

unsafe extern "C-unwind" fn entered(this: *mut AnyObject, sel: Sel, info: *mut AnyObject) -> NSUInteger {
    unsafe {
        if !file_paths(info).is_empty() {
            return COPY;
        }
        let f: OpFn = std::mem::transmute(*ENTERED.get().unwrap());
        f(this, sel, info)
    }
}

unsafe extern "C-unwind" fn updated(this: *mut AnyObject, sel: Sel, info: *mut AnyObject) -> NSUInteger {
    unsafe {
        if !file_paths(info).is_empty() {
            return COPY;
        }
        let f: OpFn = std::mem::transmute(*UPDATED.get().unwrap());
        f(this, sel, info)
    }
}

unsafe extern "C-unwind" fn perform(this: *mut AnyObject, sel: Sel, info: *mut AnyObject) -> Bool {
    unsafe {
        let paths = file_paths(info);
        if !paths.is_empty() {
            if let Some(f) = ON_DROP.get() {
                f(paths);
            }
            return Bool::YES;
        }
        let f: BoolFn = std::mem::transmute(*PERFORM.get().unwrap());
        f(this, sel, info)
    }
}

/// Takes file drops on `webview` (a WKWebView) and hands their paths to `on_drop`.
pub fn install(webview: *mut c_void, on_drop: impl Fn(Vec<String>) + Send + Sync + 'static) {
    if ON_DROP.set(Box::new(on_drop)).is_err() {
        return;
    }
    unsafe {
        let obj = &*(webview as *mut AnyObject);
        let cls: &AnyClass = obj.class();
        let swap = |name: Sel, keep: &OnceLock<Imp>, imp: Imp| {
            if let Some(m) = cls.instance_method(name) {
                let _ = keep.set(m.implementation());
                m.set_implementation(imp);
            }
        };
        swap(sel!(draggingEntered:), &ENTERED, std::mem::transmute::<OpFn, Imp>(entered));
        swap(sel!(draggingUpdated:), &UPDATED, std::mem::transmute::<OpFn, Imp>(updated));
        swap(sel!(performDragOperation:), &PERFORM, std::mem::transmute::<BoolFn, Imp>(perform));
    }
}
