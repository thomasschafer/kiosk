use ratatui::style::Color;

pub struct Theme {
    pub accent: Color,
    pub secondary: Color,
    pub success: Color,
}

impl Theme {
    pub fn from_config(config: &kiosk_core::config::ThemeConfig) -> Self {
        Self {
            accent: parse_color(config.accent.as_deref()).unwrap_or(Color::Magenta),
            secondary: parse_color(config.secondary.as_deref()).unwrap_or(Color::Cyan),
            success: parse_color(config.success.as_deref()).unwrap_or(Color::Green),
        }
    }
}

fn parse_color(s: Option<&str>) -> Option<Color> {
    let s = s?;
    // Try hex first
    if let Some(hex) = s.strip_prefix('#')
        && hex.len() == 6
    {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        return Some(Color::Rgb(r, g, b));
    }
    // Named colors
    match s.to_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiosk_core::config::ThemeConfig;

    #[test]
    fn test_parse_color_named() {
        assert_eq!(parse_color(Some("magenta")), Some(Color::Magenta));
        assert_eq!(parse_color(Some("cyan")), Some(Color::Cyan));
        assert_eq!(parse_color(Some("green")), Some(Color::Green));
        assert_eq!(parse_color(Some("RED")), Some(Color::Red));
    }

    #[test]
    fn test_parse_color_hex() {
        assert_eq!(parse_color(Some("#ff0000")), Some(Color::Rgb(255, 0, 0)));
        assert_eq!(parse_color(Some("#00ff00")), Some(Color::Rgb(0, 255, 0)));
        assert_eq!(parse_color(Some("#0000ff")), Some(Color::Rgb(0, 0, 255)));
    }

    #[test]
    fn test_parse_color_invalid() {
        assert_eq!(parse_color(None), None);
        assert_eq!(parse_color(Some("notacolor")), None);
        assert_eq!(parse_color(Some("#fff")), None); // too short
        assert_eq!(parse_color(Some("#zzzzzz")), None); // invalid hex
    }

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
            accent: Some("blue".to_string()),
            secondary: Some("#ff00ff".to_string()),
            success: None,
        };
        let theme = Theme::from_config(&config);
        assert_eq!(theme.accent, Color::Blue);
        assert_eq!(theme.secondary, Color::Rgb(255, 0, 255));
        assert_eq!(theme.success, Color::Green); // default
    }
}
