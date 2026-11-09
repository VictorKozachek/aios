//! Модули, загруженные загрузчиком рядом с ядром (ramdisk, образы сервисов).

/// Один загруженный модуль.
///
/// `path` — строка из ответа загрузчика; она живёт в bootloader-reclaimable
/// памяти, которую ядро не освобождает, поэтому ссылка `'static` безопасна.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Module {
    /// Адрес загрузки (HHDM).
    pub addr: u64,
    /// Размер в байтах.
    pub size: u64,
    /// Путь внутри тома, с ведущим слэшем (например, `/boot/initrd.img`).
    pub path: &'static str,
}

impl Module {
    /// Пустой модуль: значение по умолчанию для массива.
    pub const EMPTY: Self = Self {
        addr: 0,
        size: 0,
        path: "",
    };

    pub const fn new(addr: u64, size: u64, path: &'static str) -> Self {
        Self { addr, size, path }
    }

    /// Конец области модуля; насыщается при переполнении.
    pub const fn end(&self) -> u64 {
        self.addr.saturating_add(self.size)
    }

    /// Конец области; `None`, если `addr + size` переполняет `u64`.
    pub const fn end_exact(&self) -> Option<u64> {
        self.addr.checked_add(self.size)
    }

    /// Пересекается ли область с другим модулем.
    pub const fn overlaps(&self, other: &Self) -> bool {
        match (self.end_exact(), other.end_exact()) {
            (Some(end), Some(other_end)) => self.addr < other_end && other.addr < end,
            _ => false,
        }
    }
}
