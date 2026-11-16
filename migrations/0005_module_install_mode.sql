-- Keep the configured server default until an owner explicitly chooses a mode.
ALTER TABLE store_settings ADD COLUMN module_install_mode text
 CHECK (module_install_mode IN ('cli','panel'));
