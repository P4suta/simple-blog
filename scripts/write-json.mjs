import { renameSync, writeFileSync } from "node:fs";

/**
 * @param {string} path
 * @param {unknown} value
 * @returns {void}
 */
export function writeJson(path, value) {
	writeFileSync(`${path}.tmp`, `${JSON.stringify(value, null, 2)}\n`, {
		mode: 0o600,
	});
	renameSync(`${path}.tmp`, path);
}
