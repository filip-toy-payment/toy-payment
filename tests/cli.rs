//! Runs the compiled binary on the sample files in `tests/fixtures`.
//!
//! Each `<name>.csv` is an input. The matching `<name>.expected.csv` holds the
//! standard output the binary must produce for it. The expected files were
//! written by hand from the rules. They are not recorded program output.

// The lint exemption for tests covers only functions marked as tests. The
// helpers in this file are test code too.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(file_name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(file_name)
}

fn run(arguments: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_toy-payment"))
        .args(arguments)
        .output()
        .unwrap()
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap()
}

/// Runs the binary on a fixture and checks its standard output.
///
/// Returns what the binary wrote to standard error.
fn check(name: &str) -> String {
    let output = run(&[&fixture(&format!("{name}.csv"))]);
    let expected = fs::read_to_string(fixture(&format!("{name}.expected.csv"))).unwrap();

    assert!(output.status.success(), "{name}: {:?}", output.status);
    assert_eq!(text(output.stdout), expected, "{name}");
    text(output.stderr)
}

/// The record numbers named in the log lines.
fn logged_records(diagnostics: &str) -> Vec<u64> {
    diagnostics
        .lines()
        .map(|line| {
            let number = line.strip_prefix("record ").unwrap();
            let (number, _) = number.split_once(':').unwrap();
            number.parse().unwrap()
        })
        .collect()
}

#[test]
fn basic_deposits_and_withdrawals() {
    assert_eq!(check("basic"), "");
}

#[test]
fn disputes_resolves_and_chargebacks() {
    assert_eq!(check("disputes"), "");
}

#[test]
fn rows_the_engine_ignores_are_not_logged() {
    assert_eq!(check("ignored_rows"), "");
}

#[test]
fn bad_rows_are_skipped_and_logged() {
    let diagnostics = check("bad_rows");

    assert_eq!(
        logged_records(&diagnostics),
        [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
    );
}

#[test]
fn flexible_formatting_is_accepted() {
    assert_eq!(check("formatting"), "");
}

#[test]
fn a_file_without_a_header_loses_no_row() {
    assert_eq!(check("no_header"), "");
}

#[test]
fn amounts_stay_exact() {
    assert_eq!(check("precision"), "");
}

#[test]
fn an_empty_file_gives_only_the_header() {
    assert_eq!(check("empty"), "");
}

#[test]
fn a_missing_argument_is_a_usage_error() {
    let output = run(&[]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(text(output.stderr).starts_with("usage:"));
}

#[test]
fn a_second_argument_is_a_usage_error() {
    let input = fixture("basic.csv");
    let output = run(&[&input, &input]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(text(output.stderr).starts_with("usage:"));
}

#[test]
fn a_file_that_cannot_be_opened_is_an_error() {
    let output = run(&[&fixture("does_not_exist.csv")]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(text(output.stderr).starts_with("error: cannot open"));
}

#[test]
fn a_file_that_cannot_be_read_is_an_error() {
    // A directory opens but cannot be read.
    let output = run(&[&fixture("")]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(text(output.stderr).starts_with("error: cannot read input"));
}
