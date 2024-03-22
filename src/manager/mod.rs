#[cfg(feature = "api")]
mod api;

mod manager;
mod upstream;

#[cfg(feature = "api")]
use api::*;

pub use manager::*;
use upstream::*;

pub fn support_api() -> bool {
    cfg_if::cfg_if! {
      if #[cfg(feature = "api")] {
        true
      } else {
        false
      }
    }
}
