#![no_std]
#![forbid(unsafe_code)]

//! Arch-нейтральное описание того, что загрузчик передаёт ядру (docs/ARCHITECTURE.md §3).
//!
//! Здесь только типы, валидация и вычисления: никаких портов, Limine-структур и
//! обращений к памяти. Реальные структуры Limine конвертируются в эти типы в
//! `kernel/src/boot/` — поэтому библиотека тестируется на хосте обычным `cargo test`.
//!
//! Задача #011 (T2a): реализация. Интеграция в ядро — T2b после #001.
//!
//! Что внутри:
//! - [`BootInfo`] — всё, что ядро узнало от загрузчика: карта памяти, HHDM,
//!   режим страниц, framebuffer, RSDP, частота TSC, энтропия, модули;
//! - [`MemmapKind`]/[`MemmapEntry`] — типы диапазонов карты памяти и арифметика
//!   диапазонов без переполнений;
//! - [`Framebuffer`] — геометрия и формат пикселя;
//! - [`Module`] — загруженный загрузчиком модуль;
//! - [`Summary`] — сводка в лог ядра без аллокации (`core::fmt` или буфер).

pub mod error;
pub mod framebuffer;
pub mod info;
pub mod memmap;
pub mod module;
pub mod summary;

pub use error::BootInfoError;
pub use framebuffer::{Channel, Framebuffer, MEMORY_MODEL_RGB};
pub use info::{BootInfo, CpuInfo, EntropySource, PagingMode};
pub use memmap::{MemmapEntry, MemmapKind, MemmapSummary};
pub use module::Module;
pub use summary::Summary;

/// Верхняя граница физического адресного пространства x86-64: 52 бита.
///
/// Больше этого на современных x86-64 не бывает (и заведомо хватает для
/// aarch64/riscv64 в M0). Записи карты памяти за границей — признак мусора.
pub const PHYS_ADDR_LIMIT: u64 = 1 << 52;

/// Минимальная страница MMU: адреса и длины проверяются на границе страниц.
pub const PAGE_SIZE: u64 = 4096;

/// Сколько записей карты памяти помещается в `BootInfo` (в QEMU обычно 20–40).
pub const MAX_MEMMAP: usize = 128;

/// Сколько загруженных модулей помещается в `BootInfo`.
pub const MAX_MODULES: usize = 8;

/// Сколько процессоров помещается в `BootInfo` (MP-ответ Limine).
pub const MAX_CPUS: usize = 8;

/// Длина энтропии в `BootInfo`: 32 байта хватает на seed ключей и API-токенов.
pub const ENTROPY_BYTES: usize = 32;

/// 1 МиБ — для человекочитаемых сводок.
pub const MIB: u64 = 1024 * 1024;

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;
