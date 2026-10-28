use crate::{
    weread::{self, Api, Cover, Credentials, Error, Poll, ShelfGroup, ShelfSnapshot, Store},
    AppWindow, ShelfPreview, ShelfRow,
};
use futures_util::{stream, StreamExt};
use slint::{ComponentHandle, ModelRc, VecModel};
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
    Shelf(ShelfSnapshot),
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
    let all_groups = Rc::new(RefCell::new(Vec::<ShelfGroup>::new()));
    for (action, install) in [
        (Action::Login, 0),
        (Action::Refresh, 1),
        (Action::Logout, 2),
    ] {
        let weak = ui.as_weak();
        let commands = commands.clone();
        let generation = generation.clone();
        let all_books = all_books.clone();
        let all_groups = all_groups.clone();
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
                    all_groups.borrow_mut().clear();
                    reset_groups(&ui);
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
    bind_shelf_filters(ui, &all_books, &all_groups);
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
                    Event::Shelf(ShelfSnapshot { books, groups }) => {
                        *all_groups.borrow_mut() = groups;
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
                                ..Default::default()
                            })
                            .collect();
                        ui.set_total_books(count as i32);
                        show_filtered(&ui, &all_books.borrow(), &all_groups.borrow(), true);
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
                            // Folder thumbnails share the same asynchronously loaded covers.
                            show_filtered(&ui, &all, &all_groups.borrow(), false);
                        }
                    }
                    Event::SignedOut => {
                        all_books.borrow_mut().clear();
                        all_groups.borrow_mut().clear();
                        reset_groups(&ui);
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
    let cached = store.load_shelf_snapshot(&identity).await?;
    if !cached.books.is_empty() || !cached.groups.is_empty() {
        emit(Event::Shelf(cached.clone()));
        for book in &cached.books {
            if let Some(image) = weread::cached_cover(store, &identity, book).await {
                emit(Event::Cover {
                    id: book.id.clone(),
                    image: Some(image),
                });
            }
        }
    }
    emit(Event::Status("正在加载书架…".into()));
    let snapshot = match api.shelf_snapshot(old).await {
        Ok(books) => books,
        Err(error) if error.auth => {
            let new = api.refresh(old).await?;
            // Persist rotating refresh tokens before the shelf retry.
            store.rotate_session(&identity, &new).await?;
            *credentials = Some(new);
            api.shelf_snapshot(credentials.as_ref().unwrap()).await?
        }
        Err(error) => return Err(error),
    };
    store
        .apply_shelf_snapshot_with_groups(&identity, snapshot.clone(), None)
        .await?;
    let books = snapshot.books.clone();
    emit(Event::Shelf(snapshot));
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
fn bind_shelf_filters(
    ui: &AppWindow,
    all_books: &Rc<RefCell<Vec<ShelfRow>>>,
    all_groups: &Rc<RefCell<Vec<ShelfGroup>>>,
) {
    let weak = ui.as_weak();
    let searchable = all_books.clone();
    let searchable_groups = all_groups.clone();
    ui.on_search(move || {
        if let Some(ui) = weak.upgrade() {
            show_filtered(&ui, &searchable.borrow(), &searchable_groups.borrow(), true);
        }
    });
    let weak = ui.as_weak();
    let grouped_books = all_books.clone();
    let selectable_groups = all_groups.clone();
    let shelf_page = Cell::new(1);
    ui.on_select_group(move |key| {
        if let Some(ui) = weak.upgrade() {
            let returning = key == "all" && ui.get_selected_group() != "all";
            if key != "all" && ui.get_selected_group() == "all" {
                shelf_page.set(ui.get_page());
            }
            ui.set_selected_group(key);
            ui.set_query("".into());
            show_filtered(
                &ui,
                &grouped_books.borrow(),
                &selectable_groups.borrow(),
                true,
            );
            if returning {
                ui.set_page(shelf_page.get().clamp(1, ui.get_page_count()));
            }
        }
    });
}

fn reset_groups(ui: &AppWindow) {
    ui.set_selected_group("all".into());
    ui.set_group_name("书架".into());
}

fn shelf_rows(all: &[ShelfRow], groups: &[ShelfGroup]) -> Vec<ShelfRow> {
    let mut rows = Vec::new();
    let mut emitted = std::collections::HashSet::new();
    // Replace each group's first book with its folder, keeping ungrouped books
    // in shelf order. Empty groups follow the books in server group order.
    for book in all {
        let mut grouped = false;
        for group in groups {
            if group.book_ids.iter().any(|id| id == book.book_id.as_str()) {
                grouped = true;
                if emitted.insert(group.id.as_str()) {
                    rows.push(folder_row(all, groups, group));
                }
            }
        }
        if !grouped {
            rows.push(book.clone());
        }
    }
    for group in groups {
        if emitted.insert(group.id.as_str()) {
            rows.push(folder_row(all, groups, group));
        }
    }
    rows
}

fn folder_row(all: &[ShelfRow], groups: &[ShelfGroup], group: &ShelfGroup) -> ShelfRow {
    let key = format!("archive:{}", group.id);
    let books = group_books(all, groups, &key);
    let previews = books
        .iter()
        .take(4)
        .map(|book| ShelfPreview {
            cover: book.cover.clone(),
            cover_state: book.cover_state,
            title: book.title.clone(),
        })
        .collect::<Vec<_>>();
    ShelfRow {
        title: group.name.clone().into(),
        group_key: key.into(),
        group_count: books.len() as i32,
        group_covers: ModelRc::new(VecModel::from(previews)),
        ..Default::default()
    }
}

fn group_books(all: &[ShelfRow], groups: &[ShelfGroup], key: &str) -> Vec<ShelfRow> {
    let ids: std::collections::HashSet<&str> = groups
        .iter()
        .find(|g| key == format!("archive:{}", g.id))
        .into_iter()
        .flat_map(|g| g.book_ids.iter().map(String::as_str))
        .collect();
    all.iter()
        .filter(|book| key == "all" || ids.contains(book.book_id.as_str()))
        .cloned()
        .collect()
}

fn show_filtered(ui: &AppWindow, all: &[ShelfRow], groups: &[ShelfGroup], reset: bool) {
    let selected = ui.get_selected_group();
    let group = groups
        .iter()
        .find(|g| selected == format!("archive:{}", g.id));
    if selected != "all" && group.is_none() {
        ui.set_selected_group("all".into());
    }
    ui.set_group_name(group.map_or("书架", |g| g.name.as_str()).into());
    let query = ui.get_query();
    let rows = if ui.get_selected_group() == "all" {
        if query.trim().is_empty() {
            shelf_rows(all, groups)
        } else {
            filtered_rows(all, query.as_str())
        }
    } else {
        filtered_rows(
            &group_books(all, groups, ui.get_selected_group().as_str()),
            query.as_str(),
        )
    };
    ui.set_books(ModelRc::new(VecModel::from(rows)));
    ui.set_page(if reset {
        1
    } else {
        ui.get_page().clamp(1, ui.get_page_count())
    });
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
    use crate::weread::{
        tests::{credentials, temporary, Mock},
        Book,
    };
    use serde_json::json;
    use slint::Model;

    #[test]
    fn group_controls_reset_pagination_search_and_recover_from_removed_groups() {
        use slint::platform::{
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
            Platform, PlatformError, WindowAdapter,
        };
        struct TestPlatform(Rc<MinimalSoftwareWindow>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
                Ok(self.0.clone())
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
        window.set_size(slint::PhysicalSize::new(600, 800));
        let ui = AppWindow::new().unwrap();
        let books = Rc::new(RefCell::new(
            (0..25)
                .map(|i| {
                    let (width, height) = match i % 3 {
                        0 => (120, 170),
                        1 => (100, 250),
                        _ => (250, 100),
                    };
                    let mut cover =
                        slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(width, height);
                    let shade = 40 + i as u8 * 7;
                    cover.make_mut_slice().fill(slint::Rgb8Pixel::new(
                        shade,
                        shade + 5,
                        shade + 10,
                    ));
                    ShelfRow {
                        book_id: i.to_string().into(),
                        title: format!("书籍{i}").into(),
                        cover: slint::Image::from_rgb8(cover),
                        cover_state: 1,
                        ..Default::default()
                    }
                })
                .collect::<Vec<_>>(),
        ));
        let groups = Rc::new(RefCell::new(vec![
            ShelfGroup {
                id: "1".into(),
                name: "人民文学出版社".into(),
                book_ids: (5..18).map(|i| i.to_string()).collect(),
            },
            ShelfGroup {
                id: "2".into(),
                name: "空组".into(),
                book_ids: vec![],
            },
        ]));
        bind_shelf_filters(&ui, &books, &groups);
        show_filtered(&ui, &books.borrow(), &groups.borrow(), true);
        assert_eq!(ui.get_page_count(), 2);
        ui.set_logged_in(true);
        ui.set_total_books(25);
        ui.set_status("共 25 本书".into());
        window
            .renderer()
            .register_font_from_memory(crate::DEFAULT_FONT)
            .unwrap();
        ui.show().unwrap();
        let click = |x, y| {
            let position = slint::LogicalPosition::new(x, y);
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerPressed {
                    position,
                    button: slint::platform::PointerEventButton::Left,
                });
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                    position,
                    button: slint::platform::PointerEventButton::Left,
                });
        };
        for (width, height) in [(600, 800), (820, 1060)] {
            window.set_size(slint::PhysicalSize::new(width, height));
            window.request_redraw();
            let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
            slint::platform::update_timers_and_animations();
            assert!(window.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, width as usize);
            }));
            let rgb: Vec<u8> = pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
            image::save_buffer(
                format!("/tmp/rereader-groups-{width}.png"),
                &rgb,
                width,
                height,
                image::ColorType::Rgb8,
            )
            .unwrap();
            let mut bounds = Vec::new();
            for id in 5..9 {
                let shade = 40 + id as u8 * 7;
                let points: Vec<_> = pixels
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| (p.r, p.g, p.b) == (shade, shade + 5, shade + 10))
                    .map(|(i, _)| (i as u32 % width, i as u32 / width))
                    .collect();
                assert!(!points.is_empty(), "folder must show preview {id}");
                let left = points.iter().map(|p| p.0).min().unwrap();
                let right = points.iter().map(|p| p.0).max().unwrap();
                let top = points.iter().map(|p| p.1).min().unwrap();
                let bottom = points.iter().map(|p| p.1).max().unwrap();
                assert!(
                    right - left < width / 8 && bottom - top < height / 6,
                    "preview must stay within a quarter of the cover"
                );
                bounds.push((left, top, right, bottom));
            }
            assert!(bounds[0].2 < bounds[1].0 && bounds[2].2 < bounds[3].0);
            assert!(bounds[0].3 < bounds[2].1 && bounds[1].3 < bounds[3].1);
            // The folder is the second item in the second row, like the reference.
            click(width as f32 * 0.36, height as f32 * 0.48);
            assert_eq!(ui.get_selected_group(), "archive:1");
            window.request_redraw();
            assert!(window.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, width as usize);
            }));
            let rgb: Vec<u8> = pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
            image::save_buffer(
                format!("/tmp/rereader-group-open-{width}.png"),
                &rgb,
                width,
                height,
                image::ColorType::Rgb8,
            )
            .unwrap();
            click(width as f32 * 0.07, height as f32 * 0.12);
            assert_eq!(ui.get_selected_group(), "all");
        }
        ui.set_page(2);
        ui.invoke_select_group("archive:1".into());
        assert_eq!(ui.get_group_name(), "人民文学出版社");
        assert_eq!(ui.get_page(), 1);
        assert_eq!(ui.get_page_count(), 2);
        assert_eq!(ui.get_books().row_count(), 13);
        ui.set_query("书籍12".into());
        ui.invoke_search();
        assert_eq!(ui.get_books().row_count(), 1);
        ui.invoke_select_group("all".into());
        assert_eq!(ui.get_page(), 2);
        assert!(ui.get_query().is_empty());
        assert_eq!(ui.get_books().row_count(), 14);
        ui.invoke_select_group("archive:2".into());
        assert_eq!(ui.get_books().row_count(), 0);
        assert_eq!(ui.get_page_count(), 1);
        groups.borrow_mut().clear();
        show_filtered(&ui, &books.borrow(), &groups.borrow(), true);
        assert_eq!(ui.get_selected_group(), "all");
        assert_eq!(ui.get_books().row_count(), 25);
        reset_groups(&ui);
        assert_eq!(ui.get_group_name(), "书架");
        ui.hide().unwrap();
    }

    #[test]
    fn groups_filter_by_id_without_duplicates_and_search_within_the_group() {
        let rows = vec![
            ShelfRow {
                book_id: "a".into(),
                title: "文学一".into(),
                ..Default::default()
            },
            ShelfRow {
                book_id: "b".into(),
                title: "文学二".into(),
                ..Default::default()
            },
            ShelfRow {
                book_id: "c".into(),
                title: "科学".into(),
                ..Default::default()
            },
        ];
        let groups = vec![
            ShelfGroup {
                id: "1".into(),
                name: "同名组".into(),
                book_ids: vec!["b".into(), "a".into(), "a".into(), "missing".into()],
            },
            ShelfGroup {
                id: "2".into(),
                name: "同名组".into(),
                book_ids: vec![],
            },
        ];
        let grouped = group_books(&rows, &groups, "archive:1");
        assert_eq!(
            grouped
                .iter()
                .map(|b| b.book_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(filtered_rows(&grouped, "二").len(), 1);
        assert!(filtered_rows(&grouped, "科学").is_empty());
        assert!(group_books(&rows, &groups, "archive:2").is_empty());
        assert_eq!(group_books(&rows, &[], "all").len(), 3);
        let shelf = shelf_rows(&rows, &groups);
        assert_eq!(shelf.len(), 3);
        assert_eq!(shelf[0].group_key, "archive:1");
        assert_eq!(shelf[0].group_count, 2);
        assert_eq!(shelf[0].group_covers.row_count(), 2);
        assert_eq!(shelf[1].book_id, "c");
        assert_eq!(shelf[2].group_key, "archive:2");
        assert_eq!(shelf[2].group_count, 0);
    }

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
            (
                200,
                json!({"books":[{"bookId":"b","title":"Book"}], "archive":[{"archiveId":7,"name":"文学","bookIds":["b"]}]}),
            ),
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
            .any(|e| matches!(e,Event::Shelf(shelf) if shelf.books.len()==1 && shelf.groups.len()==1 && shelf.groups[0].id=="7")));
        let identity = store.identity().await.unwrap().unwrap();
        let snapshot = store.load_shelf_snapshot(&identity).await.unwrap();
        assert_eq!(snapshot.groups[0].book_ids, ["b"]);
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
            .any(|e| matches!(e, Event::Shelf(shelf) if shelf.books.is_empty())));
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
        let snapshot = ShelfSnapshot {
            books: vec![book.clone()],
            groups: vec![ShelfGroup {
                id: "offline-group".into(),
                name: "离线分组".into(),
                book_ids: vec![book.id.clone()],
            }],
        };
        store
            .apply_shelf_snapshot_with_groups(&identity, snapshot.clone(), None)
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
            .any(|e| matches!(e,Event::Shelf(shelf) if shelf==&snapshot)));
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
