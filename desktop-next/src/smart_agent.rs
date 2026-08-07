use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct HeartbeatRequest {
    pub machine_id: String,
    pub service_key: String,
    pub hostname: String,
    pub os: String,
    pub os_version: String,
    pub rustdesk_id: String,
}

// Manual `Debug` (not derived) so a stray `{:?}` can never print the
// organization's `service_key` secret — same rationale as `ViewerBootstrap`.
impl std::fmt::Debug for HeartbeatRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeartbeatRequest")
            .field("machine_id", &self.machine_id)
            .field("service_key", &"<redacted>")
            .field("hostname", &self.hostname)
            .field("os", &self.os)
            .field("os_version", &self.os_version)
            .field("rustdesk_id", &self.rustdesk_id)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentNotification {
    pub id: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub link_label: String,
    #[serde(default)]
    pub image_url: String,
    #[serde(default)]
    pub severity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentOperator {
    pub machine_id: String,
    #[serde(default)]
    pub rustdesk_id: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub last_seen: String,
    #[serde(default)]
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConfigUpdate {
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub api_server: String,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct SupportRequest {
    pub machine_id: String,
    pub service_key: String,
    pub hostname: String,
    pub message: String,
    pub target_machine_id: String,
    pub target_rustdesk_id: String,
    pub from_rustdesk_id: String,
}

// Manual `Debug` (not derived) — see `HeartbeatRequest`'s impl above.
impl std::fmt::Debug for SupportRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SupportRequest")
            .field("machine_id", &self.machine_id)
            .field("service_key", &"<redacted>")
            .field("hostname", &self.hostname)
            .field("message", &self.message)
            .field("target_machine_id", &self.target_machine_id)
            .field("target_rustdesk_id", &self.target_rustdesk_id)
            .field("from_rustdesk_id", &self.from_rustdesk_id)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportAction {
    Accept,
    Defer10,
    Defer60,
    Decline,
}

impl SupportAction {
    pub const fn as_api_value(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Defer10 => "defer10",
            Self::Defer60 => "defer60",
            Self::Decline => "decline",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportOptions {
    pub request_id: Option<u64>,
    pub from_rustdesk_id: String,
    pub actions: Vec<SupportAction>,
}

pub fn heartbeat(api_url: &str, request: &HeartbeatRequest) -> Result<(), String> {
    let response = agent()
        .post(&url(api_url, "/admin/agent/heartbeat"))
        .send_json(request);
    response_value(response).and_then(check_ok)
}

pub fn inbox(
    api_url: &str,
    machine_id: &str,
    service_key: &str,
) -> Result<Vec<AgentNotification>, String> {
    let response = agent()
        .get(&url(api_url, "/admin/agent/inbox"))
        .query("machine_id", machine_id)
        .query("service_key", service_key)
        .call();
    let value = response_value(response)?;
    serde_json::from_value(
        value
            .get("items")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .map_err(|error| format!("Smart Agent inbox JSON: {error}"))
}

pub fn acknowledge(api_url: &str, machine_id: &str, notification_id: u64) -> Result<(), String> {
    let path = format!("/admin/agent/notification/{notification_id}/ack");
    let response = agent()
        .post(&url(api_url, &path))
        .query("machine_id", machine_id)
        .send_bytes(&[]);
    response_value(response).and_then(check_ok)
}

pub fn vote(
    api_url: &str,
    machine_id: &str,
    notification_id: u64,
    vote: &str,
) -> Result<(), String> {
    let path = format!("/admin/agent/notification/{notification_id}/vote");
    let response = agent()
        .post(&url(api_url, &path))
        .query("machine_id", machine_id)
        .send_json(serde_json::json!({ "vote": vote }));
    response_value(response).and_then(check_ok)
}

pub fn operators(api_url: &str, service_key: &str) -> Result<Vec<AgentOperator>, String> {
    let response = agent()
        .get(&url(api_url, "/admin/agent/operators"))
        .query("service_key", service_key)
        .call();
    let value = response_value(response)?;
    serde_json::from_value(
        value
            .get("items")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .map_err(|error| format!("Smart Agent operators JSON: {error}"))
}

pub fn request_support(api_url: &str, request: &SupportRequest) -> Result<u64, String> {
    let response = agent()
        .post(&url(api_url, "/admin/agent/support-request"))
        .send_json(request);
    let value = response_value(response)?;
    check_ok(value.clone())?;
    value
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Smart Agent не вернул ID запроса поддержки".to_owned())
}

pub fn respond_to_support(
    api_url: &str,
    machine_id: &str,
    service_key: &str,
    request_id: u64,
    action: SupportAction,
    message: &str,
) -> Result<(), String> {
    let response = agent()
        .post(&url(api_url, "/admin/agent/support-request/respond"))
        .send_json(serde_json::json!({
            "machine_id": machine_id,
            "service_key": service_key,
            "request_id": request_id,
            "action": action.as_api_value(),
            "message": message,
        }));
    response_value(response).and_then(check_ok)
}

pub fn parse_support_options(options: &[String]) -> SupportOptions {
    let mut parsed = SupportOptions {
        request_id: None,
        from_rustdesk_id: String::new(),
        actions: Vec::new(),
    };
    for option in options {
        if let Some(remote_id) = option.strip_prefix("meta:from_rdid=") {
            parsed.from_rustdesk_id = remote_id.trim().to_owned();
            continue;
        }
        let Some((action, reference)) = option.split_once(':') else {
            continue;
        };
        let Some(request_id) = reference
            .strip_prefix("req-")
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        let action = match action {
            "accept" => SupportAction::Accept,
            "defer10" => SupportAction::Defer10,
            "defer60" => SupportAction::Defer60,
            "decline" => SupportAction::Decline,
            _ => continue,
        };
        parsed.request_id.get_or_insert(request_id);
        if parsed.request_id == Some(request_id) && !parsed.actions.contains(&action) {
            parsed.actions.push(action);
        }
    }
    parsed
}

pub fn parse_config_update(body: &str) -> Option<ConfigUpdate> {
    let update: ConfigUpdate = serde_json::from_str(body).ok()?;
    if update.server.trim().is_empty()
        && update.key.trim().is_empty()
        && update.api_server.trim().is_empty()
    {
        return None;
    }
    Some(update)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build()
}

fn url(api_url: &str, path: &str) -> String {
    format!("{}{}", api_url.trim_end_matches('/'), path)
}

fn response_value(result: Result<ureq::Response, ureq::Error>) -> Result<Value, String> {
    let response = match result {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let message = response.into_string().unwrap_or_default();
            return Err(format!("Smart Agent HTTP {code}: {message}"));
        }
        Err(error) => return Err(format!("Smart Agent request failed: {error}")),
    };
    let length = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    if length > MAX_RESPONSE_BYTES {
        return Err("Smart Agent response exceeds 1 MiB".to_owned());
    }
    let mut reader = response.into_reader().take(MAX_RESPONSE_BYTES + 1);
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Smart Agent response error: {error}"))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("Smart Agent response exceeds 1 MiB".to_owned());
    }
    if bytes.is_empty() {
        return Ok(serde_json::json!({ "ok": true }));
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("Smart Agent JSON: {error}"))
}

fn check_ok(value: Value) -> Result<(), String> {
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty())
            .unwrap_or(error);
        return Err(message.to_owned());
    }
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err("Smart Agent отклонил запрос".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inbox_contract() {
        let value = serde_json::json!({
            "id": 42,
            "title": "Плановые работы",
            "body": "Текст",
            "type": "banner",
            "severity": "warning"
        });
        let notification: AgentNotification = serde_json::from_value(value).unwrap();
        assert_eq!(notification.id, 42);
        assert_eq!(notification.kind, "banner");
        assert!(notification.options.is_empty());
    }

    #[test]
    fn parses_support_actions_and_metadata() {
        let options = [
            "accept:req-15",
            "defer10:req-15",
            "decline:req-15",
            "meta:from_rdid=123 456 789",
        ]
        .map(str::to_owned);
        let parsed = parse_support_options(&options);
        assert_eq!(parsed.request_id, Some(15));
        assert_eq!(parsed.from_rustdesk_id, "123 456 789");
        assert_eq!(
            parsed.actions,
            vec![
                SupportAction::Accept,
                SupportAction::Defer10,
                SupportAction::Decline
            ]
        );
    }

    #[test]
    fn parses_config_update_body() {
        let parsed = parse_config_update(
            r#"{"server":"relay.everty.ru","key":"public-key","api_server":"https://desk.everty.ru"}"#,
        )
        .unwrap();
        assert_eq!(parsed.server, "relay.everty.ru");
        assert_eq!(parsed.key, "public-key");
        assert_eq!(parsed.api_server, "https://desk.everty.ru");
        assert!(parse_config_update("{}").is_none());
        assert!(parse_config_update("not json").is_none());
    }

    #[test]
    fn joins_agent_base_url_without_double_slash() {
        assert_eq!(
            url("https://desk.everty.ru/", "/admin/agent/inbox"),
            "https://desk.everty.ru/admin/agent/inbox"
        );
    }
}
