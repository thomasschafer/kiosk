use kiosk_core::config::{NamedColor, ThemeColor};
use ratatui::style::Color;

pub struct Theme {
    pub accent: Color,
    pub secondary: Color,
    pub success: Color,
}

impl Theme {
    pub fn from_config(config: &kiosk_core::config::ThemeConfig) -> Self {
        Self {
            accent: to_ratatui_color(&config.accent),
            secondary: to_ratatui_color(&config.secondary),
            success: to_ratatui_color(&config.success),
        }
    }
}

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
    }

    #[test]
    fn test_theme_custom() {
        let config = ThemeConfig {
            accent: ThemeColor::Named(NamedColor::Blue),
            secondary: ThemeColor::Rgb(255, 0, 255),
            success: ThemeColor::Named(NamedColor::Green),
        };
        let theme = Theme::from_config(&config);
        assert_eq!(theme.accent, Color::Blue);
        assert_eq!(theme.secondary, Color::Rgb(255, 0, 255));
        assert_eq!(theme.success, Color::Green);
    }
}
