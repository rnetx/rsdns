mod basic;
mod control;
mod dialer;
mod interface;

#[cfg(feature = "upstream-dialer-socks5")]
mod socks5;

mod socksaddr;

use basic::*;
use control::*;
pub(crate) use dialer::*;
pub(super) use interface::*;

#[cfg(feature = "upstream-dialer-socks5")]
use socks5::*;

pub(super) use control::set_interface;
pub use socksaddr::*;

pub fn support_socks5_dialer() -> bool {
    cfg_if::cfg_if! {
      if #[cfg(feature = "upstream-dialer-socks5")] {
        true
      } else {
        false
      }
    }
}
