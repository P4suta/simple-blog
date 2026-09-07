const MINUTE = 60_000;
export const EVIDENCE_RESERVE_MS = 5 * MINUTE;
const TERMINATION_GRACE_MS = 5_000;

export function verificationBudget(env, now = Date.now()) {
  const start = env.VERIFY_JOB_STARTED_AT, limit = env.VERIFY_JOB_TIMEOUT_MINUTES;
  if (start === undefined && limit === undefined) return null;
  if (!/^\d+$/.test(start ?? '') || !/^\d+$/.test(limit ?? '') || Number(limit) <= 5 ||
      !Number.isSafeInteger(Number(start) * 1000) || !Number.isSafeInteger(Number(limit) * MINUTE) ||
      !Number.isSafeInteger(Number(start) * 1000 + Number(limit) * MINUTE) || Number(start) * 1000 > now + MINUTE) {
    throw new Error('Invalid CI verification deadline');
  }
  return { jobStartedAt: Number(start) * 1000, jobTimeoutMs: Number(limit) * MINUTE,
    evidenceReserveMs: EVIDENCE_RESERVE_MS,
    stopAt: Number(start) * 1000 + Number(limit) * MINUTE - EVIDENCE_RESERVE_MS };
}

export function stepTimeout(budget, requested = 30 * MINUTE, now = Date.now()) {
  return budget ? Math.max(0, Math.min(requested, budget.stopAt - now - TERMINATION_GRACE_MS)) : requested;
}
