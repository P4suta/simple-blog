export function inquiryId(value: string | null | undefined): string | null {
  return value && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value) ? value : null;
}

export class RequestFailure extends Error {
  readonly status: number;
  readonly detail: string;
  readonly requestId: string | null;
  constructor(status: number, detail: string, requestId?: string | null) {
    super(detail || `HTTP ${status}`);
    this.status = status;
    this.detail = detail;
    this.requestId = inquiryId(requestId);
  }
}

export function withInquiry(message: string, reason: unknown, label = 'Inquiry ID'): string {
  return reason instanceof RequestFailure && reason.requestId
    ? `${message} (${label}: ${reason.requestId})` : message;
}
