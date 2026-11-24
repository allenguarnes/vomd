// x11-rs: Rust bindings for X11 libraries
// The X11 libraries are available under the MIT license.
// These bindings are public domain.

use std::os::raw::{c_char, c_int, c_long, c_uchar, c_uint, c_ulong, c_void};

use crate::xlib::{Cursor, Display};

//
// functions
//

x11_link! { Xcursor, xcursor, ["libXcursor.so.1", "libXcursor.so"], 4,
  pub fn XcursorImageCreate (_2: c_int, _1: c_int) -> *mut XcursorImage,
  pub fn XcursorImageDestroy (_1: *mut XcursorImage) -> (),
  pub fn XcursorImageLoadCursor (_2: *mut Display, _1: *const XcursorImage) -> c_ulong,
  pub fn XcursorLibraryLoadCursor (_2: *mut Display, _1: *const c_char) -> c_ulong,
variadic:
globals:
}

//
// types
//

pub type XcursorBool = c_int;
pub type XcursorDim = XcursorUInt;
pub type XcursorPixel = XcursorUInt;
pub type XcursorUInt = c_uint;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct _XcursorAnimate {
    pub cursors: *mut XcursorCursors,
    pub sequence: c_int,
}
pub type XcursorAnimate = _XcursorAnimate;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct _XcursorChunkHeader {
    pub header: XcursorUInt,
    pub type_: XcursorUInt,
    pub subtype: XcursorUInt,
    pub version: XcursorUInt,
}
pub type XcursorChunkHeader = _XcursorChunkHeader;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct _XcursorComment {
    pub version: XcursorUInt,
    pub comment_type: XcursorUInt,
    pub comment: *mut c_char,
}
pub type XcursorComment = _XcursorComment;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct _XcursorComments {
    pub ncomment: c_int,
    pub comments: *mut *mut XcursorComment,
}
pub type XcursorComments = _XcursorComments;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct _XcursorCursors {
    pub dpy: *mut Display,
    pub ref_: c_int,
    pub ncursor: c_int,
    pub cursors: *mut Cursor,
}
pub type XcursorCursors = _XcursorCursors;

#[derive(Debug, Copy)]
#[repr(C)]
pub struct _XcursorFile {
    pub closure: *mut c_void,
    pub read: Option<unsafe extern "C" fn(*mut XcursorFile, *mut c_uchar, c_int) -> c_int>,
    pub write: Option<unsafe extern "C" fn(*mut XcursorFile, *mut c_uchar, c_int) -> c_int>,
    pub seek: Option<unsafe extern "C" fn(*mut XcursorFile, c_long, c_int) -> c_int>,
}
pub type XcursorFile = _XcursorFile;

impl Clone for _XcursorFile {
    fn clone(&self) -> _XcursorFile {
        _XcursorFile {
            closure: self.closure,
            read: self.read,
            write: self.write,
            seek: self.seek,
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct _XcursorFileHeader {
    pub magic: XcursorUInt,
    pub header: XcursorUInt,
    pub version: XcursorUInt,
    pub ntoc: XcursorUInt,
    pub tocs: *mut XcursorFileToc,
}
pub type XcursorFileHeader = _XcursorFileHeader;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct _XcursorFileToc {
    pub type_: XcursorUInt,
    pub subtype: XcursorUInt,
    pub position: XcursorUInt,
}
pub type XcursorFileToc = _XcursorFileToc;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct _XcursorImage {
    pub version: XcursorUInt,
    pub size: XcursorDim,
    pub width: XcursorDim,
    pub height: XcursorDim,
    pub xhot: XcursorDim,
    pub yhot: XcursorDim,
    pub delay: XcursorUInt,
    pub pixels: *mut XcursorPixel,
}
pub type XcursorImage = _XcursorImage;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct _XcursorImages {
    pub nimage: c_int,
    pub images: *mut *mut XcursorImage,
    pub name: *mut c_char,
}
pub type XcursorImages = _XcursorImages;
