//! Deliberately small x86-64 (long mode) encoder: only forms whose encoding is unique and independent of the
//! assembler. Anything else is refused instead of guessing bytes (FR-052). Port of `x86.js`.
//! Nothing here executes, assembles to a file, or touches a process.

use crate::util::{is_js_space, js_trim, skip_ws, utf16_len};
use std::collections::HashMap;
use std::sync::OnceLock;

pub const R64: [&str; 16] = [
    "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13",
    "r14", "r15",
];
pub const R32: [&str; 16] = [
    "eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi", "r8d", "r9d", "r10d", "r11d", "r12d",
    "r13d", "r14d", "r15d",
];

const FIXED: [(&str, &[u8]); 11] = [
    ("nop", &[0x90]),
    ("ret", &[0xC3]),
    ("int3", &[0xCC]),
    ("syscall", &[0x0F, 0x05]),
    ("leave", &[0xC9]),
    ("cpuid", &[0x0F, 0xA2]),
    ("hlt", &[0xF4]),
    ("ud2", &[0x0F, 0x0B]),
    ("pause", &[0xF3, 0x90]),
    ("cdq", &[0x99]),
    ("cqo", &[0x48, 0x99]),
];

/// (mnemonic, opcode, /digit) in the order of the JS table (this order drives the prediction catalogue).
const ALU: [(&str, u8, i32); 7] = [
    ("add", 0x01, 0),
    ("or", 0x09, 1),
    ("and", 0x21, 4),
    ("sub", 0x29, 5),
    ("xor", 0x31, 6),
    ("cmp", 0x39, 7),
    ("mov", 0x89, -1),
];

const BRANCH: [&str; 25] = [
    "jmp", "call", "je", "jne", "jz", "jnz", "ja", "jae", "jb", "jbe", "jg", "jge", "jl", "jle",
    "js", "jns", "jo", "jno", "jp", "jnp", "jc", "jnc", "loop", "loope", "loopne",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncodeResult {
    Ok {
        bytes: Vec<u8>,
        hex: String,
        form: String,
    },
    Err {
        code: &'static str,
        reason: &'static str,
    },
}

impl EncodeResult {
    pub fn is_ok(&self) -> bool {
        matches!(self, EncodeResult::Ok { .. })
    }
}

pub fn hex_of(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn reg64(name: &str) -> Option<usize> {
    R64.iter().position(|r| *r == name)
}

fn reg32(name: &str) -> Option<usize> {
    R32.iter().position(|r| *r == name)
}

fn le32(n: u32) -> [u8; 4] {
    n.to_le_bytes()
}

/// `parseImm`: hex (`0x..`, optional `-`) or decimal; `None` when the token is not a number. Values are `f64`
/// like in JavaScript so oversized literals simply fail the range checks.
fn parse_imm(token: &str) -> Option<f64> {
    let t = js_trim(token).to_lowercase();
    let (neg, body) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.as_str()),
    };
    if let Some(h) = body.strip_prefix("0x") {
        if !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit()) {
            let mut v = 0f64;
            for c in h.chars() {
                v = v * 16.0 + f64::from(c.to_digit(16).unwrap_or(0));
            }
            return Some(if neg { -v } else { v });
        }
        return None;
    }
    if !body.is_empty() && body.chars().all(|c| c.is_ascii_digit()) {
        let v: f64 = body.parse().ok()?;
        return Some(if neg { -v } else { v });
    }
    None
}

/// `normalize`: trim, lower-case, collapse white space, `a ,b` -> `a, b`.
pub fn normalize(text: &str) -> String {
    let lower = js_trim(text).to_lowercase();
    let mut collapsed = String::new();
    let mut in_ws = false;
    for c in lower.chars() {
        if is_js_space(c) {
            if !in_ws {
                collapsed.push(' ');
            }
            in_ws = true;
        } else {
            collapsed.push(c);
            in_ws = false;
        }
    }
    let t: Vec<char> = collapsed.chars().collect();
    let mut out = String::new();
    let mut p = 0usize;
    while p < t.len() {
        let q = skip_ws(&t, p);
        if q < t.len() && t[q] == ',' {
            out.push_str(", ");
            p = skip_ws(&t, q + 1);
        } else {
            out.push(t[p]);
            p += 1;
        }
    }
    out
}

fn rex_rm(w: bool, reg: usize, rm: usize) -> Vec<u8> {
    let rex =
        0x40u8 | if w { 8 } else { 0 } | if reg >= 8 { 4 } else { 0 } | if rm >= 8 { 1 } else { 0 };
    if rex != 0x40 || w { vec![rex] } else { vec![] }
}

fn done(bytes: Vec<u8>, form: String) -> EncodeResult {
    let hex = hex_of(&bytes);
    EncodeResult::Ok { bytes, hex, form }
}

fn unsupported(reason: &'static str) -> EncodeResult {
    EncodeResult::Err {
        code: "UNSUPPORTED_ENCODING",
        reason,
    }
}

/// `LABEL = /^[A-Za-z_.$][\w.$@]*$/`
fn is_label(s: &str) -> bool {
    let mut it = s.chars();
    match it.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '.' || c == '$' => {}
        _ => return false,
    }
    it.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '$' | '@'))
}

/// Returns the encoding, or an error with a stable code. Never panics on user text.
pub fn encode(instruction: &str) -> EncodeResult {
    let text = normalize(instruction);
    if text.is_empty() || utf16_len(&text) > 120 {
        return unsupported("empty or too long");
    }
    if let Some((_, b)) = FIXED.iter().find(|(k, _)| *k == text) {
        return done(b.to_vec(), "fixed".into());
    }
    let (mnemonic, operands): (&str, Vec<&str>) = match text.find(' ') {
        None => (text.as_str(), vec![]),
        Some(sp) => (&text[..sp], text[sp + 1..].split(", ").collect()),
    };
    if BRANCH.contains(&mnemonic) {
        if operands.len() == 1
            && is_label(operands[0])
            && parse_imm(operands[0]).is_none()
            && reg64(operands[0]).is_none()
            && reg32(operands[0]).is_none()
        {
            return EncodeResult::Err {
                code: "BYTE_ENCODING_UNRESOLVED",
                reason: "label/address unknown: relocation or displacement is not determined",
            };
        }
        return unsupported(
            "branch forms other than a symbolic target are not in the verified table",
        );
    }
    if (mnemonic == "push" || mnemonic == "pop") && operands.len() == 1 {
        let Some(r) = reg64(operands[0]) else {
            return unsupported("only 64-bit register operands are supported");
        };
        let mut bytes = Vec::new();
        if r >= 8 {
            bytes.push(0x41);
        }
        bytes.push((if mnemonic == "push" { 0x50 } else { 0x58 }) + (r & 7) as u8);
        return done(bytes, format!("{mnemonic} r64"));
    }
    if let Some(&(_, opcode, ext)) = ALU.iter().find(|(k, _, _)| *k == mnemonic)
        && operands.len() == 2
    {
        let (dst, src) = (operands[0], operands[1]);
        for (table, wide) in [(&R64, true), (&R32, false)] {
            let d = table.iter().position(|r| *r == dst);
            let s = table.iter().position(|r| *r == src);
            if let (Some(d), Some(s)) = (d, s) {
                let mut bytes = rex_rm(wide, s, d);
                bytes.push(opcode);
                bytes.push(0xC0 | (((s & 7) as u8) << 3) | (d & 7) as u8);
                let f = if wide { "r64" } else { "r32" };
                return done(bytes, format!("{mnemonic} {f}, {f}"));
            }
        }
        let imm = parse_imm(src);
        if mnemonic == "mov" {
            if let (Some(d), Some(v)) = (reg32(dst), imm)
                && (0.0..=4_294_967_295.0).contains(&v)
            {
                let mut bytes = Vec::new();
                if d >= 8 {
                    bytes.push(0x41);
                }
                bytes.push(0xB8 + (d & 7) as u8);
                bytes.extend_from_slice(&le32(v as u64 as u32));
                return done(bytes, "mov r32, imm32".into());
            }
            if reg64(dst).is_some() && imm.is_some() {
                return unsupported(
                    "mov r64, imm has several valid encodings; the choice depends on the assembler",
                );
            }
        }
        if (mnemonic == "add" || mnemonic == "sub")
            && let (Some(d), Some(v)) = (reg64(dst), imm)
        {
            let digit = ext as u8;
            if (-128.0..=127.0).contains(&v) {
                let mut bytes = rex_rm(true, 0, d);
                bytes.push(0x83);
                bytes.push(0xC0 | (digit << 3) | (d & 7) as u8);
                bytes.push((v as i64 & 0xFF) as u8);
                return done(bytes, format!("{mnemonic} r64, imm8"));
            }
            if (-2_147_483_648.0..=2_147_483_647.0).contains(&v) {
                let mut bytes = rex_rm(true, 0, d);
                bytes.push(0x81);
                bytes.push(0xC0 | (digit << 3) | (d & 7) as u8);
                bytes.extend_from_slice(&le32(v as i64 as u32));
                return done(bytes, format!("{mnemonic} r64, imm32"));
            }
        }
    }
    unsupported("not in the verified x86-64 table (this is not a general assembler)")
}

#[derive(Clone, Debug)]
pub struct Encodable {
    pub asm: String,
    pub hex: String,
}

/// All instruction texts that [`encode`] accepts (prediction catalogue, same order as the JS engine).
pub fn list_encodable() -> &'static [Encodable] {
    static CATALOG: OnceLock<Vec<Encodable>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut asm: Vec<String> = FIXED.iter().map(|(k, _)| (*k).to_string()).collect();
        for r in R64 {
            asm.push(format!("push {r}"));
            asm.push(format!("pop {r}"));
        }
        for (op, _, _) in ALU {
            for d in R64 {
                for s in R64 {
                    asm.push(format!("{op} {d}, {s}"));
                }
            }
            for d in R32 {
                for s in R32 {
                    asm.push(format!("{op} {d}, {s}"));
                }
            }
        }
        asm.into_iter()
            .filter_map(|a| match encode(&a) {
                EncodeResult::Ok { hex, .. } => Some(Encodable { asm: a, hex }),
                EncodeResult::Err { .. } => None,
            })
            .collect()
    })
}

/// Reverse lookup for known forms (first catalogue entry wins).
pub fn disassemble_known(hex_text: &str) -> Option<&'static str> {
    static REVERSE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    let digits: Vec<char> = hex_text
        .to_uppercase()
        .chars()
        .filter(|c| matches!(c, '0'..='9' | 'A'..='F'))
        .collect();
    if digits.is_empty() {
        return None;
    }
    let spaced = digits
        .chunks(2)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ");
    let map = REVERSE.get_or_init(|| {
        let mut m = HashMap::new();
        for e in list_encodable() {
            m.entry(e.hex.as_str()).or_insert(e.asm.as_str());
        }
        m
    });
    map.get(spaced.as_str()).copied()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HexResult {
    Ok {
        bytes: Vec<u8>,
        hex: String,
    },
    Err {
        code: &'static str,
        reason: &'static str,
    },
}

/// UT-023 style hex validation: even nibble count, hex digits only, bounded size.
pub fn validate_hex(value: &str, max_bytes: usize) -> HexResult {
    let compact: String = value.chars().filter(|c| !is_js_space(*c)).collect();
    if compact.is_empty() {
        return HexResult::Err {
            code: "CONTEXT_MISSING",
            reason: "empty",
        };
    }
    if !compact.chars().all(|c| c.is_ascii_hexdigit()) {
        return HexResult::Err {
            code: "INVALID_HEX",
            reason: "non-hex character",
        };
    }
    if !compact.len().is_multiple_of(2) {
        return HexResult::Err {
            code: "ODD_HEX_LENGTH",
            reason: "incomplete byte",
        };
    }
    if compact.len() / 2 > max_bytes {
        return HexResult::Err {
            code: "HEX_TOO_LARGE",
            reason: "too many bytes",
        };
    }
    let bytes: Vec<u8> = (0..compact.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&compact[i..i + 2], 16).unwrap_or(0))
        .collect();
    let hex = hex_of(&bytes);
    HexResult::Ok { bytes, hex }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> String {
        match encode(s) {
            EncodeResult::Ok { hex, .. } => hex,
            EncodeResult::Err { code, .. } => format!("ERR:{code}"),
        }
    }

    #[test]
    fn documented_forms() {
        assert_eq!(hex("nop"), "90");
        assert_eq!(hex("mov rax, rbx"), "48 89 D8");
        assert_eq!(hex("push r8"), "41 50");
        assert_eq!(hex("sub rsp, 0x20"), "48 83 EC 20");
        assert_eq!(hex("mov eax, 1"), "B8 01 00 00 00");
        assert_eq!(hex("add rsp, -8"), "48 83 C4 F8");
    }

    #[test]
    fn unresolved_and_unsupported_never_yield_bytes() {
        assert_eq!(hex("jmp external_symbol"), "ERR:BYTE_ENCODING_UNRESOLVED");
        assert_eq!(hex("mov rax, 1"), "ERR:UNSUPPORTED_ENCODING");
        assert_eq!(hex(""), "ERR:UNSUPPORTED_ENCODING");
    }

    #[test]
    fn normalize_matches_js() {
        assert_eq!(normalize("MOV  RAX ,RBX"), "mov rax, rbx");
        assert_eq!(normalize("a , , b"), "a, , b");
    }

    #[test]
    fn catalogue_and_reverse_lookup() {
        assert_eq!(disassemble_known("48 89 d8"), Some("mov rax, rbx"));
        assert!(list_encodable().len() > 600);
    }

    #[test]
    fn validate_hex_cases() {
        assert!(matches!(validate_hex("90 C3", 2048), HexResult::Ok { .. }));
        assert_eq!(
            validate_hex("GG", 2048),
            HexResult::Err {
                code: "INVALID_HEX",
                reason: "non-hex character"
            }
        );
        assert_eq!(
            validate_hex("9", 2048),
            HexResult::Err {
                code: "ODD_HEX_LENGTH",
                reason: "incomplete byte"
            }
        );
    }
}
