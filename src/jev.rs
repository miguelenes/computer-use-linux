use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

pub(crate) const ENABLE_ENV: &str = "COMPUTER_USE_LINUX_ENABLE_JEV";
pub(crate) const KEY_ENV: &str = "TYPESAFE_API_KEY";
const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const MODEL: &str = "jev-latest";
const RESPONSE_LIMIT: usize = 65_536;

pub(crate) fn configured() -> bool {
    std::env::var(ENABLE_ENV).as_deref() == Ok("1")
        && std::env::var(KEY_ENV).is_ok_and(|key| valid_api_key(&key))
}

pub(crate) fn configuration_message() -> Option<&'static str> {
    if std::env::var(ENABLE_ENV).as_deref() != Ok("1") {
        return Some(
            "Jev orchestration is disabled; set COMPUTER_USE_LINUX_ENABLE_JEV=1 to opt in.",
        );
    }
    match std::env::var(KEY_ENV) {
        Ok(key) if valid_api_key(&key) => None,
        Ok(_) => Some("TYPESAFE_API_KEY is malformed; Jev orchestration remains unavailable."),
        Err(_) => Some("TYPESAFE_API_KEY is missing; Jev orchestration remains unavailable."),
    }
}

fn valid_api_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 4096 && !key.bytes().any(|byte| byte.is_ascii_control())
}

pub(crate) fn request_choice(state: &Value, candidates: &[(String, String)]) -> Result<String> {
    if candidates.len() < 2 {
        bail!("not enough safe actions are available for a Jev decision");
    }
    let key = std::env::var(KEY_ENV).context("TYPESAFE_API_KEY is missing")?;
    if !valid_api_key(&key) {
        bail!("TYPESAFE_API_KEY is malformed");
    }
    let criteria: serde_json::Map<String, Value> = candidates
        .iter()
        .map(|(id, description)| (id.clone(), Value::String(description.clone())))
        .collect();
    let body = serde_json::json!({
        "model": MODEL,
        "state": state,
        "questions": {
            "next_action": {
                "type": "choice",
                "instructions": "Select the one candidate action that best advances the caller's task. Return only a candidate choice; do not invent actions or targets.",
                "criteria": criteria,
            }
        }
    });
    let body = serde_json::to_string(&body).context("could not serialize Jev decision request")?;
    let body_config = curl_config_string(&body)?;
    let key_config = curl_config_string(&format!("Authorization: Bearer {key}"))?;
    let config = format!(
        "url = \"{ENDPOINT}\"\nrequest = \"POST\"\nheader = \"Content-Type: application/json\"\nheader = \"{key_config}\"\ndata = \"{body_config}\"\nsilent\nshow-error\nfail-with-body\nmax-time = 15\nmax-filesize = {RESPONSE_LIMIT}\n"
    );
    let mut child = Command::new("curl")
        .args(["--config", "-"])
        .env_remove(KEY_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("curl is required for the opt-in Jev decision request")?;
    child
        .stdin
        .take()
        .context("could not open Jev request pipe")?
        .write_all(config.as_bytes())
        .context("could not send the Jev request")?;
    let output = child
        .wait_with_output()
        .context("could not read the Jev response")?;
    if !output.status.success() {
        bail!("Jev decision request failed (HTTP transport or provider error); no action was selected");
    }
    if output.stdout.len() > RESPONSE_LIMIT {
        bail!("Jev response exceeded the configured size limit; no action was selected");
    }
    let choice = parse_choice_response(&output.stdout)?;
    validate_candidate_choice(&choice, candidates)
}

fn parse_choice_response(body: &[u8]) -> Result<String> {
    let response: Value = serde_json::from_slice(body)
        .context("Jev returned malformed structured output; no action was selected")?;
    let answer = response
        .pointer("/answers/next_action")
        .context("Jev omitted the next-action answer")?;
    let confidence = answer
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let selected = answer
        .get("choice")
        .and_then(Value::as_str)
        .context("Jev did not select a valid action candidate")?;
    let selected_probability = answer
        .pointer(&format!(
            "/probabilities/{}",
            selected.replace('~', "~0").replace('/', "~1")
        ))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if confidence < 0.6 || selected_probability < 0.6 {
        bail!("Jev decision confidence was below the execution threshold");
    }
    Ok(selected.to_string())
}

fn validate_candidate_choice(choice: &str, candidates: &[(String, String)]) -> Result<String> {
    if candidates
        .iter()
        .any(|(candidate_id, _)| candidate_id == choice)
    {
        Ok(choice.to_string())
    } else {
        bail!("Jev abstained or selected a stale candidate; no action was selected")
    }
}

fn curl_config_string(value: &str) -> Result<String> {
    if value.bytes().any(|byte| byte.is_ascii_control()) {
        bail!("Jev request contains a disallowed control character");
    }
    Ok(value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::{
        curl_config_string, parse_choice_response, valid_api_key, validate_candidate_choice,
    };

    #[test]
    fn key_validation_rejects_empty_or_header_injection() {
        assert!(valid_api_key("ts_example-secret"));
        assert!(!valid_api_key(""));
        assert!(!valid_api_key("key\r\nInjected: yes"));
    }

    #[test]
    fn curl_config_escapes_quotes_and_rejects_newlines() {
        assert_eq!(curl_config_string("x\\\"y").unwrap(), "x\\\\\\\"y");
        assert!(curl_config_string("x\ny").is_err());
    }

    #[test]
    fn malformed_abstaining_and_low_confidence_responses_are_rejected() {
        assert!(parse_choice_response(b"not json").is_err());
        assert!(parse_choice_response(br#"{"answers":{"next_action":{"type":"choice","choice":"abstain","confidence":0.99,"probabilities":{"abstain":1.0}}}}"#).is_ok());
        assert!(parse_choice_response(br#"{"answers":{"next_action":{"type":"choice","choice":"candidate","confidence":0.51,"probabilities":{"candidate":0.9}}}}"#).is_err());
        assert!(parse_choice_response(br#"{"answers":{"next_action":{"type":"choice","choice":"candidate","confidence":0.9,"probabilities":{"candidate":0.9}}}}"#).is_ok());
        assert!(parse_choice_response(br#"{"answers":{"next_action":{"type":"choice","choice":"candidate-a","confidence":0.9,"probabilities":{"candidate-a":0.2,"candidate-b":0.8}}}}"#).is_err());
        let candidates = vec![("fresh-id".to_string(), "safe action".to_string())];
        assert!(validate_candidate_choice("abstain", &candidates).is_err());
        assert!(validate_candidate_choice("stale-id", &candidates).is_err());
        assert_eq!(
            validate_candidate_choice("fresh-id", &candidates).unwrap(),
            "fresh-id"
        );
    }
}
