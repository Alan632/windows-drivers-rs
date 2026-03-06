// T057: Integration test for output capture with mock driver
use driver_test_cli::debug::{CaptureConfig, DebugCapture, DebugLevel};
use std::{fs::File, io::Write};

#[test]
fn test_debug_capture_integration() {
    // Create a temporary log file
    let temp_dir = std::env::temp_dir();
    let log_file = temp_dir.join(format!(
        "test_debug_{}.log",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));

    // Write initial debug messages
    {
        let mut f = File::create(&log_file).unwrap();
        writeln!(f, "MyDriver: Driver loaded successfully").unwrap();
        writeln!(f, "Warning: Low memory condition detected").unwrap();
        writeln!(f, "Error: Device initialization failed").unwrap();
    }

    // Start capture session
    let config = CaptureConfig {
        path: log_file.clone(),
        ..Default::default()
    };

    let mut session = DebugCapture::start(config);

    // Poll to read messages
    DebugCapture::poll(&mut session);

    // Verify captured messages
    assert_eq!(
        session.messages.len(),
        3,
        "Expected 3 messages to be captured"
    );

    // Verify message levels
    let levels: Vec<DebugLevel> = session.messages.iter().map(|m| m.level.clone()).collect();
    assert!(levels.contains(&DebugLevel::Info));
    assert!(levels.contains(&DebugLevel::Warn));
    assert!(levels.contains(&DebugLevel::Error));

    // Verify message content
    assert!(session
        .messages
        .iter()
        .any(|m| m.raw.contains("loaded successfully")));
    assert!(session
        .messages
        .iter()
        .any(|m| m.raw.contains("Low memory")));
    assert!(session
        .messages
        .iter()
        .any(|m| m.raw.contains("initialization failed")));

    // Add more messages
    {
        let mut f = File::options().append(true).open(&log_file).unwrap();
        writeln!(f, "Verbose: Processing request queue").unwrap();
        writeln!(f, "Info: Transaction completed").unwrap();
    }

    // Poll again
    DebugCapture::poll(&mut session);

    assert_eq!(session.messages.len(), 5, "Expected 5 total messages");

    // Stop capture and get all messages
    let all_messages = DebugCapture::stop(session);

    assert_eq!(all_messages.len(), 5);
    assert!(all_messages.iter().any(|m| m.level == DebugLevel::Verbose));

    // Cleanup
    std::fs::remove_file(log_file).ok();
}

#[test]
fn test_debug_capture_rotation() {
    let temp_dir = std::env::temp_dir();
    let log_file = temp_dir.join(format!(
        "test_rotation_{}.log",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));

    // Create log with many messages
    {
        let mut f = File::create(&log_file).unwrap();
        for i in 1..=100 {
            writeln!(f, "Message {}: Test data", i).unwrap();
        }
    }

    // Start capture with small max_messages limit
    let config = CaptureConfig {
        path: log_file.clone(),
        max_messages: 50, // Only keep last 50 messages
        ..Default::default()
    };

    let mut session = DebugCapture::start(config);
    DebugCapture::poll(&mut session);

    // Verify rotation occurred
    assert_eq!(
        session.messages.len(),
        50,
        "Should have rotated to max 50 messages"
    );

    // Verify oldest messages were dropped
    assert!(!session
        .messages
        .iter()
        .any(|m| m.raw.contains("Message 1:")));
    assert!(!session
        .messages
        .iter()
        .any(|m| m.raw.contains("Message 25:")));

    // Verify newest messages were kept
    assert!(session
        .messages
        .iter()
        .any(|m| m.raw.contains("Message 51:")));
    assert!(session
        .messages
        .iter()
        .any(|m| m.raw.contains("Message 100:")));

    // Cleanup
    std::fs::remove_file(log_file).ok();
}
