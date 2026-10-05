use std::{env, error::Error, fs, path::PathBuf};

use pru_ssn::eval::evaluate;

fn main() -> Result<(), Box<dyn Error>> {
    let output = env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("eval/ssn_canaries.json"), PathBuf::from);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    let report = evaluate();
    let json = serde_json::to_string_pretty(&report)?;
    fs::write(&output, format!("{json}\n"))?;

    println!("wrote {}", output.display());
    println!(
        "positives={} true_positives={} misses={} recall={:.6}",
        report.dataset.positive_canaries,
        report.results.true_positives,
        report.results.misses,
        report.results.recall
    );
    println!(
        "negative_controls={} flagged={} false_positive_spans={}",
        report.dataset.negative_controls,
        report.results.negative_controls_flagged,
        report.results.false_positive_spans
    );
    println!(
        "miss_rate_upper_bound_95_one_sided={:.9}",
        report.results.miss_rate_upper_bound_95_one_sided
    );
    Ok(())
}
