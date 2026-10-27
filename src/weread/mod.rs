//! E-ink client protocol, ported from weread-omni (see THIRD_PARTY_NOTICES.md).
mod covers;
pub use covers::{cached_cover, Cover};
#[cfg(test)]
pub(crate) mod tests;
pub use crate::storage::Store;

use rand::Rng;
use reqwest::{header::HeaderMap, Client, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug)]
pub struct Error {
    pub message: String,
    pub auth: bool,
}
impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            auth: false,
        }
    }
    fn auth(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            auth: true,
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}

// Deliberately no Debug: credentials must never appear in diagnostics.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    pub vid: String,
    pub access_token: String,
    pub refresh_token: String,
    pub device_id: String,
}
impl Credentials {
    pub fn validate(&self) -> Result<()> {
        if [
            &self.vid,
            &self.access_token,
            &self.refresh_token,
            &self.device_id,
        ]
        .iter()
        .any(|s| s.is_empty())
        {
            return Err(Error::new(
                "Saved session is incomplete. Please sign in again.",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Book {
    pub id: String,
    pub title: String,
    pub author: String,
    pub cover: String,
    pub finished: bool,
}
#[derive(Debug, PartialEq)]
pub enum Poll {
    Scanned,
    Waiting,
    Confirmed(String),
    Expired,
    Declined,
}

pub struct Api {
    http: Client,
    api: String,
    wx: String,
    poll: String,
}
impl Api {
    #[cfg(test)]
    pub fn test_endpoint(base: &str) -> Self {
        let mut api = Self::new().unwrap();
        api.api = base.into();
        api.wx = base.into();
        api.poll = base.into();
        api
    }
    pub fn new() -> Result<Self> {
        let mut headers = HeaderMap::new();
        for (key, value) in [
            ("baseapi", "30"),
            ("appver", "2.1.2.10245900"),
            ("basever", "2.1.2.10245900"),
            ("osver", "11"),
            ("channelid", "900"),
        ] {
            headers.insert(
                reqwest::header::HeaderName::from_static(key),
                value.parse().unwrap(),
            );
        }
        let http = Client::builder().default_headers(headers)
            .user_agent("WeRead/2.1.2 WRBrand/Onyx wr_eink Dalvik/2.1.0 (Linux; U; Android 11; BOOX Build/onyx)")
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(30))
            .build().map_err(|_| Error::new("Could not initialize HTTPS."))?;
        Ok(Self {
            http,
            api: "https://i.weread.qq.com".into(),
            wx: "https://open.weixin.qq.com".into(),
            poll: "https://long.open.weixin.qq.com".into(),
        })
    }
    async fn send(&self, request: RequestBuilder) -> Result<Value> {
        let mut response = request.send().await.map_err(|e| {
            Error::new(if e.is_timeout() {
                "Request timed out. Check your connection and retry."
            } else {
                "Network or TLS connection failed. Please retry."
            })
        })?;
        let status = response.status();
        if status.as_u16() == 401 {
            return Err(Error::auth("Session expired. Please sign in again."));
        }
        if !status.is_success() {
            return Err(Error::new(format!(
                "Service returned HTTP {}. Please retry.",
                status.as_u16()
            )));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::new("Could not read service response."))?
        {
            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                return Err(Error::new("Service response is too large."));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| Error::new("Service returned invalid JSON."))?;
        if !value.is_object() {
            return Err(Error::new("Service returned an invalid response."));
        }
        if let Some(code) = value.get("errCode").or_else(|| value.get("errcode")) {
            let code = integer(code)
                .ok_or_else(|| Error::new("Service returned an invalid error code."))?;
            match code {
                0 => {}
                -2012 => return Err(Error::auth("Session expired. Please sign in again.")),
                -2041 => {
                    return Err(Error::new(
                        "WeRead requires human verification in the official client.",
                    ))
                }
                -2014 => return Err(Error::new("Too many requests. Please try again later.")),
                code => {
                    return Err(Error::new(format!(
                        "WeRead rejected the request (code {code})."
                    )))
                }
            }
        }
        Ok(value)
    }
    pub async fn qr(&self) -> Result<(String, String)> {
        let ticket = self
            .send(
                self.http
                    .get(format!("{}/wxticket", self.api))
                    .query(&[("nonceStr", "weread")]),
            )
            .await?;
        let signature = text(&ticket, "signature")?;
        let timestamp = scalar(ticket.get("timeStamp"))
            .ok_or_else(|| Error::new("Login ticket has no timestamp."))?;
        let qr = self
            .send(
                self.http
                    .get(format!("{}/connect/sdk/qrconnect", self.wx))
                    .query(&[
                        ("appid", "wxab9b71ad2b90ff34"),
                        ("noncestr", "weread"),
                        ("timestamp", &timestamp),
                        ("scope", "snsapi_userinfo,snsapi_timeline,snsapi_friend"),
                        ("signature", &signature),
                    ]),
            )
            .await?;
        if qr.get("errcode").and_then(integer) != Some(0) {
            return Err(Error::new("Could not create QR login."));
        }
        let uuid = text(&qr, "uuid")?;
        let mut confirm = reqwest::Url::parse(&format!("{}/connect/confirm", self.wx))
            .map_err(|_| Error::new("Invalid login URL."))?;
        confirm.query_pairs_mut().append_pair("uuid", &uuid);
        Ok((uuid, confirm.into()))
    }
    pub async fn poll(
        &self,
        uuid: &str,
        last: Option<i64>,
        timeout: Duration,
    ) -> Result<(i64, Poll)> {
        let mut request = self
            .http
            .get(format!("{}/connect/l/qrconnect", self.poll))
            .header("User-Agent", "Mozilla/5.0")
            .timeout(timeout)
            .query(&[("f", "json"), ("uuid", uuid)]);
        if let Some(last) = last {
            request = request.query(&[("last", last)]);
        }
        let value = self.send(request).await?;
        let code = value
            .get("wx_errcode")
            .and_then(integer)
            .ok_or_else(|| Error::new("Invalid QR login status."))?;
        let state = match code {
            404 => Poll::Scanned,
            405 => Poll::Confirmed(text(&value, "wx_code")?),
            402 => Poll::Expired,
            403 => Poll::Declined,
            408 => Poll::Waiting,
            _ => return Err(Error::new(format!("Unknown QR login status ({code})."))),
        };
        Ok((code, state))
    }
    pub async fn exchange(&self, code: &str, device: &str) -> Result<Credentials> {
        let timestamp = timestamp()?;
        let random = rand::rng().random_range(0..1000u32);
        let install = format!(
            "eink31{}",
            (0..26)
                .map(|_| char::from(b'0' + rand::rng().random_range(0..10)))
                .collect::<String>()
        );
        let value = self.send(self.http.post(format!("{}/login", self.api)).json(&json!({
            "appFirstInstall":1,"code":code,"deviceId":device,"deviceName":"BOOX","installId":install,
            "isAutoLogout":0,"isFromQrcode":1,"random":random,"signature":signature(timestamp,device,random),
            "timestamp":timestamp,"trackId":"","deviceType":3
        }))).await?;
        let credentials = Credentials {
            vid: scalar(value.get("vid"))
                .ok_or_else(|| Error::new("Login returned no account identity."))?,
            access_token: text(&value, "accessToken")?,
            refresh_token: text(&value, "refreshToken")?,
            device_id: device.into(),
        };
        credentials.validate()?;
        Ok(credentials)
    }
    pub async fn refresh(&self, old: &Credentials) -> Result<Credentials> {
        let timestamp = timestamp()?;
        let random = rand::rng().random_range(1..=1000u32);
        let value = self.send(self.http.post(format!("{}/login", self.api)).json(&json!({
            "deviceId":old.device_id,"deviceName":"BOOX","inBackground":0,"kickType":1,"random":random,
            "refCgi":"","refreshToken":old.refresh_token,"signature":signature(timestamp,&old.device_id,random),
            "timestamp":timestamp,"trackId":"","deviceType":3
        }))).await?;
        let vid = match value.get("vid") {
            None => old.vid.clone(),
            Some(v) => scalar(Some(v)).ok_or_else(|| Error::new("Invalid refreshed identity."))?,
        };
        if vid != old.vid {
            return Err(Error::new("Refreshed session belongs to another account."));
        }
        let refresh_token = if value.get("refreshToken").is_some() {
            text(&value, "refreshToken")?
        } else {
            old.refresh_token.clone()
        };
        Ok(Credentials {
            vid,
            access_token: text(&value, "accessToken")?,
            refresh_token,
            device_id: old.device_id.clone(),
        })
    }
    pub async fn shelf(&self, credentials: &Credentials) -> Result<Vec<Book>> {
        let value = self
            .send(
                self.http
                    .get(format!("{}/shelf/sync", self.api))
                    .header("vid", &credentials.vid)
                    .header("accessToken", &credentials.access_token),
            )
            .await?;
        parse_shelf(&value)
    }
}
fn integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}
fn scalar(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(n.to_string()),
        _ => None,
    }
}
fn text(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Error::new(format!("Service response is missing {key}.")))
}
fn timestamp() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::new("Set the device clock before signing in."))?
        .as_millis() as u64)
}
fn signature(timestamp: u64, device: &str, random: u32) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{timestamp}{device}{random}").as_bytes())
    )
}
pub fn new_device_id() -> String {
    format!(
        "eink334691225{:019}",
        rand::rng().random::<u64>() & 0x7fff_ffff_ffff_ffff
    )
}
fn parse_shelf(value: &Value) -> Result<Vec<Book>> {
    let entries = value
        .get("books")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new("Service response has no valid bookshelf."))?;
    entries
        .iter()
        .map(|v| {
            let metadata = v.get("bookInfo").filter(|v| v.is_object()).unwrap_or(v);
            let id = text(v, "bookId").or_else(|_| text(metadata, "bookId"))?;
            let field = |key: &str| {
                metadata
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| {
                        v.get(key)
                            .and_then(Value::as_str)
                            .filter(|s| !s.trim().is_empty())
                    })
            };
            Ok(Book {
                title: field("title").unwrap_or(&id).to_owned(),
                author: field("author").unwrap_or("").to_owned(),
                cover: field("cover").unwrap_or("").to_owned(),
                finished: v.get("finishReading").and_then(integer) == Some(1),
                id,
            })
        })
        .collect()
}
