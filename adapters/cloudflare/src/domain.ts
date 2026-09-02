export type ProvisioningStatus = "pending" | "active" | "failed";

export type DomainRegistrationState =
  | "pending_ownership"
  | "pending_certificate"
  | "pending_dns"
  | "ready_for_owner"
  | "active"
  | "action_required";

export interface DomainObservation {
  checked_at: string;
  hostname: ProvisioningStatus;
  certificate: ProvisioningStatus;
  dns_routed: boolean;
  provider_error_code: string | null;
}

const RESERVED_SUFFIXES = new Set(["example", "invalid", "localhost", "test"]);

export function normalizeDomain(input: string): string {
  if (
    input.length === 0 ||
    input.length > 253 ||
    input.trim() !== input ||
    !/^[\x00-\x7f]+$/.test(input) ||
    input.endsWith(".") ||
    /^\d{1,3}(?:\.\d{1,3}){3}$/.test(input)
  ) {
    throw new Error("invalid_domain");
  }
  const domain = input.toLowerCase();
  const labels = domain.split(".");
  const validLabel = (label: string): boolean =>
    label.length > 0 &&
    label.length <= 63 &&
    /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label);
  const suffix = labels.at(-1) ?? "";
  if (
    labels.length < 2 ||
    !labels.every(validLabel) ||
    RESERVED_SUFFIXES.has(suffix) ||
    /^\d+$/.test(suffix)
  ) {
    throw new Error("invalid_domain");
  }
  return domain;
}

export function nextDomainState(
  ownerRegistered: boolean,
  observation: Pick<
    DomainObservation,
    "hostname" | "certificate" | "dns_routed"
  >,
): DomainRegistrationState {
  const ready =
    observation.hostname === "active" &&
    observation.certificate === "active" &&
    observation.dns_routed;
  const failed =
    observation.hostname === "failed" || observation.certificate === "failed";
  if (failed || (ownerRegistered && !ready)) return "action_required";
  if (observation.hostname !== "active") return "pending_ownership";
  if (observation.certificate !== "active") return "pending_certificate";
  if (!observation.dns_routed) return "pending_dns";
  return ownerRegistered ? "active" : "ready_for_owner";
}
