use serde::Serialize;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
use tracing::warn;

// T045: DebugMessage struct with message_text, timestamp, source, level fields
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DebugLevel {
    Info,
    Warn,
    Error,
    Verbose,
}

#[derive(Debug, Clone, Serialize)]
pub struct DebugMessage {
    #[serde(rename = "message")]
    pub raw: String,
    pub level: DebugLevel,
    #[serde(skip)] // Instant is not serializable
    pub ts: Instant,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

pub fn classify(line: &str) -> DebugMessage {
    let lower = line.to_ascii_lowercase();
    let level = if lower.contains("error") || lower.contains("fail") {
        DebugLevel::Error
    } else if lower.contains("warn") || lower.contains("deprecated") {
        DebugLevel::Warn
    } else if lower.contains("verbose") || lower.contains("trace") {
        DebugLevel::Verbose
    } else {
        DebugLevel::Info
    };
    // crude source extraction: prefix before ':' if present
    let source = line
        .split_once(':')
        .map(|(s, _)| s.trim().to_string())
        .filter(|s| s.len() <= 64);
    DebugMessage {
        raw: line.to_string(),
        level,
        ts: Instant::now(),
        source,
    }
}

#[derive(Debug)]
pub struct CaptureConfig {
    pub path: PathBuf,
    pub poll_interval: Duration,
    pub max_messages: usize,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("debugview.log"),
            poll_interval: Duration::from_millis(200),
            max_messages: 1000,
        }
    }
}

#[derive(Debug)]
pub struct CaptureSession {
    cfg: CaptureConfig,
    last_len: usize,
    pub messages: Vec<DebugMessage>,
    started: Instant,
    last_poll: Instant,
}

impl CaptureSession {
    pub fn new(cfg: CaptureConfig) -> Self {
        let now = Instant::now();
        let interval = cfg.poll_interval;
        let last_poll = now.checked_sub(interval).unwrap_or(now);
        Self {
            cfg,
            last_len: 0,
            messages: Vec::new(),
            started: now,
            last_poll,
        }
    }
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
    pub fn duration_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }
}

// T046: DebugOutputCapture trait for capture abstraction
pub trait DebugOutputCapture {
    fn start_capture(&mut self, cfg: CaptureConfig) -> Result<CaptureSession, String>;
    fn read_messages(&self, session: &mut CaptureSession) -> Result<Vec<DebugMessage>, String>;
    fn stop_capture(&mut self, session: CaptureSession) -> Result<Vec<DebugMessage>, String>;
}

pub struct DebugCapture;

impl DebugCapture {
    // T047: start_capture with DebugView deployment to VM (simplified file-based logging)
    pub fn start(cfg: CaptureConfig) -> CaptureSession {
        CaptureSession::new(cfg)
    }

    // T048: read_messages with real-time streaming (via polling)
    pub fn poll(session: &mut CaptureSession) {
        let _elapsed = session.last_poll.elapsed();
        let path = &session.cfg.path;
        let Ok(content) = fs::read_to_string(path) else {
            return;
        };
        let lines: Vec<&str> = content.lines().collect();
        let new = &lines[session.last_len..];
        for l in new {
            let msg = classify(l);
            session.messages.push(msg);
        }
        session.last_len = lines.len();
        session.last_poll = Instant::now();
        // rotation if exceeding
        if session.messages.len() > session.cfg.max_messages {
            let overflow = session.messages.len() - session.cfg.max_messages;
            let elapsed_ms = session.elapsed().as_millis();
            warn!(
                overflow,
                elapsed_ms, "debug capture rotation dropping oldest messages"
            );
            session.messages.drain(0..overflow);
        }
    }

    // T049: stop_capture with log collection
    pub fn stop(mut session: CaptureSession) -> Vec<DebugMessage> {
        Self::poll(&mut session); // final poll
        session.messages
    }
}

impl DebugOutputCapture for DebugCapture {
    fn start_capture(&mut self, cfg: CaptureConfig) -> Result<CaptureSession, String> {
        Ok(Self::start(cfg))
    }

    fn read_messages(&self, session: &mut CaptureSession) -> Result<Vec<DebugMessage>, String> {
        Self::poll(session);
        Ok(session.messages.clone())
    }

    fn stop_capture(&mut self, session: CaptureSession) -> Result<Vec<DebugMessage>, String> {
        Ok(Self::stop(session))
    }
}

// T050: Message classification (Info/Warning/Error) - implemented in classify()
// T051: Timestamp parsing and correlation - timestamps captured via Instant::now()

// T053: Validate output patterns for expected message matching
pub fn validate_output_patterns(messages: &[DebugMessage], patterns: &[String]) -> Vec<String> {
    let mut missing = Vec::new();
    for pattern in patterns {
        let found = messages.iter().any(|msg| msg.raw.contains(pattern));
        if !found {
            missing.push(pattern.clone());
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, io::Write};

    #[test]
    fn capture_reads_new_lines_and_rotates() {
        let log_path = std::env::temp_dir().join(format!(
            "dbg_{}.log",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        let mut f = File::create(&log_path).unwrap();
        writeln!(f, "alpha info").unwrap();
        let mut session = DebugCapture::start(CaptureConfig {
            path: log_path.clone(),
            poll_interval: Duration::from_millis(10),
            max_messages: 3,
        });
        DebugCapture::poll(&mut session);
        assert_eq!(session.messages.len(), 1);
        // append more lines
        writeln!(f, "beta warn something").unwrap();
        writeln!(f, "gamma error occurred").unwrap();
        writeln!(f, "delta verbose trace").unwrap();
        DebugCapture::poll(&mut session);
        // max_messages=3 should rotate dropping oldest (alpha)
        assert_eq!(session.messages.len(), 3);
        assert!(session.messages.iter().all(|m| m.raw != "alpha info"));
        let levels: Vec<DebugLevel> = session.messages.iter().map(|m| m.level.clone()).collect();
        assert!(levels.contains(&DebugLevel::Warn));
        assert!(levels.contains(&DebugLevel::Error));
        assert!(levels.contains(&DebugLevel::Verbose));
    }
}
