export interface KvNamespace {
  get(key: string, type: "json"): Promise<unknown | null>;
  put(key: string, value: string): Promise<void>;
  delete(key: string): Promise<void>;
}

export interface R2HttpMetadata {
  contentType?: string;
}

export interface R2ObjectBody {
  customMetadata?: Record<string, string>;
  httpMetadata?: R2HttpMetadata;
  arrayBuffer(): Promise<ArrayBuffer>;
}

export interface R2ObjectMetadata {
  customMetadata?: Record<string, string>;
  httpMetadata?: R2HttpMetadata;
}

export interface R2PutOptions {
  customMetadata?: Record<string, string>;
  httpMetadata?: R2HttpMetadata;
  onlyIf?: { etagDoesNotMatch?: string; etagMatches?: string };
  sha256?: string | ArrayBuffer;
}

export interface R2Bucket {
  get(key: string): Promise<R2ObjectBody | null>;
  head?(key: string): Promise<R2ObjectMetadata | null>;
  put(
    key: string,
    value: ArrayBuffer | Uint8Array | string,
    options?: R2PutOptions,
  ): Promise<unknown | null>;
}

export interface DurableObjectId {}

export interface DurableObjectStub {
  fetch(request: Request): Promise<Response>;
}

export interface DurableObjectNamespace {
  idFromName(name: string): DurableObjectId;
  get(id: DurableObjectId): DurableObjectStub;
}

export interface DurableObjectStorage {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<boolean>;
  getAlarm(): Promise<number | null>;
  setAlarm(scheduledTime: number): Promise<void>;
  deleteAlarm(): Promise<void>;
}

export interface DurableObjectState {
  storage: DurableObjectStorage;
}

export interface Fetcher {
  fetch(request: Request): Promise<Response>;
}

export interface RateLimit {
  limit(options: { key: string }): Promise<{ success: boolean }>;
}

export interface SiteCoordinatorEnv {
  INTERNAL_DO_TOKEN: string;
  CORE: Fetcher;
  HOSTS?: KvNamespace;
  RELEASES?: R2Bucket;
}

export interface D1Result {
  success: boolean;
  meta?: Record<string, unknown>;
}

export interface D1PreparedStatement {
  bind(...values: unknown[]): D1PreparedStatement;
  first<T = Record<string, unknown>>(): Promise<T | null>;
  run(): Promise<D1Result>;
  all<T = Record<string, unknown>>(): Promise<{ results: T[]; success: boolean }>;
}

export interface D1Database {
  prepare(query: string): D1PreparedStatement;
  batch(statements: D1PreparedStatement[]): Promise<D1Result[]>;
}

export interface WorkerEnv {
  HOSTS: KvNamespace;
  RELEASES: R2Bucket;
  REGISTRY: D1Database;
  SITES: DurableObjectNamespace;
  REGISTRATION_RATE_LIMITER: RateLimit;
  CONTROL_HOSTNAME: string;
  ANONYMOUS_DEMO_HOSTNAME: string;
  SAAS_CNAME_TARGET: string;
  CF_ZONE_ID: string;
  CF_API_TOKEN: string;
  INTERNAL_DO_TOKEN: string;
  DIAGNOSTIC_TOKEN: string;
  CORE: Fetcher;
}

export interface WorkerExecutionContext {
  waitUntil(promise: Promise<unknown>): void;
}
