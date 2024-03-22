mod adapter;
mod common;
pub mod listener;
#[allow(hidden_glob_reexports)]
mod log;
pub mod manager;
pub mod option;
pub mod plugin;
pub mod upstream;
#[allow(hidden_glob_reexports)]
mod workflow;

pub use listener::*;
pub use manager::*;
pub use option::*;
pub use plugin::*;
pub use upstream::*;

/// Get the version of the application.
/// Return: (Version, Git Commit ID)
pub fn get_app_version() -> (String, String) {
    let version = format!("v{}", env!("CARGO_PKG_VERSION")); // example: v0.0.1
    let git_commit_id = env!("GIT_COMMIT_ID").to_string(); // example: cf3e4e3
    (version, git_commit_id)
}
