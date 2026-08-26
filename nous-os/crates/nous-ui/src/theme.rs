//! The look of the system.
//!
//! Two ideas drive this and everything else follows from them.
//!
//! **Colour means something.** Every accent in the interface is tied to the
//! risk of an action, using the same four levels the capability model already
//! defines. Nothing is coloured for decoration. A user who learns that amber
//! means "this writes to a file" has learned it everywhere, because there is
//! nowhere else amber is used.
//!
//! **Surfaces are quiet.** No gradients, no shadows, no chrome. Depth comes
//! from a hairline border and a small shift in background, which stays legible
//! at any scale and does not fight the desktop behind a translucent panel.
//!
//! The result should be recognisable at a glance and never mistaken for a
//! search bar with a wallpaper behind it.

use crate::draw::{Font, Rgba};

/// The risk levels the policy engine works in, re-exported rather than
/// redefined. The UI never invents its own severity: a level it renders is
/// always the one the capability itself carries, and adding a level to the
/// policy engine is a compile error here until it has a colour.
pub use nous_core::cap::Risk;

/// Read a risk level back from the name the daemon sends over the wire.
///
/// Anything unrecognised is treated as Critical. A risk level this build has
/// never heard of is not one to render calmly.
pub fn parse_risk(s: &str) -> Risk {
    match s.trim().to_ascii_lowercase().as_str() {
        "read" => Risk::Read,
        "write" => Risk::Write,
        "elevated" => Risk::Elevated,
        _ => Risk::Critical,
    }
}

/// The risk of a capability named as `domain.action:scope`, from the same table
/// the policy engine consults. An unparseable name is Critical for the reason
/// above.
pub fn risk_of(capability: &str) -> Risk {
    nous_core::cap::Capability::parse(capability).map_or(Risk::Critical, |c| c.risk())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub mode: Mode,

    /// The panel body. Alpha below 1.0 only takes effect on a compositing
    /// desktop; the window falls back to `backdrop_opaque` otherwise.
    pub backdrop: Rgba,
    pub backdrop_opaque: Rgba,
    /// A raised area inside the panel: the input line, a result card.
    pub surface: Rgba,
    pub surface_hover: Rgba,
    pub surface_active: Rgba,
    /// The hairline that separates surfaces. Never a shadow.
    pub hairline: Rgba,

    pub text: Rgba,
    /// Metadata, timestamps, capability names.
    pub text_dim: Rgba,
    /// Placeholder text and disabled controls.
    pub text_faint: Rgba,

    /// The four risk colours. Used for the accent bar on a step, the dot beside
    /// a capability, and nothing else.
    pub risk_read: Rgba,
    pub risk_write: Rgba,
    pub risk_elevated: Rgba,
    pub risk_critical: Rgba,

    /// The caret, and the mark of the system speaking rather than echoing.
    pub voice: Rgba,
    pub ok: Rgba,
    pub warn: Rgba,
    pub danger: Rgba,

    pub font: Font,
    pub font_mono: Font,
}

/// Fixed spacing. Layout code refers to these rather than to bare numbers, so
/// the rhythm of the interface is set in one place.
pub struct Metrics;

impl Metrics {
    /// The base unit. Every gap is a multiple of it.
    pub const UNIT: f64 = 6.0;
    pub const PAD: f64 = 18.0;
    pub const GAP: f64 = 12.0;
    pub const RADIUS: f64 = 14.0;
    pub const RADIUS_SMALL: f64 = 8.0;
    pub const HAIRLINE: f64 = 1.0;
    /// Height of the prompt line, including its padding.
    pub const PROMPT_HEIGHT: f64 = 58.0;
    /// Height of one result row.
    pub const ROW_HEIGHT: f64 = 46.0;
    /// The panel's width, and the widest it will grow on a large screen.
    pub const PANEL_WIDTH: f64 = 720.0;
    pub const PANEL_MAX_HEIGHT: f64 = 560.0;
    /// The coloured bar down the left of a step, keyed to its risk.
    pub const ACCENT_BAR: f64 = 3.0;
}

impl Theme {
    pub fn dark() -> Theme {
        Theme {
            mode: Mode::Dark,
            // Not grey: a trace of blue-violet, so the panel reads as its own
            // material rather than as a dimmed window.
            backdrop: Rgba::rgba(16, 17, 24, 0.92),
            backdrop_opaque: Rgba::rgb(16, 17, 24),
            surface: Rgba::rgba(255, 255, 255, 0.045),
            surface_hover: Rgba::rgba(255, 255, 255, 0.08),
            surface_active: Rgba::rgba(255, 255, 255, 0.12),
            hairline: Rgba::rgba(255, 255, 255, 0.09),

            text: Rgba::rgb(236, 237, 243),
            text_dim: Rgba::rgba(236, 237, 243, 0.58),
            text_faint: Rgba::rgba(236, 237, 243, 0.32),

            risk_read: Rgba::rgb(106, 176, 255),
            risk_write: Rgba::rgb(240, 184, 92),
            risk_elevated: Rgba::rgb(255, 138, 76),
            risk_critical: Rgba::rgb(255, 94, 94),

            voice: Rgba::rgb(240, 184, 92),
            ok: Rgba::rgb(112, 208, 148),
            warn: Rgba::rgb(240, 184, 92),
            danger: Rgba::rgb(255, 94, 94),

            font: Font::new("Ubuntu", 12.0),
            font_mono: Font::new("Ubuntu Mono", 11.0),
        }
    }

    pub fn light() -> Theme {
        Theme {
            mode: Mode::Light,
            backdrop: Rgba::rgba(250, 250, 252, 0.94),
            backdrop_opaque: Rgba::rgb(250, 250, 252),
            surface: Rgba::rgba(12, 14, 28, 0.045),
            surface_hover: Rgba::rgba(12, 14, 28, 0.08),
            surface_active: Rgba::rgba(12, 14, 28, 0.12),
            hairline: Rgba::rgba(12, 14, 28, 0.12),

            text: Rgba::rgb(22, 24, 34),
            text_dim: Rgba::rgba(22, 24, 34, 0.62),
            text_faint: Rgba::rgba(22, 24, 34, 0.38),

            // Darkened so they hold their meaning against a light background;
            // the dark theme's colours would wash out.
            risk_read: Rgba::rgb(24, 108, 200),
            risk_write: Rgba::rgb(178, 116, 12),
            risk_elevated: Rgba::rgb(196, 84, 18),
            risk_critical: Rgba::rgb(198, 32, 40),

            voice: Rgba::rgb(178, 116, 12),
            ok: Rgba::rgb(26, 132, 78),
            warn: Rgba::rgb(178, 116, 12),
            danger: Rgba::rgb(198, 32, 40),

            font: Font::new("Ubuntu", 12.0),
            font_mono: Font::new("Ubuntu Mono", 11.0),
        }
    }

    /// Pick a theme from the desktop's own setting, so the panel matches the
    /// rest of the session instead of imposing a look.
    ///
    /// `NOUS_THEME=dark|light` overrides it; that is what a user sets when the
    /// guess is wrong.
    pub fn detect() -> Theme {
        if let Ok(v) = std::env::var("NOUS_THEME") {
            match v.trim().to_ascii_lowercase().as_str() {
                "light" => return Theme::light(),
                "dark" => return Theme::dark(),
                _ => {}
            }
        }
        if gtk_theme_is_light() {
            Theme::light()
        } else {
            Theme::dark()
        }
    }

    pub fn risk(&self, r: Risk) -> Rgba {
        match r {
            Risk::Read => self.risk_read,
            Risk::Write => self.risk_write,
            Risk::Elevated => self.risk_elevated,
            Risk::Critical => self.risk_critical,
        }
    }

    pub fn title_font(&self) -> Font {
        self.font.clone().size(15.0).weight(500)
    }

    pub fn prompt_font(&self) -> Font {
        self.font.clone().size(17.0)
    }

    pub fn small_font(&self) -> Font {
        self.font.clone().size(10.5)
    }
}

/// Read the GTK theme name from the user's settings and decide whether it is a
/// light one.
///
/// Deliberately does not shell out to `gsettings`: the panel must open in a few
/// milliseconds, and spawning a process on the summon path is the kind of delay
/// that makes an interface feel slow. The settings file is authoritative for
/// anything the user changed, and "dark" in the name is how every Mint, Adwaita
/// and Yaru variant marks itself.
fn gtk_theme_is_light() -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    for rel in [
        "/.config/gtk-4.0/settings.ini",
        "/.config/gtk-3.0/settings.ini",
    ] {
        let Ok(body) = std::fs::read_to_string(format!("{home}{rel}")) else {
            continue;
        };
        if let Some(v) = ini_value(&body, "gtk-application-prefer-dark-theme") {
            return !matches!(v.trim(), "1" | "true" | "TRUE" | "True");
        }
        if let Some(v) = ini_value(&body, "gtk-theme-name") {
            return !v.to_ascii_lowercase().contains("dark");
        }
    }
    // No preference recorded. Dark is the safer default: the panel floats over
    // whatever is on screen, and a bright rectangle at night is worse than a
    // dark one in daylight.
    false
}

fn ini_value(body: &str, key: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && !l.starts_with(';'))
        .find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == key).then(|| v.trim().to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_risk_names_are_treated_as_the_most_dangerous() {
        assert_eq!(parse_risk("read"), Risk::Read);
        assert_eq!(parse_risk("  Write "), Risk::Write);
        assert_eq!(parse_risk("ELEVATED"), Risk::Elevated);
        assert_eq!(parse_risk("critical"), Risk::Critical);
        // A risk level this build has never heard of must not render as calm.
        assert_eq!(parse_risk("catastrophic"), Risk::Critical);
        assert_eq!(parse_risk(""), Risk::Critical);
    }

    #[test]
    fn every_risk_level_has_a_distinct_colour_in_both_themes() {
        for theme in [Theme::dark(), Theme::light()] {
            let all = [Risk::Read, Risk::Write, Risk::Elevated, Risk::Critical];
            for (i, a) in all.iter().enumerate() {
                for b in &all[i + 1..] {
                    assert_ne!(
                        theme.risk(*a),
                        theme.risk(*b),
                        "{:?} and {:?} share a colour in {:?}",
                        a,
                        b,
                        theme.mode
                    );
                }
            }
        }
    }

    #[test]
    fn text_stays_readable_against_its_own_backdrop() {
        // Relative luminance, WCAG's formula. Body text must clear 4.5:1 or the
        // interface is unusable for anyone with less than perfect sight.
        fn lum(c: Rgba) -> f64 {
            let f = |v: f64| {
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * f(c.0) + 0.7152 * f(c.1) + 0.0722 * f(c.2)
        }
        fn ratio(a: Rgba, b: Rgba) -> f64 {
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        }
        for theme in [Theme::dark(), Theme::light()] {
            let bg = theme.backdrop_opaque;
            assert!(
                ratio(theme.text, bg) >= 4.5,
                "{:?} body text is {:.1}:1",
                theme.mode,
                ratio(theme.text, bg)
            );
            // Dim text is smaller and secondary, so 3:1 is the bar it must
            // clear. Composited over the backdrop, not used at face value.
            let dim = bg.mix(theme.text, theme.text_dim.3);
            assert!(
                ratio(dim, bg) >= 3.0,
                "{:?} dim text is {:.1}:1",
                theme.mode,
                ratio(dim, bg)
            );
        }
    }

    #[test]
    fn risk_colours_are_visible_against_their_backdrop() {
        fn lum(c: Rgba) -> f64 {
            let f = |v: f64| {
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * f(c.0) + 0.7152 * f(c.1) + 0.0722 * f(c.2)
        }
        for theme in [Theme::dark(), Theme::light()] {
            let bg = theme.backdrop_opaque;
            for r in [Risk::Read, Risk::Write, Risk::Elevated, Risk::Critical] {
                let c = theme.risk(r);
                let (x, y) = (lum(c), lum(bg));
                let ratio = (x.max(y) + 0.05) / (x.min(y) + 0.05);
                assert!(
                    ratio >= 3.0,
                    "{:?} {} is {:.1}:1",
                    theme.mode,
                    r.as_str(),
                    ratio
                );
            }
        }
    }

    #[test]
    fn ini_parsing_ignores_comments_and_whitespace() {
        let body = "[Settings]\n# gtk-theme-name=Wrong\n  gtk-theme-name = Mint-Y-Dark \n";
        assert_eq!(
            ini_value(body, "gtk-theme-name").as_deref(),
            Some("Mint-Y-Dark")
        );
        assert_eq!(ini_value(body, "missing"), None);
    }
}
