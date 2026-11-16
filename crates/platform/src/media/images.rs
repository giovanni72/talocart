//! Shared bounded raster decoder. Callers translate invalid data to their public error.
impl super::MediaStore {
    /// Product photo profile retains portrait/landscape proportions and transparency.
    pub async fn prepare_product(
        &self,
        raw: Vec<u8>,
    ) -> Result<super::PreparedImage, super::MediaError> {
        self.prepare_raster(raw, super::ImageProfile::Large).await
    }
    /// Shared rectangular public logo profile; validation and encoding run outside Tokio.
    pub async fn prepare_logo(
        &self,
        raw: Vec<u8>,
    ) -> Result<super::PreparedImage, super::MediaError> {
        self.prepare_raster(raw, super::ImageProfile::Logo).await
    }
    async fn prepare_raster(
        &self,
        raw: Vec<u8>,
        profile: super::ImageProfile,
    ) -> Result<super::PreparedImage, super::MediaError> {
        let permit = self
            .image_slots()
            .try_acquire_owned()
            .map_err(|_| super::MediaError::Busy)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let decoded = decode(&raw, 10 * 1024 * 1024, 8192, 128 * 1024 * 1024)
                .map_err(|_| super::MediaError::Invalid)?;
            let fitted = profile.render(&decoded);
            let mut output = std::io::Cursor::new(Vec::new());
            fitted
                .write_to(&mut output, image::ImageFormat::Png)
                .map_err(|_| super::MediaError::Invalid)?;
            // Ordinary sources use the background rendition generator, never duplicate PNG copies.
            Ok(super::PreparedImage::source(
                output.into_inner(),
                fitted.width(),
                fitted.height(),
            ))
        })
        .await
        .map_err(|_| super::MediaError::Storage)?
    }
}
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use std::io::Cursor;

pub(crate) fn decode(
    raw: &[u8],
    upload_limit: usize,
    dimension: u32,
    decoded_limit: u64,
) -> std::result::Result<DynamicImage, ()> {
    if raw.is_empty() || raw.len() > upload_limit {
        return Err(());
    }
    let mut reader = ImageReader::new(Cursor::new(raw))
        .with_guessed_format()
        .map_err(|_| ())?;
    if !matches!(
        reader.format(),
        Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP)
    ) {
        return Err(());
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(dimension);
    limits.max_image_height = Some(dimension);
    limits.max_alloc = Some(decoded_limit);
    let mut decoder: Box<dyn ImageDecoder> = if reader.format() == Some(ImageFormat::WebP) {
        let mut webp = image::codecs::webp::WebPDecoder::new(Cursor::new(raw)).map_err(|_| ())?;
        // Store images are static; never silently discard animation frames.
        if webp.has_animation() {
            return Err(());
        }
        webp.set_limits(limits).map_err(|_| ())?;
        Box::new(webp)
    } else {
        reader.limits(limits);
        Box::new(reader.into_decoder().map_err(|_| ())?)
    };
    // from_decoder does not itself enforce max_alloc: bound its output before allocating.
    if decoder.total_bytes() > decoded_limit {
        return Err(());
    }
    let orientation = decoder.orientation().map_err(|_| ())?;
    let mut decoded = image::DynamicImage::from_decoder(decoder).map_err(|_| ())?;
    decoded.apply_orientation(orientation);
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    const LOSSLESS: &[u8] = include_bytes!("../../../web/tests/fixtures/webp-lossless.webp");
    const LOSSY: &[u8] = include_bytes!("../../../web/tests/fixtures/webp-lossy.webp");
    #[test]
    fn webp_static_lossless_and_lossy_preserve_dimensions_and_transparency() {
        for raw in [LOSSLESS, LOSSY] {
            let image = decode(raw, 1024 * 1024, 8192, 128 * 1024 * 1024)
                .unwrap()
                .to_rgba8();
            assert_eq!(image.dimensions(), (512, 128));
            assert_eq!(image.get_pixel(0, 0).0[3], 0);
            assert_eq!(image.get_pixel(256, 64).0[3], 128);
            assert!(decode(raw, raw.len() - 1, 8192, 128 * 1024 * 1024).is_err());
            assert!(decode(raw, 1024 * 1024, 511, 128 * 1024 * 1024).is_err());
            assert!(decode(raw, 1024 * 1024, 8192, 512 * 128 * 4 - 1).is_err());
        }
    }
    #[test]
    fn webp_corrupt_and_animated_are_rejected() {
        let animated = include_bytes!("../../../web/tests/fixtures/webp-animated.webp");
        assert!(
            image::codecs::webp::WebPDecoder::new(Cursor::new(animated))
                .unwrap()
                .has_animation()
        );
        for raw in [
            b"RIFF\0\0\0\0WEBP".as_slice(),
            &LOSSLESS[..20],
            &LOSSY[..LOSSY.len() / 2],
            animated.as_slice(),
        ] {
            assert!(decode(raw, 1024 * 1024, 8192, 128 * 1024 * 1024).is_err());
        }
    }
}
