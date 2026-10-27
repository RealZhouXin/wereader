use crate::{
    weread::{self, Api, Book, Cover, Credentials, Error, Poll, Store},
    AppWindow, ShelfRow,
};
use futures_util::{stream, StreamExt};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::mpsc,
    thread,
    time::Duration,
};
use tokio::sync::mpsc as channel;

#[derive(Clone, Copy)]
enum Action {
    Restore,
    Login,
    Refresh,
    Logout,
    Shutdown,
}
struct Command {
    generation: u64,
    action: Action,
}
enum Event {
    Status(String),
    Qr(String),
    SignedIn(String),
    Shelf(Vec<Book>),
    AccountMetrics {
        reading_time: String,
        weekly_reading_time: String,
    },
    Cover {
        id: String,
        image: Option<Cover>,
    },
    SignedOut,
    Failed(String),
}
struct Update {
    generation: u64,
    event: Event,
}

pub struct Controller {
    timer: slint::Timer,
    commands: channel::UnboundedSender<Command>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.timer.stop();
        let _ = self.commands.send(Command {
            generation: u64::MAX,
            action: Action::Shutdown,
        });
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn start(ui: &AppWindow) -> weread::Result<Controller> {
    let store = Store::new(Store::default_path()?)?;
    let api = Api::new()?;
    ui.set_busy(true);
    ui.set_status("正在恢复登录…".into());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| Error::new("Cannot start network runtime."))?;
    let (commands, receiver) = channel::unbounded_channel();
    let (events, updates) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("weread-network".into())
        .spawn(move || runtime.block_on(run(api, store, receiver, events)))
        .map_err(|_| Error::new("Cannot start network worker."))?;
    let generation = Rc::new(Cell::new(0u64));
    let all_books: Rc<RefCell<Vec<ShelfRow>>> = Rc::new(RefCell::new(Vec::new()));
    for (action, install) in [
        (Action::Login, 0),
        (Action::Refresh, 1),
        (Action::Logout, 2),
    ] {
        let weak = ui.as_weak();
        let commands = commands.clone();
        let generation = generation.clone();
        let all_books = all_books.clone();
        let callback = move || {
            if let Some(ui) = weak.upgrade() {
                let id = generation.get().wrapping_add(1);
                generation.set(id);
                ui.set_busy(true);
                ui.set_qr_image(slint::Image::default());
                ui.set_has_qr(false);
                if matches!(action, Action::Login | Action::Logout) {
                    ui.set_logged_in(false);
                    all_books.borrow_mut().clear();
                    ui.set_query("".into());
                    ui.set_total_books(0);
                    ui.set_account("".into());
                    ui.set_reading_time("".into());
                    ui.set_weekly_reading_time("".into());
                    ui.set_books(ModelRc::new(VecModel::default()));
                    ui.set_page(1);
                }
                ui.set_status(
                    match action {
                        Action::Login => "正在创建二维码…",
                        Action::Logout => "正在退出登录…",
                        _ => "正在加载书架…",
                    }
                    .into(),
                );
                if commands
                    .send(Command {
                        generation: id,
                        action,
                    })
                    .is_err()
                {
                    ui.set_busy(false);
                    ui.set_status("网络任务已停止，请重启应用".into());
                }
            }
        };
        match install {
            0 => ui.on_login(callback),
            1 => ui.on_refresh(callback),
            _ => ui.on_logout(callback),
        }
    }
    let weak = ui.as_weak();
    let searchable = all_books.clone();
    ui.on_search(move || {
        if let Some(ui) = weak.upgrade() {
            show_filtered(&ui, &searchable.borrow(), true);
        }
    });
    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    // A Slint timer works with both winit and the Kindle backend event loop.
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            let Some(ui) = weak.upgrade() else { return };
            while let Ok(update) = updates.try_recv() {
                if update.generation != generation.get() {
                    continue;
                }
                match update.event {
                    Event::Status(status) => ui.set_status(status.into()),
                    Event::Qr(url) => match qr_image(&url) {
                        Ok(image) => {
                            ui.set_qr_image(image);
                            ui.set_has_qr(true);
                        }
                        Err(error) => {
                            ui.set_status(error.message.into());
                            ui.set_busy(false);
                        }
                    },
                    Event::SignedIn(vid) => {
                        ui.set_logged_in(true);
                        ui.set_account(format!("账号 {vid}").into());
                        ui.set_has_qr(false);
                        ui.set_qr_image(slint::Image::default());
                        ui.set_reading_time("加载中…".into());
                        ui.set_weekly_reading_time("加载中…".into());
                    }
                    Event::AccountMetrics {
                        reading_time,
                        weekly_reading_time,
                    } => {
                        ui.set_reading_time(reading_time.into());
                        ui.set_weekly_reading_time(weekly_reading_time.into());
                    }
                    Event::Shelf(books) => {
                        let count = books.len();
                        *all_books.borrow_mut() = books
                            .into_iter()
                            .map(|b| ShelfRow {
                                book_id: b.id.into(),
                                title: b.title.into(),
                                author: b.author.into(),
                                cover: slint::Image::default(),
                                cover_state: if b.cover.is_empty() { 2 } else { 0 },
                                finished: b.finished,
                            })
                            .collect();
                        ui.set_total_books(count as i32);
                        show_filtered(&ui, &all_books.borrow(), true);
                        ui.set_busy(false);
                        ui.set_status(if count == 0 {
                            "书架还是空的".into()
                        } else {
                            format!("共 {count} 本书").into()
                        });
                    }
                    Event::Cover { id, image } => {
                        let mut all = all_books.borrow_mut();
                        if let Some(row) = all.iter_mut().find(|row| row.book_id.as_str() == id) {
                            if let Some(image) = image {
                                let buffer =
                                    slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                                        &image.pixels,
                                        image.width,
                                        image.height,
                                    );
                                row.cover = slint::Image::from_rgba8(buffer);
                                row.cover_state = 1;
                            } else {
                                row.cover_state = 2;
                            }
                            let model = ui.get_books();
                            for index in 0..model.row_count() {
                                if model
                                    .row_data(index)
                                    .is_some_and(|r| r.book_id == row.book_id)
                                {
                                    model.set_row_data(index, row.clone());
                                }
                            }
                        }
                    }
                    Event::SignedOut => {
                        all_books.borrow_mut().clear();
                        ui.set_total_books(0);
                        ui.set_logged_in(false);
                        ui.set_busy(false);
                        ui.set_has_qr(false);
                        ui.set_account("".into());
                        ui.set_reading_time("".into());
                        ui.set_weekly_reading_time("".into());
                        ui.set_books(ModelRc::new(VecModel::default()));
                        ui.set_status("点击登录，扫码查看微信读书书架".into());
                    }
                    Event::Failed(message) => {
                        if ui.get_reading_time() == "加载中…" {
                            ui.set_reading_time("暂不可用".into());
                        }
                        if ui.get_weekly_reading_time() == "加载中…" {
                            ui.set_weekly_reading_time("暂不可用".into());
                        }
                        ui.set_busy(false);
                        ui.set_has_qr(false);
                        ui.set_qr_image(slint::Image::default());
                        ui.set_status(message.into());
                    }
                }
            }
        },
    );
    let _ = commands.send(Command {
        generation: 0,
        action: Action::Restore,
    });
    Ok(Controller {
        timer,
        commands,
        worker: Some(worker),
    })
}

async fn run(
    api: Api,
    store: Store,
    mut commands: channel::UnboundedReceiver<Command>,
    events: mpsc::Sender<Update>,
) {
    let mut credentials = None;
    let mut next = commands.recv().await;
    while let Some(command) = next {
        if matches!(command.action, Action::Shutdown) {
            break;
        }
        let emit = |event| {
            let _ = events.send(Update {
                generation: command.generation,
                event,
            });
        };
        // Dropping an operation cancels its HTTP future. All credential commits are
        // synchronous on this one worker; old generations cannot commit afterwards.
        next = tokio::select! {
            biased;
            command = commands.recv() => command,
            result = execute(&api, &store, &mut credentials, command.action, &emit) => {
                if let Err(error) = result { emit(Event::Failed(error.message)); }
                commands.recv().await
            }
        };
    }
}

async fn execute(
    api: &Api,
    store: &Store,
    credentials: &mut Option<Credentials>,
    action: Action,
    emit: &impl Fn(Event),
) -> weread::Result<()> {
    match action {
        Action::Restore => {
            *credentials = store.load().await?;
            if credentials.is_none() {
                emit(Event::SignedOut);
                return Ok(());
            }
        }
        Action::Logout => {
            store.clear().await?;
            *credentials = None;
            emit(Event::SignedOut);
            return Ok(());
        }
        Action::Login => {
            // Remove the old account before obtaining a new session.
            store.clear().await?;
            let device = credentials
                .as_ref()
                .map(|c| c.device_id.clone())
                .unwrap_or_else(weread::new_device_id);
            *credentials = None;
            let (uuid, url) = api.qr().await?;
            emit(Event::Qr(url));
            emit(Event::Status("请使用微信扫码，在手机上确认登录".into()));
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            let mut last = None;
            let code = loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return Err(Error::new("二维码已过期，请重新扫码登录"));
                }
                let (code, state) = api
                    .poll(&uuid, last, remaining.min(Duration::from_secs(65)))
                    .await?;
                last = Some(code);
                match state {
                    Poll::Confirmed(code) => break code,
                    Poll::Expired => return Err(Error::new("二维码已过期，请重新扫码登录")),
                    Poll::Declined => return Err(Error::new("已取消授权，请重新扫码登录")),
                    Poll::Scanned => emit(Event::Status("已扫码，请在手机上确认登录".into())),
                    Poll::Waiting => {}
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            };
            emit(Event::Status("正在确认账号…".into()));
            let new = api.exchange(&code, &device).await?;
            store.save(&new).await?;
            *credentials = Some(new);
        }
        Action::Refresh => {}
        Action::Shutdown => return Ok(()),
    }
    let old = credentials
        .as_ref()
        .ok_or_else(|| Error::new("Sign in before refreshing your shelf."))?;
    let identity = store
        .identity()
        .await?
        .ok_or_else(|| Error::new("Sign in before loading data."))?;
    if identity.account_id != old.vid {
        return Err(Error::new(
            "Account session changed. Please restore or sign in again.",
        ));
    }
    emit(Event::SignedIn(old.vid.clone()));
    let cached = store.load_shelf(&identity).await?;
    if !cached.is_empty() {
        emit(Event::Shelf(cached.clone()));
        for book in &cached {
            if let Some(image) = weread::cached_cover(store, &identity, book).await {
                emit(Event::Cover {
                    id: book.id.clone(),
                    image: Some(image),
                });
            }
        }
    }
    emit(Event::Status("正在加载书架…".into()));
    let books = match api.shelf(old).await {
        Ok(books) => books,
        Err(error) if error.auth => {
            let new = api.refresh(old).await?;
            // Persist rotating refresh tokens before the shelf retry.
            store.rotate_session(&identity, &new).await?;
            *credentials = Some(new);
            api.shelf(credentials.as_ref().unwrap()).await?
        }
        Err(error) => return Err(error),
    };
    store
        .apply_shelf_snapshot(&identity, books.clone(), None)
        .await?;
    emit(Event::Shelf(books.clone()));
    // Account failures are independent of the shelf and of each other.
    let current = credentials.as_ref().unwrap();
    let (mut reading, mut weekly) =
        tokio::join!(api.reading_time(current), api.weekly_reading_time(current));
    if reading.as_ref().is_err_and(|e| e.auth) || weekly.as_ref().is_err_and(|e| e.auth) {
        let refreshed = async {
            let new = api.refresh(current).await?;
            store.rotate_session(&identity, &new).await?;
            Ok::<_, Error>(new)
        }
        .await;
        match refreshed {
            Ok(new) => {
                *credentials = Some(new);
                let current = credentials.as_ref().unwrap();
                (reading, weekly) =
                    tokio::join!(api.reading_time(current), api.weekly_reading_time(current));
            }
            Err(error) => {
                if reading.as_ref().is_err_and(|e| e.auth) {
                    reading = Err(error.clone());
                }
                if weekly.as_ref().is_err_and(|e| e.auth) {
                    weekly = Err(error);
                }
            }
        }
    }
    emit(Event::AccountMetrics {
        reading_time: reading
            .map(format_reading_time)
            .unwrap_or_else(|_| "暂不可用".into()),
        weekly_reading_time: weekly
            .map(format_reading_time)
            .unwrap_or_else(|_| "暂不可用".into()),
    });
    let mut covers = stream::iter(books.into_iter().filter(|b| !b.cover.is_empty()))
        .map(|book| {
            let identity = &identity;
            async move {
                let image = api.cover(store, identity, &book).await.ok();
                (book.id, image)
            }
        })
        .buffer_unordered(4);
    while let Some((id, image)) = covers.next().await {
        emit(Event::Cover { id, image });
    }
    Ok(())
}

fn format_reading_time(seconds: u64) -> String {
    if (1..60).contains(&seconds) {
        "不足 1 分钟".into()
    } else {
        format!("{} 小时 {} 分钟", seconds / 3600, seconds % 3600 / 60)
    }
}

fn filtered_rows(all: &[ShelfRow], query: &str) -> Vec<ShelfRow> {
    let query = query.trim().to_lowercase();
    all.iter()
        .filter(|b| {
            query.is_empty()
                || b.title.to_lowercase().contains(&query)
                || b.author.to_lowercase().contains(&query)
        })
        .cloned()
        .collect()
}
fn show_filtered(ui: &AppWindow, all: &[ShelfRow], reset: bool) {
    ui.set_books(ModelRc::new(VecModel::from(filtered_rows(
        all,
        ui.get_query().as_str(),
    ))));
    if reset {
        ui.set_page(1);
    }
}

pub fn qr_image(url: &str) -> weread::Result<slint::Image> {
    let code = qrcode::QrCode::new(url.as_bytes())
        .map_err(|_| Error::new("Could not generate login QR code."))?;
    let modules = code.width() + 8;
    let scale = (280 / modules).max(1);
    let size = 280.max(modules);
    let padding = (size - code.width() * scale) / 2;
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(size as u32, size as u32);
    for (i, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
        let x = i % size;
        let y = i / size;
        let dark = x >= padding
            && y >= padding
            && x < padding + code.width() * scale
            && y < padding + code.width() * scale
            && code[((x - padding) / scale, (y - padding) / scale)] == qrcode::Color::Dark;
        let c = if dark { 0 } else { 255 };
        *pixel = slint::Rgb8Pixel::new(c, c, c);
    }
    Ok(slint::Image::from_rgb8(buffer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weread::tests::{credentials, temporary, Mock};
    use serde_json::json;

    #[test]
    fn shelf_search_matches_chinese_titles_and_authors() {
        let rows = vec![
            ShelfRow {
                title: "生死疲劳".into(),
                author: "莫言".into(),
                ..Default::default()
            },
            ShelfRow {
                title: "Klara and the Sun".into(),
                author: "石黑一雄".into(),
                ..Default::default()
            },
        ];
        assert_eq!(filtered_rows(&rows, "疲劳").len(), 1);
        assert_eq!(filtered_rows(&rows, " 石黑 ").len(), 1);
        assert_eq!(filtered_rows(&rows, "KLARA").len(), 1);
        assert!(filtered_rows(&rows, "不存在").is_empty());
        assert_eq!(filtered_rows(&rows, "").len(), 2);
    }

    #[tokio::test]
    async fn restore_refreshes_once_and_commits_rotated_credentials() {
        let mock = Mock::new(vec![
            (200, json!({"errCode":-2012})),
            (
                200,
                json!({"vid":"42","accessToken":"rotated","refreshToken":"rotated-refresh"}),
            ),
            (200, json!({"books":[{"bookId":"b","title":"Book"}]})),
            (200, json!({"totalReadingTime":3661,"totalReadTime":1800})),
            (200, json!({"totalReadingTime":3661,"totalReadTime":1800})),
        ]);
        let path = temporary();
        let store = Store::new(path.clone()).unwrap();
        store.save(&credentials()).await.unwrap();
        let (tx, rx) = mpsc::channel();
        let mut current = None;
        execute(&mock.api, &store, &mut current, Action::Restore, &|event| {
            tx.send(event).unwrap();
        })
        .await
        .unwrap();
        assert_eq!(
            store.load().await.unwrap().unwrap().refresh_token,
            "rotated-refresh"
        );
        let events: Vec<_> = rx.try_iter().collect();
        assert!(events
            .iter()
            .any(|e| matches!(e,Event::Shelf(books) if books.len()==1)));
        assert!(events.iter().any(
            |e| matches!(e, Event::AccountMetrics { reading_time, weekly_reading_time }
            if reading_time == "1 小时 1 分钟" && weekly_reading_time == "0 小时 30 分钟")
        ));
        let requests: Vec<_> = (0..5).map(|_| mock.requests.recv().unwrap()).collect();
        assert!(requests[1].starts_with("POST /login "));
        assert!(requests[2].contains("accesstoken: rotated"));
        assert!(requests[3..]
            .iter()
            .all(|r| r.contains("accesstoken: rotated")));
        execute(&mock.api, &store, &mut current, Action::Logout, &|_| {})
            .await
            .unwrap();
        assert!(current.is_none());
        assert!(store.load().await.unwrap().is_none());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
        mock.finish();
    }

    #[tokio::test]
    async fn missing_weekly_time_does_not_hide_total_reading_time_or_shelf() {
        let mock = Mock::new(vec![
            (200, json!({"books":[]})),
            (200, json!({"totalReadingTime":0})),
            (200, json!({"totalReadingTime":0})),
        ]);
        let path = temporary();
        let store = Store::new(path.clone()).unwrap();
        store.save(&credentials()).await.unwrap();
        let events = RefCell::new(Vec::new());
        execute(&mock.api, &store, &mut None, Action::Restore, &|e| {
            events.borrow_mut().push(e)
        })
        .await
        .unwrap();
        assert!(events
            .borrow()
            .iter()
            .any(|e| matches!(e, Event::Shelf(books) if books.is_empty())));
        assert!(events.borrow().iter().any(
            |e| matches!(e, Event::AccountMetrics { reading_time, weekly_reading_time }
            if reading_time == "0 小时 0 分钟" && weekly_reading_time == "暂不可用")
        ));
        let requests: Vec<_> = (0..3).map(|_| mock.requests.recv().unwrap()).collect();
        assert!(requests
            .iter()
            .any(|r| r.starts_with("GET /user/profile?userVid=42&totalReadingTime=1 ")));
        assert!(requests
            .iter()
            .any(|r| r.starts_with("GET /readdata/detail?mode=weekly&baseTime=")));
        let weekly_request = requests
            .iter()
            .find(|r| r.starts_with("GET /readdata/detail?"))
            .unwrap();
        let request_path = weekly_request.split_whitespace().nth(1).unwrap();
        let url = reqwest::Url::parse(&format!("http://localhost{request_path}")).unwrap();
        let base_time: u64 = url
            .query_pairs()
            .find(|(key, _)| key == "baseTime")
            .unwrap()
            .1
            .parse()
            .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(
            base_time <= now && now - base_time < 60,
            "current week must be anchored in Unix seconds"
        );
        assert!(!requests.iter().any(|r| r.contains("/pay/")));
        assert!(requests
            .iter()
            .all(|r| r.contains("vid: 42") && r.contains("accesstoken: test-access")));
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
        mock.finish();
    }

    #[test]
    fn reading_time_formats_minutes_without_rounding_up() {
        assert_eq!(format_reading_time(0), "0 小时 0 分钟");
        assert_eq!(format_reading_time(59), "不足 1 分钟");
        assert_eq!(format_reading_time(60), "0 小时 1 分钟");
        assert_eq!(format_reading_time(3659), "1 小时 0 分钟");
        assert_eq!(format_reading_time(360000), "100 小时 0 分钟");
    }

    #[tokio::test]
    async fn account_metrics_share_one_token_refresh_and_retry_once() {
        let mock = Mock::new(vec![
            (200, json!({"books":[]})),
            (401, json!({})),
            (200, json!({"errCode":-2012})),
            (
                200,
                json!({"accessToken":"metrics-access","refreshToken":"metrics-refresh"}),
            ),
            (200, json!({"totalReadingTime":7200,"totalReadTime":0})),
            (200, json!({"totalReadingTime":7200,"totalReadTime":0})),
        ]);
        let path = temporary();
        let store = Store::new(path.clone()).unwrap();
        store.save(&credentials()).await.unwrap();
        let events = RefCell::new(Vec::new());
        let mut current = None;
        execute(&mock.api, &store, &mut current, Action::Restore, &|e| {
            events.borrow_mut().push(e)
        })
        .await
        .unwrap();
        assert_eq!(
            store.load().await.unwrap().unwrap().refresh_token,
            "metrics-refresh"
        );
        assert!(events.borrow().iter().any(
            |e| matches!(e, Event::AccountMetrics { reading_time, weekly_reading_time }
            if reading_time == "2 小时 0 分钟" && weekly_reading_time == "0 小时 0 分钟")
        ));
        let requests: Vec<_> = (0..6).map(|_| mock.requests.recv().unwrap()).collect();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.starts_with("POST /login "))
                .count(),
            1
        );
        assert!(requests[4..]
            .iter()
            .all(|r| r.contains("accesstoken: metrics-access")));
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
        mock.finish();
    }

    #[tokio::test]
    async fn network_failure_keeps_the_cached_shelf_visible() {
        let mock = Mock::new(vec![(503, json!({}))]);
        let path = temporary();
        let store = Store::new(path.clone()).unwrap();
        let identity = store.save(&credentials()).await.unwrap();
        let book = Book {
            id: "offline".into(),
            title: "离线书架".into(),
            author: String::new(),
            cover: String::new(),
            finished: false,
        };
        store
            .apply_shelf_snapshot(&identity, vec![book.clone()], None)
            .await
            .unwrap();
        let events = RefCell::new(Vec::new());
        let mut current = None;
        let result = execute(&mock.api, &store, &mut current, Action::Restore, &|e| {
            events.borrow_mut().push(e)
        })
        .await;
        assert!(result.is_err());
        assert!(events
            .borrow()
            .iter()
            .any(|e| matches!(e,Event::Shelf(books) if books==&vec![book.clone()])));
        assert_eq!(store.load_shelf(&identity).await.unwrap(), vec![book]);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
        mock.finish();
    }

    #[tokio::test]
    async fn logout_cancels_a_blocked_login_and_shutdown_finishes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = Api::test_endpoint(&format!("http://{}", listener.local_addr().unwrap()));
        let path = temporary();
        let (tx, rx) = channel::unbounded_channel();
        let (events, updates) = mpsc::channel();
        let task = tokio::spawn(run(api, Store::new(path.clone()).unwrap(), rx, events));
        tx.send(Command {
            generation: 1,
            action: Action::Login,
        })
        .unwrap();
        let (_socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .unwrap()
            .unwrap();
        tx.send(Command {
            generation: 2,
            action: Action::Logout,
        })
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(update) = updates.try_recv() {
                    assert_eq!(update.generation, 2);
                    assert!(matches!(update.event, Event::SignedOut));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        tx.send(Command {
            generation: 3,
            action: Action::Shutdown,
        })
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert!(!path.join("session.json").exists());
        std::fs::remove_dir_all(path).unwrap();
    }
}
