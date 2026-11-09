//! Карта памяти: типы диапазонов, арифметика и сводка по типам.
//!
//! Типы повторяют значения протокола Limine (base revision 3), но без
//! зависимости от его структур: конвертация `raw → [`MemmapKind`]` живёт здесь,
//! чтобы ядро не дублировало магические числа, а тесты проверяли соответствие.

use crate::PHYS_ADDR_LIMIT;

/// Тип диапазона физической памяти.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemmapKind {
    /// Свободная RAM: отдаётся аллокатору.
    Usable,
    /// Зарезервировано прошивкой/железом: не трогать.
    Reserved,
    /// ACPI-данные, которые можно освободить, когда они станут не нужны.
    AcpiReclaimable,
    /// ACPI non-volatile storage: не трогать.
    AcpiNvs,
    /// Плохая память: обращаться нельзя.
    BadMemory,
    /// Память загрузчика/прошивки, которую можно освободить после инициализации.
    BootloaderReclaimable,
    /// Исполняемый образ ядра и модули (справочно, не для поиска адресов).
    ExecutableAndModules,
    /// Memory-mapped framebuffer (справочно, адрес берётся из запроса fb).
    Framebuffer,
    /// Область, которую загрузчик уже отобразил в HHDM (ACPI, SMBIOS, EFI runtime).
    ReservedMapped,
    /// Неизвестный тип: сохраняем сырое значение, чтобы не терять данные.
    Other(u64),
}

impl MemmapKind {
    /// Тип по значению из протокола Limine (`limine_memmap_entry.type`).
    pub const fn from_raw(raw: u64) -> Self {
        match raw {
            0 => Self::Usable,
            1 => Self::Reserved,
            2 => Self::AcpiReclaimable,
            3 => Self::AcpiNvs,
            4 => Self::BadMemory,
            5 => Self::BootloaderReclaimable,
            6 => Self::ExecutableAndModules,
            7 => Self::Framebuffer,
            8 => Self::ReservedMapped,
            other => Self::Other(other),
        }
    }

    /// Обратное преобразование в значение протокола.
    pub const fn raw(self) -> u64 {
        match self {
            Self::Usable => 0,
            Self::Reserved => 1,
            Self::AcpiReclaimable => 2,
            Self::AcpiNvs => 3,
            Self::BadMemory => 4,
            Self::BootloaderReclaimable => 5,
            Self::ExecutableAndModules => 6,
            Self::Framebuffer => 7,
            Self::ReservedMapped => 8,
            Self::Other(raw) => raw,
        }
    }

    /// Имя для лога ядра.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Usable => "usable",
            Self::Reserved => "reserved",
            Self::AcpiReclaimable => "acpi-reclaimable",
            Self::AcpiNvs => "acpi-nvs",
            Self::BadMemory => "bad-memory",
            Self::BootloaderReclaimable => "bootloader-reclaimable",
            Self::ExecutableAndModules => "executable-and-modules",
            Self::Framebuffer => "framebuffer",
            Self::ReservedMapped => "reserved-mapped",
            Self::Other(_) => "other",
        }
    }

    /// Свободная память, которую можно отдавать аллокатору сразу.
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Usable)
    }

    /// Память, которую можно вернуть в оборот после инициализации подсистем.
    pub const fn is_reclaimable(self) -> bool {
        matches!(self, Self::AcpiReclaimable | Self::BootloaderReclaimable)
    }

    /// Память «не для записи»: железо, прошивка, ACPI, плохие страницы.
    pub const fn is_reserved_like(self) -> bool {
        matches!(
            self,
            Self::Reserved
                | Self::AcpiReclaimable
                | Self::AcpiNvs
                | Self::BadMemory
                | Self::ReservedMapped
        )
    }

    /// Пересечение с чем-либо запрещено протоколом (usable и bootloader-reclaimable).
    pub const fn is_disjoint_guaranteed(self) -> bool {
        matches!(self, Self::Usable | Self::BootloaderReclaimable)
    }
}

/// Один диапазон карты памяти: `[base, base + length)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemmapEntry {
    /// Физическое начало диапазона.
    pub base: u64,
    /// Длина в байтах; ноль — невалидный диапазон.
    pub length: u64,
    /// Тип диапазона.
    pub kind: MemmapKind,
}

impl MemmapEntry {
    /// Пустая запись: используется как значение по умолчанию в массиве.
    pub const EMPTY: Self = Self {
        base: 0,
        length: 0,
        kind: MemmapKind::Reserved,
    };

    pub const fn new(base: u64, length: u64, kind: MemmapKind) -> Self {
        Self { base, length, kind }
    }

    /// Конец диапазона; при переполнении насыщается, а не паникует.
    pub const fn end(&self) -> u64 {
        self.base.saturating_add(self.length)
    }

    /// Конец диапазона; `None`, если `base + length` переполняет `u64`.
    pub const fn end_exact(&self) -> Option<u64> {
        self.base.checked_add(self.length)
    }

    /// Пустой диапазон (также считается невалидным в [`validate`](crate::BootInfo::validate)).
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Лежит ли адрес внутри `[base, base + length)`.
    pub const fn contains(&self, address: u64) -> bool {
        match self.end_exact() {
            Some(end) => address >= self.base && address < end,
            None => false,
        }
    }

    /// Пересекаются ли диапазоны (полуинтервалы).
    pub const fn overlaps(&self, other: &Self) -> bool {
        match (self.end_exact(), other.end_exact()) {
            (Some(end), Some(other_end)) => self.base < other_end && other.base < end,
            _ => false,
        }
    }

    /// Лежит ли диапазон целиком в физическом адресном пространстве.
    pub const fn fits_phys_space(&self) -> bool {
        match self.end_exact() {
            Some(end) => self.base < PHYS_ADDR_LIMIT && end <= PHYS_ADDR_LIMIT,
            None => false,
        }
    }
}

/// Суммы по типам карты памяти. Считаются насыщающей арифметикой, чтобы
/// битые данные не превращались в панику, а помечались флагом `overflowed`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemmapSummary {
    /// Сколько записей просуммировано.
    pub entries: usize,
    /// Свободная память.
    pub usable: u64,
    /// Зарезервированная память.
    pub reserved: u64,
    /// ACPI, которую можно освободить.
    pub acpi_reclaimable: u64,
    /// ACPI NVS.
    pub acpi_nvs: u64,
    /// Плохая память.
    pub bad_memory: u64,
    /// Память загрузчика.
    pub bootloader_reclaimable: u64,
    /// Исполняемый образ и модули.
    pub executable_and_modules: u64,
    /// Framebuffer.
    pub framebuffer: u64,
    /// Уже отображённые области (ACPI/SMBIOS/EFI runtime).
    pub reserved_mapped: u64,
    /// Неизвестные типы.
    pub other: u64,
    /// Хотя бы одна сумма насытилась (данные битые).
    pub overflowed: bool,
}

impl MemmapSummary {
    /// Сумма по всем записям без аллокации.
    pub fn from_entries(entries: &[MemmapEntry]) -> Self {
        let mut summary = Self::default();
        for entry in entries {
            summary.add(entry);
        }
        summary
    }

    /// Учесть одну запись.
    pub fn add(&mut self, entry: &MemmapEntry) {
        self.entries += 1;
        let (target, overflowed) = match entry.kind {
            MemmapKind::Usable => (&mut self.usable, &mut self.overflowed),
            MemmapKind::Reserved => (&mut self.reserved, &mut self.overflowed),
            MemmapKind::AcpiReclaimable => (&mut self.acpi_reclaimable, &mut self.overflowed),
            MemmapKind::AcpiNvs => (&mut self.acpi_nvs, &mut self.overflowed),
            MemmapKind::BadMemory => (&mut self.bad_memory, &mut self.overflowed),
            MemmapKind::BootloaderReclaimable => {
                (&mut self.bootloader_reclaimable, &mut self.overflowed)
            }
            MemmapKind::ExecutableAndModules => {
                (&mut self.executable_and_modules, &mut self.overflowed)
            }
            MemmapKind::Framebuffer => (&mut self.framebuffer, &mut self.overflowed),
            MemmapKind::ReservedMapped => (&mut self.reserved_mapped, &mut self.overflowed),
            MemmapKind::Other(_) => (&mut self.other, &mut self.overflowed),
        };
        match target.checked_add(entry.length) {
            Some(total) => *target = total,
            None => {
                *target = u64::MAX;
                *overflowed = true;
            }
        }
    }

    /// Свободная память в МиБ.
    pub const fn usable_mib(&self) -> u64 {
        self.usable / crate::MIB
    }

    /// Вся учтённая память (насыщающе).
    pub const fn total_mapped(&self) -> u64 {
        self.usable
            .saturating_add(self.reserved)
            .saturating_add(self.acpi_reclaimable)
            .saturating_add(self.acpi_nvs)
            .saturating_add(self.bad_memory)
            .saturating_add(self.bootloader_reclaimable)
            .saturating_add(self.executable_and_modules)
            .saturating_add(self.framebuffer)
            .saturating_add(self.reserved_mapped)
            .saturating_add(self.other)
    }
}
