use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

use crate::AppState;
use crate::middleware::auth::ClientAuth;
use crate::providers::{anthropic, extract_provider_and_model, ProviderKind};

#[derive(serde::Deserialize)]
struct ChatCompletionRequestHelper {
    model: String,
    stream: Option<bool>,
}

/// Handler for POST /v1/chat/completions
pub async fn handle_chat_completions(
    State(app_state): State<Arc<AppState>>,
    auth: ClientAuth,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let start_time = Instant::now();

    // 1. Extract model string from the request JSON
    let helper: ChatCompletionRequestHelper = match serde_json::from_slice(&body) {
        Ok(h) => h,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Invalid JSON request body: {}", e),
            )
                .into_response();
        }
    };

    let full_model = helper.model;
    let is_streaming = helper.stream.unwrap_or(false);

    // 2. Parse provider prefix (e.g. "groq/llama-3.1-8b-instant")
    let (mut current_provider, mut current_model) = match extract_provider_and_model(&full_model) {
        Some(res) => res,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "Invalid model name format. Must be prefixed with provider name, e.g. 'groq/llama-3.1-8b-instant'",
            )
                .into_response();
        }
    };

    let original_provider = current_provider;
    let original_model = current_model.clone();

    // 3. Apply lock-free sliding window rate limiting
    let api_key = &auth.0;
    let tenant = headers
        .get("x-ferrum-tenant")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("default-tenant");
    let provider_str = current_provider.as_str();

    let key_limit = format!("key:{}", api_key);
    let tenant_limit = format!("tenant:{}", tenant);
    let provider_limit = format!("provider:{}", provider_str);

    if !app_state.rate_limiter.check_limit(&key_limit)
        || !app_state.rate_limiter.check_limit(&tenant_limit)
        || !app_state.rate_limiter.check_limit(&provider_limit)
    {
        app_state.metrics.record_request(
            provider_str,
            &current_model,
            "/v1/chat/completions",
            429,
            Duration::from_secs(0),
        );
        return (StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded").into_response();
    }

    // 4. Fallback and retry loop
    let Some(mut provider_config) = app_state.config.providers.get(current_provider.as_str()) else {
        return (
            StatusCode::BAD_REQUEST,
            format!("Provider {} not configured", current_provider.as_str()),
        )
            .into_response();
    };
    let Some(mut outbound_headers) = current_provider.upstream_headers(provider_config) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "No usable API key configured for provider {}",
                current_provider.as_str()
            ),
        )
            .into_response();
    };

    let fallback_config = app_state.config.fallback.as_ref().filter(|cfg| cfg.enabled);
    let mut fallbacks = fallback_config.map(|cfg| {
        std::iter::once(&cfg.default_fallback_model)
            .chain(&cfg.additional_fallback_models)
            .filter_map(|model| {
                let (provider, model) = extract_provider_and_model(model)?;
                if provider == original_provider && model == original_model {
                    return None;
                }
                let config = app_state.config.providers.get(provider.as_str())?;
                let headers = provider.upstream_headers(config)?;
                Some((provider, model, config, headers))
            })
            .take(cfg.max_attempts as usize)
    });
    let mut attempt = 0;
    let mut used_fallback = false;

    loop {
        // Determine request details
        let target_url: String;
        let target_body: Vec<u8>;

        if current_provider == ProviderKind::Anthropic {
            target_url = format!("{}/messages", provider_config.base_url);

            // Parse request and translate to Anthropic format
            match serde_json::from_slice::<anthropic::OpenAIRequest>(&body) {
                Ok(openai_req) => {
                    let anth_req = anthropic::translate_request(openai_req, current_model.clone());
                    target_body = serde_json::to_vec(&anth_req).unwrap();
                }
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        format!("Failed to parse OpenAI request for Anthropic translation: {}", e),
                    )
                        .into_response();
                }
            }
        } else {
            target_url = format!("{}/chat/completions", provider_config.base_url);
            // Replace the model key in the request JSON
            match serde_json::from_slice::<serde_json::Value>(&body) {
                Ok(mut val) => {
                    val["model"] = serde_json::Value::String(current_model.clone());
                    target_body = serde_json::to_vec(&val).unwrap();
                }
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, format!("Invalid request body: {}", e)).into_response();
                }
            }
        }

        info!(
            "Forwarding request to provider={} model={} (attempt {})",
            current_provider.as_str(),
            current_model,
            attempt + 1
        );

        let req_builder = app_state
            .client
            .post(&target_url)
            .headers(outbound_headers.clone())
            .body(target_body)
            .timeout(Duration::from_secs(provider_config.timeout_secs));

        let (mut last_status, mut last_error_msg) = match req_builder.send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    // Log success metric
                    app_state.metrics.record_request(
                        current_provider.as_str(),
                        &current_model,
                        "/v1/chat/completions",
                        status.as_u16(),
                        start_time.elapsed(),
                    );

                    // Transform response
                    if current_provider == ProviderKind::Anthropic {
                        if is_streaming {
                            let mut translator = anthropic::AnthropicStreamTranslator::new();
                            let mapped_stream = response.bytes_stream().map(move |chunk_result| {
                                match chunk_result {
                                    Ok(chunk) => {
                                        let lines = translator.process_chunk(&chunk);
                                        Ok::<_, std::io::Error>(Bytes::from(lines.join("")))
                                    }
                                    Err(e) => Err(std::io::Error::other(e)),
                                }
                            });

                            let mut res = Response::new(Body::from_stream(mapped_stream));
                            res.headers_mut().insert("content-type", "text/event-stream".parse().unwrap());
                            res.headers_mut().insert("cache-control", "no-cache".parse().unwrap());
                            res.headers_mut().insert("connection", "keep-alive".parse().unwrap());
                            return res;
                        } else {
                            // Unary Anthropic response translation
                            match response.json::<anthropic::AnthropicResponse>().await {
                                Ok(anth_resp) => {
                                    let openai_resp = anthropic::translate_response(anth_resp);
                                    let mut res = Response::new(Body::from(serde_json::to_vec(&openai_resp).unwrap()));
                                    res.headers_mut().insert("content-type", "application/json".parse().unwrap());
                                    return res;
                                }
                                Err(e) => {
                                    error!("Failed to parse Anthropic response: {}", e);
                                    return (StatusCode::BAD_GATEWAY, "Failed to parse provider response").into_response();
                                }
                            }
                        }
                    } else {
                        // Standard passthrough
                        let mut res_builder = Response::builder().status(status);
                        for (key, val) in response.headers().iter() {
                            let k = key.as_str().to_lowercase();
                            if k != "content-encoding"
                                && k != "content-length"
                                && k != "transfer-encoding"
                                && k != "connection"
                            {
                                res_builder = res_builder.header(key, val);
                            }
                        }

                        let res_stream = response.bytes_stream().map(|chunk_result| {
                            chunk_result.map_err(std::io::Error::other)
                        });

                        return res_builder.body(Body::from_stream(res_stream)).unwrap();
                    }
                } else {
                    let status_u16 = status.as_u16();
                    // Per FR-4: only retry/fallback on 429 or 5xx — not on 4xx client errors
                    let is_retryable = status_u16 == 429 || status_u16 >= 500;
                    warn!(
                        "Provider {} returned status {} (retryable={})",
                        current_provider.as_str(),
                        status_u16,
                        is_retryable,
                    );
                    if !is_retryable {
                        // Non-retryable 4xx — return immediately without burning retries
                        app_state.metrics.record_request(
                            current_provider.as_str(),
                            &current_model,
                            "/v1/chat/completions",
                            status_u16,
                            start_time.elapsed(),
                        );
                        return (StatusCode::from_u16(status_u16).unwrap_or(StatusCode::BAD_GATEWAY),
                            format!("Upstream returned non-retryable status {}", status_u16))
                            .into_response();
                    }
                    (status, format!("Upstream provider returned status {}", status_u16))
                }
            }
            Err(e) => {
                error!("Connection to provider {} failed: {}", current_provider.as_str(), e);
                (StatusCode::BAD_GATEWAY, format!("Failed to reach upstream provider: {}", e))
            }
        };

        // Retry / Fallback decision logic
        attempt += 1;
        let max_retries = provider_config.max_retries;

        if attempt <= max_retries {
            // Exponential backoff with jitter per FR-4
            let backoff_ms = app_state.config.fallback.as_ref().map(|f| f.backoff_ms).unwrap_or(500);
            let base_delay = backoff_ms * 2u64.pow(attempt - 1);
            // Jitter: ±25% of base delay using time-based entropy
            let jitter_range = base_delay / 4;
            let time_entropy = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos() as u64;
            let jitter = time_entropy % (jitter_range.max(1) * 2);
            let sleep_ms = base_delay.saturating_sub(jitter_range).saturating_add(jitter);
            let sleep_duration = Duration::from_millis(sleep_ms);
            warn!(
                "Retrying provider {} in {}ms with jitter (attempt {}/{})",
                current_provider.as_str(),
                sleep_duration.as_millis(),
                attempt,
                max_retries
            );
            tokio::time::sleep(sleep_duration).await;
        } else {
            if let Some((provider, model, config, headers)) = fallbacks.as_mut().and_then(Iterator::next) {
                warn!("Falling back to provider {} model {}", provider.as_str(), model);
                current_provider = provider;
                current_model = model;
                provider_config = config;
                outbound_headers = headers;
                attempt = 0;
                used_fallback = true;
                continue;
            }
            if fallback_config.is_some() && !used_fallback {
                last_status = StatusCode::SERVICE_UNAVAILABLE;
                last_error_msg.push_str("; no usable fallback provider with a configured API key");
            }

            // Record failed request metric
            app_state.metrics.record_request(
                original_provider.as_str(),
                &original_model,
                "/v1/chat/completions",
                last_status.as_u16(),
                start_time.elapsed(),
            );

            // Out of retries and fallbacks
            return (last_status, last_error_msg).into_response();
        }
    }
}

/// Handler for POST /v1/messages
pub async fn handle_messages(
    State(app_state): State<Arc<AppState>>,
    auth: ClientAuth,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let start_time = Instant::now();

    // For /v1/messages, we expect the Anthropic model format.
    // If the model is prefixed (e.g. "anthropic/claude-3-5-sonnet"), we parse it.
    let helper: ChatCompletionRequestHelper = match serde_json::from_slice(&body) {
        Ok(h) => h,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Invalid JSON request body: {}", e),
            )
                .into_response();
        }
    };

    let full_model = helper.model;

    let (current_provider, current_model) = match extract_provider_and_model(&full_model) {
        Some(res) => res,
        None => {
            // Fallback to Anthropic if no prefix but hit /v1/messages
            (ProviderKind::Anthropic, full_model)
        }
    };

    let provider_config = match app_state.config.providers.get(current_provider.as_str()) {
        Some(cfg) => cfg,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Provider {} not configured", current_provider.as_str()),
            )
                .into_response();
        }
    };

    let api_key = &auth.0;
    let tenant = headers
        .get("x-ferrum-tenant")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("default-tenant");
    let provider_str = current_provider.as_str();

    let key_limit = format!("key:{}", api_key);
    let tenant_limit = format!("tenant:{}", tenant);
    let provider_limit = format!("provider:{}", provider_str);

    if !app_state.rate_limiter.check_limit(&key_limit)
        || !app_state.rate_limiter.check_limit(&tenant_limit)
        || !app_state.rate_limiter.check_limit(&provider_limit)
    {
        app_state.metrics.record_request(
            provider_str,
            &current_model,
            "/v1/messages",
            429,
            Duration::from_secs(0),
        );
        return (StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded").into_response();
    }

    let target_url = format!("{}/messages", provider_config.base_url);
    let Some(mut outbound_headers) = current_provider.upstream_headers(provider_config) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "No usable API key configured for provider {}",
                current_provider.as_str()
            ),
        )
            .into_response();
    };

    // Forward any extra headers
    if let Some(version) = headers.get("anthropic-version") {
        outbound_headers.insert("anthropic-version", version.clone());
    }

    let mut body_val: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("Invalid JSON request body: {}", e)).into_response();
        }
    };
    body_val["model"] = serde_json::Value::String(current_model.clone());
    let target_body = serde_json::to_vec(&body_val).unwrap();

    let req_builder = app_state
        .client
        .post(&target_url)
        .headers(outbound_headers)
        .body(target_body)
        .timeout(Duration::from_secs(provider_config.timeout_secs));

    match req_builder.send().await {
        Ok(response) => {
            let status = response.status();
            app_state.metrics.record_request(
                current_provider.as_str(),
                &current_model,
                "/v1/messages",
                status.as_u16(),
                start_time.elapsed(),
            );

            let mut res_builder = Response::builder().status(status);
            for (key, val) in response.headers().iter() {
                let k = key.as_str().to_lowercase();
                if k != "content-encoding"
                    && k != "content-length"
                    && k != "transfer-encoding"
                    && k != "connection"
                {
                    res_builder = res_builder.header(key, val);
                }
            }

            let res_stream = response.bytes_stream().map(|chunk_result| {
                chunk_result.map_err(std::io::Error::other)
            });

            res_builder.body(Body::from_stream(res_stream)).unwrap()
        }
        Err(e) => {
            error!("Connection to provider {} failed: {}", current_provider.as_str(), e);
            app_state.metrics.record_request(
                current_provider.as_str(),
                &current_model,
                "/v1/messages",
                502,
                start_time.elapsed(),
            );
            (StatusCode::BAD_GATEWAY, format!("Failed to reach upstream provider: {}", e)).into_response()
        }
    }
}
