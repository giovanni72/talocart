-- Preserve the existing guest checkout policy on supported upgrades.
ALTER TABLE customer_registration_settings
    ADD COLUMN allow_guest_checkout boolean NOT NULL DEFAULT true;
