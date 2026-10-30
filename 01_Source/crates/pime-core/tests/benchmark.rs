//! UT-068: local candidate computation benchmark - 1000 prefix events, P95 must be <= 20 ms.
//! This is a diagnostic UT measurement; the formal P95 is measured in ST under controlled conditions.

use pime_core::predict::{engine, PredictOptions};
use std::time::Instant;

const CASES: [(&str, &[&str]); 7] = [
    (
        "rust",
        &[
            "f", "fn", "fnm", "impl", "mat", "println", "cou", "ext", "res", "str",
        ],
    ),
    (
        "python",
        &["d", "def", "fori", "class", "try", "lc", "pri", "wi"],
    ),
    (
        "typescript",
        &["fn", "garrow", "int", "comp", "as", "pro", "cla"],
    ),
    (
        "powershell",
        &["Ge", "func", "tern", "foreach", "para", "gci"],
    ),
    ("c", &["main", "#inc", "static", "fnptr", "str", "ty"]),
    ("assembly", &["mov", "prol", "sec", "ret", "sys"]),
    (
        "machine-code",
        &[
            "nop",
            "push r",
            "mov rax, rbx",
            "jmp foo",
            "48 89 d8",
            "xor eax",
            "mov r8",
        ],
    ),
];

fn before_text(profile: &str, i: usize) -> String {
    // realistic surrounding code (about 1.5 KB) so masking and block detection are part of the measurement
    let unit = match profile {
        "python" => "class Foo:\n    def run(self, items):\n        for item in items:\n            print(item)  # note\n",
        "rust" => "impl Foo {\n    fn run(&self) {\n        let counter = 1; // note\n        println!(\"{}\", counter);\n    }\n}\n",
        "typescript" => "class Foo {\n  run(items: string[]): void {\n    const counter = `x${items.length}`; // note\n  }\n}\n",
        "powershell" => "function Get-Foo {\n    param($Path)\n    Get-ChildItem -Path $Path # note\n}\n",
        "c" => "int main(void) {\n    int counter = 0; /* note */\n    printf(\"%d\", counter);\n}\n",
        "assembly" => "_start:\n    mov rax, rbx ; note\n    push rbp\n",
        _ => "",
    };
    unit.repeat(8 + i % 4)
}

#[test]
fn ut_068_local_candidate_p95_within_20ms() {
    let engine = engine(); // one-time pack load is initialisation, not a prefix event
    let _ = engine.predict(&PredictOptions::new("rust", "fnm"));
    let mut micros: Vec<u128> = Vec::with_capacity(1000);
    let mut produced = 0usize;
    for i in 0..1000 {
        let (profile, prefixes) = CASES[i % CASES.len()];
        let prefix = prefixes[(i / CASES.len()) % prefixes.len()];
        let opts = PredictOptions::new(profile, prefix).before(&before_text(profile, i));
        let t = Instant::now();
        let r = engine.predict(&opts).expect("predict");
        micros.push(t.elapsed().as_micros());
        produced += r.len();
    }
    micros.sort_unstable();
    let p50 = micros[499] as f64 / 1000.0;
    let p95 = micros[949] as f64 / 1000.0;
    let max = micros[999] as f64 / 1000.0;
    let profile = if cfg!(debug_assertions) {
        "debug (unoptimized)"
    } else {
        "release"
    };
    println!("UT-068 BENCHMARK: events=1000 candidates_produced={produced} P50={p50:.3} ms P95={p95:.3} ms max={max:.3} ms build={profile}");
    assert!(
        produced > 0,
        "the benchmark must exercise real candidate generation"
    );
    assert!(
        p95 <= 20.0,
        "local P95 {p95:.3} ms exceeds the 20 ms target ({profile} build)"
    );
}
