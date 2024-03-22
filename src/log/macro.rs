#[macro_export]
macro_rules! debug {
    ($logger:expr, { tracker = $tracker:expr }, $($arg:tt)* ) => {
      $logger.log_with_tracker($crate::log::Level::Debug, &$tracker, format_args!($($arg)*))
    };

    ($logger:expr, { option_tracker = $option_tracker:expr }, $($arg:tt)* ) => {
      $logger.log_with_option_tracker($crate::log::Level::Debug, $option_tracker, format_args!($($arg)*))
    };

    ($logger:expr, $tracker:ident, $($arg:tt)*) => {
      $logger.log_with_tracker($crate::log::Level::Debug, &$tracker, format_args!($($arg)*))
    };

    ($logger:expr, $($arg:tt)*) => {
      $logger.log($crate::log::Level::Debug, format_args!($($arg)*))
    }
}

#[macro_export]
macro_rules! info {
    ($logger:expr, { tracker = $tracker:expr }, $($arg:tt)* ) => {
      $logger.log_with_tracker($crate::log::Level::Info, &$tracker, format_args!($($arg)*))
    };

    ($logger:expr, { option_tracker = $option_tracker:expr }, $($arg:tt)* ) => {
      $logger.log_with_option_tracker($crate::log::Level::Info, $option_tracker, format_args!($($arg)*))
    };

    ($logger:expr, $tracker:ident, $($arg:tt)*) => {
      $logger.log_with_tracker($crate::log::Level::Info, &$tracker, format_args!($($arg)*))
    };

    ($logger:expr, $($arg:tt)*) => {
      $logger.log($crate::log::Level::Info, format_args!($($arg)*))
    }
}

#[macro_export]
macro_rules! warn {
    ($logger:expr, { tracker = $tracker:expr }, $($arg:tt)* ) => {
      $logger.log_with_tracker($crate::log::Level::Warn, &$tracker, format_args!($($arg)*))
    };

    ($logger:expr, { option_tracker = $option_tracker:expr }, $($arg:tt)* ) => {
      $logger.log_with_option_tracker($crate::log::Level::Warn, $option_tracker, format_args!($($arg)*))
    };

    ($logger:expr, $tracker:ident, $($arg:tt)*) => {
      $logger.log_with_tracker($crate::log::Level::Warn, &$tracker, format_args!($($arg)*))
    };

    ($logger:expr, $($arg:tt)*) => {
      $logger.log($crate::log::Level::Warn, format_args!($($arg)*))
    }
}

#[macro_export]
macro_rules! error {
    ($logger:expr, { tracker = $tracker:expr }, $($arg:tt)* ) => {
      $logger.log_with_tracker($crate::log::Level::Error, &$tracker, format_args!($($arg)*))
    };

    ($logger:expr, { option_tracker = $option_tracker:expr }, $($arg:tt)* ) => {
      $logger.log_with_option_tracker($crate::log::Level::Error, $option_tracker, format_args!($($arg)*))
    };

    ($logger:expr, $tracker:ident, $($arg:tt)*) => {
      $logger.log_with_tracker($crate::log::Level::Error, &$tracker, format_args!($($arg)*))
    };

    ($logger:expr, $($arg:tt)*) => {
      $logger.log($crate::log::Level::Error, format_args!($($arg)*))
    }
}

#[macro_export]
macro_rules! fatal {
    ($logger:expr, { tracker = $tracker:expr }, $($arg:tt)* ) => {
      $logger.log_with_tracker($crate::log::Level::Fatal, &$tracker, format_args!($($arg)*))
    };

    ($logger:expr, { option_tracker = $option_tracker:expr }, $($arg:tt)* ) => {
      $logger.log_with_option_tracker($crate::log::Level::Fatal, $option_tracker, format_args!($($arg)*))
    };

    ($logger:expr, $tracker:ident, $($arg:tt)*) => {
      $logger.log_with_tracker($crate::log::Level::Fatal, &$tracker, format_args!($($arg)*))
    };

    ($logger:expr, $($arg:tt)*) => {
      $logger.log($crate::log::Level::Fatal, format_args!($($arg)*))
    }
}
