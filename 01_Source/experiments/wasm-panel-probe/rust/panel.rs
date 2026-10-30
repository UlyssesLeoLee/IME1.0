// P0 feasibility probe (NOT product code): a dependency-free WebAssembly module that a webview can load.
// Build: rustc --target wasm32-unknown-unknown --crate-type cdylib -O -C panic=abort rust/panel.rs -o ext/media/panel.wasm
// It does no DOM access (that is wasm-bindgen's job in the real design); it only proves that a Rust wasm module is
// loaded and executed inside a VS Code webview under the planned CSP, and that data crosses the boundary both ways.
#![no_std]

use core::panic::PanicInfo;
#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop {} }

static mut INPUT: [u8; 65536] = [0; 65536];
static mut OUTPUT: [u8; 256] = [0; 256];
static mut OUTPUT_LEN: usize = 0;

#[no_mangle] pub extern "C" fn input_ptr() -> *mut u8 { core::ptr::addr_of_mut!(INPUT) as *mut u8 }
#[no_mangle] pub extern "C" fn output_ptr() -> *const u8 { core::ptr::addr_of!(OUTPUT) as *const u8 }
#[no_mangle] pub extern "C" fn output_len() -> usize { unsafe { OUTPUT_LEN } }

/// Reads `len` bytes of a JSON view model from INPUT, counts the hunks and writes "hunks=<n>;bytes=<len>" to OUTPUT.
#[no_mangle]
pub extern "C" fn render(len: usize) -> usize {
    let (hunks, out_len);
    unsafe {
        let input = core::slice::from_raw_parts(core::ptr::addr_of!(INPUT) as *const u8, len.min(65536));
        let needle = b"\"hunk\"";
        let mut n = 0usize;
        let mut i = 0usize;
        while i + needle.len() <= input.len() { if &input[i..i + needle.len()] == needle { n += 1; } i += 1; }
        hunks = n;
        let out = &mut *core::ptr::addr_of_mut!(OUTPUT);
        let mut w = 0usize;
        for b in b"hunks=" { out[w] = *b; w += 1; }
        w += write_num(&mut out[w..], hunks);
        for b in b";bytes=" { out[w] = *b; w += 1; }
        w += write_num(&mut out[w..], len);
        OUTPUT_LEN = w;
        out_len = w;
    }
    let _ = hunks;
    out_len
}

fn write_num(buf: &mut [u8], mut n: usize) -> usize {
    let mut tmp = [0u8; 20];
    let mut i = 0;
    if n == 0 { tmp[0] = b'0'; i = 1; }
    while n > 0 { tmp[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
    for k in 0..i { buf[k] = tmp[i - 1 - k]; }
    i
}
