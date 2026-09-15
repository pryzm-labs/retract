use serde_json::Value;
use std::collections::HashMap;
use zeroize::Zeroizing;

use crate::error::AppError;
use crate::providers::discord::session::valid_user_token;

const MAX_TRACKED_REQUESTS: usize = 4096;

pub(crate) struct CapturedAuthorization(Zeroizing<String>);

impl CapturedAuthorization {
    pub(crate) fn expose<T>(&self, operation: impl FnOnce(&str) -> T) -> T {
        operation(&self.0)
    }

    pub(crate) fn into_inner(self) -> Zeroizing<String> {
        self.0
    }
}

#[derive(Default)]
pub(crate) struct ProtocolParser {
    cdp_discord_requests: HashMap<String, ()>,
}

impl ProtocolParser {
    pub(crate) fn parse_cdp(
        &mut self,
        event: &str,
    ) -> Result<Option<CapturedAuthorization>, AppError> {
        let value: Value = serde_json::from_str(event).map_err(|_| malformed_event())?;
        match value.get("method").and_then(Value::as_str) {
            Some("Network.requestWillBeSent") => {
                let params = value.get("params").ok_or_else(malformed_event)?;
                let request_id = params
                    .get("requestId")
                    .and_then(Value::as_str)
                    .ok_or_else(malformed_event)?;
                let request = params.get("request").ok_or_else(malformed_event)?;
                let url = request
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(malformed_event)?;
                if !is_discord_api_url(url) {
                    return Ok(None);
                }
                if self.cdp_discord_requests.len() >= MAX_TRACKED_REQUESTS {
                    self.cdp_discord_requests.clear();
                }
                self.cdp_discord_requests.insert(request_id.to_owned(), ());
                Ok(extract_object_authorization(request.get("headers")))
            }
            Some("Network.requestWillBeSentExtraInfo") => {
                let params = value.get("params").ok_or_else(malformed_event)?;
                let request_id = params
                    .get("requestId")
                    .and_then(Value::as_str)
                    .ok_or_else(malformed_event)?;
                if !self.cdp_discord_requests.contains_key(request_id) {
                    return Ok(None);
                }
                let captured = extract_object_authorization(params.get("headers"));
                if captured.is_some() {
                    self.cdp_discord_requests.remove(request_id);
                }
                Ok(captured)
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn parse_bidi(
        &mut self,
        event: &str,
    ) -> Result<Option<CapturedAuthorization>, AppError> {
        let value: Value = serde_json::from_str(event).map_err(|_| malformed_event())?;
        if value.get("method").and_then(Value::as_str) != Some("network.beforeRequestSent") {
            return Ok(None);
        }
        let request = value
            .get("params")
            .and_then(|params| params.get("request"))
            .ok_or_else(malformed_event)?;
        let url = request
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(malformed_event)?;
        if !is_discord_api_url(url) {
            return Ok(None);
        }
        let Some(headers) = request.get("headers").and_then(Value::as_array) else {
            return Ok(None);
        };
        for header in headers {
            if !header
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case("authorization"))
            {
                continue;
            }
            let raw = header.get("value");
            let token = raw.and_then(Value::as_str).or_else(|| {
                raw.and_then(|value| value.get("value"))
                    .and_then(Value::as_str)
            });
            if let Some(token) = token.filter(|token| valid_user_token(token)) {
                return Ok(Some(CapturedAuthorization(Zeroizing::new(
                    token.to_owned(),
                ))));
            }
        }
        Ok(None)
    }
}

fn extract_object_authorization(headers: Option<&Value>) -> Option<CapturedAuthorization> {
    let headers = headers?.as_object()?;
    headers.iter().find_map(|(name, value)| {
        if !name.eq_ignore_ascii_case("authorization") {
            return None;
        }
        value
            .as_str()
            .filter(|token| valid_user_token(token))
            .map(|token| CapturedAuthorization(Zeroizing::new(token.to_owned())))
    })
}

fn is_discord_api_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str() == Some("discord.com")
        && (url.path() == "/api" || url.path().starts_with("/api/"))
}

fn malformed_event() -> AppError {
    AppError::InvalidRequest("Browser authentication event was malformed".into())
}
