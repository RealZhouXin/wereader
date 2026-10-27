pub mod storage;
pub mod weread;

pub const DEFAULT_FONT: &[u8] = include_bytes!("../assets/NotoSansSC-Regular.otf");

pub mod app;
slint::include_modules!();
