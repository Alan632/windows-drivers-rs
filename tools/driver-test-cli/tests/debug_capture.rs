// T056: Unit tests for message classification
use driver_test_cli::debug::{classify, validate_output_patterns, DebugLevel};

#[test]
fn test_classify_info_message() {
    let msg = classify("Driver initialized successfully");
    assert_eq!(msg.level, DebugLevel::Info);
    assert_eq!(msg.raw, "Driver initialized successfully");
}

#[test]
fn test_classify_warning_message() {
    let msg = classify("Warning: Buffer size exceeds threshold");
    assert_eq!(msg.level, DebugLevel::Warn);
    assert!(msg.raw.contains("Warning"));
}

#[test]
fn test_classify_error_message() {
    let msg = classify("Error: Failed to allocate memory");
    assert_eq!(msg.level, DebugLevel::Error);
    assert!(msg.raw.contains("Error"));
}

#[test]
fn test_classify_verbose_message() {
    let msg = classify("Verbose: Entering function ProcessRequest");
    assert_eq!(msg.level, DebugLevel::Verbose);
}

#[test]
fn test_classify_extracts_source() {
    let msg = classify("MyDriver: Device ready");
    assert_eq!(msg.source, Some("MyDriver".to_string()));
    assert!(msg.raw.contains("Device ready"));
}

#[test]
fn test_classify_no_source() {
    let msg = classify("Simple message without source");
    assert_eq!(msg.source, None);
}

#[test]
fn test_validate_output_patterns_all_found() {
    let messages = vec![
        classify("Driver initialized"),
        classify("Device connected"),
        classify("Ready to process"),
    ];

    let patterns = vec!["initialized".to_string(), "connected".to_string()];
    let missing = validate_output_patterns(&messages, &patterns);

    assert!(missing.is_empty(), "Expected all patterns to be found");
}

#[test]
fn test_validate_output_patterns_some_missing() {
    let messages = vec![classify("Driver initialized"), classify("Device connected")];

    let patterns = vec![
        "initialized".to_string(),
        "connected".to_string(),
        "ready".to_string(), // This pattern is missing
    ];
    let missing = validate_output_patterns(&messages, &patterns);

    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0], "ready");
}

#[test]
fn test_validate_output_patterns_case_sensitive() {
    let messages = vec![classify("Driver INITIALIZED")];

    let patterns = vec!["initialized".to_string()]; // lowercase
    let missing = validate_output_patterns(&messages, &patterns);

    // Pattern matching is case-sensitive via contains()
    assert_eq!(
        missing.len(),
        1,
        "Pattern should not match due to case difference"
    );
}
