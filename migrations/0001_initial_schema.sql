-- Initial base schema. New installations only; no legacy upgrade path.
SET LOCAL check_function_bodies = false;
CREATE FUNCTION advance_stock_revision() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 NEW.stock_revision := OLD.stock_revision + CASE
   WHEN NEW.stock IS DISTINCT FROM OLD.stock
     OR (to_jsonb(NEW)->'manage_stock') IS DISTINCT FROM (to_jsonb(OLD)->'manage_stock')
   THEN 1 ELSE 0 END;
 RETURN NEW;
END; $$;

CREATE FUNCTION catalog_attribute_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(5426645964561798229);
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 IF TG_OP='UPDATE' THEN
  IF TG_TABLE_NAME='catalog_product_attributes' THEN
   IF NEW.product_id<>OLD.product_id OR NEW.attribute_id<>OLD.attribute_id THEN
    RAISE EXCEPTION 'Immutable attribute assignment' USING ERRCODE='23514',CONSTRAINT='catalog_attribute_owner';
   END IF;
  END IF;
  NEW.revision := OLD.revision+1;
 ELSE
  IF (TG_TABLE_NAME='catalog_attribute_groups' AND (SELECT count(*) FROM catalog_attribute_groups)>=100)
   OR (TG_TABLE_NAME='catalog_attributes' AND (SELECT count(*) FROM catalog_attributes)>=500) THEN
   RAISE EXCEPTION 'Attribute limit' USING ERRCODE='23514',CONSTRAINT='catalog_attribute_limit';
  END IF;
  IF TG_TABLE_NAME='catalog_product_attributes' THEN
   IF (SELECT count(*) FROM catalog_product_attributes WHERE product_id=NEW.product_id)>=100 THEN
    RAISE EXCEPTION 'Product attribute limit' USING ERRCODE='23514',CONSTRAINT='catalog_attribute_limit';
   END IF;
  END IF;
 END IF;
 RETURN NEW;
END $$;

CREATE FUNCTION catalog_category_hierarchy() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE ancestor BIGINT; next_parent BIGINT; levels INTEGER:=1; subtree_depth INTEGER;
BEGIN
    PERFORM pg_advisory_xact_lock(5426645964561798228);
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    IF TG_OP='INSERT' AND (SELECT count(*) FROM catalog_categories)>=1000 THEN
        RAISE EXCEPTION 'Category limit' USING ERRCODE='23514', CONSTRAINT='catalog_categories_limit';
    END IF;
    ancestor:=NEW.parent_id;
    WHILE ancestor IS NOT NULL LOOP
        IF ancestor=NEW.id OR levels>=16 THEN
            RAISE EXCEPTION 'Invalid category hierarchy' USING ERRCODE='23514', CONSTRAINT='catalog_categories_hierarchy';
        END IF;
        SELECT parent_id INTO next_parent FROM catalog_categories WHERE id=ancestor;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'Missing parent' USING ERRCODE='23503', CONSTRAINT='catalog_categories_parent_id_fkey';
        END IF;
        ancestor:=next_parent; levels:=levels+1;
    END LOOP;
    WITH RECURSIVE descendants AS (
        SELECT id,1 AS depth FROM catalog_categories WHERE parent_id=NEW.id
        UNION ALL SELECT c.id,d.depth+1 FROM catalog_categories c JOIN descendants d ON c.parent_id=d.id
    ) SELECT coalesce(max(depth),0) INTO subtree_depth FROM descendants;
    IF levels+subtree_depth>16 THEN
        RAISE EXCEPTION 'Category depth' USING ERRCODE='23514', CONSTRAINT='catalog_categories_hierarchy';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION catalog_option_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(5426645964561798229);
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 IF TG_TABLE_NAME='catalog_option_values' THEN
  IF TG_OP='UPDATE' AND NEW.option_id<>OLD.option_id THEN
   RAISE EXCEPTION 'Immutable option owner' USING ERRCODE='23514',CONSTRAINT='catalog_option_values_owner';
  END IF;
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION checkout_country_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(5426645964561798229);
 PERFORM pg_advisory_xact_lock(5426645964561798230);
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION coupon_currency_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF OLD.currency<>NEW.currency AND EXISTS(SELECT 1 FROM coupons WHERE kind='fixed') THEN
  RAISE EXCEPTION 'Fixed coupons prevent currency conversion' USING ERRCODE='23514',CONSTRAINT='store_currency_catalog';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION coupon_use_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE c coupons%ROWTYPE;
BEGIN
 SELECT * INTO c FROM coupons WHERE id=NEW.coupon_id FOR UPDATE;
 IF NOT FOUND OR NOT c.active OR
    (c.usage_limit IS NOT NULL AND (SELECT count(*) FROM coupon_uses WHERE coupon_id=c.id)>=c.usage_limit) OR
    (c.expires_on IS NOT NULL AND c.expires_on < (statement_timestamp() AT TIME ZONE (SELECT time_zone FROM platform_regional_settings WHERE singleton))::date) THEN
  RAISE EXCEPTION 'Coupon unavailable' USING ERRCODE='23514',CONSTRAINT='coupon_unavailable';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION currency_catalog_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(5426645964561798229);
 IF OLD.currency<>NEW.currency AND (EXISTS(SELECT 1 FROM catalog_products) OR EXISTS(SELECT 1 FROM payment_methods WHERE minimum_cents>0 OR (fee_kind='fixed' AND fee_value>0)) OR EXISTS(SELECT 1 FROM shipping_methods WHERE free_from_cents>0 OR (fee_kind='fixed' AND fee_value>0))) THEN
  RAISE EXCEPTION 'Existing monetary values lock currency' USING ERRCODE='23514',CONSTRAINT='store_currency_catalog';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION customer_address_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF TG_OP='INSERT' THEN
  PERFORM id FROM customers WHERE id=NEW.customer_id FOR UPDATE;
  IF (SELECT count(*) FROM customer_addresses WHERE customer_id=NEW.customer_id)>=20 THEN
   RAISE EXCEPTION 'Customer address limit' USING ERRCODE='23514',CONSTRAINT='customer_address_limit';
  END IF;
 ELSIF ROW(NEW.id,NEW.customer_id) IS DISTINCT FROM ROW(OLD.id,OLD.customer_id)
       OR NEW.revision<>OLD.revision+1 THEN
  RAISE EXCEPTION 'Immutable address owner' USING ERRCODE='23514',CONSTRAINT='customer_address_owner';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION customer_billing_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF NEW.customer_id<>OLD.customer_id OR NEW.revision<=OLD.revision THEN
  RAISE EXCEPTION 'Immutable billing owner / invalid revision' USING ERRCODE='23514',CONSTRAINT='customer_billing_owner';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION customer_confirmation_invalidate() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 DELETE FROM customer_email_confirmations WHERE customer_id=OLD.id;
 RETURN NEW;
END; $$;

CREATE FUNCTION customer_default_group() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF NEW.group_id IS NULL THEN
  SELECT default_group_id INTO STRICT NEW.group_id FROM customer_group_configuration WHERE id=1 FOR SHARE;
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION customer_field_policy_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF TG_OP='DELETE' OR NEW.singleton IS DISTINCT FROM OLD.singleton OR NEW.revision<>OLD.revision+1 THEN
  RAISE EXCEPTION 'Field policy is required and revisioned' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION customer_field_policy_valid(value jsonb) RETURNS boolean
    LANGUAGE sql IMMUTABLE
    AS $$
 SELECT NOT EXISTS(SELECT 1 FROM jsonb_each_text(value) AS p(key,mode)
 WHERE split_part(key,'.',1) NOT IN ('private','business')
 OR key <> split_part(key,'.',1)||'.'||split_part(key,'.',2)
 OR split_part(key,'.',2) NOT IN ('email','first_name','last_name','phone','recipient_first_name','recipient_last_name','shipping_company','shipping_country','shipping_street','shipping_number','shipping_extra','shipping_postal','shipping_city','shipping_region','delivery_notes','billing_first_name','billing_last_name','billing_company','billing_country','billing_street','billing_number','billing_extra','billing_postal','billing_city','billing_region','vat_id','tax_id','recipient_code','pec')
 OR mode IS NULL OR mode NOT IN ('hidden','optional','required')
 OR (split_part(key,'.',2) IN ('email','shipping_country','billing_country') AND mode<>'required'));
$$;

CREATE FUNCTION customer_group_capacity_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(5426645964561798231);
 IF (SELECT count(*) FROM customer_groups)>=100 THEN
  RAISE EXCEPTION 'Customer group capacity' USING ERRCODE='23514',CONSTRAINT='customer_groups_capacity';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION customer_group_configuration_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF TG_OP='DELETE' OR NEW.id IS DISTINCT FROM OLD.id THEN
  RAISE EXCEPTION 'Customer group configuration is required' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION customer_group_quantity_schedule_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE target BIGINT;
BEGIN
 IF TG_TABLE_NAME='customer_groups' THEN target:=NEW.id;
 ELSIF TG_OP='DELETE' THEN target:=OLD.group_id;
 ELSE
  target:=NEW.group_id;
  IF TG_OP='UPDATE' AND OLD.group_id IS DISTINCT FROM NEW.group_id AND
     EXISTS(SELECT 1 FROM customer_groups WHERE id=OLD.group_id AND quantity_discounts_enabled) AND
     NOT EXISTS(SELECT 1 FROM customer_group_quantity_tiers WHERE group_id=OLD.group_id) THEN
   RAISE EXCEPTION 'An enabled quantity schedule requires a tier' USING ERRCODE='23514';
  END IF;
 END IF;
 IF EXISTS(SELECT 1 FROM customer_groups WHERE id=target AND quantity_discounts_enabled)
    AND NOT EXISTS(SELECT 1 FROM customer_group_quantity_tiers WHERE group_id=target) THEN
  RAISE EXCEPTION 'An enabled quantity schedule requires a tier' USING ERRCODE='23514';
 END IF;
 RETURN NULL;
END; $$;

CREATE FUNCTION guest_cart_limit() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 PERFORM id FROM guest_carts WHERE id=NEW.cart_id FOR UPDATE;
 IF (SELECT count(*) FROM guest_cart_items WHERE cart_id=NEW.cart_id AND id<>NEW.id)>=100 THEN
  RAISE EXCEPTION 'Cart item limit' USING ERRCODE='23514',CONSTRAINT='guest_cart_item_limit';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION information_page_reserve_slug() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 INSERT INTO information_page_slugs(slug,page_id) VALUES(NEW.slug,NEW.id)
 ON CONFLICT(slug) DO UPDATE SET page_id=EXCLUDED.page_id WHERE information_page_slugs.page_id=EXCLUDED.page_id;
 IF NOT FOUND THEN RAISE EXCEPTION 'Page slug already reserved' USING ERRCODE='23505'; END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION initialize_default_taxes() RETURNS void
    LANGUAGE plpgsql
    AS $$
DECLARE
    rate BIGINT;
    fiscal_class BIGINT;
    italy BIGINT;
    standard_class BIGINT;
    preset RECORD;
BEGIN
    PERFORM pg_advisory_xact_lock(5426645964561798229);
    -- Even partial configuration is merchant-owned. Never fill in or reinterpret it.
    IF EXISTS(SELECT 1 FROM tax_rates)
       OR EXISTS(SELECT 1 FROM tax_classes)
       OR EXISTS(SELECT 1 FROM tax_zones)
       OR EXISTS(SELECT 1 FROM pricing_settings WHERE default_class_id IS NOT NULL OR country_code IS NOT NULL)
    THEN
        RETURN;
    END IF;

    INSERT INTO tax_zones(name,description)
    VALUES ('Italia / Italy', 'Regole fiscali per Italia (IT). / Tax rules for Italy (IT).')
    RETURNING id INTO italy;
    INSERT INTO tax_zone_countries(country_code,zone_id) VALUES ('IT',italy);

    FOR preset IN SELECT * FROM (VALUES
        ('IVA / VAT 22% · IT',2200,'Aliquota ordinaria italiana. / Italian standard rate.'),
        ('IVA / VAT 10% · IT',1000,'Aliquota ridotta italiana. / Italian reduced rate.'),
        ('IVA / VAT 5% · IT',500,'Aliquota ridotta italiana. / Italian reduced rate.'),
        ('IVA / VAT 4% · IT',400,'Aliquota ridotta italiana. / Italian reduced rate.'),
        ('Esente IVA / VAT exempt 0%',0,'Esenzione IVA, senza riconoscimento automatico. / VAT exemption, no automatic eligibility checks.')
    ) AS defaults(name,basis_points,description)
    LOOP
        INSERT INTO tax_rates(name,description,basis_points)
        VALUES (preset.name,preset.description,preset.basis_points) RETURNING id INTO rate;
        INSERT INTO tax_classes(name,description,rate_id)
        VALUES (preset.name,preset.description,rate) RETURNING id INTO fiscal_class;
        INSERT INTO tax_zone_rules(zone_id,class_id,rate_id) VALUES (italy,fiscal_class,rate);
        IF preset.basis_points=2200 THEN standard_class=fiscal_class; END IF;
    END LOOP;
    UPDATE pricing_settings SET default_class_id=standard_class,country_code='IT' WHERE id=1;
END;
$$;

CREATE FUNCTION media_adopt_source(source text) RETURNS void
    LANGUAGE plpgsql
    AS $$
DECLARE location TEXT; parts TEXT[]; folder BIGINT; parent BIGINT; current_path TEXT=''; i INTEGER;
BEGIN
 IF NOT EXISTS(SELECT 1 FROM media_objects WHERE key=source AND public)
    OR EXISTS(SELECT 1 FROM media_variants WHERE object_key=source)
    OR EXISTS(SELECT 1 FROM media_renditions WHERE object_key=source AND source_key<>source)
    OR EXISTS(SELECT 1 FROM media_transforms WHERE object_key=source AND source_key<>source) THEN RETURN; END IF;
 SELECT path INTO location FROM media_locations WHERE key=source;
 parts=string_to_array(location,'/');
 IF array_length(parts,1)=1 THEN
  INSERT INTO media_folders(name,path,protected) VALUES('/','',true) ON CONFLICT(path) DO NOTHING;
  SELECT id INTO folder FROM media_folders WHERE path='';
 ELSE
  FOR i IN 1..array_length(parts,1)-1 LOOP
   current_path=CASE WHEN current_path='' THEN parts[i] ELSE current_path||'/'||parts[i] END;
   INSERT INTO media_folders(parent_id,name,path,protected) VALUES(parent,parts[i],current_path,i=1 AND parts[i] IN ('objects','brand','library','private','staging','cache')) ON CONFLICT(path) DO NOTHING;
   SELECT id INTO folder FROM media_folders WHERE path=current_path;
   parent=folder;
  END LOOP;
 END IF;
 INSERT INTO media_library(key,folder_id,name) VALUES(source,folder,parts[array_length(parts,1)]) ON CONFLICT(key) DO NOTHING;
END; $$;

CREATE FUNCTION media_location_insert() RETURNS trigger
    LANGUAGE plpgsql
    AS $$ BEGIN
 INSERT INTO media_locations VALUES(NEW.key,CASE WHEN position('/' in NEW.key)=0 THEN 'objects/'||NEW.key ELSE NEW.key END);
 RETURN NEW; END; $$;

CREATE FUNCTION media_reference_changed(old_key text, new_key text) RETURNS void
    LANGUAGE sql
    AS $$
    UPDATE media_objects SET unused_since=now()
    WHERE (key=old_key OR key IN (SELECT object_key FROM media_variants WHERE source_key=old_key))
      AND NOT EXISTS (SELECT 1 FROM media_roots WHERE key=old_key);
    UPDATE media_objects SET unused_since=NULL WHERE key=new_key;
$$;

CREATE FUNCTION media_rendition_retired() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF TG_OP='DELETE' OR OLD.object_key IS DISTINCT FROM NEW.object_key THEN
  UPDATE media_objects SET unused_since=now() WHERE key=OLD.object_key OR key IN(SELECT object_key FROM media_variants WHERE source_key=OLD.object_key);
 END IF;
 RETURN NULL;
END; $$;

CREATE FUNCTION media_track_account() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP='DELETE' THEN PERFORM media_reference_changed(OLD.avatar_key,NULL);
    ELSIF TG_OP='INSERT' THEN PERFORM media_reference_changed(NULL,NEW.avatar_key);
    ELSE PERFORM media_reference_changed(OLD.avatar_key,NEW.avatar_key);
    END IF;
    RETURN NULL;
END;
$$;

CREATE FUNCTION media_track_brand() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP='DELETE' THEN PERFORM media_reference_changed(OLD.logo_key,NULL);
    ELSIF TG_OP='INSERT' THEN PERFORM media_reference_changed(NULL,NEW.logo_key);
    ELSE PERFORM media_reference_changed(OLD.logo_key,NEW.logo_key);
    END IF;
    RETURN NULL;
END;
$$;

CREATE FUNCTION media_track_product() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP='DELETE' THEN PERFORM media_reference_changed(OLD.image_key,NULL);
    ELSIF TG_OP='INSERT' THEN PERFORM media_reference_changed(NULL,NEW.image_key);
    ELSE PERFORM media_reference_changed(OLD.image_key,NEW.image_key);
    END IF;
    RETURN NULL;
END;
$$;

CREATE FUNCTION media_track_store() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        PERFORM media_reference_changed(OLD.logo_key,NULL);
        PERFORM media_reference_changed(OLD.favicon_key,NULL);
    ELSIF TG_OP='INSERT' THEN
        PERFORM media_reference_changed(NULL,NEW.logo_key);
        PERFORM media_reference_changed(NULL,NEW.favicon_key);
    ELSE
        PERFORM media_reference_changed(OLD.logo_key,NEW.logo_key);
        PERFORM media_reference_changed(OLD.favicon_key,NEW.favicon_key);
    END IF;
    RETURN NULL;
END;
$$;

CREATE FUNCTION next_order_reference(issued_at timestamp with time zone) RETURNS text
    LANGUAGE plpgsql STRICT
    AS $$
DECLARE
    reference_year INTEGER;
    zone TEXT;
    reference_number BIGINT;
BEGIN
    -- A settings save waits until this order commits, keeping its numbering
    -- zone consistent throughout confirmation and notification rendering.
    SELECT time_zone INTO STRICT zone FROM platform_regional_settings WHERE singleton FOR SHARE;
    reference_year := extract(year FROM issued_at AT TIME ZONE zone)::integer;
    INSERT INTO order_reference_counters(year, last_number)
    VALUES (reference_year, 1)
    ON CONFLICT (year) DO UPDATE
        SET last_number = order_reference_counters.last_number + 1
    RETURNING last_number INTO reference_number;

    RETURN 'ORD-' || reference_year::text || '-' || reference_number::text;
END;
$$;

CREATE FUNCTION online_capture_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM payment_attempts a WHERE a.id=NEW.attempt_id AND a.intent_id=NEW.intent_id AND a.amount_cents=NEW.amount_cents AND a.currency=NEW.currency) THEN
  RAISE EXCEPTION 'Capture mismatch' USING ERRCODE='23514',CONSTRAINT='online_capture'; END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION online_refund() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM online_captures c WHERE c.attempt_id=NEW.attempt_id AND c.amount_cents=NEW.amount_cents AND c.currency=NEW.currency) THEN
  RAISE EXCEPTION 'Refund mismatch' USING ERRCODE='23514',CONSTRAINT='online_refund'; END IF;
 UPDATE orders o SET payment_state='refunded',revision=revision+1
 FROM order_online_payments p WHERE p.order_id=o.id AND p.attempt_id=NEW.attempt_id;
 RETURN NEW;
END; $$;

CREATE FUNCTION online_settlement() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE target orders%ROWTYPE;
BEGIN
 SELECT * INTO target FROM orders WHERE id=NEW.order_id FOR UPDATE;
 IF target.payment_state<>'unpaid' OR NOT EXISTS(
    SELECT 1 FROM online_captures c JOIN payment_attempts a ON a.id=c.attempt_id
    WHERE c.attempt_id=NEW.attempt_id AND a.order_id=NEW.order_id AND c.amount_cents=target.total_cents AND c.currency=target.currency) THEN
  RAISE EXCEPTION 'Settlement mismatch' USING ERRCODE='23514',CONSTRAINT='online_settlement'; END IF;
 UPDATE orders SET payment_state='paid',revision=revision+1 WHERE id=NEW.order_id;
 RETURN NEW;
END; $$;

CREATE FUNCTION operation_event_immutable() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 RAISE EXCEPTION 'Operation events are append-only' USING ERRCODE='23514';
END; $$;

CREATE FUNCTION order_current_status_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE o orders%ROWTYPE; kind TEXT; physical TEXT;
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Current order status is permanent' USING ERRCODE='23514'; END IF;
 SELECT * INTO STRICT o FROM orders WHERE id=NEW.order_id FOR UPDATE;
 IF TG_OP='UPDATE' AND (NEW.order_id<>OLD.order_id OR NEW.revision<>OLD.revision+1) THEN
  RAISE EXCEPTION 'Stale order status' USING ERRCODE='23514';
 END IF;
 SELECT role INTO kind FROM order_status_roles WHERE status_id=NEW.status_id;
 SELECT state INTO physical FROM order_fulfillment WHERE order_id=NEW.order_id;
 IF (kind='refunded' AND o.payment_state<>'refunded') OR
    (kind='cancelled' AND o.state<>'cancelled') OR
    (kind IN ('shipped','delivered') AND kind IS DISTINCT FROM physical) OR
    (o.payment_state='refunded' AND kind IS DISTINCT FROM 'refunded' AND NOT coalesce(physical IN ('shipped','delivered') AND kind=physical,false)) OR
    (o.payment_state<>'refunded' AND o.state='cancelled' AND kind IS DISTINCT FROM 'cancelled') OR
    (o.payment_state<>'refunded' AND o.state='open' AND physical IN ('shipped','delivered') AND kind IS DISTINCT FROM physical)
 THEN RAISE EXCEPTION 'Use the protected order operation' USING ERRCODE='23514',CONSTRAINT='order_status_operation'; END IF;
 RETURN NEW;
END $$;

CREATE FUNCTION order_fulfillment_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE o orders%ROWTYPE;
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Fulfillment is permanent' USING ERRCODE='23514'; END IF;
 SELECT * INTO o FROM orders WHERE id=NEW.order_id FOR UPDATE;
 IF TG_OP='INSERT' THEN
  IF NEW.state<>'pending' OR NEW.revision<>0 THEN RAISE EXCEPTION 'Invalid initial fulfillment' USING ERRCODE='23514'; END IF;
  RETURN NEW;
 END IF;
 IF NEW.order_id<>OLD.order_id OR NEW.revision<>OLD.revision+1 THEN RAISE EXCEPTION 'Invalid fulfillment revision' USING ERRCODE='23514'; END IF;
 -- Notes may be appended to closed orders without changing public shipment data.
 IF (to_jsonb(NEW)-'revision')=(to_jsonb(OLD)-'revision') THEN RETURN NEW; END IF;
 IF o.state<>'open' OR NOT (
  (OLD.state='pending' AND NEW.state='preparing') OR
  (OLD.state='preparing' AND NEW.state='shipped') OR
  (OLD.state='shipped' AND NEW.state='delivered') OR
  (OLD.state=NEW.state AND OLD.state IN ('shipped','delivered'))
 ) THEN RAISE EXCEPTION 'Invalid fulfillment transition' USING ERRCODE='23514',CONSTRAINT='fulfillment_transition'; END IF;
 IF OLD.state='preparing' AND NEW.state='shipped' AND (
  o.payment_state='refunded' OR
  EXISTS(SELECT 1 FROM payment_attempts a WHERE a.order_id=o.id AND ((a.state='review' AND a.last_error<>'temporary') OR ((a.state='refund_pending' OR a.refund_reason IS NOT NULL) AND EXISTS(SELECT 1 FROM order_online_payments p WHERE p.order_id=o.id AND p.attempt_id=a.id))))
 ) THEN RAISE EXCEPTION 'Payment does not permit shipment' USING ERRCODE='23514',CONSTRAINT='fulfillment_payment'; END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION order_editor_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE oid bigint;
BEGIN
 IF TG_OP='UPDATE' THEN
  IF TG_TABLE_NAME='orders' THEN oid:=OLD.id; ELSE oid:=OLD.order_id; END IF;
  IF EXISTS(SELECT 1 FROM orders o JOIN order_edit_events e ON e.order_id=o.id
      WHERE o.id=oid AND o.state='open' AND o.payment_state='unpaid'
      AND e.transaction_id=pg_current_xact_id()
      AND e.revision IN (o.revision,o.revision+1)
      AND NOT EXISTS(SELECT 1 FROM order_fulfillment f WHERE f.order_id=o.id AND f.state IN ('shipped','delivered'))
      AND NOT EXISTS(SELECT 1 FROM payment_attempts p WHERE p.order_id=o.id)) THEN
   IF TG_TABLE_NAME='orders' THEN
    IF NEW.revision=OLD.revision+1 AND
      (to_jsonb(NEW)-ARRAY['snapshot','merchandise_cents','shipping_cents','fee_cents','net_cents','tax_cents','total_cents','revision'])=
      (to_jsonb(OLD)-ARRAY['snapshot','merchandise_cents','shipping_cents','fee_cents','net_cents','tax_cents','total_cents','revision']) THEN RETURN NEW; END IF;
   ELSIF TG_TABLE_NAME='order_lines' THEN
    IF OLD.active AND NOT NEW.active AND
      (to_jsonb(NEW)-'active')=(to_jsonb(OLD)-'active') THEN RETURN NEW; END IF;
   ELSIF TG_TABLE_NAME='order_customer_summaries' THEN
    IF NEW.order_id=OLD.order_id THEN RETURN NEW; END IF;
   END IF;
  END IF;
 END IF;
 RAISE EXCEPTION 'Immutable order content' USING ERRCODE='23514',CONSTRAINT='immutable_order_snapshot';
END; $$;

CREATE FUNCTION order_snapshot_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF TG_TABLE_NAME='orders' AND TG_OP='UPDATE' THEN
  IF (to_jsonb(NEW)-ARRAY['state','revision','cancelled_at','payment_state']) =
     (to_jsonb(OLD)-ARRAY['state','revision','cancelled_at','payment_state']) AND NEW.revision=OLD.revision+1 THEN
   IF OLD.state='open' AND NEW.state='cancelled' AND NEW.payment_state=OLD.payment_state AND
      (OLD.payment_state='unpaid' OR (OLD.payment_state='refunded' AND EXISTS(
        SELECT 1 FROM order_online_payments p JOIN online_refunds r ON r.attempt_id=p.attempt_id WHERE p.order_id=OLD.id))) THEN RETURN NEW; END IF;
   IF NEW.state=OLD.state AND NEW.cancelled_at IS NOT DISTINCT FROM OLD.cancelled_at THEN
    IF OLD.payment_state='unpaid' AND NEW.payment_state='paid' AND
       (EXISTS(SELECT 1 FROM order_online_payments WHERE order_id=OLD.id) OR
        (OLD.state='open' AND EXISTS(SELECT 1 FROM order_manual_payments WHERE order_id=OLD.id))) THEN RETURN NEW; END IF;
    IF OLD.payment_state='paid' AND NEW.payment_state='refunded' AND EXISTS(
       SELECT 1 FROM order_online_payments p JOIN online_refunds r ON r.attempt_id=p.attempt_id WHERE p.order_id=OLD.id) THEN RETURN NEW; END IF;
   END IF;
  END IF;
 END IF;
 RAISE EXCEPTION 'Immutable order snapshot' USING ERRCODE='23514',CONSTRAINT='immutable_order_snapshot';
END; $$;

CREATE FUNCTION order_status_initialize() RETURNS trigger
    LANGUAGE plpgsql
    AS $_$
DECLARE selected BIGINT;
BEGIN
 selected:=substring(NEW.snapshot from '(?n)^status_id = ([0-9]+)$')::bigint;
 IF NEW.payment_state='refunded' OR NEW.state='cancelled' THEN
  SELECT status_id INTO STRICT selected FROM order_status_roles WHERE role=CASE WHEN NEW.payment_state='refunded' THEN 'refunded' ELSE 'cancelled' END;
 END IF;
 IF selected IS NOT NULL AND EXISTS(SELECT 1 FROM order_statuses WHERE id=selected) THEN
  INSERT INTO order_current_status(order_id,status_id) VALUES(NEW.id,selected);
 END IF;
 RETURN NEW;
END $_$;

CREATE FUNCTION order_status_record() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 INSERT INTO order_status_events(order_id,status_id,revision) VALUES(NEW.order_id,NEW.status_id,NEW.revision);
 RETURN NEW;
END $$;

CREATE FUNCTION order_status_sync() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE oid BIGINT; target BIGINT; kind TEXT;
BEGIN
 IF TG_TABLE_NAME='orders' THEN
  oid:=NEW.id;
  IF NEW.payment_state='refunded' THEN kind:='refunded';
  ELSIF NEW.state='cancelled' THEN kind:='cancelled'; END IF;
 ELSE
  oid:=NEW.order_id;
  IF TG_OP='UPDATE' AND NEW.state=OLD.state THEN RETURN NEW; END IF;
  IF NEW.state IN ('preparing','shipped','delivered') THEN kind:=NEW.state; END IF;
 END IF;
 IF kind IS NOT NULL THEN
  SELECT status_id INTO STRICT target FROM order_status_roles WHERE role=kind;
  INSERT INTO order_current_status(order_id,status_id) VALUES(oid,target)
  ON CONFLICT(order_id) DO UPDATE SET status_id=excluded.status_id,revision=order_current_status.revision+1
  WHERE order_current_status.status_id<>excluded.status_id;
 END IF;
 RETURN NEW;
END $$;

CREATE FUNCTION order_statuses_capacity_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(5426645964561798229);
 IF (SELECT count(*) FROM order_statuses)>=1000 THEN
  RAISE EXCEPTION 'Order status capacity reached' USING ERRCODE='23514',CONSTRAINT='order_statuses_capacity';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION order_stock_structure_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM order_lines l JOIN orders o ON o.id=l.order_id WHERE l.product_id=NEW.product_id AND l.variant_id IS NULL AND l.active AND o.state='open') THEN
  RAISE EXCEPTION 'Outstanding order stock target' USING ERRCODE='23514',CONSTRAINT='order_stock_structure';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION payment_attempt_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE target orders%ROWTYPE;
BEGIN
 IF TG_OP='INSERT' THEN
  SELECT * INTO target FROM orders WHERE id=NEW.order_id FOR UPDATE;
  IF NOT FOUND OR target.state<>'open' OR target.payment_state<>'unpaid' OR
    target.total_cents<>NEW.amount_cents OR target.currency<>NEW.currency OR
    (SELECT count(*) FROM payment_attempts WHERE order_id=NEW.order_id)>=10 THEN
   RAISE EXCEPTION 'Invalid payment attempt' USING ERRCODE='23514',CONSTRAINT='payment_attempt_order';
  END IF;
  RETURN NEW;
 ELSIF TG_OP='UPDATE' THEN
  IF ROW(NEW.id,NEW.order_id,NEW.request_key,NEW.amount_cents,NEW.currency,NEW.title,NEW.return_url,NEW.locale,NEW.created_at)
   IS NOT DISTINCT FROM ROW(OLD.id,OLD.order_id,OLD.request_key,OLD.amount_cents,OLD.currency,OLD.title,OLD.return_url,OLD.locale,OLD.created_at)
   AND (OLD.session_id IS NULL OR NEW.session_id IS NOT DISTINCT FROM OLD.session_id)
   AND (OLD.intent_id IS NULL OR NEW.intent_id IS NOT DISTINCT FROM OLD.intent_id)
   AND (OLD.refund_id IS NULL OR NEW.refund_id IS NOT DISTINCT FROM OLD.refund_id) THEN RETURN NEW; END IF;
 END IF;
 RAISE EXCEPTION 'Immutable payment identity' USING ERRCODE='23514',CONSTRAINT='payment_attempt_identity';
END; $$;

CREATE FUNCTION payment_attempt_mode_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF NEW.ui_mode IS DISTINCT FROM OLD.ui_mode THEN
  RAISE EXCEPTION 'Immutable payment mode' USING ERRCODE='23514',CONSTRAINT='payment_attempt_mode';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION payment_initial_status_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM order_status_roles WHERE status_id=NEW.initial_order_status_id AND role<>'preparing') THEN
  RAISE EXCEPTION 'Invalid initial order status' USING ERRCODE='23514',CONSTRAINT='payment_initial_status';
 END IF;
 RETURN NEW;
END $$;

CREATE FUNCTION payment_methods_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(5426645964561798229);
 IF TG_OP='UPDATE' AND OLD.code<>NEW.code THEN
  RAISE EXCEPTION 'Payment implementation is immutable' USING ERRCODE='23514',CONSTRAINT='payment_method_code_immutable';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION payment_provider_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF NEW.provider_id IS DISTINCT FROM OLD.provider_id THEN
  RAISE EXCEPTION 'Immutable payment provider' USING ERRCODE='23514',CONSTRAINT='payment_provider_immutable';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION platform_datetime(instant timestamp with time zone, zone text, style text) RETURNS text
    LANGUAGE sql STABLE STRICT
    AS $$
    SELECT to_char(instant AT TIME ZONE zone,
        CASE style
            WHEN 'dmy' THEN 'DD/MM/YYYY HH24:MI:SS'
            WHEN 'ymd' THEN 'YYYY-MM-DD HH24:MI:SS'
            WHEN 'mdy' THEN 'MM/DD/YYYY HH24:MI:SS'
        END);
$$;

CREATE FUNCTION platform_time_zone_valid(zone text) RETURNS boolean
    LANGUAGE sql STABLE STRICT
    AS $$
    SELECT EXISTS (
        SELECT 1 FROM pg_timezone_names
        WHERE name = zone AND (
            name = 'UTC' OR name ~ '^(Africa|America|Antarctica|Arctic|Asia|Atlantic|Australia|Europe|Indian|Pacific)/'
        )
    );
$$;

CREATE FUNCTION product_gallery_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(5426645964561798229);
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    IF TG_OP='UPDATE' AND NEW.product_id<>OLD.product_id THEN
        RAISE EXCEPTION 'Immutable gallery owner' USING ERRCODE='23514',CONSTRAINT='catalog_product_images_owner';
    END IF;
    IF TG_OP='INSERT' AND (SELECT count(*) FROM catalog_product_images WHERE product_id=NEW.product_id)>=12 THEN
        RAISE EXCEPTION 'Gallery limit' USING ERRCODE='23514',CONSTRAINT='catalog_product_images_limit';
    END IF;
    RETURN NEW;
END; $$;

CREATE FUNCTION product_mode_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(5426645964561798229);
    IF OLD.price_mode<>NEW.price_mode AND EXISTS(SELECT 1 FROM catalog_products) THEN
        RAISE EXCEPTION 'Existing product prices lock mode' USING ERRCODE='23514',CONSTRAINT='pricing_settings_product_prices';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION product_options_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE parent BIGINT;
BEGIN
 PERFORM pg_advisory_xact_lock(5426645964561798229);
 IF TG_OP='UPDATE' AND NEW.product_id<>OLD.product_id THEN
  RAISE EXCEPTION 'Immutable option assignment owner' USING ERRCODE='23514',CONSTRAINT='catalog_product_options_in_use';
 END IF;
 IF TG_OP='DELETE' THEN parent=OLD.product_id; ELSE parent=NEW.product_id; END IF;
 IF EXISTS(SELECT 1 FROM catalog_products WHERE id=parent) AND EXISTS(SELECT 1 FROM catalog_variants WHERE product_id=parent) THEN
  RAISE EXCEPTION 'Remove variants before changing options' USING ERRCODE='23514',CONSTRAINT='catalog_product_options_in_use';
 END IF;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION product_price_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(5426645964561798229);
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    IF NEW.status='published' AND NOT EXISTS(
        SELECT 1 FROM tax_classes t JOIN tax_rates r ON r.id=t.rate_id
        WHERE t.id=NEW.tax_class_id AND t.active AND r.active
        AND (NEW.category_id IS NULL OR EXISTS(
            SELECT 1 FROM catalog_categories c WHERE c.id=NEW.category_id AND c.active))
        AND (NEW.brand_id IS NULL OR EXISTS(
            SELECT 1 FROM catalog_brands b WHERE b.id=NEW.brand_id AND b.active))
    ) THEN
        RAISE EXCEPTION 'Active publication references required' USING ERRCODE='23514',CONSTRAINT='catalog_products_publication';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION protect_admin_role() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF OLD.key = 'owner' THEN
        RAISE EXCEPTION 'Protected system role' USING ERRCODE = '23514';
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.key <> OLD.key THEN
        RAISE EXCEPTION 'Role key is immutable' USING ERRCODE = '23514';
    END IF;
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION protect_cart_customer() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.customer_id IS DISTINCT FROM OLD.customer_id THEN
        RAISE EXCEPTION 'Cart ownership is immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION record_manual_payment() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE target orders%ROWTYPE;
BEGIN
 SELECT * INTO target FROM orders WHERE id=NEW.order_id FOR UPDATE;
 IF NOT FOUND OR target.state<>'open' OR target.payment_state<>'unpaid' OR
    target.total_cents<>NEW.amount_cents OR target.currency<>NEW.currency THEN
  RAISE EXCEPTION 'Manual payment does not match open unpaid order'
   USING ERRCODE='23514',CONSTRAINT='manual_payment_order';
 END IF;
 UPDATE orders SET payment_state='paid',revision=revision+1 WHERE id=NEW.order_id;
 RETURN NEW;
END; $$;

CREATE FUNCTION record_operation_event() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE row_data JSONB; actor TEXT;
BEGIN
 IF TG_OP='UPDATE' AND NEW IS NOT DISTINCT FROM OLD THEN RETURN NULL; END IF;
 IF TG_OP='DELETE' THEN row_data:=to_jsonb(OLD); ELSE row_data:=to_jsonb(NEW); END IF;
 actor:=nullif(current_setting('talocart.administrator_id',true),'');
 INSERT INTO operation_events(area,resource,resource_key,action,administrator_id)
 VALUES(TG_ARGV[0],TG_TABLE_NAME,row_data->>TG_ARGV[1],TG_OP,actor::bigint);
 RETURN NULL;
END; $$;

CREATE FUNCTION refund_cancellation_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM orders o JOIN order_online_payments p ON p.order_id=o.id LEFT JOIN order_fulfillment f ON f.order_id=o.id WHERE p.attempt_id=NEW.attempt_id AND o.state<>'cancelled' AND coalesce(f.state,'pending') NOT IN ('shipped','delivered')) THEN
  RAISE EXCEPTION 'Unshipped canonical refund requires cancellation' USING ERRCODE='23514',CONSTRAINT='refund_cancellation';
 END IF;
 RETURN NULL;
END; $$;

CREATE FUNCTION resolve_tax(destination text) RETURNS TABLE(class_id bigint, tax_name text, basis_points integer, zone_name text, explicit_rule boolean)
    LANGUAGE sql STABLE
    AS $$
 SELECT c.id,c.name,coalesce(zr.basis_points,r.basis_points),z.name,q.class_id IS NOT NULL
 FROM tax_classes c JOIN tax_rates r ON r.id=c.rate_id AND r.active
 LEFT JOIN tax_zone_countries countries ON countries.country_code=destination
 LEFT JOIN tax_zones z ON z.id=countries.zone_id AND z.active
 LEFT JOIN tax_zone_rules q ON q.zone_id=z.id AND q.class_id=c.id
 LEFT JOIN tax_rates zr ON zr.id=q.rate_id AND zr.active
 WHERE c.active
$$;

CREATE FUNCTION shipped_order_cancellation_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF NEW.state='cancelled' AND OLD.state<>'cancelled' AND EXISTS(SELECT 1 FROM order_fulfillment WHERE order_id=OLD.id AND state IN ('shipped','delivered')) THEN
  RAISE EXCEPTION 'Shipped order cannot be cancelled' USING ERRCODE='23514',CONSTRAINT='shipped_order_cancellation';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION shipping_methods_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(5426645964561798229);
 IF TG_OP='UPDATE' AND OLD.code<>NEW.code THEN
  RAISE EXCEPTION 'Shipping implementation is immutable' USING ERRCODE='23514',CONSTRAINT='shipping_method_code_immutable';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION shipping_revision_changed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 UPDATE shipping_configuration SET revision=revision+1 WHERE id=1;
 RETURN NULL;
END; $$;

CREATE FUNCTION tax_configuration_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(5426645964561798229);
    IF TG_OP='DELETE' THEN
        IF TG_TABLE_NAME='pricing_settings' THEN
            RAISE EXCEPTION 'Pricing settings required' USING ERRCODE='23514',CONSTRAINT='pricing_settings_required';
        END IF;
        RETURN OLD;
    END IF;
    IF TG_TABLE_NAME='tax_rates' THEN
        IF TG_OP='INSERT' AND (SELECT count(*) FROM tax_rates)>=100 THEN
            RAISE EXCEPTION 'Rate limit' USING ERRCODE='23514',CONSTRAINT='tax_rates_limit';
        END IF;
        IF NOT NEW.active AND EXISTS(SELECT 1 FROM tax_classes WHERE rate_id=NEW.id AND active) THEN
            RAISE EXCEPTION 'Active classes use rate' USING ERRCODE='23514',CONSTRAINT='tax_rates_active_reference';
        END IF;
    ELSIF TG_TABLE_NAME='tax_classes' THEN
        IF TG_OP='INSERT' AND (SELECT count(*) FROM tax_classes)>=100 THEN
            RAISE EXCEPTION 'Class limit' USING ERRCODE='23514',CONSTRAINT='tax_classes_limit';
        END IF;
        IF NEW.active AND NOT EXISTS(SELECT 1 FROM tax_rates WHERE id=NEW.rate_id AND active) THEN
            RAISE EXCEPTION 'Active rate required' USING ERRCODE='23514',CONSTRAINT='tax_classes_active_rate';
        END IF;
        IF NOT NEW.active AND EXISTS(SELECT 1 FROM pricing_settings WHERE default_class_id=NEW.id) THEN
            RAISE EXCEPTION 'Active default required' USING ERRCODE='23514',CONSTRAINT='tax_classes_active_default';
        END IF;
    ELSE
        IF NEW.default_class_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM tax_classes c JOIN tax_rates r ON r.id=c.rate_id WHERE c.id=NEW.default_class_id AND c.active AND r.active) THEN
            RAISE EXCEPTION 'Invalid default class' USING ERRCODE='23514',CONSTRAINT='pricing_settings_active_default';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION tax_revision_changed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 UPDATE tax_configuration SET revision=revision+1 WHERE id=1;
 RETURN NULL;
END; $$;

CREATE FUNCTION tax_zone_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(5426645964561798229);
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 IF TG_TABLE_NAME='tax_zones' THEN
  IF TG_OP='INSERT' AND (SELECT count(*) FROM tax_zones)>=100 THEN
   RAISE EXCEPTION 'Zone limit' USING ERRCODE='23514',CONSTRAINT='tax_zones_limit';
  END IF;
  IF NEW.active AND EXISTS(SELECT 1 FROM tax_zone_rules q JOIN tax_classes c ON c.id=q.class_id JOIN tax_rates r ON r.id=q.rate_id WHERE q.zone_id=NEW.id AND (NOT c.active OR NOT r.active)) THEN
   RAISE EXCEPTION 'Active rules required' USING ERRCODE='23514',CONSTRAINT='tax_zones_active_class';
  END IF;
 ELSIF TG_TABLE_NAME='tax_zone_rules' THEN
  IF EXISTS(SELECT 1 FROM tax_zones WHERE id=NEW.zone_id AND active) AND NOT EXISTS(SELECT 1 FROM tax_classes c CROSS JOIN tax_rates r WHERE c.id=NEW.class_id AND r.id=NEW.rate_id AND c.active AND r.active) THEN
   RAISE EXCEPTION 'Active rule references required' USING ERRCODE='23514',CONSTRAINT='tax_zones_active_class';
  END IF;
 ELSIF TG_TABLE_NAME='tax_classes' THEN
  IF NOT NEW.active AND EXISTS(SELECT 1 FROM tax_zone_rules q JOIN tax_zones z ON z.id=q.zone_id WHERE q.class_id=NEW.id AND z.active) THEN
   RAISE EXCEPTION 'Active zones use class' USING ERRCODE='23514',CONSTRAINT='tax_classes_active_zones';
  END IF;
 ELSIF TG_TABLE_NAME='tax_rates' THEN
  IF NOT NEW.active AND EXISTS(SELECT 1 FROM tax_zone_rules q JOIN tax_zones z ON z.id=q.zone_id WHERE q.rate_id=NEW.id AND z.active) THEN
   RAISE EXCEPTION 'Active zones use rate' USING ERRCODE='23514',CONSTRAINT='tax_rates_active_zones';
  END IF;
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION tax_zone_nonempty_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE zone BIGINT;
BEGIN
 IF TG_TABLE_NAME='tax_zones' THEN zone=NEW.id;
 ELSIF TG_OP='DELETE' THEN zone=OLD.zone_id;
 ELSE zone=NEW.zone_id; END IF;
 IF EXISTS(SELECT 1 FROM tax_zones WHERE id=zone) AND NOT EXISTS(SELECT 1 FROM tax_zone_countries WHERE zone_id=zone) THEN
  RAISE EXCEPTION 'Zone countries required' USING ERRCODE='23514',CONSTRAINT='tax_zone_countries_required';
 END IF;
 IF TG_TABLE_NAME='tax_zone_countries' THEN
 IF TG_OP='UPDATE' AND OLD.zone_id<>NEW.zone_id AND EXISTS(SELECT 1 FROM tax_zones WHERE id=OLD.zone_id) AND NOT EXISTS(SELECT 1 FROM tax_zone_countries WHERE zone_id=OLD.zone_id) THEN
  RAISE EXCEPTION 'Old zone countries required' USING ERRCODE='23514',CONSTRAINT='tax_zone_countries_required';
 END IF;
 END IF;
 RETURN NULL;
END; $$;

CREATE FUNCTION variant_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE expected INTEGER; actual INTEGER;
BEGIN
 PERFORM pg_advisory_xact_lock(5426645964561798229);
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 IF TG_OP='UPDATE' AND NEW.product_id<>OLD.product_id THEN
  RAISE EXCEPTION 'Immutable variant owner' USING ERRCODE='23514',CONSTRAINT='catalog_variants_owner';
 END IF;
 SELECT count(*) INTO expected FROM catalog_product_options WHERE product_id=NEW.product_id;
 NEW.value_ids=ARRAY(SELECT DISTINCT v FROM unnest(NEW.value_ids) v ORDER BY v);
 SELECT count(DISTINCT v.option_id) INTO actual FROM catalog_option_values v JOIN catalog_product_options a ON a.option_id=v.option_id AND a.product_id=NEW.product_id WHERE v.id=ANY(NEW.value_ids);
 IF expected=0 OR cardinality(NEW.value_ids)<>expected OR actual<>expected THEN
  RAISE EXCEPTION 'One value per assigned option required' USING ERRCODE='23514',CONSTRAINT='catalog_variants_combination';
 END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION variant_values_sync() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 DELETE FROM catalog_variant_values WHERE variant_id=NEW.id;
 INSERT INTO catalog_variant_values(variant_id,value_id) SELECT NEW.id,v FROM unnest(NEW.value_ids) v;
 RETURN NULL;
END; $$;

CREATE TABLE admin_role_permissions (
    role_key text NOT NULL,
    permission text NOT NULL,
    CONSTRAINT admin_role_permissions_permission_check CHECK ((permission = ANY (ARRAY['overview.read'::text, 'settings.read'::text, 'settings.write'::text, 'users.read'::text, 'users.write'::text, 'roles.read'::text, 'roles.write'::text, 'categories.read'::text, 'categories.write'::text, 'brands.read'::text, 'brands.write'::text, 'taxes.read'::text, 'taxes.write'::text, 'products.read'::text, 'products.write'::text, 'media.read'::text, 'media.write'::text, 'orders.read'::text, 'orders.write'::text, 'pages.read'::text, 'pages.write'::text, 'operations.read'::text, 'customers.read'::text, 'customers.write'::text]))),
    CONSTRAINT admin_role_permissions_role_key_check CHECK ((role_key <> 'owner'::text))
);

CREATE TABLE admin_roles (
    key text NOT NULL,
    name text NOT NULL,
    CONSTRAINT admin_roles_key_check CHECK (((key = ANY (ARRAY['owner'::text, 'viewer'::text])) OR (key ~ '^role_[0-9a-f]{16}$'::text))),
    CONSTRAINT admin_roles_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 80)) AND (name = btrim(name))))
);

CREATE TABLE admin_sessions (
    token_hash text NOT NULL,
    csrf text NOT NULL,
    administrator_id bigint,
    expires_at timestamp with time zone NOT NULL
);

CREATE TABLE administrator_languages (
    code text NOT NULL,
    CONSTRAINT administrator_languages_code_check CHECK ((code ~ '^[a-z]{2,3}(-[A-Za-z0-9]{2,8})*$'::text))
);

CREATE TABLE administrators (
    id bigint NOT NULL,
    email text NOT NULL,
    password_hash text NOT NULL,
    role text NOT NULL,
    active boolean DEFAULT true NOT NULL,
    display_name text DEFAULT ''::text NOT NULL,
    avatar_version bigint DEFAULT 0 NOT NULL,
    avatar_key text,
    locale text DEFAULT 'it'::text NOT NULL,
    product_hidden_columns text[] DEFAULT '{}'::text[] NOT NULL,
    CONSTRAINT administrators_avatar_version_check CHECK ((avatar_version >= 0)),
    CONSTRAINT administrators_display_name_check CHECK (((char_length(display_name) <= 80) AND (display_name = btrim(display_name)))),
    CONSTRAINT administrators_product_hidden_columns_check CHECK (((cardinality(product_hidden_columns) < 14) AND (array_position(product_hidden_columns, NULL::text) IS NULL) AND (product_hidden_columns <@ ARRAY['selection'::text, 'id'::text, 'image'::text, 'type'::text, 'name'::text, 'model'::text, 'category'::text, 'brand'::text, 'price'::text, 'variants'::text, 'stock'::text, 'status'::text, 'modified_at'::text, 'actions'::text])))
);

ALTER TABLE administrators ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME administrators_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE auth_limits (
    bucket text NOT NULL,
    attempts integer NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

CREATE TABLE catalog_attribute_groups (
    id bigint NOT NULL,
    name text NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT catalog_attribute_groups_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 80)) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_attribute_groups_revision_check CHECK ((revision >= 0)),
    CONSTRAINT catalog_attribute_groups_sort_order_check CHECK (((sort_order >= 0) AND (sort_order <= 9999)))
);

ALTER TABLE catalog_attribute_groups ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME catalog_attribute_groups_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE catalog_attribute_groups_translations (
    entity_id bigint NOT NULL,
    locale text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    CONSTRAINT catalog_attribute_groups_translations_description_check CHECK ((description = ''::text)),
    CONSTRAINT catalog_attribute_groups_translations_name_check CHECK (((char_length(name) <= 80) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_attribute_groups_translations_seo_description_check CHECK ((seo_description = ''::text)),
    CONSTRAINT catalog_attribute_groups_translations_seo_title_check CHECK ((seo_title = ''::text))
);

CREATE TABLE catalog_attributes (
    id bigint NOT NULL,
    name text NOT NULL,
    group_id bigint NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT catalog_attributes_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 80)) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_attributes_revision_check CHECK ((revision >= 0)),
    CONSTRAINT catalog_attributes_sort_order_check CHECK (((sort_order >= 0) AND (sort_order <= 9999)))
);

ALTER TABLE catalog_attributes ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME catalog_attributes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE catalog_attributes_translations (
    entity_id bigint NOT NULL,
    locale text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    CONSTRAINT catalog_attributes_translations_description_check CHECK ((description = ''::text)),
    CONSTRAINT catalog_attributes_translations_name_check CHECK (((char_length(name) <= 80) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_attributes_translations_seo_description_check CHECK ((seo_description = ''::text)),
    CONSTRAINT catalog_attributes_translations_seo_title_check CHECK ((seo_title = ''::text))
);

CREATE TABLE catalog_brands (
    id bigint NOT NULL,
    name text NOT NULL,
    slug text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    active boolean DEFAULT true NOT NULL,
    logo_key text,
    CONSTRAINT catalog_brands_description_check CHECK ((char_length(description) <= 4000)),
    CONSTRAINT catalog_brands_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 120)) AND (name = btrim(name)))),
    CONSTRAINT catalog_brands_slug_check CHECK ((((char_length(slug) >= 1) AND (char_length(slug) <= 160)) AND (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$'::text))),
    CONSTRAINT catalog_brands_sort_order_check CHECK (((sort_order >= 0) AND (sort_order <= 999999)))
);

ALTER TABLE catalog_brands ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME catalog_brands_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE catalog_brands_translations (
    entity_id bigint NOT NULL,
    locale text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    CONSTRAINT catalog_brands_translations_description_check CHECK (((char_length(description) <= 4000) AND (description = btrim(description)) AND (description !~ '[\x01-\x08\x0B\x0C\x0E-\x1F\x7F]'::text))),
    CONSTRAINT catalog_brands_translations_name_check CHECK (((char_length(name) <= 120) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_brands_translations_seo_description_check CHECK (((char_length(seo_description) <= 0) AND (seo_description = btrim(seo_description)) AND (seo_description !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_brands_translations_seo_title_check CHECK (((char_length(seo_title) <= 0) AND (seo_title = btrim(seo_title)) AND (seo_title !~ '[[:cntrl:]]'::text)))
);

CREATE TABLE catalog_categories (
    id bigint NOT NULL,
    name text NOT NULL,
    slug text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    parent_id bigint,
    sort_order integer DEFAULT 0 NOT NULL,
    active boolean DEFAULT true NOT NULL,
    CONSTRAINT catalog_categories_check CHECK (((parent_id IS NULL) OR (parent_id <> id))),
    CONSTRAINT catalog_categories_description_check CHECK ((char_length(description) <= 4000)),
    CONSTRAINT catalog_categories_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 120)) AND (name = btrim(name)))),
    CONSTRAINT catalog_categories_slug_check CHECK ((((char_length(slug) >= 1) AND (char_length(slug) <= 160)) AND (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$'::text))),
    CONSTRAINT catalog_categories_sort_order_check CHECK (((sort_order >= 0) AND (sort_order <= 999999)))
);

ALTER TABLE catalog_categories ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME catalog_categories_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE catalog_categories_translations (
    entity_id bigint NOT NULL,
    locale text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    CONSTRAINT catalog_categories_translations_description_check CHECK (((char_length(description) <= 4000) AND (description = btrim(description)) AND (description !~ '[\x01-\x08\x0B\x0C\x0E-\x1F\x7F]'::text))),
    CONSTRAINT catalog_categories_translations_name_check CHECK (((char_length(name) <= 120) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_categories_translations_seo_description_check CHECK (((char_length(seo_description) <= 0) AND (seo_description = btrim(seo_description)) AND (seo_description !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_categories_translations_seo_title_check CHECK (((char_length(seo_title) <= 0) AND (seo_title = btrim(seo_title)) AND (seo_title !~ '[[:cntrl:]]'::text)))
);

CREATE TABLE catalog_demo_packages (
    package text NOT NULL,
    version integer NOT NULL,
    price_mode text NOT NULL,
    installed_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT catalog_demo_packages_package_check CHECK (((char_length(package) >= 1) AND (char_length(package) <= 80))),
    CONSTRAINT catalog_demo_packages_price_mode_check CHECK ((price_mode = ANY (ARRAY['inclusive'::text, 'exclusive'::text]))),
    CONSTRAINT catalog_demo_packages_version_check CHECK ((version > 0))
);

CREATE TABLE pricing_settings (
    id smallint NOT NULL,
    price_mode text DEFAULT 'inclusive'::text NOT NULL,
    default_class_id bigint,
    country_code text,
    CONSTRAINT pricing_settings_id_check CHECK ((id = 1)),
    CONSTRAINT pricing_settings_price_mode_check CHECK ((price_mode = ANY (ARRAY['inclusive'::text, 'exclusive'::text])))
);

CREATE TABLE store_settings (
    id smallint NOT NULL,
    name text NOT NULL,
    currency text DEFAULT 'EUR'::text NOT NULL,
    locale text DEFAULT 'it'::text NOT NULL,
    legal_name text DEFAULT ''::text NOT NULL,
    contact_email text DEFAULT ''::text NOT NULL,
    phone text DEFAULT ''::text NOT NULL,
    address_line1 text DEFAULT ''::text NOT NULL,
    address_line2 text DEFAULT ''::text NOT NULL,
    postal_code text DEFAULT ''::text NOT NULL,
    city text DEFAULT ''::text NOT NULL,
    province text DEFAULT ''::text NOT NULL,
    country text DEFAULT ''::text NOT NULL,
    logo_key text,
    logo_version bigint DEFAULT 0 NOT NULL,
    favicon_key text,
    favicon_version bigint DEFAULT 0 NOT NULL,
    CONSTRAINT store_settings_address_line1_check CHECK (((char_length(address_line1) <= 160) AND (address_line1 = btrim(address_line1)) AND (address_line1 !~ '[[:cntrl:]]'::text))),
    CONSTRAINT store_settings_address_line2_check CHECK (((char_length(address_line2) <= 160) AND (address_line2 = btrim(address_line2)) AND (address_line2 !~ '[[:cntrl:]]'::text))),
    CONSTRAINT store_settings_city_check CHECK (((char_length(city) <= 80) AND (city = btrim(city)) AND (city !~ '[[:cntrl:]]'::text))),
    CONSTRAINT store_settings_contact_email_check CHECK (((char_length(contact_email) <= 254) AND (contact_email = btrim(contact_email)) AND (contact_email !~ '[[:cntrl:]]'::text) AND ((contact_email = ''::text) OR ((contact_email = lower(contact_email)) AND (contact_email ~ '^[^[:space:]@]+@[^[:space:]@]+\.[^[:space:]@]+$'::text))))),
    CONSTRAINT store_settings_country_check CHECK (((char_length(country) <= 80) AND (country = btrim(country)) AND (country !~ '[[:cntrl:]]'::text))),
    CONSTRAINT store_settings_currency_check CHECK ((currency = ANY (ARRAY['EUR'::text, 'USD'::text, 'GBP'::text, 'CHF'::text, 'CAD'::text, 'AUD'::text, 'NZD'::text, 'SEK'::text, 'NOK'::text, 'DKK'::text, 'PLN'::text, 'CZK'::text, 'RON'::text]))),
    CONSTRAINT store_settings_favicon_version_check CHECK ((favicon_version >= 0)),
    CONSTRAINT store_settings_id_check CHECK ((id = 1)),
    CONSTRAINT store_settings_legal_name_check CHECK (((char_length(legal_name) <= 160) AND (legal_name = btrim(legal_name)) AND (legal_name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT store_settings_locale_check CHECK ((locale = 'it'::text)),
    CONSTRAINT store_settings_logo_version_check CHECK ((logo_version >= 0)),
    CONSTRAINT store_settings_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 80))),
    CONSTRAINT store_settings_phone_check CHECK (((char_length(phone) <= 40) AND (phone = btrim(phone)) AND (phone !~ '[[:cntrl:]]'::text))),
    CONSTRAINT store_settings_postal_code_check CHECK (((char_length(postal_code) <= 20) AND (postal_code = btrim(postal_code)) AND (postal_code !~ '[[:cntrl:]]'::text))),
    CONSTRAINT store_settings_province_check CHECK (((char_length(province) <= 80) AND (province = btrim(province)) AND (province !~ '[[:cntrl:]]'::text)))
);

CREATE VIEW catalog_effective_tax AS
 SELECT t.class_id,
    t.tax_name,
    t.basis_points,
    t.zone_name,
    t.explicit_rule,
    p.price_mode,
    ss.currency
   FROM ((pricing_settings p
     CROSS JOIN store_settings ss)
     CROSS JOIN LATERAL resolve_tax(p.country_code) t(class_id, tax_name, basis_points, zone_name, explicit_rule))
  WHERE ((p.id = 1) AND (ss.id = 1));

CREATE TABLE catalog_option_values (
    id bigint NOT NULL,
    option_id bigint NOT NULL,
    name text NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    CONSTRAINT catalog_option_values_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 80)) AND (name = btrim(name)))),
    CONSTRAINT catalog_option_values_sort_order_check CHECK (((sort_order >= 0) AND (sort_order <= 9999)))
);

ALTER TABLE catalog_option_values ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME catalog_option_values_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE catalog_option_values_translations (
    entity_id bigint NOT NULL,
    locale text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    option_id bigint NOT NULL,
    CONSTRAINT catalog_option_values_translations_description_check CHECK (((char_length(description) <= 0) AND (description = btrim(description)) AND (description !~ '[\x01-\x08\x0B\x0C\x0E-\x1F\x7F]'::text))),
    CONSTRAINT catalog_option_values_translations_name_check CHECK (((char_length(name) <= 80) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_option_values_translations_seo_description_check CHECK (((char_length(seo_description) <= 0) AND (seo_description = btrim(seo_description)) AND (seo_description !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_option_values_translations_seo_title_check CHECK (((char_length(seo_title) <= 0) AND (seo_title = btrim(seo_title)) AND (seo_title !~ '[[:cntrl:]]'::text)))
);

CREATE TABLE catalog_options (
    id bigint NOT NULL,
    name text NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    CONSTRAINT catalog_options_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 80)) AND (name = btrim(name)))),
    CONSTRAINT catalog_options_sort_order_check CHECK (((sort_order >= 0) AND (sort_order <= 9999)))
);

ALTER TABLE catalog_options ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME catalog_options_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE catalog_options_translations (
    entity_id bigint NOT NULL,
    locale text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    CONSTRAINT catalog_options_translations_description_check CHECK (((char_length(description) <= 0) AND (description = btrim(description)) AND (description !~ '[\x01-\x08\x0B\x0C\x0E-\x1F\x7F]'::text))),
    CONSTRAINT catalog_options_translations_name_check CHECK (((char_length(name) <= 80) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_options_translations_seo_description_check CHECK (((char_length(seo_description) <= 0) AND (seo_description = btrim(seo_description)) AND (seo_description !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_options_translations_seo_title_check CHECK (((char_length(seo_title) <= 0) AND (seo_title = btrim(seo_title)) AND (seo_title !~ '[[:cntrl:]]'::text)))
);

CREATE TABLE catalog_product_attributes (
    id bigint NOT NULL,
    product_id bigint NOT NULL,
    attribute_id bigint NOT NULL,
    name text NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT catalog_product_attributes_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 500)) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_product_attributes_revision_check CHECK ((revision >= 0)),
    CONSTRAINT catalog_product_attributes_sort_order_check CHECK (((sort_order >= 0) AND (sort_order <= 9999)))
);

ALTER TABLE catalog_product_attributes ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME catalog_product_attributes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE catalog_product_attributes_translations (
    entity_id bigint NOT NULL,
    locale text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    CONSTRAINT catalog_product_attributes_translations_description_check CHECK ((description = ''::text)),
    CONSTRAINT catalog_product_attributes_translations_name_check CHECK (((char_length(name) <= 500) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_product_attributes_translations_seo_description_check CHECK ((seo_description = ''::text)),
    CONSTRAINT catalog_product_attributes_translations_seo_title_check CHECK ((seo_title = ''::text))
);

CREATE TABLE catalog_product_images (
    id bigint NOT NULL,
    product_id bigint NOT NULL,
    image_key text NOT NULL,
    "position" integer NOT NULL,
    CONSTRAINT catalog_product_images_position_check CHECK ((("position" >= 1) AND ("position" <= 12)))
);

ALTER TABLE catalog_product_images ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME catalog_product_images_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE catalog_product_options (
    product_id bigint NOT NULL,
    "position" smallint NOT NULL,
    option_id bigint NOT NULL,
    CONSTRAINT catalog_product_options_position_check CHECK ((("position" >= 1) AND ("position" <= 3)))
);

CREATE TABLE catalog_products (
    id bigint NOT NULL,
    name text NOT NULL,
    model text NOT NULL,
    slug text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    price_cents bigint NOT NULL,
    stock integer DEFAULT 0 NOT NULL,
    manage_stock boolean DEFAULT true NOT NULL,
    status text DEFAULT 'draft'::text NOT NULL,
    category_id bigint,
    brand_id bigint,
    tax_class_id bigint,
    image_key text,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    minimum_quantity integer DEFAULT 1 NOT NULL,
    packs integer DEFAULT 1 NOT NULL,
    stock_revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT catalog_products_check CHECK (((status = 'draft'::text) OR (tax_class_id IS NOT NULL))),
    CONSTRAINT catalog_products_description_check CHECK ((char_length(description) <= 4000)),
    CONSTRAINT catalog_products_minimum_quantity_check CHECK (((minimum_quantity >= 1) AND (minimum_quantity <= 999999))),
    CONSTRAINT catalog_products_model_check CHECK ((((char_length(model) >= 1) AND (char_length(model) <= 64)) AND (model ~ '^[A-Z0-9][A-Z0-9._/+-]*$'::text))),
    CONSTRAINT catalog_products_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 120)) AND (name = btrim(name)))),
    CONSTRAINT catalog_products_packs_check CHECK (((packs >= 1) AND (packs <= 999999))),
    CONSTRAINT catalog_products_price_cents_check CHECK (((price_cents >= 0) AND (price_cents <= '999999999999'::bigint))),
    CONSTRAINT catalog_products_seo_description_check CHECK ((char_length(seo_description) <= 320)),
    CONSTRAINT catalog_products_seo_title_check CHECK ((char_length(seo_title) <= 120)),
    CONSTRAINT catalog_products_slug_check CHECK ((((char_length(slug) >= 1) AND (char_length(slug) <= 160)) AND (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$'::text))),
    CONSTRAINT catalog_products_status_check CHECK ((status = ANY (ARRAY['draft'::text, 'published'::text]))),
    CONSTRAINT catalog_products_stock_check CHECK (((stock >= 0) AND (stock <= 999999))),
    CONSTRAINT catalog_products_stock_revision_check CHECK ((stock_revision >= 0))
);

ALTER TABLE catalog_products ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME catalog_products_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE catalog_products_translations (
    entity_id bigint NOT NULL,
    locale text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    CONSTRAINT catalog_products_translations_description_check CHECK (((char_length(description) <= 4000) AND (description = btrim(description)) AND (description !~ '[\x01-\x08\x0B\x0C\x0E-\x1F\x7F]'::text))),
    CONSTRAINT catalog_products_translations_name_check CHECK (((char_length(name) <= 120) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_products_translations_seo_description_check CHECK (((char_length(seo_description) <= 320) AND (seo_description = btrim(seo_description)) AND (seo_description !~ '[[:cntrl:]]'::text))),
    CONSTRAINT catalog_products_translations_seo_title_check CHECK (((char_length(seo_title) <= 120) AND (seo_title = btrim(seo_title)) AND (seo_title !~ '[[:cntrl:]]'::text)))
);

CREATE TABLE catalog_variant_values (
    variant_id bigint NOT NULL,
    value_id bigint NOT NULL
);

CREATE TABLE catalog_variants (
    id bigint NOT NULL,
    product_id bigint NOT NULL,
    value_ids bigint[] NOT NULL,
    sku text NOT NULL,
    price_cents bigint NOT NULL,
    stock integer NOT NULL,
    active boolean DEFAULT true NOT NULL,
    image_key text,
    stock_revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT catalog_variants_price_cents_check CHECK (((price_cents >= 0) AND (price_cents <= '999999999999'::bigint))),
    CONSTRAINT catalog_variants_sku_check CHECK ((((char_length(sku) >= 1) AND (char_length(sku) <= 64)) AND (sku ~ '^[A-Z0-9][A-Z0-9._/+-]*$'::text))),
    CONSTRAINT catalog_variants_stock_check CHECK (((stock >= 0) AND (stock <= 999999))),
    CONSTRAINT catalog_variants_stock_revision_check CHECK ((stock_revision >= 0))
);

ALTER TABLE catalog_variants ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME catalog_variants_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE tax_classes (
    id bigint NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    rate_id bigint NOT NULL,
    active boolean DEFAULT true NOT NULL,
    CONSTRAINT tax_classes_description_check CHECK ((char_length(description) <= 500)),
    CONSTRAINT tax_classes_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 80)) AND (name = btrim(name))))
);

CREATE TABLE tax_rates (
    id bigint NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    basis_points integer NOT NULL,
    active boolean DEFAULT true NOT NULL,
    CONSTRAINT tax_rates_basis_points_check CHECK (((basis_points >= 0) AND (basis_points <= 10000))),
    CONSTRAINT tax_rates_description_check CHECK ((char_length(description) <= 500)),
    CONSTRAINT tax_rates_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 80)) AND (name = btrim(name))))
);

CREATE VIEW catalog_visible_products AS
 SELECT p.id,
    p.name,
    p.model,
    p.slug,
    p.description,
        CASE
            WHEN a.configured THEN v.price_cents
            ELSE p.price_cents
        END AS price_cents,
        CASE
            WHEN a.configured THEN v.stock
            ELSE p.stock
        END AS stock,
    p.status,
    p.category_id,
    p.brand_id,
    p.tax_class_id,
    p.image_key,
    c.name AS category_name,
    c.slug AS category_slug,
    b.name AS brand_name,
    COALESCE(e.tax_name, t.name) AS tax_name,
    COALESCE(e.basis_points, r.basis_points) AS basis_points,
    s.price_mode,
    a.configured AS has_variants,
    COALESCE(ss.currency, 'EUR'::text) AS currency
   FROM (((((((((catalog_products p
     LEFT JOIN catalog_categories c ON ((c.id = p.category_id)))
     JOIN tax_classes t ON (((t.id = p.tax_class_id) AND t.active)))
     JOIN tax_rates r ON (((r.id = t.rate_id) AND r.active)))
     CROSS JOIN pricing_settings s)
     LEFT JOIN store_settings ss ON ((ss.id = 1)))
     LEFT JOIN catalog_effective_tax e ON ((e.class_id = p.tax_class_id)))
     LEFT JOIN catalog_brands b ON ((b.id = p.brand_id)))
     CROSS JOIN LATERAL ( SELECT (EXISTS ( SELECT 1
                   FROM catalog_product_options
                  WHERE (catalog_product_options.product_id = p.id))) AS configured) a)
     LEFT JOIN LATERAL ( SELECT min(catalog_variants.price_cents) AS price_cents,
            max(catalog_variants.stock) AS stock
           FROM catalog_variants
          WHERE ((catalog_variants.product_id = p.id) AND catalog_variants.active)) v ON (true))
  WHERE ((s.id = 1) AND (p.status = 'published'::text) AND ((p.category_id IS NULL) OR c.active) AND ((p.brand_id IS NULL) OR b.active) AND ((NOT a.configured) OR (v.price_cents IS NOT NULL)));

CREATE TABLE checkout_countries (
    country_code text NOT NULL
);

CREATE TABLE checkout_details (
    cart_id bigint NOT NULL,
    email text NOT NULL,
    first_name text NOT NULL,
    last_name text NOT NULL,
    phone text NOT NULL,
    recipient_first_name text NOT NULL,
    recipient_last_name text NOT NULL,
    shipping_company text NOT NULL,
    shipping_country text NOT NULL,
    shipping_street text NOT NULL,
    shipping_number text NOT NULL,
    shipping_extra text NOT NULL,
    shipping_postal text NOT NULL,
    shipping_city text NOT NULL,
    shipping_region text NOT NULL,
    delivery_notes text NOT NULL,
    customer_type text NOT NULL,
    billing_first_name text NOT NULL,
    billing_last_name text NOT NULL,
    billing_company text NOT NULL,
    billing_country text NOT NULL,
    billing_street text NOT NULL,
    billing_number text NOT NULL,
    billing_extra text NOT NULL,
    billing_postal text NOT NULL,
    billing_city text NOT NULL,
    billing_region text NOT NULL,
    vat_id text NOT NULL,
    tax_id text NOT NULL,
    invoice_channel text NOT NULL,
    recipient_code text NOT NULL,
    pec text NOT NULL,
    invoice_requested boolean NOT NULL,
    billing_same boolean NOT NULL,
    CONSTRAINT checkout_details_billing_city_check CHECK ((length(billing_city) <= 100)),
    CONSTRAINT checkout_details_billing_company_check CHECK ((length(billing_company) <= 160)),
    CONSTRAINT checkout_details_billing_country_check CHECK ((length(billing_country) <= 2)),
    CONSTRAINT checkout_details_billing_extra_check CHECK ((length(billing_extra) <= 160)),
    CONSTRAINT checkout_details_billing_first_name_check CHECK ((length(billing_first_name) <= 100)),
    CONSTRAINT checkout_details_billing_last_name_check CHECK ((length(billing_last_name) <= 100)),
    CONSTRAINT checkout_details_billing_number_check CHECK ((length(billing_number) <= 20)),
    CONSTRAINT checkout_details_billing_postal_check CHECK ((length(billing_postal) <= 20)),
    CONSTRAINT checkout_details_billing_region_check CHECK ((length(billing_region) <= 100)),
    CONSTRAINT checkout_details_billing_street_check CHECK ((length(billing_street) <= 160)),
    CONSTRAINT checkout_details_check CHECK (((customer_type <> 'business'::text) OR invoice_requested)),
    CONSTRAINT checkout_details_customer_type_check CHECK ((length(customer_type) <= 12)),
    CONSTRAINT checkout_details_customer_type_check1 CHECK ((customer_type = ANY (ARRAY['private'::text, 'business'::text]))),
    CONSTRAINT checkout_details_delivery_notes_check CHECK ((length(delivery_notes) <= 1000)),
    CONSTRAINT checkout_details_email_check CHECK ((length(email) <= 254)),
    CONSTRAINT checkout_details_first_name_check CHECK ((length(first_name) <= 100)),
    CONSTRAINT checkout_details_invoice_channel_check CHECK ((length(invoice_channel) <= 8)),
    CONSTRAINT checkout_details_invoice_channel_check1 CHECK ((invoice_channel = ANY (ARRAY['none'::text, 'code'::text, 'pec'::text, 'both'::text]))),
    CONSTRAINT checkout_details_last_name_check CHECK ((length(last_name) <= 100)),
    CONSTRAINT checkout_details_pec_check CHECK ((length(pec) <= 254)),
    CONSTRAINT checkout_details_phone_check CHECK ((length(phone) <= 40)),
    CONSTRAINT checkout_details_recipient_code_check CHECK ((length(recipient_code) <= 7)),
    CONSTRAINT checkout_details_recipient_first_name_check CHECK ((length(recipient_first_name) <= 100)),
    CONSTRAINT checkout_details_recipient_last_name_check CHECK ((length(recipient_last_name) <= 100)),
    CONSTRAINT checkout_details_shipping_city_check CHECK ((length(shipping_city) <= 100)),
    CONSTRAINT checkout_details_shipping_company_check CHECK ((length(shipping_company) <= 160)),
    CONSTRAINT checkout_details_shipping_country_check CHECK ((length(shipping_country) <= 2)),
    CONSTRAINT checkout_details_shipping_extra_check CHECK ((length(shipping_extra) <= 160)),
    CONSTRAINT checkout_details_shipping_number_check CHECK ((length(shipping_number) <= 20)),
    CONSTRAINT checkout_details_shipping_postal_check CHECK ((length(shipping_postal) <= 20)),
    CONSTRAINT checkout_details_shipping_region_check CHECK ((length(shipping_region) <= 100)),
    CONSTRAINT checkout_details_shipping_street_check CHECK ((length(shipping_street) <= 160)),
    CONSTRAINT checkout_details_tax_id_check CHECK ((length(tax_id) <= 40)),
    CONSTRAINT checkout_details_vat_id_check CHECK ((length(vat_id) <= 40))
);

CREATE TABLE checkout_methods (
    cart_id bigint NOT NULL,
    shipping_method_id bigint,
    payment_method_id bigint
);

CREATE TABLE countries (
    code text NOT NULL,
    CONSTRAINT tax_countries_code_check CHECK ((code ~ '^[A-Z]{2}$'::text))
);

CREATE TABLE coupon_uses (
    order_id bigint NOT NULL,
    coupon_id bigint NOT NULL,
    discount_cents bigint NOT NULL,
    CONSTRAINT coupon_uses_discount_cents_check CHECK ((discount_cents >= 0))
);

CREATE TABLE coupons (
    id bigint NOT NULL,
    code text NOT NULL,
    kind text NOT NULL,
    value bigint NOT NULL,
    currency text NOT NULL,
    active boolean DEFAULT false NOT NULL,
    expires_on date,
    usage_limit bigint,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT coupons_check CHECK (((value > 0) AND (value <= '999999999999'::bigint) AND ((kind <> 'percent'::text) OR (value <= 10000)))),
    CONSTRAINT coupons_code_check CHECK ((code ~ '^[A-Z0-9][A-Z0-9_-]{0,39}$'::text)),
    CONSTRAINT coupons_currency_check CHECK ((currency ~ '^[A-Z]{3}$'::text)),
    CONSTRAINT coupons_expires_on_check CHECK (((expires_on >= '2000-01-01'::date) AND (expires_on <= '9999-12-31'::date))),
    CONSTRAINT coupons_kind_check CHECK ((kind = ANY (ARRAY['fixed'::text, 'percent'::text]))),
    CONSTRAINT coupons_revision_check CHECK ((revision >= 0)),
    CONSTRAINT coupons_usage_limit_check CHECK (((usage_limit >= 1) AND (usage_limit <= 1000000000)))
);

ALTER TABLE coupons ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME coupons_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE customer_addresses (
    id bigint NOT NULL,
    customer_id bigint NOT NULL,
    is_default boolean DEFAULT false NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    label text NOT NULL,
    first_name text NOT NULL,
    last_name text NOT NULL,
    company text NOT NULL,
    phone text NOT NULL,
    country text NOT NULL,
    street text NOT NULL,
    number text NOT NULL,
    extra text NOT NULL,
    postal text NOT NULL,
    city text NOT NULL,
    region text NOT NULL,
    CONSTRAINT customer_addresses_city_check CHECK ((char_length(city) <= 100)),
    CONSTRAINT customer_addresses_company_check CHECK ((char_length(company) <= 160)),
    CONSTRAINT customer_addresses_extra_check CHECK ((char_length(extra) <= 160)),
    CONSTRAINT customer_addresses_first_name_check CHECK ((char_length(first_name) <= 100)),
    CONSTRAINT customer_addresses_label_check CHECK (((char_length(label) >= 1) AND (char_length(label) <= 80))),
    CONSTRAINT customer_addresses_last_name_check CHECK ((char_length(last_name) <= 100)),
    CONSTRAINT customer_addresses_number_check CHECK ((char_length(number) <= 20)),
    CONSTRAINT customer_addresses_phone_check CHECK ((char_length(phone) <= 40)),
    CONSTRAINT customer_addresses_postal_check CHECK ((char_length(postal) <= 20)),
    CONSTRAINT customer_addresses_region_check CHECK ((char_length(region) <= 100)),
    CONSTRAINT customer_addresses_revision_check CHECK ((revision >= 0)),
    CONSTRAINT customer_addresses_street_check CHECK ((char_length(street) <= 160))
);

ALTER TABLE customer_addresses ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME customer_addresses_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE SEQUENCE customer_billing_revision
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

CREATE TABLE customer_billing (
    customer_id bigint NOT NULL,
    revision bigint DEFAULT nextval('customer_billing_revision'::regclass) NOT NULL,
    invoice_requested boolean DEFAULT false NOT NULL,
    billing_first_name text NOT NULL,
    billing_last_name text NOT NULL,
    billing_company text NOT NULL,
    billing_country text NOT NULL,
    billing_street text NOT NULL,
    billing_number text NOT NULL,
    billing_extra text NOT NULL,
    billing_postal text NOT NULL,
    billing_city text NOT NULL,
    billing_region text NOT NULL,
    vat_id text NOT NULL,
    tax_id text NOT NULL,
    recipient_code text NOT NULL,
    pec text NOT NULL,
    CONSTRAINT customer_billing_billing_city_check CHECK ((char_length(billing_city) <= 100)),
    CONSTRAINT customer_billing_billing_company_check CHECK ((char_length(billing_company) <= 160)),
    CONSTRAINT customer_billing_billing_extra_check CHECK ((char_length(billing_extra) <= 160)),
    CONSTRAINT customer_billing_billing_first_name_check CHECK ((char_length(billing_first_name) <= 100)),
    CONSTRAINT customer_billing_billing_last_name_check CHECK ((char_length(billing_last_name) <= 100)),
    CONSTRAINT customer_billing_billing_number_check CHECK ((char_length(billing_number) <= 20)),
    CONSTRAINT customer_billing_billing_postal_check CHECK ((char_length(billing_postal) <= 20)),
    CONSTRAINT customer_billing_billing_region_check CHECK ((char_length(billing_region) <= 100)),
    CONSTRAINT customer_billing_billing_street_check CHECK ((char_length(billing_street) <= 160)),
    CONSTRAINT customer_billing_pec_check CHECK ((char_length(pec) <= 254)),
    CONSTRAINT customer_billing_recipient_code_check CHECK ((char_length(recipient_code) <= 7)),
    CONSTRAINT customer_billing_revision_check CHECK ((revision >= 0)),
    CONSTRAINT customer_billing_tax_id_check CHECK ((char_length(tax_id) <= 40)),
    CONSTRAINT customer_billing_vat_id_check CHECK ((char_length(vat_id) <= 40))
);

CREATE TABLE customer_email_confirmations (
    customer_id bigint NOT NULL,
    email text NOT NULL,
    token_hash text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    last_sent_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT customer_email_confirmations_token_hash_check CHECK ((token_hash ~ '^[a-f0-9]{64}$'::text))
);

CREATE TABLE customer_field_policy (
    singleton boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    overrides jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT customer_field_policy_overrides_check CHECK (((jsonb_typeof(overrides) = 'object'::text) AND customer_field_policy_valid(overrides))),
    CONSTRAINT customer_field_policy_revision_check CHECK ((revision >= 0)),
    CONSTRAINT customer_field_policy_singleton_check CHECK (singleton)
);

CREATE TABLE customer_group_configuration (
    id smallint NOT NULL,
    default_group_id bigint NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT customer_group_configuration_id_check CHECK ((id = 1)),
    CONSTRAINT customer_group_configuration_revision_check CHECK ((revision >= 0))
);

CREATE TABLE customer_group_quantity_tiers (
    group_id bigint NOT NULL,
    "position" smallint NOT NULL,
    min_quantity bigint NOT NULL,
    discount_basis_points integer NOT NULL,
    CONSTRAINT customer_group_quantity_tiers_discount_basis_points_check CHECK (((discount_basis_points >= 1) AND (discount_basis_points <= 10000))),
    CONSTRAINT customer_group_quantity_tiers_min_quantity_check CHECK (((min_quantity >= 1) AND (min_quantity <= 99999900))),
    CONSTRAINT customer_group_quantity_tiers_position_check CHECK ((("position" >= 0) AND ("position" <= 19)))
);

CREATE TABLE customer_groups (
    id bigint NOT NULL,
    name_it text NOT NULL,
    name_en text NOT NULL,
    quantity_discounts_enabled boolean DEFAULT false NOT NULL,
    quantity_discounts_for_guests boolean DEFAULT false NOT NULL,
    CONSTRAINT customer_groups_name_en_check CHECK (((char_length(btrim(name_en)) >= 1) AND (char_length(btrim(name_en)) <= 120))),
    CONSTRAINT customer_groups_name_it_check CHECK (((char_length(btrim(name_it)) >= 1) AND (char_length(btrim(name_it)) <= 120)))
);

ALTER TABLE customer_groups ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME customer_groups_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE customer_orders (
    order_id bigint NOT NULL,
    customer_id bigint NOT NULL
);

CREATE TABLE customer_password_resets (
    token_hash text NOT NULL,
    customer_id bigint NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT customer_password_resets_token_hash_check CHECK ((token_hash ~ '^[a-f0-9]{64}$'::text))
);

CREATE TABLE customer_registration_settings (
    singleton boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    require_email_confirmation boolean DEFAULT false NOT NULL,
    CONSTRAINT customer_registration_settings_revision_check CHECK ((revision >= 0)),
    CONSTRAINT customer_registration_settings_singleton_check CHECK (singleton)
);

CREATE TABLE customer_sessions (
    token_hash text NOT NULL,
    csrf text NOT NULL,
    customer_id bigint,
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT customer_sessions_csrf_check CHECK ((csrf ~ '^[a-f0-9]{64}$'::text)),
    CONSTRAINT customer_sessions_token_hash_check CHECK ((token_hash ~ '^[a-f0-9]{64}$'::text))
);

CREATE TABLE customers (
    id bigint NOT NULL,
    email text NOT NULL,
    password_hash text NOT NULL,
    first_name text NOT NULL,
    last_name text NOT NULL,
    active boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    group_id bigint NOT NULL,
    customer_type text DEFAULT 'private'::text NOT NULL,
    phone text DEFAULT ''::text NOT NULL,
    CONSTRAINT customers_customer_type_check CHECK ((customer_type = ANY (ARRAY['private'::text, 'business'::text]))),
    CONSTRAINT customers_email_check CHECK ((((length(email) >= 3) AND (length(email) <= 254)) AND (email = lower(email)))),
    CONSTRAINT customers_first_name_check CHECK ((char_length(first_name) <= 100)),
    CONSTRAINT customers_last_name_check CHECK ((char_length(last_name) <= 100)),
    CONSTRAINT customers_phone_check CHECK ((char_length(phone) <= 40)),
    CONSTRAINT customers_revision_check CHECK ((revision >= 0))
);

ALTER TABLE customers ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME customers_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE email_attempts (
    message_id bigint NOT NULL,
    attempt integer NOT NULL,
    started_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    finished_at timestamp with time zone,
    outcome text NOT NULL,
    CONSTRAINT email_attempts_attempt_check CHECK ((attempt > 0)),
    CONSTRAINT email_attempts_check CHECK (((outcome = 'sending'::text) = (finished_at IS NULL))),
    CONSTRAINT email_attempts_outcome_check CHECK ((outcome = ANY (ARRAY['sending'::text, 'accepted'::text, 'transient'::text, 'permanent'::text, 'timeout'::text, 'interrupted'::text, 'invalid'::text])))
);

CREATE TABLE email_configuration (
    id smallint NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    from_name text DEFAULT ''::text NOT NULL,
    from_email text DEFAULT ''::text NOT NULL,
    reply_to text DEFAULT ''::text NOT NULL,
    admin_recipients text[] DEFAULT '{}'::text[] NOT NULL,
    CONSTRAINT email_configuration_admin_recipients_check CHECK ((cardinality(admin_recipients) <= 10)),
    CONSTRAINT email_configuration_from_email_check CHECK (((length(from_email) <= 254) AND (from_email !~ '[\r\n]'::text))),
    CONSTRAINT email_configuration_from_name_check CHECK (((length(from_name) <= 120) AND (from_name !~ '[\r\n]'::text))),
    CONSTRAINT email_configuration_id_check CHECK ((id = 1)),
    CONSTRAINT email_configuration_reply_to_check CHECK (((length(reply_to) <= 254) AND (reply_to !~ '[\r\n]'::text))),
    CONSTRAINT email_configuration_revision_check CHECK ((revision >= 0))
);

CREATE TABLE email_event_audiences (
    event_key text NOT NULL,
    recipients text[] NOT NULL,
    CONSTRAINT email_event_audiences_event_key_check CHECK (((length(event_key) >= 1) AND (length(event_key) <= 200))),
    CONSTRAINT email_event_audiences_recipients_check CHECK ((cardinality(recipients) >= 1))
);

CREATE TABLE email_outbox (
    id bigint NOT NULL,
    event_key text NOT NULL,
    recipient text NOT NULL,
    locale text NOT NULL,
    template text NOT NULL,
    subject text NOT NULL,
    body_text text NOT NULL,
    body_html text NOT NULL,
    from_name text NOT NULL,
    from_email text NOT NULL,
    reply_to text NOT NULL,
    message_id text NOT NULL,
    content_hash bytea NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    cycle_attempts smallint DEFAULT 0 NOT NULL,
    available_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    accepted_at timestamp with time zone,
    lease_until timestamp with time zone,
    lease_token text,
    last_outcome text DEFAULT ''::text NOT NULL,
    order_id bigint,
    superseded boolean DEFAULT false NOT NULL,
    CONSTRAINT email_outbox_attempts_check CHECK ((attempts >= 0)),
    CONSTRAINT email_outbox_body_html_check CHECK (((octet_length(body_html) >= 1) AND (octet_length(body_html) <= 8388608))),
    CONSTRAINT email_outbox_body_text_check CHECK (((octet_length(body_text) >= 1) AND (octet_length(body_text) <= 786432))),
    CONSTRAINT email_outbox_check CHECK (((state = 'sending'::text) = ((lease_until IS NOT NULL) AND (lease_token IS NOT NULL)))),
    CONSTRAINT email_outbox_check1 CHECK (((state = 'accepted'::text) = (accepted_at IS NOT NULL))),
    CONSTRAINT email_outbox_content_hash_check CHECK ((octet_length(content_hash) = 32)),
    CONSTRAINT email_outbox_cycle_attempts_check CHECK (((cycle_attempts >= 0) AND (cycle_attempts <= 5))),
    CONSTRAINT email_outbox_event_key_check CHECK (((length(event_key) >= 1) AND (length(event_key) <= 200))),
    CONSTRAINT email_outbox_from_email_check CHECK ((((length(from_email) >= 3) AND (length(from_email) <= 254)) AND (from_email !~ '[\r\n]'::text))),
    CONSTRAINT email_outbox_from_name_check CHECK ((((length(from_name) >= 1) AND (length(from_name) <= 120)) AND (from_name !~ '[\r\n]'::text))),
    CONSTRAINT email_outbox_locale_check CHECK ((locale = ANY (ARRAY['it'::text, 'en'::text]))),
    CONSTRAINT email_outbox_recipient_check CHECK ((((length(recipient) >= 3) AND (length(recipient) <= 254)) AND (recipient = lower(recipient)) AND (recipient !~ '[\r\n]'::text))),
    CONSTRAINT email_outbox_reply_to_check CHECK (((length(reply_to) <= 254) AND (reply_to !~ '[\r\n]'::text))),
    CONSTRAINT email_outbox_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'sending'::text, 'accepted'::text, 'failed'::text, 'superseded'::text]))),
    CONSTRAINT email_outbox_subject_check CHECK ((((length(subject) >= 1) AND (length(subject) <= 300)) AND (subject !~ '[\r\n]'::text))),
    CONSTRAINT email_outbox_superseded_check CHECK (((state <> 'superseded'::text) OR superseded)),
    CONSTRAINT email_outbox_template_check CHECK ((template = ANY (ARRAY['order_received'::text, 'order_updated'::text, 'order_admin'::text, 'payment_received'::text, 'order_cancelled'::text, 'cancellation_admin'::text, 'payment_refunded'::text, 'order_shipped'::text, 'order_delivered'::text, 'order_tracking'::text, 'customer_confirmation'::text])))
);

ALTER TABLE email_outbox ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME email_outbox_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE email_transport (
    id smallint NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    host text DEFAULT '127.0.0.1'::text NOT NULL,
    port integer DEFAULT 1025 NOT NULL,
    security text DEFAULT 'local'::text NOT NULL,
    username text DEFAULT ''::text NOT NULL,
    password_ciphertext bytea DEFAULT '\x'::bytea NOT NULL,
    timeout_seconds integer DEFAULT 10 NOT NULL,
    CONSTRAINT email_transport_check CHECK (((username = ''::text) = (octet_length(password_ciphertext) = 0))),
    CONSTRAINT email_transport_check1 CHECK (((security <> 'local'::text) OR ((host = ANY (ARRAY['127.0.0.1'::text, '::1'::text])) AND (username = ''::text)))),
    CONSTRAINT email_transport_host_check CHECK ((((octet_length(host) >= 1) AND (octet_length(host) <= 253)) AND (host !~ '[[:space:]/@]'::text))),
    CONSTRAINT email_transport_id_check CHECK ((id = 1)),
    CONSTRAINT email_transport_password_ciphertext_check CHECK (((octet_length(password_ciphertext) = 0) OR ((octet_length(password_ciphertext) >= 30) AND (octet_length(password_ciphertext) <= 4125)))),
    CONSTRAINT email_transport_port_check CHECK (((port >= 1) AND (port <= 65535))),
    CONSTRAINT email_transport_revision_check CHECK ((revision >= 0)),
    CONSTRAINT email_transport_security_check CHECK ((security = ANY (ARRAY['local'::text, 'starttls'::text, 'tls'::text]))),
    CONSTRAINT email_transport_timeout_seconds_check CHECK (((timeout_seconds >= 1) AND (timeout_seconds <= 20))),
    CONSTRAINT email_transport_username_check CHECK ((octet_length(username) <= 320))
);

CREATE TABLE guest_cart_items (
    id bigint NOT NULL,
    cart_id bigint NOT NULL,
    item_key text NOT NULL,
    product_id bigint,
    variant_id bigint,
    had_variant boolean NOT NULL,
    quantity integer NOT NULL,
    name text NOT NULL,
    code text NOT NULL,
    unit_cents bigint NOT NULL,
    basis_points integer NOT NULL,
    price_mode text NOT NULL,
    currency text NOT NULL,
    CONSTRAINT guest_cart_items_basis_points_check CHECK (((basis_points >= 0) AND (basis_points <= 10000))),
    CONSTRAINT guest_cart_items_item_key_check CHECK ((length(item_key) <= 50)),
    CONSTRAINT guest_cart_items_name_check CHECK (((length(name) >= 1) AND (length(name) <= 120))),
    CONSTRAINT guest_cart_items_price_mode_check CHECK ((price_mode = ANY (ARRAY['inclusive'::text, 'exclusive'::text]))),
    CONSTRAINT guest_cart_items_quantity_check CHECK (((quantity >= 1) AND (quantity <= 999999))),
    CONSTRAINT guest_cart_items_unit_cents_check CHECK ((unit_cents >= 0))
);

ALTER TABLE guest_cart_items ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME guest_cart_items_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE guest_carts (
    id bigint NOT NULL,
    token_hash text NOT NULL,
    csrf text NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    expires_at timestamp with time zone DEFAULT (now() + '30 days'::interval) NOT NULL,
    customer_id bigint,
    coupon_id bigint,
    CONSTRAINT customer_cart_persistent CHECK (((customer_id IS NULL) OR (expires_at = 'infinity'::timestamp with time zone))),
    CONSTRAINT guest_carts_csrf_check CHECK ((csrf ~ '^[a-f0-9]{64}$'::text)),
    CONSTRAINT guest_carts_revision_check CHECK ((revision >= 0)),
    CONSTRAINT guest_carts_token_hash_check CHECK ((token_hash ~ '^[a-f0-9]{64}$'::text))
);

ALTER TABLE guest_carts ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME guest_carts_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE home_banners (
    id bigint NOT NULL,
    label text NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    image_key text NOT NULL,
    active boolean DEFAULT false NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT home_banners_description_check CHECK (((char_length(description) <= 2048) AND (description = btrim(description)) AND (description !~ '[[:cntrl:]]'::text))),
    CONSTRAINT home_banners_label_check CHECK ((((char_length(label) >= 1) AND (char_length(label) <= 120)) AND (label = btrim(label)) AND (label !~ '[[:cntrl:]]'::text))),
    CONSTRAINT home_banners_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 250)) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT home_banners_revision_check CHECK ((revision >= 0)),
    CONSTRAINT home_banners_sort_order_check CHECK (((sort_order >= 0) AND (sort_order <= 9999)))
);

ALTER TABLE home_banners ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME home_banners_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE home_banners_translations (
    entity_id bigint NOT NULL,
    locale text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    CONSTRAINT home_banners_translations_description_check CHECK (((char_length(description) <= 2048) AND (description = btrim(description)) AND (description !~ '[[:cntrl:]]'::text))),
    CONSTRAINT home_banners_translations_name_check CHECK (((char_length(name) <= 250) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT home_banners_translations_seo_description_check CHECK ((seo_description = ''::text)),
    CONSTRAINT home_banners_translations_seo_title_check CHECK ((seo_title = ''::text))
);

CREATE TABLE identity_state (
    id smallint NOT NULL,
    installed boolean DEFAULT false NOT NULL,
    CONSTRAINT identity_state_id_check CHECK ((id = 1))
);

CREATE TABLE information_page_slugs (
    slug text NOT NULL,
    page_id bigint NOT NULL,
    CONSTRAINT information_page_slugs_slug_check CHECK ((((length(slug) >= 1) AND (length(slug) <= 160)) AND (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$'::text)))
);

CREATE TABLE information_pages (
    id bigint NOT NULL,
    slug text NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    published boolean DEFAULT false NOT NULL,
    header_menu boolean DEFAULT false NOT NULL,
    footer_menu boolean DEFAULT false NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT information_pages_check CHECK (((NOT published) OR (description <> ''::text))),
    CONSTRAINT information_pages_description_check CHECK (((char_length(description) <= 20000) AND (description = btrim(description)) AND (description !~ '[\x01-\x08\x0B\x0C\x0E-\x1F\x7F]'::text))),
    CONSTRAINT information_pages_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 120)) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT information_pages_revision_check CHECK ((revision >= 0)),
    CONSTRAINT information_pages_seo_description_check CHECK (((char_length(seo_description) <= 320) AND (seo_description = btrim(seo_description)) AND (seo_description !~ '[[:cntrl:]]'::text))),
    CONSTRAINT information_pages_seo_title_check CHECK (((char_length(seo_title) <= 120) AND (seo_title = btrim(seo_title)) AND (seo_title !~ '[[:cntrl:]]'::text))),
    CONSTRAINT information_pages_slug_check CHECK ((((length(slug) >= 1) AND (length(slug) <= 160)) AND (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$'::text))),
    CONSTRAINT information_pages_sort_order_check CHECK (((sort_order >= '-999999'::integer) AND (sort_order <= 999999)))
);

ALTER TABLE information_pages ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME information_pages_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE information_pages_translations (
    entity_id bigint NOT NULL,
    locale text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    CONSTRAINT information_pages_translations_description_check CHECK (((char_length(description) <= 20000) AND (description = btrim(description)) AND (description !~ '[\x01-\x08\x0B\x0C\x0E-\x1F\x7F]'::text))),
    CONSTRAINT information_pages_translations_name_check CHECK (((char_length(name) <= 120) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT information_pages_translations_seo_description_check CHECK (((char_length(seo_description) <= 320) AND (seo_description = btrim(seo_description)) AND (seo_description !~ '[[:cntrl:]]'::text))),
    CONSTRAINT information_pages_translations_seo_title_check CHECK (((char_length(seo_title) <= 120) AND (seo_title = btrim(seo_title)) AND (seo_title !~ '[[:cntrl:]]'::text)))
);

CREATE VIEW media_usage AS
 SELECT catalog_products.image_key AS key,
    'Prodotto'::text AS kind,
    catalog_products.id AS resource_id,
    catalog_products.name AS label,
    ('/admin/catalog/products/'::text || catalog_products.id) AS href
   FROM catalog_products
  WHERE (catalog_products.image_key IS NOT NULL)
UNION ALL
 SELECT v.image_key AS key,
    'Variante'::text AS kind,
    v.id AS resource_id,
    v.sku AS label,
    ((('/admin/catalog/products/'::text || v.product_id) || '/variants/'::text) || v.id) AS href
   FROM catalog_variants v
  WHERE (v.image_key IS NOT NULL)
UNION ALL
 SELECT g.image_key AS key,
    'Galleria'::text AS kind,
    g.id AS resource_id,
    p.name AS label,
    (('/admin/catalog/products/'::text || p.id) || '?tab=image'::text) AS href
   FROM (catalog_product_images g
     JOIN catalog_products p ON ((p.id = g.product_id)))
UNION ALL
 SELECT catalog_brands.logo_key AS key,
    'Marchio'::text AS kind,
    catalog_brands.id AS resource_id,
    catalog_brands.name AS label,
    ('/admin/catalog/brands/'::text || catalog_brands.id) AS href
   FROM catalog_brands
  WHERE (catalog_brands.logo_key IS NOT NULL)
UNION ALL
 SELECT store_settings.logo_key AS key,
    'Logo negozio'::text AS kind,
    1 AS resource_id,
    store_settings.name AS label,
    '/admin/settings/appearance'::text AS href
   FROM store_settings
  WHERE (store_settings.logo_key IS NOT NULL)
UNION ALL
 SELECT store_settings.favicon_key AS key,
    'Favicon'::text AS kind,
    1 AS resource_id,
    store_settings.name AS label,
    '/admin/settings/appearance'::text AS href
   FROM store_settings
  WHERE (store_settings.favicon_key IS NOT NULL)
UNION ALL
 SELECT administrators.avatar_key AS key,
    'Avatar'::text AS kind,
    administrators.id AS resource_id,
    'Immagine personale'::text AS label,
    '/admin/account'::text AS href
   FROM administrators
  WHERE (administrators.avatar_key IS NOT NULL)
UNION ALL
 SELECT home_banners.image_key AS key,
    'Banner'::text AS kind,
    home_banners.id AS resource_id,
    home_banners.label,
    ('/admin/banners/'::text || home_banners.id) AS href
   FROM home_banners;

CREATE VIEW media_business_roots AS
 SELECT DISTINCT key
   FROM media_usage;

CREATE TABLE media_cache_jobs (
    id bigint NOT NULL,
    source_key text NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    lease_until timestamp with time zone,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    lease_token text,
    rebuild boolean DEFAULT false NOT NULL,
    CONSTRAINT media_cache_jobs_attempts_check CHECK (((attempts >= 0) AND (attempts <= 3))),
    CONSTRAINT media_cache_jobs_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'running'::text, 'complete'::text, 'failed'::text])))
);

ALTER TABLE media_cache_jobs ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME media_cache_jobs_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE media_folders (
    id bigint NOT NULL,
    parent_id bigint,
    name text NOT NULL,
    path text NOT NULL,
    owner_id bigint,
    protected boolean DEFAULT false NOT NULL,
    CONSTRAINT media_folders_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 255))),
    CONSTRAINT media_folders_path_check CHECK (((length(path) <= 1024) AND (path !~ '(^/|\\|(^|/)\.|//)'::text) AND (POSITION(('..'::text) IN (path)) = 0)))
);

ALTER TABLE media_folders ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME media_folders_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE media_library (
    key text NOT NULL,
    folder_id bigint NOT NULL,
    name text NOT NULL,
    owner_id bigint,
    trashed_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT media_library_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 255)) AND (name !~ '[/\\]'::text) AND (POSITION(('..'::text) IN (name)) = 0)))
);

CREATE TABLE media_renditions (
    source_key text NOT NULL,
    profile text NOT NULL,
    object_key text NOT NULL,
    CONSTRAINT media_renditions_profile_check CHECK ((profile = ANY (ARRAY['thumbnail'::text, 'card'::text, 'detail'::text, 'large'::text, 'logo'::text, 'favicon'::text, 'avatar'::text])))
);

CREATE VIEW media_roots AS
 SELECT media_business_roots.key
   FROM media_business_roots
UNION
 SELECT media_library.key
   FROM media_library;

CREATE TABLE media_transforms (
    source_key text NOT NULL,
    kind text NOT NULL,
    object_key text NOT NULL,
    CONSTRAINT media_transforms_kind_check CHECK ((kind = ANY (ARRAY['logo'::text, 'favicon'::text, 'avatar'::text])))
);

CREATE TABLE media_variants (
    source_key text NOT NULL,
    profile text NOT NULL,
    object_key text NOT NULL,
    CONSTRAINT media_variants_check CHECK ((source_key <> object_key)),
    CONSTRAINT media_variants_profile_check CHECK ((profile = ANY (ARRAY['thumbnail'::text, 'card'::text, 'detail'::text])))
);

CREATE VIEW media_live_keys AS
 SELECT media_roots.key
   FROM media_roots
UNION
 SELECT v.object_key AS key
   FROM (media_variants v
     JOIN media_roots r ON ((r.key = v.source_key)))
  WHERE (NOT (EXISTS ( SELECT 1
           FROM (media_transforms t
             JOIN media_renditions d ON (((d.source_key = t.source_key) AND (d.profile = t.kind))))
          WHERE ((t.object_key = v.source_key) AND (d.object_key <> t.object_key)))))
UNION
 SELECT v.object_key AS key
   FROM (media_renditions v
     JOIN media_roots r ON ((r.key = v.source_key)))
UNION
 SELECT v.object_key AS key
   FROM (media_transforms v
     JOIN media_roots r ON ((r.key = v.source_key)))
UNION
 SELECT v.object_key AS key
   FROM ((media_variants v
     JOIN media_renditions d ON ((d.object_key = v.source_key)))
     JOIN media_roots r ON ((r.key = d.source_key)));

CREATE TABLE media_locations (
    key text NOT NULL,
    path text NOT NULL,
    CONSTRAINT media_locations_path_check CHECK (((length(path) <= 1024) AND (path !~ '(^/|\\|(^|/)\.|//)'::text) AND (POSITION(('..'::text) IN (path)) = 0)))
);

CREATE TABLE media_objects (
    key text NOT NULL,
    content_type text NOT NULL,
    byte_size bigint NOT NULL,
    width integer NOT NULL,
    height integer NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    id bigint NOT NULL,
    public boolean DEFAULT false NOT NULL,
    variants_ready boolean DEFAULT false NOT NULL,
    unused_since timestamp with time zone DEFAULT now(),
    CONSTRAINT media_objects_byte_size_check CHECK ((byte_size >= 0)),
    CONSTRAINT media_objects_content_type_check CHECK ((content_type ~ '^[a-z]+/[A-Za-z0-9.+-]+$'::text)),
    CONSTRAINT media_objects_key_check CHECK (((((key ~ '^(brand/)?[A-Za-z0-9][A-Za-z0-9._-]{0,119}\.png$'::text) AND (length(key) <= 124)) OR ((key ~ '^[A-Za-z0-9][A-Za-z0-9_-]+/([A-Za-z0-9][A-Za-z0-9._ -]*/)*[A-Za-z0-9][A-Za-z0-9._ -]*\.(png|jpg|jpeg|webp|PNG|JPG|JPEG|WEBP)$'::text) AND (split_part(key, '/'::text, 1) <> ALL (ARRAY['objects'::text, 'brand'::text])) AND (length(key) <= 1024))) AND (POSITION(('..'::text) IN (key)) = 0))),
    CONSTRAINT media_objects_source_shape_check CHECK ((((content_type = ANY (ARRAY['image/png'::text, 'image/jpeg'::text, 'image/webp'::text])) AND (width > 0) AND (height > 0)) OR ((content_type <> ALL (ARRAY['image/png'::text, 'image/jpeg'::text, 'image/webp'::text])) AND (width = 0) AND (height = 0))))
);

ALTER TABLE media_objects ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME media_objects_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE media_retired_directories (
    path text NOT NULL,
    retired_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT media_retired_directories_path_check CHECK (((path <> ''::text) AND (length(path) <= 1024)))
);

CREATE TABLE media_retired_files (
    path text NOT NULL,
    retired_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE online_captures (
    attempt_id bigint NOT NULL,
    intent_id text NOT NULL,
    amount_cents bigint NOT NULL,
    currency text NOT NULL,
    recorded_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    provider_id text NOT NULL,
    CONSTRAINT online_captures_amount_cents_check CHECK ((amount_cents > 0))
);

CREATE TABLE online_payment_events (
    id text NOT NULL,
    kind text NOT NULL,
    payload_hash text NOT NULL,
    attempt_id bigint,
    received_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    provider_id text NOT NULL,
    CONSTRAINT online_payment_events_id_check CHECK ((((octet_length(id) >= 1) AND (octet_length(id) <= 255)) AND (id ~ '^[!-~]+$'::text))),
    CONSTRAINT online_payment_events_provider_id_check CHECK ((provider_id ~ '^[a-z][a-z0-9_-]{0,63}$'::text)),
    CONSTRAINT online_payment_events_kind_check CHECK (((length(kind) >= 1) AND (length(kind) <= 120))),
    CONSTRAINT online_payment_events_payload_hash_check CHECK ((payload_hash ~ '^[a-f0-9]{64}$'::text))
);

CREATE TABLE online_refunds (
    attempt_id bigint NOT NULL,
    refund_id text NOT NULL,
    amount_cents bigint NOT NULL,
    currency text NOT NULL,
    reason text NOT NULL,
    recorded_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    provider_id text NOT NULL,
    CONSTRAINT online_refunds_amount_cents_check CHECK ((amount_cents > 0)),
    CONSTRAINT online_refunds_reason_check CHECK ((reason = ANY (ARRAY['admin_cancel'::text, 'late'::text, 'duplicate'::text, 'external'::text]))),
    CONSTRAINT online_refunds_refund_id_check CHECK ((((octet_length(refund_id) >= 1) AND (octet_length(refund_id) <= 255)) AND (refund_id ~ '^[!-~]+$'::text)))
);

CREATE TABLE operation_events (
    id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT statement_timestamp() NOT NULL,
    area text NOT NULL,
    resource text NOT NULL,
    resource_key text NOT NULL,
    action text NOT NULL,
    administrator_id bigint,
    CONSTRAINT operation_events_action_check CHECK ((action = ANY (ARRAY['INSERT'::text, 'UPDATE'::text, 'DELETE'::text]))),
    CONSTRAINT operation_events_administrator_id_check CHECK ((administrator_id > 0)),
    CONSTRAINT operation_events_area_check CHECK ((area = ANY (ARRAY['orders'::text, 'catalog'::text, 'pages'::text, 'settings'::text, 'access'::text]))),
    CONSTRAINT operation_events_resource_check CHECK ((resource = ANY (ARRAY['orders'::text, 'order_manual_payments'::text, 'order_online_payments'::text, 'online_refunds'::text, 'order_fulfillment_events'::text, 'catalog_products'::text, 'catalog_variants'::text, 'catalog_categories'::text, 'catalog_brands'::text, 'information_pages'::text, 'information_pages_translations'::text, 'coupons'::text, 'shipping_methods'::text, 'payment_methods'::text, 'order_statuses'::text, 'store_settings'::text, 'storefront_settings'::text, 'platform_regional_settings'::text, 'email_configuration'::text, 'tax_rates'::text, 'tax_classes'::text, 'tax_zones'::text, 'administrators'::text, 'admin_roles'::text, 'admin_role_permissions'::text, 'catalog_products_translations'::text, 'catalog_categories_translations'::text, 'catalog_brands_translations'::text, 'tax_zone_countries'::text, 'tax_zone_rules'::text]))),
    CONSTRAINT operation_events_resource_key_check CHECK (((length(resource_key) >= 1) AND (length(resource_key) <= 160)))
);

ALTER TABLE operation_events ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME operation_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE order_current_status (
    order_id bigint NOT NULL,
    status_id bigint NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT order_current_status_revision_check CHECK ((revision >= 0))
);

CREATE TABLE order_customer_summaries (
    order_id bigint NOT NULL,
    first_name text NOT NULL,
    last_name text NOT NULL,
    email text NOT NULL
);

CREATE TABLE order_fulfillment (
    order_id bigint NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    carrier text DEFAULT ''::text NOT NULL,
    tracking_number text DEFAULT ''::text NOT NULL,
    tracking_url text DEFAULT ''::text NOT NULL,
    CONSTRAINT order_fulfillment_carrier_check CHECK (((length(carrier) <= 80) AND (carrier !~ '[[:cntrl:]]'::text))),
    CONSTRAINT order_fulfillment_check CHECK (((state = ANY (ARRAY['shipped'::text, 'delivered'::text])) = (carrier <> ''::text))),
    CONSTRAINT order_fulfillment_check1 CHECK (((tracking_url = ''::text) OR (tracking_number <> ''::text))),
    CONSTRAINT order_fulfillment_revision_check CHECK ((revision >= 0)),
    CONSTRAINT order_fulfillment_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'preparing'::text, 'shipped'::text, 'delivered'::text]))),
    CONSTRAINT order_fulfillment_tracking_number_check CHECK (((length(tracking_number) <= 120) AND (tracking_number !~ '[[:cntrl:]]'::text))),
    CONSTRAINT order_fulfillment_tracking_url_check CHECK (((length(tracking_url) <= 2048) AND ((tracking_url = ''::text) OR (tracking_url ~* '^https://'::text)) AND (tracking_url !~ '[[:cntrl:]]'::text)))
);

CREATE TABLE order_fulfillment_events (
    id bigint NOT NULL,
    order_id bigint NOT NULL,
    request_key text NOT NULL,
    payload text NOT NULL,
    kind text NOT NULL,
    body text DEFAULT ''::text NOT NULL,
    carrier text DEFAULT ''::text NOT NULL,
    tracking_number text DEFAULT ''::text NOT NULL,
    tracking_url text DEFAULT ''::text NOT NULL,
    administrator_id bigint NOT NULL,
    administrator_name text NOT NULL,
    created_at timestamp with time zone DEFAULT statement_timestamp() NOT NULL,
    CONSTRAINT order_fulfillment_events_body_check CHECK ((length(body) <= 2000)),
    CONSTRAINT order_fulfillment_events_check CHECK (((kind = 'note'::text) = (body <> ''::text))),
    CONSTRAINT order_fulfillment_events_check1 CHECK (((length(carrier) <= 80) AND (length(tracking_number) <= 120) AND (length(tracking_url) <= 2048))),
    CONSTRAINT order_fulfillment_events_kind_check CHECK ((kind = ANY (ARRAY['preparing'::text, 'shipped'::text, 'delivered'::text, 'tracking'::text, 'note'::text]))),
    CONSTRAINT order_fulfillment_events_payload_check CHECK ((octet_length(payload) <= 24000)),
    CONSTRAINT order_fulfillment_events_request_key_check CHECK ((request_key ~ '^[a-f0-9]{64}$'::text))
);

ALTER TABLE order_fulfillment_events ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME order_fulfillment_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE order_lines (
    active boolean NOT NULL DEFAULT true,
    id bigint NOT NULL,
    order_id bigint NOT NULL,
    product_id bigint NOT NULL,
    variant_id bigint,
    quantity integer NOT NULL,
    name text NOT NULL,
    label text NOT NULL,
    code text NOT NULL,
    unit_cents bigint NOT NULL,
    class_id bigint NOT NULL,
    tax_name text NOT NULL,
    basis_points integer NOT NULL,
    zone_name text,
    price_mode text NOT NULL,
    net_cents bigint NOT NULL,
    tax_cents bigint NOT NULL,
    gross_cents bigint NOT NULL,
    CONSTRAINT order_lines_basis_points_check CHECK (((basis_points >= 0) AND (basis_points <= 10000))),
    CONSTRAINT order_lines_check CHECK (((gross_cents)::numeric = ((net_cents)::numeric + (tax_cents)::numeric))),
    CONSTRAINT order_lines_gross_cents_check CHECK ((gross_cents >= 0)),
    CONSTRAINT order_lines_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 120))),
    CONSTRAINT order_lines_net_cents_check CHECK ((net_cents >= 0)),
    CONSTRAINT order_lines_price_mode_check CHECK ((price_mode = ANY (ARRAY['inclusive'::text, 'exclusive'::text]))),
    CONSTRAINT order_lines_quantity_check CHECK (((quantity >= 1) AND (quantity <= 999999))),
    CONSTRAINT order_lines_tax_cents_check CHECK ((tax_cents >= 0)),
    CONSTRAINT order_lines_unit_cents_check CHECK ((unit_cents >= 0))
);

ALTER TABLE order_lines ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME order_lines_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE order_manual_payments (
    order_id bigint NOT NULL,
    amount_cents bigint NOT NULL,
    currency text NOT NULL,
    method_code text NOT NULL,
    reference text NOT NULL,
    administrator_id bigint NOT NULL,
    administrator_name text NOT NULL,
    recorded_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT order_manual_payments_amount_cents_check CHECK ((amount_cents >= 0)),
    CONSTRAINT order_manual_payments_currency_check CHECK ((currency ~ '^[A-Z]{3}$'::text)),
    CONSTRAINT order_manual_payments_method_code_check CHECK ((method_code = ANY (ARRAY['bank_transfer'::text, 'cash_on_delivery'::text]))),
    CONSTRAINT order_manual_payments_reference_check CHECK ((((char_length(reference) >= 1) AND (char_length(reference) <= 160)) AND (reference = btrim(reference)) AND (reference !~ '[[:cntrl:]]'::text)))
);

CREATE TABLE order_online_payments (
    order_id bigint NOT NULL,
    attempt_id bigint NOT NULL,
    recorded_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL
);

CREATE TABLE order_reference_counters (
    year integer NOT NULL,
    last_number bigint NOT NULL,
    CONSTRAINT order_reference_counters_last_number_check CHECK ((last_number > 0)),
    CONSTRAINT order_reference_counters_year_check CHECK (((year >= 1) AND (year <= 9999)))
);

CREATE TABLE order_status_events (
    id bigint NOT NULL,
    order_id bigint NOT NULL,
    status_id bigint NOT NULL,
    revision bigint NOT NULL,
    created_at timestamp with time zone DEFAULT statement_timestamp() NOT NULL
);

ALTER TABLE order_status_events ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME order_status_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE order_status_roles (
    role text NOT NULL,
    status_id bigint NOT NULL,
    CONSTRAINT order_status_roles_role_check CHECK ((role = ANY (ARRAY['preparing'::text, 'shipped'::text, 'delivered'::text, 'cancelled'::text, 'refunded'::text])))
);

CREATE TABLE order_statuses (
    id bigint NOT NULL,
    name_it text NOT NULL,
    name_en text NOT NULL,
    color text NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    CONSTRAINT order_statuses_color_check CHECK ((color ~ '^#[0-9a-f]{6}$'::text)),
    CONSTRAINT order_statuses_name_en_check CHECK (((length(btrim(name_en)) >= 1) AND (length(btrim(name_en)) <= 120))),
    CONSTRAINT order_statuses_name_it_check CHECK (((length(btrim(name_it)) >= 1) AND (length(btrim(name_it)) <= 120))),
    CONSTRAINT order_statuses_sort_order_check CHECK (((sort_order >= 0) AND (sort_order <= 9999)))
);

ALTER TABLE order_statuses ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME order_statuses_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE order_stock_moves (
    line_id bigint NOT NULL,
    kind text NOT NULL,
    quantity integer NOT NULL,
    stock_after integer NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT order_stock_moves_kind_check CHECK ((kind = ANY (ARRAY['debit'::text, 'credit'::text]))),
    CONSTRAINT order_stock_moves_quantity_check CHECK (((quantity >= 1) AND (quantity <= 999999))),
    CONSTRAINT order_stock_moves_stock_after_check CHECK (((stock_after >= 0) AND (stock_after <= 999999)))
);

CREATE TABLE orders (
    id bigint NOT NULL,
    reference text NOT NULL,
    source_cart_id bigint,
    origin text NOT NULL DEFAULT 'storefront' CHECK (origin IN ('storefront','admin')),
    CONSTRAINT orders_origin_source CHECK ((origin='storefront' AND source_cart_id IS NOT NULL) OR (origin='admin' AND source_cart_id IS NULL)),
    source_revision bigint NOT NULL,
    owner_hash text NOT NULL,
    snapshot text NOT NULL,
    currency text NOT NULL,
    merchandise_cents bigint NOT NULL,
    shipping_cents bigint NOT NULL,
    fee_cents bigint NOT NULL,
    net_cents bigint NOT NULL,
    tax_cents bigint NOT NULL,
    total_cents bigint NOT NULL,
    state text DEFAULT 'open'::text NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    cancelled_at timestamp with time zone,
    payment_state text DEFAULT 'unpaid'::text NOT NULL,
    CONSTRAINT orders_check CHECK (((total_cents)::numeric = (((merchandise_cents)::numeric + (shipping_cents)::numeric) + (fee_cents)::numeric))),
    CONSTRAINT orders_check1 CHECK (((total_cents)::numeric = ((net_cents)::numeric + (tax_cents)::numeric))),
    CONSTRAINT orders_check2 CHECK (((state = 'cancelled'::text) = (cancelled_at IS NOT NULL))),
    CONSTRAINT orders_currency_check CHECK ((currency ~ '^[A-Z]{3}$'::text)),
    CONSTRAINT orders_fee_cents_check CHECK ((fee_cents >= 0)),
    CONSTRAINT orders_merchandise_cents_check CHECK ((merchandise_cents >= 0)),
    CONSTRAINT orders_net_cents_check CHECK ((net_cents >= 0)),
    CONSTRAINT orders_owner_hash_check CHECK ((owner_hash ~ '^[a-f0-9]{64}$'::text)),
    CONSTRAINT orders_payment_state_check CHECK ((payment_state = ANY (ARRAY['unpaid'::text, 'paid'::text, 'refunded'::text]))),
    CONSTRAINT orders_reference_check CHECK (((length(reference) >= 1) AND (length(reference) <= 80))),
    CONSTRAINT orders_revision_check CHECK ((revision >= 0)),
    CONSTRAINT orders_shipping_cents_check CHECK ((shipping_cents >= 0)),
    CONSTRAINT orders_snapshot_check CHECK (((octet_length(snapshot) >= 1) AND (octet_length(snapshot) <= 262144))),
    CONSTRAINT orders_source_revision_check CHECK ((source_revision >= 0)),
    CONSTRAINT orders_state_check CHECK ((state = ANY (ARRAY['open'::text, 'cancelled'::text]))),
    CONSTRAINT orders_tax_cents_check CHECK ((tax_cents >= 0)),
    CONSTRAINT orders_total_cents_check CHECK ((total_cents >= 0))
);

ALTER TABLE orders ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME orders_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE password_resets (
    token_hash text NOT NULL,
    administrator_id bigint NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

CREATE TABLE payment_attempts (
    id bigint NOT NULL,
    order_id bigint NOT NULL,
    request_key text NOT NULL,
    amount_cents bigint NOT NULL,
    currency text NOT NULL,
    title text NOT NULL,
    return_url text NOT NULL,
    locale text NOT NULL,
    session_id text,
    intent_id text,
    checkout_url text,
    refund_id text,
    state text DEFAULT 'creating'::text NOT NULL,
    action text DEFAULT 'create'::text NOT NULL,
    refund_reason text,
    refund_requested_at timestamp with time zone,
    error_count integer DEFAULT 0 NOT NULL,
    last_error text DEFAULT ''::text NOT NULL,
    next_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    lease_token text,
    lease_until timestamp with time zone,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    ui_mode text DEFAULT 'hosted'::text NOT NULL,
    provider_id text NOT NULL,
    CONSTRAINT payment_attempts_action_check CHECK ((action = ANY (ARRAY['create'::text, 'fetch'::text, 'refund'::text, 'refund_fetch'::text, 'none'::text]))),
    CONSTRAINT payment_attempts_amount_cents_check CHECK (((amount_cents >= 1) AND (amount_cents <= 99999999))),
    CONSTRAINT payment_attempts_check CHECK (((lease_token IS NULL) = (lease_until IS NULL))),
    CONSTRAINT payment_attempts_checkout_url_check CHECK ((octet_length(checkout_url) <= 4096)),
    CONSTRAINT payment_attempts_currency_check CHECK ((currency ~ '^[A-Z]{3}$'::text)),
    CONSTRAINT payment_attempts_error_count_check CHECK (((error_count >= 0) AND (error_count <= 5))),
    CONSTRAINT payment_attempts_intent_id_check CHECK ((((octet_length(intent_id) >= 1) AND (octet_length(intent_id) <= 255)) AND (intent_id ~ '^[!-~]+$'::text))),
    CONSTRAINT payment_attempts_last_error_check CHECK ((last_error = ANY (ARRAY[''::text, 'temporary'::text, 'rejected'::text, 'missing'::text, 'invalid'::text, 'retry_window'::text, 'partial_refund'::text]))),
    CONSTRAINT payment_attempts_locale_check CHECK ((locale = ANY (ARRAY['it'::text, 'en'::text]))),
    CONSTRAINT payment_attempts_provider_id_check CHECK ((provider_id ~ '^[a-z][a-z0-9_-]{0,63}$'::text)),
    CONSTRAINT payment_attempts_refund_id_check CHECK ((((octet_length(refund_id) >= 1) AND (octet_length(refund_id) <= 255)) AND (refund_id ~ '^[!-~]+$'::text))),
    CONSTRAINT payment_attempts_refund_reason_check CHECK ((refund_reason = ANY (ARRAY['admin_cancel'::text, 'late'::text, 'duplicate'::text, 'external'::text]))),
    CONSTRAINT payment_attempts_request_key_check CHECK ((request_key ~ '^[a-f0-9]{64}$'::text)),
    CONSTRAINT payment_attempts_return_url_check CHECK (((octet_length(return_url) >= 1) AND (octet_length(return_url) <= 4096))),
    CONSTRAINT payment_attempts_session_id_check CHECK ((((octet_length(session_id) >= 1) AND (octet_length(session_id) <= 255)) AND (session_id ~ '^[!-~]+$'::text))),
    CONSTRAINT payment_attempts_state_check CHECK ((state = ANY (ARRAY['creating'::text, 'open'::text, 'processing'::text, 'paid'::text, 'expired'::text, 'failed'::text, 'review'::text, 'refund_pending'::text, 'refunded'::text]))),
    CONSTRAINT payment_attempts_title_check CHECK (((char_length(title) >= 1) AND (char_length(title) <= 240))),
    CONSTRAINT payment_attempts_ui_mode_check CHECK ((ui_mode = ANY (ARRAY['hosted'::text, 'elements'::text])))
);

ALTER TABLE payment_attempts ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME payment_attempts_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE payment_configuration (
    id smallint NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT payment_configuration_id_check CHECK ((id = 1)),
    CONSTRAINT payment_configuration_revision_check CHECK ((revision >= 0))
);

CREATE TABLE payment_methods (
    id bigint NOT NULL,
    code text NOT NULL,
    name_it text NOT NULL,
    name_en text NOT NULL,
    instructions_it text DEFAULT ''::text NOT NULL,
    instructions_en text DEFAULT ''::text NOT NULL,
    active boolean DEFAULT false NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    beneficiary text DEFAULT ''::text NOT NULL,
    iban text DEFAULT ''::text NOT NULL,
    bic text DEFAULT ''::text NOT NULL,
    fee_kind text DEFAULT 'none'::text NOT NULL,
    fee_value bigint DEFAULT 0 NOT NULL,
    minimum_cents bigint DEFAULT 0 NOT NULL,
    initial_order_status_id bigint NOT NULL,
    tax_class_id bigint,
    CONSTRAINT payment_methods_beneficiary_check CHECK ((length(beneficiary) <= 200)),
    CONSTRAINT payment_methods_bic_check CHECK ((length(bic) <= 11)),
    CONSTRAINT payment_methods_charge_class CHECK (((NOT active) OR (fee_value = 0) OR (tax_class_id IS NOT NULL))),
    CONSTRAINT payment_methods_check CHECK ((((fee_kind = 'none'::text) AND (fee_value = 0)) OR (fee_kind = 'fixed'::text) OR ((fee_kind = 'percent'::text) AND (fee_value <= 10000)))),
    CONSTRAINT payment_methods_check1 CHECK (((NOT active) OR (code <> 'bank_transfer'::text) OR ((length(btrim(beneficiary)) > 0) AND (iban ~ '^[A-Z]{2}[0-9]{2}[A-Z0-9]{11,30}$'::text)))),
    CONSTRAINT payment_methods_check2 CHECK (((code = 'bank_transfer'::text) OR ((beneficiary = ''::text) AND (iban = ''::text) AND (bic = ''::text)))),
    CONSTRAINT payment_methods_code_check CHECK ((code = ANY (ARRAY['bank_transfer'::text, 'cash_on_delivery'::text, 'online_checkout'::text]))),
    CONSTRAINT payment_methods_fee_kind_check CHECK ((fee_kind = ANY (ARRAY['none'::text, 'fixed'::text, 'percent'::text]))),
    CONSTRAINT payment_methods_fee_value_check CHECK ((fee_value >= 0)),
    CONSTRAINT payment_methods_iban_check CHECK ((length(iban) <= 34)),
    CONSTRAINT payment_methods_instructions_en_check CHECK ((length(instructions_en) <= 4000)),
    CONSTRAINT payment_methods_instructions_it_check CHECK ((length(instructions_it) <= 4000)),
    CONSTRAINT payment_methods_minimum_cents_check CHECK ((minimum_cents >= 0)),
    CONSTRAINT payment_methods_name_en_check CHECK (((length(btrim(name_en)) >= 1) AND (length(btrim(name_en)) <= 120))),
    CONSTRAINT payment_methods_name_it_check CHECK (((length(btrim(name_it)) >= 1) AND (length(btrim(name_it)) <= 120))),
    CONSTRAINT payment_methods_sort_order_check CHECK (((sort_order >= 0) AND (sort_order <= 9999)))
);

ALTER TABLE payment_methods ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME payment_methods_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE platform_metadata (
    id smallint NOT NULL,
    installed_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT platform_metadata_id_check CHECK ((id = 1))
);

CREATE TABLE platform_regional_settings (
    singleton boolean DEFAULT true NOT NULL,
    time_zone text DEFAULT 'Europe/Rome'::text NOT NULL,
    date_format text DEFAULT 'dmy'::text NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT platform_regional_settings_date_format_check CHECK ((date_format = ANY (ARRAY['dmy'::text, 'ymd'::text, 'mdy'::text]))),
    CONSTRAINT platform_regional_settings_revision_check CHECK ((revision >= 0)),
    CONSTRAINT platform_regional_settings_singleton_check CHECK (singleton),
    CONSTRAINT platform_regional_settings_time_zone_check CHECK (((length(time_zone) <= 100) AND platform_time_zone_valid(time_zone)))
);

CREATE TABLE shipping_configuration (
    id smallint NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT shipping_configuration_id_check CHECK ((id = 1)),
    CONSTRAINT shipping_configuration_revision_check CHECK ((revision >= 0))
);

CREATE TABLE shipping_method_countries (
    method_id bigint NOT NULL,
    country_code text NOT NULL
);

CREATE TABLE shipping_methods (
    id bigint NOT NULL,
    code text NOT NULL,
    name_it text NOT NULL,
    name_en text NOT NULL,
    instructions_it text DEFAULT ''::text NOT NULL,
    instructions_en text DEFAULT ''::text NOT NULL,
    active boolean DEFAULT false NOT NULL,
    delivery_days integer,
    sort_order integer DEFAULT 0 NOT NULL,
    fee_kind text DEFAULT 'fixed'::text NOT NULL,
    fee_value bigint DEFAULT 0 NOT NULL,
    free_from_cents bigint DEFAULT 0 NOT NULL,
    tax_class_id bigint,
    CONSTRAINT shipping_methods_charge_class CHECK (((NOT active) OR (fee_value = 0) OR (tax_class_id IS NOT NULL))),
    CONSTRAINT shipping_methods_check CHECK (((fee_kind <> 'percent'::text) OR (fee_value <= 10000))),
    CONSTRAINT shipping_methods_check1 CHECK (((code <> 'free_shipping'::text) OR ((fee_kind = 'fixed'::text) AND (fee_value = 0) AND (free_from_cents = 0)))),
    CONSTRAINT shipping_methods_code_check CHECK ((code = ANY (ARRAY['national_express'::text, 'free_shipping'::text]))),
    CONSTRAINT shipping_methods_delivery_days_check CHECK (((delivery_days >= 0) AND (delivery_days <= 365))),
    CONSTRAINT shipping_methods_fee_kind_check CHECK ((fee_kind = ANY (ARRAY['fixed'::text, 'percent'::text]))),
    CONSTRAINT shipping_methods_fee_value_check CHECK ((fee_value >= 0)),
    CONSTRAINT shipping_methods_free_class CHECK (((code <> 'free_shipping'::text) OR (tax_class_id IS NULL))),
    CONSTRAINT shipping_methods_free_from_cents_check CHECK ((free_from_cents >= 0)),
    CONSTRAINT shipping_methods_instructions_en_check CHECK ((length(instructions_en) <= 4000)),
    CONSTRAINT shipping_methods_instructions_it_check CHECK ((length(instructions_it) <= 4000)),
    CONSTRAINT shipping_methods_name_en_check CHECK (((length(btrim(name_en)) >= 1) AND (length(btrim(name_en)) <= 120))),
    CONSTRAINT shipping_methods_name_it_check CHECK (((length(btrim(name_it)) >= 1) AND (length(btrim(name_it)) <= 120))),
    CONSTRAINT shipping_methods_sort_order_check CHECK (((sort_order >= 0) AND (sort_order <= 9999)))
);

ALTER TABLE shipping_methods ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME shipping_methods_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE storefront_language_prefixes (
    code text NOT NULL
);

CREATE TABLE storefront_languages (
    code text NOT NULL
);

CREATE TABLE storefront_settings (
    id smallint DEFAULT 1 NOT NULL,
    palette text DEFAULT 'ocean'::text NOT NULL,
    typography text DEFAULT 'modern'::text NOT NULL,
    density text DEFAULT 'compact'::text NOT NULL,
    show_hero boolean DEFAULT true NOT NULL,
    show_featured boolean DEFAULT true NOT NULL,
    heading text DEFAULT 'Le cose fatte bene.'::text NOT NULL,
    introduction text DEFAULT 'Scopri i prodotti del nostro negozio.'::text NOT NULL,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    public_origin text DEFAULT ''::text NOT NULL,
    primary_locale text DEFAULT 'it'::text NOT NULL,
    theme_id text DEFAULT 'default'::text NOT NULL,
    maintenance_enabled boolean DEFAULT false NOT NULL,
    indexing_enabled boolean DEFAULT false NOT NULL,
    CONSTRAINT storefront_settings_density_check CHECK ((density = ANY (ARRAY['comfortable'::text, 'compact'::text]))),
    CONSTRAINT storefront_settings_heading_check CHECK (((char_length(heading) >= 1) AND (char_length(heading) <= 120))),
    CONSTRAINT storefront_settings_id_check CHECK ((id = 1)),
    CONSTRAINT storefront_settings_introduction_check CHECK ((char_length(introduction) <= 500)),
    CONSTRAINT storefront_settings_palette_check CHECK ((palette = ANY (ARRAY['forest'::text, 'ocean'::text, 'clay'::text]))),
    CONSTRAINT storefront_settings_public_origin_check CHECK ((char_length(public_origin) <= 255)),
    CONSTRAINT storefront_settings_seo_description_check CHECK ((char_length(seo_description) <= 320)),
    CONSTRAINT storefront_settings_seo_title_check CHECK ((char_length(seo_title) <= 120)),
    CONSTRAINT storefront_settings_theme_id_check CHECK ((theme_id = ANY (ARRAY['default'::text, 'essential'::text]))),
    CONSTRAINT storefront_settings_typography_check CHECK ((typography = ANY (ARRAY['editorial'::text, 'modern'::text])))
);

CREATE TABLE storefront_settings_translations (
    entity_id smallint NOT NULL,
    locale text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    seo_title text DEFAULT ''::text NOT NULL,
    seo_description text DEFAULT ''::text NOT NULL,
    CONSTRAINT storefront_settings_translations_description_check CHECK (((char_length(description) <= 500) AND (description = btrim(description)) AND (description !~ '[\x01-\x08\x0B\x0C\x0E-\x1F\x7F]'::text))),
    CONSTRAINT storefront_settings_translations_name_check CHECK (((char_length(name) <= 120) AND (name = btrim(name)) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT storefront_settings_translations_seo_description_check CHECK (((char_length(seo_description) <= 320) AND (seo_description = btrim(seo_description)) AND (seo_description !~ '[[:cntrl:]]'::text))),
    CONSTRAINT storefront_settings_translations_seo_title_check CHECK (((char_length(seo_title) <= 120) AND (seo_title = btrim(seo_title)) AND (seo_title !~ '[[:cntrl:]]'::text)))
);

ALTER TABLE tax_classes ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME tax_classes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE tax_configuration (
    id smallint NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT tax_configuration_id_check CHECK ((id = 1)),
    CONSTRAINT tax_configuration_revision_check CHECK ((revision >= 0))
);

ALTER TABLE tax_rates ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME tax_rates_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE tax_zone_countries (
    country_code text NOT NULL,
    zone_id bigint NOT NULL
);

CREATE TABLE tax_zone_rules (
    zone_id bigint NOT NULL,
    class_id bigint NOT NULL,
    rate_id bigint NOT NULL
);

CREATE TABLE tax_zones (
    id bigint NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    active boolean DEFAULT true NOT NULL,
    CONSTRAINT tax_zones_description_check CHECK ((char_length(description) <= 500)),
    CONSTRAINT tax_zones_name_check CHECK ((((char_length(name) >= 1) AND (char_length(name) <= 80)) AND (name = btrim(name))))
);

ALTER TABLE tax_zones ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME tax_zones_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

INSERT INTO admin_role_permissions VALUES ('viewer', 'overview.read');
INSERT INTO admin_role_permissions VALUES ('viewer', 'settings.read');
INSERT INTO admin_role_permissions VALUES ('viewer', 'media.read');

INSERT INTO admin_roles VALUES ('owner', 'Proprietario');
INSERT INTO admin_roles VALUES ('viewer', 'Lettore');

INSERT INTO administrator_languages VALUES ('it');
INSERT INTO administrator_languages VALUES ('en');

INSERT INTO countries VALUES ('AD');
INSERT INTO countries VALUES ('AE');
INSERT INTO countries VALUES ('AF');
INSERT INTO countries VALUES ('AG');
INSERT INTO countries VALUES ('AI');
INSERT INTO countries VALUES ('AL');
INSERT INTO countries VALUES ('AM');
INSERT INTO countries VALUES ('AO');
INSERT INTO countries VALUES ('AQ');
INSERT INTO countries VALUES ('AR');
INSERT INTO countries VALUES ('AS');
INSERT INTO countries VALUES ('AT');
INSERT INTO countries VALUES ('AU');
INSERT INTO countries VALUES ('AW');
INSERT INTO countries VALUES ('AX');
INSERT INTO countries VALUES ('AZ');
INSERT INTO countries VALUES ('BA');
INSERT INTO countries VALUES ('BB');
INSERT INTO countries VALUES ('BD');
INSERT INTO countries VALUES ('BE');
INSERT INTO countries VALUES ('BF');
INSERT INTO countries VALUES ('BG');
INSERT INTO countries VALUES ('BH');
INSERT INTO countries VALUES ('BI');
INSERT INTO countries VALUES ('BJ');
INSERT INTO countries VALUES ('BL');
INSERT INTO countries VALUES ('BM');
INSERT INTO countries VALUES ('BN');
INSERT INTO countries VALUES ('BO');
INSERT INTO countries VALUES ('BQ');
INSERT INTO countries VALUES ('BR');
INSERT INTO countries VALUES ('BS');
INSERT INTO countries VALUES ('BT');
INSERT INTO countries VALUES ('BV');
INSERT INTO countries VALUES ('BW');
INSERT INTO countries VALUES ('BY');
INSERT INTO countries VALUES ('BZ');
INSERT INTO countries VALUES ('CA');
INSERT INTO countries VALUES ('CC');
INSERT INTO countries VALUES ('CD');
INSERT INTO countries VALUES ('CF');
INSERT INTO countries VALUES ('CG');
INSERT INTO countries VALUES ('CH');
INSERT INTO countries VALUES ('CI');
INSERT INTO countries VALUES ('CK');
INSERT INTO countries VALUES ('CL');
INSERT INTO countries VALUES ('CM');
INSERT INTO countries VALUES ('CN');
INSERT INTO countries VALUES ('CO');
INSERT INTO countries VALUES ('CR');
INSERT INTO countries VALUES ('CU');
INSERT INTO countries VALUES ('CV');
INSERT INTO countries VALUES ('CW');
INSERT INTO countries VALUES ('CX');
INSERT INTO countries VALUES ('CY');
INSERT INTO countries VALUES ('CZ');
INSERT INTO countries VALUES ('DE');
INSERT INTO countries VALUES ('DJ');
INSERT INTO countries VALUES ('DK');
INSERT INTO countries VALUES ('DM');
INSERT INTO countries VALUES ('DO');
INSERT INTO countries VALUES ('DZ');
INSERT INTO countries VALUES ('EC');
INSERT INTO countries VALUES ('EE');
INSERT INTO countries VALUES ('EG');
INSERT INTO countries VALUES ('EH');
INSERT INTO countries VALUES ('ER');
INSERT INTO countries VALUES ('ES');
INSERT INTO countries VALUES ('ET');
INSERT INTO countries VALUES ('FI');
INSERT INTO countries VALUES ('FJ');
INSERT INTO countries VALUES ('FK');
INSERT INTO countries VALUES ('FM');
INSERT INTO countries VALUES ('FO');
INSERT INTO countries VALUES ('FR');
INSERT INTO countries VALUES ('GA');
INSERT INTO countries VALUES ('GB');
INSERT INTO countries VALUES ('GD');
INSERT INTO countries VALUES ('GE');
INSERT INTO countries VALUES ('GF');
INSERT INTO countries VALUES ('GG');
INSERT INTO countries VALUES ('GH');
INSERT INTO countries VALUES ('GI');
INSERT INTO countries VALUES ('GL');
INSERT INTO countries VALUES ('GM');
INSERT INTO countries VALUES ('GN');
INSERT INTO countries VALUES ('GP');
INSERT INTO countries VALUES ('GQ');
INSERT INTO countries VALUES ('GR');
INSERT INTO countries VALUES ('GS');
INSERT INTO countries VALUES ('GT');
INSERT INTO countries VALUES ('GU');
INSERT INTO countries VALUES ('GW');
INSERT INTO countries VALUES ('GY');
INSERT INTO countries VALUES ('HK');
INSERT INTO countries VALUES ('HM');
INSERT INTO countries VALUES ('HN');
INSERT INTO countries VALUES ('HR');
INSERT INTO countries VALUES ('HT');
INSERT INTO countries VALUES ('HU');
INSERT INTO countries VALUES ('ID');
INSERT INTO countries VALUES ('IE');
INSERT INTO countries VALUES ('IL');
INSERT INTO countries VALUES ('IM');
INSERT INTO countries VALUES ('IN');
INSERT INTO countries VALUES ('IO');
INSERT INTO countries VALUES ('IQ');
INSERT INTO countries VALUES ('IR');
INSERT INTO countries VALUES ('IS');
INSERT INTO countries VALUES ('IT');
INSERT INTO countries VALUES ('JE');
INSERT INTO countries VALUES ('JM');
INSERT INTO countries VALUES ('JO');
INSERT INTO countries VALUES ('JP');
INSERT INTO countries VALUES ('KE');
INSERT INTO countries VALUES ('KG');
INSERT INTO countries VALUES ('KH');
INSERT INTO countries VALUES ('KI');
INSERT INTO countries VALUES ('KM');
INSERT INTO countries VALUES ('KN');
INSERT INTO countries VALUES ('KP');
INSERT INTO countries VALUES ('KR');
INSERT INTO countries VALUES ('KW');
INSERT INTO countries VALUES ('KY');
INSERT INTO countries VALUES ('KZ');
INSERT INTO countries VALUES ('LA');
INSERT INTO countries VALUES ('LB');
INSERT INTO countries VALUES ('LC');
INSERT INTO countries VALUES ('LI');
INSERT INTO countries VALUES ('LK');
INSERT INTO countries VALUES ('LR');
INSERT INTO countries VALUES ('LS');
INSERT INTO countries VALUES ('LT');
INSERT INTO countries VALUES ('LU');
INSERT INTO countries VALUES ('LV');
INSERT INTO countries VALUES ('LY');
INSERT INTO countries VALUES ('MA');
INSERT INTO countries VALUES ('MC');
INSERT INTO countries VALUES ('MD');
INSERT INTO countries VALUES ('ME');
INSERT INTO countries VALUES ('MF');
INSERT INTO countries VALUES ('MG');
INSERT INTO countries VALUES ('MH');
INSERT INTO countries VALUES ('MK');
INSERT INTO countries VALUES ('ML');
INSERT INTO countries VALUES ('MM');
INSERT INTO countries VALUES ('MN');
INSERT INTO countries VALUES ('MO');
INSERT INTO countries VALUES ('MP');
INSERT INTO countries VALUES ('MQ');
INSERT INTO countries VALUES ('MR');
INSERT INTO countries VALUES ('MS');
INSERT INTO countries VALUES ('MT');
INSERT INTO countries VALUES ('MU');
INSERT INTO countries VALUES ('MV');
INSERT INTO countries VALUES ('MW');
INSERT INTO countries VALUES ('MX');
INSERT INTO countries VALUES ('MY');
INSERT INTO countries VALUES ('MZ');
INSERT INTO countries VALUES ('NA');
INSERT INTO countries VALUES ('NC');
INSERT INTO countries VALUES ('NE');
INSERT INTO countries VALUES ('NF');
INSERT INTO countries VALUES ('NG');
INSERT INTO countries VALUES ('NI');
INSERT INTO countries VALUES ('NL');
INSERT INTO countries VALUES ('NO');
INSERT INTO countries VALUES ('NP');
INSERT INTO countries VALUES ('NR');
INSERT INTO countries VALUES ('NU');
INSERT INTO countries VALUES ('NZ');
INSERT INTO countries VALUES ('OM');
INSERT INTO countries VALUES ('PA');
INSERT INTO countries VALUES ('PE');
INSERT INTO countries VALUES ('PF');
INSERT INTO countries VALUES ('PG');
INSERT INTO countries VALUES ('PH');
INSERT INTO countries VALUES ('PK');
INSERT INTO countries VALUES ('PL');
INSERT INTO countries VALUES ('PM');
INSERT INTO countries VALUES ('PN');
INSERT INTO countries VALUES ('PR');
INSERT INTO countries VALUES ('PS');
INSERT INTO countries VALUES ('PT');
INSERT INTO countries VALUES ('PW');
INSERT INTO countries VALUES ('PY');
INSERT INTO countries VALUES ('QA');
INSERT INTO countries VALUES ('RE');
INSERT INTO countries VALUES ('RO');
INSERT INTO countries VALUES ('RS');
INSERT INTO countries VALUES ('RU');
INSERT INTO countries VALUES ('RW');
INSERT INTO countries VALUES ('SA');
INSERT INTO countries VALUES ('SB');
INSERT INTO countries VALUES ('SC');
INSERT INTO countries VALUES ('SD');
INSERT INTO countries VALUES ('SE');
INSERT INTO countries VALUES ('SG');
INSERT INTO countries VALUES ('SH');
INSERT INTO countries VALUES ('SI');
INSERT INTO countries VALUES ('SJ');
INSERT INTO countries VALUES ('SK');
INSERT INTO countries VALUES ('SL');
INSERT INTO countries VALUES ('SM');
INSERT INTO countries VALUES ('SN');
INSERT INTO countries VALUES ('SO');
INSERT INTO countries VALUES ('SR');
INSERT INTO countries VALUES ('SS');
INSERT INTO countries VALUES ('ST');
INSERT INTO countries VALUES ('SV');
INSERT INTO countries VALUES ('SX');
INSERT INTO countries VALUES ('SY');
INSERT INTO countries VALUES ('SZ');
INSERT INTO countries VALUES ('TC');
INSERT INTO countries VALUES ('TD');
INSERT INTO countries VALUES ('TF');
INSERT INTO countries VALUES ('TG');
INSERT INTO countries VALUES ('TH');
INSERT INTO countries VALUES ('TJ');
INSERT INTO countries VALUES ('TK');
INSERT INTO countries VALUES ('TL');
INSERT INTO countries VALUES ('TM');
INSERT INTO countries VALUES ('TN');
INSERT INTO countries VALUES ('TO');
INSERT INTO countries VALUES ('TR');
INSERT INTO countries VALUES ('TT');
INSERT INTO countries VALUES ('TV');
INSERT INTO countries VALUES ('TW');
INSERT INTO countries VALUES ('TZ');
INSERT INTO countries VALUES ('UA');
INSERT INTO countries VALUES ('UG');
INSERT INTO countries VALUES ('UM');
INSERT INTO countries VALUES ('US');
INSERT INTO countries VALUES ('UY');
INSERT INTO countries VALUES ('UZ');
INSERT INTO countries VALUES ('VA');
INSERT INTO countries VALUES ('VC');
INSERT INTO countries VALUES ('VE');
INSERT INTO countries VALUES ('VG');
INSERT INTO countries VALUES ('VI');
INSERT INTO countries VALUES ('VN');
INSERT INTO countries VALUES ('VU');
INSERT INTO countries VALUES ('WF');
INSERT INTO countries VALUES ('WS');
INSERT INTO countries VALUES ('YE');
INSERT INTO countries VALUES ('YT');
INSERT INTO countries VALUES ('ZA');
INSERT INTO countries VALUES ('ZM');
INSERT INTO countries VALUES ('ZW');

INSERT INTO customer_field_policy VALUES (true, 0, '{}');

INSERT INTO customer_group_configuration VALUES (1, 1, 0);

INSERT INTO customer_groups OVERRIDING SYSTEM VALUE VALUES (1, 'Clienti', 'Customers', false, false);

INSERT INTO customer_registration_settings VALUES (true, 0, false);

INSERT INTO email_configuration VALUES (1, 0, false, '', '', '', '{}');

INSERT INTO email_transport VALUES (1, 0, false, '127.0.0.1', 1025, 'local', '', '\x', 10);

INSERT INTO identity_state VALUES (1, false);

INSERT INTO media_folders OVERRIDING SYSTEM VALUE VALUES (1, NULL, 'Media', 'library', NULL, true);

INSERT INTO order_status_roles VALUES ('preparing', 3);
INSERT INTO order_status_roles VALUES ('shipped', 4);
INSERT INTO order_status_roles VALUES ('delivered', 5);
INSERT INTO order_status_roles VALUES ('cancelled', 6);
INSERT INTO order_status_roles VALUES ('refunded', 7);

INSERT INTO order_statuses OVERRIDING SYSTEM VALUE VALUES (1, 'In attesa di pagamento', 'Awaiting payment', '#b7791f', 0);
INSERT INTO order_statuses OVERRIDING SYSTEM VALUE VALUES (2, 'Pagato', 'Paid', '#16803c', 10);
INSERT INTO order_statuses OVERRIDING SYSTEM VALUE VALUES (3, 'In lavorazione', 'Processing', '#2563eb', 20);
INSERT INTO order_statuses OVERRIDING SYSTEM VALUE VALUES (4, 'Spedito', 'Shipped', '#7c3aed', 30);
INSERT INTO order_statuses OVERRIDING SYSTEM VALUE VALUES (5, 'Consegnato', 'Delivered', '#0f766e', 40);
INSERT INTO order_statuses OVERRIDING SYSTEM VALUE VALUES (6, 'Annullato', 'Cancelled', '#dc2626', 50);
INSERT INTO order_statuses OVERRIDING SYSTEM VALUE VALUES (7, 'Rimborsato', 'Refunded', '#64748b', 60);

INSERT INTO payment_configuration VALUES (1, 3);

INSERT INTO payment_methods OVERRIDING SYSTEM VALUE VALUES (1, 'bank_transfer', 'Bonifico bancario', 'Bank transfer', '', '', false, 0, '', '', '', 'none', 0, 0, 1, NULL);
INSERT INTO payment_methods OVERRIDING SYSTEM VALUE VALUES (2, 'cash_on_delivery', 'Contrassegno', 'Cash on delivery', 'Paga alla consegna.', 'Pay on delivery.', false, 10, '', '', '', 'none', 0, 0, 1, NULL);
INSERT INTO payment_methods OVERRIDING SYSTEM VALUE VALUES (3, 'online_checkout', 'Pagamento online', 'Online payment', '', '', false, 20, '', '', '', 'none', 0, 0, 1, NULL);

INSERT INTO platform_metadata VALUES (1, CURRENT_TIMESTAMP);

INSERT INTO platform_regional_settings VALUES (true, 'Europe/Rome', 'dmy', 0);

INSERT INTO pricing_settings VALUES (1, 'inclusive', NULL, NULL);

INSERT INTO shipping_configuration VALUES (1, 1);

INSERT INTO shipping_methods OVERRIDING SYSTEM VALUE VALUES (1, 'national_express', 'Corriere espresso nazionale', 'National express courier', '', '', false, NULL, 0, 'fixed', 0, 0, NULL);
INSERT INTO shipping_methods OVERRIDING SYSTEM VALUE VALUES (2, 'free_shipping', 'Spedizione gratuita', 'Free shipping', '', '', false, NULL, 0, 'fixed', 0, 0, NULL);

INSERT INTO storefront_languages VALUES ('it');
INSERT INTO storefront_languages VALUES ('en');

INSERT INTO storefront_settings VALUES (1, 'ocean', 'modern', 'compact', true, true, 'Le cose fatte bene.', 'Scopri i prodotti del nostro negozio.', '', '', '', 'it', 'default', false, false);

INSERT INTO storefront_settings_translations VALUES (1, 'it', 'Le cose fatte bene.', 'Scopri i prodotti del nostro negozio.', '', '');

INSERT INTO tax_configuration VALUES (1, 0);

SELECT pg_catalog.setval('administrators_id_seq', 1, false);

SELECT pg_catalog.setval('catalog_attribute_groups_id_seq', 1, false);

SELECT pg_catalog.setval('catalog_attributes_id_seq', 1, false);

SELECT pg_catalog.setval('catalog_brands_id_seq', 1, false);

SELECT pg_catalog.setval('catalog_categories_id_seq', 1, false);

SELECT pg_catalog.setval('catalog_option_values_id_seq', 1, false);

SELECT pg_catalog.setval('catalog_options_id_seq', 1, false);

SELECT pg_catalog.setval('catalog_product_attributes_id_seq', 1, false);

SELECT pg_catalog.setval('catalog_product_images_id_seq', 1, false);

SELECT pg_catalog.setval('catalog_products_id_seq', 1, false);

SELECT pg_catalog.setval('catalog_variants_id_seq', 1, false);

SELECT pg_catalog.setval('coupons_id_seq', 1, false);

SELECT pg_catalog.setval('customer_addresses_id_seq', 1, false);

SELECT pg_catalog.setval('customer_billing_revision', 1, false);

SELECT pg_catalog.setval('customer_groups_id_seq', 1, true);

SELECT pg_catalog.setval('customers_id_seq', 1, false);

SELECT pg_catalog.setval('email_outbox_id_seq', 1, false);

SELECT pg_catalog.setval('guest_cart_items_id_seq', 1, false);

SELECT pg_catalog.setval('guest_carts_id_seq', 1, false);

SELECT pg_catalog.setval('home_banners_id_seq', 1, false);

SELECT pg_catalog.setval('information_pages_id_seq', 1, false);

SELECT pg_catalog.setval('media_cache_jobs_id_seq', 1, false);

SELECT pg_catalog.setval('media_folders_id_seq', 1, true);

SELECT pg_catalog.setval('media_objects_id_seq', 1, false);

SELECT pg_catalog.setval('operation_events_id_seq', 1, false);

SELECT pg_catalog.setval('order_fulfillment_events_id_seq', 1, false);

SELECT pg_catalog.setval('order_lines_id_seq', 1, false);

SELECT pg_catalog.setval('order_status_events_id_seq', 1, false);

SELECT pg_catalog.setval('order_statuses_id_seq', 7, true);

SELECT pg_catalog.setval('orders_id_seq', 1, false);

SELECT pg_catalog.setval('payment_attempts_id_seq', 1, false);

SELECT pg_catalog.setval('payment_methods_id_seq', 3, true);

SELECT pg_catalog.setval('shipping_methods_id_seq', 2, true);

SELECT pg_catalog.setval('tax_classes_id_seq', 1, false);

SELECT pg_catalog.setval('tax_rates_id_seq', 1, false);

SELECT pg_catalog.setval('tax_zones_id_seq', 1, false);

ALTER TABLE ONLY admin_role_permissions
    ADD CONSTRAINT admin_role_permissions_pkey PRIMARY KEY (role_key, permission);

ALTER TABLE ONLY admin_roles
    ADD CONSTRAINT admin_roles_pkey PRIMARY KEY (key);

ALTER TABLE ONLY admin_sessions
    ADD CONSTRAINT admin_sessions_pkey PRIMARY KEY (token_hash);

ALTER TABLE ONLY administrator_languages
    ADD CONSTRAINT administrator_languages_pkey PRIMARY KEY (code);

ALTER TABLE ONLY administrators
    ADD CONSTRAINT administrators_email_key UNIQUE (email);

ALTER TABLE ONLY administrators
    ADD CONSTRAINT administrators_pkey PRIMARY KEY (id);

ALTER TABLE ONLY auth_limits
    ADD CONSTRAINT auth_limits_pkey PRIMARY KEY (bucket);

ALTER TABLE ONLY catalog_attribute_groups
    ADD CONSTRAINT catalog_attribute_groups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY catalog_attribute_groups_translations
    ADD CONSTRAINT catalog_attribute_groups_translations_pkey PRIMARY KEY (entity_id, locale);

ALTER TABLE ONLY catalog_attributes
    ADD CONSTRAINT catalog_attributes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY catalog_attributes_translations
    ADD CONSTRAINT catalog_attributes_translations_pkey PRIMARY KEY (entity_id, locale);

ALTER TABLE ONLY catalog_brands
    ADD CONSTRAINT catalog_brands_pkey PRIMARY KEY (id);

ALTER TABLE ONLY catalog_brands
    ADD CONSTRAINT catalog_brands_slug_key UNIQUE (slug);

ALTER TABLE ONLY catalog_brands_translations
    ADD CONSTRAINT catalog_brands_translations_pkey PRIMARY KEY (entity_id, locale);

ALTER TABLE ONLY catalog_categories
    ADD CONSTRAINT catalog_categories_pkey PRIMARY KEY (id);

ALTER TABLE ONLY catalog_categories
    ADD CONSTRAINT catalog_categories_slug_key UNIQUE (slug);

ALTER TABLE ONLY catalog_categories_translations
    ADD CONSTRAINT catalog_categories_translations_pkey PRIMARY KEY (entity_id, locale);

ALTER TABLE ONLY catalog_demo_packages
    ADD CONSTRAINT catalog_demo_packages_pkey PRIMARY KEY (package);

ALTER TABLE ONLY catalog_option_values
    ADD CONSTRAINT catalog_option_values_id_owner_unique UNIQUE (id, option_id);

ALTER TABLE ONLY catalog_option_values
    ADD CONSTRAINT catalog_option_values_pkey PRIMARY KEY (id);

ALTER TABLE ONLY catalog_option_values_translations
    ADD CONSTRAINT catalog_option_values_translations_pkey PRIMARY KEY (entity_id, locale);

ALTER TABLE ONLY catalog_options
    ADD CONSTRAINT catalog_options_pkey PRIMARY KEY (id);

ALTER TABLE ONLY catalog_options_translations
    ADD CONSTRAINT catalog_options_translations_pkey PRIMARY KEY (entity_id, locale);

ALTER TABLE ONLY catalog_product_attributes
    ADD CONSTRAINT catalog_product_attributes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY catalog_product_attributes
    ADD CONSTRAINT catalog_product_attributes_product_id_attribute_id_key UNIQUE (product_id, attribute_id);

ALTER TABLE ONLY catalog_product_attributes_translations
    ADD CONSTRAINT catalog_product_attributes_translations_pkey PRIMARY KEY (entity_id, locale);

ALTER TABLE ONLY catalog_product_images
    ADD CONSTRAINT catalog_product_images_pkey PRIMARY KEY (id);

ALTER TABLE ONLY catalog_product_images
    ADD CONSTRAINT catalog_product_images_product_id_position_key UNIQUE (product_id, "position") DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY catalog_product_options
    ADD CONSTRAINT catalog_product_options_pkey PRIMARY KEY (product_id, "position");

ALTER TABLE ONLY catalog_product_options
    ADD CONSTRAINT catalog_product_options_product_id_option_id_key UNIQUE (product_id, option_id);

ALTER TABLE ONLY catalog_products
    ADD CONSTRAINT catalog_products_pkey PRIMARY KEY (id);

ALTER TABLE ONLY catalog_products
    ADD CONSTRAINT catalog_products_slug_key UNIQUE (slug);

ALTER TABLE ONLY catalog_products_translations
    ADD CONSTRAINT catalog_products_translations_pkey PRIMARY KEY (entity_id, locale);

ALTER TABLE ONLY catalog_variant_values
    ADD CONSTRAINT catalog_variant_values_pkey PRIMARY KEY (variant_id, value_id);

ALTER TABLE ONLY catalog_variants
    ADD CONSTRAINT catalog_variants_pkey PRIMARY KEY (id);

ALTER TABLE ONLY catalog_variants
    ADD CONSTRAINT catalog_variants_product_id_value_ids_key UNIQUE (product_id, value_ids);

ALTER TABLE ONLY checkout_countries
    ADD CONSTRAINT checkout_countries_pkey PRIMARY KEY (country_code);

ALTER TABLE ONLY checkout_details
    ADD CONSTRAINT checkout_details_pkey PRIMARY KEY (cart_id);

ALTER TABLE ONLY checkout_methods
    ADD CONSTRAINT checkout_methods_pkey PRIMARY KEY (cart_id);

ALTER TABLE ONLY coupon_uses
    ADD CONSTRAINT coupon_uses_pkey PRIMARY KEY (order_id);

ALTER TABLE ONLY coupons
    ADD CONSTRAINT coupons_code_key UNIQUE (code);

ALTER TABLE ONLY coupons
    ADD CONSTRAINT coupons_pkey PRIMARY KEY (id);

ALTER TABLE ONLY customer_addresses
    ADD CONSTRAINT customer_addresses_pkey PRIMARY KEY (id);

ALTER TABLE ONLY customer_billing
    ADD CONSTRAINT customer_billing_pkey PRIMARY KEY (customer_id);

ALTER TABLE ONLY customer_email_confirmations
    ADD CONSTRAINT customer_email_confirmations_pkey PRIMARY KEY (customer_id);

ALTER TABLE ONLY customer_email_confirmations
    ADD CONSTRAINT customer_email_confirmations_token_hash_key UNIQUE (token_hash);

ALTER TABLE ONLY customer_field_policy
    ADD CONSTRAINT customer_field_policy_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY customer_group_configuration
    ADD CONSTRAINT customer_group_configuration_pkey PRIMARY KEY (id);

ALTER TABLE ONLY customer_group_quantity_tiers
    ADD CONSTRAINT customer_group_quantity_tiers_group_id_min_quantity_key UNIQUE (group_id, min_quantity);

ALTER TABLE ONLY customer_group_quantity_tiers
    ADD CONSTRAINT customer_group_quantity_tiers_pkey PRIMARY KEY (group_id, "position");

ALTER TABLE ONLY customer_groups
    ADD CONSTRAINT customer_groups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY customer_orders
    ADD CONSTRAINT customer_orders_pkey PRIMARY KEY (order_id);

ALTER TABLE ONLY customer_password_resets
    ADD CONSTRAINT customer_password_resets_customer_id_key UNIQUE (customer_id);

ALTER TABLE ONLY customer_password_resets
    ADD CONSTRAINT customer_password_resets_pkey PRIMARY KEY (token_hash);

ALTER TABLE ONLY customer_registration_settings
    ADD CONSTRAINT customer_registration_settings_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY customer_sessions
    ADD CONSTRAINT customer_sessions_pkey PRIMARY KEY (token_hash);

ALTER TABLE ONLY customers
    ADD CONSTRAINT customers_email_key UNIQUE (email);

ALTER TABLE ONLY customers
    ADD CONSTRAINT customers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY email_attempts
    ADD CONSTRAINT email_attempts_pkey PRIMARY KEY (message_id, attempt);

ALTER TABLE ONLY email_configuration
    ADD CONSTRAINT email_configuration_pkey PRIMARY KEY (id);

ALTER TABLE ONLY email_event_audiences
    ADD CONSTRAINT email_event_audiences_pkey PRIMARY KEY (event_key);

ALTER TABLE ONLY email_outbox
    ADD CONSTRAINT email_outbox_event_key_recipient_key UNIQUE (event_key, recipient);

ALTER TABLE ONLY email_outbox
    ADD CONSTRAINT email_outbox_message_id_key UNIQUE (message_id);

ALTER TABLE ONLY email_outbox
    ADD CONSTRAINT email_outbox_pkey PRIMARY KEY (id);

ALTER TABLE ONLY email_transport
    ADD CONSTRAINT email_transport_pkey PRIMARY KEY (id);

ALTER TABLE ONLY guest_cart_items
    ADD CONSTRAINT guest_cart_items_cart_id_item_key_key UNIQUE (cart_id, item_key);

ALTER TABLE ONLY guest_cart_items
    ADD CONSTRAINT guest_cart_items_pkey PRIMARY KEY (id);

ALTER TABLE ONLY guest_carts
    ADD CONSTRAINT guest_carts_customer_id_key UNIQUE (customer_id);

ALTER TABLE ONLY guest_carts
    ADD CONSTRAINT guest_carts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY guest_carts
    ADD CONSTRAINT guest_carts_token_hash_key UNIQUE (token_hash);

ALTER TABLE ONLY home_banners
    ADD CONSTRAINT home_banners_pkey PRIMARY KEY (id);

ALTER TABLE ONLY home_banners_translations
    ADD CONSTRAINT home_banners_translations_pkey PRIMARY KEY (entity_id, locale);

ALTER TABLE ONLY identity_state
    ADD CONSTRAINT identity_state_pkey PRIMARY KEY (id);

ALTER TABLE ONLY information_page_slugs
    ADD CONSTRAINT information_page_slugs_pkey PRIMARY KEY (slug);

ALTER TABLE ONLY information_pages
    ADD CONSTRAINT information_pages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY information_pages
    ADD CONSTRAINT information_pages_slug_key UNIQUE (slug);

ALTER TABLE ONLY information_pages_translations
    ADD CONSTRAINT information_pages_translations_pkey PRIMARY KEY (entity_id, locale);

ALTER TABLE ONLY media_cache_jobs
    ADD CONSTRAINT media_cache_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY media_cache_jobs
    ADD CONSTRAINT media_cache_jobs_source_key_key UNIQUE (source_key);

ALTER TABLE ONLY media_folders
    ADD CONSTRAINT media_folders_path_key UNIQUE (path);

ALTER TABLE ONLY media_folders
    ADD CONSTRAINT media_folders_pkey PRIMARY KEY (id);

ALTER TABLE ONLY media_library
    ADD CONSTRAINT media_library_pkey PRIMARY KEY (key);

ALTER TABLE ONLY media_locations
    ADD CONSTRAINT media_locations_path_key UNIQUE (path);

ALTER TABLE ONLY media_locations
    ADD CONSTRAINT media_locations_pkey PRIMARY KEY (key);

ALTER TABLE ONLY media_objects
    ADD CONSTRAINT media_objects_id_key UNIQUE (id);

ALTER TABLE ONLY media_objects
    ADD CONSTRAINT media_objects_pkey PRIMARY KEY (key);

ALTER TABLE ONLY media_renditions
    ADD CONSTRAINT media_renditions_pkey PRIMARY KEY (source_key, profile);

ALTER TABLE ONLY media_retired_directories
    ADD CONSTRAINT media_retired_directories_pkey PRIMARY KEY (path);

ALTER TABLE ONLY media_retired_files
    ADD CONSTRAINT media_retired_files_pkey PRIMARY KEY (path);

ALTER TABLE ONLY media_transforms
    ADD CONSTRAINT media_transforms_pkey PRIMARY KEY (source_key, kind);

ALTER TABLE ONLY media_variants
    ADD CONSTRAINT media_variants_object_key_key UNIQUE (object_key);

ALTER TABLE ONLY media_variants
    ADD CONSTRAINT media_variants_pkey PRIMARY KEY (source_key, profile);

ALTER TABLE ONLY online_captures
    ADD CONSTRAINT online_captures_pkey PRIMARY KEY (attempt_id);

ALTER TABLE ONLY online_captures
    ADD CONSTRAINT online_captures_provider_id_intent_id_key UNIQUE (provider_id, intent_id);

ALTER TABLE ONLY online_payment_events
    ADD CONSTRAINT online_payment_events_pkey PRIMARY KEY (provider_id, id);

ALTER TABLE ONLY online_refunds
    ADD CONSTRAINT online_refunds_pkey PRIMARY KEY (attempt_id);

ALTER TABLE ONLY online_refunds
    ADD CONSTRAINT online_refunds_provider_id_refund_id_key UNIQUE (provider_id, refund_id);

ALTER TABLE ONLY operation_events
    ADD CONSTRAINT operation_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY order_current_status
    ADD CONSTRAINT order_current_status_pkey PRIMARY KEY (order_id);

ALTER TABLE ONLY order_customer_summaries
    ADD CONSTRAINT order_customer_summaries_pkey PRIMARY KEY (order_id);

ALTER TABLE ONLY order_fulfillment_events
    ADD CONSTRAINT order_fulfillment_events_order_id_request_key_key UNIQUE (order_id, request_key);

ALTER TABLE ONLY order_fulfillment_events
    ADD CONSTRAINT order_fulfillment_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY order_fulfillment
    ADD CONSTRAINT order_fulfillment_pkey PRIMARY KEY (order_id);

ALTER TABLE ONLY order_lines
    ADD CONSTRAINT order_lines_pkey PRIMARY KEY (id);

ALTER TABLE ONLY order_manual_payments
    ADD CONSTRAINT order_manual_payments_pkey PRIMARY KEY (order_id);

ALTER TABLE ONLY order_online_payments
    ADD CONSTRAINT order_online_payments_attempt_id_key UNIQUE (attempt_id);

ALTER TABLE ONLY order_online_payments
    ADD CONSTRAINT order_online_payments_pkey PRIMARY KEY (order_id);

ALTER TABLE ONLY order_reference_counters
    ADD CONSTRAINT order_reference_counters_pkey PRIMARY KEY (year);

ALTER TABLE ONLY order_status_events
    ADD CONSTRAINT order_status_events_order_id_revision_key UNIQUE (order_id, revision);

ALTER TABLE ONLY order_status_events
    ADD CONSTRAINT order_status_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY order_status_roles
    ADD CONSTRAINT order_status_roles_pkey PRIMARY KEY (role);

ALTER TABLE ONLY order_status_roles
    ADD CONSTRAINT order_status_roles_status_id_key UNIQUE (status_id);

ALTER TABLE ONLY order_statuses
    ADD CONSTRAINT order_statuses_pkey PRIMARY KEY (id);

ALTER TABLE ONLY order_stock_moves
    ADD CONSTRAINT order_stock_moves_pkey PRIMARY KEY (line_id, kind);

ALTER TABLE ONLY orders
    ADD CONSTRAINT orders_pkey PRIMARY KEY (id);

ALTER TABLE ONLY orders
    ADD CONSTRAINT orders_reference_key UNIQUE (reference);

ALTER TABLE ONLY orders
    ADD CONSTRAINT orders_source_cart_id_source_revision_key UNIQUE (source_cart_id, source_revision);

ALTER TABLE ONLY password_resets
    ADD CONSTRAINT password_resets_administrator_id_key UNIQUE (administrator_id);

ALTER TABLE ONLY password_resets
    ADD CONSTRAINT password_resets_pkey PRIMARY KEY (token_hash);

ALTER TABLE ONLY payment_attempts
    ADD CONSTRAINT payment_attempts_id_provider_id_key UNIQUE (id, provider_id);

ALTER TABLE ONLY payment_attempts
    ADD CONSTRAINT payment_attempts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY payment_attempts
    ADD CONSTRAINT payment_attempts_provider_id_intent_id_key UNIQUE (provider_id, intent_id);

ALTER TABLE ONLY payment_attempts
    ADD CONSTRAINT payment_attempts_provider_id_refund_id_key UNIQUE (provider_id, refund_id);

ALTER TABLE ONLY payment_attempts
    ADD CONSTRAINT payment_attempts_provider_id_session_id_key UNIQUE (provider_id, session_id);

ALTER TABLE ONLY payment_attempts
    ADD CONSTRAINT payment_attempts_request_key_key UNIQUE (request_key);

ALTER TABLE ONLY payment_configuration
    ADD CONSTRAINT payment_configuration_pkey PRIMARY KEY (id);

ALTER TABLE ONLY payment_methods
    ADD CONSTRAINT payment_methods_code_key UNIQUE (code);

ALTER TABLE ONLY payment_methods
    ADD CONSTRAINT payment_methods_pkey PRIMARY KEY (id);

ALTER TABLE ONLY platform_metadata
    ADD CONSTRAINT platform_metadata_pkey PRIMARY KEY (id);

ALTER TABLE ONLY platform_regional_settings
    ADD CONSTRAINT platform_regional_settings_pkey PRIMARY KEY (singleton);

ALTER TABLE ONLY pricing_settings
    ADD CONSTRAINT pricing_settings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY shipping_configuration
    ADD CONSTRAINT shipping_configuration_pkey PRIMARY KEY (id);

ALTER TABLE ONLY shipping_method_countries
    ADD CONSTRAINT shipping_method_countries_pkey PRIMARY KEY (method_id, country_code);

ALTER TABLE ONLY shipping_methods
    ADD CONSTRAINT shipping_methods_code_key UNIQUE (code);

ALTER TABLE ONLY shipping_methods
    ADD CONSTRAINT shipping_methods_pkey PRIMARY KEY (id);

ALTER TABLE ONLY store_settings
    ADD CONSTRAINT store_settings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY storefront_language_prefixes
    ADD CONSTRAINT storefront_language_prefixes_pkey PRIMARY KEY (code);

ALTER TABLE ONLY storefront_languages
    ADD CONSTRAINT storefront_languages_pkey PRIMARY KEY (code);

ALTER TABLE ONLY storefront_settings
    ADD CONSTRAINT storefront_settings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY storefront_settings_translations
    ADD CONSTRAINT storefront_settings_translations_pkey PRIMARY KEY (entity_id, locale);

ALTER TABLE ONLY tax_classes
    ADD CONSTRAINT tax_classes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY tax_configuration
    ADD CONSTRAINT tax_configuration_pkey PRIMARY KEY (id);

ALTER TABLE ONLY countries
    ADD CONSTRAINT tax_countries_pkey PRIMARY KEY (code);

ALTER TABLE ONLY tax_rates
    ADD CONSTRAINT tax_rates_pkey PRIMARY KEY (id);

ALTER TABLE ONLY tax_zone_countries
    ADD CONSTRAINT tax_zone_countries_pkey PRIMARY KEY (country_code);

ALTER TABLE ONLY tax_zone_rules
    ADD CONSTRAINT tax_zone_rules_pkey PRIMARY KEY (zone_id, class_id);

ALTER TABLE ONLY tax_zones
    ADD CONSTRAINT tax_zones_pkey PRIMARY KEY (id);

CREATE UNIQUE INDEX admin_roles_name_unique ON admin_roles USING btree (lower(name));

CREATE INDEX admin_sessions_administrator ON admin_sessions USING btree (administrator_id);

CREATE INDEX admin_sessions_expiry ON admin_sessions USING btree (expires_at);

CREATE INDEX administrators_avatar_key_idx ON administrators USING btree (avatar_key) WHERE (avatar_key IS NOT NULL);

CREATE INDEX administrators_role ON administrators USING btree (role);

CREATE UNIQUE INDEX catalog_attribute_groups_translation_name_unique ON catalog_attribute_groups_translations USING btree (locale, lower(name)) WHERE (name <> ''::text);

CREATE INDEX catalog_attributes_group_idx ON catalog_attributes USING btree (group_id, sort_order, id);

CREATE UNIQUE INDEX catalog_attributes_translation_name_unique ON catalog_attributes_translations USING btree (locale, lower(name)) WHERE (name <> ''::text);

CREATE INDEX catalog_brands_list_order ON catalog_brands USING btree (sort_order, lower(name), id);

CREATE INDEX catalog_brands_logo_idx ON catalog_brands USING btree (logo_key) WHERE (logo_key IS NOT NULL);

CREATE UNIQUE INDEX catalog_brands_translation_name_unique ON catalog_brands_translations USING btree (locale, lower(name)) WHERE (name <> ''::text);

CREATE INDEX catalog_categories_list_order ON catalog_categories USING btree (sort_order, lower(name), id);

CREATE INDEX catalog_categories_parent_order ON catalog_categories USING btree (parent_id, sort_order, id);

CREATE INDEX catalog_option_values_order_idx ON catalog_option_values USING btree (option_id, sort_order, lower(name), id);

CREATE INDEX catalog_option_values_owner_idx ON catalog_option_values USING btree (option_id, id);

CREATE INDEX catalog_options_order_idx ON catalog_options USING btree (sort_order, lower(name), id);

CREATE UNIQUE INDEX catalog_options_translation_name_unique ON catalog_options_translations USING btree (locale, lower(name)) WHERE (name <> ''::text);

CREATE INDEX catalog_product_attributes_attribute_idx ON catalog_product_attributes USING btree (attribute_id);

CREATE INDEX catalog_product_images_media_idx ON catalog_product_images USING btree (image_key);

CREATE INDEX catalog_product_options_option_idx ON catalog_product_options USING btree (option_id);

CREATE INDEX catalog_products_brand_idx ON catalog_products USING btree (brand_id);

CREATE INDEX catalog_products_category_idx ON catalog_products USING btree (category_id, id);

CREATE INDEX catalog_products_image_idx ON catalog_products USING btree (image_key) WHERE (image_key IS NOT NULL);

CREATE INDEX catalog_products_list_idx ON catalog_products USING btree (lower(name), id);

CREATE UNIQUE INDEX catalog_products_model_unique ON catalog_products USING btree (model);

CREATE INDEX catalog_products_tax_idx ON catalog_products USING btree (tax_class_id);

CREATE UNIQUE INDEX catalog_values_translation_name_unique ON catalog_option_values_translations USING btree (option_id, locale, lower(name)) WHERE (name <> ''::text);

CREATE INDEX catalog_variant_values_value_idx ON catalog_variant_values USING btree (value_id);

CREATE INDEX catalog_variants_image_idx ON catalog_variants USING btree (image_key) WHERE (image_key IS NOT NULL);

CREATE INDEX catalog_variants_product_idx ON catalog_variants USING btree (product_id, id);

CREATE UNIQUE INDEX catalog_variants_sku_unique ON catalog_variants USING btree (sku);

CREATE INDEX coupon_uses_coupon ON coupon_uses USING btree (coupon_id);

CREATE INDEX customer_address_owner ON customer_addresses USING btree (customer_id, id);

CREATE UNIQUE INDEX customer_address_default ON customer_addresses (customer_id) WHERE is_default;

CREATE INDEX customer_group_members ON customers USING btree (group_id, id);

CREATE UNIQUE INDEX customer_groups_name_en ON customer_groups USING btree (lower(name_en));

CREATE UNIQUE INDEX customer_groups_name_it ON customer_groups USING btree (lower(name_it));

CREATE INDEX customer_order_history ON customer_orders USING btree (customer_id, order_id DESC);

CREATE INDEX customer_session_expiry ON customer_sessions USING btree (expires_at);

CREATE INDEX customer_session_owner ON customer_sessions USING btree (customer_id);

CREATE INDEX email_outbox_customer_confirmation ON email_outbox USING btree (event_key text_pattern_ops) WHERE ((template = 'customer_confirmation'::text) AND (state <> 'accepted'::text));

CREATE INDEX email_outbox_expired ON email_outbox USING btree (lease_until, id) WHERE (state = 'sending'::text);

CREATE INDEX email_outbox_order ON email_outbox USING btree (order_id, id) WHERE (order_id IS NOT NULL);

CREATE INDEX email_outbox_pending ON email_outbox USING btree (available_at, id) WHERE (state = 'pending'::text);

CREATE INDEX guest_cart_product ON guest_cart_items USING btree (product_id);

CREATE INDEX guest_cart_variant ON guest_cart_items USING btree (variant_id);

CREATE INDEX guest_carts_expiry ON guest_carts USING btree (expires_at, id);

CREATE INDEX home_banners_image ON home_banners USING btree (image_key);

CREATE INDEX home_banners_public ON home_banners USING btree (sort_order, id) WHERE active;

CREATE INDEX information_page_slugs_page ON information_page_slugs USING btree (page_id);

CREATE INDEX information_pages_navigation ON information_pages USING btree (sort_order, id) WHERE (published AND (header_menu OR footer_menu));

CREATE INDEX media_folders_parent_idx ON media_folders USING btree (parent_id, lower(name), id);

CREATE INDEX media_library_folder_idx ON media_library USING btree (folder_id, trashed_at, lower(name), key);

CREATE UNIQUE INDEX media_library_name_idx ON media_library USING btree (folder_id, name);

CREATE INDEX media_library_owner_idx ON media_library USING btree (owner_id, trashed_at);

CREATE INDEX media_locations_names_idx ON media_locations USING btree (lower(path));

CREATE INDEX media_objects_unused_idx ON media_objects USING btree (unused_since, key) WHERE (unused_since IS NOT NULL);

CREATE INDEX media_renditions_object_idx ON media_renditions USING btree (object_key);

CREATE INDEX media_transforms_object_idx ON media_transforms USING btree (object_key);

CREATE INDEX operation_events_area_id ON operation_events USING btree (area, id DESC);

CREATE INDEX order_current_status_status ON order_current_status USING btree (status_id, order_id);

CREATE INDEX order_fulfillment_events_order ON order_fulfillment_events USING btree (order_id, id DESC);

CREATE INDEX order_lines_current ON order_lines(order_id,id) WHERE active;

CREATE INDEX order_lines_order ON order_lines USING btree (order_id, id);

CREATE INDEX order_lines_product ON order_lines USING btree (product_id);

CREATE INDEX order_lines_variant ON order_lines USING btree (variant_id) WHERE (variant_id IS NOT NULL);

CREATE INDEX order_status_events_order ON order_status_events USING btree (order_id, id DESC);

CREATE UNIQUE INDEX order_statuses_name_en_idx ON order_statuses USING btree (lower(btrim(name_en)));

CREATE UNIQUE INDEX order_statuses_name_it_idx ON order_statuses USING btree (lower(btrim(name_it)));

CREATE INDEX order_statuses_order_idx ON order_statuses USING btree (sort_order, id);

CREATE INDEX orders_created ON orders USING btree (created_at DESC, id DESC);

CREATE INDEX orders_owner ON orders USING btree (owner_hash, id DESC);

CREATE UNIQUE INDEX payment_attempt_active ON payment_attempts USING btree (order_id) WHERE (state = ANY (ARRAY['creating'::text, 'open'::text, 'processing'::text]));

CREATE INDEX payment_attempt_order ON payment_attempts USING btree (order_id, id DESC);

CREATE INDEX payment_attempt_work ON payment_attempts USING btree (next_at, id) WHERE (action <> 'none'::text);

CREATE INDEX payment_methods_initial_order_status_idx ON payment_methods USING btree (initial_order_status_id);

CREATE INDEX payment_methods_order_idx ON payment_methods USING btree (sort_order, id);

CREATE INDEX payment_methods_tax_class_idx ON payment_methods USING btree (tax_class_id);

CREATE INDEX product_modified_events_idx ON operation_events USING btree (resource_key, created_at DESC, id DESC) WHERE (resource = ANY (ARRAY['catalog_products'::text, 'catalog_products_translations'::text]));

CREATE INDEX shipping_method_countries_country_idx ON shipping_method_countries USING btree (country_code, method_id);

CREATE INDEX shipping_methods_order_idx ON shipping_methods USING btree (sort_order, id);

CREATE INDEX shipping_methods_tax_class_idx ON shipping_methods USING btree (tax_class_id);

CREATE UNIQUE INDEX tax_classes_name_unique ON tax_classes USING btree (lower(name));

CREATE INDEX tax_classes_rate_idx ON tax_classes USING btree (rate_id);

CREATE UNIQUE INDEX tax_rates_name_unique ON tax_rates USING btree (lower(name));

CREATE INDEX tax_zone_countries_zone_idx ON tax_zone_countries USING btree (zone_id, country_code);

CREATE INDEX tax_zone_rules_class_idx ON tax_zone_rules USING btree (class_id);

CREATE INDEX tax_zone_rules_rate_idx ON tax_zone_rules USING btree (rate_id);

CREATE UNIQUE INDEX tax_zones_name_unique ON tax_zones USING btree (lower(name));

CREATE TRIGGER account_media_reference AFTER INSERT OR DELETE OR UPDATE OF avatar_key ON administrators FOR EACH ROW EXECUTE FUNCTION media_track_account();

CREATE TRIGGER admin_role_protection BEFORE DELETE OR UPDATE ON admin_roles FOR EACH ROW EXECUTE FUNCTION protect_admin_role();

CREATE TRIGGER brand_media_reference AFTER INSERT OR DELETE OR UPDATE OF logo_key ON catalog_brands FOR EACH ROW EXECUTE FUNCTION media_track_brand();

CREATE TRIGGER cart_customer_immutable BEFORE UPDATE OF customer_id ON guest_carts FOR EACH ROW EXECUTE FUNCTION protect_cart_customer();

CREATE TRIGGER catalog_attribute_groups_guard BEFORE INSERT OR DELETE OR UPDATE ON catalog_attribute_groups FOR EACH ROW EXECUTE FUNCTION catalog_attribute_guard();

CREATE TRIGGER catalog_attributes_guard BEFORE INSERT OR DELETE OR UPDATE ON catalog_attributes FOR EACH ROW EXECUTE FUNCTION catalog_attribute_guard();

CREATE TRIGGER catalog_category_graph BEFORE INSERT OR DELETE OR UPDATE OF parent_id ON catalog_categories FOR EACH ROW EXECUTE FUNCTION catalog_category_hierarchy();

CREATE TRIGGER catalog_option_guard BEFORE INSERT OR DELETE OR UPDATE ON catalog_options FOR EACH ROW EXECUTE FUNCTION catalog_option_guard();

CREATE TRIGGER catalog_product_attributes_guard BEFORE INSERT OR DELETE OR UPDATE ON catalog_product_attributes FOR EACH ROW EXECUTE FUNCTION catalog_attribute_guard();

CREATE TRIGGER catalog_value_guard BEFORE INSERT OR DELETE OR UPDATE ON catalog_option_values FOR EACH ROW EXECUTE FUNCTION catalog_option_guard();

CREATE TRIGGER checkout_countries_guard BEFORE INSERT OR DELETE OR UPDATE ON checkout_countries FOR EACH ROW EXECUTE FUNCTION checkout_country_guard();

CREATE TRIGGER checkout_countries_revision AFTER INSERT OR DELETE OR UPDATE ON checkout_countries FOR EACH STATEMENT EXECUTE FUNCTION tax_revision_changed();

CREATE TRIGGER coupon_currency_guard BEFORE UPDATE OF currency ON store_settings FOR EACH ROW EXECUTE FUNCTION coupon_currency_guard();

CREATE TRIGGER coupon_use_guard BEFORE INSERT ON coupon_uses FOR EACH ROW EXECUTE FUNCTION coupon_use_guard();

CREATE TRIGGER coupon_use_immutable BEFORE DELETE OR UPDATE ON coupon_uses FOR EACH ROW EXECUTE FUNCTION order_snapshot_guard();

CREATE TRIGGER customer_address_guard BEFORE INSERT OR UPDATE ON customer_addresses FOR EACH ROW EXECUTE FUNCTION customer_address_guard();

CREATE TRIGGER customer_billing_guard BEFORE UPDATE ON customer_billing FOR EACH ROW EXECUTE FUNCTION customer_billing_guard();

CREATE TRIGGER customer_confirmation_invalidate BEFORE UPDATE OF active, email ON customers FOR EACH ROW EXECUTE FUNCTION customer_confirmation_invalidate();

CREATE TRIGGER customer_default_group BEFORE INSERT ON customers FOR EACH ROW EXECUTE FUNCTION customer_default_group();

CREATE TRIGGER customer_field_policy_guard BEFORE DELETE OR UPDATE ON customer_field_policy FOR EACH ROW EXECUTE FUNCTION customer_field_policy_guard();

CREATE TRIGGER customer_group_capacity_guard BEFORE INSERT ON customer_groups FOR EACH ROW EXECUTE FUNCTION customer_group_capacity_guard();

CREATE TRIGGER customer_group_configuration_guard BEFORE DELETE OR UPDATE ON customer_group_configuration FOR EACH ROW EXECUTE FUNCTION customer_group_configuration_guard();

CREATE CONSTRAINT TRIGGER customer_group_quantity_schedule_guard AFTER INSERT OR UPDATE ON customer_groups DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION customer_group_quantity_schedule_guard();

CREATE CONSTRAINT TRIGGER customer_group_quantity_tier_guard AFTER INSERT OR DELETE OR UPDATE ON customer_group_quantity_tiers DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION customer_group_quantity_schedule_guard();

CREATE TRIGGER customer_order_immutable BEFORE DELETE OR UPDATE ON customer_orders FOR EACH ROW EXECUTE FUNCTION order_snapshot_guard();

CREATE TRIGGER gallery_media_reference AFTER INSERT OR DELETE OR UPDATE OF image_key ON catalog_product_images FOR EACH ROW EXECUTE FUNCTION media_track_product();

CREATE TRIGGER guest_cart_item_limit BEFORE INSERT OR UPDATE OF cart_id ON guest_cart_items FOR EACH ROW EXECUTE FUNCTION guest_cart_limit();

CREATE TRIGGER information_page_slug AFTER INSERT OR UPDATE OF slug ON information_pages FOR EACH ROW EXECUTE FUNCTION information_page_reserve_slug();

CREATE TRIGGER manual_payment_immutable BEFORE DELETE OR UPDATE ON order_manual_payments FOR EACH ROW EXECUTE FUNCTION order_snapshot_guard();

CREATE TRIGGER media_location_insert AFTER INSERT ON media_objects FOR EACH ROW EXECUTE FUNCTION media_location_insert();

CREATE TRIGGER media_rendition_retired AFTER DELETE OR UPDATE ON media_renditions FOR EACH ROW EXECUTE FUNCTION media_rendition_retired();

CREATE TRIGGER online_capture_guard BEFORE INSERT ON online_captures FOR EACH ROW EXECUTE FUNCTION online_capture_guard();

CREATE TRIGGER online_capture_immutable BEFORE DELETE OR UPDATE ON online_captures FOR EACH ROW EXECUTE FUNCTION order_snapshot_guard();

CREATE TRIGGER online_refund AFTER INSERT ON online_refunds FOR EACH ROW EXECUTE FUNCTION online_refund();

CREATE TRIGGER online_refund_immutable BEFORE DELETE OR UPDATE ON online_refunds FOR EACH ROW EXECUTE FUNCTION order_snapshot_guard();

CREATE TRIGGER online_settlement AFTER INSERT ON order_online_payments FOR EACH ROW EXECUTE FUNCTION online_settlement();

CREATE TRIGGER online_settlement_immutable BEFORE DELETE OR UPDATE ON order_online_payments FOR EACH ROW EXECUTE FUNCTION order_snapshot_guard();

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON admin_role_permissions FOR EACH ROW EXECUTE FUNCTION record_operation_event('access', 'role_key');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON admin_roles FOR EACH ROW EXECUTE FUNCTION record_operation_event('access', 'key');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON administrators FOR EACH ROW EXECUTE FUNCTION record_operation_event('access', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON catalog_brands FOR EACH ROW EXECUTE FUNCTION record_operation_event('catalog', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON catalog_brands_translations FOR EACH ROW EXECUTE FUNCTION record_operation_event('catalog', 'entity_id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON catalog_categories FOR EACH ROW EXECUTE FUNCTION record_operation_event('catalog', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON catalog_categories_translations FOR EACH ROW EXECUTE FUNCTION record_operation_event('catalog', 'entity_id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON catalog_products FOR EACH ROW EXECUTE FUNCTION record_operation_event('catalog', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON catalog_products_translations FOR EACH ROW EXECUTE FUNCTION record_operation_event('catalog', 'entity_id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON catalog_variants FOR EACH ROW EXECUTE FUNCTION record_operation_event('catalog', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON coupons FOR EACH ROW EXECUTE FUNCTION record_operation_event('settings', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON email_configuration FOR EACH ROW EXECUTE FUNCTION record_operation_event('settings', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON information_pages FOR EACH ROW EXECUTE FUNCTION record_operation_event('pages', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON information_pages_translations FOR EACH ROW EXECUTE FUNCTION record_operation_event('pages', 'entity_id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON online_refunds FOR EACH ROW EXECUTE FUNCTION record_operation_event('orders', 'attempt_id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON order_fulfillment_events FOR EACH ROW EXECUTE FUNCTION record_operation_event('orders', 'order_id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON order_manual_payments FOR EACH ROW EXECUTE FUNCTION record_operation_event('orders', 'order_id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON order_online_payments FOR EACH ROW EXECUTE FUNCTION record_operation_event('orders', 'order_id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON order_statuses FOR EACH ROW EXECUTE FUNCTION record_operation_event('settings', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON orders FOR EACH ROW EXECUTE FUNCTION record_operation_event('orders', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON payment_methods FOR EACH ROW EXECUTE FUNCTION record_operation_event('settings', 'code');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON platform_regional_settings FOR EACH ROW EXECUTE FUNCTION record_operation_event('settings', 'singleton');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON shipping_methods FOR EACH ROW EXECUTE FUNCTION record_operation_event('settings', 'code');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON store_settings FOR EACH ROW EXECUTE FUNCTION record_operation_event('settings', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON storefront_settings FOR EACH ROW EXECUTE FUNCTION record_operation_event('settings', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON tax_classes FOR EACH ROW EXECUTE FUNCTION record_operation_event('settings', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON tax_rates FOR EACH ROW EXECUTE FUNCTION record_operation_event('settings', 'id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON tax_zone_countries FOR EACH ROW EXECUTE FUNCTION record_operation_event('settings', 'zone_id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON tax_zone_rules FOR EACH ROW EXECUTE FUNCTION record_operation_event('settings', 'zone_id');

CREATE TRIGGER operation_event AFTER INSERT OR DELETE OR UPDATE ON tax_zones FOR EACH ROW EXECUTE FUNCTION record_operation_event('settings', 'id');

CREATE TRIGGER operation_event_immutable BEFORE DELETE OR UPDATE OR TRUNCATE ON operation_events FOR EACH STATEMENT EXECUTE FUNCTION operation_event_immutable();

CREATE TRIGGER order_current_status_guard BEFORE DELETE OR UPDATE ON order_current_status FOR EACH ROW EXECUTE FUNCTION order_current_status_guard();

CREATE TRIGGER order_customer_summary_immutable BEFORE DELETE OR UPDATE ON order_customer_summaries FOR EACH ROW EXECUTE FUNCTION order_editor_guard();

CREATE TRIGGER order_fulfillment_event_immutable BEFORE DELETE OR UPDATE ON order_fulfillment_events FOR EACH ROW EXECUTE FUNCTION order_snapshot_guard();

CREATE TRIGGER order_fulfillment_guard BEFORE INSERT OR DELETE OR UPDATE ON order_fulfillment FOR EACH ROW EXECUTE FUNCTION order_fulfillment_guard();

CREATE TRIGGER order_line_guard BEFORE DELETE OR UPDATE ON order_lines FOR EACH ROW EXECUTE FUNCTION order_editor_guard();

CREATE TRIGGER order_snapshot_guard BEFORE UPDATE ON orders
 FOR EACH ROW WHEN (OLD.snapshot IS NOT DISTINCT FROM NEW.snapshot
   AND OLD.merchandise_cents=NEW.merchandise_cents AND OLD.shipping_cents=NEW.shipping_cents
   AND OLD.fee_cents=NEW.fee_cents AND OLD.net_cents=NEW.net_cents
   AND OLD.tax_cents=NEW.tax_cents AND OLD.total_cents=NEW.total_cents)
 EXECUTE FUNCTION order_snapshot_guard();
CREATE TRIGGER order_editor_guard BEFORE UPDATE ON orders
 FOR EACH ROW WHEN (OLD.snapshot IS DISTINCT FROM NEW.snapshot
   OR OLD.merchandise_cents<>NEW.merchandise_cents OR OLD.shipping_cents<>NEW.shipping_cents
   OR OLD.fee_cents<>NEW.fee_cents OR OLD.net_cents<>NEW.net_cents
   OR OLD.tax_cents<>NEW.tax_cents OR OLD.total_cents<>NEW.total_cents)
 EXECUTE FUNCTION order_editor_guard();
CREATE TRIGGER order_delete_guard BEFORE DELETE ON orders
 FOR EACH ROW EXECUTE FUNCTION order_snapshot_guard();

CREATE TRIGGER order_status_event_immutable BEFORE DELETE OR UPDATE ON order_status_events FOR EACH ROW EXECUTE FUNCTION order_snapshot_guard();

CREATE TRIGGER order_status_initialize AFTER INSERT ON orders FOR EACH ROW EXECUTE FUNCTION order_status_initialize();

CREATE TRIGGER order_status_record AFTER INSERT OR UPDATE ON order_current_status FOR EACH ROW EXECUTE FUNCTION order_status_record();

CREATE TRIGGER order_status_role_immutable BEFORE DELETE OR UPDATE ON order_status_roles FOR EACH ROW EXECUTE FUNCTION order_snapshot_guard();

CREATE TRIGGER order_status_sync AFTER INSERT OR UPDATE OF state ON order_fulfillment FOR EACH ROW EXECUTE FUNCTION order_status_sync();

CREATE TRIGGER order_status_sync AFTER UPDATE OF state, payment_state ON orders FOR EACH ROW EXECUTE FUNCTION order_status_sync();

CREATE TRIGGER order_statuses_capacity_guard BEFORE INSERT ON order_statuses FOR EACH ROW EXECUTE FUNCTION order_statuses_capacity_guard();

CREATE TRIGGER order_stock_guard BEFORE DELETE OR UPDATE ON order_stock_moves FOR EACH ROW EXECUTE FUNCTION order_snapshot_guard();

CREATE TRIGGER order_stock_structure BEFORE INSERT OR UPDATE ON catalog_product_options FOR EACH ROW EXECUTE FUNCTION order_stock_structure_guard();

CREATE TRIGGER payment_attempt_guard BEFORE INSERT OR DELETE OR UPDATE ON payment_attempts FOR EACH ROW EXECUTE FUNCTION payment_attempt_guard();

CREATE TRIGGER payment_attempt_mode_guard BEFORE UPDATE ON payment_attempts FOR EACH ROW EXECUTE FUNCTION payment_attempt_mode_guard();

CREATE TRIGGER payment_initial_status_guard BEFORE INSERT OR UPDATE OF initial_order_status_id ON payment_methods FOR EACH ROW EXECUTE FUNCTION payment_initial_status_guard();

CREATE TRIGGER payment_methods_guard BEFORE INSERT OR UPDATE ON payment_methods FOR EACH ROW EXECUTE FUNCTION payment_methods_guard();

CREATE TRIGGER payment_provider_immutable BEFORE UPDATE ON payment_attempts FOR EACH ROW EXECUTE FUNCTION payment_provider_guard();

CREATE TRIGGER pricing_products_mode BEFORE UPDATE OF price_mode ON pricing_settings FOR EACH ROW EXECUTE FUNCTION product_mode_guard();

CREATE TRIGGER pricing_settings_guard BEFORE INSERT OR DELETE OR UPDATE ON pricing_settings FOR EACH ROW EXECUTE FUNCTION tax_configuration_guard();

CREATE TRIGGER pricing_settings_revision AFTER INSERT OR DELETE OR UPDATE ON pricing_settings FOR EACH STATEMENT EXECUTE FUNCTION tax_revision_changed();

CREATE TRIGGER product_gallery_guard BEFORE INSERT OR DELETE OR UPDATE ON catalog_product_images FOR EACH ROW EXECUTE FUNCTION product_gallery_guard();

CREATE TRIGGER product_media_reference AFTER INSERT OR DELETE OR UPDATE OF image_key ON catalog_products FOR EACH ROW EXECUTE FUNCTION media_track_product();

CREATE TRIGGER product_options_guard BEFORE INSERT OR DELETE OR UPDATE ON catalog_product_options FOR EACH ROW EXECUTE FUNCTION product_options_guard();

CREATE TRIGGER product_price_guard BEFORE INSERT OR DELETE OR UPDATE ON catalog_products FOR EACH ROW EXECUTE FUNCTION product_price_guard();

CREATE TRIGGER record_manual_payment AFTER INSERT ON order_manual_payments FOR EACH ROW EXECUTE FUNCTION record_manual_payment();

CREATE CONSTRAINT TRIGGER refund_cancellation_guard AFTER INSERT ON online_refunds DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION refund_cancellation_guard();

CREATE TRIGGER shipped_order_cancellation_guard BEFORE UPDATE OF state ON orders FOR EACH ROW EXECUTE FUNCTION shipped_order_cancellation_guard();

CREATE TRIGGER shipping_countries_revision AFTER INSERT OR DELETE OR UPDATE ON shipping_method_countries FOR EACH STATEMENT EXECUTE FUNCTION shipping_revision_changed();

CREATE TRIGGER shipping_methods_guard BEFORE INSERT OR UPDATE ON shipping_methods FOR EACH ROW EXECUTE FUNCTION shipping_methods_guard();

CREATE TRIGGER stock_revision BEFORE UPDATE ON catalog_products FOR EACH ROW EXECUTE FUNCTION advance_stock_revision();

CREATE TRIGGER stock_revision BEFORE UPDATE ON catalog_variants FOR EACH ROW EXECUTE FUNCTION advance_stock_revision();

CREATE TRIGGER store_currency_catalog BEFORE UPDATE OF currency ON store_settings FOR EACH ROW EXECUTE FUNCTION currency_catalog_guard();

CREATE TRIGGER store_currency_revision AFTER UPDATE OF currency ON store_settings FOR EACH STATEMENT EXECUTE FUNCTION tax_revision_changed();

CREATE TRIGGER store_media_reference AFTER INSERT OR DELETE OR UPDATE OF logo_key, favicon_key ON store_settings FOR EACH ROW EXECUTE FUNCTION media_track_store();

CREATE TRIGGER tax_classes_guard BEFORE INSERT OR DELETE OR UPDATE ON tax_classes FOR EACH ROW EXECUTE FUNCTION tax_configuration_guard();

CREATE TRIGGER tax_classes_revision AFTER INSERT OR DELETE OR UPDATE ON tax_classes FOR EACH STATEMENT EXECUTE FUNCTION tax_revision_changed();

CREATE TRIGGER tax_classes_zones_guard BEFORE UPDATE OF active ON tax_classes FOR EACH ROW EXECUTE FUNCTION tax_zone_guard();

CREATE TRIGGER tax_rates_guard BEFORE INSERT OR DELETE OR UPDATE ON tax_rates FOR EACH ROW EXECUTE FUNCTION tax_configuration_guard();

CREATE TRIGGER tax_rates_revision AFTER INSERT OR DELETE OR UPDATE ON tax_rates FOR EACH STATEMENT EXECUTE FUNCTION tax_revision_changed();

CREATE TRIGGER tax_rates_zones_guard BEFORE UPDATE OF active ON tax_rates FOR EACH ROW EXECUTE FUNCTION tax_zone_guard();

CREATE TRIGGER tax_zone_countries_guard BEFORE INSERT OR DELETE OR UPDATE ON tax_zone_countries FOR EACH ROW EXECUTE FUNCTION tax_zone_guard();

CREATE CONSTRAINT TRIGGER tax_zone_countries_nonempty AFTER INSERT OR DELETE OR UPDATE ON tax_zone_countries DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION tax_zone_nonempty_guard();

CREATE TRIGGER tax_zone_countries_revision AFTER INSERT OR DELETE OR UPDATE ON tax_zone_countries FOR EACH STATEMENT EXECUTE FUNCTION tax_revision_changed();

CREATE TRIGGER tax_zone_rules_guard BEFORE INSERT OR DELETE OR UPDATE ON tax_zone_rules FOR EACH ROW EXECUTE FUNCTION tax_zone_guard();

CREATE TRIGGER tax_zone_rules_revision AFTER INSERT OR DELETE OR UPDATE ON tax_zone_rules FOR EACH STATEMENT EXECUTE FUNCTION tax_revision_changed();

CREATE TRIGGER tax_zones_guard BEFORE INSERT OR DELETE OR UPDATE ON tax_zones FOR EACH ROW EXECUTE FUNCTION tax_zone_guard();

CREATE CONSTRAINT TRIGGER tax_zones_nonempty AFTER INSERT OR UPDATE ON tax_zones DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION tax_zone_nonempty_guard();

CREATE TRIGGER tax_zones_revision AFTER INSERT OR DELETE OR UPDATE ON tax_zones FOR EACH STATEMENT EXECUTE FUNCTION tax_revision_changed();

CREATE TRIGGER variant_guard BEFORE INSERT OR DELETE OR UPDATE ON catalog_variants FOR EACH ROW EXECUTE FUNCTION variant_guard();

CREATE TRIGGER variant_media_reference AFTER INSERT OR DELETE OR UPDATE OF image_key ON catalog_variants FOR EACH ROW EXECUTE FUNCTION media_track_product();

CREATE TRIGGER variant_values_sync AFTER INSERT OR UPDATE OF value_ids ON catalog_variants FOR EACH ROW EXECUTE FUNCTION variant_values_sync();

ALTER TABLE ONLY admin_role_permissions
    ADD CONSTRAINT admin_role_permissions_role_key_fkey FOREIGN KEY (role_key) REFERENCES admin_roles(key) ON DELETE CASCADE;

ALTER TABLE ONLY admin_sessions
    ADD CONSTRAINT admin_sessions_administrator_id_fkey FOREIGN KEY (administrator_id) REFERENCES administrators(id) ON DELETE CASCADE;

ALTER TABLE ONLY administrators
    ADD CONSTRAINT administrators_avatar_key_fkey FOREIGN KEY (avatar_key) REFERENCES media_objects(key);

ALTER TABLE ONLY administrators
    ADD CONSTRAINT administrators_locale_fkey FOREIGN KEY (locale) REFERENCES administrator_languages(code);

ALTER TABLE ONLY administrators
    ADD CONSTRAINT administrators_role_fkey FOREIGN KEY (role) REFERENCES admin_roles(key) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_attribute_groups_translations
    ADD CONSTRAINT catalog_attribute_groups_translations_entity_id_fkey FOREIGN KEY (entity_id) REFERENCES catalog_attribute_groups(id) ON DELETE CASCADE;

ALTER TABLE ONLY catalog_attribute_groups_translations
    ADD CONSTRAINT catalog_attribute_groups_translations_locale_fkey FOREIGN KEY (locale) REFERENCES administrator_languages(code);

ALTER TABLE ONLY catalog_attributes
    ADD CONSTRAINT catalog_attributes_group_id_fkey FOREIGN KEY (group_id) REFERENCES catalog_attribute_groups(id) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_attributes_translations
    ADD CONSTRAINT catalog_attributes_translations_entity_id_fkey FOREIGN KEY (entity_id) REFERENCES catalog_attributes(id) ON DELETE CASCADE;

ALTER TABLE ONLY catalog_attributes_translations
    ADD CONSTRAINT catalog_attributes_translations_locale_fkey FOREIGN KEY (locale) REFERENCES administrator_languages(code);

ALTER TABLE ONLY catalog_brands
    ADD CONSTRAINT catalog_brands_logo_key_fkey FOREIGN KEY (logo_key) REFERENCES media_objects(key) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_brands_translations
    ADD CONSTRAINT catalog_brands_translations_entity_id_fkey FOREIGN KEY (entity_id) REFERENCES catalog_brands(id) ON DELETE CASCADE;

ALTER TABLE ONLY catalog_brands_translations
    ADD CONSTRAINT catalog_brands_translations_locale_fkey FOREIGN KEY (locale) REFERENCES administrator_languages(code);

ALTER TABLE ONLY catalog_categories
    ADD CONSTRAINT catalog_categories_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES catalog_categories(id) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_categories_translations
    ADD CONSTRAINT catalog_categories_translations_entity_id_fkey FOREIGN KEY (entity_id) REFERENCES catalog_categories(id) ON DELETE CASCADE;

ALTER TABLE ONLY catalog_categories_translations
    ADD CONSTRAINT catalog_categories_translations_locale_fkey FOREIGN KEY (locale) REFERENCES administrator_languages(code);

ALTER TABLE ONLY catalog_option_values
    ADD CONSTRAINT catalog_option_values_option_id_fkey FOREIGN KEY (option_id) REFERENCES catalog_options(id) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_option_values_translations
    ADD CONSTRAINT catalog_option_values_translations_entity_id_fkey FOREIGN KEY (entity_id) REFERENCES catalog_option_values(id) ON DELETE CASCADE;

ALTER TABLE ONLY catalog_option_values_translations
    ADD CONSTRAINT catalog_option_values_translations_locale_fkey FOREIGN KEY (locale) REFERENCES administrator_languages(code);

ALTER TABLE ONLY catalog_options_translations
    ADD CONSTRAINT catalog_options_translations_entity_id_fkey FOREIGN KEY (entity_id) REFERENCES catalog_options(id) ON DELETE CASCADE;

ALTER TABLE ONLY catalog_options_translations
    ADD CONSTRAINT catalog_options_translations_locale_fkey FOREIGN KEY (locale) REFERENCES administrator_languages(code);

ALTER TABLE ONLY catalog_product_attributes
    ADD CONSTRAINT catalog_product_attributes_attribute_id_fkey FOREIGN KEY (attribute_id) REFERENCES catalog_attributes(id) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_product_attributes
    ADD CONSTRAINT catalog_product_attributes_product_id_fkey FOREIGN KEY (product_id) REFERENCES catalog_products(id) ON DELETE CASCADE;

ALTER TABLE ONLY catalog_product_attributes_translations
    ADD CONSTRAINT catalog_product_attributes_translations_entity_id_fkey FOREIGN KEY (entity_id) REFERENCES catalog_product_attributes(id) ON DELETE CASCADE;

ALTER TABLE ONLY catalog_product_attributes_translations
    ADD CONSTRAINT catalog_product_attributes_translations_locale_fkey FOREIGN KEY (locale) REFERENCES administrator_languages(code);

ALTER TABLE ONLY catalog_product_images
    ADD CONSTRAINT catalog_product_images_image_key_fkey FOREIGN KEY (image_key) REFERENCES media_objects(key) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_product_images
    ADD CONSTRAINT catalog_product_images_product_id_fkey FOREIGN KEY (product_id) REFERENCES catalog_products(id) ON DELETE CASCADE;

ALTER TABLE ONLY catalog_product_options
    ADD CONSTRAINT catalog_product_options_option_id_fkey FOREIGN KEY (option_id) REFERENCES catalog_options(id) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_product_options
    ADD CONSTRAINT catalog_product_options_product_id_fkey FOREIGN KEY (product_id) REFERENCES catalog_products(id) ON DELETE CASCADE;

ALTER TABLE ONLY catalog_products
    ADD CONSTRAINT catalog_products_brand_id_fkey FOREIGN KEY (brand_id) REFERENCES catalog_brands(id) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_products
    ADD CONSTRAINT catalog_products_category_id_fkey FOREIGN KEY (category_id) REFERENCES catalog_categories(id) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_products
    ADD CONSTRAINT catalog_products_image_key_fkey FOREIGN KEY (image_key) REFERENCES media_objects(key) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_products
    ADD CONSTRAINT catalog_products_tax_class_id_fkey FOREIGN KEY (tax_class_id) REFERENCES tax_classes(id) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_products_translations
    ADD CONSTRAINT catalog_products_translations_entity_id_fkey FOREIGN KEY (entity_id) REFERENCES catalog_products(id) ON DELETE CASCADE;

ALTER TABLE ONLY catalog_products_translations
    ADD CONSTRAINT catalog_products_translations_locale_fkey FOREIGN KEY (locale) REFERENCES administrator_languages(code);

ALTER TABLE ONLY catalog_variant_values
    ADD CONSTRAINT catalog_variant_values_value_id_fkey FOREIGN KEY (value_id) REFERENCES catalog_option_values(id) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_variant_values
    ADD CONSTRAINT catalog_variant_values_variant_id_fkey FOREIGN KEY (variant_id) REFERENCES catalog_variants(id) ON DELETE CASCADE;

ALTER TABLE ONLY catalog_variants
    ADD CONSTRAINT catalog_variants_image_key_fkey FOREIGN KEY (image_key) REFERENCES media_objects(key) ON DELETE RESTRICT;

ALTER TABLE ONLY catalog_variants
    ADD CONSTRAINT catalog_variants_product_id_fkey FOREIGN KEY (product_id) REFERENCES catalog_products(id) ON DELETE CASCADE;

ALTER TABLE ONLY checkout_countries
    ADD CONSTRAINT checkout_countries_country_code_fkey FOREIGN KEY (country_code) REFERENCES countries(code) ON DELETE RESTRICT;

ALTER TABLE ONLY checkout_details
    ADD CONSTRAINT checkout_details_cart_id_fkey FOREIGN KEY (cart_id) REFERENCES guest_carts(id) ON DELETE CASCADE;

ALTER TABLE ONLY checkout_methods
    ADD CONSTRAINT checkout_methods_cart_id_fkey FOREIGN KEY (cart_id) REFERENCES guest_carts(id) ON DELETE CASCADE;

ALTER TABLE ONLY checkout_methods
    ADD CONSTRAINT checkout_methods_payment_method_id_fkey FOREIGN KEY (payment_method_id) REFERENCES payment_methods(id) ON DELETE SET NULL;

ALTER TABLE ONLY checkout_methods
    ADD CONSTRAINT checkout_methods_shipping_method_id_fkey FOREIGN KEY (shipping_method_id) REFERENCES shipping_methods(id) ON DELETE SET NULL;

ALTER TABLE ONLY coupon_uses
    ADD CONSTRAINT coupon_uses_coupon_id_fkey FOREIGN KEY (coupon_id) REFERENCES coupons(id) ON DELETE RESTRICT;

ALTER TABLE ONLY coupon_uses
    ADD CONSTRAINT coupon_uses_order_id_fkey FOREIGN KEY (order_id) REFERENCES orders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY customer_addresses
    ADD CONSTRAINT customer_addresses_country_fkey FOREIGN KEY (country) REFERENCES countries(code) ON DELETE RESTRICT;

ALTER TABLE ONLY customer_addresses
    ADD CONSTRAINT customer_addresses_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES customers(id) ON DELETE RESTRICT;

ALTER TABLE ONLY customer_billing
    ADD CONSTRAINT customer_billing_billing_country_fkey FOREIGN KEY (billing_country) REFERENCES countries(code) ON DELETE RESTRICT;

ALTER TABLE ONLY customer_billing
    ADD CONSTRAINT customer_billing_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES customers(id) ON DELETE RESTRICT;

ALTER TABLE ONLY customer_email_confirmations
    ADD CONSTRAINT customer_email_confirmations_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES customers(id) ON DELETE RESTRICT;

ALTER TABLE ONLY customer_group_configuration
    ADD CONSTRAINT customer_group_configuration_default_group_id_fkey FOREIGN KEY (default_group_id) REFERENCES customer_groups(id) ON DELETE RESTRICT;

ALTER TABLE ONLY customer_group_quantity_tiers
    ADD CONSTRAINT customer_group_quantity_tiers_group_id_fkey FOREIGN KEY (group_id) REFERENCES customer_groups(id) ON DELETE CASCADE;

ALTER TABLE ONLY customer_orders
    ADD CONSTRAINT customer_orders_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES customers(id) ON DELETE RESTRICT;

ALTER TABLE ONLY customer_orders
    ADD CONSTRAINT customer_orders_order_id_fkey FOREIGN KEY (order_id) REFERENCES orders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY customer_password_resets
    ADD CONSTRAINT customer_password_resets_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES customers(id) ON DELETE RESTRICT;

ALTER TABLE ONLY customer_sessions
    ADD CONSTRAINT customer_sessions_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES customers(id) ON DELETE RESTRICT;

ALTER TABLE ONLY customers
    ADD CONSTRAINT customers_group_id_fkey FOREIGN KEY (group_id) REFERENCES customer_groups(id) ON DELETE RESTRICT;

ALTER TABLE ONLY email_attempts
    ADD CONSTRAINT email_attempts_message_id_fkey FOREIGN KEY (message_id) REFERENCES email_outbox(id);

ALTER TABLE ONLY email_outbox
    ADD CONSTRAINT email_outbox_order_id_fkey FOREIGN KEY (order_id) REFERENCES orders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY guest_cart_items
    ADD CONSTRAINT guest_cart_items_cart_id_fkey FOREIGN KEY (cart_id) REFERENCES guest_carts(id) ON DELETE CASCADE;

ALTER TABLE ONLY guest_cart_items
    ADD CONSTRAINT guest_cart_items_product_id_fkey FOREIGN KEY (product_id) REFERENCES catalog_products(id) ON DELETE SET NULL;

ALTER TABLE ONLY guest_cart_items
    ADD CONSTRAINT guest_cart_items_variant_id_fkey FOREIGN KEY (variant_id) REFERENCES catalog_variants(id) ON DELETE SET NULL;

ALTER TABLE ONLY guest_carts
    ADD CONSTRAINT guest_carts_coupon_id_fkey FOREIGN KEY (coupon_id) REFERENCES coupons(id) ON DELETE RESTRICT;

ALTER TABLE ONLY guest_carts
    ADD CONSTRAINT guest_carts_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES customers(id);

ALTER TABLE ONLY home_banners
    ADD CONSTRAINT home_banners_image_key_fkey FOREIGN KEY (image_key) REFERENCES media_library(key) ON DELETE RESTRICT;

ALTER TABLE ONLY home_banners_translations
    ADD CONSTRAINT home_banners_translations_entity_id_fkey FOREIGN KEY (entity_id) REFERENCES home_banners(id) ON DELETE CASCADE;

ALTER TABLE ONLY home_banners_translations
    ADD CONSTRAINT home_banners_translations_locale_fkey FOREIGN KEY (locale) REFERENCES administrator_languages(code);

ALTER TABLE ONLY information_page_slugs
    ADD CONSTRAINT information_page_slugs_page_id_fkey FOREIGN KEY (page_id) REFERENCES information_pages(id) ON DELETE CASCADE;

ALTER TABLE ONLY information_pages_translations
    ADD CONSTRAINT information_pages_translations_entity_id_fkey FOREIGN KEY (entity_id) REFERENCES information_pages(id) ON DELETE CASCADE;

ALTER TABLE ONLY information_pages_translations
    ADD CONSTRAINT information_pages_translations_locale_fkey FOREIGN KEY (locale) REFERENCES administrator_languages(code);

ALTER TABLE ONLY media_cache_jobs
    ADD CONSTRAINT media_cache_jobs_source_key_fkey FOREIGN KEY (source_key) REFERENCES media_objects(key) ON DELETE CASCADE;

ALTER TABLE ONLY media_folders
    ADD CONSTRAINT media_folders_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES administrators(id) ON DELETE RESTRICT;

ALTER TABLE ONLY media_folders
    ADD CONSTRAINT media_folders_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES media_folders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY media_library
    ADD CONSTRAINT media_library_folder_id_fkey FOREIGN KEY (folder_id) REFERENCES media_folders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY media_library
    ADD CONSTRAINT media_library_key_fkey FOREIGN KEY (key) REFERENCES media_objects(key) ON DELETE CASCADE;

ALTER TABLE ONLY media_library
    ADD CONSTRAINT media_library_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES administrators(id) ON DELETE RESTRICT;

ALTER TABLE ONLY media_locations
    ADD CONSTRAINT media_locations_key_fkey FOREIGN KEY (key) REFERENCES media_objects(key) ON DELETE CASCADE;

ALTER TABLE ONLY media_renditions
    ADD CONSTRAINT media_renditions_object_key_fkey FOREIGN KEY (object_key) REFERENCES media_objects(key) ON DELETE CASCADE;

ALTER TABLE ONLY media_renditions
    ADD CONSTRAINT media_renditions_source_key_fkey FOREIGN KEY (source_key) REFERENCES media_objects(key) ON DELETE CASCADE;

ALTER TABLE ONLY media_transforms
    ADD CONSTRAINT media_transforms_object_key_fkey FOREIGN KEY (object_key) REFERENCES media_objects(key) ON DELETE CASCADE;

ALTER TABLE ONLY media_transforms
    ADD CONSTRAINT media_transforms_source_key_fkey FOREIGN KEY (source_key) REFERENCES media_objects(key) ON DELETE CASCADE;

ALTER TABLE ONLY media_variants
    ADD CONSTRAINT media_variants_object_key_fkey FOREIGN KEY (object_key) REFERENCES media_objects(key) ON DELETE CASCADE;

ALTER TABLE ONLY media_variants
    ADD CONSTRAINT media_variants_source_key_fkey FOREIGN KEY (source_key) REFERENCES media_objects(key) ON DELETE CASCADE;

ALTER TABLE ONLY online_captures
    ADD CONSTRAINT online_captures_attempt_id_fkey FOREIGN KEY (attempt_id) REFERENCES payment_attempts(id) ON DELETE RESTRICT;

ALTER TABLE ONLY online_captures
    ADD CONSTRAINT online_captures_attempt_id_provider_id_fkey FOREIGN KEY (attempt_id, provider_id) REFERENCES payment_attempts(id, provider_id) ON DELETE RESTRICT;

ALTER TABLE ONLY online_payment_events
    ADD CONSTRAINT online_payment_events_attempt_id_provider_id_fkey FOREIGN KEY (attempt_id, provider_id) REFERENCES payment_attempts(id, provider_id) ON DELETE RESTRICT;

ALTER TABLE ONLY online_refunds
    ADD CONSTRAINT online_refunds_attempt_id_fkey FOREIGN KEY (attempt_id) REFERENCES online_captures(attempt_id) ON DELETE RESTRICT;

ALTER TABLE ONLY online_refunds
    ADD CONSTRAINT online_refunds_attempt_id_provider_id_fkey FOREIGN KEY (attempt_id, provider_id) REFERENCES payment_attempts(id, provider_id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_current_status
    ADD CONSTRAINT order_current_status_order_id_fkey FOREIGN KEY (order_id) REFERENCES orders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_current_status
    ADD CONSTRAINT order_current_status_status_id_fkey FOREIGN KEY (status_id) REFERENCES order_statuses(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_customer_summaries
    ADD CONSTRAINT order_customer_summaries_order_id_fkey FOREIGN KEY (order_id) REFERENCES orders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_fulfillment_events
    ADD CONSTRAINT order_fulfillment_events_administrator_id_fkey FOREIGN KEY (administrator_id) REFERENCES administrators(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_fulfillment_events
    ADD CONSTRAINT order_fulfillment_events_order_id_fkey FOREIGN KEY (order_id) REFERENCES orders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_fulfillment
    ADD CONSTRAINT order_fulfillment_order_id_fkey FOREIGN KEY (order_id) REFERENCES orders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_lines
    ADD CONSTRAINT order_lines_order_id_fkey FOREIGN KEY (order_id) REFERENCES orders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_lines
    ADD CONSTRAINT order_lines_product_id_fkey FOREIGN KEY (product_id) REFERENCES catalog_products(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_lines
    ADD CONSTRAINT order_lines_variant_id_fkey FOREIGN KEY (variant_id) REFERENCES catalog_variants(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_manual_payments
    ADD CONSTRAINT order_manual_payments_administrator_id_fkey FOREIGN KEY (administrator_id) REFERENCES administrators(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_manual_payments
    ADD CONSTRAINT order_manual_payments_order_id_fkey FOREIGN KEY (order_id) REFERENCES orders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_online_payments
    ADD CONSTRAINT order_online_payments_attempt_id_fkey FOREIGN KEY (attempt_id) REFERENCES online_captures(attempt_id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_online_payments
    ADD CONSTRAINT order_online_payments_order_id_fkey FOREIGN KEY (order_id) REFERENCES orders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_status_events
    ADD CONSTRAINT order_status_events_order_id_fkey FOREIGN KEY (order_id) REFERENCES orders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_status_events
    ADD CONSTRAINT order_status_events_status_id_fkey FOREIGN KEY (status_id) REFERENCES order_statuses(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_status_roles
    ADD CONSTRAINT order_status_roles_status_id_fkey FOREIGN KEY (status_id) REFERENCES order_statuses(id) ON DELETE RESTRICT;

ALTER TABLE ONLY order_stock_moves
    ADD CONSTRAINT order_stock_moves_line_id_fkey FOREIGN KEY (line_id) REFERENCES order_lines(id) ON DELETE RESTRICT;

ALTER TABLE ONLY password_resets
    ADD CONSTRAINT password_resets_administrator_id_fkey FOREIGN KEY (administrator_id) REFERENCES administrators(id) ON DELETE CASCADE;

ALTER TABLE ONLY payment_attempts
    ADD CONSTRAINT payment_attempts_order_id_fkey FOREIGN KEY (order_id) REFERENCES orders(id) ON DELETE RESTRICT;

ALTER TABLE ONLY payment_methods
    ADD CONSTRAINT payment_methods_initial_order_status_fk FOREIGN KEY (initial_order_status_id) REFERENCES order_statuses(id) ON DELETE RESTRICT;

ALTER TABLE ONLY payment_methods
    ADD CONSTRAINT payment_methods_tax_class_id_fkey FOREIGN KEY (tax_class_id) REFERENCES tax_classes(id) ON DELETE RESTRICT;

ALTER TABLE ONLY pricing_settings
    ADD CONSTRAINT pricing_settings_country_code_fkey FOREIGN KEY (country_code) REFERENCES countries(code) ON DELETE RESTRICT;

ALTER TABLE ONLY pricing_settings
    ADD CONSTRAINT pricing_settings_default_class_id_fkey FOREIGN KEY (default_class_id) REFERENCES tax_classes(id) ON DELETE RESTRICT;

ALTER TABLE ONLY shipping_method_countries
    ADD CONSTRAINT shipping_method_countries_country_code_fkey FOREIGN KEY (country_code) REFERENCES checkout_countries(country_code) ON DELETE CASCADE;

ALTER TABLE ONLY shipping_method_countries
    ADD CONSTRAINT shipping_method_countries_method_id_fkey FOREIGN KEY (method_id) REFERENCES shipping_methods(id) ON DELETE CASCADE;

ALTER TABLE ONLY shipping_methods
    ADD CONSTRAINT shipping_methods_tax_class_id_fkey FOREIGN KEY (tax_class_id) REFERENCES tax_classes(id) ON DELETE RESTRICT;

ALTER TABLE ONLY store_settings
    ADD CONSTRAINT store_settings_favicon_key_fkey FOREIGN KEY (favicon_key) REFERENCES media_objects(key);

ALTER TABLE ONLY store_settings
    ADD CONSTRAINT store_settings_logo_key_fkey FOREIGN KEY (logo_key) REFERENCES media_objects(key);

ALTER TABLE ONLY storefront_language_prefixes
    ADD CONSTRAINT storefront_language_prefixes_code_fkey FOREIGN KEY (code) REFERENCES administrator_languages(code);

ALTER TABLE ONLY storefront_languages
    ADD CONSTRAINT storefront_languages_code_fkey FOREIGN KEY (code) REFERENCES administrator_languages(code);

ALTER TABLE ONLY storefront_settings
    ADD CONSTRAINT storefront_primary_language_fk FOREIGN KEY (primary_locale) REFERENCES storefront_languages(code) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY storefront_settings_translations
    ADD CONSTRAINT storefront_settings_translations_entity_id_fkey FOREIGN KEY (entity_id) REFERENCES storefront_settings(id) ON DELETE CASCADE;

ALTER TABLE ONLY storefront_settings_translations
    ADD CONSTRAINT storefront_settings_translations_locale_fkey FOREIGN KEY (locale) REFERENCES administrator_languages(code);

ALTER TABLE ONLY online_payment_events
    ADD CONSTRAINT online_payment_events_attempt_id_fkey FOREIGN KEY (attempt_id) REFERENCES payment_attempts(id) ON DELETE RESTRICT;

ALTER TABLE ONLY tax_classes
    ADD CONSTRAINT tax_classes_rate_id_fkey FOREIGN KEY (rate_id) REFERENCES tax_rates(id) ON DELETE RESTRICT;

ALTER TABLE ONLY tax_zone_countries
    ADD CONSTRAINT tax_zone_countries_country_code_fkey FOREIGN KEY (country_code) REFERENCES countries(code) ON DELETE RESTRICT;

ALTER TABLE ONLY tax_zone_countries
    ADD CONSTRAINT tax_zone_countries_zone_id_fkey FOREIGN KEY (zone_id) REFERENCES tax_zones(id) ON DELETE CASCADE;

ALTER TABLE ONLY tax_zone_rules
    ADD CONSTRAINT tax_zone_rules_class_id_fkey FOREIGN KEY (class_id) REFERENCES tax_classes(id) ON DELETE RESTRICT;

ALTER TABLE ONLY tax_zone_rules
    ADD CONSTRAINT tax_zone_rules_rate_id_fkey FOREIGN KEY (rate_id) REFERENCES tax_rates(id) ON DELETE RESTRICT;

ALTER TABLE ONLY tax_zone_rules
    ADD CONSTRAINT tax_zone_rules_zone_id_fkey FOREIGN KEY (zone_id) REFERENCES tax_zones(id) ON DELETE CASCADE;

ALTER TABLE ONLY catalog_option_values_translations
    ADD CONSTRAINT translation_value_owner_fk FOREIGN KEY (entity_id, option_id) REFERENCES catalog_option_values(id, option_id) ON DELETE CASCADE;

-- Administrative drafts and immutable commercial revision audit.
CREATE TABLE order_drafts (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    target_order_id bigint REFERENCES orders(id),
    target_revision bigint,
    revision bigint NOT NULL DEFAULT 0 CHECK (revision>=0),
    payload text NOT NULL CHECK (octet_length(payload)<=262144),
    administrator_id bigint NOT NULL REFERENCES administrators(id),
    committed_order_id bigint REFERENCES orders(id),
    created_at timestamptz NOT NULL DEFAULT statement_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT statement_timestamp(),
    CHECK ((target_order_id IS NULL)=(target_revision IS NULL))
);
CREATE INDEX order_drafts_open ON order_drafts(updated_at DESC,id DESC)
    WHERE committed_order_id IS NULL;

CREATE TABLE order_edit_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    order_id bigint NOT NULL REFERENCES orders(id),
    draft_id bigint NOT NULL UNIQUE REFERENCES order_drafts(id),
    revision bigint NOT NULL CHECK (revision>=0),
    administrator_id bigint NOT NULL REFERENCES administrators(id),
    administrator_name text NOT NULL,
    reason text NOT NULL CHECK (length(reason) BETWEEN 1 AND 1000),
    before_data text NOT NULL CHECK (octet_length(before_data)<=524288),
    after_data text NOT NULL CHECK (octet_length(after_data)<=524288),
    transaction_id xid8 NOT NULL DEFAULT pg_current_xact_id(),
    created_at timestamptz NOT NULL DEFAULT statement_timestamp(),
    UNIQUE(order_id,revision)
);
CREATE TRIGGER order_edit_event_immutable BEFORE DELETE OR UPDATE ON order_edit_events
    FOR EACH ROW EXECUTE FUNCTION order_snapshot_guard();
