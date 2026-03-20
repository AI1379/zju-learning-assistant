pub mod common;
pub mod mail;

pub use common::*;
pub use mail::send_email;

#[cfg(target_os = "macos")]
pub mod macos;
