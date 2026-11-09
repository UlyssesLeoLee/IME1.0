//! The cost and the latency benchmark of the product (the M4 evidence of the mapping document).
//!
//! Two numbers are measured, both with the rules of the product itself:
//!
//! * **the latency of the local candidate path**: the candidates of the core for a prefix, the time they take.
//!   The design book (DD-IME §20.2) sets the budget of the local cache: `p95 <= 50 ms`.
//! * **the cost of one model call**: the tokens of a typical request, counted the way the core counts them
//!   (`ceil(utf16 length / 3)`), priced with the table the operator supplies.
//!
//! The price table is **not** a claim about any provider: it is a file the operator puts on the disk
//! (`01_Source/assets/pricing/sample-price-table.json` is a sample). Put your own table there and the numbers
//! below are your own.
//!
//! Run it:
//!
//! ```text
//! cargo run --release --example cost_bench -- [price table path] [iterations]
//! ```
//!
//! It prints one JSON object on the standard output; the script `01_Source/scripts/bench-cost.js` runs it, writes
//! the evidence into `02_Verification/cost-benchmark.json` and checks the budget.

use std::time::Instant;

use pime_core::budget::estimate_tokens;
use pime_core::json::{Json, Limits};
use pime_core::predict::{PredictOptions, engine};
use tase_metrics::meter::Usage;
use tase_metrics::pricing::{PriceEntry, PriceTable};

/// The budget of the local cache: `p95 <= 50 ms` (DD-IME §20.2).
const LOCAL_BUDGET_MS: u64 = 50;

/// The everyday calls of the product: the three languages of the IME and the prefixes a programmer types.
const CASES: [(&str, &str); 6] = [
    ("zh-CN", "ni"),
    ("zh-CN", "shi"),
    ("ja-JP", "konnichiiha"),
    ("en-US", "fn"),
    ("en-US", "let "),
    ("en-US", "impl "),
];

/// A typical request: the instructions of the product and the code around the caret.
///
/// The text is the real shape of a request (a short piece of code and the question about it), so the token count
/// is the count of a real request, not of a sentence.
fn typical_request() -> (String, String) {
    let instructions = "You are a programming assistant inside a code editor. Answer with the code only.";
    let payload = r#"{"language":"rust","file":"src/main.rs","selection":"fn main() {\n    let total: u32 = items.iter().map(|i| i.price).sum();\n    println!(\"{total}\");\n}","question":"name the variable that holds the sum"}"#;
    (instructions.to_string(), payload.to_string())
}

/// The percentiles of the samples (the sample is sorted; the percentile is the nearest rank).
fn percentile(sorted: &[u128], fraction: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[index]
}

/// The price table of the operator: a JSON file with the entries of the models.
///
/// The shape: `{"entries": [{"provider": "...", "model": "...", "input_per_million": 1.0,
/// "output_per_million": 2.0, "cached_input_per_million": 0.1}]}`.
fn read_price_table(path: &str) -> Result<PriceTable, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path} is not readable: {e}"))?;
    let value = pime_core::json::parse(&text, Limits::DEFAULT)
        .map_err(|e| format!("{path} is not valid JSON: {e:?}"))?;
    let entries = value
        .get("entries")
        .and_then(Json::as_array)
        .ok_or_else(|| format!("{path} has no \"entries\" array"))?;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let provider = entry
            .get("provider")
            .and_then(Json::as_str)
            .ok_or_else(|| "an entry has no provider".to_string())?;
        let model = entry
            .get("model")
            .and_then(Json::as_str)
            .ok_or_else(|| "an entry has no model".to_string())?;
        let number = |name: &str| {
            entry
                .get(name)
                .and_then(Json::as_f64)
                .unwrap_or_else(|| panic!("the entry of {model} has no number {name}"))
        };
        out.push(PriceEntry::new(
            provider,
            model,
            number("input_per_million"),
            number("output_per_million"),
            number("cached_input_per_million"),
        ));
    }
    Ok(PriceTable::new(out))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let table_path = args
        .next()
        .unwrap_or_else(|| "assets/pricing/sample-price-table.json".to_string());
    let iterations: usize = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(200);

    // ---- the latency of the local candidate path -------------------------------------------------------
    // The engine is asked once before the measurement: the first call pays for what is loaded.
    for (profile, prefix) in CASES {
        let _ = engine().predict(&PredictOptions::new(profile, prefix)).expect("the engine answers");
    }
    let mut samples: Vec<u128> = Vec::with_capacity(iterations * CASES.len());
    let mut candidates_seen = 0usize;
    for _ in 0..iterations {
        for (profile, prefix) in CASES {
            let started = Instant::now();
            let candidates = engine()
                .predict(&PredictOptions::new(profile, prefix))
                .expect("the engine answers");
            samples.push(started.elapsed().as_nanos());
            candidates_seen += candidates.len();
        }
    }
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    let p50 = percentile(&sorted, 0.50);
    let p95 = percentile(&sorted, 0.95);
    let max = sorted.last().copied().unwrap_or(0);

    // ---- the cost of one model call -------------------------------------------------------------------
    let (instructions, payload) = typical_request();
    let input_tokens = estimate_tokens(&instructions) + estimate_tokens(&payload);
    // The output of the model is an estimate too: the product asks for a short answer.
    const OUTPUT_TOKENS: u64 = 200;
    let table = read_price_table(&table_path).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    // Every entry of the table is priced: the report names the models and their prices, so the reader can pick
    // the one that is his.
    let mut priced: Vec<(String, String, f64)> = Vec::new();
    for entry in &table.entries {
        let usage = Usage::new(input_tokens as u64, OUTPUT_TOKENS, 0, None);
        if let Some(cost) = table.cost_of(&usage, &entry.provider, &entry.model) {
            priced.push((entry.provider.clone(), entry.model.clone(), cost));
        }
    }
    priced.sort_by(|left, right| left.2.total_cmp(&right.2));

    let models: Vec<Json> = priced
        .iter()
        .map(|(provider, model, cost)| {
            Json::obj(vec![
                ("provider", Json::str(provider)),
                ("model", Json::str(model)),
                ("cost_usd", Json::Num(*cost)),
            ])
        })
        .collect();

    let report = Json::obj(vec![
        (
            "latency",
            Json::obj(vec![
                ("samples", Json::int(sorted.len() as i64)),
                ("candidates_seen", Json::int(candidates_seen as i64)),
                ("p50_us", Json::Num(p50 as f64 / 1000.0)),
                ("p95_us", Json::Num(p95 as f64 / 1000.0)),
                ("max_us", Json::Num(max as f64 / 1000.0)),
                ("p95_ns", Json::int(p95 as i64)),
                ("budget_us", Json::int((LOCAL_BUDGET_MS * 1000) as i64)),
                (
                    "within_budget",
                    Json::Bool(p95 <= (LOCAL_BUDGET_MS as u128) * 1_000_000),
                ),
            ]),
        ),
        (
            "cost",
            Json::obj(vec![
                ("price_table", Json::str(&table_path)),
                ("input_tokens", Json::int(input_tokens)),
                ("output_tokens", Json::int(OUTPUT_TOKENS as i64)),
                ("models", Json::Arr(models)),
            ]),
        ),
    ]);
    println!("{}", report.to_json_string());
}
