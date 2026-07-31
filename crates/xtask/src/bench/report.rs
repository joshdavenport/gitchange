//! Scaling-shape analysis and rendering: pure functions from case
//! results to the markdown report and CSV rows. Shape, not wall-clock,
//! is the headline (issue #16): absolute numbers on a dev machine are
//! optimistic, but the log-log exponent per dimension is
//! machine-independent enough to drive mitigation decisions.

use std::fmt::Write as _;

use super::case::CaseResult;

pub struct Meta {
    pub date: String,
    pub host: String,
    pub commit: String,
    pub iterations: usize,
    pub warmup: usize,
}

/// Median of raw samples; NaN-free inputs assumed (timings).
pub fn median(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let n = sorted.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// The log-log slope between two (scale, time) points: the local
/// exponent k in t ~ scale^k. `None` when either point can't support
/// the fit (zero scale or non-positive time).
pub fn exponent(from: (f64, f64), to: (f64, f64)) -> Option<f64> {
    let (s1, t1) = from;
    let (s2, t2) = to;
    if s1 <= 0.0 || s2 <= 0.0 || t1 <= 0.0 || t2 <= 0.0 || s1 == s2 {
        return None;
    }
    Some((t2 / t1).ln() / (s2 / s1).ln())
}

/// Least-squares log-log fit over every usable point — the headline
/// exponent. An endpoint-only fit could hide curvature between the
/// first and last scales; regression weighs every point.
pub fn fit_exponent(points: &[(f64, f64)]) -> Option<f64> {
    let logs: Vec<(f64, f64)> = points
        .iter()
        .filter(|(s, t)| *s > 0.0 && *t > 0.0)
        .map(|(s, t)| (s.ln(), t.ln()))
        .collect();
    if logs.len() < 2 {
        return None;
    }
    let n = logs.len() as f64;
    let mean_x = logs.iter().map(|(x, _)| x).sum::<f64>() / n;
    let mean_y = logs.iter().map(|(_, y)| y).sum::<f64>() / n;
    let denominator: f64 = logs.iter().map(|(x, _)| (x - mean_x).powi(2)).sum();
    if denominator == 0.0 {
        return None;
    }
    let numerator: f64 = logs
        .iter()
        .map(|(x, y)| (x - mean_x) * (y - mean_y))
        .sum();
    Some(numerator / denominator)
}

/// Min and max of raw samples.
pub fn spread(samples: &[f64]) -> (f64, f64) {
    let min = samples.iter().copied().fold(f64::INFINITY, f64::min);
    let max = samples.iter().copied().fold(0.0, f64::max);
    (min, max)
}

/// Human name for an exponent's growth class.
pub fn shape_label(exp: f64) -> &'static str {
    if exp < 0.85 {
        "sub-linear"
    } else if exp <= 1.15 {
        "≈ linear"
    } else if exp <= 1.6 {
        "super-linear"
    } else {
        "≈ quadratic or worse"
    }
}

fn fmt_ms(ms: f64) -> String {
    if ms < 10.0 {
        format!("{ms:.2}")
    } else if ms < 100.0 {
        format!("{ms:.1}")
    } else {
        format!("{ms:.0}")
    }
}

fn fmt_mb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0))
}

fn fmt_opt_exp(exp: Option<f64>) -> String {
    exp.map_or_else(|| "—".into(), |e| format!("{e:.2}"))
}

/// Dimensions in first-seen order.
fn dimensions(results: &[CaseResult]) -> Vec<String> {
    let mut seen = Vec::new();
    for result in results {
        if !seen.contains(&result.spec.dimension) {
            seen.push(result.spec.dimension.clone());
        }
    }
    seen
}

fn blurb(dimension: &str) -> &'static str {
    match dimension {
        "files" => "changed files, 4 hunks each, all hunks assigned across 4 changelists",
        "hunks" => "hunks per file across 25 changed files, all assigned across 4 changelists",
        "records" => {
            "dormant membership records on top of a fixed 50-file × 4-hunk diff (200 live records)"
        }
        "huge-file" => {
            "single generated file rewritten in full, no changelists — the unbounded-diff-memory probe (ADR 0005 caveat)"
        }
        _ => "",
    }
}

/// The full markdown report: one table per dimension, an overall shape
/// line, contrast rows against their baselines, and the standing
/// caveats a reader needs to interpret the numbers.
pub fn render_markdown(results: &[CaseResult], meta: &Meta) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# RefreshJob benchmark\n");
    let _ = writeln!(out, "- date: {}", meta.date);
    let _ = writeln!(out, "- host: {}", meta.host);
    let _ = writeln!(out, "- commit: {}", meta.commit);
    let _ = writeln!(
        out,
        "- per case: {} timed refreshes after {} warmup, fresh subprocess",
        meta.iterations, meta.warmup
    );

    for dimension in dimensions(results) {
        let (base, contrast): (Vec<&CaseResult>, Vec<&CaseResult>) = results
            .iter()
            .filter(|r| r.spec.dimension == dimension)
            .partition(|r| r.spec.contrast_of.is_none());

        let _ = writeln!(out, "\n## {dimension} — {}\n", blurb(&dimension));
        if dimension == "huge-file" {
            let _ = writeln!(
                out,
                "| case | lines | content MB | median ms | min | max | ×prev | step exp | peak RSS MB |"
            );
            let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|");
        } else {
            let _ = writeln!(
                out,
                "| case | scale | hunks | records | median ms | min | max | ×prev | step exp |"
            );
            let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|");
        }

        let mut prev: Option<(f64, f64)> = None;
        for result in &base {
            let med = median(&result.times_ms);
            let point = (result.spec.scale as f64, med);
            let (ratio, step) = match prev {
                Some((_, t1)) if t1 > 0.0 => (
                    format!("{:.1}×", med / t1),
                    fmt_opt_exp(prev.and_then(|p| exponent(p, point))),
                ),
                _ => ("—".into(), "—".into()),
            };
            let (min, max) = spread(&result.times_ms);
            if dimension == "huge-file" {
                let rss = result
                    .peak_rss_after
                    .map(|after| {
                        let before = result.peak_rss_before.unwrap_or(0);
                        format!("{} → {}", fmt_mb(before), fmt_mb(after))
                    })
                    .unwrap_or_else(|| "n/a".into());
                let _ = writeln!(
                    out,
                    "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                    result.spec.name,
                    result.spec.scale,
                    fmt_mb(result.content_bytes),
                    fmt_ms(med),
                    fmt_ms(min),
                    fmt_ms(max),
                    ratio,
                    step,
                    rss
                );
            } else {
                let _ = writeln!(
                    out,
                    "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                    result.spec.name,
                    result.spec.scale,
                    result.hunks_total,
                    result.live_records + result.dormant_records,
                    fmt_ms(med),
                    fmt_ms(min),
                    fmt_ms(max),
                    ratio,
                    step
                );
            }
            prev = Some(point);
        }

        let fit: Vec<(f64, f64)> = base
            .iter()
            .map(|r| (r.spec.scale as f64, median(&r.times_ms)))
            .filter(|(s, t)| *s > 0.0 && *t > 0.0)
            .collect();
        if let Some(exp) = fit_exponent(&fit) {
            let worst_step = fit
                .windows(2)
                .filter_map(|pair| exponent(pair[0], pair[1]))
                .fold(f64::NEG_INFINITY, f64::max);
            let _ = writeln!(
                out,
                "\nShape: t ~ scale^{exp:.2} over {}→{} ({}; least-squares over {} points, worst step {worst_step:.2})",
                fit[0].0,
                fit[fit.len() - 1].0,
                shape_label(exp),
                fit.len()
            );
        }

        if !contrast.is_empty() {
            let _ = writeln!(out, "\nContrasts:\n");
            if dimension == "huge-file" {
                let _ = writeln!(out, "| case | median ms | vs baseline | peak RSS MB |");
                let _ = writeln!(out, "|---|---|---|---|");
            } else {
                let _ = writeln!(out, "| case | median ms | vs baseline |");
                let _ = writeln!(out, "|---|---|---|");
            }
            for result in &contrast {
                let med = median(&result.times_ms);
                let vs = result
                    .spec
                    .contrast_of
                    .as_deref()
                    .and_then(|name| results.iter().find(|r| r.spec.name == name))
                    .map(|b| {
                        let base_med = median(&b.times_ms);
                        if base_med > 0.0 {
                            format!("{:.2}× of {}", med / base_med, b.spec.name)
                        } else {
                            "—".into()
                        }
                    })
                    .unwrap_or_else(|| "—".into());
                if dimension == "huge-file" {
                    let rss = result
                        .peak_rss_after
                        .map(|after| {
                            let before = result.peak_rss_before.unwrap_or(0);
                            format!("{} → {}", fmt_mb(before), fmt_mb(after))
                        })
                        .unwrap_or_else(|| "n/a".into());
                    let _ = writeln!(
                        out,
                        "| {} | {} | {} | {} |",
                        result.spec.name,
                        fmt_ms(med),
                        vs,
                        rss
                    );
                } else {
                    let _ = writeln!(out, "| {} | {} | {} |", result.spec.name, fmt_ms(med), vs);
                }
            }
        }
    }

    let _ = writeln!(
        out,
        "\n## Caveats\n\n\
         - Synthetic repos carry a single baseline commit, so the constant\n\
         \x20 recent-commits window (300) is under-represented; it doesn't\n\
         \x20 affect per-dimension shape.\n\
         - Dormant records accrue on vanished paths (the stash/revert\n\
         \x20 reality); live-path tier-1 scan cost is exercised by the hunks\n\
         \x20 dimension instead.\n\
         - Unassigned contrasts run with no changelists at all: the matcher\n\
         \x20 does no record work, isolating diff cost from matching cost.\n\
         - Absolute numbers are dev-machine optimistic (issue #16); the\n\
         \x20 exponents are the decision inputs."
    );
    out
}

/// One row per case, raw values — the recordable format alongside the
/// markdown (issue #29 acceptance criteria).
pub fn render_csv(results: &[CaseResult]) -> String {
    let mut out = String::from(
        "name,dimension,scale,files,hunks_per_file,changelists,live_records,dormant_records,\
         staged_files,huge_lines,content_bytes,hunks_total,median_ms,min_ms,max_ms,iterations,\
         peak_rss_before_bytes,peak_rss_after_bytes\n",
    );
    for result in results {
        let med = median(&result.times_ms);
        let (min, max) = spread(&result.times_ms);
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{},{},{},{},{},{},{:.3},{:.3},{:.3},{},{},{}",
            result.spec.name,
            result.spec.dimension,
            result.spec.scale,
            result.spec.files,
            result.spec.hunks_per_file,
            result.spec.changelists,
            result.live_records,
            result.dormant_records,
            result.spec.staged_files,
            result.spec.huge_lines,
            result.content_bytes,
            result.hunks_total,
            med,
            min,
            max,
            result.times_ms.len(),
            result.peak_rss_before.unwrap_or(0),
            result.peak_rss_after.unwrap_or(0),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::case::CaseSpec;
    use super::*;

    fn result(name: &str, dimension: &str, scale: u64, times: &[f64]) -> CaseResult {
        CaseResult {
            spec: CaseSpec {
                name: name.into(),
                dimension: dimension.into(),
                scale,
                files: 10,
                hunks_per_file: 4,
                changelists: 4,
                dormant: 0,
                staged_files: 0,
                huge_lines: 0,
                touch_every_iteration: false,
                warmup: 1,
                iterations: times.len(),
                contrast_of: None,
            },
            times_ms: times.to_vec(),
            hunks_total: 40,
            live_records: 40,
            dormant_records: 0,
            content_bytes: 0,
            peak_rss_before: None,
            peak_rss_after: None,
        }
    }

    #[test]
    fn median_handles_odd_and_even_counts() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), 2.5);
        assert_eq!(median(&[]), 0.0);
    }

    #[test]
    fn exponent_is_the_log_log_slope() {
        // 10× scale, 10× time → linear.
        let exp = exponent((10.0, 5.0), (100.0, 50.0)).unwrap();
        assert!((exp - 1.0).abs() < 1e-9);
        // 10× scale, 100× time → quadratic.
        let exp = exponent((10.0, 5.0), (100.0, 500.0)).unwrap();
        assert!((exp - 2.0).abs() < 1e-9);
    }

    #[test]
    fn exponent_refuses_unfittable_points() {
        assert!(exponent((0.0, 5.0), (100.0, 50.0)).is_none());
        assert!(exponent((10.0, 0.0), (100.0, 50.0)).is_none());
        assert!(exponent((10.0, 5.0), (10.0, 50.0)).is_none());
    }

    #[test]
    fn fit_exponent_regresses_over_all_points() {
        // Perfect quadratic through three decades.
        let quadratic = [(10.0, 1.0), (100.0, 100.0), (1000.0, 10000.0)];
        let exp = fit_exponent(&quadratic).unwrap();
        assert!((exp - 2.0).abs() < 1e-9);
        // A late knee: linear for three decades, then a quadratic step.
        // Regression weighs all four points (slope 1.6), where the
        // endpoint pair alone would report 5/3.
        let knee = [
            (10.0, 10.0),
            (100.0, 100.0),
            (1000.0, 1000.0),
            (10000.0, 1_000_000.0),
        ];
        let regressed = fit_exponent(&knee).unwrap();
        assert!((regressed - 1.6).abs() < 1e-9);
        assert!((exponent(knee[0], knee[3]).unwrap() - 5.0 / 3.0).abs() < 1e-9);
        // Zero-scale points are dropped; a single survivor can't fit.
        assert!(fit_exponent(&[(0.0, 5.0), (1000.0, 6.0)]).is_none());
    }

    #[test]
    fn spread_returns_min_and_max() {
        assert_eq!(spread(&[3.0, 1.0, 2.0]), (1.0, 3.0));
    }

    #[test]
    fn shape_labels_cover_the_classes() {
        assert_eq!(shape_label(0.5), "sub-linear");
        assert_eq!(shape_label(1.0), "≈ linear");
        assert_eq!(shape_label(1.4), "super-linear");
        assert_eq!(shape_label(2.0), "≈ quadratic or worse");
    }

    #[test]
    fn markdown_reports_shape_per_dimension_and_contrasts() {
        let mut contrast = result("files-100-unassigned", "files", 100, &[6.0]);
        contrast.spec.contrast_of = Some("files-100".into());
        contrast.spec.changelists = 0;
        let results = vec![
            result("files-10", "files", 10, &[1.0, 1.2, 1.1]),
            result("files-100", "files", 100, &[11.0, 12.0, 13.0]),
            contrast,
        ];
        let meta = Meta {
            date: "2026-07-31".into(),
            host: "test".into(),
            commit: "abc1234".into(),
            iterations: 3,
            warmup: 1,
        };
        let md = render_markdown(&results, &meta);
        assert!(md.contains("## files"));
        assert!(md.contains("| files-10 | 10 |"));
        // 10× scale, ~10.9× time → exponent ≈ 1.04, labelled linear.
        assert!(md.contains("≈ linear"), "{md}");
        assert!(md.contains("Contrasts:"));
        assert!(md.contains("0.50× of files-100"));
    }

    #[test]
    fn markdown_skips_shape_when_a_scale_is_zero() {
        let results = vec![
            result("dormant-0", "records", 0, &[5.0]),
            result("dormant-1000", "records", 1000, &[6.0]),
        ];
        let meta = Meta {
            date: String::new(),
            host: String::new(),
            commit: String::new(),
            iterations: 1,
            warmup: 1,
        };
        let md = render_markdown(&results, &meta);
        // The zero point can't anchor a log-log fit; with one usable
        // point no shape line renders for the dimension.
        assert!(!md.contains("Shape: t ~ scale^"), "{md}");
        assert!(md.contains("| dormant-0 | 0 |"));
    }

    #[test]
    fn csv_has_a_row_per_case_with_header() {
        let results = vec![
            result("files-10", "files", 10, &[1.0]),
            result("files-100", "files", 100, &[11.0]),
        ];
        let csv = render_csv(&results);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("name,dimension,scale"));
        assert!(lines[1].starts_with("files-10,files,10,"));
        let columns = lines[0].split(',').count();
        assert!(
            lines[1..].iter().all(|l| l.split(',').count() == columns),
            "ragged CSV rows"
        );
    }
}
