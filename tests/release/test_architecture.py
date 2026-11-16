import importlib.util
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('architecture', ROOT / 'scripts/check-architecture.py')
architecture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(architecture)


class ArchitectureTests(unittest.TestCase):
    def fixture(self, root):
        (root / 'apps/talocart').mkdir(parents=True)
        (root / 'apps/talocart/Cargo.toml').write_text('[dependencies]\n')
        for crate in ('platform', 'commerce', 'web'):
            folder = root / 'crates' / crate
            (folder / 'src').mkdir(parents=True)
            (folder / 'Cargo.toml').write_text('[dependencies]\n')
        (root / 'crates/commerce/src/pricing.rs').write_text('pub struct Money(i64);')
        return root / 'crates/web/src/lib.rs'

    def test_real_checkout_respects_boundaries(self):
        self.assertEqual(architecture.violations(ROOT), [])

    def test_platform_templates_are_limited_to_confirmation_email(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            self.fixture(root)
            manifest = root / 'crates/platform/Cargo.toml'
            manifest.write_text('[dependencies]\naskama.workspace = true\n')
            confirmation = root / 'crates/platform/src/identity/customer_registration.rs'
            confirmation.parent.mkdir()
            confirmation.write_text('use askama::Template;')
            self.assertEqual(architecture.violations(root), [])
            other = root / 'crates/platform/src/settings.rs'
            for dependency, source in [
                ('askama.workspace = true', 'use askama::Template;'),
                ('renderer = {package="askama", version="0.14"}', 'use renderer::Template;'),
            ]:
                manifest.write_text('[dependencies]\n' + dependency + '\n')
                other.write_text(source)
                failures = architecture.violations(root)
                self.assertEqual(len(failures), 1)
                self.assertIn('Askama is limited to confirmation email', failures[0])
            other.unlink()
            manifest.write_text('[dependencies]\naskama.workspace = true\naxum = "0.8"\n')
            self.assertTrue(any('forbidden runtime/build dependency axum' in f
                                for f in architecture.violations(root)))

    def test_rejects_sql_after_test_module_and_ignores_literals(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            web = self.fixture(root)
            web.write_text('''// sqlx::query("example")
const X: &str = r##"sqlx::query("literal")"##;
/* outer /* nested */ sqlx::query */
#[cfg(test)] mod tests { fn test() { sqlx::query("test"); } }
async fn production() { sqlx::query("live"); }
''')
            failures = architecture.violations(root)
            self.assertEqual(len(failures), 1)
            self.assertIn(':5:', failures[0])

    def test_rejects_optional_test_composition_and_target_dependencies(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            self.fixture(root)
            manifest = root / 'crates/web/Cargo.toml'
            manifest.write_text('[dev-dependencies]\ntalocart-koala-import = "1"\n')
            self.assertEqual(len(architecture.violations(root)), 1)
            manifest.write_text('[target.\'cfg(unix)\'.dependencies]\nprivate = {package="talocart-koala-import", version="1"}\n')
            self.assertEqual(len(architecture.violations(root)), 1)

    def test_query_imports_and_pure_pricing_are_guarded(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            web = self.fixture(root)
            web.write_text('use sqlx::{query as run};')
            (root / 'crates/commerce/src/pricing.rs').write_text('use askama::Template;')
            self.assertEqual(len(architecture.violations(root)), 2)
            web.write_text('use sqlx::PgPool; struct X { m: sqlx::migrate::Migrator }')
            (root / 'crates/commerce/src/pricing.rs').write_text('fn f<\'a>(s: &\'a str) {}')
            self.assertEqual(architecture.violations(root), [])

    def test_transaction_opening_is_owned_by_services(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            web = self.fixture(root)
            web.write_text('''// pool.begin().await
const EXAMPLE: &str = "pool.begin_with()";
#[cfg(test)] mod tests { async fn f() { pool.begin().await; } }
async fn handler() { state.pool.begin().await; pool . begin_with("x").await; }
''')
            self.assertEqual(len(architecture.violations(root)), 2)
            web.write_text('async fn handler() { installation::install(&pool).await; }')
            self.assertEqual(architecture.violations(root), [])

    def test_catalog_writes_stay_outside_source_adapter(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            self.fixture(root)
            adapter = root / 'modules/koala-import/src/transform.rs'
            adapter.parent.mkdir(parents=True)
            adapter.write_text('''// UPDATE catalog_products SET name='comment'
#[cfg(test)] mod tests { fn fixture() { run("INSERT INTO catalog_products(name) VALUES('test')"); } }
fn live() {
 run("INSERT INTO catalog_import_products VALUES(1)");
 run("SELECT id FROM catalog_products");
 run("UPDATE catalog_products SET name='live'");
 run(r#"DELETE FROM catalog_variants WHERE id=1"#);
}
''')
            failures = architecture.violations(root)
            self.assertEqual(len(failures), 2)
            self.assertTrue(all('catalog writes belong to commerce' in f for f in failures))
            adapter.write_text('fn live() { writer.create_product(&input); }')
            self.assertEqual(architecture.violations(root), [])

    def test_capabilities_are_explicit_but_presentation_scopes_remain_valid(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            web = self.fixture(root)
            web.write_text('''// customers::current()
const EXAMPLE: &str = "cart::guest()";
#[cfg(test)] mod tests { tokio::task_local! { static GUEST: Context; } }
tokio::task_local! { static CUSTOMER: Context; }
fn handler() { cart::guest(); super::customers::current(); }
''')
            failures = architecture.violations(root)
            self.assertEqual(len(failures), 3)
            self.assertTrue(all('explicit extractors' in failure for failure in failures))
            web.write_text('''tokio::task_local! { static PRESENTATION: RefCell<View>; }
fn handler(cart: CartContext, customer: OptionalCustomer) { cart.guest(); }
''')
            self.assertEqual(architecture.violations(root), [])

    def test_payment_provider_cannot_own_database_access(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            self.fixture(root)
            provider = root / 'modules/stripe/src/gateway.rs'
            provider.parent.mkdir(parents=True)
            provider.write_text('''// sqlx::query("example")
const EXAMPLE: &str = "pool.begin()";
#[cfg(test)] mod tests { fn gateway_fixture() { sqlx::query("fixture"); } }
fn gateway() { client.send(); }
''')
            self.assertEqual(architecture.violations(root), [])
            provider.write_text(provider.read_text() + '\nfn live() { sqlx::query("UPDATE orders SET state=$1"); }')
            child = provider.with_suffix('') / 'refund.rs'
            child.parent.mkdir()
            child.write_text('async fn refund() { pool.begin().await; }')
            failures = architecture.violations(root)
            self.assertEqual(len(failures), 2)
            self.assertTrue(all('payment provider' in failure for failure in failures))
