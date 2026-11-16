-- Additive, preserving upgrade: recommendations are opt-in for existing products.
CREATE TABLE catalog_related_settings (
    product_id bigint PRIMARY KEY REFERENCES catalog_products(id) ON DELETE CASCADE,
    fallback text NOT NULL DEFAULT 'none' CHECK (fallback IN ('none', 'category', 'brand', 'either'))
);
CREATE TABLE catalog_related_products (
    product_id bigint NOT NULL REFERENCES catalog_products(id) ON DELETE CASCADE,
    related_id bigint NOT NULL REFERENCES catalog_products(id) ON DELETE CASCADE,
    position smallint NOT NULL CHECK (position BETWEEN 1 AND 12),
    PRIMARY KEY (product_id, related_id),
    UNIQUE (product_id, position),
    CHECK (product_id <> related_id)
);
CREATE INDEX catalog_related_products_target ON catalog_related_products(related_id);
