//! Integration scenarios grouped for proportional compilation; PostgreSQL stays serial.
include!("identity/shared.rs");

#[path = "identity/attributes.rs"]
mod attributes;

#[path = "identity/brand_store.rs"]
mod brand_store;

#[path = "identity/bulk_category.rs"]
mod bulk_category;

#[path = "identity/catalog_pagination.rs"]
mod catalog_pagination;

#[path = "identity/category_store.rs"]
mod category_store;

#[path = "identity/content_localization.rs"]
mod content_localization;

#[path = "identity/demo.rs"]
mod demo;

#[path = "identity/error_pages.rs"]
mod error_pages;

#[path = "identity/gallery.rs"]
mod gallery;

#[path = "identity/library.rs"]
mod library;

#[path = "identity/media.rs"]
mod media;

#[path = "identity/media_consolidation.rs"]
mod media_consolidation;

#[path = "identity/media_files.rs"]
mod media_files;

#[path = "identity/media_simplification.rs"]
mod media_simplification;

#[path = "identity/media_tree.rs"]
mod media_tree;

#[path = "identity/optional_category.rs"]
mod optional_category;

#[path = "identity/product_bulk.rs"]
mod product_bulk;

#[path = "identity/products.rs"]
mod products;

#[path = "identity/search_store.rs"]
mod search_store;

#[path = "identity/shared_pagination.rs"]
mod shared_pagination;

#[path = "identity/taxonomy.rs"]
mod taxonomy;

#[path = "identity/variants.rs"]
mod variants;
