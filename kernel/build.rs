//! Дополнительные аргументы линковщика для ядра.
//!
//! `.cargo/config.toml` общий для всего workspace (его ведёт лид), поэтому
//! линкерский скрипт и точка входа задаются здесь, на уровне пакета ядра.

use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let linker_script = manifest_dir.join("linker.ld");

    // Скрипт компоновки: старшая половина, отдельные PT_LOAD для W^X.
    println!("cargo:rustc-link-arg-bins=-T");
    println!("cargo:rustc-link-arg-bins={}", linker_script.display());
    // Ядро — ET_EXEC: rustc по умолчанию собирает этот таргет как PIE,
    // а rust-lld 21 с `-pie` и скриптом на 0xffffffff80000000 падает;
    // Limine ждёт обычный исполняемый ELF с фиксированными адресами.
    println!("cargo:rustc-link-arg-bins=-no-pie");
    // Точка входа по протоколу Limine — `_start` из main.rs.
    println!("cargo:rustc-link-arg-bins=-e");
    println!("cargo:rustc-link-arg-bins=_start");

    // Compile-time test switches (`option_env!` in kernel sources). Without
    // these lines cargo may reuse a cached kernel after the variable changes,
    // and a test run silently gets the normal kernel instead of the test one.
    println!("cargo:rerun-if-env-changed=AIOS_EXC_TEST");
    println!("cargo:rerun-if-env-changed=AIOS_MEM_TEST");

    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=build.rs");
}
