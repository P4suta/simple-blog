import { nextDomainState, normalizeDomain, type DomainRegistrationState, type ProvisioningStatus } from "./domain.ts";

export interface DnsInstruction {
  type: "txt";
  name: string;
  value: string;
}

export interface CustomHostname {
  id: string;
  hostnameStatus: ProvisioningStatus;
  certificateStatus: ProvisioningStatus;
  providerErrorCode: string | null;
  ownershipVerification: DnsInstruction | null;
  certificateValidation: DnsInstruction[];
}

export interface CustomHostnameProvider {
  create(domain: string): Promise<CustomHostname>;
  inspect(providerHostnameId: string): Promise<CustomHostname>;
}

export interface DomainRouteVerifier {
  routesToService(domain: string, cnameTarget: string): Promise<boolean>;
}

export interface ClaimSecret {
  token: string;
  hash: string;
}

export interface ClaimSecrets {
  issue(): Promise<ClaimSecret>;
  hash(token: string): Promise<string>;
}

export interface RegistrationRecord {
  id: string;
  domain: string;
  claimHash: string;
  claimExpiresAt: string;
  providerHostnameId: string | null;
  state: DomainRegistrationState;
  ownershipVerification: DnsInstruction | null;
  certificateValidation: DnsInstruction[];
  providerErrorCode: string | null;
  lastObservedAt: string | null;
  ownerRegisteredAt: string | null;
  createdAt: string;
  updatedAt: string;
  /** Adapter-local optimistic concurrency token; never exposed to callers. */
  storageVersion: number;
}

export interface RegistrationRepository {
  reserve(record: RegistrationRecord): Promise<boolean>;
  authorized(id: string, claimHash: string, now: string): Promise<RegistrationRecord | null>;
  save(record: RegistrationRecord): Promise<void>;
}

export interface RegistrationView {
  id: string;
  domain: string;
  state: DomainRegistrationState;
  cnameTarget: string;
  ownershipVerification: DnsInstruction | null;
  certificateValidation: DnsInstruction[];
  providerErrorCode: string | null;
  ownerSetupUrl: string | null;
}

export interface StartedRegistration extends RegistrationView {
  claimToken: string;
}

export class RegistrationService {
  private readonly repository: RegistrationRepository;
  private readonly provider: CustomHostnameProvider;
  private readonly dns: DomainRouteVerifier;
  private readonly claims: ClaimSecrets;
  private readonly now: () => string;
  private readonly newId: () => string;
  private readonly cnameTarget: string;
  private readonly providerZone: string;

  constructor(
    repository: RegistrationRepository,
    provider: CustomHostnameProvider,
    dns: DomainRouteVerifier,
    claims: ClaimSecrets,
    now: () => string,
    newId: () => string,
    cnameTarget: string,
  ) {
    this.repository = repository;
    this.provider = provider;
    this.dns = dns;
    this.claims = claims;
    this.now = now;
    this.newId = newId;
    this.cnameTarget = normalizeDomain(cnameTarget);
    const labels = this.cnameTarget.split(".");
    this.providerZone = labels.length > 2 ? labels.slice(1).join(".") : this.cnameTarget;
  }

  async start(input: string): Promise<StartedRegistration> {
    const domain = normalizeDomain(input);
    if (domain === this.providerZone || domain.endsWith(`.${this.providerZone}`)) {
      throw new Error("domain_unavailable");
    }
    const now = this.now();
    const claim = await this.claims.issue();
    const record: RegistrationRecord = {
      id: this.newId(),
      domain,
      claimHash: claim.hash,
      claimExpiresAt: new Date(Date.parse(now) + 24 * 60 * 60 * 1000).toISOString(),
      providerHostnameId: null,
      state: "pending_ownership",
      ownershipVerification: null,
      certificateValidation: [],
      providerErrorCode: null,
      lastObservedAt: null,
      ownerRegisteredAt: null,
      createdAt: now,
      updatedAt: now,
      storageVersion: 0,
    };
    if (!(await this.repository.reserve(record))) throw new Error("domain_unavailable");
    await this.provision(record);
    return { ...this.view(record, claim.token), claimToken: claim.token };
  }

  async refresh(id: string, claimToken: string): Promise<RegistrationView> {
    const now = this.now();
    const claimHash = await this.claims.hash(claimToken);
    const record = await this.repository.authorized(id, claimHash, now);
    if (record === null) throw new Error("registration_not_found");
    const hostname = record.providerHostnameId === null
      ? await this.createSafely(record)
      : await this.provider.inspect(record.providerHostnameId);
    if (hostname === null) return this.view(record, claimToken);
    const dnsRouted = hostname.hostnameStatus === "active" &&
      hostname.certificateStatus === "active" &&
      await this.dns.routesToService(record.domain, this.cnameTarget);
    this.applyProvider(record, hostname, dnsRouted, now);
    await this.repository.save(record);
    return this.view(record, claimToken);
  }

  private async provision(record: RegistrationRecord): Promise<void> {
    const hostname = await this.createSafely(record);
    if (hostname === null) return;
    this.applyProvider(record, hostname, false, this.now());
    await this.repository.save(record);
  }

  private async createSafely(record: RegistrationRecord): Promise<CustomHostname | null> {
    try {
      return await this.provider.create(record.domain);
    } catch {
      record.state = "action_required";
      record.providerErrorCode = "provider_create_failed";
      record.updatedAt = this.now();
      await this.repository.save(record);
      return null;
    }
  }

  private applyProvider(
    record: RegistrationRecord,
    hostname: CustomHostname,
    dnsRouted: boolean,
    checkedAt: string,
  ): void {
    record.providerHostnameId = hostname.id;
    record.state = nextDomainState(record.ownerRegisteredAt !== null, {
      hostname: hostname.hostnameStatus,
      certificate: hostname.certificateStatus,
      dns_routed: dnsRouted,
    });
    record.ownershipVerification = hostname.ownershipVerification;
    record.certificateValidation = hostname.certificateValidation;
    record.providerErrorCode = hostname.providerErrorCode;
    record.lastObservedAt = checkedAt;
    record.updatedAt = checkedAt;
  }

  private view(record: RegistrationRecord, claimToken: string): RegistrationView {
    return {
      id: record.id,
      domain: record.domain,
      state: record.state,
      cnameTarget: this.cnameTarget,
      ownershipVerification: record.ownershipVerification,
      certificateValidation: record.certificateValidation,
      providerErrorCode: record.providerErrorCode,
      ownerSetupUrl: record.state === "ready_for_owner"
        ? `https://${record.domain}/admin/setup/#claim=${encodeURIComponent(claimToken)}`
        : null,
    };
  }
}
