use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::mpsc,
    thread,
};

pub struct Mock {
    pub api: Api,
    pub requests: mpsc::Receiver<String>,
    worker: thread::JoinHandle<()>,
}
impl Mock {
    pub fn new(responses: Vec<(u16, Value)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (tx, requests) = mpsc::channel();
        let worker = thread::spawn(move || {
            for (status, value) in responses {
                let started = std::time::Instant::now();
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                started.elapsed() < Duration::from_secs(10),
                                "expected a request"
                            );
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut raw = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
                    let n = socket.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    raw.extend_from_slice(&buffer[..n]);
                    if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&raw[..end]);
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if raw.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                tx.send(String::from_utf8(raw).unwrap()).unwrap();
                let body = value.to_string();
                write!(socket,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        let mut api = Api::new().unwrap();
        api.api = base.clone();
        api.wx = base.clone();
        api.poll = base;
        Self {
            api,
            requests,
            worker,
        }
    }
    pub fn finish(self) {
        self.worker.join().unwrap();
    }
}
pub fn credentials() -> Credentials {
    Credentials {
        vid: "42".into(),
        access_token: "test-access".into(),
        refresh_token: "test-refresh".into(),
        device_id: "test-device".into(),
    }
}
pub fn temporary() -> PathBuf {
    std::env::temp_dir().join(format!(
        "rereader-test-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ))
}
fn body(request: &str) -> Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test]
async fn qr_login_and_shelf_preserve_client_contract() {
    let mock = Mock::new(vec![
        (200, json!({"signature":"ticket","timeStamp":123})),
        (200, json!({"errcode":0,"uuid":"qr-one"})),
        (200, json!({"wx_errcode":404})),
        (200, json!({"wx_errcode":405,"wx_code":"confirm-code"})),
        (
            200,
            json!({"vid":42,"accessToken":"test-access","refreshToken":"test-refresh"}),
        ),
        (
            200,
            json!({"books":[{"bookId":"book-1","title":"三体","author":"刘慈欣"}]}),
        ),
    ]);
    let (uuid, url) = mock.api.qr().await.unwrap();
    assert_eq!(uuid, "qr-one");
    assert!(url.ends_with("/connect/confirm?uuid=qr-one"));
    assert_eq!(
        mock.api
            .poll(&uuid, None, Duration::from_secs(5))
            .await
            .unwrap(),
        (404, Poll::Scanned)
    );
    assert_eq!(
        mock.api
            .poll(&uuid, Some(404), Duration::from_secs(5))
            .await
            .unwrap(),
        (405, Poll::Confirmed("confirm-code".into()))
    );
    let c = mock
        .api
        .exchange("confirm-code", "device-one")
        .await
        .unwrap();
    assert_eq!(c.vid, "42");
    assert_eq!(mock.api.shelf(&c).await.unwrap()[0].title, "三体");
    let requests: Vec<_> = (0..6).map(|_| mock.requests.recv().unwrap()).collect();
    assert!(requests[0].starts_with("GET /wxticket?nonceStr=weread "));
    assert!(requests[1].contains("appid=wxab9b71ad2b90ff34"));
    assert!(requests[1].contains("appver: 2.1.2.10245900"));
    assert!(requests[3].contains("last=404"));
    let login = body(&requests[4]);
    assert_eq!(login["code"], "confirm-code");
    assert_eq!(login["deviceType"], 3);
    assert_eq!(login["deviceName"], "BOOX");
    assert_eq!(
        login["signature"],
        signature(
            login["timestamp"].as_u64().unwrap(),
            "device-one",
            login["random"].as_u64().unwrap() as u32
        )
    );
    assert_eq!(login["installId"].as_str().unwrap().len(), 32);
    assert!(requests[5].starts_with("GET /shelf/sync "));
    assert!(requests[5].contains("vid: 42"));
    assert!(requests[5].contains("accesstoken: test-access"));
    assert!(!requests[5].to_lowercase().contains("cookie:"));
    mock.finish();
}

#[tokio::test]
async fn errors_and_refresh_rotation_are_checked() {
    let mock = Mock::new(vec![
        (200, json!({"errCode":-2012})),
        (
            200,
            json!({"vid":42,"accessToken":"new-access","refreshToken":"new-refresh"}),
        ),
        (200, json!({"vid":99,"accessToken":"wrong"})),
        (
            200,
            json!({"errCode":-2041,"errMsg":"secret-should-not-appear"}),
        ),
        (200, json!({"books":null})),
        (200, json!({"wx_errcode":402})),
        (200, json!({"wx_errcode":403})),
    ]);
    let old = credentials();
    assert!(mock.api.shelf(&old).await.unwrap_err().auth);
    let new = mock.api.refresh(&old).await.unwrap();
    assert_eq!(new.refresh_token, "new-refresh");
    assert!(mock
        .api
        .refresh(&old)
        .await
        .err()
        .unwrap()
        .message
        .contains("another account"));
    let error = mock.api.shelf(&old).await.unwrap_err();
    assert!(!error.auth);
    assert!(!error.message.contains("secret"));
    assert!(mock.api.shelf(&old).await.is_err());
    assert_eq!(
        mock.api
            .poll("qr", None, Duration::from_secs(5))
            .await
            .unwrap()
            .1,
        Poll::Expired
    );
    assert_eq!(
        mock.api
            .poll("qr", None, Duration::from_secs(5))
            .await
            .unwrap()
            .1,
        Poll::Declined
    );
    mock.finish();
}

#[test]
fn credentials_are_private_atomic_and_removable() {
    let path = temporary();
    let store = Store::new(path.clone());
    assert!(store.load().unwrap().is_none());
    store.save(&credentials()).unwrap();
    assert_eq!(store.load().unwrap().unwrap().vid, "42");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(path.join("session.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(!path.join("session.json.part").exists());
    std::fs::write(path.join("session.json"), "{}").unwrap();
    assert!(store.load().is_err());
    store.clear().unwrap();
    store.clear().unwrap();
    assert!(store.load().unwrap().is_none());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn empty_shelf_is_valid_but_missing_shelf_is_not() {
    assert!(parse_shelf(&json!({"books":[]})).unwrap().is_empty());
    assert!(parse_shelf(&json!({})).is_err());
    assert!(parse_shelf(&json!({"books":[{"title":"missing identity"}]})).is_err());
    for _ in 0..10 {
        let device = new_device_id();
        assert_eq!(device.len(), 32);
        assert!(device.starts_with("eink334691225"));
    }
}

#[tokio::test]
#[ignore = "creates an anonymous live WeChat QR session; no account login"]
async fn anonymous_qr_service_probe() {
    let api = Api::new().unwrap();
    let (uuid, url) = api.qr().await.unwrap();
    assert!(!uuid.is_empty());
    assert!(url.starts_with("https://open.weixin.qq.com/connect/confirm?uuid="));
}

#[test]
fn shelf_preserves_cover_and_nested_metadata() {
    let books=parse_shelf(&json!({"books":[
        {"bookId":"a","title":"生死疲劳","cover":"https://qpic.cn/a.jpg","finishReading":1},
        {"bookId":"b","title":"","bookInfo":{"title":"克拉拉与太阳","author":"石黑一雄","cover":"https://qpic.cn/b.jpg"}},
        {"bookId":"c","title":"","cover":null}
    ]})).unwrap();
    assert_eq!(books[0].title, "生死疲劳");
    assert_eq!(books[0].cover, "https://qpic.cn/a.jpg");
    assert!(books[0].finished);
    assert_eq!(books[1].title, "克拉拉与太阳");
    assert_eq!(books[1].author, "石黑一雄");
    assert_eq!(books[1].cover, "https://qpic.cn/b.jpg");
    assert_eq!(books[2].title, "c");
    assert!(books[2].cover.is_empty());
}

#[test]
fn bundled_font_contains_chinese_book_and_ui_characters() {
    let face = ttf_parser::Face::parse(crate::DEFAULT_FONT, 0).unwrap();
    for ch in "书架生死疲劳克拉拉与太阳微信读书登录搜索".chars() {
        assert!(face.glyph_index(ch).is_some(), "missing {ch}");
    }
}
