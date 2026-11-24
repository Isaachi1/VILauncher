//! VILauncher's core: runs VIctory (VILinux) or XCL (LinuxCL) on Linux.
//!
//! VIctory and XCL are Windows programs. Both already detect Wine (ntdll's
//! `wine_get_version`) and adapt to it, and XCL's D3D12 code is written against
//! vkd3d-proton. On Linux they run under Wine or Proton, with vkd3d-proton as
//! the D3D12 implementation (D3D12 -> Vulkan). The title's x86-64 code still
//! runs directly on the CPU: there is no CPU emulation anywhere in the chain.
//!
//! This crate turns a [`config::Config`] into that launch: it finds Proton or
//! prepares a Wine prefix, translates Linux paths to the `Z:\` paths the
//! program sees, builds each backend's documented command line and
//! environment, and checks the machine first ([`doctor`]).
//!
//! It only runs an unpacked folder of a game the user owns. Packaged titles
//! (XVC / MSIXVC) are refused; nothing is unpacked, decrypted or patched.

pub mod backends;
pub mod config;
pub mod doctor;
pub mod gamedir;
pub mod launch;
pub mod paths;
pub mod runners;

/// Everything that can stop a launch, as a message for the user.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Game(String),
    #[error("{0}")]
    Runner(String),
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
pub(crate) mod testutil;
