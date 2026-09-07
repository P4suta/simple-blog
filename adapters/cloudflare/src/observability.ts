/** A bounded classification: arbitrary paths, slugs and capabilities never enter logs. */
export function diagnosticPath(path: string): string {
  if (path === "/" || path === "/healthz" || path === "/internal/doctor") return path;
  if (/^\/admin\/share\/[^/]+\/$/.test(path)) return "/admin/share/{token}/";
  if (path === "/admin" || path.startsWith("/admin/")) return "/admin/{route}";
  if (path.startsWith("/internal/sites/")) return "/internal/sites/{site}/{route}";
  if (path.startsWith("/internal/registrations/")) return "/internal/registrations/{id}/{route}";
  if (path.startsWith("/v1/registrations")) return "/v1/registrations/{route}";
  if (path.startsWith("/media/")) return "/media/{filename}";
  if (/^\/likes\/\d+$/.test(path)) return "/likes/{id}";
  return "<unmatched>";
}

export function validCorrelationId(value: string | null): string | null {
  return value !== null && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value) ? value : null;
}

export function diagnosticMethod(method: string): string {
  return ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "CONNECT", "TRACE"].includes(method) ? method : "<other>";
}
