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
