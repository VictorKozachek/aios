#![no_std]

//! Форматы подсистемы памяти AIOS:
//! сегменты и записи (ARCHITECTURE.md §5.2), checkpoint (§5.3),
//! каноническая сериализация (§5.12). Реализация — задача T5.

#[cfg(test)]
extern crate std;
