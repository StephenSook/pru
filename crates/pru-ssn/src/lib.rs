//! SSN span recognition and a deterministic synthetic canary evaluation.
//!
//! The recognizer deliberately accepts group `00`. The SSA does not issue that
//! group, but IRS public test taxpayers use it. Such spans are marked
//! `test_only` so a test identifier cannot be mistaken for a real taxpayer ID.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// The surface form that caused a span to be recognized.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SsnFormat {
    Dashed,
    Spaced,
    Plain,
    LineBreak,
    Masked,
    OcrNoise,
}

/// A byte range in the original text containing an SSN-like value.
///
/// The normalized identifier is intentionally not retained. Callers that need
/// an audit identifier should hash the original bytes and avoid logging them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SsnSpan {
    pub start: usize,
    pub end: usize,
    pub format: SsnFormat,
    pub test_only: bool,
}

/// Find SSN spans in `text`.
///
/// Plain nine-digit candidates and candidates containing OCR substitutions
/// (`O` for zero or `l` for one) require a nearby explicit SSN label. This
/// keeps unlabeled account numbers and ordinary words out of the result.
#[must_use]
pub fn recognize(text: &str) -> Vec<SsnSpan> {
    candidate_regex()
        .captures_iter(text)
        .filter_map(|captures| {
            let candidate = captures.get(0)?;
            let start = candidate.start();
            let end = candidate.end();

            if !has_token_boundaries(text, start, end) {
                return None;
            }

            let raw = candidate.as_str();
            let masked = captures.name("masked").is_some();
            let plain = captures.name("plain").is_some();
            let has_ocr_noise = raw
                .bytes()
                .any(|byte| matches!(byte, b'O' | b'o' | b'L' | b'l'));

            if (plain || has_ocr_noise) && !has_ssn_context(text, start, end) {
                return None;
            }

            if masked {
                return Some(SsnSpan {
                    start,
                    end,
                    format: SsnFormat::Masked,
                    test_only: false,
                });
            }

            let digits = normalize_digits(raw)?;
            let area = parse_component(&digits[0..3])?;
            let group = parse_component(&digits[3..5])?;
            let serial = parse_component(&digits[5..9])?;
            let test_only = group == 0 || area >= 900;

            if !test_only && (area == 0 || area == 666 || group == 0 || serial == 0) {
                return None;
            }

            let format = if has_ocr_noise {
                SsnFormat::OcrNoise
            } else if plain {
                SsnFormat::Plain
            } else if raw.contains('\n') || raw.contains('\r') {
                SsnFormat::LineBreak
            } else if raw.contains('-') {
                SsnFormat::Dashed
            } else {
                SsnFormat::Spaced
            };

            Some(SsnSpan {
                start,
                end,
                format,
                test_only,
            })
        })
        .collect()
}

fn candidate_regex() -> &'static Regex {
    static CANDIDATE: OnceLock<Regex> = OnceLock::new();
    CANDIDATE.get_or_init(|| {
        Regex::new(
            r"(?x)
              (?P<masked>[Xx]{3}[\x20\t\r\n-]+[Xx]{2}[\x20\t\r\n-]+[0-9]{4})
              |
              (?P<separated>[0-9OoLl]{3}[\x20\t\r\n-]+[0-9OoLl]{2}[\x20\t\r\n-]+[0-9OoLl]{4})
              |
              (?P<plain>[0-9]{9})",
        )
        .expect("the SSN candidate regex is static and valid")
    })
}

fn context_regex() -> &'static Regex {
    static CONTEXT: OnceLock<Regex> = OnceLock::new();
    CONTEXT.get_or_init(|| {
        Regex::new(r"(?i)\b(?:ssn|social[ \t\r\n]+security(?:[ \t\r\n]+number)?)\b")
            .expect("the SSN context regex is static and valid")
    })
}

fn has_token_boundaries(text: &str, start: usize, end: usize) -> bool {
    let bytes = text.as_bytes();
    let left_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
    let right_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
    left_ok && right_ok
}

fn has_ssn_context(text: &str, start: usize, end: usize) -> bool {
    const CONTEXT_WINDOW: usize = 48;

    let mut window_start = start.saturating_sub(CONTEXT_WINDOW);
    while !text.is_char_boundary(window_start) {
        window_start += 1;
    }

    let mut window_end = end.saturating_add(CONTEXT_WINDOW).min(text.len());
    while !text.is_char_boundary(window_end) {
        window_end -= 1;
    }

    context_regex().is_match(&text[window_start..window_end])
}

fn normalize_digits(raw: &str) -> Option<String> {
    let digits: String = raw
        .chars()
        .filter_map(|character| match character {
            '0'..='9' => Some(character),
            'O' | 'o' => Some('0'),
            'L' | 'l' => Some('1'),
            _ => None,
        })
        .collect();
    (digits.len() == 9).then_some(digits)
}

fn parse_component(digits: &str) -> Option<u16> {
    digits.parse().ok()
}

/// Deterministic synthetic canary generation and measurement.
pub mod eval {
    use std::collections::BTreeMap;

    use serde::Serialize;

    use super::{SsnFormat, recognize};

    const CASES_PER_FORMAT: usize = 60;
    const NEGATIVE_CASES_PER_KIND: usize = 20;

    #[derive(Debug)]
    struct Canary {
        text: String,
        format: SsnFormat,
        expected_test_only: bool,
    }

    /// The machine-readable evaluation report written to `eval/ssn_canaries.json`.
    #[derive(Debug, Serialize)]
    pub struct EvalReport {
        pub schema_version: u8,
        pub dataset: DatasetSummary,
        pub results: EvalResults,
        pub format_counts: BTreeMap<String, usize>,
        pub notes: Vec<String>,
    }

    #[derive(Debug, Serialize)]
    pub struct DatasetSummary {
        pub positive_canaries: usize,
        pub negative_controls: usize,
        pub generator: String,
        pub synthetic_identifier_ranges: Vec<String>,
    }

    #[derive(Debug, Serialize)]
    pub struct EvalResults {
        pub true_positives: usize,
        pub misses: usize,
        pub recall: f64,
        pub false_positive_spans: usize,
        pub negative_controls_flagged: usize,
        pub false_positive_rate_per_control: f64,
        pub miss_rate_upper_bound_95_one_sided: f64,
        pub confidence_method: String,
    }

    /// Generate the fixed corpus in memory and evaluate the recognizer.
    #[must_use]
    pub fn evaluate() -> EvalReport {
        let positives = generate_canaries();
        let negatives = generate_negative_controls();

        let mut true_positives = 0;
        let mut format_counts = BTreeMap::new();
        for canary in &positives {
            *format_counts
                .entry(format_name(canary.format).to_owned())
                .or_default() += 1;
            let spans = recognize(&canary.text);
            let matched = spans.len() == 1
                && spans[0].format == canary.format
                && spans[0].test_only == canary.expected_test_only;
            true_positives += usize::from(matched);
        }

        let misses = positives.len() - true_positives;
        let mut false_positive_spans = 0;
        let mut negative_controls_flagged = 0;
        for control in &negatives {
            let spans = recognize(control);
            false_positive_spans += spans.len();
            negative_controls_flagged += usize::from(!spans.is_empty());
        }

        EvalReport {
            schema_version: 1,
            dataset: DatasetSummary {
                positive_canaries: positives.len(),
                negative_controls: negatives.len(),
                generator: "pru-ssn deterministic generator v1".to_owned(),
                synthetic_identifier_ranges: vec![
                    "group 00, which SSA does not issue".to_owned(),
                    "area 9xx, which SSA does not issue".to_owned(),
                ],
            },
            results: EvalResults {
                true_positives,
                misses,
                recall: as_f64(true_positives) / as_f64(positives.len()),
                false_positive_spans,
                negative_controls_flagged,
                false_positive_rate_per_control: as_f64(negative_controls_flagged)
                    / as_f64(negatives.len()),
                miss_rate_upper_bound_95_one_sided: one_sided_upper_bound_95(
                    misses,
                    positives.len(),
                ),
                confidence_method: "exact Clopper-Pearson binomial bound".to_owned(),
            },
            format_counts,
            notes: vec![
                "All full synthetic canaries use group 00 or area 9xx; neither is SSA-issued."
                    .to_owned(),
                "Masked canaries reveal only a generated four-digit suffix and cannot be tagged test_only from their visible bytes."
                    .to_owned(),
                "Negative controls include synthetic phone numbers, EINs, ZIP+4 values, dates, unlabeled nine-digit account IDs, and OCR-like prose."
                    .to_owned(),
            ],
        }
    }

    fn generate_canaries() -> Vec<Canary> {
        let mut canaries = Vec::with_capacity(CASES_PER_FORMAT * 6);

        for index in 0..CASES_PER_FORMAT {
            let serial = 1000 + index;
            let area_group_00 = 101 + index;
            let area_9xx = 900 + index;
            let group = 10 + (index % 80);

            canaries.push(Canary {
                text: format!(
                    "Synthetic intake record {index}: taxpayer SSN {area_group_00:03}-00-{serial:04}."
                ),
                format: SsnFormat::Dashed,
                expected_test_only: true,
            });
            canaries.push(Canary {
                text: format!(
                    "Synthetic organizer {index} lists SSN {area_9xx:03} {group:02} {serial:04} for routing tests."
                ),
                format: SsnFormat::Spaced,
                expected_test_only: true,
            });
            canaries.push(Canary {
                text: format!(
                    "Synthetic taxpayer {index} Social Security number: {area_group_00:03}00{serial:04}."
                ),
                format: SsnFormat::Plain,
                expected_test_only: true,
            });
            canaries.push(Canary {
                text: format!(
                    "Synthetic scanned worksheet {index} contains SSN {area_group_00:03}-\n00-\n{serial:04}."
                ),
                format: SsnFormat::LineBreak,
                expected_test_only: true,
            });
            canaries.push(Canary {
                text: format!(
                    "Synthetic portal preview {index} masks the identifier as XXX-XX-{serial:04}."
                ),
                format: SsnFormat::Masked,
                expected_test_only: false,
            });

            let ocr_source = format!("{area_9xx:03}{group:02}{serial:04}");
            let ocr_rendered: String = ocr_source
                .chars()
                .enumerate()
                .map(|(position, character)| match character {
                    '0' => 'O',
                    '1' if position % 2 == 1 => 'l',
                    other => other,
                })
                .collect();
            canaries.push(Canary {
                text: format!(
                    "Synthetic OCR output {index}; SSN: {}-{}-{}.",
                    &ocr_rendered[0..3],
                    &ocr_rendered[3..5],
                    &ocr_rendered[5..9]
                ),
                format: SsnFormat::OcrNoise,
                expected_test_only: true,
            });
        }

        canaries
    }

    fn generate_negative_controls() -> Vec<String> {
        let mut controls = Vec::with_capacity(NEGATIVE_CASES_PER_KIND * 6);
        for index in 0..NEGATIVE_CASES_PER_KIND {
            controls.push(format!(
                "Synthetic contact {index}: phone (404) 555-{:04}.",
                1000 + index
            ));
            controls.push(format!(
                "Synthetic vendor {index}: EIN 90-000{:04}.",
                1000 + index
            ));
            controls.push(format!(
                "Synthetic mailing record {index}: ZIP+4 99999-{:04}.",
                1000 + index
            ));
            controls.push(format!(
                "Synthetic appointment {index}: date 2026-10-{:02}.",
                1 + (index % 28)
            ));
            controls.push(format!(
                "Synthetic invoice {index}: account 90000{:04}.",
                1000 + index
            ));
            controls.push(format!(
                "Synthetic OCR note {index}: SOLO-LINE item OOl is not an identifier."
            ));
        }
        controls
    }

    fn format_name(format: SsnFormat) -> &'static str {
        match format {
            SsnFormat::Dashed => "dashed",
            SsnFormat::Spaced => "spaced",
            SsnFormat::Plain => "plain_in_context",
            SsnFormat::LineBreak => "split_across_line_break",
            SsnFormat::Masked => "partially_masked",
            SsnFormat::OcrNoise => "ocr_noise",
        }
    }

    fn one_sided_upper_bound_95(misses: usize, trials: usize) -> f64 {
        assert!(misses <= trials);
        if trials == 0 || misses == trials {
            return 1.0;
        }

        let mut low = as_f64(misses) / as_f64(trials);
        let mut high = 1.0;
        for _ in 0..100 {
            let midpoint = f64::midpoint(low, high);
            if binomial_cdf(misses, trials, midpoint) > 0.05 {
                low = midpoint;
            } else {
                high = midpoint;
            }
        }
        f64::midpoint(low, high)
    }

    fn binomial_cdf(successes: usize, trials: usize, probability: f64) -> f64 {
        if probability == 0.0 {
            return 1.0;
        }
        if probability >= 1.0 {
            return f64::from(successes == trials);
        }

        let failure_probability = 1.0 - probability;
        let mut term =
            failure_probability.powi(i32::try_from(trials).expect("trial count fits i32"));
        let mut total = term;
        for observed in 0..successes {
            term *= as_f64(trials - observed) / as_f64(observed + 1);
            term *= probability / failure_probability;
            total += term;
        }
        total
    }

    fn as_f64(value: usize) -> f64 {
        f64::from(u32::try_from(value).expect("evaluation count fits u32"))
    }

    #[cfg(test)]
    mod tests {
        use super::{evaluate, one_sided_upper_bound_95};

        #[test]
        fn canary_harness_meets_phase_a_floor() {
            let report = evaluate();
            assert_eq!(report.dataset.positive_canaries, 360);
            assert_eq!(report.dataset.negative_controls, 120);
            assert_eq!(report.results.true_positives, 360);
            assert_eq!(report.results.misses, 0);
            assert_eq!(report.results.false_positive_spans, 0);
            assert_eq!(report.results.negative_controls_flagged, 0);
            assert!(report.results.miss_rate_upper_bound_95_one_sided < 0.01);
        }

        #[test]
        fn zero_miss_bound_matches_closed_form() {
            let expected = 1.0 - 0.05_f64.powf(1.0 / 300.0);
            let measured = one_sided_upper_bound_95(0, 300);
            assert!((measured - expected).abs() < 1e-12);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SsnFormat, recognize};

    fn single(text: &str) -> super::SsnSpan {
        let spans = recognize(text);
        assert_eq!(spans.len(), 1, "unexpected spans for {text:?}: {spans:?}");
        spans.into_iter().next().expect("one span")
    }

    #[test]
    fn recognizes_required_formats() {
        let cases = [
            ("Synthetic SSN 123-00-4567.", SsnFormat::Dashed, true),
            ("Synthetic SSN 900 12 4567.", SsnFormat::Spaced, true),
            ("Synthetic SSN 123004567.", SsnFormat::Plain, true),
            ("Synthetic SSN 123-\n00-\n4567.", SsnFormat::LineBreak, true),
            ("Masked copy XXX-XX-4567.", SsnFormat::Masked, false),
            ("Synthetic SSN: 9Ol-2O-lOOl.", SsnFormat::OcrNoise, true),
        ];

        for (text, expected_format, expected_test_only) in cases {
            let span = single(text);
            assert_eq!(span.format, expected_format);
            assert_eq!(span.test_only, expected_test_only);
            assert!(!text[span.start..span.end].is_empty());
        }
    }

    #[test]
    fn group_00_is_accepted_and_marked_test_only() {
        let span = single("IRS public test taxpayer SSN: 101-00-1001.");
        assert!(span.test_only);
        assert_eq!(
            &"IRS public test taxpayer SSN: 101-00-1001."[span.start..span.end],
            "101-00-1001"
        );
    }

    #[test]
    fn area_9xx_is_accepted_and_marked_test_only() {
        assert!(single("Synthetic SSN: 999-12-1001.").test_only);
    }

    #[test]
    fn plain_and_ocr_candidates_require_explicit_context() {
        assert!(recognize("Synthetic account 900121001 is unlabeled.").is_empty());
        assert!(recognize("Synthetic OCR token 9Ol-2O-lOOl is unlabeled.").is_empty());
        assert_eq!(recognize("Social Security number 900121001.").len(), 1);
        assert_eq!(recognize("SSN 9Ol-2O-lOOl.").len(), 1);
    }

    #[test]
    fn token_boundaries_prevent_embedded_matches() {
        assert!(recognize("prefixA900-12-1001Zsuffix").is_empty());
    }
}
