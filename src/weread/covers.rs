use super::{Api, Error, Result, Store};
use image::{ImageReader, Limits};
use sha2::{Digest, Sha256};
use std::{io::Cursor, path::PathBuf, time::Duration};

pub struct Cover {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub fn cover_url(raw: &str) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(raw).map_err(|_| Error::new("Invalid cover URL."))?;
    let host = url.host_str().unwrap_or("");
    let allowed = ["myqcloud.com", "qpic.cn", "qlogo.cn", "weread.qq.com"]
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));
    if !allowed || !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
        return Err(Error::new("Unsupported cover host."));
    }
    if url.scheme() == "http" {
        let _ = url.set_scheme("https");
    }
    if url.scheme() != "https" {
        return Err(Error::new("Cover URL must use HTTPS."));
    }
    Ok(url)
}
fn decode(bytes: &[u8]) -> Result<image::DynamicImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| Error::new("Invalid cover image."))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|_| Error::new("Could not decode cover image."))
}
fn pixels(image: image::DynamicImage) -> Cover {
    let image = thumbnail(image).to_rgba8();
    Cover {
        width: image.width(),
        height: image.height(),
        pixels: image.into_raw(),
    }
}
fn thumbnail(image: image::DynamicImage) -> image::DynamicImage {
    image
        .thumbnail(image.width().min(200), image.height().min(300))
        .grayscale()
}
impl Store {
    fn cover_path(&self, account: &str, url: &str) -> PathBuf {
        let key = format!(
            "{:x}",
            Sha256::digest(format!("cover-v1:{account}:{url}").as_bytes())
        );
        self.directory.join("covers").join(format!("{key}.png"))
    }
}
impl Api {
    pub async fn cover(&self, store: &Store, account: &str, raw_url: &str) -> Result<Cover> {
        let url = cover_url(raw_url)?;
        let path = store.cover_path(account, url.as_str());
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() <= 1024 * 1024 {
                if let Ok(bytes) = std::fs::read(&path) {
                    if let Ok(image) = decode(&bytes) {
                        return Ok(pixels(image));
                    }
                }
            }
            let _ = std::fs::remove_file(&path);
        }
        // Credentials are attached only to API requests, never to cover requests.
        let mut response = self
            .http
            .get(url)
            .timeout(Duration::from_secs(12))
            .send()
            .await
            .map_err(|_| Error::new("Cover request failed."))?;
        if !response.status().is_success() {
            return Err(Error::new("Cover is unavailable."));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::new("Cover response failed."))?
        {
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err(Error::new("Cover image is too large."));
            }
            bytes.extend_from_slice(&chunk);
        }
        let image = thumbnail(decode(&bytes)?);
        // Cache failures never prevent the current cover from being displayed.
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_ok() {
                let temporary = path.with_extension("part");
                if image
                    .save_with_format(&temporary, image::ImageFormat::Png)
                    .is_ok()
                {
                    let _ = std::fs::rename(&temporary, &path);
                }
                let _ = std::fs::remove_file(&temporary);
            }
        }
        Ok(pixels(image))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cover_hosts_are_validated_and_http_is_upgraded() {
        assert_eq!(
            cover_url("http://wfqqreader-1252317822.image.myqcloud.com/cover/a.jpg")
                .unwrap()
                .scheme(),
            "https"
        );
        for url in [
            "file:///etc/passwd",
            "https://localhost/cover",
            "https://qpic.cn.evil.test/x",
            "https://user:pass@qpic.cn/x",
            "https://qpic.cn:8443/x",
        ] {
            assert!(cover_url(url).is_err());
        }
    }
    #[test]
    fn cache_is_account_scoped_and_invalid_images_are_rejected() {
        let store = Store::new(PathBuf::from("cache"));
        assert_ne!(
            store.cover_path("one", "https://qpic.cn/a"),
            store.cover_path("two", "https://qpic.cn/a")
        );
        assert!(decode(b"<html>error</html>").is_err());
        let cover = pixels(image::DynamicImage::new_rgb8(500, 800));
        assert!(cover.width <= 200 && cover.height <= 300);
        assert_eq!(
            cover.pixels.len(),
            (cover.width * cover.height * 4) as usize
        );
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[tokio::test]
    async fn cached_cover_loads_without_network() {
        let directory = crate::weread::tests::temporary();
        let store = Store::new(directory.clone());
        let url = "https://qpic.cn/offline-test.png";
        let path = store.cover_path("test", url);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::DynamicImage::new_rgb8(120, 180)
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        let cover = Api::new()
            .unwrap()
            .cover(&store, "test", url)
            .await
            .unwrap();
        assert_eq!((cover.width, cover.height), (120, 180));
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    #[ignore = "fetches an anonymous public cover from the official CDN"]
    async fn public_cover_service_probe() {
        let directory = crate::weread::tests::temporary();
        let store = Store::new(directory.clone());
        let url="https://cdn.weread.qq.com/weread/cover/49/yuewen_674073/t6_yuewen_6740731774011595.jpg";
        let cover = Api::new()
            .unwrap()
            .cover(&store, "anonymous", url)
            .await
            .unwrap();
        assert!(cover.width > 0 && cover.width <= 200 && cover.height > 0 && cover.height <= 300);
        assert!(store.cover_path("anonymous", url).exists());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
