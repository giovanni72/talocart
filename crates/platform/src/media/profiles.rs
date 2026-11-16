//! One profile registry for sizing, rendering, URL parsing and background maintenance.
use image::{DynamicImage, imageops::FilterType};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageProfile {
    Original,
    Thumbnail,
    Card,
    Detail,
    Large,
    Logo,
    Favicon,
    Avatar,
}
impl ImageProfile {
    pub const STANDARD: [Self; 4] = [Self::Thumbnail, Self::Card, Self::Detail, Self::Large];
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "original" => Self::Original,
            "thumbnail" => Self::Thumbnail,
            "card" => Self::Card,
            "detail" => Self::Detail,
            "large" => Self::Large,
            "logo" => Self::Logo,
            "favicon" => Self::Favicon,
            "avatar" => Self::Avatar,
            _ => return None,
        })
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Thumbnail => "thumbnail",
            Self::Card => "card",
            Self::Detail => "detail",
            Self::Large => "large",
            Self::Logo => "logo",
            Self::Favicon => "favicon",
            Self::Avatar => "avatar",
        }
    }
    pub fn edge(self) -> u32 {
        match self {
            Self::Original => 0,
            Self::Thumbnail => 128,
            Self::Card => 512,
            Self::Detail => 1024,
            Self::Large | Self::Logo => 2048,
            Self::Favicon => 64,
            Self::Avatar => 256,
        }
    }
    pub fn attachment(self) -> bool {
        matches!(self, Self::Logo | Self::Favicon | Self::Avatar)
    }
    pub(crate) fn render(self, decoded: &DynamicImage) -> DynamicImage {
        match self {
            Self::Original => decoded.clone(),
            Self::Avatar => decoded.resize_to_fill(256, 256, FilterType::Lanczos3),
            Self::Favicon => {
                let fit = fit(decoded, 64, 64).to_rgba8();
                let mut square = image::RgbaImage::new(64, 64);
                image::imageops::overlay(
                    &mut square,
                    &fit,
                    i64::from((64 - fit.width()) / 2),
                    i64::from((64 - fit.height()) / 2),
                );
                DynamicImage::ImageRgba8(square)
            }
            Self::Logo => fit(decoded, 2048, 1024),
            _ => fit(decoded, self.edge(), self.edge()),
        }
    }
}
fn fit(image: &DynamicImage, width: u32, height: u32) -> DynamicImage {
    if image.width() <= width && image.height() <= height {
        image.clone()
    } else {
        image.thumbnail(width, height)
    }
}
impl From<super::ImageVariant> for ImageProfile {
    fn from(value: super::ImageVariant) -> Self {
        match value {
            super::ImageVariant::Original => Self::Original,
            super::ImageVariant::Thumbnail => Self::Thumbnail,
            super::ImageVariant::Card => Self::Card,
            super::ImageVariant::Detail => Self::Detail,
        }
    }
}
impl From<super::AttachmentProfile> for ImageProfile {
    fn from(value: super::AttachmentProfile) -> Self {
        match value {
            super::AttachmentProfile::Logo => Self::Logo,
            super::AttachmentProfile::Favicon => Self::Favicon,
            super::AttachmentProfile::Avatar => Self::Avatar,
        }
    }
}
