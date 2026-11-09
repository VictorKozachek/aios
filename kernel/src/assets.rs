//! Baked design assets for the splash (task T7).
//!
//! The kernel has no PNG decoder: `tools/png2rust.py` bakes the designer's
//! masks into plain byte arrays at build time. The files live in
//! `kernel/assets/` and are included as modules here.

#[path = "../assets/logo_aios.rs"]
pub mod logo_aios;

#[path = "../assets/wordmark_aios.rs"]
pub mod wordmark_aios;

#[path = "../assets/atlas_jbm15.rs"]
pub mod atlas_jbm15;
