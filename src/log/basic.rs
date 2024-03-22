use std::{
    io::Write,
    sync::{Arc, Mutex},
};

pub(crate) struct BasicLogger {
    disable_timestamp: bool,
    level: super::Level,
    output: Arc<Mutex<Box<dyn Write>>>,
}

impl BasicLogger {
    pub(crate) fn new(
        disable_timestamp: bool,
        level: super::Level,
        output: Box<dyn Write>,
    ) -> Self {
        Self {
            disable_timestamp,
            level,
            output: Arc::new(Mutex::new(output)),
        }
    }

    pub(crate) fn into_box(self) -> Box<dyn super::Logger> {
        Box::new(self)
    }
}

unsafe impl Send for BasicLogger {}
unsafe impl Sync for BasicLogger {}

impl super::Logger for BasicLogger {
    fn enabled(&self, level: super::Level) -> bool {
        self.level <= level
    }

    fn log(&self, level: super::Level, message: std::fmt::Arguments<'_>) {
        if !self.enabled(level) {
            return;
        }

        let mut s = if self.disable_timestamp {
            format!("[{}] {}", level, message.to_string().trim_end())
        } else {
            format!(
                "[{}] [{}] {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                level,
                message.to_string().trim_end()
            )
        };
        s.push_str("\n");

        if let Ok(mut output) = self.output.lock() {
            output.write_all(s.as_bytes()).ok();
        }
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
                level,
                format!("{} {}ms", tracker.id(), tracker.duration().as_millis()),
                message.to_string().trim_end()
            )
        } else {
            format!(
                "[{}] [{}] [{}] {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                level,
                format!("{} {}ms", tracker.id(), tracker.duration().as_millis()),
                message.to_string().trim_end()
            )
        };
        s.push_str("\n");

        if let Ok(mut output) = self.output.lock() {
            output.write_all(s.as_bytes()).ok();
        }
    }
}
