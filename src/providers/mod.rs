pub mod anthropic;
pub mod cerebras;
pub mod gemini;
pub mod groq;
pub mod openai;

use crate::config::ProviderConfig;
use reqwest::header::{HeaderMap, AUTHORIZATION};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAI,
    Groq,
    Cerebras,
    Gemini,
    Anthropic,
}

impl FromStr for ProviderKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(Self::OpenAI),
            "groq" => Ok(Self::Groq),
            "cerebras" => Ok(Self::Cerebras),
            "gemini" => Ok(Self::Gemini),
            "anthropic" => Ok(Self::Anthropic),
            _ => Err(format!("Unknown provider: {}", s)),
        }
    }
}

impl ProviderKind {
    pub(crate) fn upstream_headers(self, config: &ProviderConfig) -> Option<HeaderMap> {
        let key = config.api_key.as_deref()?.trim();
        if key.is_empty() {
            return None;
        }

        let bearer = format!("Bearer {key}");
        let mut headers = match self {
            Self::Anthropic => anthropic::build_request_headers(config, Some(&bearer)),
            Self::Groq => groq::build_request_headers(config, Some(&bearer)),
            Self::Cerebras => cerebras::build_request_headers(config, Some(&bearer)),
            Self::Gemini => gemini::build_request_headers(config, Some(&bearer)),
            Self::OpenAI => openai::build_request_headers(config, Some(&bearer)),
        };
        let auth = if self == Self::Anthropic {
            headers.get_mut("x-api-key")
        } else {
            headers.get_mut(AUTHORIZATION)
        };
        auth?.set_sensitive(true);
        Some(headers)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Groq => "groq",
            Self::Cerebras => "cerebras",
            Self::Gemini => "gemini",
            Self::Anthropic => "anthropic",
        }
    }
}

/// Extracts the provider kind and the stripped model name from the model string.
/// E.g., "groq/llama-3.1-8b-instant" -> Some((ProviderKind::Groq, "llama-3.1-8b-instant"))
pub fn extract_provider_and_model(full_model: &str) -> Option<(ProviderKind, String)> {
    if let Some((prefix, rest)) = full_model.split_once('/') {
        if let Ok(kind) = ProviderKind::from_str(prefix) {
            return Some((kind, rest.to_string()));
        }
    }
    None
}
