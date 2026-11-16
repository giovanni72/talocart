// Deterministic local bundle and complete runtime dependency license inventory.
const fs = require('node:fs');
const path = require('node:path');
const esbuild = require('esbuild');
const out = 'assets/vendor/tiptap';
const check = process.argv.includes('--check');
fs.mkdirSync(out, { recursive: true });
const result = esbuild.buildSync({ entryPoints: ['assets/admin/rich-text.js'], outfile: out + '/rich-text.min.js', bundle: true, minify: true, target: ['es2020'], legalComments: 'eof', metafile: true, write: false });
const packages = new Map();
for (const input of Object.keys(result.metafile.inputs)) {
  let dir = path.dirname(path.resolve(input));
  while (!fs.existsSync(path.join(dir, 'package.json')) && dir !== path.dirname(dir)) dir = path.dirname(dir);
  if (!dir.includes('node_modules')) continue;
  const info = JSON.parse(fs.readFileSync(path.join(dir, 'package.json'), 'utf8'));
  if (packages.has(info.name)) continue;
  const license = fs.readdirSync(dir).find(name => /^licen[sc]e(?:\.md|\.txt)?$/i.test(name));
  if (!license) throw Error('Missing license for ' + info.name);
  if (!['MIT', 'ISC'].includes(info.license)) throw Error('Review license: ' + info.name + ' ' + info.license);
  packages.set(info.name, info.name + '@' + info.version + ' (' + info.license + ')\n' + fs.readFileSync(path.join(dir, license), 'utf8'));
}
const licenses = [...packages].sort(([a], [b]) => a.localeCompare(b, 'en')).map(([, text]) => text).join('\n\n----------------\n\n');
// Browser recipients receive the complete notices with the JavaScript itself.
if (licenses.includes('*/')) throw Error('License text cannot be embedded safely in a JS comment');
const bundle = Buffer.concat([Buffer.from(result.outputFiles[0].contents), Buffer.from('\n/*!\nThird-party runtime licenses\n\n' + licenses + '\n*/\n')]);
for (const [file, data] of [[out + '/rich-text.min.js', bundle], [out + '/LICENSES.txt', Buffer.from(licenses)]]) {
  if (check) {
    if (!fs.existsSync(file) || !fs.readFileSync(file).equals(Buffer.from(data))) throw Error('Stale editor artifact: ' + file + '; run npm run build:editor');
  } else fs.writeFileSync(file, data);
}
console.log((check ? 'Verified' : 'Bundled') + ' Tiptap locally; licenses included for ' + packages.size + ' packages.');
