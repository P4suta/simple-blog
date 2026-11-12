import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import {
	evidenceArray,
	evidenceObject,
	evidenceText,
} from "./evidence-shape.mjs";

/**
 * @param {string} path
 * @returns {Record<string, unknown>}
 */
function readObject(path) {
	return evidenceObject(JSON.parse(readFileSync(path, "utf8")));
}

const collector = fileURLToPath(
	new URL("./collect-evidence.mjs", import.meta.url),
);

test("a failed symbol build still exports its safe logs and explicitly reports the missing pair", (t) => {
	const root = mkdtempSync(join(tmpdir(), "simple-blog-partial-symbols-"));
	t.after(() => rmSync(root, { recursive: true, force: true }));
	const symbols = join(root, "target/verification/symbols-release");
	mkdirSync(symbols, { recursive: true });
	writeFileSync(join(symbols, "simple-blog.exe"), "PRIVATE_FIXTURE_CANARY");
	writeFileSync(
		join(root, "target/verification/build.log"),
		"OpenSSL configuration failed",
	);
	const result = spawnSync(process.execPath, [collector], {
		cwd: root,
		windowsHide: true,
	});
	assert.equal(result.status, 0, result.stderr.toString());
	const index = readObject(join(root, "target/shareable/evidence.json"));
	assert.equal(index.status, "partial");
	assert.deepEqual(index.omissions, [
		{ scope: "symbols-release", code: "symbols.incomplete" },
	]);
	assert.equal(evidenceArray(index.files).length, 1);
	assert.equal(
		readFileSync(join(root, "target/shareable/verification/build.log"), "utf8"),
		"OpenSSL configuration failed",
	);
});
test("failed secret inspection cannot publish partial evidence", (t) => {
	const root = mkdtempSync(join(tmpdir(), "simple-blog-evidence-"));
	t.after(() => rmSync(root, { recursive: true, force: true }));
	mkdirSync(join(root, "target/verification"), { recursive: true });
	writeFileSync(
		join(root, "target/verification/a.json"),
		'{"status":"failed"}',
	);
	writeFileSync(
		join(root, "target/verification/z.log"),
		"PRIVATE_FIXTURE_CANARY",
	);
	const result = spawnSync(process.execPath, [collector], {
		cwd: root,
		windowsHide: true,
	});
	assert.notEqual(result.status, 0);
	assert.equal(existsSync(join(root, "target/shareable")), false);
});
test("validated failure evidence is exported with integrity hashes", (t) => {
	const root = mkdtempSync(join(tmpdir(), "simple-blog-evidence-"));
	t.after(() => rmSync(root, { recursive: true, force: true }));
	mkdirSync(join(root, "target/verification"), { recursive: true });
	writeFileSync(
		join(root, "target/verification/recovery.json"),
		'{"status":"timeout","cookie":"private"}',
	);
	const result = spawnSync(process.execPath, [collector], {
		cwd: root,
		windowsHide: true,
	});
	assert.equal(result.status, 0, result.stderr.toString());
	const index = readObject(join(root, "target/shareable/evidence.json"));
	assert.equal(evidenceArray(index.files).length, 1);
	const [first] = evidenceArray(index.files).map(evidenceObject);
	assert.ok(first);
	assert.equal(evidenceText(first.sha256)?.length, 64);
	assert.equal(
		readObject(join(root, "target/shareable/verification/recovery.json"))
			.cookie,
		"[redacted]",
	);
});

test("private source snapshots and unclassified files beside symbols are never exported", (t) => {
	const root = mkdtempSync(join(tmpdir(), "simple-blog-evidence-"));
	t.after(() => rmSync(root, { recursive: true, force: true }));
	const snapshots = join(
		root,
		"target/verification/run/_source/browser-report",
	);
	mkdirSync(snapshots, { recursive: true });
	writeFileSync(join(snapshots, "results.json"), "PRIVATE_FIXTURE_CANARY");
	const symbols = join(root, "target/verification/symbols-release");
	mkdirSync(symbols, { recursive: true });
	const files = ["simple-blog", "simple-blog.debug"].map((name) => {
		const bytes = Buffer.from(name);
		writeFileSync(join(symbols, name), bytes);
		return {
			name,
			bytes: bytes.length,
			sha256: createHash("sha256").update(bytes).digest("hex"),
		};
	});
	writeFileSync(
		join(symbols, "symbols.json"),
		JSON.stringify({ schema: 1, files }),
	);
	writeFileSync(join(symbols, "private.txt"), "PRIVATE_FIXTURE_CANARY");
	const result = spawnSync(process.execPath, [collector], {
		cwd: root,
		windowsHide: true,
	});
	assert.equal(result.status, 0, result.stderr.toString());
	const index = readObject(join(root, "target/shareable/evidence.json"));
	assert.equal(evidenceArray(index.files).length, 3);
	assert.equal(
		evidenceArray(index.files)
			.map(evidenceObject)
			.some((file) => {
				assert.ok(typeof file.path === "string");
				return /private|_source/.test(file.path);
			}),
		false,
	);
});

for (const replaced of ["simple-blog", "simple-blog.debug", "symbols.json"]) {
	test(`validated bytes survive replacement of ${replaced}`, (t) => {
		const root = mkdtempSync(join(tmpdir(), "simple-blog-symbol-race-"));
		t.after(() => rmSync(root, { recursive: true, force: true }));
		const symbols = join(root, "target/verification/symbols-release");
		mkdirSync(symbols, { recursive: true });
		const files = ["simple-blog", "simple-blog.debug"].map((name) => {
			const bytes = Buffer.from(`validated ${name}`);
			writeFileSync(join(symbols, name), bytes);
			return {
				name,
				bytes: bytes.length,
				sha256: createHash("sha256").update(bytes).digest("hex"),
			};
		});
		writeFileSync(
			join(symbols, "symbols.json"),
			JSON.stringify({ schema: 1, files }),
		);
		const hook = join(root, "replace.cjs");
		writeFileSync(
			hook,
			`
    const fs = require('node:fs');
    const read = fs.readFileSync, open = fs.openSync, close = fs.closeSync;
    const descriptors = new Map(); let fired = false;
    function replace(path) {
      if (!fired && typeof path === 'string' && path.replaceAll('\\\\', '/').endsWith('/' + ${JSON.stringify(replaced)})) {
        fired = true; fs.writeFileSync(path, 'changed after validation'); fs.writeFileSync('fired', 'yes');
      }
    }
    fs.openSync = function(path, ...args) { const fd = open.call(this, path, ...args); descriptors.set(fd, path); return fd; };
    fs.closeSync = function(fd) { const path = descriptors.get(fd); descriptors.delete(fd); const result = close.call(this, fd); replace(path); return result; };
    fs.readFileSync = function(path, ...args) { const bytes = read.call(this, path, ...args); replace(path); return bytes; };
    require('node:module').syncBuiltinESMExports();
  `,
		);
		const result = spawnSync(process.execPath, ["--require", hook, collector], {
			cwd: root,
			windowsHide: true,
		});
		assert.equal(result.status, 0, result.stderr.toString());
		assert.equal(readFileSync(join(root, "fired"), "utf8"), "yes");
		const exported = join(root, "target/shareable/symbols/symbols-release");
		const manifest = readObject(join(exported, "symbols.json"));
		const entries = evidenceArray(manifest.files).map(evidenceObject);
		assert.equal(entries.length, 2);
		assert.deepEqual(entries.map((entry) => entry.name).sort(), [
			"simple-blog",
			"simple-blog.debug",
		]);
		for (const entry of entries) {
			assert.ok(typeof entry.name === "string");
			assert.ok(typeof entry.sha256 === "string");
			assert.equal(
				createHash("sha256")
					.update(readFileSync(join(exported, entry.name)))
					.digest("hex"),
				entry.sha256,
			);
		}
	});
}

test("malformed symbol identities cannot replace an existing validated export", (t) => {
	for (const manifest of [
		null,
		[],
		{ schema: 2, files: [] },
		{ schema: 1, files: [null, null] },
		{
			schema: 1,
			files: [
				{ name: "simple-blog", bytes: -1, sha256: "bad" },
				{ name: "simple-blog.debug", bytes: 0, sha256: "0".repeat(64) },
			],
		},
	]) {
		const root = mkdtempSync(join(tmpdir(), "simple-blog-invalid-symbols-"));
		t.after(() => rmSync(root, { recursive: true }));
		const symbols = join(root, "target/verification/symbols-release");
		const previous = join(root, "target/shareable");
		mkdirSync(symbols, { recursive: true });
		mkdirSync(previous, { recursive: true });
		writeFileSync(join(previous, "kept.log"), "Validated earlier export.");
		writeFileSync(join(symbols, "symbols.json"), JSON.stringify(manifest));
		const result = spawnSync(process.execPath, [collector], {
			cwd: root,
			windowsHide: true,
		});
		assert.notEqual(result.status, 0);
		assert.match(
			result.stderr.toString(),
			/Invalid evidence|Incomplete symbol|Invalid symbol/,
		);
		assert.equal(
			readFileSync(join(previous, "kept.log"), "utf8"),
			"Validated earlier export.",
		);
	}
});
