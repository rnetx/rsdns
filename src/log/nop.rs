use std::fmt;

#[derive(Default)]
pub(crate) struct NopLogger;

impl NopLogger {
    pub(crate) fn into_box(self) -> Box<dyn super::Logger> {
        Box::new(self)
    }
}

impl super::Logger for NopLogger {
    fn enabled(&self, _level: super::Level) -> bool {
        false
    }

    fn color_enabled(&self) -> bool {
        false
    }

    fn log(&self, _level: super::Level, _message: fmt::Arguments<'_>) {}

    fn log_with_tracker(
        &self,
        _level: super::Level,
        _tracker: &super::Tracker,
        _message: fmt::Arguments<'_>,
    ) {
    }
}
