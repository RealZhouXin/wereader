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

pub fn screenshot(path: &Path) -> Result<(), Box<dyn Error>> {
    let width = 600;
    let height = 800;
    let window = install(width, height)?;
    let ui = crate::create_ui()?;
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
    fn slint_ui_renders_and_updates_page() {
        let window = super::install(600, 800).unwrap();
        let ui = crate::create_ui().unwrap();
        window
            .renderer()
            .register_font_from_memory(crate::DEFAULT_FONT)
            .unwrap();
        ui.invoke_previous_page();
        assert_eq!(ui.get_page(), 1);
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
        ui.hide().unwrap();
    }

    #[test]
    fn grayscale_preserves_extremes() {
        assert_eq!(super::grayscale(0, 0, 0), 0);
        assert_eq!(super::grayscale(255, 255, 255), 255);
        assert_eq!(super::grayscale(128, 128, 128), 128);
    }
}
