//! Persist high scores to disk (XDG config or ~/.config/setrixtui).

use anyhow::Result;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

const FILENAME: &str = "highscores";

/// Returns the path to the high scores file (config dir / setrixtui / highscores).
fn config_path() -> Result<PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if xdg.is_empty() {
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".config")
        } else {
            PathBuf::from(xdg)
        }
    } else {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(|_| PathBuf::from("."))
    };
    Ok(base.join("setrixtui").join(FILENAME))
}

/// Load high scores from disk. Returns (endless, timed, clear); 0 on missing/parse error.
pub fn load_high_scores() -> (u32, u32, u32) {
    let path = match config_path() {
        Ok(p) => p,
        Err(_) => return (0, 0, 0),
    };
    let content = match fs::read(path) {
        Ok(c) => c,
        Err(_) => return (0, 0, 0),
    };
    let mut endless = 0u32;
    let mut timed = 0u32;
    let mut clear = 0u32;
    for (i, line) in BufReader::new(&content[..]).lines().take(3).enumerate() {
        let n = line
            .ok()
            .as_ref()
            .and_then(|l| l.trim().parse::<u32>().ok())
            .unwrap_or(0);
        match i {
            0 => endless = n,
            1 => timed = n,
            2 => clear = n,
            _ => {}
        }
    }
    (endless, timed, clear)
}

/// Save high scores to disk. Creates config directory if needed.
pub fn save_high_scores(endless: u32, timed: u32, clear: u32) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::File::create(path)?;
    writeln!(f, "{}", endless)?;
    writeln!(f, "{}", timed)?;
    writeln!(f, "{}", clear)?;
    Ok(())
}
