-- Preserve the legacy method, its identity, configuration and all order references.
ALTER TABLE payment_methods ADD COLUMN provider_id text;
ALTER TABLE payment_methods DROP CONSTRAINT payment_methods_code_key;
CREATE UNIQUE INDEX payment_methods_builtin_key ON payment_methods(code) WHERE code <> 'online_checkout';
CREATE UNIQUE INDEX payment_methods_provider_key ON payment_methods(provider_id) WHERE provider_id IS NOT NULL;
CREATE UNIQUE INDEX payment_methods_legacy_key ON payment_methods(code) WHERE code = 'online_checkout' AND provider_id IS NULL;
ALTER TABLE payment_methods ADD CONSTRAINT payment_method_provider_check CHECK (
 (provider_id IS NULL) OR (code = 'online_checkout' AND provider_id ~ '^[a-z][a-z0-9-]{0,63}$'));
-- Historical attempts retain their own provider identity. They do not identify
-- the currently installed module: a store may have switched providers over time.
-- Leave the legacy method unbound until the host registers its installed module.
CREATE OR REPLACE FUNCTION payment_methods_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(5426645964561798229);
 IF TG_OP='UPDATE' AND (OLD.code<>NEW.code OR
     (OLD.provider_id IS NOT NULL AND OLD.provider_id IS DISTINCT FROM NEW.provider_id)) THEN
  RAISE EXCEPTION 'Payment implementation is immutable' USING ERRCODE='23514',CONSTRAINT='payment_method_code_immutable';
 END IF;
 RETURN NEW;
END; $$;
