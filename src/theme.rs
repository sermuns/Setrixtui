//! Theme loading: btop-style `theme[key]="value"` and hex → ratatui Color.

use ratatui::style::Color;
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

/// One Dark palette and UI colours loaded from a theme file.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Sand/piece colours (index 0..=5): green, yellow, red, blue, magenta, cyan.
    pub sand: [Color; 6],
    /// Playfield background.
    pub bg: Color,
    /// Grid / border.
    pub div_line: Color,
    /// Text (score, level).
    pub main_fg: Color,
    /// Highlight / titles.
    pub title: Color,
    /// Inactive / secondary text. Reserved for future UI (e.g. pause menu).
    #[allow(dead_code)]
    pub inactive_fg: Color,
}

#[derive(Debug, Error)]
pub enum ThemeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid hex: {0}")]
    InvalidHex(String),
}

impl Default for Theme {
    fn default() -> Self {
        Self::onedark_default()
    }
}

impl Theme {
    /// Hardcoded One Dark defaults: exact hex values from onedark.theme (no saturation tweaks).
    pub fn onedark_default() -> Self {
        Self {
            sand: [
                parse_hex("#98C379").unwrap(), // mem_box / green
                parse_hex("#E5C07B").unwrap(), // title / cpu_mid / yellow
                parse_hex("#E06C75").unwrap(), // cpu_end / temp_end / red
                parse_hex("#61AFEF").unwrap(), // cpu_box / blue
                parse_hex("#C678DD").unwrap(), // net_box / magenta
                parse_hex("#56B6C2").unwrap(), // hi_fg / proc_misc / cyan
            ],
            bg: parse_hex("#31353F").unwrap(), // meter_bg from onedark.theme
            div_line: parse_hex("#3F444F").unwrap(), // div_line
            main_fg: parse_hex("#ABB2BF").unwrap(), // main_fg
            title: parse_hex("#E5C07B").unwrap(), // title
            inactive_fg: parse_hex("#5C6370").unwrap(), // inactive_fg
        }
    }

    /// Load theme from a btop-style file: `theme[key]="value"` or `theme[key]='value'`.
    /// Falls back to One Dark defaults if path is None or file is missing/invalid.
    /// `palette` selects colour variant: Normal (theme), HighContrast, or Colorblind.
    pub fn load(path: Option<&Path>, palette: crate::Palette) -> Result<Self, ThemeError> {
        let path = match path {
            Some(p) if p.exists() => p,
            _ => return Ok(Self::default_for_palette(palette)),
        };
        let s = std::fs::read_to_string(path)?;
        let map = parse_theme_file(&s);
        let mut theme = Self::from_map(&map);
        theme.apply_palette(palette);
        Ok(theme)
    }

    /// Default theme for a palette when no file is loaded.
    fn default_for_palette(palette: crate::Palette) -> Self {
        let mut t = Self::onedark_default();
        t.apply_palette(palette);
        t
    }

    /// Override sand (and optionally UI) colours for high-contrast or colorblind.
    pub fn apply_palette(&mut self, palette: crate::Palette) {
        match palette {
            crate::Palette::Normal => {}
            crate::Palette::HighContrast => {
                // High-contrast: distinct saturated colours on dark bg
                self.sand = [
                    parse_hex("#00FF00").unwrap(), // bright green
                    parse_hex("#FFFF00").unwrap(), // yellow
                    parse_hex("#FF0000").unwrap(), // red
                    parse_hex("#0088FF").unwrap(), // blue
                    parse_hex("#FF00FF").unwrap(), // magenta
                    parse_hex("#00FFFF").unwrap(), // cyan
                ];
            }
            crate::Palette::Colorblind => {
                // Colorblind-friendly: avoid red/green alone; use shape-friendly contrast
                self.sand = [
                    parse_hex("#0077BB").unwrap(), // blue
                    parse_hex("#EE7733").unwrap(), // orange
                    parse_hex("#009988").unwrap(), // teal
                    parse_hex("#CC3311").unwrap(), // red (distinct from blue/orange)
                    parse_hex("#EE3377").unwrap(), // magenta
                    parse_hex("#BBBB00").unwrap(), // yellow
                ];
            }
        }
    }

    fn from_map(map: &HashMap<String, String>) -> Self {
        let get = |key: &str| {
            map.get(key)
                .and_then(|v| parse_hex(v.trim_matches('"').trim_matches('\'').trim()).ok())
        };
        // Keys match onedark.theme; fallbacks are the same file’s hex values (no extra saturation).
        Self {
            sand: [
                get("mem_box")
                    .or_else(|| get("cpu_start"))
                    .unwrap_or_else(|| parse_hex("#98C379").unwrap()),
                get("title")
                    .or_else(|| get("cpu_mid"))
                    .unwrap_or_else(|| parse_hex("#E5C07B").unwrap()),
                get("cpu_end")
                    .or_else(|| get("temp_end"))
                    .unwrap_or_else(|| parse_hex("#E06C75").unwrap()),
                get("cpu_box").unwrap_or_else(|| parse_hex("#61AFEF").unwrap()),
                get("net_box").unwrap_or_else(|| parse_hex("#C678DD").unwrap()),
                get("hi_fg")
                    .or_else(|| get("proc_misc"))
                    .unwrap_or_else(|| parse_hex("#56B6C2").unwrap()),
            ],
            bg: get("meter_bg").unwrap_or_else(|| parse_hex("#31353F").unwrap()),
            div_line: get("div_line").unwrap_or_else(|| parse_hex("#3F444F").unwrap()),
            main_fg: get("main_fg").unwrap_or_else(|| parse_hex("#ABB2BF").unwrap()),
            title: get("title").unwrap_or_else(|| parse_hex("#E5C07B").unwrap()),
            inactive_fg: get("inactive_fg").unwrap_or_else(|| parse_hex("#5C6370").unwrap()),
        }
    }

    /// Sand colour for piece/sand index (0..6).
    #[inline]
    pub fn sand_color(&self, index: u8) -> Color {
        self.sand[(index as usize) % 6]
    }
}

/// Parse btop-style theme file into key -> value map.
fn parse_theme_file(s: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(stripped) = line.strip_prefix("theme[") {
            if let Some(end) = stripped.find(']') {
                let key = stripped[..end].trim();
                let rest = stripped[end + 1..].trim();
                if let Some(eq) = rest.find('=') {
                    let value = rest[eq + 1..]
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string();
                    if !value.is_empty() {
                        map.insert(key.to_string(), value);
                    }
                }
            }
        }
    }
    map
}

/// Parse hex colour "#RRGGBB" or "#RGB" into ratatui Color.
pub fn parse_hex(s: &str) -> Result<Color, ThemeError> {
    let s = s.trim().trim_start_matches('#');
    let (r, g, b) = if s.len() == 6 {
        let r =
            u8::from_str_radix(&s[0..2], 16).map_err(|_| ThemeError::InvalidHex(s.to_string()))?;
        let g =
            u8::from_str_radix(&s[2..4], 16).map_err(|_| ThemeError::InvalidHex(s.to_string()))?;
        let b =
            u8::from_str_radix(&s[4..6], 16).map_err(|_| ThemeError::InvalidHex(s.to_string()))?;
        (r, g, b)
    } else if s.len() == 3 {
        let r = u8::from_str_radix(&s[0..1], 16)
            .map_err(|_| ThemeError::InvalidHex(s.to_string()))?
            * 17;
        let g = u8::from_str_radix(&s[1..2], 16)
            .map_err(|_| ThemeError::InvalidHex(s.to_string()))?
            * 17;
        let b = u8::from_str_radix(&s[2..3], 16)
            .map_err(|_| ThemeError::InvalidHex(s.to_string()))?
            * 17;
        (r, g, b)
    } else {
        return Err(ThemeError::InvalidHex(s.to_string()));
    };
    Ok(Color::Rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_6() {
        let c = parse_hex("#98C379").unwrap();
        assert!(matches!(c, Color::Rgb(0x98, 0xC3, 0x79)));
    }

    #[test]
    fn test_parse_hex_3() {
        let c = parse_hex("#FFF").unwrap();
        assert!(matches!(c, Color::Rgb(255, 255, 255)));
    }

    #[test]
    fn test_parse_theme_line() {
        let map = parse_theme_file(r##"theme[meter_bg]="#31353F""##);
        assert_eq!(map.get("meter_bg"), Some(&"#31353F".to_string()));
    }
}
