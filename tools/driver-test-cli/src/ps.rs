use serde_json::Value;
use std::process::Command;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PsError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("PowerShell JSON parse error\nCommand: {command}\nStdout: {stdout}\nStderr: {stderr}\nError: {error}")]
    Json {
        command: String,
        stdout: String,
        stderr: String,
        error: String,
    },
    #[error("PowerShell execution failed (exit code {exit_code})\nCommand: {command}\nStdout: {stdout}\nStderr: {stderr}")]
    Fail {
        command: String,
        stdout: String,
        stderr: String,
        exit_code: i32,
    },
}

pub fn run_ps_json(script: &str) -> Result<Value, PsError> {
    let needs_convert = !script.contains("ConvertTo-Json");
    let body = if needs_convert {
        format!("{script} | ConvertTo-Json -Compress")
    } else {
        script.to_string()
    };
    let wrapped = format!("$ErrorActionPreference='Stop'; try {{ {body} }} catch {{ $_ | ConvertTo-Json -Compress | Write-Error; exit 2 }}");
    let out = Command::new("powershell")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &wrapped,
        ])
        .output()?;
    
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let exit_code = out.status.code().unwrap_or(-1);
    
    if !out.status.success() {
        return Err(PsError::Fail {
            command: script.to_string(),
            stdout,
            stderr,
            exit_code,
        });
    }
    
    serde_json::from_slice(&out.stdout).map_err(|e| PsError::Json {
        command: script.to_string(),
        stdout: stdout.clone(),
        stderr,
        error: e.to_string(),
    })
}
