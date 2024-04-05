use std::{
    io::Write,
    sync::{Arc, Mutex},
};

pub(crate) struct BasicLogger {
    disable_timestamp: bool,
    level: super::Level,
    color_enabled: bool,
    output: Arc<Mutex<Box<dyn Write + Send + Sync>>>,
}

impl BasicLogger {
    pub(crate) fn new(
        disable_timestamp: bool,
        level: super::Level,
        color_enabled: bool,
        output: Box<dyn Write + Send + Sync>,
    ) -> Self {
        Self {
            disable_timestamp,
            level,
            color_enabled,
            output: Arc::new(Mutex::new(output)),
        }
    }

    pub(crate) fn into_box(self) -> Box<dyn super::Logger> {
        Box::new(self)
    }

    fn write_to_output(&self, s: &String) {
        if let Ok(mut output) = self.output.lock() {
            if output.write_all(s.as_bytes()).ok().is_some() {
                output.flush().ok();
            }
        }
    }
}

impl super::Logger for BasicLogger {
    fn enabled(&self, level: super::Level) -> bool {
        self.level <= level
    }

    fn color_enabled(&self) -> bool {
        self.color_enabled
    }

    fn log(&self, level: super::Level, message: std::fmt::Arguments<'_>) {
        if !self.enabled(level) {
            return;
        }

        let mut s = if self.disable_timestamp {
            format!(
                "[{}] {}",
                if self.color_enabled {
                    level.to_color_str()
                } else {
                    level.to_str()
                },
                message.to_string().trim_end()
            )
        } else {
            format!(
                "[{}] [{}] {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                if self.color_enabled {
                    level.to_color_str()
                } else {
                    level.to_str()
                },
                message.to_string().trim_end()
            )
        };
        s.push_str("\n");

        self.write_to_output(&s);
    }

    fn log_with_tracker(
        &self,
        level: super::Level,
        tracker: &super::Tracker,
        message: std::fmt::Arguments<'_>,
    ) {
        if !self.enabled(level) {
            return;
        }

        let mut s = if self.disable_timestamp {
            format!(
                "[{}] [{}] {}",
                if self.color_enabled {
                    level.to_color_str()
                } else {
                    level.to_str()
                },
                if self.color_enabled {
                    tracker.to_color_str()
                } else {
                    tracker.to_str()
                },
                message.to_string().trim_end()
            )
        } else {
            format!(
                "[{}] [{}] [{}] {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                if self.color_enabled {
                    level.to_color_str()
                } else {
                    level.to_str()
                },
                if self.color_enabled {
                    tracker.to_color_str()
                } else {
                    tracker.to_str()
                },
                message.to_string().trim_end()
            )
        };
        s.push_str("\n");

        self.write_to_output(&s);
    }
}
