import {
	evidenceArray,
	evidenceNumber,
	evidenceObject,
	evidenceText,
} from "./evidence-shape.mjs";

const identifiers =
	/^(?:request_id|operation_id|parent_operation_id|build_id|release_id)$/;
const labels = /^(?:name|event|operation|error_code|phase|disposition|method)$/;
const measurements =
	/^(?:status|elapsed_ms|public_revision|route_count|staged_object_count|content_count|redirect_count|media_count|bytes|line_number)$/;

/**
 * @param {unknown} value
 * @returns {Record<string, unknown>}
 */
function allowedFields(value) {
	if (!value || typeof value !== "object" || Array.isArray(value)) return {};
	return Object.fromEntries(
		Object.entries(value).filter(
			([key, field]) =>
				(identifiers.test(key) &&
					typeof field === "string" &&
					/^(?:[0-9a-f]{64}|[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})$/i.test(
						field,
					)) ||
				(labels.test(key) &&
					typeof field === "string" &&
					/^[a-z0-9_.<>]{1,120}$/i.test(field)) ||
				(measurements.test(key) &&
					typeof field === "number" &&
					Number.isFinite(field) &&
					field >= 0),
		),
	);
}

/**
 * @param {string} text
 * @returns {string}
 */
export function sanitizeServerEvents(text) {
	return `${text
		.split("\n")
		.filter((line) => line.trim())
		.map((line, index) => {
			let source;
			try {
				source = evidenceObject(JSON.parse(line));
			} catch {
				return JSON.stringify({
					fields: {
						event: "evidence.unclassified_server_line",
						line_number: index + 1,
					},
				});
			}
			const result = allowedFields(source);
			if (
				typeof source.timestamp === "string" &&
				/^\d{4}-\d\d-\d\dT[\d:.]+Z$/.test(source.timestamp)
			)
				result.timestamp = source.timestamp;
			if (
				typeof source.level === "string" &&
				["TRACE", "DEBUG", "INFO", "WARN", "ERROR"].includes(source.level)
			)
				result.level = source.level;
			if (source.fields) result.fields = allowedFields(source.fields);
			if (source.span) result.span = allowedFields(source.span);
			if (Array.isArray(source.spans))
				result.spans = source.spans.map(allowedFields);
			return JSON.stringify(result);
		})
		.join("\n")}\n`;
}

/** @typedef {{status: string | undefined, duration: number | undefined, retry: number | undefined}} BrowserResult */
/** @typedef {{title: string | undefined, file: string | undefined, line: number | undefined, project: string | undefined, expectedStatus: string | undefined, status: string | undefined, results: BrowserResult[]}} BrowserCase */
/** @typedef {{name: string, path: string} | {name: 'server events', body: string}} BrowserAttachment */
/** @typedef {{startTime: string | undefined, duration: number | undefined, expected: number | undefined, skipped: number | undefined, unexpected: number | undefined, flaky: number | undefined}} BrowserStats */

/**
 * @param {unknown} value
 * @returns {unknown[]}
 */
function optionalItems(value) {
	return value === undefined ? [] : evidenceArray(value);
}

/**
 * @param {unknown} value
 * @returns {BrowserResult}
 */
function resultSummary(value) {
	const result = evidenceObject(value);
	return {
		status: evidenceText(result.status),
		duration: evidenceNumber(result.duration),
		retry: evidenceNumber(result.retry),
	};
}

/**
 * @param {unknown} value
 * @returns {BrowserStats}
 */
function statsSummary(value) {
	const stats = value === undefined ? {} : evidenceObject(value);
	return {
		startTime: evidenceText(stats.startTime),
		duration: evidenceNumber(stats.duration),
		expected: evidenceNumber(stats.expected),
		skipped: evidenceNumber(stats.skipped),
		unexpected: evidenceNumber(stats.unexpected),
		flaky: evidenceNumber(stats.flaky),
	};
}

/**
 * @param {unknown} report
 * @returns {{summary: {stats: BrowserStats, tests: BrowserCase[]}, attachments: BrowserAttachment[]}}
 */
export function summarizeBrowser(report) {
	const source = evidenceObject(report);
	/** @type {BrowserCase[]} */
	const tests = [];
	/** @type {BrowserAttachment[]} */
	const attachments = [];
	/**
	 * @param {unknown} value
	 * @returns {void}
	 */
	function visit(value) {
		const suite = evidenceObject(value);
		for (const rawSpec of optionalItems(suite.specs)) {
			const spec = evidenceObject(rawSpec);
			for (const rawTest of optionalItems(spec.tests)) {
				const test = evidenceObject(rawTest);
				const results = evidenceArray(test.results);
				tests.push({
					title: evidenceText(spec.title),
					file: evidenceText(spec.file),
					line: evidenceNumber(spec.line),
					project: evidenceText(test.projectName),
					expectedStatus: evidenceText(test.expectedStatus),
					status: evidenceText(test.status),
					results: results.map(resultSummary),
				});
				for (const rawResult of results) {
					const result = evidenceObject(rawResult);
					for (const rawItem of optionalItems(result.attachments)) {
						const item = evidenceObject(rawItem);
						if (
							(item.name === "sanitized trace" ||
								item.name === "server events") &&
							typeof item.path === "string"
						) {
							attachments.push({ name: item.name, path: item.path });
						} else if (
							item.name === "server events" &&
							typeof item.body === "string"
						) {
							const bytes = Buffer.from(item.body, "base64");
							if (bytes.toString("base64") !== item.body)
								throw new Error("Invalid inline log encoding");
							attachments.push({
								name: item.name,
								body: Buffer.from(
									sanitizeServerEvents(bytes.toString("utf8")),
								).toString("base64"),
							});
						}
					}
				}
			}
		}
		for (const child of optionalItems(suite.suites)) visit(child);
	}
	visit(source);
	return { summary: { stats: statsSummary(source.stats), tests }, attachments };
}
