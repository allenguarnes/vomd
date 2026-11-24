// x11-rs: Rust bindings for X11 libraries
// The X11 libraries are available under the MIT license.
// These bindings are public domain.

use std::fmt;
use std::os::raw::{
    c_char, c_int, c_long, c_schar, c_short, c_uchar, c_uint, c_ulong, c_ushort, c_void,
};
use std::slice;

use libc::wchar_t;

use super::internal::{mem_eq, transmute_union};
use super::xf86vmode;
use super::xrandr;
use super::xss;

// deprecated
pub mod xkb {}

//
// functions
//

x11_link! { Xlib, x11, ["libX11.so.6", "libX11.so"], 42,
  pub fn XCheckIfEvent (_4: *mut Display, _3: *mut XEvent, _2: Option<unsafe extern "C" fn (*mut Display, *mut XEvent, *mut c_char) -> c_int>, _1: *mut c_char) -> c_int,
  pub fn XCloseDisplay (_1: *mut Display) -> c_int,
  pub fn XCloseIM (_1: XIM) -> c_int,
  pub fn XCreateBitmapFromData (_5: *mut Display, _4: c_ulong, _3: *const c_char, _2: c_uint, _1: c_uint) -> c_ulong,
  pub fn XCreatePixmapCursor (_7: *mut Display, _6: c_ulong, _5: c_ulong, _4: *mut XColor, _3: *mut XColor, _2: c_uint, _1: c_uint) -> c_ulong,
  pub fn XDefaultRootWindow (_1: *mut Display) -> c_ulong,
  pub fn XDefaultScreen (_1: *mut Display) -> c_int,
  pub fn XDestroyIC (_1: XIC) -> (),
  pub fn XFilterEvent (_2: *mut XEvent, _1: c_ulong) -> c_int,
  pub fn XFlush (_1: *mut Display) -> c_int,
  pub fn XFree (_1: *mut c_void) -> c_int,
  pub fn XFreeCursor (_2: *mut Display, _1: c_ulong) -> c_int,
  pub fn XFreeEventData (_2: *mut Display, _1: *mut XGenericEventCookie) -> (),
  pub fn XFreeModifiermap (_1: *mut XModifierKeymap) -> c_int,
  pub fn XFreePixmap (_2: *mut Display, _1: c_ulong) -> c_int,
  pub fn XGetAtomNames (_4: *mut Display, _3: *mut c_ulong, _2: c_int, _1: *mut *mut c_char) -> c_int,
  pub fn XGetErrorText (_4: *mut Display, _3: c_int, _2: *mut c_char, _1: c_int) -> c_int,
  pub fn XGetEventData (_2: *mut Display, _1: *mut XGenericEventCookie) -> c_int,
  pub fn XGetModifierMapping (_1: *mut Display) -> *mut XModifierKeymap,
  pub fn XInitThreads () -> c_int,
  pub fn XkbGetState (_3: *mut Display, _2: c_uint, _1: XkbStatePtr) -> c_int,
  pub fn XkbSelectEvents (_4: *mut Display, _3: c_uint, _2: c_ulong, _1: c_ulong) -> c_int,
  pub fn XkbSetDetectableAutoRepeat (_3: *mut Display, _2: c_int, _1: *mut c_int) -> c_int,
  pub fn XOpenDisplay (_1: *const c_char) -> *mut Display,
  pub fn XOpenIM (_4: *mut Display, _3: XrmDatabase, _2: *mut c_char, _1: *mut c_char) -> XIM,
  pub fn XPending (_1: *mut Display) -> c_int,
  pub fn XQueryKeymap (_2: *mut Display, _1: *mut c_char) -> c_int,
  pub fn XRegisterIMInstantiateCallback (_6: *mut Display, _5: XrmDatabase, _4: *mut c_char, _3: *mut c_char, _2: Option<unsafe extern "C" fn (*mut Display, *mut c_char, *mut c_char)>, _1: *mut c_char) -> c_int,
  pub fn XRootWindow (_2: *mut Display, _1: c_int) -> c_ulong,
  pub fn XSetErrorHandler (_1: Option<unsafe extern "C" fn (*mut Display, *mut XErrorEvent) -> c_int>) -> Option<unsafe extern "C" fn (*mut Display, *mut XErrorEvent) -> c_int>,
  pub fn XSetICFocus (_1: XIC) -> (),
  pub fn XSetLocaleModifiers (_1: *const c_char) -> *mut c_char,
  pub fn XSupportsLocale () -> c_int,
  pub fn XSync (_2: *mut Display, _1: c_int) -> c_int,
  pub fn XUnregisterIMInstantiateCallback (_6: *mut Display, _5: XrmDatabase, _4: *mut c_char, _3: *mut c_char, _2: Option<unsafe extern "C" fn (*mut Display, *mut c_char, *mut c_char)>, _1: *mut c_char) -> c_int,
  pub fn XUnsetICFocus (_1: XIC) -> (),
  pub fn Xutf8LookupString (_6: XIC, _5: *mut XKeyEvent, _4: *mut c_char, _3: c_int, _2: *mut c_ulong, _1: *mut c_int) -> c_int,
variadic:
  pub fn XCreateIC (_1: XIM) -> XIC,
  pub fn XGetIMValues (_1: XIM) -> *mut c_char,
  pub fn XSetICValues (_1: XIC) -> *mut c_char,
  pub fn XSetIMValues (_1: XIM) -> *mut c_char,
  pub fn XVaCreateNestedList (_1: c_int) -> *mut c_void,
globals:
}

//
// types
//

// common types
pub type Atom = XID;
pub type Bool = c_int;
pub type Colormap = XID;
pub type Cursor = XID;
pub type Drawable = XID;
pub type Font = XID;
pub type GContext = XID;
pub type KeyCode = c_uchar;
pub type KeySym = XID;
pub type Mask = c_ulong;
pub type Pixmap = XID;
pub type Status = Bool;
pub type Time = c_ulong;
pub type VisualID = XID;
pub type Window = XID;
pub type XID = c_ulong;
pub type XPointer = *mut c_char;

// opaque structures
pub enum _XDisplay {}
pub enum xError {}
pub enum xEvent {}
pub enum _XGC {}
pub enum _XIC {}
pub enum _XIM {}
pub enum _XRegion {}
pub enum _XOC {}
pub enum _XOM {}
pub enum _XrmHashBucketRec {}

// TODO structs
#[repr(C)]
pub struct _XcmsCCC;
#[repr(C)]
pub struct XcmsColor;
#[repr(C)]
pub struct _XcmsColorSpace;
#[repr(C)]
pub struct _XcmsFunctionSet;
#[repr(C)]
pub struct _XkbAction;
#[repr(C)]
pub struct _XkbBounds;
#[repr(C)]
pub struct _XkbChanges;
#[repr(C)]
pub struct _XkbClientMapRec;
#[repr(C)]
pub struct _XkbColor;
#[repr(C)]
pub struct _XkbComponentList;
#[repr(C)]
pub struct _XkbComponentNames;
#[repr(C)]
pub struct _XkbControls {
    pub mk_dflt_btn: c_uchar,
    pub num_groups: c_uchar,
    pub groups_wrap: c_uchar,
    pub internal: XkbModsRec,
    pub ignore_loc: XkbModsRec,
    pub enabled_ctrls: c_uint,
    pub repeat_delay: c_ushort,
    pub repeat_interval: c_ushort,
    pub slow_keys_delay: c_ushort,
    pub debounce_delay: c_ushort,
    pub mk_delay: c_ushort,
    pub mk_interval: c_ushort,
    pub mk_time_to_max: c_ushort,
    pub mk_max_speed: c_ushort,
    pub mk_curve: c_short,
    pub ax_options: c_ushort,
    pub ax_timeout: c_ushort,
    pub axt_opts_mask: c_ushort,
    pub axt_opts_values: c_ushort,
    pub axt_ctrls_mask: c_uint,
    pub axt_ctrls_values: c_uint,
    pub per_key_repeat: [c_uchar; 32],
}
#[repr(C)]
pub struct _XkbControlsChanges;
#[repr(C)]
pub struct _XkbControlsNotify;
#[repr(C)]
pub struct _XkbDeviceChanges;
#[repr(C)]
pub struct _XkbDeviceInfo;
#[repr(C)]
pub struct _XkbDeviceLedInfo;
#[repr(C)]
pub struct _XkbDoodad;
#[repr(C)]
pub struct _XkbExtensionDeviceNotify;
#[repr(C)]
pub struct _XkbGeometry;
#[repr(C)]
pub struct _XkbGeometrySizes;
#[repr(C)]
pub struct _XkbIndicatorMapRec;
#[repr(C)]
pub struct _XkbKey;
#[repr(C)]
pub struct _XkbKeyType;
#[repr(C)]
pub struct _XkbMapChanges;
#[repr(C)]
pub struct _XkbMods {
    pub mask: c_uchar,
    pub real_mods: c_uchar,
    pub vmods: c_ushort,
}
#[repr(C)]
pub struct _XkbNameChanges;
#[repr(C)]
pub struct _XkbNamesNotify;
#[repr(C)]
pub struct _XkbOutline;
#[repr(C)]
pub struct _XkbOverlay;
#[repr(C)]
pub struct _XkbOverlayKey;
#[repr(C)]
pub struct _XkbOverlayRow;
#[repr(C)]
pub struct _XkbProperty;
#[repr(C)]
pub struct _XkbRow;
#[repr(C)]
pub struct _XkbSection;
#[repr(C)]
pub struct _XkbServerMapRec;
#[repr(C)]
pub struct _XkbShape;
#[repr(C)]
pub struct _XkbSymInterpretRec;

// union placeholders
pub type XEDataObject = *mut c_void;

// misc typedefs
pub type Display = _XDisplay;
pub type GC = *mut _XGC;
pub type Region = *mut _XRegion;
pub type XcmsCCC = *mut _XcmsCCC;
pub type XcmsColorSpace = _XcmsColorSpace;
pub type XcmsFunctionSet = _XcmsFunctionSet;
pub type XContext = c_int;
pub type XFontSet = *mut _XOC;
pub type XIC = *mut _XIC;
pub type XIM = *mut _XIM;
pub type XkbAction = _XkbAction;
pub type XkbBoundsPtr = *mut _XkbBounds;
pub type XkbChangesPtr = *mut _XkbChanges;
pub type XkbClientMapPtr = *mut _XkbClientMapRec;
pub type XkbColorPtr = *mut _XkbColor;
pub type XkbCompatMapPtr = *mut _XkbCompatMapRec;
pub type XkbComponentListPtr = *mut _XkbComponentList;
pub type XkbComponentNamesPtr = *mut _XkbComponentNames;
pub type XkbControlsChangesPtr = *mut _XkbControlsChanges;
pub type XkbControlsNotifyEvent = _XkbControlsNotify;
pub type XkbControlsPtr = *mut _XkbControls;
pub type XkbDescPtr = *mut _XkbDesc;
pub type XkbDeviceChangesPtr = *mut _XkbDeviceChanges;
pub type XkbDeviceInfoPtr = *mut _XkbDeviceInfo;
pub type XkbDeviceLedInfoPtr = *mut _XkbDeviceLedInfo;
pub type XkbDoodadPtr = *mut _XkbDoodad;
pub type XkbExtensionDeviceNotifyEvent = _XkbExtensionDeviceNotify;
pub type XkbGeometryPtr = *mut _XkbGeometry;
pub type XkbGeometrySizesPtr = *mut _XkbGeometrySizes;
pub type XkbIndicatorMapPtr = *mut _XkbIndicatorMapRec;
pub type XkbIndicatorMapRec = _XkbIndicatorMapRec;
pub type XkbIndicatorPtr = *mut _XkbIndicatorRec;
pub type XkbKeyTypePtr = *mut _XkbKeyType;
pub type XkbMapChangesPtr = *mut _XkbMapChanges;
pub type XkbMapNotifyEvent = _XkbMapNotifyEvent;
pub type XkbModsPtr = *mut _XkbMods;
pub type XkbModsRec = _XkbMods;
pub type XkbNameChangesPtr = *mut _XkbNameChanges;
pub type XkbNamesNotifyEvent = _XkbNamesNotify;
pub type XkbNamesPtr = *mut _XkbNamesRec;
pub type XkbKeyAliasPtr = *mut _XkbKeyAliasRec;
pub type XkbKeyNamePtr = *mut _XkbKeyNameRec;
pub type XkbKeyPtr = *mut _XkbKey;
pub type XkbOutlinePtr = *mut _XkbOutline;
pub type XkbOverlayKeyPtr = *mut _XkbOverlayKey;
pub type XkbOverlayPtr = *mut _XkbOverlay;
pub type XkbOverlayRowPtr = *mut _XkbOverlayRow;
pub type XkbPropertyPtr = *mut _XkbProperty;
pub type XkbRowPtr = *mut _XkbRow;
pub type XkbSectionPtr = *mut _XkbSection;
pub type XkbServerMapPtr = *mut _XkbServerMapRec;
pub type XkbShapePtr = *mut _XkbShape;
pub type XkbStatePtr = *mut _XkbStateRec;
pub type XkbStateRec = _XkbStateRec;
pub type XkbSymInterpretPtr = *mut _XkbSymInterpretRec;
pub type XOM = *mut _XOM;
pub type XrmDatabase = *mut _XrmHashBucketRec;
pub type XrmOptionDescList = *mut XrmOptionDescRec;

// function pointers
pub type XConnectionWatchProc =
    Option<unsafe extern "C" fn(*mut Display, XPointer, c_int, Bool, XPointer)>;
pub type XIMProc = Option<unsafe extern "C" fn(XIM, XPointer, XPointer)>;
pub type XICProc = Option<unsafe extern "C" fn(XIC, XPointer, XPointer) -> Bool>;

// C enums
pub type XICCEncodingStyle = c_int;
pub type XOrientation = c_int;
pub type XrmBinding = c_int;
pub type XrmOptionKind = c_int;

#[allow(dead_code)]
#[cfg(test)]
#[repr(C)]
enum TestEnum {
    Variant1,
    Variant2,
}

#[test]
fn enum_size_test() {
    assert!(::std::mem::size_of::<TestEnum>() == ::std::mem::size_of::<c_int>());
}

//
// event structures
//

#[derive(Clone, Copy)]
#[repr(C)]
pub union XEvent {
    pub type_: c_int,
    pub any: XAnyEvent,
    pub button: XButtonEvent,
    pub circulate: XCirculateEvent,
    pub circulate_request: XCirculateRequestEvent,
    pub client_message: XClientMessageEvent,
    pub colormap: XColormapEvent,
    pub configure: XConfigureEvent,
    pub configure_request: XConfigureRequestEvent,
    pub create_window: XCreateWindowEvent,
    pub crossing: XCrossingEvent,
    pub destroy_window: XDestroyWindowEvent,
    pub error: XErrorEvent,
    pub expose: XExposeEvent,
    pub focus_change: XFocusChangeEvent,
    pub generic_event_cookie: XGenericEventCookie,
    pub graphics_expose: XGraphicsExposeEvent,
    pub gravity: XGravityEvent,
    pub key: XKeyEvent,
    pub keymap: XKeymapEvent,
    pub map: XMapEvent,
    pub mapping: XMappingEvent,
    pub map_request: XMapRequestEvent,
    pub motion: XMotionEvent,
    pub no_expose: XNoExposeEvent,
    pub property: XPropertyEvent,
    pub reparent: XReparentEvent,
    pub resize_request: XResizeRequestEvent,
    pub selection_clear: XSelectionClearEvent,
    pub selection: XSelectionEvent,
    pub selection_request: XSelectionRequestEvent,
    pub unmap: XUnmapEvent,
    pub visibility: XVisibilityEvent,
    pub pad: [c_long; 24],
    // xf86vidmode
    pub xf86vm_notify: xf86vmode::XF86VidModeNotifyEvent,
    // xrandr
    pub xrr_screen_change_notify: xrandr::XRRScreenChangeNotifyEvent,
    pub xrr_notify: xrandr::XRRNotifyEvent,
    pub xrr_output_change_notify: xrandr::XRROutputChangeNotifyEvent,
    pub xrr_crtc_change_notify: xrandr::XRRCrtcChangeNotifyEvent,
    pub xrr_output_property_notify: xrandr::XRROutputPropertyNotifyEvent,
    pub xrr_provider_change_notify: xrandr::XRRProviderChangeNotifyEvent,
    pub xrr_provider_property_notify: xrandr::XRRProviderPropertyNotifyEvent,
    pub xrr_resource_change_notify: xrandr::XRRResourceChangeNotifyEvent,
    // xscreensaver
    pub xss_notify: xss::XScreenSaverNotifyEvent,
}

impl XEvent {
    pub fn get_type(&self) -> c_int {
        unsafe { self.type_ }
    }
}

impl fmt::Debug for XEvent {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut d = f.debug_struct("XEvent");
        unsafe {
            match self.type_ {
                KeyPress => d.field("key", &self.key),
                KeyRelease => d.field("key", &self.key),
                ButtonPress => d.field("button", &self.button),
                ButtonRelease => d.field("button", &self.button),
                MotionNotify => d.field("motion", &self.motion),
                EnterNotify => d.field("crossing", &self.crossing),
                LeaveNotify => d.field("crossing", &self.crossing),
                FocusIn => d.field("focus_change", &self.focus_change),
                FocusOut => d.field("focus_change", &self.focus_change),
                KeymapNotify => d.field("keymap", &self.keymap),
                Expose => d.field("expose", &self.expose),
                GraphicsExpose => d.field("graphics_expose", &self.graphics_expose),
                NoExpose => d.field("no_expose", &self.no_expose),
                VisibilityNotify => d.field("visibility", &self.visibility),
                CreateNotify => d.field("create_window", &self.create_window),
                DestroyNotify => d.field("destroy_window", &self.destroy_window),
                UnmapNotify => d.field("unmap", &self.unmap),
                MapNotify => d.field("map", &self.map),
                MapRequest => d.field("map_request", &self.map_request),
                ReparentNotify => d.field("reparent", &self.reparent),
                ConfigureNotify => d.field("configure", &self.configure),
                ConfigureRequest => d.field("configure_request", &self.configure_request),
                GravityNotify => d.field("gravity", &self.gravity),
                ResizeRequest => d.field("resize_request", &self.resize_request),
                CirculateNotify => d.field("circulate", &self.circulate),
                CirculateRequest => d.field("circulate_request", &self.circulate_request),
                PropertyNotify => d.field("property", &self.property),
                SelectionClear => d.field("selection_clear", &self.selection_clear),
                SelectionRequest => d.field("selection_request", &self.selection_request),
                SelectionNotify => d.field("selection", &self.selection),
                ColormapNotify => d.field("colormap", &self.colormap),
                ClientMessage => d.field("client_message", &self.client_message),
                MappingNotify => d.field("mapping", &self.mapping),
                GenericEvent => d.field("generic_event_cookie", &self.generic_event_cookie),
                _ => d.field("any", &self.any),
            }
        }
        .finish()
    }
}

macro_rules! event_conversions_and_tests {
  { $($field:ident: $ty:ty,)* } => {
    #[test]
    fn xevent_size_test () {
      use std::mem::size_of;
      let xevent_size = size_of::<XEvent>();
      $(assert!(xevent_size >= size_of::<$ty>());)*
    }

    $(
      impl AsMut<$ty> for XEvent {
        fn as_mut (&mut self) -> &mut $ty {
          unsafe { &mut self.$field }
        }
      }

      impl AsRef<$ty> for XEvent {
        fn as_ref (&self) -> &$ty {
          unsafe { &self.$field }
        }
      }

      impl From<$ty> for XEvent {
        fn from (other: $ty) -> XEvent {
          XEvent{ $field: other }
        }
      }

      impl<'a> From<&'a $ty> for XEvent {
        fn from (other: &'a $ty) -> XEvent {
          XEvent{ $field: other.clone() }
        }
      }

      impl From<XEvent> for $ty {
        fn from (xevent: XEvent) -> $ty {
          unsafe { xevent.$field }
        }
      }

      impl<'a> From<&'a XEvent> for $ty {
        fn from (xevent: &'a XEvent) -> $ty {
          unsafe { xevent.$field }
        }
      }
    )*
  };
}

event_conversions_and_tests! {
  any: XAnyEvent,
  button: XButtonEvent,
  circulate: XCirculateEvent,
  circulate_request: XCirculateRequestEvent,
  client_message: XClientMessageEvent,
  colormap: XColormapEvent,
  configure: XConfigureEvent,
  configure_request: XConfigureRequestEvent,
  create_window: XCreateWindowEvent,
  crossing: XCrossingEvent,
  destroy_window: XDestroyWindowEvent,
  error: XErrorEvent,
  expose: XExposeEvent,
  focus_change: XFocusChangeEvent,
  generic_event_cookie: XGenericEventCookie,
  graphics_expose: XGraphicsExposeEvent,
  gravity: XGravityEvent,
  key: XKeyEvent,
  keymap: XKeymapEvent,
  map: XMapEvent,
  mapping: XMappingEvent,
  map_request: XMapRequestEvent,
  motion: XMotionEvent,
  no_expose: XNoExposeEvent,
  property: XPropertyEvent,
  reparent: XReparentEvent,
  resize_request: XResizeRequestEvent,
  selection_clear: XSelectionClearEvent,
  selection: XSelectionEvent,
  selection_request: XSelectionRequestEvent,
  unmap: XUnmapEvent,
  visibility: XVisibilityEvent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XAnyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XButtonEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub button: c_uint,
    pub same_screen: Bool,
}
pub type XButtonPressedEvent = XButtonEvent;
pub type XButtonReleasedEvent = XButtonEvent;

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XCirculateEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub place: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XCirculateRequestEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub parent: Window,
    pub window: Window,
    pub place: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XClientMessageEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub message_type: Atom,
    pub format: c_int,
    pub data: ClientMessageData,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XColormapEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub colormap: Colormap,
    pub new: Bool,
    pub state: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XConfigureEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub above: Window,
    pub override_redirect: Bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XConfigureRequestEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub parent: Window,
    pub window: Window,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub above: Window,
    pub detail: c_int,
    pub value_mask: c_ulong,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XCreateWindowEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub parent: Window,
    pub window: Window,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub override_redirect: Bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XCrossingEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub mode: c_int,
    pub detail: c_int,
    pub same_screen: Bool,
    pub focus: Bool,
    pub state: c_uint,
}
pub type XEnterWindowEvent = XCrossingEvent;
pub type XLeaveWindowEvent = XCrossingEvent;

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XDestroyWindowEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XErrorEvent {
    pub type_: c_int,
    pub display: *mut Display,
    pub resourceid: XID,
    pub serial: c_ulong,
    pub error_code: c_uchar,
    pub request_code: c_uchar,
    pub minor_code: c_uchar,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XExposeEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub count: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XFocusChangeEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub mode: c_int,
    pub detail: c_int,
}
pub type XFocusInEvent = XFocusChangeEvent;
pub type XFocusOutEvent = XFocusChangeEvent;

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XGraphicsExposeEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub drawable: Drawable,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub count: c_int,
    pub major_code: c_int,
    pub minor_code: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XGravityEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub x: c_int,
    pub y: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XKeyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub keycode: c_uint,
    pub same_screen: Bool,
}
pub type XKeyPressedEvent = XKeyEvent;
pub type XKeyReleasedEvent = XKeyEvent;

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XKeymapEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub key_vector: [c_char; 32],
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XMapEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub override_redirect: Bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XMappingEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub event: Window,
    pub request: c_int,
    pub first_keycode: c_int,
    pub count: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XMapRequestEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub parent: Window,
    pub window: Window,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XMotionEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub is_hint: c_char,
    pub same_screen: Bool,
}
pub type XPointerMovedEvent = XMotionEvent;

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XNoExposeEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub drawable: Drawable,
    pub major_code: c_int,
    pub minor_code: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XPropertyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub atom: Atom,
    pub time: Time,
    pub state: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XReparentEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub parent: Window,
    pub x: c_int,
    pub y: c_int,
    pub override_redirect: Bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XResizeRequestEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub width: c_int,
    pub height: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XSelectionClearEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub selection: Atom,
    pub time: Time,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XSelectionEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub requestor: Window,
    pub selection: Atom,
    pub target: Atom,
    pub property: Atom,
    pub time: Time,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XSelectionRequestEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub owner: Window,
    pub requestor: Window,
    pub selection: Atom,
    pub target: Atom,
    pub property: Atom,
    pub time: Time,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XUnmapEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub from_configure: Bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XVisibilityEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub state: c_int,
}

//
// Xkb structs
//

#[repr(C)]
pub struct _XkbCompatMapRec {
    pub sym_interpret: XkbSymInterpretPtr,
    pub groups: [XkbModsRec; XkbNumKbdGroups],
    pub num_si: c_ushort,
    pub size_si: c_ushort,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct _XkbDesc {
    pub dpy: *mut Display,
    pub flags: c_ushort,
    pub device_spec: c_ushort,
    pub min_key_code: KeyCode,
    pub max_key_code: KeyCode,
    pub ctrls: XkbControlsPtr,
    pub server: XkbServerMapPtr,
    pub map: XkbClientMapPtr,
    pub indicators: XkbIndicatorPtr,
    pub names: XkbNamesPtr,
    pub compat: XkbCompatMapPtr,
    pub geom: XkbGeometryPtr,
}

#[repr(C)]
pub struct _XkbIndicatorRec {
    pub phys_indicators: c_ulong,
    pub maps: [XkbIndicatorMapRec; XkbNumIndicators],
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct _XkbKeyAliasRec {
    pub real: [c_char; XkbKeyNameLength],
    pub alias: [c_char; XkbKeyNameLength],
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct _XkbKeyNameRec {
    pub name: [c_char; XkbKeyNameLength],
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct _XkbNamesRec {
    pub keycodes: Atom,
    pub geometry: Atom,
    pub symbols: Atom,
    pub types: Atom,
    pub compat: Atom,
    pub vmods: [Atom; XkbNumVirtualMods],
    pub indicators: [Atom; XkbNumIndicators],
    pub groups: [Atom; XkbNumKbdGroups],
    pub keys: XkbKeyNamePtr,
    pub key_aliases: XkbKeyAliasPtr,
    pub radio_groups: *mut Atom,
    pub phys_symbols: Atom,
    pub num_keys: c_uchar,
    pub num_key_aliases: c_uchar,
    pub num_rg: c_ushort,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct _XkbStateRec {
    pub group: c_uchar,
    pub base_group: c_ushort,
    pub latched_group: c_ushort,
    pub locked_group: c_uchar,

    pub mods: c_uchar,
    pub base_mods: c_uchar,
    pub latched_mods: c_uchar,
    pub locked_mods: c_uchar,

    pub compat_state: c_uchar,

    pub grab_mods: c_uchar,
    pub compat_grab_mods: c_uchar,

    pub lookup_mods: c_uchar,
    pub compat_lookup_mods: c_uchar,

    pub ptr_buttons: c_ushort,
}

//
// Xkb event structs
//

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XkbAnyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub time: Time,
    pub xkb_type: c_int,
    pub device: c_uint,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XkbNewKeyboardNotifyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub time: Time,
    pub xkb_type: c_int,
    pub device: c_int,
    pub old_device: c_int,
    pub min_key_code: c_int,
    pub max_key_code: c_int,
    pub old_min_key_code: c_int,
    pub old_max_key_code: c_int,
    pub changed: c_uint,
    pub req_major: c_char,
    pub req_minor: c_char,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct _XkbMapNotifyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub time: Time,
    pub xkb_type: c_int,
    pub device: c_int,
    pub changed: c_uint,
    pub flags: c_uint,
    pub first_type: c_int,
    pub num_types: c_int,
    pub min_key_code: KeyCode,
    pub max_key_code: KeyCode,
    pub first_key_sym: KeyCode,
    pub first_key_act: KeyCode,
    pub first_key_bahavior: KeyCode,
    pub first_key_explicit: KeyCode,
    pub first_modmap_key: KeyCode,
    pub first_vmodmap_key: KeyCode,
    pub num_key_syms: c_int,
    pub num_key_acts: c_int,
    pub num_key_behaviors: c_int,
    pub num_key_explicit: c_int,
    pub num_modmap_keys: c_int,
    pub num_vmodmap_keys: c_int,
    pub vmods: c_uint,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XkbStateNotifyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub time: Time,
    pub xkb_type: c_int,
    pub device: c_int,
    pub changed: c_uint,
    pub group: c_int,
    pub base_group: c_int,
    pub latched_group: c_int,
    pub locked_group: c_int,
    pub mods: c_uint,
    pub base_mods: c_uint,
    pub latched_mods: c_uint,
    pub locked_mods: c_uint,
    pub compat_state: c_int,
    pub grab_mods: c_uchar,
    pub compat_grab_mods: c_uchar,
    pub lookup_mods: c_uchar,
    pub compat_lookup_mods: c_uchar,
    pub ptr_buttons: c_int,
    pub keycode: KeyCode,
    pub event_type: c_char,
    pub req_major: c_char,
    pub req_minor: c_char,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct _XkbControlsNotifyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub time: Time,
    pub xkb_type: c_int,
    pub device: c_int,
    pub changed_ctrls: c_uint,
    pub enabled_ctrls: c_uint,
    pub enabled_ctrl_changes: c_uint,
    pub num_groups: c_int,
    pub keycode: KeyCode,
    pub event_type: c_char,
    pub req_major: c_char,
    pub req_minor: c_char,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XkbIndicatorNotifyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub time: Time,
    pub xkb_type: c_int,
    pub device: c_int,
    pub changed: c_uint,
    pub state: c_uint,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct _XkbNamesNotifyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub time: Time,
    pub xkb_type: c_int,
    pub device: c_int,
    pub changed: c_uint,
    pub first_type: c_int,
    pub num_types: c_int,
    pub first_lvl: c_int,
    pub num_lvls: c_int,
    pub num_aliases: c_int,
    pub num_radio_groups: c_int,
    pub changed_vmods: c_uint,
    pub changed_groups: c_uint,
    pub changed_indicators: c_uint,
    pub first_key: c_int,
    pub num_keys: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XkbCompatMapNotifyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub time: Time,
    pub xkb_type: c_int,
    pub device: c_int,
    pub changed_groups: c_uint,
    pub first_si: c_int,
    pub num_si: c_int,
    pub num_total_si: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XkbBellNotifyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub time: Time,
    pub xkb_type: c_int,
    pub device: c_int,
    pub percent: c_int,
    pub pitch: c_int,
    pub duration: c_int,
    pub bell_class: c_int,
    pub bell_id: c_int,
    pub name: Atom,
    pub window: Window,
    pub event_only: Bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XkbActionMessageEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub time: Time,
    pub xkb_type: c_int,
    pub device: c_int,
    pub keycode: KeyCode,
    pub press: Bool,
    pub key_event_follows: Bool,
    pub group: c_int,
    pub mods: c_uint,
    pub message: [c_char; XkbActionMessageLength + 1],
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XkbAccessXNotifyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub time: Time,
    pub xkb_type: c_int,
    pub device: c_int,
    pub detail: c_int,
    pub keycode: c_int,
    pub sk_delay: c_int,
    pub debounce_delay: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct _XkbExtensionDeviceNotifyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub time: Time,
    pub xkb_type: c_int,
    pub device: c_int,
    pub reason: c_uint,
    pub supported: c_uint,
    pub unsupported: c_uint,
    pub first_btn: c_int,
    pub num_btns: c_int,
    pub leds_defined: c_uint,
    pub led_state: c_uint,
    pub led_class: c_int,
    pub led_id: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XkbEvent {
    _pad: [c_long; 24],
}

#[cfg(test)]
macro_rules! test_xkb_event_size {
  { $($ty:ty,)* } => { $(
    assert!(::std::mem::size_of::<XkbEvent>() >= ::std::mem::size_of::<$ty>());
  )* };
}

#[test]
fn xkb_event_size_test() {
    test_xkb_event_size! {
      XkbAnyEvent,
      XkbNewKeyboardNotifyEvent,
      XkbMapNotifyEvent,
      XkbStateNotifyEvent,
      XkbControlsNotifyEvent,
      XkbIndicatorNotifyEvent,
      XkbNamesNotifyEvent,
      XkbCompatMapNotifyEvent,
      XkbBellNotifyEvent,
      XkbActionMessageEvent,
      XkbAccessXNotifyEvent,
      XkbExtensionDeviceNotifyEvent,
    }
}

pub enum XkbKbdDpyStateRec {}
pub type XkbKbdDpyStatePtr = *mut XkbKbdDpyStateRec;

//
// other structures
//

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct Depth {
    pub depth: c_int,
    pub nvisuals: c_int,
    pub visuals: *mut Visual,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct Screen {
    pub ext_data: *mut XExtData,
    pub display: *mut Display,
    pub root: Window,
    pub width: c_int,
    pub height: c_int,
    pub mwidth: c_int,
    pub mheight: c_int,
    pub ndepths: c_int,
    pub depths: *mut Depth,
    pub root_depth: c_int,
    pub root_visual: *mut Visual,
    pub default_gc: GC,
    pub cmap: Colormap,
    pub white_pixel: c_ulong,
    pub black_pixel: c_ulong,
    pub max_maps: c_int,
    pub min_maps: c_int,
    pub backing_store: c_int,
    pub save_unders: Bool,
    pub root_input_mask: c_long,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct ScreenFormat {
    pub ext_data: *mut XExtData,
    pub depth: c_int,
    pub bits_per_pixel: c_int,
    pub scanline_pad: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct Visual {
    pub ext_data: *mut XExtData,
    pub visualid: VisualID,
    pub class: c_int,
    pub red_mask: c_ulong,
    pub green_mask: c_ulong,
    pub blue_mask: c_ulong,
    pub bits_per_rgb: c_int,
    pub map_entries: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XArc {
    pub x: c_short,
    pub y: c_short,
    pub width: c_ushort,
    pub height: c_ushort,
    pub angle1: c_short,
    pub angle2: c_short,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XChar2b {
    pub byte1: c_uchar,
    pub byte2: c_uchar,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XCharStruct {
    pub lbearing: c_short,
    pub rbearing: c_short,
    pub width: c_short,
    pub ascent: c_short,
    pub descent: c_short,
    pub attributes: c_ushort,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XClassHint {
    pub res_name: *mut c_char,
    pub res_class: *mut c_char,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XColor {
    pub pixel: c_ulong,
    pub red: c_ushort,
    pub green: c_ushort,
    pub blue: c_ushort,
    pub flags: c_char,
    pub pad: c_char,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XComposeStatus {
    pub compose_ptr: XPointer,
    pub chars_matched: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XExtCodes {
    pub extension: c_int,
    pub major_opcode: c_int,
    pub first_event: c_int,
    pub first_error: c_int,
}

#[repr(C)]
pub struct XExtData {
    pub number: c_int,
    pub next: *mut XExtData,
    pub free_private: Option<unsafe extern "C" fn() -> c_int>,
    pub private_data: XPointer,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XFontProp {
    pub name: Atom,
    pub card32: c_ulong,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XFontSetExtents {
    pub max_ink_extent: XRectangle,
    pub max_logical_extent: XRectangle,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XFontStruct {
    pub ext_data: *mut XExtData,
    pub fid: Font,
    pub direction: c_uint,
    pub min_char_or_byte2: c_uint,
    pub max_char_or_byte2: c_uint,
    pub min_byte1: c_uint,
    pub max_byte1: c_uint,
    pub all_chars_exist: Bool,
    pub default_char: c_uint,
    pub n_properties: c_int,
    pub properties: *mut XFontProp,
    pub min_bounds: XCharStruct,
    pub max_bounds: XCharStruct,
    pub per_char: *mut XCharStruct,
    pub ascent: c_int,
    pub descent: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XGCValues {
    pub function: c_int,
    pub plane_mask: c_ulong,
    pub foreground: c_ulong,
    pub background: c_ulong,
    pub line_width: c_int,
    pub line_style: c_int,
    pub cap_style: c_int,
    pub join_style: c_int,
    pub fill_style: c_int,
    pub fill_rule: c_int,
    pub arc_mode: c_int,
    pub tile: Pixmap,
    pub stipple: Pixmap,
    pub ts_x_origin: c_int,
    pub ts_y_origin: c_int,
    pub font: Font,
    pub subwindow_mode: c_int,
    pub graphics_exposures: Bool,
    pub clip_x_origin: c_int,
    pub clip_y_origin: c_int,
    pub clip_mask: Pixmap,
    pub dash_offset: c_int,
    pub dashes: c_char,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XGenericEventCookie {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub extension: c_int,
    pub evtype: c_int,
    pub cookie: c_uint,
    pub data: *mut c_void,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XHostAddress {
    pub family: c_int,
    pub length: c_int,
    pub address: *mut c_char,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct XServerInterpretedAddress {
    pub typelength: c_int,
    pub valuelength: c_int,
    pub type_: *mut c_char,
    pub value: *mut c_char,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XIconSize {
    pub min_width: c_int,
    pub min_height: c_int,
    pub max_width: c_int,
    pub max_height: c_int,
    pub width_inc: c_int,
    pub height_inc: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XImage {
    pub width: c_int,
    pub height: c_int,
    pub xoffset: c_int,
    pub format: c_int,
    pub data: *mut c_char,
    pub byte_order: c_int,
    pub bitmap_unit: c_int,
    pub bitmap_bit_order: c_int,
    pub bitmap_pad: c_int,
    pub depth: c_int,
    pub bytes_per_line: c_int,
    pub bits_per_pixel: c_int,
    pub red_mask: c_ulong,
    pub green_mask: c_ulong,
    pub blue_mask: c_ulong,
    pub obdata: XPointer,
    pub funcs: ImageFns,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XKeyboardControl {
    pub key_click_percent: c_int,
    pub bell_percent: c_int,
    pub bell_pitch: c_int,
    pub bell_duration: c_int,
    pub led: c_int,
    pub led_mode: c_int,
    pub key: c_int,
    pub auto_repeat_mode: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XKeyboardState {
    pub key_click_percent: c_int,
    pub bell_percent: c_int,
    pub bell_pitch: c_uint,
    pub bell_duration: c_uint,
    pub led_mask: c_ulong,
    pub global_auto_repeat: c_int,
    pub auto_repeats: [c_char; 32],
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XmbTextItem {
    pub chars: *mut c_char,
    pub nchars: c_int,
    pub delta: c_int,
    pub font_set: XFontSet,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XModifierKeymap {
    pub max_keypermod: c_int,
    pub modifiermap: *mut KeyCode,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XOMCharSetList {
    pub charset_count: c_int,
    pub charset_list: *mut *mut c_char,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XPixmapFormatValues {
    pub depth: c_int,
    pub bits_per_pixel: c_int,
    pub scanline_pad: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XPoint {
    pub x: c_short,
    pub y: c_short,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XRectangle {
    pub x: c_short,
    pub y: c_short,
    pub width: c_ushort,
    pub height: c_ushort,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XrmOptionDescRec {
    pub option: *mut c_char,
    pub specifier: *mut c_char,
    pub argKind: XrmOptionKind,
    pub value: XPointer,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XrmValue {
    pub size: c_uint,
    pub addr: XPointer,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XSegment {
    pub x1: c_short,
    pub y1: c_short,
    pub x2: c_short,
    pub y2: c_short,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XSetWindowAttributes {
    pub background_pixmap: Pixmap,
    pub background_pixel: c_ulong,
    pub border_pixmap: Pixmap,
    pub border_pixel: c_ulong,
    pub bit_gravity: c_int,
    pub win_gravity: c_int,
    pub backing_store: c_int,
    pub backing_planes: c_ulong,
    pub backing_pixel: c_ulong,
    pub save_under: Bool,
    pub event_mask: c_long,
    pub do_not_propagate_mask: c_long,
    pub override_redirect: Bool,
    pub colormap: Colormap,
    pub cursor: Cursor,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XSizeHints {
    pub flags: c_long,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub min_width: c_int,
    pub min_height: c_int,
    pub max_width: c_int,
    pub max_height: c_int,
    pub width_inc: c_int,
    pub height_inc: c_int,
    pub min_aspect: AspectRatio,
    pub max_aspect: AspectRatio,
    pub base_width: c_int,
    pub base_height: c_int,
    pub win_gravity: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XStandardColormap {
    pub colormap: Colormap,
    pub red_max: c_ulong,
    pub red_mult: c_ulong,
    pub green_max: c_ulong,
    pub green_mult: c_ulong,
    pub blue_max: c_ulong,
    pub blue_mult: c_ulong,
    pub base_pixel: c_ulong,
    pub visualid: VisualID,
    pub killid: XID,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XTextItem {
    pub chars: *mut c_char,
    pub nchars: c_int,
    pub delta: c_int,
    pub font: Font,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XTextItem16 {
    pub chars: *mut XChar2b,
    pub nchars: c_int,
    pub delta: c_int,
    pub font: Font,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XTextProperty {
    pub value: *mut c_uchar,
    pub encoding: Atom,
    pub format: c_int,
    pub nitems: c_ulong,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XTimeCoord {
    pub time: Time,
    pub x: c_short,
    pub y: c_short,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XVisualInfo {
    pub visual: *mut Visual,
    pub visualid: VisualID,
    pub screen: c_int,
    pub depth: c_int,
    pub class: c_int,
    pub red_mask: c_ulong,
    pub green_mask: c_ulong,
    pub blue_mask: c_ulong,
    pub colormap_size: c_int,
    pub bits_per_rgb: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XwcTextItem {
    pub chars: *mut wchar_t,
    pub nchars: c_int,
    pub delta: c_int,
    pub font_set: XFontSet,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XWindowAttributes {
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub depth: c_int,
    pub visual: *mut Visual,
    pub root: Window,
    pub class: c_int,
    pub bit_gravity: c_int,
    pub win_gravity: c_int,
    pub backing_store: c_int,
    pub backing_planes: c_ulong,
    pub backing_pixel: c_ulong,
    pub save_under: Bool,
    pub colormap: Colormap,
    pub map_installed: Bool,
    pub map_state: c_int,
    pub all_event_masks: c_long,
    pub your_event_mask: c_long,
    pub do_not_propagate_mask: c_long,
    pub override_redirect: Bool,
    pub screen: *mut Screen,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XWindowChanges {
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub sibling: Window,
    pub stack_mode: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XWMHints {
    pub flags: c_long,
    pub input: Bool,
    pub initial_state: c_int,
    pub icon_pixmap: Pixmap,
    pub icon_window: Window,
    pub icon_x: c_int,
    pub icon_y: c_int,
    pub icon_mask: Pixmap,
    pub window_group: XID,
}

#[repr(C)]
pub struct XIMCallback {
    pub client_data: XPointer,
    pub callback: XIMProc,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub enum XIMCaretDirection {
    XIMForwardChar,
    XIMBackwardChar,
    XIMForwardWord,
    XIMBackwardWord,
    XIMCaretUp,
    XIMCaretDown,
    XIMNextLine,
    XIMPreviousLine,
    XIMLineStart,
    XIMLineEnd,
    XIMAbsolutePosition,
    XIMDontChange,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub enum XIMCaretStyle {
    XIMIsInvisible,
    XIMIsPrimary,
    XIMIsSecondary,
}

pub type XIMFeedback = c_ulong;

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XIMPreeditDrawCallbackStruct {
    pub caret: c_int,
    pub chg_first: c_int,
    pub chg_length: c_int,
    pub text: *mut XIMText,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct XIMPreeditCaretCallbackStruct {
    pub position: c_int,
    pub direction: XIMCaretDirection,
    pub style: XIMCaretStyle,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub union XIMTextString {
    pub multi_byte: *mut c_char,
    pub wide_char: wchar_t,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct XIMText {
    pub length: c_ushort,
    pub feedback: *mut XIMFeedback,
    pub encoding_is_wchar: Bool,
    pub string: XIMTextString,
}

#[repr(C)]
pub struct XICCallback {
    pub client_data: XPointer,
    pub callback: XICProc,
}

//
// anonymous structures
//

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct AspectRatio {
    pub x: c_int,
    pub y: c_int,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[repr(C)]
pub struct ClientMessageData {
    longs: [c_long; 5],
}

impl ClientMessageData {
    pub fn as_bytes(&self) -> &[c_char] {
        self.as_ref()
    }

    pub fn as_bytes_mut(&mut self) -> &mut [c_char] {
        self.as_mut()
    }

    pub fn as_longs(&self) -> &[c_long] {
        self.as_ref()
    }

    pub fn as_longs_mut(&mut self) -> &mut [c_long] {
        self.as_mut()
    }

    pub fn as_shorts(&self) -> &[c_short] {
        self.as_ref()
    }

    pub fn as_shorts_mut(&mut self) -> &mut [c_short] {
        self.as_mut()
    }

    pub fn get_byte(&self, index: usize) -> c_char {
        self.as_bytes()[index]
    }

    pub fn get_long(&self, index: usize) -> c_long {
        self.longs[index]
    }

    pub fn get_short(&self, index: usize) -> c_short {
        self.as_shorts()[index]
    }

    pub fn new() -> ClientMessageData {
        ClientMessageData { longs: [0; 5] }
    }

    pub fn set_byte(&mut self, index: usize, value: c_char) {
        self.as_bytes_mut()[index] = value;
    }

    pub fn set_long(&mut self, index: usize, value: c_long) {
        self.longs[index] = value;
    }

    pub fn set_short(&mut self, index: usize, value: c_short) {
        self.as_shorts_mut()[index] = value;
    }
}

macro_rules! client_message_data_conversions {
  { $($ty:ty[$n:expr],)* } => {
    $(
      impl AsMut<[$ty]> for ClientMessageData {
        fn as_mut (&mut self) -> &mut [$ty] {
          unsafe { slice::from_raw_parts_mut(self.longs.as_mut_ptr() as *mut $ty, $n) }
        }
      }

      impl AsRef<[$ty]> for ClientMessageData {
        fn as_ref (&self) -> &[$ty] {
          unsafe { slice::from_raw_parts(self.longs.as_ptr() as *mut $ty, $n) }
        }
      }

      impl From<[$ty; $n]> for ClientMessageData {
        fn from (array: [$ty; $n]) -> ClientMessageData {
          unsafe { transmute_union(&array) }
        }
      }
    )*
  };
}

client_message_data_conversions! {
  c_schar[20],
  c_uchar[20],
  c_short[10],
  c_ushort[10],
  c_long[5],
  c_ulong[5],
}

#[test]
fn client_message_size_test() {
    assert!(::std::mem::size_of::<ClientMessageData>() >= ::std::mem::size_of::<[c_char; 20]>());
    assert!(::std::mem::size_of::<ClientMessageData>() >= ::std::mem::size_of::<[c_short; 10]>());
}

#[derive(Debug, Copy)]
#[repr(C)]
pub struct ImageFns {
    pub create_image: Option<
        unsafe extern "C" fn(
            *mut Display,
            *mut Visual,
            c_uint,
            c_int,
            c_int,
            *mut c_char,
            c_uint,
            c_uint,
            c_int,
            c_int,
        ) -> *mut XImage,
    >,
    pub destroy_image: Option<unsafe extern "C" fn(*mut XImage) -> c_int>,
    pub get_pixel: Option<unsafe extern "C" fn(*mut XImage, c_int, c_int) -> c_ulong>,
    pub put_pixel: Option<unsafe extern "C" fn(*mut XImage, c_int, c_int, c_ulong) -> c_int>,
    pub sub_image:
        Option<unsafe extern "C" fn(*mut XImage, c_int, c_int, c_uint, c_uint) -> *mut XImage>,
    pub add_pixel: Option<unsafe extern "C" fn(*mut XImage, c_long) -> c_int>,
}

impl Clone for ImageFns {
    fn clone(&self) -> ImageFns {
        *self
    }
}

impl PartialEq for ImageFns {
    fn eq(&self, rhs: &ImageFns) -> bool {
        unsafe { mem_eq(self, rhs) }
    }
}

//
// constants
//

// allocate colormap
pub const AllocNone: c_int = 0;
pub const AllocAll: c_int = 1;

// array sizes
pub const XkbKeyNameLength: usize = 4;
pub const XkbNumIndicators: usize = 32;
pub const XkbNumKbdGroups: usize = 4;
pub const XkbNumVirtualMods: usize = 16;

// atoms
pub const XA_PRIMARY: Atom = 1;
pub const XA_SECONDARY: Atom = 2;
pub const XA_ARC: Atom = 3;
pub const XA_ATOM: Atom = 4;
pub const XA_BITMAP: Atom = 5;
pub const XA_CARDINAL: Atom = 6;
pub const XA_COLORMAP: Atom = 7;
pub const XA_CURSOR: Atom = 8;
pub const XA_CUT_BUFFER0: Atom = 9;
pub const XA_CUT_BUFFER1: Atom = 10;
pub const XA_CUT_BUFFER2: Atom = 11;
pub const XA_CUT_BUFFER3: Atom = 12;
pub const XA_CUT_BUFFER4: Atom = 13;
pub const XA_CUT_BUFFER5: Atom = 14;
pub const XA_CUT_BUFFER6: Atom = 15;
pub const XA_CUT_BUFFER7: Atom = 16;
pub const XA_DRAWABLE: Atom = 17;
pub const XA_FONT: Atom = 18;
pub const XA_INTEGER: Atom = 19;
pub const XA_PIXMAP: Atom = 20;
pub const XA_POINT: Atom = 21;
pub const XA_RECTANGLE: Atom = 22;
pub const XA_RESOURCE_MANAGER: Atom = 23;
pub const XA_RGB_COLOR_MAP: Atom = 24;
pub const XA_RGB_BEST_MAP: Atom = 25;
pub const XA_RGB_BLUE_MAP: Atom = 26;
pub const XA_RGB_DEFAULT_MAP: Atom = 27;
pub const XA_RGB_GRAY_MAP: Atom = 28;
pub const XA_RGB_GREEN_MAP: Atom = 29;
pub const XA_RGB_RED_MAP: Atom = 30;
pub const XA_STRING: Atom = 31;
pub const XA_VISUALID: Atom = 32;
pub const XA_WINDOW: Atom = 33;
pub const XA_WM_COMMAND: Atom = 34;
pub const XA_WM_HINTS: Atom = 35;
pub const XA_WM_CLIENT_MACHINE: Atom = 36;
pub const XA_WM_ICON_NAME: Atom = 37;
pub const XA_WM_ICON_SIZE: Atom = 38;
pub const XA_WM_NAME: Atom = 39;
pub const XA_WM_NORMAL_HINTS: Atom = 40;
pub const XA_WM_SIZE_HINTS: Atom = 41;
pub const XA_WM_ZOOM_HINTS: Atom = 42;
pub const XA_MIN_SPACE: Atom = 43;
pub const XA_NORM_SPACE: Atom = 44;
pub const XA_MAX_SPACE: Atom = 45;
pub const XA_END_SPACE: Atom = 46;
pub const XA_SUPERSCRIPT_X: Atom = 47;
pub const XA_SUPERSCRIPT_Y: Atom = 48;
pub const XA_SUBSCRIPT_X: Atom = 49;
pub const XA_SUBSCRIPT_Y: Atom = 50;
pub const XA_UNDERLINE_POSITION: Atom = 51;
pub const XA_UNDERLINE_THICKNESS: Atom = 52;
pub const XA_STRIKEOUT_ASCENT: Atom = 53;
pub const XA_STRIKEOUT_DESCENT: Atom = 54;
pub const XA_ITALIC_ANGLE: Atom = 55;
pub const XA_X_HEIGHT: Atom = 56;
pub const XA_QUAD_WIDTH: Atom = 57;
pub const XA_WEIGHT: Atom = 58;
pub const XA_POINT_SIZE: Atom = 59;
pub const XA_RESOLUTION: Atom = 60;
pub const XA_COPYRIGHT: Atom = 61;
pub const XA_NOTICE: Atom = 62;
pub const XA_FONT_NAME: Atom = 63;
pub const XA_FAMILY_NAME: Atom = 64;
pub const XA_FULL_NAME: Atom = 65;
pub const XA_CAP_HEIGHT: Atom = 66;
pub const XA_WM_CLASS: Atom = 67;
pub const XA_WM_TRANSIENT_FOR: Atom = 68;

// boolean values
pub const False: Bool = 0;
pub const True: Bool = 1;

// clip rect ordering
pub const Unsorted: c_int = 0;
pub const YSorted: c_int = 1;
pub const YXSorted: c_int = 2;
pub const YXBanded: c_int = 3;

// color component mask
pub const DoRed: c_char = 1;
pub const DoGreen: c_char = 2;
pub const DoBlue: c_char = 4;

// error codes
pub const Success: c_uchar = 0;
pub const BadRequest: c_uchar = 1;
pub const BadValue: c_uchar = 2;
pub const BadWindow: c_uchar = 3;
pub const BadPixmap: c_uchar = 4;
pub const BadAtom: c_uchar = 5;
pub const BadCursor: c_uchar = 6;
pub const BadFont: c_uchar = 7;
pub const BadMatch: c_uchar = 8;
pub const BadDrawable: c_uchar = 9;
pub const BadAccess: c_uchar = 10;
pub const BadAlloc: c_uchar = 11;
pub const BadColor: c_uchar = 12;
pub const BadGC: c_uchar = 13;
pub const BadIDChoice: c_uchar = 14;
pub const BadName: c_uchar = 15;
pub const BadLength: c_uchar = 16;
pub const BadImplementation: c_uchar = 17;
pub const FirstExtensionError: c_uchar = 128;
pub const LastExtensionError: c_uchar = 255;

// event kinds
pub const KeyPress: c_int = 2;
pub const KeyRelease: c_int = 3;
pub const ButtonPress: c_int = 4;
pub const ButtonRelease: c_int = 5;
pub const MotionNotify: c_int = 6;
pub const EnterNotify: c_int = 7;
pub const LeaveNotify: c_int = 8;
pub const FocusIn: c_int = 9;
pub const FocusOut: c_int = 10;
pub const KeymapNotify: c_int = 11;
pub const Expose: c_int = 12;
pub const GraphicsExpose: c_int = 13;
pub const NoExpose: c_int = 14;
pub const VisibilityNotify: c_int = 15;
pub const CreateNotify: c_int = 16;
pub const DestroyNotify: c_int = 17;
pub const UnmapNotify: c_int = 18;
pub const MapNotify: c_int = 19;
pub const MapRequest: c_int = 20;
pub const ReparentNotify: c_int = 21;
pub const ConfigureNotify: c_int = 22;
pub const ConfigureRequest: c_int = 23;
pub const GravityNotify: c_int = 24;
pub const ResizeRequest: c_int = 25;
pub const CirculateNotify: c_int = 26;
pub const CirculateRequest: c_int = 27;
pub const PropertyNotify: c_int = 28;
pub const SelectionClear: c_int = 29;
pub const SelectionRequest: c_int = 30;
pub const SelectionNotify: c_int = 31;
pub const ColormapNotify: c_int = 32;
pub const ClientMessage: c_int = 33;
pub const MappingNotify: c_int = 34;
pub const GenericEvent: c_int = 35;
pub const LASTEvent: c_int = 36;

// event mask
pub const NoEventMask: c_long = 0;
pub const KeyPressMask: c_long = 0x0000_0001;
pub const KeyReleaseMask: c_long = 0x0000_0002;
pub const ButtonPressMask: c_long = 0x0000_0004;
pub const ButtonReleaseMask: c_long = 0x0000_0008;
pub const EnterWindowMask: c_long = 0x0000_0010;
pub const LeaveWindowMask: c_long = 0x0000_0020;
pub const PointerMotionMask: c_long = 0x0000_0040;
pub const PointerMotionHintMask: c_long = 0x0000_0080;
pub const Button1MotionMask: c_long = 0x0000_0100;
pub const Button2MotionMask: c_long = 0x0000_0200;
pub const Button3MotionMask: c_long = 0x0000_0400;
pub const Button4MotionMask: c_long = 0x0000_0800;
pub const Button5MotionMask: c_long = 0x0000_1000;
pub const ButtonMotionMask: c_long = 0x0000_2000;
pub const KeymapStateMask: c_long = 0x0000_4000;
pub const ExposureMask: c_long = 0x0000_8000;
pub const VisibilityChangeMask: c_long = 0x0001_0000;
pub const StructureNotifyMask: c_long = 0x0002_0000;
pub const ResizeRedirectMask: c_long = 0x0004_0000;
pub const SubstructureNotifyMask: c_long = 0x0008_0000;
pub const SubstructureRedirectMask: c_long = 0x0010_0000;
pub const FocusChangeMask: c_long = 0x0020_0000;
pub const PropertyChangeMask: c_long = 0x0040_0000;
pub const ColormapChangeMask: c_long = 0x0080_0000;
pub const OwnerGrabButtonMask: c_long = 0x0100_0000;

// property modes
pub const PropModeReplace: c_int = 0;
pub const PropModePrepend: c_int = 1;
pub const PropModeAppend: c_int = 2;

// modifier names
pub const ShiftMapIndex: c_int = 0;
pub const LockMapIndex: c_int = 1;
pub const ControlMapIndex: c_int = 2;
pub const Mod1MapIndex: c_int = 3;
pub const Mod2MapIndex: c_int = 4;
pub const Mod3MapIndex: c_int = 5;
pub const Mod4MapIndex: c_int = 6;
pub const Mod5MapIndex: c_int = 7;

// button masks
pub const Button1Mask: c_uint = 1 << 8;
pub const Button2Mask: c_uint = 1 << 9;
pub const Button3Mask: c_uint = 1 << 10;
pub const Button4Mask: c_uint = 1 << 11;
pub const Button5Mask: c_uint = 1 << 12;
pub const AnyModifier: c_uint = 1 << 15;

// Notify modes
pub const NotifyNormal: c_int = 0;
pub const NotifyGrab: c_int = 1;
pub const NotifyUngrab: c_int = 2;
pub const NotifyWhileGrabbed: c_int = 3;

pub const NotifyHint: c_int = 1;

// Notify detail
pub const NotifyAncestor: c_int = 0;
pub const NotifyVirtual: c_int = 1;
pub const NotifyInferior: c_int = 2;
pub const NotifyNonlinear: c_int = 3;
pub const NotifyNonlinearVirtual: c_int = 4;
pub const NotifyPointer: c_int = 5;
pub const NotifyPointerRoot: c_int = 6;
pub const NotifyDetailNone: c_int = 7;

// Visibility notify
pub const VisibilityUnobscured: c_int = 0;
pub const VisibilityPartiallyObscured: c_int = 1;
pub const VisibilityFullyObscured: c_int = 2;

// Circulation request
pub const PlaceOnTop: c_int = 0;
pub const PlaceOnBottom: c_int = 1;

// protocol families
pub const FamilyInternet: c_int = 0;
pub const FamilyDECnet: c_int = 1;
pub const FamilyChaos: c_int = 2;
pub const FamilyInternet6: c_int = 6;

// authentication families not tied to a specific protocol
pub const FamilyServerInterpreted: c_int = 5;

// property notification
pub const PropertyNewValue: c_int = 0;
pub const PropertyDelete: c_int = 1;

// Color Map notification
pub const ColormapUninstalled: c_int = 0;
pub const ColormapInstalled: c_int = 1;

// grab modes
pub const GrabModeSync: c_int = 0;
pub const GrabModeAsync: c_int = 1;

// grab status
pub const GrabSuccess: c_int = 0;
pub const AlreadyGrabbed: c_int = 1;
pub const GrabInvalidTime: c_int = 2;
pub const GrabNotViewable: c_int = 3;
pub const GrabFrozen: c_int = 4;

// AllowEvents modes
pub const AsyncPointer: c_int = 0;
pub const SyncPointer: c_int = 1;
pub const ReplayPointer: c_int = 2;
pub const AsyncKeyboard: c_int = 3;
pub const SyncKeyboard: c_int = 4;
pub const ReplayKeyboard: c_int = 5;
pub const AsyncBoth: c_int = 6;
pub const SyncBoth: c_int = 7;

// Used in SetInputFocus, GetInputFocus
pub const RevertToNone: c_int = 0;
pub const RevertToPointerRoot: c_int = 1;
pub const RevertToParent: c_int = 2;

// ConfigureWindow structure
pub const CWX: c_ushort = 1 << 0;
pub const CWY: c_ushort = 1 << 1;
pub const CWWidth: c_ushort = 1 << 2;
pub const CWHeight: c_ushort = 1 << 3;
pub const CWBorderWidth: c_ushort = 1 << 4;
pub const CWSibling: c_ushort = 1 << 5;
pub const CWStackMode: c_ushort = 1 << 6;

// gravity
pub const ForgetGravity: c_int = 0;
pub const UnmapGravity: c_int = 0;
pub const NorthWestGravity: c_int = 1;
pub const NorthGravity: c_int = 2;
pub const NorthEastGravity: c_int = 3;
pub const WestGravity: c_int = 4;
pub const CenterGravity: c_int = 5;
pub const EastGravity: c_int = 6;
pub const SouthWestGravity: c_int = 7;
pub const SouthGravity: c_int = 8;
pub const SouthEastGravity: c_int = 9;
pub const StaticGravity: c_int = 10;

// image format
pub const XYBitmap: c_int = 0;
pub const XYPixmap: c_int = 1;
pub const ZPixmap: c_int = 2;

// Used in CreateWindow for backing-store hint
pub const NotUseful: c_int = 0;
pub const WhenMapped: c_int = 1;
pub const Always: c_int = 2;

// map state
pub const IsUnmapped: c_int = 0;
pub const IsUnviewable: c_int = 1;
pub const IsViewable: c_int = 2;

// modifier keys mask
pub const ShiftMask: c_uint = 0x01;
pub const LockMask: c_uint = 0x02;
pub const ControlMask: c_uint = 0x04;
pub const Mod1Mask: c_uint = 0x08;
pub const Mod2Mask: c_uint = 0x10;
pub const Mod3Mask: c_uint = 0x20;
pub const Mod4Mask: c_uint = 0x40;
pub const Mod5Mask: c_uint = 0x80;

// mouse buttons
pub const Button1: c_uint = 1;
pub const Button2: c_uint = 2;
pub const Button3: c_uint = 3;
pub const Button4: c_uint = 4;
pub const Button5: c_uint = 5;

// size hints mask
pub const USPosition: c_long = 0x0001;
pub const USSize: c_long = 0x0002;
pub const PPosition: c_long = 0x0004;
pub const PSize: c_long = 0x0008;
pub const PMinSize: c_long = 0x0010;
pub const PMaxSize: c_long = 0x0020;
pub const PResizeInc: c_long = 0x0040;
pub const PAspect: c_long = 0x0080;
pub const PBaseSize: c_long = 0x0100;
pub const PWinGravity: c_long = 0x0200;
pub const PAllHints: c_long = PPosition | PSize | PMinSize | PMaxSize | PResizeInc | PAspect;

// Used in ChangeSaveSet
pub const SetModeInsert: c_int = 0;
pub const SetModeDelete: c_int = 1;

// Used in ChangeCloseDownMode
pub const DestroyAll: c_int = 0;
pub const RetainPermanent: c_int = 1;
pub const RetainTemporary: c_int = 2;

// Window stacking method (in configureWindow)
pub const Above: c_int = 0;
pub const Below: c_int = 1;
pub const TopIf: c_int = 2;
pub const BottomIf: c_int = 3;
pub const Opposite: c_int = 4;

// Circulation direction
pub const RaiseLowest: c_int = 0;
pub const LowerHighest: c_int = 1;

// graphics functions
pub const GXclear: c_int = 0x0;
pub const GXand: c_int = 0x1;
pub const GXandReverse: c_int = 0x2;
pub const GXcopy: c_int = 0x3;
pub const GXandInverted: c_int = 0x4;
pub const GXnoop: c_int = 0x5;
pub const GXxor: c_int = 0x6;
pub const GXor: c_int = 0x7;
pub const GXnor: c_int = 0x8;
pub const GXequiv: c_int = 0x9;
pub const GXinvert: c_int = 0xa;
pub const GXorReverse: c_int = 0xb;
pub const GXcopyInverted: c_int = 0xc;
pub const GXorInverted: c_int = 0xd;
pub const GXnand: c_int = 0xe;
pub const GXset: c_int = 0xf;

// LineStyle
pub const LineSolid: c_int = 0;
pub const LineOnOffDash: c_int = 1;
pub const LineDoubleDash: c_int = 2;

// capStyle
pub const CapNotLast: c_int = 0;
pub const CapButt: c_int = 1;
pub const CapRound: c_int = 2;
pub const CapProjecting: c_int = 3;

// joinStyle
pub const JoinMiter: c_int = 0;
pub const JoinRound: c_int = 1;
pub const JoinBevel: c_int = 2;

// fillStyle
pub const FillSolid: c_int = 0;
pub const FillTiled: c_int = 1;
pub const FillStippled: c_int = 2;
pub const FillOpaqueStippled: c_int = 3;

// fillRule
pub const EvenOddRule: c_int = 0;
pub const WindingRule: c_int = 1;

// subwindow mode
pub const ClipByChildren: c_int = 0;
pub const IncludeInferiors: c_int = 1;

// CoordinateMode for drawing routines
pub const CoordModeOrigin: c_int = 0;
pub const CoordModePrevious: c_int = 1;

// Polygon shapes
pub const Complex: c_int = 0;
pub const Nonconvex: c_int = 1;
pub const Convex: c_int = 2;

// Arc modes for PolyFillArc
pub const ArcChord: c_int = 0;
pub const ArcPieSlice: c_int = 1;

// GC components
pub const GCFunction: c_uint = 1 << 0;
pub const GCPlaneMask: c_uint = 1 << 1;
pub const GCForeground: c_uint = 1 << 2;
pub const GCBackground: c_uint = 1 << 3;
pub const GCLineWidth: c_uint = 1 << 4;
pub const GCLineStyle: c_uint = 1 << 5;
pub const GCCapStyle: c_uint = 1 << 6;
pub const GCJoinStyle: c_uint = 1 << 7;
pub const GCFillStyle: c_uint = 1 << 8;
pub const GCFillRule: c_uint = 1 << 9;
pub const GCTile: c_uint = 1 << 10;
pub const GCStipple: c_uint = 1 << 11;
pub const GCTileStipXOrigin: c_uint = 1 << 12;
pub const GCTileStipYOrigin: c_uint = 1 << 13;
pub const GCFont: c_uint = 1 << 14;
pub const GCSubwindowMode: c_uint = 1 << 15;
pub const GCGraphicsExposures: c_uint = 1 << 16;
pub const GCClipXOrigin: c_uint = 1 << 17;
pub const GCClipYOrigin: c_uint = 1 << 18;
pub const GCClipMask: c_uint = 1 << 19;
pub const GCDashOffset: c_uint = 1 << 20;
pub const GCDashList: c_uint = 1 << 21;
pub const GCArcMode: c_uint = 1 << 22;

pub const GCLastBit: c_uint = 22;

// draw direction
pub const FontLeftToRight: c_int = 0;
pub const FontRightToLeft: c_int = 1;

pub const FontChange: c_uchar = 255;

// QueryBestSize Class
pub const CursorShape: c_int = 0;
pub const TileShape: c_int = 1;
pub const StippleShape: c_int = 2;

// keyboard autorepeat
pub const AutoRepeatModeOff: c_int = 0;
pub const AutoRepeatModeOn: c_int = 1;
pub const AutoRepeatModeDefault: c_int = 2;

pub const LedModeOff: c_int = 0;
pub const LedModeOn: c_int = 1;

// masks for ChangeKeyboardControl
pub const KBKeyClickPercent: c_ulong = 1 << 0;
pub const KBBellPercent: c_ulong = 1 << 1;
pub const KBBellPitch: c_ulong = 1 << 2;
pub const KBBellDuration: c_ulong = 1 << 3;
pub const KBLed: c_ulong = 1 << 4;
pub const KBLedMode: c_ulong = 1 << 5;
pub const KBKey: c_ulong = 1 << 6;
pub const KBAutoRepeatMode: c_ulong = 1 << 7;

pub const MappingSuccess: c_uchar = 0;
pub const MappingBusy: c_uchar = 1;
pub const MappingFailed: c_uchar = 2;

pub const MappingModifier: c_int = 0;
pub const MappingKeyboard: c_int = 1;
pub const MappingPointer: c_int = 2;

// screensaver
pub const DontPreferBlanking: c_int = 0;
pub const PreferBlanking: c_int = 1;
pub const DefaultBlanking: c_int = 2;

pub const DisableScreenSaver: c_int = 0;
pub const DisableScreenInterval: c_int = 0;

pub const DontAllowExposures: c_int = 0;
pub const AllowExposures: c_int = 1;
pub const DefaultExposures: c_int = 2;

pub const ScreenSaverReset: c_int = 0;
pub const ScreenSaverActive: c_int = 1;

// hosts and connections
pub const HostInsert: c_uchar = 0;
pub const HostDelete: c_uchar = 1;

pub const EnableAccess: c_int = 1;
pub const DisableAccess: c_int = 0;

// visual class
pub const StaticGray: c_int = 0;
pub const GrayScale: c_int = 1;
pub const StaticColor: c_int = 2;
pub const PseudoColor: c_int = 3;
pub const TrueColor: c_int = 4;
pub const DirectColor: c_int = 5;

// visual info mask
pub const VisualNoMask: c_long = 0x0000;
pub const VisualIDMask: c_long = 0x0001;
pub const VisualScreenMask: c_long = 0x0002;
pub const VisualDepthMask: c_long = 0x0004;
pub const VisualClassMask: c_long = 0x0008;
pub const VisualRedMaskMask: c_long = 0x0010;
pub const VisualGreenMaskMask: c_long = 0x0020;
pub const VisualBlueMaskMask: c_long = 0x0040;
pub const VisualColormapSizeMask: c_long = 0x0080;
pub const VisualBitsPerRGBMask: c_long = 0x0100;
pub const VisualAllMask: c_long = 0x01ff;

// window attributes
pub const CWBackPixmap: c_ulong = 0x0001;
pub const CWBackPixel: c_ulong = 0x0002;
pub const CWBorderPixmap: c_ulong = 0x0004;
pub const CWBorderPixel: c_ulong = 0x0008;
pub const CWBitGravity: c_ulong = 0x0010;
pub const CWWinGravity: c_ulong = 0x0020;
pub const CWBackingStore: c_ulong = 0x0040;
pub const CWBackingPlanes: c_ulong = 0x0080;
pub const CWBackingPixel: c_ulong = 0x0100;
pub const CWOverrideRedirect: c_ulong = 0x0200;
pub const CWSaveUnder: c_ulong = 0x0400;
pub const CWEventMask: c_ulong = 0x0800;
pub const CWDontPropagate: c_ulong = 0x1000;
pub const CWColormap: c_ulong = 0x2000;
pub const CWCursor: c_ulong = 0x4000;

// window classes
pub const InputOutput: c_int = 1;
pub const InputOnly: c_int = 2;

// XCreateIC values
pub const XIMPreeditArea: c_int = 0x0001;
pub const XIMPreeditCallbacks: c_int = 0x0002;
pub const XIMPreeditPosition: c_int = 0x0004;
pub const XIMPreeditNothing: c_int = 0x0008;
pub const XIMPreeditNone: c_int = 0x0010;
pub const XIMStatusArea: c_int = 0x0100;
pub const XIMStatusCallbacks: c_int = 0x0200;
pub const XIMStatusNothing: c_int = 0x0400;
pub const XIMStatusNone: c_int = 0x0800;

// Byte order  used in imageByteOrder and bitmapBitOrder
pub const LSBFirst: c_int = 0;
pub const MSBFirst: c_int = 1;

// Reserved resource and constant definitions
//pub const None: c_int = 0;
pub const ParentRelative: c_int = 1;
pub const CopyFromParent: c_int = 0;
pub const PointerWindow: c_int = 0;
pub const InputFocus: c_int = 1;
pub const PointerRoot: c_int = 1;
pub const AnyPropertyType: c_int = 0;
pub const AnyKey: c_int = 0;
pub const AnyButton: c_int = 0;
pub const AllTemporary: c_int = 0;
pub const CurrentTime: Time = 0;
pub const NoSymbol: c_int = 0;

/* Definitions for the X window system likely to be used by applications */
pub const X_PROTOCOL: c_int = 11;
pub const X_PROTOCOL_REVISION: c_int = 0;

pub const XNVaNestedList: &str = "XNVaNestedList";
pub const XNQueryInputStyle: &str = "queryInputStyle";
pub const XNClientWindow: &str = "clientWindow";
pub const XNInputStyle: &str = "inputStyle";
pub const XNFocusWindow: &str = "focusWindow";
pub const XNResourceName: &str = "resourceName";
pub const XNResourceClass: &str = "resourceClass";
pub const XNGeometryCallback: &str = "geometryCallback";
pub const XNDestroyCallback: &str = "destroyCallback";
pub const XNFilterEvents: &str = "filterEvents";
pub const XNPreeditStartCallback: &str = "preeditStartCallback";
pub const XNPreeditDoneCallback: &str = "preeditDoneCallback";
pub const XNPreeditDrawCallback: &str = "preeditDrawCallback";
pub const XNPreeditCaretCallback: &str = "preeditCaretCallback";
pub const XNPreeditStateNotifyCallback: &str = "preeditStateNotifyCallback";
pub const XNPreeditAttributes: &str = "preeditAttributes";
pub const XNStatusStartCallback: &str = "statusStartCallback";
pub const XNStatusDoneCallback: &str = "statusDoneCallback";
pub const XNStatusDrawCallback: &str = "statusDrawCallback";
pub const XNStatusAttributes: &str = "statusAttributes";
pub const XNArea: &str = "area";
pub const XNAreaNeeded: &str = "areaNeeded";
pub const XNSpotLocation: &str = "spotLocation";
pub const XNColormap: &str = "colorMap";
pub const XNStdColormap: &str = "stdColorMap";
pub const XNForeground: &str = "foreground";
pub const XNBackground: &str = "background";
pub const XNBackgroundPixmap: &str = "backgroundPixmap";
pub const XNFontSet: &str = "fontSet";
pub const XNLineSpace: &str = "lineSpace";
pub const XNCursor: &str = "cursor";

pub const XNVaNestedList_0: &[u8] = b"XNVaNestedList\0";
pub const XNQueryInputStyle_0: &[u8] = b"queryInputStyle\0";
pub const XNClientWindow_0: &[u8] = b"clientWindow\0";
pub const XNInputStyle_0: &[u8] = b"inputStyle\0";
pub const XNFocusWindow_0: &[u8] = b"focusWindow\0";
pub const XNResourceName_0: &[u8] = b"resourceName\0";
pub const XNResourceClass_0: &[u8] = b"resourceClass\0";
pub const XNGeometryCallback_0: &[u8] = b"geometryCallback\0";
pub const XNDestroyCallback_0: &[u8] = b"destroyCallback\0";
pub const XNFilterEvents_0: &[u8] = b"filterEvents\0";
pub const XNPreeditStartCallback_0: &[u8] = b"preeditStartCallback\0";
pub const XNPreeditDoneCallback_0: &[u8] = b"preeditDoneCallback\0";
pub const XNPreeditDrawCallback_0: &[u8] = b"preeditDrawCallback\0";
pub const XNPreeditCaretCallback_0: &[u8] = b"preeditCaretCallback\0";
pub const XNPreeditStateNotifyCallback_0: &[u8] = b"preeditStateNotifyCallback\0";
pub const XNPreeditAttributes_0: &[u8] = b"preeditAttributes\0";
pub const XNStatusStartCallback_0: &[u8] = b"statusStartCallback\0";
pub const XNStatusDoneCallback_0: &[u8] = b"statusDoneCallback\0";
pub const XNStatusDrawCallback_0: &[u8] = b"statusDrawCallback\0";
pub const XNStatusAttributes_0: &[u8] = b"statusAttributes\0";
pub const XNArea_0: &[u8] = b"area\0";
pub const XNAreaNeeded_0: &[u8] = b"areaNeeded\0";
pub const XNSpotLocation_0: &[u8] = b"spotLocation\0";
pub const XNColormap_0: &[u8] = b"colorMap\0";
pub const XNStdColormap_0: &[u8] = b"stdColorMap\0";
pub const XNForeground_0: &[u8] = b"foreground\0";
pub const XNBackground_0: &[u8] = b"background\0";
pub const XNBackgroundPixmap_0: &[u8] = b"backgroundPixmap\0";
pub const XNFontSet_0: &[u8] = b"fontSet\0";
pub const XNLineSpace_0: &[u8] = b"lineSpace\0";
pub const XNCursor_0: &[u8] = b"cursor\0";

pub const XNQueryIMValuesList: &str = "queryIMValuesList";
pub const XNQueryICValuesList: &str = "queryICValuesList";
pub const XNVisiblePosition: &str = "visiblePosition";
pub const XNR6PreeditCallback: &str = "r6PreeditCallback";
pub const XNStringConversionCallback: &str = "stringConversionCallback";
pub const XNStringConversion: &str = "stringConversion";
pub const XNResetState: &str = "resetState";
pub const XNHotKey: &str = "hotKey";
pub const XNHotKeyState: &str = "hotKeyState";
pub const XNPreeditState: &str = "preeditState";
pub const XNSeparatorofNestedList: &str = "separatorofNestedList";

pub const XNQueryIMValuesList_0: &[u8] = b"queryIMValuesList\0";
pub const XNQueryICValuesList_0: &[u8] = b"queryICValuesList\0";
pub const XNVisiblePosition_0: &[u8] = b"visiblePosition\0";
pub const XNR6PreeditCallback_0: &[u8] = b"r6PreeditCallback\0";
pub const XNStringConversionCallback_0: &[u8] = b"stringConversionCallback\0";
pub const XNStringConversion_0: &[u8] = b"stringConversion\0";
pub const XNResetState_0: &[u8] = b"resetState\0";
pub const XNHotKey_0: &[u8] = b"hotKey\0";
pub const XNHotKeyState_0: &[u8] = b"hotKeyState\0";
pub const XNPreeditState_0: &[u8] = b"preeditState\0";
pub const XNSeparatorofNestedList_0: &[u8] = b"separatorofNestedList\0";

pub const XBufferOverflow: i32 = -1;
pub const XLookupNone: i32 = 1;
pub const XLookupChars: i32 = 2;
pub const XLookupKeySym: i32 = 3;
pub const XLookupBoth: i32 = 4;

// Xkb constants
pub const XkbActionMessageLength: usize = 6;

pub const XkbOD_Success: c_int = 0;
pub const XkbOD_BadLibraryVersion: c_int = 1;
pub const XkbOD_ConnectionRefused: c_int = 2;
pub const XkbOD_NonXkbServer: c_int = 3;
pub const XkbOD_BadServerVersion: c_int = 4;

pub const XkbLC_ForceLatinLookup: c_uint = 1 << 0;
pub const XkbLC_ConsumeLookupMods: c_uint = 1 << 1;
pub const XkbLC_AlwaysConsumeShiftAndLock: c_uint = 1 << 2;
pub const XkbLC_IgnoreNewKeyboards: c_uint = 1 << 3;
pub const XkbLC_ControlFallback: c_uint = 1 << 4;
pub const XkbLC_ConsumeKeysOnComposeFail: c_uint = 1 << 29;
pub const XkbLC_ComposeLED: c_uint = 1 << 30;
pub const XkbLC_BeepOnComposeFail: c_uint = 1 << 31;

pub const XkbLC_AllComposeControls: c_uint = 0xc000_0000;
pub const XkbLC_AllControls: c_uint = 0xc000_001f;

pub const XkbNewKeyboardNotify: c_int = 0;
pub const XkbMapNotify: c_int = 1;
pub const XkbStateNotify: c_int = 2;
pub const XkbControlsNotify: c_int = 3;
pub const XkbIndicatorStateNotify: c_int = 4;
pub const XkbIndicatorMapNotify: c_int = 5;
pub const XkbNamesNotify: c_int = 6;
pub const XkbCompatMapNotify: c_int = 7;
pub const XkbBellNotify: c_int = 8;
pub const XkbActionMessage: c_int = 9;
pub const XkbAccessXNotify: c_int = 10;
pub const XkbExtensionDeviceNotify: c_int = 11;

pub const XkbNewKeyboardNotifyMask: c_ulong = 1 << 0;
pub const XkbMapNotifyMask: c_ulong = 1 << 1;
pub const XkbStateNotifyMask: c_ulong = 1 << 2;
pub const XkbControlsNotifyMask: c_ulong = 1 << 3;
pub const XkbIndicatorStateNotifyMask: c_ulong = 1 << 4;
pub const XkbIndicatorMapNotifyMask: c_ulong = 1 << 5;
pub const XkbNamesNotifyMask: c_ulong = 1 << 6;
pub const XkbCompatMapNotifyMask: c_ulong = 1 << 7;
pub const XkbBellNotifyMask: c_ulong = 1 << 8;
pub const XkbActionMessageMask: c_ulong = 1 << 9;
pub const XkbAccessXNotifyMask: c_ulong = 1 << 10;
pub const XkbExtensionDeviceNotifyMask: c_ulong = 1 << 11;
pub const XkbAllEventsMask: c_ulong = 0xfff;

pub const XkbModifierStateMask: c_ulong = 1 << 0;
pub const XkbModifierBaseMask: c_ulong = 1 << 1;
pub const XkbModifierLatchMask: c_ulong = 1 << 2;
pub const XkbModifierLockMask: c_ulong = 1 << 3;
pub const XkbGroupStateMask: c_ulong = 1 << 4;
pub const XkbGroupBaseMask: c_ulong = 1 << 5;
pub const XkbGroupLatchMask: c_ulong = 1 << 6;
pub const XkbGroupLockMask: c_ulong = 1 << 7;
pub const XkbCompatStateMask: c_ulong = 1 << 8;
pub const XkbGrabModsMask: c_ulong = 1 << 9;
pub const XkbCompatGrabModsMask: c_ulong = 1 << 10;
pub const XkbLookupModsMask: c_ulong = 1 << 11;
pub const XkbCompatLookupModsMask: c_ulong = 1 << 12;
pub const XkbPointerButtonMask: c_ulong = 1 << 13;
pub const XkbAllStateComponentsMask: c_ulong = 0x3fff;

// Bitmask returned by XParseGeometry
pub const NoValue: c_int = 0x0000;
pub const XValue: c_int = 0x0001;
pub const YValue: c_int = 0x0002;
pub const WidthValue: c_int = 0x0004;
pub const HeightValue: c_int = 0x0008;
pub const AllValues: c_int = 0x000f;
pub const XNegative: c_int = 0x0010;
pub const YNegative: c_int = 0x0020;

// Definition for flags of XWMHints
pub const InputHint: c_long = 1 << 0;
pub const StateHint: c_long = 1 << 1;
pub const IconPixmapHint: c_long = 1 << 2;
pub const IconWindowHint: c_long = 1 << 3;
pub const IconPositionHint: c_long = 1 << 4;
pub const IconMaskHint: c_long = 1 << 5;
pub const WindowGroupHint: c_long = 1 << 6;
pub const AllHints: c_long = InputHint
    | StateHint
    | IconPixmapHint
    | IconWindowHint
    | IconPositionHint
    | IconMaskHint
    | WindowGroupHint;
pub const XUrgencyHint: c_long = 1 << 8;

// XICCEncodingStyle
pub const XStringStyle: c_int = 0;
pub const XCompoundTextStyle: c_int = 1;
pub const XTextStyle: c_int = 2;
pub const XStdICCTextStyle: c_int = 3;
pub const XUTF8StringStyle: c_int = 4;

//
// inline functions
//

