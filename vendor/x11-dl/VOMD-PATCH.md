# x11-dl 2.21.0, patched for VOMD

The library's sources (`src/`), build script and license (MIT) of x11-dl 2.21.0 from crates.io
(<https://github.com/AltF02/x11-rs>), used through `[patch.crates-io]` in VOMD's `Cargo.toml`
and `fuzz/Cargo.toml`. winit (X11, Linux) is its only user. Its `Cargo.toml` is the published
one's source (`Cargo.toml.orig`) without the `workspace` key. The change:

- **Only the functions winit calls** (size, about 105 KB; and about 1,700 fewer `dlsym` calls
  when VOMD starts on X11): each library's `x11_link!` block made a struct with a field for
  every function of the library (767 for Xlib alone), and `open` looked every one up, so all
  of them were linked and resolved. The blocks of `xlib.rs`, `xcursor.rs`, `xinput2.rs` and
  `xlib_xcb.rs` keep the 49 winit 0.30 calls (their counts updated); the others are deleted, with
  the imports only they used and `XUniqueContext` (behind a `feature = "xlib"` the crate doesn't
  have). The other modules are as upstream: winit doesn't use them, so nothing of them is
  linked. An old libX11 without one of the deleted functions no longer fails to load.

  The 49: XCheckIfEvent, XCloseDisplay, XCloseIM, XCreateBitmapFromData, XCreateIC,
  XCreatePixmapCursor, XDefaultRootWindow, XDefaultScreen, XDestroyIC, XFilterEvent, XFlush,
  XFree, XFreeCursor, XFreeEventData, XFreeModifiermap, XFreePixmap, XGetAtomNames,
  XGetErrorText, XGetEventData, XGetIMValues, XGetModifierMapping, XGetXCBConnection,
  XIFreeDeviceInfo, XIQueryDevice, XInitThreads, XOpenDisplay, XOpenIM, XPending,
  XQueryKeymap, XRegisterIMInstantiateCallback, XRootWindow, XSetErrorHandler, XSetICFocus,
  XSetICValues, XSetIMValues, XSetLocaleModifiers, XSupportsLocale, XSync,
  XUnregisterIMInstantiateCallback, XUnsetICFocus, XVaCreateNestedList, XcursorImageCreate,
  XcursorImageDestroy, XcursorImageLoadCursor, XcursorLibraryLoadCursor, XkbGetState,
  XkbSelectEvents, XkbSetDetectableAutoRepeat, Xutf8LookupString.

When winit is upgraded, the list is checked by the compiler: a function winit calls that is
missing here is a build error (`no field XFoo on type Xlib`). To make the list again: copy the
new x11-dl's `src/` here, delete every `pub fn` line of those four blocks, run
`cargo check` (on Linux, default features), add back each function an error names (from the
upstream file, in its order), and repeat until it builds; then remove the imports the compiler
says are unused. Check on X11 that a window opens, draws, takes keys and scrolls and changes
its cursor (in a container: `scripts/clipboard-check.sh` drives a real X server). x11-dl itself
changes rarely: on its upgrade, do the same.
