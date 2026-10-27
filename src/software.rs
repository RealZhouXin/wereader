use slint::{
    platform::{
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        Platform, PlatformError, WindowAdapter,
    },
    ComponentHandle,
};
use std::{error::Error, fs::File, io::Write, path::Path, rc::Rc};

pub struct SoftwarePlatform(pub Rc<MinimalSoftwareWindow>);

impl Platform for SoftwarePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

pub fn install(width: u32, height: u32) -> Result<Rc<MinimalSoftwareWindow>, Box<dyn Error>> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(SoftwarePlatform(window.clone())))?;
    window.set_size(slint::PhysicalSize::new(width, height));
    Ok(window)
}

pub fn grayscale(r: u8, g: u8, b: u8) -> u8 {
    ((u32::from(r) * 77 + u32::from(g) * 150 + u32::from(b) * 29 + 128) >> 8) as u8
}

pub fn screenshot(path: &Path, font: Option<&Path>) -> Result<(), Box<dyn Error>> {
    let width = 600;
    let height = 800;
    let window = install(width, height)?;
    let ui = crate::create_ui()?;
    if let Some(font) = font {
        crate::register_font(font)?;
    }
    window
        .renderer()
        .register_font_from_memory(crate::DEFAULT_FONT)?;
    ui.show()?;
    let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
    slint::platform::update_timers_and_animations();
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, width as usize);
    });
    let mut file = File::create(path)?;
    write!(file, "P5\n{width} {height}\n255\n")?;
    let gray: Vec<_> = pixels.iter().map(|p| grayscale(p.r, p.g, p.b)).collect();
    file.write_all(&gray)?;
    ui.hide()?;
    println!("Slint preview saved to {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use slint::platform::WindowAdapter;
    use slint::ComponentHandle;

    #[test]
    fn slint_ui_renders_shelf_qr_and_bounds_pagination() {
        let window = super::install(600, 800).unwrap();
        let ui = crate::create_ui().unwrap();
        window
            .renderer()
            .register_font_from_memory(crate::DEFAULT_FONT)
            .unwrap();
        ui.set_logged_in(true);
        ui.set_status("共 13 本书".into());
        ui.set_total_books(13);
        ui.set_books(slint::ModelRc::new(slint::VecModel::from(
            (0..13)
                .map(|i| crate::ShelfRow {
                    book_id: format!("book-{i}").into(),
                    title: [
                        "生死疲劳",
                        "克拉拉与太阳",
                        "宫水三叶的奇妙旅程",
                        "因子投资：方法与实践",
                        "春秋时代的历史",
                        "人民文学出版社",
                        "格罗威尔先生",
                        "无人生还",
                        "2001：太空漫游",
                        "嫌疑人的献身",
                        "斯泰尔斯庄园奇案",
                        "游戏设计的236个技巧",
                        "第十三本书",
                    ][i]
                        .into(),
                    author: "示例作者".into(),
                    ..Default::default()
                })
                .collect::<Vec<_>>(),
        )));
        assert_eq!(ui.get_page_count(), 2);
        ui.invoke_previous_page();
        assert_eq!(ui.get_page(), 1);
        ui.invoke_next_page();
        assert_eq!(ui.get_page(), 2);
        ui.invoke_next_page();
        assert_eq!(ui.get_page(), 2);
        ui.show().unwrap();
        let mut pixels = vec![slint::Rgb8Pixel::default(); 600 * 800];
        slint::platform::update_timers_and_animations();
        assert!(window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, 600);
        }));
        assert!(pixels.iter().any(|p| p.r < 50));
        assert!(pixels.iter().any(|p| p.r > 240));
        ui.set_page(1);
        let model = ui.get_books();
        use slint::Model;
        for index in 0..model.row_count() {
            let mut row = model.row_data(index).unwrap();
            let mut cover = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(120, 170);
            for (n, pixel) in cover.make_mut_slice().iter_mut().enumerate() {
                let x = n % 120;
                let y = n / 120;
                let shade = if (12..108).contains(&x) && (18..150).contains(&y) {
                    35 + (index as u8 * 15)
                } else {
                    235
                };
                *pixel = slint::Rgb8Pixel::new(shade, shade, shade);
            }
            row.cover = slint::Image::from_rgb8(cover);
            row.cover_state = 1;
            row.finished = index == 11;
            model.set_row_data(index, row);
        }
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, 600);
        });
        let rgb: Vec<u8> = pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
        image::save_buffer(
            "/tmp/rereader-shelf-test.png",
            &rgb,
            600,
            800,
            image::ColorType::Rgb8,
        )
        .unwrap();
        // Check the same adaptive layout at the prototype's native dimensions.
        window.set_size(slint::PhysicalSize::new(820, 1060));
        window.request_redraw();
        let mut native = vec![slint::Rgb8Pixel::default(); 820 * 1060];
        assert!(window.draw_if_needed(|renderer| {
            renderer.render(&mut native, 820);
        }));
        let rgb: Vec<u8> = native.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
        image::save_buffer(
            "/tmp/rereader-shelf-native-test.png",
            &rgb,
            820,
            1060,
            image::ColorType::Rgb8,
        )
        .unwrap();
        window.set_size(slint::PhysicalSize::new(600, 800));

        // Exercise the bottom navigation through actual pointer hit areas.
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
        click(217.0, 774.0);
        assert!(
            ui.get_account_open(),
            "profile navigation must open the account panel"
        );
        ui.set_account("账号 42".into());
        ui.set_reading_time("1234 小时 56 分钟".into());
        ui.set_weekly_reading_time("12 小时 30 分钟".into());
        window.request_redraw();
        assert!(window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, 600);
        }));
        let rgb: Vec<u8> = pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
        image::save_buffer(
            "/tmp/rereader-account-test.png",
            &rgb,
            600,
            800,
            image::ColorType::Rgb8,
        )
        .unwrap();
        ui.set_query("莫言".into());
        click(130.0, 774.0);
        assert!(
            !ui.get_account_open(),
            "shelf navigation must close the account panel"
        );
        assert!(
            ui.get_query().is_empty(),
            "shelf navigation must clear search"
        );

        // Regression: tall, wide and ordinary covers must never overlap the
        // caption. Solid blue isolates cover bounds from dark caption glyphs.
        for index in 0..model.row_count() {
            let mut row = model.row_data(index).unwrap();
            let (width, height) = match index % 3 {
                0 => (120, 170),
                1 => (100, 250),
                _ => (250, 100),
            };
            let mut cover = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(width, height);
            cover
                .make_mut_slice()
                .fill(slint::Rgb8Pixel::new(43, 97, 173));
            row.cover = slint::Image::from_rgb8(cover);
            model.set_row_data(index, row);
        }
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, 600);
        });
        let is_cover = |x: usize, y: usize| {
            let p = pixels[y * 600 + x];
            (p.r, p.g, p.b) == (43, 97, 173)
        };
        let mut bands = Vec::new();
        let mut y = 0usize;
        while y < 800 {
            if !is_cover(87, y) {
                y += 1;
                continue;
            }
            let top = y;
            while y < 800 && is_cover(87, y) {
                y += 1;
            }
            bands.push((top, y));
        }
        assert_eq!(bands.len(), 3, "expected three cover rows");
        for (top, bottom) in bands {
            let middle = (top + bottom) / 2;
            let left = (0..87).find(|&x| is_cover(x, middle)).unwrap();
            let right = (87..160).find(|&x| !is_cover(x, middle)).unwrap();
            for y in top..bottom {
                for x in left..right {
                    let p = pixels[y * 600 + x];
                    assert!(
                        !(p.r < 80 && p.g < 80 && p.b < 80),
                        "caption overlaps cover at {x},{y}"
                    );
                }
            }
        }
        ui.set_logged_in(false);
        ui.set_busy(true);
        ui.set_status("请使用微信扫码，在手机上确认登录".into());
        ui.set_qr_image(
            crate::app::qr_image("https://open.weixin.qq.com/connect/confirm?uuid=test").unwrap(),
        );
        ui.set_has_qr(true);
        assert!(window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, 600);
        }));
        let rgb: Vec<u8> = pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
        image::save_buffer(
            "/tmp/rereader-qr-test.png",
            &rgb,
            600,
            800,
            image::ColorType::Rgb8,
        )
        .unwrap();
        ui.hide().unwrap();
    }

    #[test]
    fn grayscale_preserves_extremes() {
        assert_eq!(super::grayscale(0, 0, 0), 0);
        assert_eq!(super::grayscale(255, 255, 255), 255);
        assert_eq!(super::grayscale(128, 128, 128), 128);
    }
}
