//! Bindings to X11, Cairo and Pango.
//!
//! Declared by hand against the system headers rather than pulled from a crate,
//! for the same reason the rest of NOUS carries no dependencies: the whole
//! system should build on an air-gapped machine with a Rust toolchain and the
//! libraries the desktop already has.
//!
//! Every signature here was checked against the installed headers
//! (`X11/Xlib.h`, `cairo/cairo-xlib.h`, `pango/pangocairo.h`). Getting one
//! wrong is silent memory corruption, not a compile error, so they are grouped
//! with the header line they came from and kept in that order.

#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code
)]

use std::os::raw::{c_char, c_int, c_long, c_uchar, c_uint, c_ulong, c_void};

pub type Display = c_void;
pub type Window = c_ulong;
pub type Drawable = c_ulong;
pub type Colormap = c_ulong;
pub type Visual = c_void;
pub type Atom = c_ulong;
pub type Time = c_ulong;
pub type KeySym = c_ulong;
pub type Status = c_int;
pub type Bool = c_int;
pub type XIM = *mut c_void;
pub type XIC = *mut c_void;
pub type XrmDatabase = *mut c_void;

// --- event masks (X.h) ---------------------------------------------------
pub const KeyPressMask: c_long = 1 << 0;
pub const KeyReleaseMask: c_long = 1 << 1;
pub const ButtonPressMask: c_long = 1 << 2;
pub const ButtonReleaseMask: c_long = 1 << 3;
pub const PointerMotionMask: c_long = 1 << 6;
pub const ExposureMask: c_long = 1 << 15;
pub const StructureNotifyMask: c_long = 1 << 17;
pub const FocusChangeMask: c_long = 1 << 21;

// --- event types (X.h) ---------------------------------------------------
pub const KeyPress: c_int = 2;
pub const KeyRelease: c_int = 3;
pub const ButtonPress: c_int = 4;
pub const ButtonRelease: c_int = 5;
pub const MotionNotify: c_int = 6;
pub const FocusIn: c_int = 9;
pub const FocusOut: c_int = 10;
pub const Expose: c_int = 12;
pub const ConfigureNotify: c_int = 22;
pub const ClientMessage: c_int = 33;

pub const CWBackPixel: c_ulong = 1 << 1;
pub const CWBorderPixel: c_ulong = 1 << 3;
pub const CWOverrideRedirect: c_ulong = 1 << 9;
pub const CWEventMask: c_ulong = 1 << 11;
pub const CWColormap: c_ulong = 1 << 13;

pub const PropertyChangeMask: c_long = 1 << 22;

pub const InputOutput: c_uint = 1;
pub const PropModeReplace: c_int = 0;
pub const XA_ATOM: Atom = 4;
pub const XA_STRING: Atom = 31;
pub const XA_CARDINAL: Atom = 6;
pub const TrueColor: c_int = 4;
pub const AllocNone: c_int = 0;
pub const CurrentTime: Time = 0;
pub const None_: c_ulong = 0;

pub const XBufferOverflow: Status = -1;
pub const XLookupNone: Status = 1;
pub const XLookupChars: Status = 2;
pub const XLookupKeySym: Status = 3;
pub const XLookupBoth: Status = 4;

// Keysyms we act on (keysymdef.h).
pub const XK_Escape: KeySym = 0xff1b;
pub const XK_Return: KeySym = 0xff0d;
pub const XK_KP_Enter: KeySym = 0xff8d;
pub const XK_BackSpace: KeySym = 0xff08;
pub const XK_Delete: KeySym = 0xffff;
pub const XK_Tab: KeySym = 0xff09;
pub const XK_Up: KeySym = 0xff52;
pub const XK_Down: KeySym = 0xff54;
pub const XK_Left: KeySym = 0xff51;
pub const XK_Right: KeySym = 0xff53;
pub const XK_Home: KeySym = 0xff50;
pub const XK_End: KeySym = 0xff57;
pub const XK_Page_Up: KeySym = 0xff55;
pub const XK_Page_Down: KeySym = 0xff56;

// Focus-change `mode` (X.h:267) and `detail` (X.h:276).
pub const NotifyNormal: c_int = 0;
pub const NotifyGrab: c_int = 1;
pub const NotifyUngrab: c_int = 2;
pub const NotifyWhileGrabbed: c_int = 3;
pub const NotifyAncestor: c_int = 0;
pub const NotifyVirtual: c_int = 1;
pub const NotifyInferior: c_int = 2;
pub const NotifyNonlinear: c_int = 3;
pub const NotifyNonlinearVirtual: c_int = 4;
pub const NotifyPointer: c_int = 5;
pub const NotifyPointerRoot: c_int = 6;

pub const ShiftMask: c_uint = 1 << 0;
pub const ControlMask: c_uint = 1 << 2;
pub const Mod1Mask: c_uint = 1 << 3;

#[repr(C)]
pub struct XSetWindowAttributes {
    pub background_pixmap: c_ulong,
    pub background_pixel: c_ulong,
    pub border_pixmap: c_ulong,
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
    pub cursor: c_ulong,
}

impl Default for XSetWindowAttributes {
    fn default() -> Self {
        // SAFETY: every field is a plain integer or handle; all-zero is the
        // documented "unset" state for each of them.
        unsafe { std::mem::zeroed() }
    }
}

/// `XEvent` is a union; 192 bytes is comfortably larger than the largest
/// member on 64-bit, and X11 only ever writes through the union.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct XEvent {
    pub type_: c_int,
    pub pad: [c_long; 24],
}

#[repr(C)]
#[derive(Clone, Copy)]
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

#[repr(C)]
#[derive(Clone, Copy)]
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

#[repr(C)]
#[derive(Clone, Copy)]
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

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XClientMessageEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub message_type: Atom,
    pub format: c_int,
    pub data_l: [c_long; 5],
}

/// FocusIn/FocusOut (Xlib.h:632). `mode` and `detail` are what separate a real
/// focus change from the flurry X sends whenever a grab activates -- opening a
/// menu, Alt-Tab, a screenshot tool, another app's global hotkey.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct XFocusChangeEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub mode: c_int,
    pub detail: c_int,
}

#[repr(C)]
pub struct XClassHint {
    pub res_name: *mut c_char,
    pub res_class: *mut c_char,
}

/// `class` is a keyword in Rust, so the field is `class_`. The layout is what
/// matters and it is unchanged (Xutil.h:288).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct XVisualInfo {
    pub visual: *mut Visual,
    pub visualid: c_ulong,
    pub screen: c_int,
    pub depth: c_int,
    pub class_: c_int,
    pub red_mask: c_ulong,
    pub green_mask: c_ulong,
    pub blue_mask: c_ulong,
    pub colormap_size: c_int,
    pub bits_per_rgb: c_int,
}

impl Default for XVisualInfo {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[link(name = "X11")]
extern "C" {
    pub fn XOpenDisplay(name: *const c_char) -> *mut Display;
    pub fn XCloseDisplay(dpy: *mut Display) -> c_int;
    pub fn XDefaultScreen(dpy: *mut Display) -> c_int;
    pub fn XRootWindow(dpy: *mut Display, screen: c_int) -> Window;
    pub fn XDefaultVisual(dpy: *mut Display, screen: c_int) -> *mut Visual;
    pub fn XDefaultColormap(dpy: *mut Display, screen: c_int) -> Colormap;
    pub fn XDefaultDepth(dpy: *mut Display, screen: c_int) -> c_int;
    pub fn XDisplayWidth(dpy: *mut Display, screen: c_int) -> c_int;
    pub fn XDisplayHeight(dpy: *mut Display, screen: c_int) -> c_int;

    pub fn XCreateWindow(
        dpy: *mut Display,
        parent: Window,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
        border_width: c_uint,
        depth: c_int,
        class: c_uint,
        visual: *mut Visual,
        valuemask: c_ulong,
        attributes: *mut XSetWindowAttributes,
    ) -> Window;
    pub fn XDestroyWindow(dpy: *mut Display, w: Window) -> c_int;
    pub fn XMapWindow(dpy: *mut Display, w: Window) -> c_int;
    pub fn XMapRaised(dpy: *mut Display, w: Window) -> c_int;
    pub fn XUnmapWindow(dpy: *mut Display, w: Window) -> c_int;
    pub fn XMoveResizeWindow(
        dpy: *mut Display,
        w: Window,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
    ) -> c_int;
    pub fn XFlush(dpy: *mut Display) -> c_int;
    pub fn XSync(dpy: *mut Display, discard: Bool) -> c_int;
    pub fn XNextEvent(dpy: *mut Display, event: *mut XEvent) -> c_int;
    pub fn XPending(dpy: *mut Display) -> c_int;
    pub fn XConnectionNumber(dpy: *mut Display) -> c_int;
    pub fn XSelectInput(dpy: *mut Display, w: Window, mask: c_long) -> c_int;
    pub fn XInternAtom(dpy: *mut Display, name: *const c_char, only_if_exists: Bool) -> Atom;
    pub fn XSetWMProtocols(
        dpy: *mut Display,
        w: Window,
        protocols: *mut Atom,
        count: c_int,
    ) -> Status;
    pub fn XChangeProperty(
        dpy: *mut Display,
        w: Window,
        property: Atom,
        type_: Atom,
        format: c_int,
        mode: c_int,
        data: *const c_uchar,
        nelements: c_int,
    ) -> c_int;
    pub fn XStoreName(dpy: *mut Display, w: Window, name: *const c_char) -> c_int;
    pub fn XSetClassHint(dpy: *mut Display, w: Window, hint: *mut XClassHint) -> c_int;
    pub fn XSetInputFocus(dpy: *mut Display, w: Window, revert_to: c_int, time: Time) -> c_int;
    pub fn XRaiseWindow(dpy: *mut Display, w: Window) -> c_int;
    pub fn XLookupKeysym(event: *mut XKeyEvent, index: c_int) -> KeySym;
    pub fn XFilterEvent(event: *mut XEvent, w: Window) -> Bool;
    pub fn XMatchVisualInfo(
        dpy: *mut Display,
        screen: c_int,
        depth: c_int,
        class: c_int,
        vinfo_return: *mut XVisualInfo,
    ) -> Status;
    pub fn XCreateColormap(
        dpy: *mut Display,
        w: Window,
        visual: *mut Visual,
        alloc: c_int,
    ) -> Colormap;
    pub fn XFreeColormap(dpy: *mut Display, cmap: Colormap) -> c_int;
    pub fn XGetSelectionOwner(dpy: *mut Display, selection: Atom) -> Window;

    pub fn XOpenIM(
        dpy: *mut Display,
        db: XrmDatabase,
        res_name: *mut c_char,
        res_class: *mut c_char,
    ) -> XIM;
    pub fn XCloseIM(im: XIM) -> Status;
    // Variadic: called as XCreateIC(im, XNInputStyle, style, XNClientWindow, w, NULL).
    pub fn XCreateIC(im: XIM, ...) -> XIC;
    pub fn XDestroyIC(ic: XIC);
    pub fn XSetICFocus(ic: XIC);
    pub fn XSupportsLocale() -> Bool;
    pub fn XSetLocaleModifiers(modifier_list: *const c_char) -> *mut c_char;
    pub fn Xutf8LookupString(
        ic: XIC,
        event: *mut XKeyEvent,
        buffer: *mut c_char,
        bytes: c_int,
        keysym: *mut KeySym,
        status: *mut Status,
    ) -> c_int;
}

pub const XIMPreeditNothing: c_long = 0x0008;
pub const XIMStatusNothing: c_long = 0x0400;
pub const RevertToParent: c_int = 2;

// --- cairo ---------------------------------------------------------------
pub type cairo_surface_t = c_void;
pub type cairo_t = c_void;

#[link(name = "cairo")]
extern "C" {
    pub fn cairo_xlib_surface_create(
        dpy: *mut Display,
        drawable: Drawable,
        visual: *mut Visual,
        width: c_int,
        height: c_int,
    ) -> *mut cairo_surface_t;
    pub fn cairo_xlib_surface_set_size(s: *mut cairo_surface_t, width: c_int, height: c_int);
    pub fn cairo_surface_destroy(s: *mut cairo_surface_t);
    pub fn cairo_surface_flush(s: *mut cairo_surface_t);
    pub fn cairo_create(target: *mut cairo_surface_t) -> *mut cairo_t;
    pub fn cairo_destroy(cr: *mut cairo_t);
    pub fn cairo_set_source_rgb(cr: *mut cairo_t, r: f64, g: f64, b: f64);
    pub fn cairo_set_source_rgba(cr: *mut cairo_t, r: f64, g: f64, b: f64, a: f64);
    pub fn cairo_paint(cr: *mut cairo_t);
    pub fn cairo_fill(cr: *mut cairo_t);
    pub fn cairo_fill_preserve(cr: *mut cairo_t);
    pub fn cairo_stroke(cr: *mut cairo_t);
    pub fn cairo_set_line_width(cr: *mut cairo_t, w: f64);
    pub fn cairo_rectangle(cr: *mut cairo_t, x: f64, y: f64, w: f64, h: f64);
    pub fn cairo_move_to(cr: *mut cairo_t, x: f64, y: f64);
    pub fn cairo_line_to(cr: *mut cairo_t, x: f64, y: f64);
    pub fn cairo_close_path(cr: *mut cairo_t);
    pub fn cairo_new_sub_path(cr: *mut cairo_t);
    pub fn cairo_arc(cr: *mut cairo_t, xc: f64, yc: f64, r: f64, a1: f64, a2: f64);
    pub fn cairo_save(cr: *mut cairo_t);
    pub fn cairo_restore(cr: *mut cairo_t);
    pub fn cairo_clip(cr: *mut cairo_t);
    pub fn cairo_translate(cr: *mut cairo_t, x: f64, y: f64);
    pub fn cairo_set_operator(cr: *mut cairo_t, op: c_int);

    // Offscreen rendering. Lets the drawing code be tested pixel-by-pixel with
    // no X server involved, which is most of the UI.
    pub fn cairo_image_surface_create(
        format: c_int,
        width: c_int,
        height: c_int,
    ) -> *mut cairo_surface_t;
    pub fn cairo_image_surface_get_data(s: *mut cairo_surface_t) -> *mut c_uchar;
    pub fn cairo_image_surface_get_stride(s: *mut cairo_surface_t) -> c_int;
    pub fn cairo_image_surface_get_width(s: *mut cairo_surface_t) -> c_int;
    pub fn cairo_image_surface_get_height(s: *mut cairo_surface_t) -> c_int;
    pub fn cairo_surface_write_to_png(s: *mut cairo_surface_t, filename: *const c_char) -> c_int;
    pub fn cairo_surface_status(s: *mut cairo_surface_t) -> c_int;
}

pub const CAIRO_FORMAT_ARGB32: c_int = 0;
pub const CAIRO_STATUS_SUCCESS: c_int = 0;

pub const CAIRO_OPERATOR_CLEAR: c_int = 0;
pub const CAIRO_OPERATOR_SOURCE: c_int = 1;
pub const CAIRO_OPERATOR_OVER: c_int = 2;

// --- pango ---------------------------------------------------------------
pub type PangoLayout = c_void;
pub type PangoFontDescription = c_void;

pub const PANGO_SCALE: c_int = 1024;
pub const PANGO_ELLIPSIZE_NONE: c_int = 0;
pub const PANGO_ELLIPSIZE_END: c_int = 3;
pub const PANGO_WRAP_WORD: c_int = 0;
/// Wrap at word boundaries, but break a word that is longer than the line
/// rather than letting it overflow. A path with no spaces in it is exactly the
/// case that overflows.
pub const PANGO_WRAP_WORD_CHAR: c_int = 2;
pub const PANGO_ALIGN_LEFT: c_int = 0;
pub const PANGO_WEIGHT_NORMAL: c_int = 400;
pub const PANGO_WEIGHT_BOLD: c_int = 700;

#[link(name = "pangocairo-1.0")]
extern "C" {
    pub fn pango_cairo_create_layout(cr: *mut cairo_t) -> *mut PangoLayout;
    pub fn pango_cairo_show_layout(cr: *mut cairo_t, layout: *mut PangoLayout);
}

#[link(name = "pango-1.0")]
extern "C" {
    pub fn pango_layout_set_text(layout: *mut PangoLayout, text: *const c_char, len: c_int);
    pub fn pango_layout_set_font_description(
        layout: *mut PangoLayout,
        desc: *const PangoFontDescription,
    );
    pub fn pango_layout_set_width(layout: *mut PangoLayout, width: c_int);
    pub fn pango_layout_set_ellipsize(layout: *mut PangoLayout, mode: c_int);
    pub fn pango_layout_set_wrap(layout: *mut PangoLayout, mode: c_int);
    pub fn pango_layout_set_line_spacing(layout: *mut PangoLayout, factor: f32);
    pub fn pango_layout_get_line_count(layout: *mut PangoLayout) -> c_int;
    pub fn pango_layout_get_pixel_size(layout: *mut PangoLayout, w: *mut c_int, h: *mut c_int);
    pub fn pango_font_description_from_string(s: *const c_char) -> *mut PangoFontDescription;
    pub fn pango_font_description_free(desc: *mut PangoFontDescription);
}

#[link(name = "gobject-2.0")]
extern "C" {
    pub fn g_object_unref(obj: *mut c_void);
}

// --- libc: waiting on the X socket with a timeout -------------------------
// The event loop has to wake for its own reasons (a reply arriving from the
// daemon, a caret blink), not only when X has something to say, so it waits on
// the connection fd rather than blocking in XNextEvent.

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct pollfd {
    pub fd: c_int,
    pub events: i16,
    pub revents: i16,
}

pub const POLLIN: i16 = 0x001;

/// glibc `bits/locale.h`: `__LC_CTYPE` is 0.
pub const LC_CTYPE: c_int = 0;

extern "C" {
    pub fn poll(fds: *mut pollfd, nfds: c_ulong, timeout: c_int) -> c_int;
    pub fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
}
