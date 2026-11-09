//! Architecture-specific code.
//!
//! The rest of the kernel talks to the CPU only through this module: ports,
//! descriptor tables and exception entry all live under `arch/<name>/`.

pub mod x86_64;
