/**
 * @param {unknown} value
 * @returns {Record<string, unknown>}
 */
export function evidenceObject(value) {
	if (!value || typeof value !== "object" || Array.isArray(value))
		throw new TypeError("Invalid evidence object");
	return Object.fromEntries(Object.entries(value));
}

/**
 * @param {unknown} value
 * @returns {unknown[]}
 */
export function evidenceArray(value) {
	if (!Array.isArray(value)) throw new TypeError("Invalid evidence array");
	return value;
}

/**
 * @param {unknown} value
 * @returns {string | undefined}
 */
export function evidenceText(value) {
	if (value === undefined || typeof value === "string") return value;
	throw new TypeError("Invalid evidence text");
}

/**
 * @param {unknown} value
 * @returns {number | undefined}
 */
export function evidenceNumber(value) {
	if (value === undefined) return undefined;
	if (typeof value !== "number" || !Number.isFinite(value) || value < 0)
		throw new TypeError("Invalid evidence number");
	return value;
}
