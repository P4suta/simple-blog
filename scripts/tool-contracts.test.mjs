import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { test } from "node:test";
import project from "../tsconfig.tools.json" with { type: "json" };

/**
 * @param {string} input
 * @returns {import('node:child_process').SpawnSyncReturns<string>}
 */
function annotationCheck(input) {
	return spawnSync(
		process.execPath,
		[
			"node_modules/eslint/bin/eslint.js",
			"--config",
			"scripts/eslint.tools.mjs",
			"--max-warnings",
			"0",
			"--stdin",
			"--stdin-filename",
			"scripts/collect-evidence.mjs",
		],
		{ input, encoding: "utf8", windowsHide: true },
	);
}

test("the annotation gate admits explicit contracts and rejects missing boundaries", () => {
	const valid =
		"/** @param {string} value\n * @returns {string} */\nfunction identity(value) { return value; }";
	const accepted = annotationCheck(valid);
	assert.equal(accepted.status, 0, accepted.stdout + accepted.stderr);
	for (const source of [
		"function identity(value) { return value; }",
		"/** @param {string} value */\nfunction identity(value) { return value; }",
		"/** @param {any} value\n * @returns {any} */\nfunction identity(value) { return value; }",
		"/** @param {string} value\n * @returns {string} */\nfunction identity(value) { /* @ts-ignore */ return value; }",
	]) {
		const rejected = annotationCheck(source);
		assert.equal(rejected.status, 1, rejected.stdout + rejected.stderr);
		assert.match(rejected.stdout, /jsdoc\/|no-warning-comments/);
	}
});

test("the project type checker rejects incompatible arguments", (t) => {
	assert.equal(project.compilerOptions.strict, true);
	assert.equal(project.compilerOptions.checkJs, true);
	assert.ok(project.files.includes("scripts/collect-evidence.mjs"));
	mkdirSync("target", { recursive: true });
	const root = mkdtempSync(resolve("target/type-contract-"));
	t.after(() => rmSync(root, { recursive: true }));
	const fixture = join(root, "rejection.mjs");
	const source =
		"/** @param {string} value\n * @returns {string} */\nfunction identity(value) { return value; }\nidentity(1);";
	writeFileSync(fixture, source);
	const config = join(root, "tsconfig.json");
	writeFileSync(
		config,
		JSON.stringify({
			extends: resolve("tsconfig.tools.json"),
			files: [fixture],
		}),
	);
	const rejected = spawnSync(
		process.execPath,
		[
			"node_modules/typescript/bin/tsc",
			"--project",
			config,
			"--pretty",
			"false",
		],
		{ encoding: "utf8", windowsHide: true },
	);
	assert.equal(rejected.status, 1, rejected.stdout + rejected.stderr);
	assert.match(rejected.stdout, /TS2345.*number.*string/);
});
