//! CRC32C (Castagnoli) — record and segment-header framing.
//!
//! ARCHITECTURE §5.2: `crc32c` only detects torn writes cheaply; the
//! cryptographic integrity of the audit log is a separate blake3 chain (§7.4)
//! and is not implemented here.
//!
//! Castagnoli polynomial `0x1EDC6F41`, reflected implementation (as in
//! iSCSI/SCTP): the least significant polynomial bit goes first, input and
//! output are reflected. No external crates; the lookup table is built at
//! compile time.

/// Castagnoli polynomial `0x1EDC6F41` in reflected form.
const POLY_REFLECTED: u32 = 0x82F6_3B78;

/// 256-entry lookup table, computed at compile time.
const fn make_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ POLY_REFLECTED
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

static TABLE: [u32; 256] = make_table();

/// Returns `crc32c` of `bytes` (init and final xor follow the CRC-32C standard).
///
/// Test vector: `crc32c(b"123456789") == 0xE306_9283`.
pub fn crc32c(bytes: &[u8]) -> u32 {
    crc32c_update(0, bytes)
}

/// Continues a CRC: `crc32c_update(crc32c(a), b) == crc32c(a ++ b)`.
///
/// Used to compute the crc of a segment as a stream without keeping the whole
/// segment in memory.
pub fn crc32c_update(previous: u32, bytes: &[u8]) -> u32 {
    let mut crc = !previous;
    for &byte in bytes {
        let index = ((crc ^ u32::from(byte)) & 0xFF) as usize;
        crc = TABLE[index] ^ (crc >> 8);
    }
    !crc
}
