//! Drawing: colours, text and the handful of shapes the shell is built from.
//!
//! A thin safe layer over Cairo and Pango. Everything the UI draws goes through
//! here, so there is exactly one place that owns a raw pointer's lifetime.

use crate::ffi::*;
use std::ffi::CString;

/// sRGB, 0.0 to 1.0, with alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba(pub f64, pub f64, pub f64, pub f64);

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, 1.0)
    }

    // clippy objects that `rgba` matches the type name, on the grounds that
    // `Type::type()` reads as a conversion rather than a constructor. It does
    // not read that way here: `rgb` and `rgba` are how colours are written
    // everywhere from CSS to Cairo, and the pair is what makes the alpha
    // argument obvious at the call site. Renaming one would break the pair.
    #[allow(clippy::self_named_constructors)]
    pub const fn rgba(r: u8, g: u8, b: u8, a: f64) -> Rgba {
        Rgba(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, a)
    }

    pub fn with_alpha(self, a: f64) -> Rgba {
        Rgba(self.0, self.1, self.2, a)
    }

    /// Blend towards another colour. `t` of 0 is self, 1 is other.
    pub fn mix(self, other: Rgba, t: f64) -> Rgba {
        let t = t.clamp(0.0, 1.0);
        Rgba(
            self.0 + (other.0 - self.0) * t,
            self.1 + (other.1 - self.1) * t,
            self.2 + (other.2 - self.2) * t,
            self.3 + (other.3 - self.3) * t,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    pub fn inset(&self, d: f64) -> Rect {
        Rect::new(
            self.x + d,
            self.y + d,
            (self.w - d * 2.0).max(0.0),
            (self.h - d * 2.0).max(0.0),
        )
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
}

/// Whether text is kept to one line or allowed to wrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    OneLine,
    Wrap,
}

/// A drawing context for one frame.
pub struct Canvas {
    pub cr: *mut cairo_t,
}

impl Canvas {
    /// # Safety
    /// `cr` must be a live cairo context for the duration of this Canvas.
    pub unsafe fn from_raw(cr: *mut cairo_t) -> Canvas {
        Canvas { cr }
    }

    pub fn set_color(&self, c: Rgba) {
        unsafe { cairo_set_source_rgba(self.cr, c.0, c.1, c.2, c.3) }
    }

    pub fn fill_rect(&self, r: Rect, c: Rgba) {
        unsafe {
            self.set_color(c);
            cairo_rectangle(self.cr, r.x, r.y, r.w, r.h);
            cairo_fill(self.cr);
        }
    }

    /// A rounded rectangle, built from four arcs. Used for every surface in the
    /// shell, so it is worth having exactly once.
    pub fn rounded_path(&self, r: Rect, radius: f64) {
        let rad = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
        let (x, y, w, h) = (r.x, r.y, r.w, r.h);
        let pi = std::f64::consts::PI;
        unsafe {
            cairo_new_sub_path(self.cr);
            cairo_arc(self.cr, x + w - rad, y + rad, rad, -pi / 2.0, 0.0);
            cairo_arc(self.cr, x + w - rad, y + h - rad, rad, 0.0, pi / 2.0);
            cairo_arc(self.cr, x + rad, y + h - rad, rad, pi / 2.0, pi);
            cairo_arc(self.cr, x + rad, y + rad, rad, pi, 1.5 * pi);
            cairo_close_path(self.cr);
        }
    }

    pub fn fill_rounded(&self, r: Rect, radius: f64, c: Rgba) {
        self.set_color(c);
        self.rounded_path(r, radius);
        unsafe { cairo_fill(self.cr) }
    }

    pub fn stroke_rounded(&self, r: Rect, radius: f64, width: f64, c: Rgba) {
        self.set_color(c);
        self.rounded_path(r, radius);
        unsafe {
            cairo_set_line_width(self.cr, width);
            cairo_stroke(self.cr);
        }
    }

    pub fn fill_circle(&self, cx: f64, cy: f64, r: f64, c: Rgba) {
        unsafe {
            self.set_color(c);
            cairo_new_sub_path(self.cr);
            cairo_arc(self.cr, cx, cy, r, 0.0, std::f64::consts::PI * 2.0);
            cairo_fill(self.cr);
        }
    }

    pub fn line(&self, x1: f64, y1: f64, x2: f64, y2: f64, width: f64, c: Rgba) {
        unsafe {
            self.set_color(c);
            cairo_set_line_width(self.cr, width);
            cairo_move_to(self.cr, x1, y1);
            cairo_line_to(self.cr, x2, y2);
            cairo_stroke(self.cr);
        }
    }

    pub fn clip_rect(&self, r: Rect) {
        unsafe {
            cairo_save(self.cr);
            cairo_rectangle(self.cr, r.x, r.y, r.w, r.h);
            cairo_clip(self.cr);
        }
    }

    pub fn restore(&self) {
        unsafe { cairo_restore(self.cr) }
    }

    /// Lay out text and return its pixel size without drawing it.
    ///
    /// With a `max_width`, the text is kept to one line and ellipsized. Prose
    /// that should wrap goes through [`Canvas::measure_wrapped`] instead —
    /// measuring one way and drawing the other is how a body ends up clipped.
    pub fn measure(&self, text: &str, font: &Font, max_width: Option<f64>) -> (f64, f64) {
        self.sized(self.layout_for(text, font, max_width, Flow::OneLine))
    }

    pub fn measure_wrapped(&self, text: &str, font: &Font, width: f64) -> (f64, f64) {
        self.sized(self.layout_for(text, font, Some(width), Flow::Wrap))
    }

    /// Draw one line of text at `(x, y)` (top-left), ellipsized to `max_width`.
    /// Returns the height drawn.
    pub fn text(
        &self,
        text: &str,
        x: f64,
        y: f64,
        font: &Font,
        c: Rgba,
        max_width: Option<f64>,
    ) -> f64 {
        self.show(
            self.layout_for(text, font, max_width, Flow::OneLine),
            x,
            y,
            c,
        )
    }

    /// Draw text wrapped to `width`. Returns the height drawn, which is what
    /// the caller must reserve for it.
    pub fn text_wrapped(
        &self,
        text: &str,
        x: f64,
        y: f64,
        font: &Font,
        c: Rgba,
        width: f64,
    ) -> f64 {
        self.show(
            self.layout_for(text, font, Some(width), Flow::Wrap),
            x,
            y,
            c,
        )
    }

    fn sized(&self, layout: *mut PangoLayout) -> (f64, f64) {
        let (mut w, mut h) = (0, 0);
        unsafe {
            pango_layout_get_pixel_size(layout, &mut w, &mut h);
            g_object_unref(layout as *mut _);
        }
        (w as f64, h as f64)
    }

    fn show(&self, layout: *mut PangoLayout, x: f64, y: f64, c: Rgba) -> f64 {
        let (mut w, mut h) = (0, 0);
        unsafe {
            pango_layout_get_pixel_size(layout, &mut w, &mut h);
            self.set_color(c);
            cairo_move_to(self.cr, x, y);
            pango_cairo_show_layout(self.cr, layout);
            g_object_unref(layout as *mut _);
        }
        let _ = w;
        h as f64
    }

    fn layout_for(
        &self,
        text: &str,
        font: &Font,
        max_width: Option<f64>,
        flow: Flow,
    ) -> *mut PangoLayout {
        unsafe {
            let layout = pango_cairo_create_layout(self.cr);
            let desc_str = CString::new(font.describe()).unwrap_or_default();
            let desc = pango_font_description_from_string(desc_str.as_ptr());
            pango_layout_set_font_description(layout, desc);
            pango_font_description_free(desc);

            // Pango takes a byte length, so a CString is unnecessary and would
            // break on text containing a NUL.
            pango_layout_set_text(layout, text.as_ptr() as *const _, text.len() as i32);
            if let Some(w) = max_width {
                pango_layout_set_width(layout, (w * PANGO_SCALE as f64) as i32);
                match flow {
                    Flow::OneLine => pango_layout_set_ellipsize(layout, PANGO_ELLIPSIZE_END),
                    Flow::Wrap => {
                        pango_layout_set_ellipsize(layout, PANGO_ELLIPSIZE_NONE);
                        pango_layout_set_wrap(layout, PANGO_WRAP_WORD_CHAR);
                        // Prose at 12pt is hard to read set solid. A quarter of
                        // a line between them is the usual remedy.
                        pango_layout_set_line_spacing(layout, 1.25);
                    }
                }
            }
            layout
        }
    }
}

/// An offscreen ARGB32 surface.
///
/// The same [`Canvas`] draws to this and to a window, so a test can render a
/// real frame and read the pixels back without an X server. Every visual claim
/// in the test suite is checked this way rather than by eye.
pub struct Image {
    surface: *mut cairo_surface_t,
    cr: *mut cairo_t,
    pub width: i32,
    pub height: i32,
}

impl Image {
    pub fn new(width: i32, height: i32) -> Result<Image, String> {
        unsafe {
            let surface = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width, height);
            if surface.is_null() || cairo_surface_status(surface) != CAIRO_STATUS_SUCCESS {
                if !surface.is_null() {
                    cairo_surface_destroy(surface);
                }
                return Err(format!("cairo refused a {width}x{height} surface"));
            }
            let cr = cairo_create(surface);
            if cr.is_null() {
                cairo_surface_destroy(surface);
                return Err("cairo could not make a context".into());
            }
            Ok(Image {
                surface,
                cr,
                width,
                height,
            })
        }
    }

    pub fn canvas(&self) -> Canvas {
        // SAFETY: the context lives as long as this Image, and Canvas does not
        // free it.
        unsafe { Canvas::from_raw(self.cr) }
    }

    /// The pixel at `(x, y)` as `(r, g, b, a)`, 0-255.
    ///
    /// ARGB32 is stored premultiplied in native byte order, so on a
    /// little-endian machine the bytes are B, G, R, A. The premultiplication is
    /// undone here so callers compare against the colour they asked for.
    pub fn pixel(&self, x: i32, y: i32) -> (u8, u8, u8, u8) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return (0, 0, 0, 0);
        }
        unsafe {
            cairo_surface_flush(self.surface);
            let data = cairo_image_surface_get_data(self.surface);
            if data.is_null() {
                return (0, 0, 0, 0);
            }
            let stride = cairo_image_surface_get_stride(self.surface) as isize;
            let px = data.offset(y as isize * stride + x as isize * 4);
            let (b, g, r, a) = (*px, *px.offset(1), *px.offset(2), *px.offset(3));
            if a == 0 {
                return (0, 0, 0, 0);
            }
            let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            (un(r), un(g), un(b), a)
        }
    }

    pub fn write_png(&self, path: &str) -> Result<(), String> {
        let c = CString::new(path).map_err(|_| "path contains a NUL".to_string())?;
        unsafe {
            cairo_surface_flush(self.surface);
            if cairo_surface_write_to_png(self.surface, c.as_ptr()) != CAIRO_STATUS_SUCCESS {
                return Err(format!("could not write {path}"));
            }
        }
        Ok(())
    }

    /// How many pixels differ from fully transparent.
    ///
    /// Only meaningful on a surface that starts transparent. A panel paints an
    /// opaque backdrop first, so every pixel is "inked" whether or not anything
    /// was drawn on top — use [`Image::variety`] there instead.
    pub fn ink(&self) -> usize {
        self.count(|p| p.3 != 0)
    }

    /// How many distinct colours appear inside `r`.
    ///
    /// This is the measure that can actually fail on an opaque surface: a
    /// region containing nothing but its background is one colour, and anything
    /// drawn into it — text, a border, a filled chip — raises the count. A test
    /// asserting "something was drawn here" is otherwise satisfied by the
    /// background alone.
    pub fn variety(&self, r: Rect) -> usize {
        let mut seen = Vec::new();
        let x0 = r.x.max(0.0) as i32;
        let y0 = r.y.max(0.0) as i32;
        let x1 = (r.right().ceil() as i32).min(self.width);
        let y1 = (r.bottom().ceil() as i32).min(self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                let p = self.pixel(x, y);
                if !seen.contains(&p) {
                    seen.push(p);
                    // A region with this many distinct colours is unambiguously
                    // not a flat fill; counting further wastes time on a large
                    // area with antialiased text in it.
                    if seen.len() >= 64 {
                        return seen.len();
                    }
                }
            }
        }
        seen.len()
    }

    fn count(&self, f: impl Fn((u8, u8, u8, u8)) -> bool) -> usize {
        let mut n = 0;
        for y in 0..self.height {
            for x in 0..self.width {
                if f(self.pixel(x, y)) {
                    n += 1;
                }
            }
        }
        n
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        unsafe {
            cairo_destroy(self.cr);
            cairo_surface_destroy(self.surface);
        }
    }
}

/// A font request, resolved by fontconfig through Pango.
#[derive(Debug, Clone, PartialEq)]
pub struct Font {
    pub family: String,
    pub size: f64,
    pub weight: u16,
}

impl Font {
    pub fn new(family: &str, size: f64) -> Font {
        Font {
            family: family.to_string(),
            size,
            weight: 400,
        }
    }

    pub fn bold(mut self) -> Font {
        self.weight = 600;
        self
    }

    pub fn weight(mut self, w: u16) -> Font {
        self.weight = w;
        self
    }

    pub fn size(mut self, s: f64) -> Font {
        self.size = s;
        self
    }

    /// A Pango font description string, e.g. `"Ubuntu Medium 11"`.
    pub fn describe(&self) -> String {
        let weight = match self.weight {
            0..=349 => "Light",
            350..=449 => "",
            450..=549 => "Medium",
            550..=649 => "Semibold",
            _ => "Bold",
        };
        if weight.is_empty() {
            format!("{} {}", self.family, self.size)
        } else {
            format!("{} {} {}", self.family, weight, self.size)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects_hit_test_their_own_area() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert!(r.contains(10.0, 20.0), "top-left corner is inside");
        assert!(r.contains(109.0, 69.0));
        assert!(!r.contains(110.0, 70.0), "bottom-right edge is exclusive");
        assert!(!r.contains(9.0, 20.0));
        assert_eq!(r.right(), 110.0);
        assert_eq!(r.bottom(), 70.0);
    }

    #[test]
    fn inset_never_produces_a_negative_size() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        let squashed = r.inset(50.0);
        assert_eq!(squashed.w, 0.0);
        assert_eq!(squashed.h, 0.0);
    }

    #[test]
    fn colours_mix_and_clamp() {
        let black = Rgba::rgb(0, 0, 0);
        let white = Rgba::rgb(255, 255, 255);
        assert_eq!(black.mix(white, 0.0), black);
        assert_eq!(black.mix(white, 1.0), white);
        // Out-of-range t is clamped rather than extrapolating past white.
        assert_eq!(black.mix(white, 5.0), white);
        let mid = black.mix(white, 0.5);
        assert!((mid.0 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn font_descriptions_match_pangos_format() {
        assert_eq!(Font::new("Ubuntu", 11.0).describe(), "Ubuntu 11");
        assert_eq!(
            Font::new("Ubuntu", 11.0).bold().describe(),
            "Ubuntu Semibold 11"
        );
        assert_eq!(
            Font::new("Ubuntu", 13.0).weight(700).describe(),
            "Ubuntu Bold 13"
        );
        assert_eq!(
            Font::new("Ubuntu", 9.0).weight(300).describe(),
            "Ubuntu Light 9"
        );
    }

    #[test]
    fn alpha_is_preserved_independently_of_channel_values() {
        let c = Rgba::rgb(20, 30, 40).with_alpha(0.5);
        assert_eq!(c.3, 0.5);
        assert!((c.0 - 20.0 / 255.0).abs() < 1e-9);
    }

    // The tests below render for real and read the pixels back. They need no
    // display, so they run everywhere the crate builds.

    #[test]
    fn a_filled_rect_lands_exactly_where_it_was_asked_to() {
        let img = Image::new(40, 40).expect("offscreen surface");
        let c = img.canvas();
        c.fill_rect(Rect::new(10.0, 10.0, 20.0, 20.0), Rgba::rgb(255, 0, 0));

        assert_eq!(img.pixel(15, 15), (255, 0, 0, 255), "inside the rect");
        assert_eq!(img.pixel(5, 15).3, 0, "left of the rect is untouched");
        assert_eq!(img.pixel(35, 15).3, 0, "right of the rect is untouched");
        assert_eq!(img.pixel(15, 5).3, 0, "above the rect is untouched");
        assert_eq!(img.pixel(15, 35).3, 0, "below the rect is untouched");
    }

    #[test]
    fn rounded_corners_are_actually_rounded() {
        let img = Image::new(60, 60).expect("offscreen surface");
        let c = img.canvas();
        c.fill_rounded(Rect::new(0.0, 0.0, 60.0, 60.0), 16.0, Rgba::rgb(0, 0, 255));

        // The very corner is outside a 16px radius, the centre is inside.
        assert_eq!(img.pixel(0, 0).3, 0, "corner pixel is cut away");
        assert_eq!(img.pixel(30, 30), (0, 0, 255, 255), "centre is filled");
        assert_eq!(img.pixel(59, 0).3, 0, "every corner, not just the first");
        assert_eq!(img.pixel(0, 59).3, 0);
        assert_eq!(img.pixel(59, 59).3, 0);
        // A zero radius must give back a plain rectangle, corners included.
        let sharp = Image::new(60, 60).expect("offscreen surface");
        sharp
            .canvas()
            .fill_rounded(Rect::new(0.0, 0.0, 60.0, 60.0), 0.0, Rgba::rgb(0, 0, 255));
        assert_eq!(sharp.pixel(0, 0), (0, 0, 255, 255));
    }

    #[test]
    fn text_puts_ink_on_the_surface_and_reports_its_height() {
        let img = Image::new(200, 60).expect("offscreen surface");
        let c = img.canvas();
        let font = Font::new("Sans", 14.0);
        let h = c.text("Nous", 10.0, 10.0, &font, Rgba::rgb(255, 255, 255), None);

        assert!(h > 0.0, "text reported no height");
        assert!(img.ink() > 20, "text drew nothing: ink={}", img.ink());
        // Measuring must agree with drawing, or layout is guesswork.
        let (mw, mh) = c.measure("Nous", &font, None);
        assert_eq!(mh, h);
        assert!(mw > 0.0);
    }

    #[test]
    fn clipping_confines_drawing_to_the_clip_rect() {
        let img = Image::new(40, 40).expect("offscreen surface");
        let c = img.canvas();
        c.clip_rect(Rect::new(0.0, 0.0, 20.0, 40.0));
        c.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), Rgba::rgb(0, 255, 0));
        c.restore();

        assert_eq!(img.pixel(10, 20), (0, 255, 0, 255), "inside the clip");
        assert_eq!(img.pixel(30, 20).3, 0, "outside the clip stayed empty");
        // After restore, the clip is gone.
        c.fill_rect(Rect::new(20.0, 0.0, 20.0, 40.0), Rgba::rgb(0, 255, 0));
        assert_eq!(img.pixel(30, 20), (0, 255, 0, 255));
    }

    #[test]
    fn half_transparent_fills_read_back_as_the_colour_asked_for() {
        let img = Image::new(20, 20).expect("offscreen surface");
        img.canvas().fill_rect(
            Rect::new(0.0, 0.0, 20.0, 20.0),
            Rgba::rgba(200, 100, 50, 0.5),
        );
        let (r, g, b, a) = img.pixel(10, 10);
        assert!((a as i32 - 128).abs() <= 1, "alpha was {a}");
        // Premultiplication is lossy at low alpha; a couple of levels is the
        // most it can drift at 50%.
        assert!((r as i32 - 200).abs() <= 2, "red was {r}");
        assert!((g as i32 - 100).abs() <= 2, "green was {g}");
        assert!((b as i32 - 50).abs() <= 2, "blue was {b}");
    }
}
