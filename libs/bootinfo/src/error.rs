//! Ошибки валидации [`BootInfo`](crate::BootInfo).
//!
//! Валидация вызывается ядром сразу после сбора данных у загрузчика: лучше
//! остановиться с внятной причиной, чем потом упасть в аллокаторе или в
//! таблицах страниц из-за мусора в карте памяти.

use core::fmt;

/// Почему `BootInfo` признан непригодным.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootInfoError {
    /// Карта памяти пуста: загрузчик ничего не ответил, работать не с чем.
    EmptyMemmap,
    /// Записей было больше, чем помещается в [`MAX_MEMMAP`](crate::MAX_MEMMAP),
    /// и часть отброшена. Работать по обрезанной карте небезопасно.
    MemmapTruncated,
    /// Диапазон нулевой длины.
    ZeroLengthRange { index: usize },
    /// Диапазон выходит за физическое адресное пространство или переполняет `u64`.
    RangeOutOfAddressSpace { index: usize },
    /// Пересекаются диапазоны, которые по протоколу пересекаться не могут.
    ///
    /// Limine гарантирует, что `Usable` и `BootloaderReclaimable` не пересекаются
    /// ни с чем; пересечение с их участием — признак битых данных.
    OverlappingRanges { first: usize, second: usize },
    /// Нет ни одного диапазона `Usable`: загружаться некуда.
    NoUsableMemory,
    /// Смещение HHDM не выровнено на страницу (или равно нулю при наличии).
    HhdmMisaligned(u64),
    /// RSDP передан как нулевой адрес.
    InvalidRsdp,
    /// TSC-частота передана как ноль при наличии ответа загрузчика.
    TscZero,
    /// Framebuffer с нулевым адресом или нулевыми размерами.
    FramebufferZeroSize,
    /// Pitch framebuffer меньше, чем ширина строки в пикселях.
    FramebufferBadPitch { pitch: u32, min_pitch: u64 },
    /// Неизвестная/неподдерживаемая глубина цвета framebuffer.
    FramebufferBadBpp(u16),
    /// Модулей пришло больше, чем помещается в [`MAX_MODULES`](crate::MAX_MODULES).
    ModulesTruncated,
    /// Модуль с нулевым адресом.
    ModuleZeroAddress { index: usize },
    /// Модуль нулевого размера.
    ModuleZeroSize { index: usize },
    /// Два модуля занимают пересекающиеся области памяти.
    OverlappingModules { first: usize, second: usize },
    /// Источник энтропии указан, а самих байт нет.
    MissingEntropy,
}

impl fmt::Display for BootInfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyMemmap => f.write_str("карта памяти пуста"),
            Self::MemmapTruncated => {
                f.write_str("карта памяти обрезана: записей больше, чем помещается")
            }
            Self::ZeroLengthRange { index } => {
                write!(f, "диапазон #{index} карты памяти нулевой длины")
            }
            Self::RangeOutOfAddressSpace { index } => {
                write!(
                    f,
                    "диапазон #{index} карты памяти вне физического адресного пространства"
                )
            }
            Self::OverlappingRanges { first, second } => {
                write!(f, "диапазоны #{first} и #{second} пересекаются")
            }
            Self::NoUsableMemory => f.write_str("в карте памяти нет ни одного диапазона usable"),
            Self::HhdmMisaligned(offset) => {
                write!(f, "смещение HHDM 0x{offset:016x} не выровнено на страницу")
            }
            Self::InvalidRsdp => f.write_str("RSDP передан как нулевой адрес"),
            Self::TscZero => f.write_str("частота TSC передана как ноль"),
            Self::FramebufferZeroSize => {
                f.write_str("framebuffer: нулевой адрес или нулевые размеры")
            }
            Self::FramebufferBadPitch { pitch, min_pitch } => write!(
                f,
                "framebuffer: pitch {pitch} меньше ширины строки {min_pitch}"
            ),
            Self::FramebufferBadBpp(bpp) => {
                write!(f, "framebuffer: неподдерживаемая глубина цвета {bpp} bpp")
            }
            Self::ModulesTruncated => {
                f.write_str("список модулей обрезан: модулей больше, чем помещается")
            }
            Self::ModuleZeroAddress { index } => {
                write!(f, "модуль #{index} имеет нулевой адрес")
            }
            Self::ModuleZeroSize { index } => write!(f, "модуль #{index} имеет нулевой размер"),
            Self::OverlappingModules { first, second } => {
                write!(f, "модули #{first} и #{second} перекрываются")
            }
            Self::MissingEntropy => f.write_str("источник энтропии указан, а байт энтропии нет"),
        }
    }
}

impl core::error::Error for BootInfoError {}
