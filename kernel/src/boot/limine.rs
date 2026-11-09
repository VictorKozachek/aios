//! Протокол Limine: base revision 3.
//!
//! Структуры и константы повторяют `limine-protocol/include/limine.h`
//! из Limine 12.9.6 (brew-пакет `limine`). Мы просим base revision 3 —
//! это ревизия из `docs/ARCHITECTURE.md` §3, и она всё ещё поддерживается
//! загрузчиком.
//!
//! Запросы — статические структуры в секциях `.limine_requests_start`,
//! `.limine_requests`, `.limine_requests_end` (см. `kernel/linker.ld`).
//! Загрузчик находит их до передачи управления, заполняет поле `response`
//! указателем на ответ и обнуляет третий компонент тега base revision,
//! если запрошенная ревизия поддерживается.

use core::ptr;

/// Запрошенная ревизия протокола.
pub const BASE_REVISION: u64 = 3;

/// Маркер начала области запросов (4 машинных слова, как в limine.h).
pub const REQUESTS_START_MARKER: [u64; 4] = [
    0xf6b8f4b39de7d1ae,
    0xfab91a6940fcb9cf,
    0x785c6ed015d3e316,
    0x181e920a7852b9d9,
];

/// Маркер конца области запросов.
pub const REQUESTS_END_MARKER: [u64; 2] = [0xadc0e0531bb10d03, 0x9572709f31764c62];

/// Магия тега base revision.
const BASE_REVISION_MAGIC: [u64; 2] = [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc];

/// Общая магия всех запросов Limine.
const COMMON_MAGIC: [u64; 2] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b];

const BOOTLOADER_INFO_ID: [u64; 2] = [0xf55038d8e2a1202f, 0x279426fcf5f59740];
const STACK_SIZE_ID: [u64; 2] = [0x224ef0460a8e8926, 0xe1cb0fc25f46ea3d];
const HHDM_ID: [u64; 2] = [0x48dcf1cb8ad2b852, 0x63984e959a98244b];
const FRAMEBUFFER_ID: [u64; 2] = [0x9d5827dcd881dd75, 0xa3148604f6fab11b];
const MP_ID: [u64; 2] = [0x95a67b819a1b857e, 0xa0b61b723b6a73e0];
const MEMMAP_ID: [u64; 2] = [0x67cf3d9d378a806f, 0xe304acdfc50c3c62];
const RSDP_ID: [u64; 2] = [0xc5e77b6b397e7b43, 0x27637845accdcf3c];
const TSC_FREQUENCY_ID: [u64; 2] = [0x10f2ee1d87d195e4, 0xf747a2b78f6ddb31];

/// Полный идентификатор запроса: общая магия + магия конкретного запроса.
const fn request_id(specific: [u64; 2]) -> [u64; 4] {
    [COMMON_MAGIC[0], COMMON_MAGIC[1], specific[0], specific[1]]
}

/// Запрос Limine: загрузчик заполняет `response`.
#[repr(C)]
pub struct Request<R> {
    id: [u64; 4],
    revision: u64,
    response: *mut R,
}

// SAFETY: структура состоит из магии, номера ревизии и указателя на ответ,
// который пишет загрузчик до передачи управления ядру. После старта ядра
// запрос только читается, поэтому разделять его между потоками безопасно.
unsafe impl<R> Sync for Request<R> {}

impl<R> Request<R> {
    const fn new(id: [u64; 4]) -> Self {
        Self {
            id,
            revision: 0,
            response: ptr::null_mut(),
        }
    }

    /// Ответ загрузчика; `None`, если запрос не был обслужен.
    pub fn response(&self) -> Option<&'static R> {
        // SAFETY: загрузчик либо оставил null, либо записал валидный указатель
        // на структуру в bootloader-reclaimable памяти, живущую всё время
        // работы ядра (мы её не освобождаем).
        unsafe { self.response.as_ref() }
    }
}

// --- Запросы, которые мы отправляем загрузчику -------------------------

/// Маркер начала области запросов.
#[used]
#[unsafe(link_section = ".limine_requests_start")]
static REQUESTS_START: [u64; 4] = REQUESTS_START_MARKER;

/// Тег запрошенной base revision. Загрузчик перезаписывает третий компонент,
/// поэтому статик изменяемый; читаем его только через сырой указатель.
#[used]
#[unsafe(link_section = ".limine_requests")]
static mut BASE_REVISION_TAG: [u64; 3] = [
    BASE_REVISION_MAGIC[0],
    BASE_REVISION_MAGIC[1],
    BASE_REVISION,
];

/// Информация о загрузчике (имя и версия).
#[used]
#[unsafe(link_section = ".limine_requests")]
static BOOTLOADER_INFO: Request<BootloaderInfoResponse> =
    Request::new(request_id(BOOTLOADER_INFO_ID));

/// Размер стека, который выделит загрузчик.
#[used]
#[unsafe(link_section = ".limine_requests")]
static STACK_SIZE: StackSizeRequest = StackSizeRequest::new(request_id(STACK_SIZE_ID), 256 * 1024);

/// Смещение Higher Half Direct Map.
#[used]
#[unsafe(link_section = ".limine_requests")]
static HHDM: Request<HhdmResponse> = Request::new(request_id(HHDM_ID));

/// Framebuffer: единственный канал вывода «на экран».
#[used]
#[unsafe(link_section = ".limine_requests")]
static FRAMEBUFFER: Request<FramebufferResponse> = Request::new(request_id(FRAMEBUFFER_ID));

/// Multiprocessor: сколько ядер видит загрузчик и какой LAPIC ID у BSP.
#[used]
#[unsafe(link_section = ".limine_requests")]
static MP: Request<MpResponse> = Request::new(request_id(MP_ID));

/// Карта памяти.
#[used]
#[unsafe(link_section = ".limine_requests")]
static MEMMAP: Request<MemmapResponse> = Request::new(request_id(MEMMAP_ID));

/// Корень ACPI (RSDP).
#[used]
#[unsafe(link_section = ".limine_requests")]
static RSDP: Request<RsdpResponse> = Request::new(request_id(RSDP_ID));

/// Частота TSC.
#[used]
#[unsafe(link_section = ".limine_requests")]
static TSC_FREQUENCY: Request<TscFrequencyResponse> =
    Request::new(request_id(TSC_FREQUENCY_ID));

/// Маркер конца области запросов.
#[used]
#[unsafe(link_section = ".limine_requests_end")]
static REQUESTS_END: [u64; 2] = REQUESTS_END_MARKER;

// --- Ответы загрузчика --------------------------------------------------

#[repr(C)]
pub struct BootloaderInfoResponse {
    pub revision: u64,
    pub name: *const u8,
    pub version: *const u8,
}

#[repr(C)]
pub struct StackSizeResponse {
    pub revision: u64,
}

#[repr(C)]
pub struct StackSizeRequest {
    id: [u64; 4],
    revision: u64,
    response: *mut StackSizeResponse,
    stack_size: u64,
}

// SAFETY: тот же аргумент, что и для `Request<R>`: после старта структура
// только читается.
unsafe impl Sync for StackSizeRequest {}

impl StackSizeRequest {
    const fn new(id: [u64; 4], stack_size: u64) -> Self {
        Self {
            id,
            revision: 0,
            response: ptr::null_mut(),
            stack_size,
        }
    }
}

#[repr(C)]
pub struct HhdmResponse {
    pub revision: u64,
    pub offset: u64,
}

#[repr(C)]
pub struct FramebufferResponse {
    pub revision: u64,
    pub framebuffer_count: u64,
    pub framebuffers: *mut *mut Framebuffer,
}

#[repr(C)]
pub struct Framebuffer {
    pub address: *mut u8,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
    pub unused: [u8; 7],
    pub edid_size: u64,
    pub edid: *mut u8,
    /// Поля ниже — response revision 1; в старых ответах там мусор,
    /// поэтому мы их не читаем.
    pub mode_count: u64,
    pub modes: *mut *mut VideoMode,
}

#[repr(C)]
pub struct VideoMode {
    pub pitch: u64,
    pub width: u64,
    pub height: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
}

#[repr(C)]
pub struct MpResponse {
    pub revision: u64,
    pub flags: u32,
    pub bsp_lapic_id: u32,
    pub cpu_count: u64,
    pub cpus: *mut *mut MpInfo,
}

#[repr(C)]
pub struct MpInfo {
    pub processor_id: u32,
    pub lapic_id: u32,
    pub reserved: u64,
    pub goto_address: u64,
    pub extra_argument: u64,
}

#[repr(C)]
pub struct MemmapResponse {
    pub revision: u64,
    pub entry_count: u64,
    pub entries: *mut *mut MemmapEntry,
}

#[repr(C)]
pub struct MemmapEntry {
    pub base: u64,
    pub length: u64,
    pub kind: u64,
}

// Типы записей карты памяти (limine.h).
pub const MEMMAP_USABLE: u64 = 0;
pub const MEMMAP_RESERVED: u64 = 1;
pub const MEMMAP_ACPI_RECLAIMABLE: u64 = 2;
pub const MEMMAP_ACPI_NVS: u64 = 3;
pub const MEMMAP_BAD_MEMORY: u64 = 4;
pub const MEMMAP_BOOTLOADER_RECLAIMABLE: u64 = 5;
pub const MEMMAP_EXECUTABLE_AND_MODULES: u64 = 6;
pub const MEMMAP_FRAMEBUFFER: u64 = 7;
pub const MEMMAP_RESERVED_MAPPED: u64 = 8;

/// Человекочитаемое имя типа записи карты памяти.
pub fn memmap_type_name(kind: u64) -> &'static str {
    match kind {
        MEMMAP_USABLE => "usable",
        MEMMAP_RESERVED => "reserved",
        MEMMAP_ACPI_RECLAIMABLE => "acpi-reclaimable",
        MEMMAP_ACPI_NVS => "acpi-nvs",
        MEMMAP_BAD_MEMORY => "bad-memory",
        MEMMAP_BOOTLOADER_RECLAIMABLE => "bootloader-reclaimable",
        MEMMAP_EXECUTABLE_AND_MODULES => "kernel-and-modules",
        MEMMAP_FRAMEBUFFER => "framebuffer",
        MEMMAP_RESERVED_MAPPED => "reserved-mapped",
        _ => "unknown",
    }
}

#[repr(C)]
pub struct RsdpResponse {
    pub revision: u64,
    pub address: *mut u8,
}

#[repr(C)]
pub struct TscFrequencyResponse {
    pub revision: u64,
    pub frequency: u64,
}

// --- Доступ к ответам ---------------------------------------------------

/// Поддерживает ли загрузчик запрошенную base revision, и какую ревизию
/// он реально применил.
pub fn base_revision() -> (bool, u64) {
    // SAFETY: читаем статик, который загрузчик мог изменить; создавать
    // ссылку на `static mut` нельзя, поэтому копируем значение по указателю.
    let tag = unsafe { *(&raw const BASE_REVISION_TAG) };
    (tag[2] == 0, tag[1])
}

pub fn bootloader_info() -> Option<&'static BootloaderInfoResponse> {
    BOOTLOADER_INFO.response()
}

pub fn hhdm() -> Option<&'static HhdmResponse> {
    HHDM.response()
}

pub fn primary_framebuffer() -> Option<&'static Framebuffer> {
    let response = FRAMEBUFFER.response()?;
    if response.framebuffer_count == 0 || response.framebuffers.is_null() {
        return None;
    }
    // SAFETY: загрузчик гарантирует массив из framebuffer_count указателей.
    let first = unsafe { *response.framebuffers };
    unsafe { first.as_ref() }
}

pub fn memmap() -> Option<&'static MemmapResponse> {
    MEMMAP.response()
}

/// Записи карты памяти в виде среза.
pub fn memmap_entries(response: &'static MemmapResponse) -> &'static [&'static MemmapEntry] {
    if response.entry_count == 0 || response.entries.is_null() {
        return &[];
    }
    // SAFETY: загрузчик гарантирует массив из entry_count ненулевых указателей
    // на записи, живущие всё время работы ядра. Представление `&T` и `*mut T`
    // совпадает (гарантия Rust для ссылок на Sized-типы), поэтому массив
    // указателей можно читать как срез ссылок.
    unsafe {
        core::slice::from_raw_parts(
            response.entries as *const &'static MemmapEntry,
            response.entry_count as usize,
        )
    }
}

pub fn rsdp() -> Option<&'static RsdpResponse> {
    RSDP.response()
}

pub fn tsc_frequency() -> Option<&'static TscFrequencyResponse> {
    TSC_FREQUENCY.response()
}

pub fn mp() -> Option<&'static MpResponse> {
    MP.response()
}

/// C-строка из загрузчика (`name`, `version` и т. п.).
///
/// # Safety
///
/// `ptr` должен указывать на NUL-терминированную строку, живущую всё время
/// работы ядра (так обещает протокол Limine).
pub unsafe fn cstr(ptr: *const u8) -> &'static str {
    if ptr.is_null() {
        return "";
    }
    let mut len = 0usize;
    // SAFETY: NUL-терминированная строка загрузчика.
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
            // Защита от бесконечного сканирования при битом указателе.
            if len > 4096 {
                break;
            }
        }
        let bytes = core::slice::from_raw_parts(ptr, len);
        core::str::from_utf8(bytes).unwrap_or("<non-utf8>")
    }
}
