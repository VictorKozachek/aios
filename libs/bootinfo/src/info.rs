//! `BootInfo` — то, что ядро узнало от загрузчика, и его валидация.

use crate::error::BootInfoError;
use crate::framebuffer::Framebuffer;
use crate::memmap::{MemmapEntry, MemmapKind, MemmapSummary};
use crate::module::Module;
use crate::{ENTROPY_BYTES, MAX_MEMMAP, MAX_MODULES, PAGE_SIZE};

/// Режим страниц, включённый загрузчиком.
///
/// Для x86-64 это 4 или 5 уровней. На других архитектурах сборщик `BootInfo`
/// отображает свой режим в 4/5 уровней, а незнакомый — в [`PagingMode::Unknown`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PagingMode {
    /// Загрузчик не сообщил режим.
    #[default]
    Unknown,
    /// Четырёхуровневые таблицы страниц (PML4).
    FourLevel,
    /// Пятиуровневые таблицы страниц (PML5).
    FiveLevel,
}

impl PagingMode {
    /// Режим по сырому значению из протокола (x86-64: 0 — 4 уровня, 1 — 5 уровней).
    pub const fn from_raw(raw: u64) -> Self {
        match raw {
            0 => Self::FourLevel,
            1 => Self::FiveLevel,
            _ => Self::Unknown,
        }
    }

    /// Уровней в таблицах страниц; `None`, если режим неизвестен.
    pub const fn levels(self) -> Option<u8> {
        match self {
            Self::FourLevel => Some(4),
            Self::FiveLevel => Some(5),
            Self::Unknown => None,
        }
    }

    /// Имя для лога ядра.
    pub const fn name(self) -> &'static str {
        match self {
            Self::FourLevel => "4-level",
            Self::FiveLevel => "5-level",
            Self::Unknown => "unknown",
        }
    }
}

/// Откуда взялись 32 байта энтропии.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EntropySource {
    /// Энтропии нет: подсистемы должны сами её добыть до работы с ключами.
    #[default]
    None,
    /// Запрос Entropy протокола Limine (лучший вариант на M0).
    Bootloader,
    /// Инструкция `RDRAND` (проверяется CPUID).
    Rdrand,
    /// Джиттер TSC: слабый источник, годится только на seed.
    TscJitter,
}

impl EntropySource {
    /// Имя для лога ядра.
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Bootloader => "bootloader",
            Self::Rdrand => "rdrand",
            Self::TscJitter => "tsc-jitter",
        }
    }

    /// Достаточно ли источник силён для ключей (без дополнительного смешивания).
    pub const fn is_strong(self) -> bool {
        matches!(self, Self::Bootloader | Self::Rdrand)
    }
}

/// Всё, что загрузчик передал ядру на входе (ARCHITECTURE.md §3).
///
/// Массивы фиксированной длины: на входе в ядро нет аллокатора. Верхние поля
/// `*_len` — сколько элементов заполнено; остальное — [`MemmapEntry::EMPTY`] и
/// [`Module::EMPTY`]. Переполнение помечается флагом `*_truncated` и делает
/// `BootInfo` невалидным: работать по обрезанной карте памяти нельзя.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootInfo {
    /// Карта памяти (сииски диапазонов в порядке загрузчика).
    pub memmap: [MemmapEntry; MAX_MEMMAP],
    /// Сколько записей карты памяти заполнено.
    pub memmap_len: u16,
    /// Записей было больше `MAX_MEMMAP`, хвост отброшен.
    pub memmap_truncated: bool,
    /// Смещение HHDM («вся физическая память в VA»); `None` — загрузчик не дал.
    pub hhdm: Option<u64>,
    /// Режим страниц, включённый загрузчиком.
    pub paging_mode: PagingMode,
    /// Основной framebuffer; `None` — экрана нет (остаётся serial/debugcon).
    pub framebuffer: Option<Framebuffer>,
    /// Физический адрес RSDP (корень ACPI). В base revision 3 — физический.
    pub rsdp: Option<u64>,
    /// Частота TSC в герцах.
    pub tsc_hz: Option<u64>,
    /// 32 байта энтропии от загрузчика/железа, если удалось добыть.
    pub entropy: Option<[u8; ENTROPY_BYTES]>,
    /// Источник энтропии.
    pub entropy_source: EntropySource,
    /// Загруженные модули.
    pub modules: [Module; MAX_MODULES],
    /// Сколько модулей заполнено.
    pub modules_len: u8,
    /// Модулей было больше `MAX_MODULES`, хвост отброшен.
    pub modules_truncated: bool,
}

impl BootInfo {
    /// Пустой `BootInfo`: всё «нет данных», массивы заполнены `EMPTY`.
    pub const fn new() -> Self {
        Self {
            memmap: [MemmapEntry::EMPTY; MAX_MEMMAP],
            memmap_len: 0,
            memmap_truncated: false,
            hhdm: None,
            paging_mode: PagingMode::Unknown,
            framebuffer: None,
            rsdp: None,
            tsc_hz: None,
            entropy: None,
            entropy_source: EntropySource::None,
            modules: [Module::EMPTY; MAX_MODULES],
            modules_len: 0,
            modules_truncated: false,
        }
    }

    /// Заполненные записи карты памяти.
    pub fn memmap(&self) -> &[MemmapEntry] {
        let len = core::cmp::min(self.memmap_len as usize, MAX_MEMMAP);
        &self.memmap[..len]
    }

    /// Заполненные модули.
    pub fn modules(&self) -> &[Module] {
        let len = core::cmp::min(self.modules_len as usize, MAX_MODULES);
        &self.modules[..len]
    }

    /// Добавить запись карты памяти; `false`, если места нет (флаг `truncated`).
    pub fn push_memmap(&mut self, entry: MemmapEntry) -> bool {
        let len = self.memmap_len as usize;
        if len >= MAX_MEMMAP {
            self.memmap_truncated = true;
            return false;
        }
        self.memmap[len] = entry;
        self.memmap_len = (len + 1) as u16;
        true
    }

    /// Добавить модуль; `false`, если места нет (флаг `truncated`).
    pub fn push_module(&mut self, module: Module) -> bool {
        let len = self.modules_len as usize;
        if len >= MAX_MODULES {
            self.modules_truncated = true;
            return false;
        }
        self.modules[len] = module;
        self.modules_len = (len + 1) as u8;
        true
    }

    /// Есть ли HHDM.
    pub const fn hhdm_available(&self) -> bool {
        self.hhdm.is_some()
    }

    /// Сумма по типам карты памяти.
    pub fn summary(&self) -> MemmapSummary {
        MemmapSummary::from_entries(self.memmap())
    }

    /// Вся свободная память в байтах.
    pub fn usable_bytes(&self) -> u64 {
        self.summary().usable
    }

    /// Самый большой диапазон usable (первый по порядку при равных длинах).
    pub fn largest_usable(&self) -> Option<&MemmapEntry> {
        let mut best: Option<&MemmapEntry> = None;
        for entry in self.memmap() {
            if !entry.kind.is_usable() {
                continue;
            }
            let better = match best {
                None => true,
                Some(current) => entry.length > current.length,
            };
            if better {
                best = Some(entry);
            }
        }
        best
    }

    /// Первый диапазон карты, содержащий адрес RSDP (обычно ACPI/NVS/reserved-mapped).
    pub fn rsdp_entry(&self) -> Option<&MemmapEntry> {
        let address = self.rsdp?;
        self.memmap().iter().find(|entry| entry.contains(address))
    }

    /// Первая пара пересекающихся диапазонов (для диагностики и тестов).
    ///
    /// Протокол разрешает непересечение не всем типам, поэтому само по себе
    /// пересечение не делает `BootInfo` невалидным — это проверяет
    /// [`validate`](Self::validate) по правилам типов.
    pub fn overlaps(&self) -> Option<(usize, usize)> {
        let entries = self.memmap();
        for first in 0..entries.len() {
            for second in (first + 1)..entries.len() {
                if entries[first].overlaps(&entries[second]) {
                    return Some((first, second));
                }
            }
        }
        None
    }

    /// Первая пара пересекающихся модулей.
    pub fn overlapping_modules(&self) -> Option<(usize, usize)> {
        let modules = self.modules();
        for first in 0..modules.len() {
            for second in (first + 1)..modules.len() {
                if modules[first].overlaps(&modules[second]) {
                    return Some((first, second));
                }
            }
        }
        None
    }

    /// Проверить `BootInfo` перед использованием ядром.
    ///
    /// Отвергает: пустую/обрезанную карту, нулевые и выходящие за физическое
    /// адресное пространство диапазоны, пересечения с участием `usable`/
    /// `bootloader-reclaimable`, отсутствие usable, невыровненный HHDM, нулевой
    /// framebuffer или неверный pitch, нулевой RSDP, нулевую частоту TSC, битые
    /// модули и противоречивое описание энтропии.
    pub fn validate(&self) -> Result<(), BootInfoError> {
        if self.memmap_truncated || self.memmap_len as usize > MAX_MEMMAP {
            return Err(BootInfoError::MemmapTruncated);
        }
        let entries = self.memmap();
        if entries.is_empty() {
            return Err(BootInfoError::EmptyMemmap);
        }

        for (index, entry) in entries.iter().enumerate() {
            if entry.length == 0 {
                return Err(BootInfoError::ZeroLengthRange { index });
            }
            if !entry.fits_phys_space() {
                return Err(BootInfoError::RangeOutOfAddressSpace { index });
            }
        }

        if !entries.iter().any(|entry| entry.kind.is_usable()) {
            return Err(BootInfoError::NoUsableMemory);
        }

        for first in 0..entries.len() {
            for second in (first + 1)..entries.len() {
                let a = &entries[first];
                let b = &entries[second];
                if a.overlaps(b)
                    && (a.kind.is_disjoint_guaranteed() || b.kind.is_disjoint_guaranteed())
                {
                    return Err(BootInfoError::OverlappingRanges { first, second });
                }
            }
        }

        if let Some(offset) = self.hhdm
            && (offset == 0 || offset % PAGE_SIZE != 0)
        {
            return Err(BootInfoError::HhdmMisaligned(offset));
        }

        if let Some(framebuffer) = self.framebuffer {
            framebuffer.validate()?;
        }

        if let Some(rsdp) = self.rsdp
            && rsdp == 0
        {
            return Err(BootInfoError::InvalidRsdp);
        }

        if let Some(tsc_hz) = self.tsc_hz
            && tsc_hz == 0
        {
            return Err(BootInfoError::TscZero);
        }

        if self.entropy.is_none() && self.entropy_source != EntropySource::None {
            return Err(BootInfoError::MissingEntropy);
        }

        if self.modules_truncated || self.modules_len as usize > MAX_MODULES {
            return Err(BootInfoError::ModulesTruncated);
        }
        let modules = self.modules();
        for (index, module) in modules.iter().enumerate() {
            if module.addr == 0 {
                return Err(BootInfoError::ModuleZeroAddress { index });
            }
            if module.size == 0 {
                return Err(BootInfoError::ModuleZeroSize { index });
            }
        }
        if let Some((first, second)) = self.overlapping_modules() {
            return Err(BootInfoError::OverlappingModules { first, second });
        }

        Ok(())
    }

    /// Тип диапазона, в котором лежит RSDP (справочно для лога).
    pub fn rsdp_kind(&self) -> Option<MemmapKind> {
        self.rsdp_entry().map(|entry| entry.kind)
    }
}

impl Default for BootInfo {
    fn default() -> Self {
        Self::new()
    }
}
