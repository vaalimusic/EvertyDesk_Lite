use std::time::Duration;

use serde_json::Value;

use crate::{normalize_remote_id, settings::ContactEntry};

pub(crate) fn login(
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

pub(crate) fn personal_ab_guid(api_url: &str, token: &str) -> Result<String, String> {
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

pub(crate) fn peers(api_url: &str, token: &str, guid: &str) -> Result<Vec<ContactEntry>, String> {
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

pub(crate) fn add_peer(
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
            "tags": [],
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

pub(crate) fn update_peer(
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
            "hostname": contact.note,
            "platform": contact.os,
            "tags": [],
        }),
        &[],
    )?;
    check_json_error(&json)
}

pub(crate) fn delete_peer(
    api_url: &str,
    token: &str,
    guid: &str,
    remote_id: &str,
) -> Result<(), String> {
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

pub(crate) fn platform() -> &'static str {
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

fn response_json(result: Result<ureq::Response, ureq::Error>) -> Result<Value, String> {
    match result {
        Ok(response) => response
            .into_string()
            .map_err(|err| format!("API response error: {err}"))
            .and_then(|text| {
                if text.trim().is_empty() {
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
