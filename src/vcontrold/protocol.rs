//! vcontrold TCP protocol handling
//!
//! Protocol constants and response parsing for vcontrold communication.

use crate::error::VcontroldError;

/// Prompt string sent by vcontrold when ready for commands
pub const PROMPT: &str = "vctrld>";

/// Goodbye message sent when disconnecting
#[allow(dead_code)]
pub const BYE: &str = "good bye!";

/// Error prefix in vcontrold responses
pub const ERR_PREFIX: &str = "ERR:";

/// Result of executing a command
#[derive(Debug, Clone)]
pub struct CommandResult {
    /// The command that was executed
    pub command: String,
    /// The parsed value (numeric or string)
    pub value: Value,
    /// Raw response string (useful for debugging)
    #[allow(dead_code)]
    pub raw: String,
    /// Error message if command failed
    pub error: Option<String>,
}

/// A value returned by vcontrold
#[derive(Debug, Clone)]
pub enum Value {
    /// Numeric value (float)
    Number(f64),
    /// String value
    String(String),
    /// No value / error
    None,
}

impl Value {
    /// Convert to JSON-compatible representation
    pub fn to_json_value(&self) -> serde_json::Value {
        match self {
            Value::Number(n) => serde_json::json!(*n),
            Value::String(s) => serde_json::json!(s),
            Value::None => serde_json::Value::Null,
        }
    }
}

/// Parse a raw response line from vcontrold
///
/// Response format: "value unit" or "value" or "ERR: message"
pub fn parse_response(command: &str, raw: &str) -> CommandResult {
    let raw = raw.trim();

    // Check for error response
    if raw.starts_with(ERR_PREFIX) {
        return CommandResult {
            command: command.to_string(),
            value: Value::None,
            raw: raw.to_string(),
            error: Some(raw.to_string()),
        };
    }

    // Try to parse as number (first word)
    let first_word = raw.split_whitespace().next().unwrap_or(raw);
    let value = if let Ok(num) = first_word.parse::<f64>() {
        Value::Number(num)
    } else if !raw.is_empty() {
        Value::String(raw.to_string())
    } else {
        Value::None
    };

    CommandResult {
        command: command.to_string(),
        value,
        raw: raw.to_string(),
        error: None,
    }
}

/// Format a command for sending to vcontrold
pub fn format_command(cmd: &str) -> String {
    format!("{}\n", cmd.trim())
}

/// Format quit command
pub fn format_quit() -> String {
    "quit\n".to_string()
}

/// Check if a buffer contains the prompt
#[allow(dead_code)]
pub fn has_prompt(buffer: &str) -> bool {
    buffer.contains(PROMPT)
}

/// Extract response from buffer (everything before the prompt)
pub fn extract_response(buffer: &str) -> Option<&str> {
    buffer.find(PROMPT).map(|idx| buffer[..idx].trim())
}

/// Check if response indicates an error
#[allow(dead_code)]
pub fn is_error_response(response: &str) -> bool {
    response.starts_with(ERR_PREFIX)
}

/// Build JSON output matching vclient -j format
///
/// Format: {"command1":value1,"command2":value2}
pub fn build_json_response(results: &[CommandResult]) -> String {
    let mut map = serde_json::Map::new();
    for result in results {
        if result.error.is_none() {
            map.insert(result.command.clone(), result.value.to_json_value());
        }
    }
    serde_json::Value::Object(map).to_string()
}

/// Validate that a command string is safe to send
pub fn validate_command(cmd: &str) -> Result<(), VcontroldError> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return Err(VcontroldError::Command("empty command".to_string()));
    }
    // Commands shouldn't contain control characters
    if cmd.chars().any(|c| c.is_control() && c != ' ') {
        return Err(VcontroldError::Command(
            "command contains invalid characters".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_numeric_response() {
        let result = parse_response("getTempWWObenIst", "48.1 Grad Celsius");
        assert!(matches!(result.value, Value::Number(n) if (n - 48.1).abs() < 0.001));
        assert!(result.error.is_none());
    }

    #[test]
    fn test_parse_error_response() {
        let result = parse_response("badCommand", "ERR: command unknown");
        assert!(matches!(result.value, Value::None));
        assert!(result.error.is_some());
    }

    #[test]
    fn test_parse_string_response() {
        let result = parse_response("getStatus", "OK");
        assert!(matches!(result.value, Value::String(ref s) if s == "OK"));
        assert!(result.error.is_none());
    }

    #[test]
    fn test_build_json_response() {
        let results = vec![
            CommandResult {
                command: "getTempA".to_string(),
                value: Value::Number(21.5),
                raw: "21.5 Grad".to_string(),
                error: None,
            },
            CommandResult {
                command: "getTempB".to_string(),
                value: Value::Number(45.0),
                raw: "45.0 Grad".to_string(),
                error: None,
            },
        ];
        let json = build_json_response(&results);
        assert!(json.contains("\"getTempA\":21.5"));
        assert!(json.contains("\"getTempB\":45"));
    }
}
