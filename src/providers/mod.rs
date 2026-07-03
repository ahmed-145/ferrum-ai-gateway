pub mod anthropic;
pub mod cerebras;
pub mod gemini;
pub mod groq;
pub mod openai;

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
