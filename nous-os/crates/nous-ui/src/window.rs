//! A window, its X11 plumbing, and the event loop that drives it.
//!
//! Two kinds of window matter to the shell:
//!
//! * [`WindowKind::Overlay`] — the summon panel. Undecorated, always on top,
//!   centred, takes the keyboard immediately and dismisses on Escape or focus
//!   loss. This is what replaces the browser window.
//! * [`WindowKind::Normal`] — a regular managed window for the longer-lived
//!   surfaces (files, journal), with a title bar the window manager draws.
//!
//! Text input goes through XIM so that dead keys, compose sequences and input
//! methods for non-Latin scripts work. Reading `XKeyEvent.keycode` directly
//! would be shorter and would quietly break for anyone not typing ASCII.

use crate::draw::Canvas;
use crate::ffi::*;
use std::collections::VecDeque;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_uchar, c_ulong};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    /// Undecorated, above everything, self-focusing.
    Overlay,
    /// Managed by the window manager like any other application window.
    Normal,
}

/// A key press, after the input method has had its say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub sym: u64,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Key {
    pub fn is(&self, sym: u64) -> bool {
        self.sym == sym
    }

    /// True for a plain press with no modifier that would change its meaning.
    pub fn bare(&self) -> bool {
        !self.ctrl && !self.alt
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The window must repaint. Coalesced: several X Exposes become one.
    Redraw,
    Resized {
        w: f64,
        h: f64,
    },
    Key(Key),
    /// Committed text from the input method. May be several characters at once.
    Text(String),
    MouseDown {
        x: f64,
        y: f64,
        button: u32,
        /// Held modifiers. Carried because a click means different things with
        /// them: ctrl adds to a selection, shift takes everything in between.
        /// Dropping them here is why a file manager can only ever act on one
        /// file at a time.
        ctrl: bool,
        shift: bool,
    },
    MouseUp {
        x: f64,
        y: f64,
        button: u32,
    },
    MouseMove {
        x: f64,
        y: f64,
    },
    FocusLost,
    /// The timeout passed to [`Window::wait`] elapsed with nothing to report.
    Tick,
    /// The window manager asked the window to close, or the display went away.
    Close,
}

struct Atoms {
    wm_delete_window: Atom,
    wm_protocols: Atom,
    net_wm_window_type: Atom,
    net_wm_window_type_dialog: Atom,
    net_wm_window_type_utility: Atom,
    net_wm_state: Atom,
    net_wm_state_above: Atom,
    net_wm_state_skip_taskbar: Atom,
    net_wm_name: Atom,
    utf8_string: Atom,
}

pub struct Window {
    dpy: *mut Display,
    win: crate::ffi::Window,
    surface: *mut cairo_surface_t,
    /// Where frames are actually drawn, before being put on screen in one go.
    ///
    /// Painting straight onto the window means the clear that starts a frame
    /// is visible: the X server can show the emptied window before anything
    /// has been drawn back into it, which is seen as the whole interface
    /// flickering under the pointer. Drawing offscreen and blitting once
    /// makes a frame atomic.
    back: *mut cairo_surface_t,
    back_w: i32,
    back_h: i32,
    colormap: Colormap,
    im: XIM,
    ic: XIC,
    atoms: Atoms,
    kind: WindowKind,
    /// True when we own a 32-bit visual *and* a compositor is running, so the
    /// window can genuinely be transparent instead of showing black.
    argb: bool,
    width: i32,
    height: i32,
    /// Events translated but not yet handed to the caller. One X event can
    /// produce two (a key press that also commits text), and a burst of
    /// Exposes produces one.
    queue: VecDeque<Event>,
}

fn atom(dpy: *mut Display, name: &str) -> Atom {
    let c = match CString::new(name) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    unsafe { XInternAtom(dpy, c.as_ptr(), 0) }
}

impl Window {
    /// Open a window of `w`x`h` logical pixels.
    ///
    /// Returns `Err` when there is no usable display — no `DISPLAY`, a refused
    /// connection, or a server that will not create the window. Callers are
    /// expected to fall back to the terminal interface rather than abort.
    pub fn open(title: &str, w: i32, h: i32, kind: WindowKind) -> Result<Window, String> {
        unsafe { Window::open_inner(title, w, h, kind) }
    }

    unsafe fn open_inner(title: &str, w: i32, h: i32, kind: WindowKind) -> Result<Window, String> {
        let dpy = XOpenDisplay(std::ptr::null());
        if dpy.is_null() {
            return Err("no X display (is DISPLAY set?)".into());
        }
        let screen = XDefaultScreen(dpy);
        let root = XRootWindow(dpy, screen);

        // Prefer a 32-bit visual so the panel can have soft edges and a
        // translucent backdrop. Without a compositor that visual paints the
        // uncovered parts black, which looks broken, so check for one first.
        let compositing = {
            let sel = atom(dpy, &format!("_NET_WM_CM_S{}", screen));
            sel != 0 && XGetSelectionOwner(dpy, sel) != None_
        };
        let mut vinfo = XVisualInfo::default();
        let argb = compositing
            && XMatchVisualInfo(dpy, screen, 32, TrueColor, &mut vinfo) != 0
            && !vinfo.visual.is_null();

        let (visual, depth) = if argb {
            (vinfo.visual, vinfo.depth)
        } else {
            (XDefaultVisual(dpy, screen), XDefaultDepth(dpy, screen))
        };
        let colormap = if argb {
            XCreateColormap(dpy, root, visual, AllocNone)
        } else {
            XDefaultColormap(dpy, screen)
        };

        // Centre on the screen. An overlay must be placed by us because
        // override-redirect windows are invisible to the window manager's
        // placement policy.
        let sw = XDisplayWidth(dpy, screen);
        let sh = XDisplayHeight(dpy, screen);
        let x = ((sw - w) / 2).max(0);
        // Slightly above centre reads better for a summon panel; the eye
        // expects it where a menu would drop, not in the dead middle.
        let y = if kind == WindowKind::Overlay {
            ((sh - h) / 3).max(0)
        } else {
            ((sh - h) / 2).max(0)
        };

        let mut attrs = XSetWindowAttributes {
            background_pixel: 0,
            border_pixel: 0,
            colormap,
            override_redirect: i32::from(kind == WindowKind::Overlay),
            event_mask: KeyPressMask
                | KeyReleaseMask
                | ButtonPressMask
                | ButtonReleaseMask
                | PointerMotionMask
                | ExposureMask
                | StructureNotifyMask
                | FocusChangeMask
                | PropertyChangeMask,
            ..Default::default()
        };
        let mask = CWBackPixel | CWBorderPixel | CWColormap | CWEventMask | CWOverrideRedirect;

        let win = XCreateWindow(
            dpy,
            root,
            x,
            y,
            w as u32,
            h as u32,
            0,
            depth,
            InputOutput,
            visual,
            mask,
            &mut attrs,
        );
        if win == 0 {
            XCloseDisplay(dpy);
            return Err("X server refused to create the window".into());
        }

        let atoms = Atoms {
            wm_delete_window: atom(dpy, "WM_DELETE_WINDOW"),
            wm_protocols: atom(dpy, "WM_PROTOCOLS"),
            net_wm_window_type: atom(dpy, "_NET_WM_WINDOW_TYPE"),
            net_wm_window_type_dialog: atom(dpy, "_NET_WM_WINDOW_TYPE_DIALOG"),
            net_wm_window_type_utility: atom(dpy, "_NET_WM_WINDOW_TYPE_UTILITY"),
            net_wm_state: atom(dpy, "_NET_WM_STATE"),
            net_wm_state_above: atom(dpy, "_NET_WM_STATE_ABOVE"),
            net_wm_state_skip_taskbar: atom(dpy, "_NET_WM_STATE_SKIP_TASKBAR"),
            net_wm_name: atom(dpy, "_NET_WM_NAME"),
            utf8_string: atom(dpy, "UTF8_STRING"),
        };

        let mut protocols = [atoms.wm_delete_window];
        XSetWMProtocols(dpy, win, protocols.as_mut_ptr(), 1);

        // _NET_WM_NAME is the UTF-8 title modern window managers read;
        // XStoreName is the Latin-1 fallback for the ones that don't.
        XChangeProperty(
            dpy,
            win,
            atoms.net_wm_name,
            atoms.utf8_string,
            8,
            PropModeReplace,
            title.as_ptr(),
            title.len() as c_int,
        );
        if let Ok(c) = CString::new(title) {
            XStoreName(dpy, win, c.as_ptr());
        }

        // The WM_CLASS is how the desktop matches a window to its .desktop
        // entry: get it wrong and the panel shows a generic icon.
        let res_name = CString::new("nous").unwrap();
        let res_class = CString::new("Nous").unwrap();
        let mut hint = XClassHint {
            res_name: res_name.as_ptr() as *mut c_char,
            res_class: res_class.as_ptr() as *mut c_char,
        };
        XSetClassHint(dpy, win, &mut hint);

        let wtype = if kind == WindowKind::Overlay {
            atoms.net_wm_window_type_utility
        } else {
            atoms.net_wm_window_type_dialog
        };
        XChangeProperty(
            dpy,
            win,
            atoms.net_wm_window_type,
            XA_ATOM,
            32,
            PropModeReplace,
            &wtype as *const Atom as *const c_uchar,
            1,
        );
        if kind == WindowKind::Overlay {
            let states = [atoms.net_wm_state_above, atoms.net_wm_state_skip_taskbar];
            XChangeProperty(
                dpy,
                win,
                atoms.net_wm_state,
                XA_ATOM,
                32,
                PropModeReplace,
                states.as_ptr() as *const c_uchar,
                2,
            );
        }

        let surface = cairo_xlib_surface_create(dpy, win, visual, w, h);
        // The offscreen surface is made on the first frame, when the window's
        // real size is known — a window manager may have resized it already.
        let back: *mut cairo_surface_t = std::ptr::null_mut();
        if surface.is_null() {
            XDestroyWindow(dpy, win);
            XCloseDisplay(dpy);
            return Err("cairo could not wrap the window".into());
        }

        // Rust never calls setlocale, so without this the process is in the C
        // locale and XOpenIM refuses. The empty string means "whatever the
        // environment says", and the empty modifier list means "whatever
        // XMODIFIERS says" — both are the user's own configuration.
        let empty = CString::new("").unwrap();
        setlocale(LC_CTYPE, empty.as_ptr());
        XSetLocaleModifiers(empty.as_ptr());
        let im = if XSupportsLocale() == 0 {
            std::ptr::null_mut()
        } else {
            XOpenIM(
                dpy,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        let ic = if im.is_null() {
            std::ptr::null_mut()
        } else {
            let style = CString::new("inputStyle").unwrap();
            let client = CString::new("clientWindow").unwrap();
            let focus = CString::new("focusWindow").unwrap();
            XCreateIC(
                im,
                style.as_ptr(),
                XIMPreeditNothing | XIMStatusNothing,
                client.as_ptr(),
                win,
                focus.as_ptr(),
                win,
                std::ptr::null::<c_char>(),
            )
        };
        if !ic.is_null() {
            XSetICFocus(ic);
        }

        XMapRaised(dpy, win);
        if kind == WindowKind::Overlay {
            // Override-redirect windows get no focus from the window manager,
            // so take it. XSync first: focusing a window the server has not
            // finished mapping is a BadMatch.
            XSync(dpy, 0);
            XSetInputFocus(dpy, win, RevertToParent, CurrentTime);
        }
        XFlush(dpy);

        Ok(Window {
            dpy,
            win,
            surface,
            back,
            back_w: 0,
            back_h: 0,
            colormap: if argb { colormap } else { 0 },
            im,
            ic,
            atoms,
            kind,
            argb,
            width: w,
            height: h,
            queue: VecDeque::new(),
        })
    }

    pub fn size(&self) -> (f64, f64) {
        (self.width as f64, self.height as f64)
    }

    /// True when the window really has an alpha channel. Callers use this to
    /// decide between a translucent backdrop and an opaque one.
    pub fn translucent(&self) -> bool {
        self.argb
    }

    pub fn kind(&self) -> WindowKind {
        self.kind
    }

    /// Paint one frame. The closure gets a [`Canvas`] already cleared to
    /// transparent (or to `opaque` when there is no compositor).
    /// Make sure the offscreen surface matches the window's size.
    ///
    /// Recreated only when the size actually changes, so a stream of motion
    /// events costs nothing.
    fn ensure_back(&mut self) {
        if !self.back.is_null() && self.back_w == self.width && self.back_h == self.height {
            return;
        }
        unsafe {
            if !self.back.is_null() {
                cairo_surface_destroy(self.back);
            }
            self.back = cairo_image_surface_create(
                CAIRO_FORMAT_ARGB32,
                self.width.max(1),
                self.height.max(1),
            );
        }
        self.back_w = self.width;
        self.back_h = self.height;
    }

    pub fn draw<F: FnOnce(&Canvas)>(&mut self, opaque: crate::draw::Rgba, f: F) {
        self.ensure_back();
        if self.back.is_null() {
            return;
        }
        unsafe {
            // The frame is built offscreen, where the clear that starts it
            // cannot be seen.
            let cr = cairo_create(self.back);
            if cr.is_null() {
                return;
            }
            // Clearing with SOURCE writes the alpha channel rather than
            // blending onto whatever the last frame left behind; with OVER a
            // transparent clear is a no-op and old frames smear.
            cairo_set_operator(cr, CAIRO_OPERATOR_SOURCE);
            if self.argb {
                cairo_set_source_rgba(cr, 0.0, 0.0, 0.0, 0.0);
            } else {
                cairo_set_source_rgba(cr, opaque.0, opaque.1, opaque.2, 1.0);
            }
            cairo_paint(cr);
            cairo_set_operator(cr, CAIRO_OPERATOR_OVER);

            let canvas = Canvas::from_raw(cr);
            f(&canvas);
            cairo_destroy(cr);
            cairo_surface_flush(self.back);

            // And put on screen in one operation, so what the server shows is
            // either the last frame or this one and never the gap between.
            let front = cairo_create(self.surface);
            if front.is_null() {
                return;
            }
            cairo_set_operator(front, CAIRO_OPERATOR_SOURCE);
            cairo_set_source_surface(front, self.back, 0.0, 0.0);
            cairo_paint(front);
            cairo_destroy(front);
            cairo_surface_flush(self.surface);
            XFlush(self.dpy);
        }
    }

    /// Wait up to `timeout_ms` for something to happen.
    ///
    /// Returns [`Event::Tick`] when the timeout elapses. A negative timeout
    /// blocks until there is an event. Consecutive Exposes collapse into a
    /// single [`Event::Redraw`], and a resize that does not change the size
    /// reports nothing, so a drag-resize does not queue up hundreds of frames.
    pub fn wait(&mut self, timeout_ms: i32) -> Event {
        unsafe { self.wait_inner(timeout_ms) }
    }

    unsafe fn wait_inner(&mut self, timeout_ms: i32) -> Event {
        if let Some(e) = self.queue.pop_front() {
            return e;
        }
        if XPending(self.dpy) == 0 {
            let mut fds = pollfd {
                fd: XConnectionNumber(self.dpy),
                events: POLLIN,
                revents: 0,
            };
            // A signal interrupting the wait is indistinguishable here from a
            // timeout, and both mean the same thing to the caller.
            if poll(&mut fds, 1, timeout_ms) <= 0 || XPending(self.dpy) == 0 {
                return Event::Tick;
            }
        }

        // Drain everything the server has queued in one go. Translating the
        // whole burst before returning is what lets Exposes coalesce.
        while XPending(self.dpy) > 0 {
            let mut ev = XEvent::default();
            XNextEvent(self.dpy, &mut ev);
            // The input method consumes the key events that make up a compose
            // sequence; handing those to the app would type the raw keys.
            if XFilterEvent(&mut ev, self.win) != 0 {
                continue;
            }
            self.translate(&mut ev);
        }
        self.queue.pop_front().unwrap_or(Event::Tick)
    }

    // X11 event and keysym names are lower-cased constants. Renaming them to
    // satisfy Rust's convention would break the one property that makes these
    // bindings checkable: that they read the same as the header.
    #[allow(non_upper_case_globals)]
    unsafe fn translate(&mut self, ev: &mut XEvent) {
        match ev.type_ {
            Expose => {
                // Every Expose in a burst describes part of the same repaint,
                // and we always redraw the whole window, so one is enough.
                if !self.queue.iter().any(|e| *e == Event::Redraw) {
                    self.queue.push_back(Event::Redraw);
                }
            }
            ConfigureNotify => {
                let c = &*(ev as *const XEvent as *const XConfigureEvent);
                if c.width != self.width || c.height != self.height {
                    self.width = c.width;
                    self.height = c.height;
                    cairo_xlib_surface_set_size(self.surface, c.width, c.height);
                    self.queue.push_back(Event::Resized {
                        w: c.width as f64,
                        h: c.height as f64,
                    });
                }
            }
            KeyPress => {
                let key = &mut *(ev as *mut XEvent as *mut XKeyEvent);
                let state = key.state;
                let (sym, text) = self.lookup(key);
                let k = Key {
                    sym,
                    ctrl: state & ControlMask != 0,
                    shift: state & ShiftMask != 0,
                    alt: state & Mod1Mask != 0,
                };
                // The keysym drives navigation and shortcuts, so it is
                // delivered first; the text it also committed follows.
                self.queue.push_back(Event::Key(k));
                if !text.is_empty() && k.bare() && !is_control_sym(sym) {
                    self.queue.push_back(Event::Text(text));
                }
            }
            ButtonPress => {
                let b = &*(ev as *const XEvent as *const XButtonEvent);
                self.queue.push_back(Event::MouseDown {
                    x: b.x as f64,
                    y: b.y as f64,
                    button: b.button,
                    ctrl: b.state & ControlMask != 0,
                    shift: b.state & ShiftMask != 0,
                });
            }
            ButtonRelease => {
                let b = &*(ev as *const XEvent as *const XButtonEvent);
                self.queue.push_back(Event::MouseUp {
                    x: b.x as f64,
                    y: b.y as f64,
                    button: b.button,
                });
            }
            MotionNotify => {
                let b = &*(ev as *const XEvent as *const XButtonEvent);
                // Only the newest pointer position is interesting; older ones
                // describe where the cursor already isn't.
                self.queue.retain(|e| !matches!(e, Event::MouseMove { .. }));
                self.queue.push_back(Event::MouseMove {
                    x: b.x as f64,
                    y: b.y as f64,
                });
            }
            FocusOut => {
                let f = &*(ev as *const XEvent as *const XFocusChangeEvent);
                if is_real_focus_loss(f.mode, f.detail) {
                    self.queue.push_back(Event::FocusLost);
                }
            }
            ClientMessage => {
                let m = &*(ev as *const XEvent as *const XClientMessageEvent);
                if m.message_type == self.atoms.wm_protocols
                    && m.data_l[0] as Atom == self.atoms.wm_delete_window
                {
                    self.queue.push_back(Event::Close);
                }
            }
            _ => {}
        }
    }

    /// Turn a key press into `(keysym, committed text)` via the input method.
    unsafe fn lookup(&self, key: &mut XKeyEvent) -> (u64, String) {
        let mut sym: KeySym = 0;
        let mut status: Status = 0;
        let mut buf = [0u8; 64];

        let n = if self.ic.is_null() {
            // No input method: a keysym is still better than nothing, and
            // ASCII typing works.
            sym = XLookupKeysym(key, if key.state & ShiftMask != 0 { 1 } else { 0 });
            0
        } else {
            Xutf8LookupString(
                self.ic,
                key,
                buf.as_mut_ptr() as *mut c_char,
                buf.len() as c_int,
                &mut sym,
                &mut status,
            )
        };

        if status == XBufferOverflow {
            // Longer than 64 bytes of committed text means an input method
            // flushed a whole phrase. Ask again with room for it.
            let mut big = vec![0u8; n.max(0) as usize + 1];
            let n2 = Xutf8LookupString(
                self.ic,
                key,
                big.as_mut_ptr() as *mut c_char,
                (big.len() - 1) as c_int,
                &mut sym,
                &mut status,
            );
            let text = String::from_utf8_lossy(&big[..n2.max(0) as usize]).into_owned();
            return (sym as u64, text);
        }

        let text = if n > 0 {
            String::from_utf8_lossy(&buf[..n as usize]).into_owned()
        } else {
            String::new()
        };
        (sym as u64, text)
    }

    pub fn resize(&mut self, w: i32, h: i32) {
        if w == self.width && h == self.height {
            return;
        }
        unsafe {
            let screen = XDefaultScreen(self.dpy);
            let sw = XDisplayWidth(self.dpy, screen);
            let sh = XDisplayHeight(self.dpy, screen);
            let x = ((sw - w) / 2).max(0);
            let y = if self.kind == WindowKind::Overlay {
                ((sh - h) / 3).max(0)
            } else {
                ((sh - h) / 2).max(0)
            };
            XMoveResizeWindow(self.dpy, self.win, x, y, w as u32, h as u32);
            cairo_xlib_surface_set_size(self.surface, w, h);
        }
        self.width = w;
        self.height = h;
    }

    /// Re-assert focus. An overlay loses it whenever another window is raised.
    pub fn focus(&self) {
        unsafe {
            XRaiseWindow(self.dpy, self.win);
            XSetInputFocus(self.dpy, self.win, RevertToParent, CurrentTime);
            XFlush(self.dpy);
        }
    }

    pub fn hide(&self) {
        unsafe {
            XUnmapWindow(self.dpy, self.win);
            XFlush(self.dpy);
        }
    }

    pub fn show(&self) {
        unsafe {
            XMapRaised(self.dpy, self.win);
            if self.kind == WindowKind::Overlay {
                XSync(self.dpy, 0);
                XSetInputFocus(self.dpy, self.win, RevertToParent, CurrentTime);
            }
            XFlush(self.dpy);
        }
    }

    /// Block until every request so far has been processed. Tests need this
    /// before they can screenshot.
    pub fn sync(&self) {
        unsafe { XSync(self.dpy, 0) };
    }

    /// Write what is currently on the window to a PNG.
    ///
    /// Reads back from the X server, so this is the window as the screen has
    /// it, not a re-render. That is the point: it is how a rendering fault that
    /// only happens on a real display gets caught, and how a user can send a
    /// picture of what went wrong.
    pub fn capture(&self, path: &str) -> Result<(), String> {
        let c = CString::new(path).map_err(|_| "path contains a NUL".to_string())?;
        unsafe {
            XSync(self.dpy, 0);
            cairo_surface_flush(self.surface);
            if cairo_surface_write_to_png(self.surface, c.as_ptr()) != CAIRO_STATUS_SUCCESS {
                return Err(format!("could not write {path}"));
            }
        }
        Ok(())
    }

    pub fn id(&self) -> c_ulong {
        self.win
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            if !self.ic.is_null() {
                XDestroyIC(self.ic);
            }
            if !self.im.is_null() {
                XCloseIM(self.im);
            }
            if !self.back.is_null() {
                cairo_surface_destroy(self.back);
            }
            cairo_surface_destroy(self.surface);
            XDestroyWindow(self.dpy, self.win);
            if self.colormap != 0 {
                XFreeColormap(self.dpy, self.colormap);
            }
            XCloseDisplay(self.dpy);
        }
    }
}

/// Did the window genuinely lose the keyboard, or is this one of the FocusOut
/// events X sends for reasons that have nothing to do with focus?
///
/// X reports a focus change whenever a grab activates or releases, and the
/// pointer entering or leaving a child window produces one too. Treating those
/// as "the user went somewhere else" means opening any menu, pressing Alt-Tab,
/// running a screenshot tool, or another application's global hotkey firing all
/// look identical to a deliberate click on another window.
#[allow(non_upper_case_globals)]
fn is_real_focus_loss(mode: i32, detail: i32) -> bool {
    // NotifyGrab / NotifyUngrab bracket a grab; the keyboard comes straight
    // back afterwards. NotifyWhileGrabbed happens with a grab already active.
    if mode != NotifyNormal {
        return false;
    }
    // Pointer-driven events describe where the mouse is, not where the keyboard
    // went. NotifyInferior means focus moved to a child of this window, which
    // for our purposes is still us.
    !matches!(detail, NotifyPointer | NotifyPointerRoot | NotifyInferior)
}

/// Keysyms that move the caret or edit the buffer rather than insert a
/// character. XIM reports a printable string for some of these (Return gives
/// "\r", Escape gives "\x1b") and inserting that would corrupt the text.
#[allow(non_upper_case_globals)]
fn is_control_sym(sym: u64) -> bool {
    matches!(
        sym,
        XK_Escape
            | XK_Return
            | XK_KP_Enter
            | XK_BackSpace
            | XK_Delete
            | XK_Tab
            | XK_Up
            | XK_Down
            | XK_Left
            | XK_Right
            | XK_Home
            | XK_End
            | XK_Page_Up
            | XK_Page_Down
    )
}

const _: () = {
    // c_long is what XEvent's padding is measured in; the union must be at
    // least as large as the largest member or X writes past the end of it.
    assert!(std::mem::size_of::<XEvent>() >= std::mem::size_of::<XKeyEvent>());
    assert!(std::mem::size_of::<XEvent>() >= std::mem::size_of::<XClientMessageEvent>());
    assert!(std::mem::size_of::<XEvent>() >= std::mem::size_of::<XConfigureEvent>());
    assert!(std::mem::size_of::<XEvent>() >= std::mem::size_of::<XButtonEvent>());
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_keys_never_insert_text() {
        assert!(is_control_sym(XK_Return));
        assert!(is_control_sym(XK_Escape));
        assert!(is_control_sym(XK_BackSpace));
        assert!(is_control_sym(XK_Left));
        // A printable key must not be treated as control, or nothing types.
        assert!(!is_control_sym(0x061)); // 'a'
        assert!(!is_control_sym(0x020)); // space
    }

    #[test]
    fn a_grab_is_not_a_focus_change() {
        // Opening a menu, Alt-Tab, a screenshot tool or another app's global
        // hotkey all produce a grab. Acting on these dismissed the panel while
        // a request was still running.
        assert!(!is_real_focus_loss(NotifyGrab, NotifyNonlinear));
        assert!(!is_real_focus_loss(NotifyUngrab, NotifyNonlinear));
        assert!(!is_real_focus_loss(NotifyWhileGrabbed, NotifyNonlinear));
        // Pointer motion is not the keyboard going anywhere.
        assert!(!is_real_focus_loss(NotifyNormal, NotifyPointer));
        assert!(!is_real_focus_loss(NotifyNormal, NotifyPointerRoot));
        // Focus moving to a child of our own window is still our window.
        assert!(!is_real_focus_loss(NotifyNormal, NotifyInferior));

        // Clicking another application really is leaving.
        assert!(is_real_focus_loss(NotifyNormal, NotifyNonlinear));
        assert!(is_real_focus_loss(NotifyNormal, NotifyNonlinearVirtual));
        assert!(is_real_focus_loss(NotifyNormal, NotifyAncestor));
    }

    #[test]
    fn modifier_state_decodes_independently() {
        let plain = Key {
            sym: 0x61,
            ctrl: false,
            shift: false,
            alt: false,
        };
        assert!(plain.bare());
        let with_shift = Key {
            shift: true,
            ..plain
        };
        assert!(with_shift.bare(), "shift alone still types a character");
        let with_ctrl = Key {
            ctrl: true,
            ..plain
        };
        assert!(!with_ctrl.bare());
        let with_alt = Key { alt: true, ..plain };
        assert!(!with_alt.bare());
    }
}
