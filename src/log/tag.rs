use std::sync::Arc;

pub(crate) struct TagLogger {
    inner: Arc<Box<dyn super::Logger>>,
    tag: String,
}

impl TagLogger {
    pub(crate) fn new(inner: Arc<Box<dyn super::Logger>>, tag: String) -> Self {
        Self { inner, tag }
    }

    pub(crate) fn into_box(self) -> Box<dyn super::Logger> {
        Box::new(self)
    }
}

impl super::Logger for TagLogger {
    fn enabled(&self, level: super::Level) -> bool {
        self.inner.enabled(level)
    }

    fn log(&self, level: super::Level, message: std::fmt::Arguments<'_>) {
        if !self.enabled(level) {
            return;
        }

        self.inner.log(
            level,
            format_args!("[{}] {}", self.tag, message.to_string().trim_end()),
        );
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

        self.inner.log_with_tracker(
            level,
            tracker,
            format_args!("[{}] {}", self.tag, message.to_string().trim_end()),
        );
    }
}
