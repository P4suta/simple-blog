const sensitive =
	/^(?:cookie|set-cookie|authorization|csrf|csrf_token|token|setup_token|recovery_codes?|password|secret|body_markdown|body_html)$/i;

/**
 * @param {string} text
 * @returns {string}
 */
function redactText(text) {
	return text
		.replace(/(\/admin\/share\/)[^/\s?"<>]+/g, "$1[redacted]")
		.replace(
			/([?&](?:token|csrf|code|claim|secret|key)=)[^\s&"<>]+/gi,
			"$1[redacted]",
		)
		.replace(/\bBearer\s+[A-Za-z0-9._~+/-]+/gi, "Bearer [redacted]");
}

/**
 * @param {unknown} value
 * @returns {unknown}
 */
function clean(value) {
	if (Array.isArray(value)) return value.map(clean);
	if (value && typeof value === "object")
		return Object.fromEntries(
			Object.entries(value).map(([key, item]) => [
				key,
				sensitive.test(key) ? "[redacted]" : clean(item),
			]),
		);
	return typeof value === "string" ? redactText(value) : value;
}

/**
 * @param {string} text
 * @returns {string}
 */
export function sanitizeText(text) {
	try {
		const output = JSON.stringify(clean(JSON.parse(text)));
		if (output === undefined) throw new TypeError("Invalid evidence JSON");
		return output;
	} catch {
		return redactText(text);
	}
}

/**
 * @param {string} text
 * @returns {string}
 */
export function safeEvidenceText(text) {
	let sanitized;
	try {
		JSON.parse(text);
		sanitized = sanitizeText(text);
	} catch {
		sanitized = text.split("\n").map(sanitizeText).join("\n");
	}
	assertSafeEvidence(sanitized);
	return sanitized;
}

/**
 * @param {string} text
 * @returns {void}
 */
export function assertSafeEvidence(text) {
	for (const pattern of [
		/PRIVATE_FIXTURE_CANARY/,
		/\bsb_(?:session|csrf)=(?!\[redacted\])[^;\s"<>]+/i,
		/\bBearer\s+(?!\[redacted\])[A-Za-z0-9._~+/-]+/i,
		/\/admin\/share\/(?!\[redacted\]|\{token\})[^/\s?"<>]+/,
		/[?&](?:token|csrf|code|claim|secret|key)=(?!\[redacted\])[^&\s"<>]+/i,
	])
		if (pattern.test(text))
			throw new Error("Evidence contains a private value; export refused");
}
