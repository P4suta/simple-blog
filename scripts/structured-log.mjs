// Tool stdout can mix ordinary lines, JSONL and formatted JSON. Hold a structured
// document until it is complete, so its sensitive fields never reach disk first.
export function structuredLog(sanitize, write) {
  let document = '', depth = 0, quoted = false, escaped = false;
  return {
    line(line) {
      if (!document && !/^\s*[{[]/.test(line)) { write(sanitize(line) + '\n'); return; }
      // Bracketed ordinary diagnostics such as [INFO] are not JSON documents.
      if (!document && /^\s*\[[A-Za-z]/.test(line) && !/^\s*\[(?:true|false|null)\b/.test(line)) {
        write(sanitize(line) + '\n'); return;
      }
      document += line + '\n';
      if (document.length > 8 * 1024 * 1024) throw new Error('Structured evidence exceeds 8 MiB');
      for (const char of line) {
        if (quoted) {
          if (escaped) escaped = false;
          else if (char === '\\') escaped = true;
          else if (char === '"') quoted = false;
        } else if (char === '"') quoted = true;
        else if (char === '{' || char === '[') depth++;
        else if (char === '}' || char === ']') depth--;
      }
      if (depth === 0 && !quoted) {
        JSON.parse(document); // Malformed structured output is evidence failure.
        write(sanitize(document) + '\n');
        document = '';
      }
    },
    finish() {
      if (document) throw new Error('Incomplete structured evidence was withheld');
    },
  };
}
