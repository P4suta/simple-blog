// Only deliberate exports enter CI artifacts. Databases, raw browser output,
// authentication state, raw traces and error-context.md remain private.
import { readdirSync, readFileSync, mkdirSync, writeFileSync, existsSync } from 'node:fs';
import { join, relative, resolve, dirname, sep } from 'node:path';
import { createHash, randomUUID } from 'node:crypto';
import { writeJson } from './runner.mjs';
import { safeEvidenceText, assertSafeEvidence } from './evidence-security.mjs';
import { summarizeBrowser } from './browser-evidence.mjs';
import { publishEvidence } from './publish-evidence.mjs';

const published = resolve('target/shareable');
const destination = resolve(`target/.shareable-${randomUUID()}`);
mkdirSync(destination, { recursive: true });
const index = [];
const omissions = [];
function save(name, bytes) {
  if (/(?:\.sqlite|\.db$|storageState|error-context|recovery-codes|cookies)/i.test(name)) throw new Error('Private evidence cannot be exported');
  const target = join(destination, name);
  mkdirSync(resolve(target, '..'), { recursive: true });
  writeFileSync(target, bytes, { mode: 0o600 });
  index.push({ path: name, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') });
}
function walk(directory) {
  if (!existsSync(directory)) return [];
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    if (entry.name === '_source') return [];
    const path = join(directory, entry.name);
    if (entry.isSymbolicLink()) throw new Error('Evidence symlink refused');
    return entry.isDirectory() ? walk(path) : [path];
  });
}
const verificationFiles = walk('target/verification');
const symbolFiles = new Set();
for (const profile of ['release', 'debug']) {
  const directory = `target/verification/symbols-${profile}`;
  if (!existsSync(directory)) continue;
  let manifest;
  try { manifest = JSON.parse(readFileSync(join(directory, 'symbols.json'))); }
  catch (error) {
    if (error.code !== 'ENOENT') throw error;
    // A failed build may create the directory before it has a validated pair.
    // Omit all unverified binaries, but keep independent failure diagnostics.
    omissions.push({ scope: `symbols-${profile}`, code: 'symbols.incomplete' });
    continue;
  }
  if (manifest.schema !== 1 || manifest.files.length !== 2) throw new Error('Incomplete symbol manifest');
  const names = manifest.files.map(entry => entry.name).sort().join(',');
  if (!['simple-blog.exe,simple_blog.pdb', 'simple-blog,simple-blog.debug'].includes(names)) throw new Error('Incomplete binary/symbol pair');
  symbolFiles.add(resolve(directory, 'symbols.json'));
  for (const entry of manifest.files) {
    if (!/^(?:simple-blog(?:\.exe|\.debug)?|simple_blog\.pdb)$/.test(entry.name)) throw new Error('Unclassified symbol artifact');
    const bytes = readFileSync(join(directory, entry.name));
    if (bytes.length !== entry.bytes || createHash('sha256').update(bytes).digest('hex') !== entry.sha256) throw new Error('Symbol artifacts do not match their identity manifest');
    symbolFiles.add(resolve(directory, entry.name));
  }
}
for (const path of verificationFiles) {
  const name = relative('target/verification', path).replaceAll('\\', '/');
  if (/^symbols-(?:release|debug)\//.test(name)) {
    if (symbolFiles.has(resolve(path))) save(`symbols/${name}`, readFileSync(path));
    continue;
  }
  if (name.includes('mutants.out') && !name.endsWith('outcomes.json')) continue;
  if (/(?:^|\/)browser-(?:results|report)\//.test(name)) continue;
  if (!/\.(?:json|log|info)$/.test(name)) continue;
  // Playwright's own diagnostic text can quote private form-fill arguments.
  if (/browser\.(?:stdout|stderr)\.log$/.test(name)) continue;
  const content = safeEvidenceText(readFileSync(path, 'utf8'));
  save(`verification/${name}`, Buffer.from(content));
}
const browserReports = verificationFiles.filter(path => path.replaceAll('\\', '/').endsWith('/browser-report/results.json'));
if (existsSync('target/browser-report/results.json')) browserReports.push('target/browser-report/results.json');
for (const path of browserReports) {
  const report = summarizeBrowser(JSON.parse(readFileSync(path)));
  const output = resolve(dirname(path), '../browser-results');
  const prefix = `browser/${createHash('sha256').update(resolve(path)).digest('hex').slice(0, 16)}`;
  for (const attachment of report.attachments) {
          if (attachment.name === 'server events' && attachment.body) {
            const bytes = Buffer.from(attachment.body, 'base64');
            if (bytes.toString('base64') !== attachment.body) throw new Error('Invalid inline log encoding');
            const name = createHash('sha256').update(bytes).digest('hex').slice(0, 16);
            save(`${prefix}/${name}.log`, Buffer.from(safeEvidenceText(bytes.toString('utf8'))));
            continue;
          }
          const source = resolve(attachment.path);
          if (!source.startsWith(output + sep)) throw new Error('Trace is outside the disposable test output');
          const bytes = readFileSync(source);
          const name = createHash('sha256').update(source).digest('hex').slice(0, 16);
          if (attachment.name === 'server events') {
            save(`${prefix}/${name}.log`, Buffer.from(safeEvidenceText(bytes.toString('utf8'))));
            continue;
          }
          const { unzipSync, strFromU8 } = await import('fflate');
          for (const part of Object.values(unzipSync(bytes))) assertSafeEvidence(strFromU8(part));
          save(`${prefix}/${name}.zip`, bytes);
  }
  save(`${prefix}/results.json`, Buffer.from(safeEvidenceText(JSON.stringify(report.summary))));
}
writeJson(join(destination, 'evidence.json'), { schema: 1, createdAt: new Date().toISOString(),
  status: omissions.length ? 'partial' : 'complete', omissions, files: index });
publishEvidence(destination, published);
console.log(`Prepared ${index.length} shareable evidence files`);
