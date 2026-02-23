use kiosk_core::config::{NamedColor, ThemeColor};
use ratatui::style::Color;

/// Generates `Theme` and `from_config` from a list of field names,
/// mirroring `ThemeConfig` without manual repetition.
macro_rules! define_theme {
    ($($field:ident),* $(,)?) => {
        pub struct Theme {
            $(pub $field: Color,)*
        }

        impl Theme {
            pub fn from_config(config: &kiosk_core::config::ThemeConfig) -> Self {
                Self {
                    $($field: to_ratatui_color(&config.$field),)*
                }
            }
        }
    };
}

define_theme!(accent, secondary, success, error, warning, muted, border, hint, highlight_fg);

fn to_ratatui_color(color: &ThemeColor) -> Color {
    match color {
        ThemeColor::Rgb(r, g, b) => Color::Rgb(*r, *g, *b),
        ThemeColor::Named(named) => match named {
            NamedColor::Black => Color::Black,
            NamedColor::Red => Color::Red,
            NamedColor::Green => Color::Green,
            NamedColor::Yellow => Color::Yellow,
            NamedColor::Blue => Color::Blue,
            NamedColor::Magenta => Color::Magenta,
            NamedColor::Cyan => Color::Cyan,
            NamedColor::White => Color::White,
            NamedColor::Gray => Color::Gray,
            NamedColor::DarkGray => Color::DarkGray,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiosk_core::config::ThemeConfig;

    #[test]
    fn test_theme_defaults() {
        let theme = Theme::from_config(&ThemeConfig::default());
        assert_eq!(theme.accent, Color::Magenta);
        assert_eq!(theme.secondary, Color::Cyan);
        assert_eq!(theme.success, Color::Green);
        assert_eq!(theme.error, Color::Red);
        assert_eq!(theme.warning, Color::Yellow);
        assert_eq!(theme.muted, Color::DarkGray);
        assert_eq!(theme.border, Color::DarkGray);
        assert_eq!(theme.hint, Color::Blue);
        assert_eq!(theme.highlight_fg, Color::Black);
    }

    #[test]
    fn test_theme_custom() {
        let config = ThemeConfig {
            accent: ThemeColor::Named(NamedColor::Blue),
            secondary: ThemeColor::Rgb(255, 0, 255),
            error: ThemeColor::Named(NamedColor::Magenta),
            highlight_fg: ThemeColor::Named(NamedColor::Yellow),
            ..ThemeConfig::default()
        };
        let theme = Theme::from_config(&config);
        assert_eq!(theme.accent, Color::Blue);
        assert_eq!(theme.secondary, Color::Rgb(255, 0, 255));
        assert_eq!(theme.success, Color::Green); // default
        assert_eq!(theme.error, Color::Magenta);
        assert_eq!(theme.highlight_fg, Color::Yellow);
    }

    #[test]
    fn test_theme_dark_gray_color() {
        let config = ThemeConfig {
            muted: ThemeColor::Named(NamedColor::DarkGray),
            ..ThemeConfig::default()
        };
        let theme = Theme::from_config(&config);
        assert_eq!(theme.muted, Color::DarkGray);
    }
}
