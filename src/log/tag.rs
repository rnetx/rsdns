use std::sync::Arc;

use colored::Colorize;

pub(crate) struct TagLogger {
    inner: Arc<Box<dyn super::Logger>>,
    tag: String,
    color: Option<colored::Color>,
}

impl TagLogger {
    pub(crate) fn new(inner: Arc<Box<dyn super::Logger>>, tag: String) -> Self {
        Self {
            inner,
            tag,
            color: None,
        }
    }

    pub(crate) fn with_color(self, color: colored::Color) -> Self {
        Self {
            color: Some(color),
            ..self
        }
    }

    pub(crate) fn into_box(self) -> Box<dyn super::Logger> {
        Box::new(self)
    }
}

impl super::Logger for TagLogger {
    fn enabled(&self, level: super::Level) -> bool {
        self.inner.enabled(level)
    }

    fn color_enabled(&self) -> bool {
        self.inner.color_enabled()
    }

    fn log(&self, level: super::Level, message: std::fmt::Arguments<'_>) {
        if !self.enabled(level) {
            return;
        }

        self.inner.log(
            level,
            format_args!(
                "[{}] {}",
                if self.inner.color_enabled() && self.color.is_some() {
                    let color = self.color.clone().unwrap();
                    self.tag.color(color).to_string()
                } else {
                    self.tag.to_string()
                },
                message.to_string().trim_end()
            ),
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
            format_args!(
                "[{}] {}",
                if self.inner.color_enabled() && self.color.is_some() {
                    let color = self.color.clone().unwrap();
                    self.tag.color(color).to_string()
                } else {
                    self.tag.to_string()
                },
                message.to_string().trim_end()
            ),
        );
    }
}
