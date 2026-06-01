use std::time::Duration;

use serde_json::{json, Value};

use crate::settings::{LlmConfig, LlmProvider};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_TERMINAL_CONTEXT_CHARS: usize = 14_000;

pub fn terminal_suggestion(
    config: LlmConfig,
    transcript: String,
    goal: String,
) -> Result<String, String> {
    if !config.enabled {
        return Err("AI терминал выключен в настройках".to_owned());
    }

    let system = if config.system_prompt.trim().is_empty() {
        LlmConfig::default().system_prompt
    } else {
        config.system_prompt.trim().to_owned()
    };
    let user = terminal_prompt(&transcript, &goal);

    let answer = match config.provider {
        LlmProvider::OpenAi => call_openai(&config, &system, &user),
        LlmProvider::YandexGpt => call_yandex_gpt(&config, &system, &user),
        LlmProvider::Ollama => call_ollama(&config, &system, &user),
    }?;

    let trimmed = answer.trim();
    if trimmed.is_empty() {
        Err("LLM вернула пустой ответ".to_owned())
    } else {
        Ok(trimmed.to_owned())
    }
}

pub fn provider_status(config: &LlmConfig) -> &'static str {
    if !config.enabled {
        return "выключен";
    }
    match config.provider {
        LlmProvider::OpenAi if config.openai_api_key.trim().is_empty() => "нужен OpenAI API key",
        LlmProvider::OpenAi => "готов",
        LlmProvider::YandexGpt if config.yandex_api_key.trim().is_empty() => "нужен Yandex API key",
        LlmProvider::YandexGpt => "готов",
        LlmProvider::Ollama if config.ollama_model.trim().is_empty() => "нужна модель Ollama",
        LlmProvider::Ollama => "готов",
    }
}

fn call_openai(config: &LlmConfig, system: &str, user: &str) -> Result<String, String> {
    let api_key = required(&config.openai_api_key, "OpenAI API key")?;
    let model = required(&config.openai_model, "OpenAI model")?;
    let url = required(&config.openai_base_url, "OpenAI endpoint")?;
    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "temperature": safe_temperature(config.temperature),
        "max_tokens": safe_max_tokens(config.max_tokens)
    });

    let value = post_json(
        ureq::post(url)
            .timeout(REQUEST_TIMEOUT)
            .set("Authorization", &format!("Bearer {api_key}"))
            .set("Content-Type", "application/json"),
        body,
    )?;
    value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "OpenAI: не найден choices[0].message.content".to_owned())
}

fn call_yandex_gpt(config: &LlmConfig, system: &str, user: &str) -> Result<String, String> {
    let api_key = required(&config.yandex_api_key, "Yandex API key или IAM token")?;
    let url = required(&config.yandex_base_url, "YandexGPT endpoint")?;
    let model_uri = yandex_model_uri(config)?;
    let body = json!({
        "modelUri": model_uri,
        "completionOptions": {
            "stream": false,
            "temperature": safe_temperature(config.temperature),
            "maxTokens": safe_max_tokens(config.max_tokens).to_string()
        },
        "messages": [
            { "role": "system", "text": system },
            { "role": "user", "text": user }
        ]
    });

    let auth = yandex_auth_header(api_key);
    let value = post_json(
        ureq::post(url)
            .timeout(REQUEST_TIMEOUT)
            .set("Authorization", &auth)
            .set("Content-Type", "application/json"),
        body,
    )?;
    value
        .pointer("/result/alternatives/0/message/text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "YandexGPT: не найден result.alternatives[0].message.text".to_owned())
}

fn call_ollama(config: &LlmConfig, system: &str, user: &str) -> Result<String, String> {
    let model = required(&config.ollama_model, "Ollama model")?;
    let base_url = required(&config.ollama_base_url, "Ollama base URL")?;
    let url = format!("{}/api/chat", base_url.trim_end_matches('/'));
    let body = json!({
        "model": model,
        "stream": false,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "options": {
            "temperature": safe_temperature(config.temperature),
            "num_predict": safe_max_tokens(config.max_tokens)
        }
    });

    let value = post_json(
        ureq::post(&url)
            .timeout(REQUEST_TIMEOUT)
            .set("Content-Type", "application/json"),
        body,
    )?;
    value
        .pointer("/message/content")
        .or_else(|| value.pointer("/response"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "Ollama: не найден message.content".to_owned())
}

fn post_json(request: ureq::Request, body: Value) -> Result<Value, String> {
    match request.send_json(body) {
        Ok(response) => response
            .into_json::<Value>()
            .map_err(|err| format!("Не удалось прочитать JSON ответ: {err}")),
        Err(ureq::Error::Status(code, response)) => {
            let text = response.into_string().unwrap_or_default();
            Err(format!("HTTP {code}: {}", compact_error(&text)))
        }
        Err(ureq::Error::Transport(err)) => Err(format!("Сеть: {err}")),
    }
}

fn terminal_prompt(transcript: &str, goal: &str) -> String {
    let goal = goal.trim();
    let goal = if goal.is_empty() {
        "Проанализируй последний вывод терминала. Если есть ошибка, предложи следующую безопасную команду. Если все нормально, кратко объясни, что делать дальше."
    } else {
        goal
    };
    let transcript = tail_text(transcript, MAX_TERMINAL_CONTEXT_CHARS);
    format!(
        "Цель оператора:\n{goal}\n\nПоследний вывод удаленного терминала:\n```text\n{transcript}\n```\n\nОтветь структурно: диагноз, команда для вставки, что проверить после выполнения."
    )
}

fn tail_text(text: &str, max_chars: usize) -> String {
    let chars = text.chars().count();
    if chars <= max_chars {
        return text.to_owned();
    }
    text.chars().skip(chars - max_chars).collect()
}

fn required<'a>(value: &'a str, label: &str) -> Result<&'a str, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(format!("{label} не заполнен"))
    } else {
        Ok(trimmed)
    }
}

fn yandex_model_uri(config: &LlmConfig) -> Result<String, String> {
    let template = required(&config.yandex_model_uri, "YandexGPT model URI")?;
    let folder_id = config.yandex_folder_id.trim();
    if template.contains("{folder_id}") {
        if folder_id.is_empty() {
            return Err("Yandex folder_id не заполнен".to_owned());
        }
        Ok(template.replace("{folder_id}", folder_id))
    } else {
        Ok(template.to_owned())
    }
}

fn yandex_auth_header(api_key: &str) -> String {
    if api_key.starts_with("Api-Key ") || api_key.starts_with("Bearer ") {
        api_key.to_owned()
    } else {
        format!("Api-Key {api_key}")
    }
}

fn safe_max_tokens(value: u32) -> u32 {
    value.clamp(128, 4_096)
}

fn safe_temperature(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 2.0)
    } else {
        0.2
    }
}

fn compact_error(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "пустой ответ".to_owned();
    }
    let compact = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= 500 {
        compact
    } else {
        format!("{}...", compact.chars().take(500).collect::<String>())
    }
}
