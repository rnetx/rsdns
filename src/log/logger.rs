use std::fmt;

pub(crate) trait Logger: Send + Sync {
    fn enabled(&self, level: super::Level) -> bool;

    fn log(&self, level: super::Level, message: fmt::Arguments<'_>);

    fn log_with_tracker(
        &self,
        level: super::Level,
        tracker: &super::Tracker,
        message: fmt::Arguments<'_>,
    );

    fn log_with_option_tracker(
        &self,
        level: super::Level,
        tracker: Option<&super::Tracker>,
        message: fmt::Arguments<'_>,
    ) {
        if let Some(tracker) = tracker {
            self.log_with_tracker(level, tracker, message);
        } else {
            self.log(level, message);
        }
    }
}
