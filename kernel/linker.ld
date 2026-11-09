/* Компоновка ядра AIOS под Limine (base revision 3).
 *
 * Ядро размещается в старшей половине адресного пространства (topmost 2 GiB),
 * как требует протокол Limine: все PT_LOAD-сегменты должны иметь
 * p_vaddr >= 0xffffffff80000000. Загрузчик сам размещает образ в физической
 * памяти и настраивает таблицы страниц на эти виртуальные адреса.
 *
 * Права сегментов заданы через PHDRS, чтобы загрузчик выдал страницам
 * правильные атрибуты (W^X): запросы Limine — RW, .text — RX, .rodata — R,
 * .data/.bss — RW.
 */

OUTPUT_FORMAT(elf64-x86-64)

ENTRY(_start)

PHDRS
{
    limine_requests PT_LOAD FLAGS(0x06); /* read + write */
    text            PT_LOAD FLAGS(0x05); /* read + execute */
    rodata          PT_LOAD FLAGS(0x04); /* read */
    data            PT_LOAD FLAGS(0x06); /* read + write */
}

SECTIONS
{
    . = 0xffffffff80000000;

    /* Запросы и маркеры протокола Limine — отдельным сегментом:
     * загрузчик пишет в них указатели на ответы, а также обнуляет
     * третий компонент тега base revision. */
    .limine_requests : {
        KEEP(*(.limine_requests_start))
        KEEP(*(.limine_requests))
        KEEP(*(.limine_requests_end))
    } :limine_requests

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .text : {
        *(.text .text.*)
    } :text

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .rodata : {
        *(.rodata .rodata.*)
    } :rodata

    .note.gnu.build-id : {
        *(.note.gnu.build-id)
    } :rodata

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .data : {
        *(.data .data.*)
    } :data

    /* .bss должен идти последним в сегменте :data, иначе в бинарник
     * попадут мегабайты нулей. */
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    /* .eh_frame и .note.* ломают загрузку на части хостов;
     * GOT ядру не нужен, но некоторые линковщики его создают. */
    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
        *(.got .got.*)
    }
}
