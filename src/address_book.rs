use std::time::Duration;

use serde_json::Value;

use crate::settings::{ContactEntry, ServerConfig};

const OIDC_PENDING_ERROR: &str = "No authed oidc is found";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcAuthStart {
    pub code: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OidcAuthQuery {
    Pending,
    Authorized { access_token: String, account: String },
}

pub fn login(
    api_url: &str,
    username: &str,
    password: &str,
    rustdesk_id: &str,
    uuid: &str,
) -> Result<String, String> {
    let json = api_public_send(
        "POST",
        api_url,
        "/api/login",
        serde_json::json!({
            "username": username,
            "password": password,
            "id": normalize_remote_id(rustdesk_id),
            "uuid": uuid,
            "autoLogin": true,
            "type": "account",
            "deviceInfo": {
                "os": platform(),
                "type": "PC",
                "name": local_hostname(),
            }
        }),
    )?;
    check_json_error(&json)?;
    extract_string_field(&json, "access_token")
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| {
            extract_string_field(&json, "type")
                .map(|kind| format!("Login requires extra step: {kind}"))
                .unwrap_or_else(|| "API did not return access_token".to_owned())
        })
}

pub fn login_options(api_url: &str) -> Result<Vec<String>, String> {
    let json = api_public_get(api_url, "/api/login-options", &[])?;
    json.as_array()
        .ok_or_else(|| "API did not return login options list".to_owned())
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
}

pub fn oidc_auth(
    api_url: &str,
    provider: &str,
    rustdesk_id: &str,
    uuid: &str,
) -> Result<OidcAuthStart, String> {
    let json = api_public_send(
        "POST",
        api_url,
        "/api/oidc/auth",
        serde_json::json!({
            "op": provider,
            "id": normalize_remote_id(rustdesk_id),
            "uuid": uuid,
            "deviceInfo": {
                "os": platform(),
                "type": "PC",
                "name": local_hostname(),
            }
        }),
    )?;
    check_json_error(&json)?;
    let code = extract_string_field(&json, "code")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "OIDC did not return code".to_owned())?;
    let url = extract_string_field(&json, "url")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "OIDC did not return browser URL".to_owned())?;
    Ok(OidcAuthStart { code, url })
}

pub fn oidc_auth_query(api_url: &str, code: &str) -> Result<OidcAuthQuery, String> {
    let json = match api_public_get(api_url, "/api/oidc/auth-query", &[("code", code)]) {
        Ok(json) => json,
        Err(error) if error.contains(OIDC_PENDING_ERROR) => return Ok(OidcAuthQuery::Pending),
        Err(error) => return Err(error),
    };
    if extract_string_field(&json, "error").as_deref() == Some(OIDC_PENDING_ERROR) {
        return Ok(OidcAuthQuery::Pending);
    }
    check_json_error(&json)?;
    let access_token = extract_string_field(&json, "access_token")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "OIDC did not return access_token".to_owned())?;
    let account = extract_string_field(json.get("user").unwrap_or(&json), "email")
        .or_else(|| extract_string_field(json.get("user").unwrap_or(&json), "display_name"))
        .or_else(|| extract_string_field(json.get("user").unwrap_or(&json), "name"))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "yandex".to_owned());
    Ok(OidcAuthQuery::Authorized {
        access_token,
        account,
    })
}

pub fn personal_ab_guid(api_url: &str, token: &str) -> Result<String, String> {
    let json = api_send(
        "POST",
        api_url,
        token,
        "/api/ab/personal",
        serde_json::json!({}),
        &[],
    )?;
    extract_string_field(&json, "guid")
        .or_else(|| extract_string_field(&json, "id"))
        .or_else(|| json.as_str().map(ToOwned::to_owned))
        .filter(|guid| !guid.trim().is_empty())
        .ok_or_else(|| "API did not return address book GUID".to_owned())
}

pub fn current_user(api_url: &str, token: &str) -> Result<Value, String> {
    let json = api_send(
        "POST",
        api_url,
        token,
        "/api/currentUser",
        serde_json::json!({}),
        &[],
    )?;
    check_json_error(&json)?;
    Ok(json)
}

pub fn public_connection(api_url: &str, token: Option<&str>) -> Result<ServerConfig, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .build();
    let url = api_url_join(api_url, "/public/connection");
    let mut request = agent.get(&url);
    if let Some(token) = token.map(str::trim).filter(|token| !token.is_empty()) {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    let json = response_json(request.call())?;
    check_json_error(&json)?;
    parse_public_connection(api_url, &json)
}

pub fn peers(api_url: &str, token: &str, guid: &str) -> Result<Vec<ContactEntry>, String> {
    let mut contacts = Vec::new();
    let mut current = 1usize;
    loop {
        let current_s = current.to_string();
        let json = api_send(
            "POST",
            api_url,
            token,
            "/api/ab/peers",
            serde_json::json!({}),
            &[("ab", guid), ("pageSize", "30"), ("current", &current_s)],
        )?;
        check_json_error(&json)?;
        let data = json
            .get("data")
            .and_then(Value::as_array)
            .or_else(|| json.as_array())
            .ok_or_else(|| "API did not return peers list".to_owned())?;
        for peer in data {
            if let Some(remote_id) = extract_string_field(peer, "id") {
                contacts.push(ContactEntry {
                    name: extract_string_field(peer, "alias").unwrap_or_default(),
                    remote_id: normalize_remote_id(&remote_id),
                    note: extract_string_field(peer, "hostname").unwrap_or_default(),
                    machine_id: String::new(),
                    os: extract_string_field(peer, "platform").unwrap_or_default(),
                    last_seen: String::new(),
                    online: false,
                    tags: extract_string_array_field(peer, "tags"),
                });
            }
        }
        let total = json
            .get("total")
            .and_then(Value::as_u64)
            .unwrap_or(contacts.len() as u64);
        if data.len() < 30 || (current as u64) * 30 >= total {
            break;
        }
        current += 1;
    }
    Ok(contacts)
}

pub fn add_peer(
    api_url: &str,
    token: &str,
    guid: &str,
    contact: &ContactEntry,
) -> Result<(), String> {
    let json = api_send(
        "POST",
        api_url,
        token,
        &format!("/api/ab/peer/add/{guid}"),
        serde_json::json!({
            "id": contact.remote_id,
            "alias": contact.name,
            "username": "",
            "hostname": contact.note,
            "platform": contact.os,
            "tags": contact.tags,
            "forceAlwaysRelay": "false",
            "rdpPort": "",
            "rdpUsername": "",
            "loginName": "",
            "same_server": "",
        }),
        &[],
    )?;
    check_json_error(&json)
}

pub fn update_peer(
    api_url: &str,
    token: &str,
    guid: &str,
    contact: &ContactEntry,
) -> Result<(), String> {
    let json = api_send(
        "PUT",
        api_url,
        token,
        &format!("/api/ab/peer/update/{guid}"),
        serde_json::json!({
            "id": contact.remote_id,
            "alias": contact.name,
            "tags": contact.tags,
        }),
        &[],
    )?;
    check_json_error(&json)
}

pub fn delete_peer(api_url: &str, token: &str, guid: &str, remote_id: &str) -> Result<(), String> {
    let json = api_send(
        "DELETE",
        api_url,
        token,
        &format!("/api/ab/peer/{guid}"),
        serde_json::json!([remote_id]),
        &[],
    )?;
    check_json_error(&json)
}

pub fn logout(api_url: &str, token: &str, rustdesk_id: &str) -> Result<(), String> {
    let json = api_send(
        "POST",
        api_url,
        token,
        "/api/logout",
        serde_json::json!({ "id": normalize_remote_id(rustdesk_id) }),
        &[],
    )?;
    check_json_error(&json)
}

pub fn platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "linux" => "Linux",
        "macos" => "Mac",
        _ => std::env::consts::OS,
    }
}

fn api_send(
    method: &str,
    api_url: &str,
    token: &str,
    path: &str,
    body: Value,
    query: &[(&str, &str)],
) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .build();
    let url = api_url_join(api_url, path);
    let mut req = agent
        .request(method, &url)
        .set("Authorization", &format!("Bearer {token}"));
    for (key, value) in query {
        req = req.query(key, value);
    }
    response_json(req.send_json(body))
}

fn api_public_send(method: &str, api_url: &str, path: &str, body: Value) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .build();
    let url = api_url_join(api_url, path);
    response_json(agent.request(method, &url).send_json(body))
}

fn api_public_get(api_url: &str, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .build();
    let url = api_url_join(api_url, path);
    let mut req = agent.get(&url);
    for (key, value) in query {
        req = req.query(key, value);
    }
    response_json(req.call())
}

fn response_json(result: Result<ureq::Response, ureq::Error>) -> Result<Value, String> {
    match result {
        Ok(response) => response
            .into_string()
            .map_err(|err| format!("API response error: {err}"))
            .and_then(|text| {
                if text.trim().is_empty() || text.trim().eq_ignore_ascii_case("ok") {
                    Ok(serde_json::json!({ "ok": true }))
                } else {
                    serde_json::from_str::<Value>(&text)
                        .map_err(|err| format!("API JSON error: {err}"))
                }
            }),
        Err(ureq::Error::Status(code, response)) => {
            let text = response.into_string().unwrap_or_default();
            Err(format!("API HTTP {code}: {text}"))
        }
        Err(err) => Err(format!("API request failed: {err}")),
    }
}

fn api_url_join(api_url: &str, path: &str) -> String {
    format!("{}{}", api_url.trim_end_matches('/'), path)
}

fn parse_public_connection(api_url: &str, json: &Value) -> Result<ServerConfig, String> {
    let id_server = extract_string_field(json, "id_server")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "API did not return id_server".to_owned())?;
    let relay_server = extract_string_field(json, "relay_server")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "API did not return relay_server".to_owned())?;
    Ok(ServerConfig {
        api_url: extract_string_field(json, "api_server")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| api_url.trim_end_matches('/').to_owned()),
        id_server,
        relay_server,
        public_key: extract_string_field(json, "public_key").unwrap_or_default(),
    })
}

fn normalize_remote_id(id: &str) -> String {
    id.chars()
        .filter(|character| !character.is_whitespace() && *character != '-' && *character != '_')
        .collect()
}

fn local_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "EvertyDesk".to_owned())
}

fn check_json_error(json: &Value) -> Result<(), String> {
    if let Some(error) = extract_string_field(json, "error") {
        if !error.trim().is_empty() {
            return Err(error);
        }
    }
    if let Some(message) = extract_string_field(json, "message") {
        if !message.trim().is_empty() && message != "ok" {
            return Err(message);
        }
    }
    Ok(())
}

fn extract_string_field(json: &Value, field: &str) -> Option<String> {
    json.get(field)
        .and_then(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .or_else(|| value.as_i64().map(|number| number.to_string()))
                .or_else(|| value.as_u64().map(|number| number.to_string()))
        })
        .or_else(|| {
            json.get("data")
                .and_then(|data| extract_string_field(data, field))
        })
}

fn extract_string_array_field(json: &Value, field: &str) -> Vec<String> {
    json.get(field)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| {
                    value
                        .as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_url_join_avoids_double_separator() {
        assert_eq!(
            api_url_join("https://desk.everty.ru/", "/api/login"),
            "https://desk.everty.ru/api/login"
        );
    }

    #[test]
    fn remote_id_is_normalized_for_cloud_requests() {
        assert_eq!(normalize_remote_id(" 123-456_789 "), "123456789");
    }

    #[test]
    fn parses_public_connection_with_optional_public_key() {
        let parsed = parse_public_connection(
            "https://desk.everty.ru/",
            &serde_json::json!({
                "data": {
                    "id_server": "hbbs.example.com",
                    "relay_server": "hbbr.example.com",
                    "public_key": ""
                }
            }),
        )
        .unwrap();
        assert_eq!(parsed.api_url, "https://desk.everty.ru");
        assert_eq!(parsed.id_server, "hbbs.example.com");
        assert_eq!(parsed.relay_server, "hbbr.example.com");
        assert!(parsed.public_key.is_empty());
    }

    #[test]
    fn oidc_query_parses_pending_and_success_shapes() {
        let pending = serde_json::json!({ "error": OIDC_PENDING_ERROR });
        assert!(matches!(
            if extract_string_field(&pending, "error").as_deref() == Some(OIDC_PENDING_ERROR) {
                OidcAuthQuery::Pending
            } else {
                panic!("expected pending")
            },
            OidcAuthQuery::Pending
        ));

        let success = serde_json::json!({
            "access_token": "token",
            "user": {
                "email": "user@example.com",
                "display_name": "User"
            }
        });
        assert_eq!(
            extract_string_field(&success, "access_token").as_deref(),
            Some("token")
        );
        assert_eq!(
            extract_string_field(success.get("user").unwrap(), "email").as_deref(),
            Some("user@example.com")
        );
    }
}
