use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{DiscussError, Result};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VerdictStyle {
    Positive,
    Neutral,
    Negative,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerdictOption {
    pub id: String,
    pub label: String,
    pub style: VerdictStyle,
    pub feedback_required: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerdictConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    pub options: Vec<VerdictOption>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub option_id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    pub decided_at: DateTime<Utc>,
}

pub fn parse_verdict_config(spec: &str, prompt: Option<String>) -> Result<VerdictConfig> {
    let options = parse_verdict_options(spec)?;

    Ok(VerdictConfig { prompt, options })
}

pub fn parse_verdict_options(spec: &str) -> Result<Vec<VerdictOption>> {
    let mut options = Vec::new();
    let mut ids = HashSet::new();
    let mut labels = HashSet::new();

    for raw_option in spec.split('|') {
        let option = parse_verdict_option(raw_option)?;

        if !ids.insert(option.id.clone()) {
            return Err(verdict_spec_error(format!(
                "duplicate verdict option id: {}",
                option.id
            )));
        }

        let folded_label = option.label.to_lowercase();
        if !labels.insert(folded_label) {
            return Err(verdict_spec_error(format!(
                "duplicate verdict option label: {}",
                option.label
            )));
        }

        options.push(option);
    }

    if options.len() < 2 {
        return Err(verdict_spec_error(
            "--verdict-options requires at least 2 options".to_string(),
        ));
    }

    Ok(options)
}

fn parse_verdict_option(raw_option: &str) -> Result<VerdictOption> {
    let (raw_option, feedback_required) = raw_option
        .strip_suffix('!')
        .map(|trimmed| (trimmed, true))
        .unwrap_or((raw_option, false));
    let parts = raw_option.split(':').collect::<Vec<_>>();

    if parts.len() > 3 {
        return Err(verdict_spec_error(format!(
            "invalid verdict option {raw_option:?}: expected id[:label][:style][!]"
        )));
    }

    let id = parts.first().copied().unwrap_or_default();
    if !is_valid_id(id) {
        return Err(verdict_spec_error(format!(
            "invalid verdict option id {id:?}: use only lowercase letters, digits, '-' or '_'"
        )));
    }

    let (label, style) = match parts.as_slice() {
        [id] => (title_case_id(id), VerdictStyle::Neutral),
        [id, second] if is_style(second) => (title_case_id(id), parse_style(second)?),
        [_id, label] => (label.to_string(), VerdictStyle::Neutral),
        [id, label, style] => {
            let label = if label.is_empty() {
                title_case_id(id)
            } else {
                label.to_string()
            };
            (label, parse_style(style)?)
        }
        _ => unreachable!("parts length checked above"),
    };

    if label.trim().is_empty() {
        return Err(verdict_spec_error(format!(
            "verdict option {id:?} label must not be blank"
        )));
    }

    Ok(VerdictOption {
        id: id.to_string(),
        label,
        style,
        feedback_required,
    })
}

fn parse_style(style: &str) -> Result<VerdictStyle> {
    match style {
        "positive" => Ok(VerdictStyle::Positive),
        "neutral" => Ok(VerdictStyle::Neutral),
        "negative" => Ok(VerdictStyle::Negative),
        _ => Err(verdict_spec_error(format!(
            "invalid verdict option style {style:?}: expected positive, neutral, or negative"
        ))),
    }
}

fn is_style(value: &str) -> bool {
    matches!(value, "positive" | "neutral" | "negative")
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

fn title_case_id(id: &str) -> String {
    id.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn verdict_spec_error(message: String) -> DiscussError {
    DiscussError::VerdictSpecError { message }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(spec: &str) -> Vec<VerdictOption> {
        parse_verdict_options(spec).expect("verdict spec should parse")
    }

    fn error_message(spec: &str) -> String {
        parse_verdict_options(spec)
            .expect_err("verdict spec should fail")
            .to_string()
    }

    #[test]
    fn parses_default_labels_and_neutral_style() {
        let options = parse("approved|needs_review");

        assert_eq!(
            options,
            vec![
                VerdictOption {
                    id: "approved".to_string(),
                    label: "Approved".to_string(),
                    style: VerdictStyle::Neutral,
                    feedback_required: false,
                },
                VerdictOption {
                    id: "needs_review".to_string(),
                    label: "Needs Review".to_string(),
                    style: VerdictStyle::Neutral,
                    feedback_required: false,
                },
            ]
        );
    }

    #[test]
    fn parses_labels_styles_and_required_feedback() {
        let options = parse("approved:Approve:positive|declined:Decline:negative!");

        assert_eq!(options[0].label, "Approve");
        assert_eq!(options[0].style, VerdictStyle::Positive);
        assert!(!options[0].feedback_required);
        assert_eq!(options[1].label, "Decline");
        assert_eq!(options[1].style, VerdictStyle::Negative);
        assert!(options[1].feedback_required);
    }

    #[test]
    fn parses_style_without_label() {
        let options = parse("approved:positive|declined:negative!");

        assert_eq!(options[0].label, "Approved");
        assert_eq!(options[0].style, VerdictStyle::Positive);
        assert_eq!(options[1].label, "Declined");
        assert_eq!(options[1].style, VerdictStyle::Negative);
        assert!(options[1].feedback_required);
    }

    #[test]
    fn rejects_fewer_than_two_options() {
        let message = error_message("approved");

        assert!(message.contains("at least 2 options"));
    }

    #[test]
    fn rejects_duplicate_ids() {
        let message = error_message("approved:Approve|approved:Ship it");

        assert!(message.contains("duplicate verdict option id"));
    }

    #[test]
    fn rejects_duplicate_labels_case_insensitive() {
        let message = error_message("approved:Approve|declined:approve");

        assert!(message.contains("duplicate verdict option label"));
    }

    #[test]
    fn rejects_invalid_style() {
        let message = error_message("approved:Approve:good|declined");

        assert!(message.contains("invalid verdict option style"));
    }

    #[test]
    fn rejects_invalid_id_charset() {
        let message = error_message("Approved|declined");

        assert!(message.contains("invalid verdict option id"));
    }

    #[test]
    fn rejects_blank_label() {
        let message = error_message("approved:|declined");

        assert!(message.contains("label must not be blank"));
    }
}
