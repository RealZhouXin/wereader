use super::{Api, Book, Error, Result, Store};
use crate::storage::{FileKind, FileResource, Identity};
use image::{ImageReader, Limits};
use sha2::{Digest, Sha256};
use std::{io::Cursor, time::Duration};

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
pub(crate) fn cover_resource(book: &Book, url: &str) -> FileResource {
    FileResource {
        book_id: book.id.clone(),
        chapter_uid: None,
        kind: FileKind::Cover,
        resource_key: format!("{:x}", Sha256::digest(url.as_bytes())),
        content_version: None,
        variant: "gray-200x300-v1".into(),
        media_type: "image/png".into(),
        pinned: false,
    }
}
pub async fn cached_cover(store: &Store, identity: &Identity, book: &Book) -> Option<Cover> {
    let url = cover_url(&book.cover).ok()?;
    let bytes = store
        .read_cached_file(identity, cover_resource(book, url.as_str()))
        .await
        .ok()??;
    decode(&bytes).ok().map(pixels)
}
impl Api {
    pub async fn cover(&self, store: &Store, identity: &Identity, book: &Book) -> Result<Cover> {
        let url = cover_url(&book.cover)?;
        if let Some(image) = cached_cover(store, identity, book).await {
            return Ok(image);
        }
        let resource = cover_resource(book, url.as_str());
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
        // A failed cache commit does not prevent displaying this response.
        let mut png = Cursor::new(Vec::new());
        if image.write_to(&mut png, image::ImageFormat::Png).is_ok() {
            let _ = store
                .commit_cached_file(identity, resource, png.into_inner())
                .await;
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
    fn invalid_images_are_rejected_and_thumbnails_are_bounded() {
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
        let store = Store::new(directory.clone()).unwrap();
        let identity = store
            .save(&crate::weread::tests::credentials())
            .await
            .unwrap();
        let book = Book {
            id: "b".into(),
            title: "Book".into(),
            author: String::new(),
            cover: "https://qpic.cn/offline-test.png".into(),
            finished: false,
        };
        store
            .apply_shelf_snapshot(&identity, vec![book.clone()], None)
            .await
            .unwrap();
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(120, 180)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        store
            .commit_cached_file(
                &identity,
                cover_resource(&book, &book.cover),
                bytes.into_inner(),
            )
            .await
            .unwrap();
        let cover = Api::new()
            .unwrap()
            .cover(&store, &identity, &book)
            .await
            .unwrap();
        assert_eq!((cover.width, cover.height), (120, 180));
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
mod online_tests {
    use super::*;
    #[tokio::test]
    #[ignore = "fetches an anonymous public cover from the official CDN"]
    async fn public_cover_service_probe() {
        let directory = crate::weread::tests::temporary();
        let store = Store::new(directory.clone()).unwrap();
        let identity = store
            .save(&crate::weread::tests::credentials())
            .await
            .unwrap();
        let book=Book { id:"public-cover".into(),title:"Public cover".into(),author:String::new(),finished:false,
            cover:"https://cdn.weread.qq.com/weread/cover/49/yuewen_674073/t6_yuewen_6740731774011595.jpg".into() };
        store
            .apply_shelf_snapshot(&identity, vec![book.clone()], None)
            .await
            .unwrap();
        let cover = Api::new()
            .unwrap()
            .cover(&store, &identity, &book)
            .await
            .unwrap();
        assert!(cover.width > 0 && cover.width <= 200 && cover.height > 0 && cover.height <= 300);
        assert!(store
            .read_cached_file(&identity, cover_resource(&book, &book.cover))
            .await
            .unwrap()
            .is_some());
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
