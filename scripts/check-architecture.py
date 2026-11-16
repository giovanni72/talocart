#!/usr/bin/env python3
"""Check concrete dependency and HTTP/query boundaries; not a Rust type checker."""
import argparse
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
# Strip comments and literals, preserving offsets/newlines and Rust lifetime names.
LEXEME = re.compile(r'//[^\n]*|/\*|(?:br|r)(?P<hashes>\#*)"|b?"|b?\'(?:\\.|[^\'\\\n])\'')


def code_only(source, preserve_literals=False):
    chars = list(source)
    pos = 0
    while match := LEXEME.search(source, pos):
        start, end = match.span()
        token = match.group()
        if token == '/*':
            depth = 1
            while depth and end < len(source):
                nested = re.search(r'/\*|\*/', source[end:])
                if not nested:
                    end = len(source)
                    break
                depth += 1 if nested.group() == '/*' else -1
                end += nested.end()
        elif match.group('hashes') is not None:
            finish = source.find('"' + match.group('hashes'), end)
            end = len(source) if finish < 0 else finish + 1 + len(match.group('hashes'))
        elif token in ('"', 'b"'):
            while end < len(source):
                if source[end] == '\\':
                    end += 2
                elif source[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
        for i in range(start, min(end, len(chars))):
            if preserve_literals and token != '/*' and not token.startswith('//'):
                continue
            if chars[i] != '\n':
                chars[i] = ' '
        pos = end
    return ''.join(chars)


def production_code(source, preserve_literals=False):
    syntax = code_only(source)
    code = code_only(source, preserve_literals) if preserve_literals else syntax
    # Remove only explicitly test-gated items; never discard the rest of a file.
    for match in reversed(list(re.finditer(r'#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]', syntax))):
        brace = syntax.find('{', match.end())
        semi = syntax.find(';', match.end())
        if semi >= 0 and (brace < 0 or semi < brace):
            end = semi + 1
        elif brace >= 0:
            depth, end = 1, brace + 1
            while depth and end < len(code):
                depth += (syntax[end] == '{') - (syntax[end] == '}')
                end += 1
        else:
            end = len(code)
        code = code[:match.start()] + re.sub(r'[^\n]', ' ', code[match.start():end]) + code[end:]
    return code


def violations(root):
    failures = []
    forbidden = {
        'platform': {'talocart-commerce', 'talocart-web', 'talocart-koala-import', 'talocart-stripe', 'axum'},
        'commerce': {'talocart-web', 'talocart-koala-import', 'talocart-stripe', 'axum'},
        'web': {'talocart-koala-import', 'talocart-stripe'},
    }
    platform_template_names = {'askama'}
    for crate, names in forbidden.items():
        path = root / 'crates' / crate / 'Cargo.toml'
        manifest = tomllib.loads(path.read_text())
        sections = [manifest.get('dependencies', {}), manifest.get('build-dependencies', {})]
        for target in manifest.get('target', {}).values():
            sections += [target.get('dependencies', {}), target.get('build-dependencies', {})]
        for deps in sections:
            for alias, value in deps.items():
                name = value.get('package', alias) if isinstance(value, dict) else alias
                if crate == 'platform' and name == 'askama':
                    platform_template_names.add(alias.replace('-', '_'))
                if name in names:
                    failures.append(f'{path.relative_to(root)}: forbidden runtime/build dependency {name}')
    for manifest_path in [root / 'apps/talocart/Cargo.toml', *sorted((root / 'crates').glob('*/Cargo.toml'))]:
        manifest = tomllib.loads(manifest_path.read_text())
        for section in ('dependencies', 'dev-dependencies', 'build-dependencies'):
            for alias, value in manifest.get(section, {}).items():
                name = value.get('package', alias) if isinstance(value, dict) else alias
                if name in {'talocart-koala-import', 'talocart-stripe'}:
                    failures.append(f'{manifest_path.relative_to(root)}: optional implementation dependency {name}')
    # Platform owns confirmation tokens and their atomic outbox entry. Permit
    # typed email rendering there, without opening general UI dependencies.
    template_use = re.compile(r'\b(?:' + '|'.join(re.escape(n) for n in sorted(platform_template_names)) + r')\b')
    for path in sorted((root / 'crates/platform/src').rglob('*.rs')):
        if path.relative_to(root).as_posix() == 'crates/platform/src/identity/customer_registration.rs':
            continue
        code = production_code(path.read_text())
        for match in template_use.finditer(code):
            line = code.count('\n', 0, match.start()) + 1
            failures.append(f'{path.relative_to(root)}:{line}: Askama is limited to confirmation email')
    # Pool/migrator types are composition contracts. SQL construction/execution belongs to owners.
    sql = re.compile(r'\bsqlx\s*::\s*(?!PgPool\b|migrate\s*::)[A-Za-z_]|\buse\s+sqlx\s*::\s*\{|\.\s*(?:fetch_one|fetch_optional|fetch_all|fetch_many|execute|execute_many|begin|begin_with)\s*\(')
    # Prevent the retired capability scopes/helpers; rendering/locale scopes remain valid.
    # Named textual contracts only: this is not semantic data-flow analysis.
    implicit_context = re.compile(r'\btask_local\s*!\s*\{\s*static\s+(?:CUSTOMER|GUEST)\b|\b(?:customers\s*::\s*(?:current|id)|cart\s*::\s*(?:guest|source))\s*\(')
    for path in sorted((root / 'crates/web/src').rglob('*.rs')):
        code = production_code(path.read_text())
        for match in sql.finditer(code):
            line = code.count('\n', 0, match.start()) + 1
            failures.append(f'{path.relative_to(root)}:{line}: SQL access belongs to a data owner')
        for match in implicit_context.finditer(code):
            line = code.count('\n', 0, match.start()) + 1
            failures.append(f'{path.relative_to(root)}:{line}: request capabilities must use explicit extractors')
    module_root = root.parent if (root.parent / 'integration/Cargo.toml').is_file() else root
    # Source adapters own catalog_import_* receipts, never catalog persistence.
    # Static SQL only: dynamic/aliased table construction still requires review.
    commercial_write = re.compile(r'\b(?:INSERT\s+INTO|UPDATE|DELETE\s+FROM)\s+(?:[a-z_][a-z_0-9]*\.)?"?catalog_(?!import_)[a-z_0-9]+\b', re.IGNORECASE)
    for path in sorted((module_root / 'modules/koala-import/src').rglob('*.rs')):
        code = production_code(path.read_text(), preserve_literals=True)
        for match in commercial_write.finditer(code):
            line = code.count('\n', 0, match.start()) + 1
            failures.append(f'{path.relative_to(module_root)}:{line}: catalog writes belong to commerce owners')
    # The provider owns network/signature mechanics; commerce owns persistent
    # payment state. Check this concrete adapter and any future private children.
    provider_sql = re.compile(r'\bsqlx\s*::|\.\s*(?:fetch_one|fetch_optional|fetch_all|fetch_many|execute|execute_many|begin|begin_with)\s*\(')
    provider_root = module_root / 'modules/stripe/src/gateway'
    provider_paths = [module_root / 'modules/stripe/src/gateway.rs', *sorted(provider_root.rglob('*.rs'))]
    for path in provider_paths:
        if not path.is_file():
            continue
        code = production_code(path.read_text())
        for match in provider_sql.finditer(code):
            line = code.count('\n', 0, match.start()) + 1
            failures.append(f'{path.relative_to(module_root)}:{line}: payment provider must not own database access')
    pure = root / 'crates/commerce/src/pricing.rs'
    code = production_code(pure.read_text())
    if re.search(r'\b(?:sqlx|axum|askama|talocart_platform)\s*::', code):
        failures.append(f'{pure.relative_to(root)}: pricing must remain independent of I/O and presentation')
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    args = parser.parse_args()
    failures = violations(args.root)
    for failure in failures:
        print(failure, file=sys.stderr)
    if failures:
        return 1
    print('Architecture boundaries: dependencies, HTTP/query ownership, payment provider and pure pricing OK.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
