use std::{error::Error, fmt, str::FromStr};

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
        match self {
            Self::Debug => write!(f, "Debug"),
            Self::Info => write!(f, "Info"),
            Self::Warn => write!(f, "Warn"),
            Self::Error => write!(f, "Error"),
            Self::Fatal => write!(f, "Fatal"),
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Debug => write!(f, "Debug"),
            Self::Info => write!(f, "Info"),
            Self::Warn => write!(f, "Warn"),
            Self::Error => write!(f, "Error"),
            Self::Fatal => write!(f, "Fatal"),
        }
    }
}

impl FromStr for Level {
    type Err = Box<dyn Error + Send + Sync>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" | "warning" => Ok(Self::Warn),
            "error" | "err" => Ok(Self::Error),
            "fatal" => Ok(Self::Fatal),
            _ => Err(format!("unknown log level: {}", s).into()),
        }
    }
}
