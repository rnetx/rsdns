use std::{fmt, str::FromStr};

use colored::Colorize;

lazy_static::lazy_static! {
    static ref DEBUG: String = "Debug".to_string();
    static ref INFO: String = "Info".to_string();
    static ref WARN: String = "Warn".to_string();
    static ref ERROR: String = "Error".to_string();
    static ref FATAL: String = "Fatal".to_string();

    static ref COLOR_DEBUG: String = "Debug".blue().to_string();
    static ref COLOR_INFO: String = "Info".green().to_string();
    static ref COLOR_WARN: String = "Warn".yellow().to_string();
    static ref COLOR_ERROR: String = "Error".red().to_string();
    static ref COLOR_FATAL: String = "Fatal".magenta().to_string();
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl Default for Level {
    fn default() -> Self {
        Self::Info
    }
}

impl fmt::Debug for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_str())
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_str())
    }
}

impl FromStr for Level {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" | "warning" => Ok(Self::Warn),
            "error" | "err" => Ok(Self::Error),
            "fatal" => Ok(Self::Fatal),
            _ => Err(anyhow::anyhow!("unknown log level: {}", s)),
        }
    }
}

impl Level {
    pub(crate) fn to_str(&self) -> &str {
        match self {
            Self::Debug => &DEBUG,
            Self::Info => &INFO,
            Self::Warn => &WARN,
            Self::Error => &ERROR,
            Self::Fatal => &FATAL,
        }
    }

    pub(crate) fn to_color_str(&self) -> &str {
        match self {
            Self::Debug => &COLOR_DEBUG,
            Self::Info => &COLOR_INFO,
            Self::Warn => &COLOR_WARN,
            Self::Error => &COLOR_ERROR,
            Self::Fatal => &COLOR_FATAL,
        }
    }
}
