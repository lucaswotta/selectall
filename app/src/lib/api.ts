import { invoke } from "@tauri-apps/api/core";

export type Chain = { id: number; name: string };
export type AppInfo = { version: string; dataPath: string; schemaVersion: number };
export type Theme = "system" | "light" | "dark";

export type Engine = "mysql" | "postgres" | "sqlserver";
export type Profile = {
  id: number;
  chain_id: number;
  name: string;
  engine: Engine;
  port: number;
  database: string;
  username: string;
  tls: boolean;
};
export type ProfileView = Profile & { has_password: boolean };
export type Store = { id: number; chain_id: number; code: string; name: string; subnet: string };
export type Device = { id: number; store_id: number; name: string; kind: string; host: string; profile_ids: number[] };
export type AddressRule = { kind: string; template: string; profile_id: number | null };
export type Fleet = { stores: Store[]; devices: Device[]; profiles: ProfileView[]; rules: AddressRule[] };

export type LineError = { line: number; message: string };
export type StoreSpec = { line: number; code: string; name: string; subnet: string };
export type Entry = {
  line: number;
  store_code: string | null;
  host: string;
  kind: string | null;
  name: string | null;
  profile_id: number | null;
  profile_name: string | null;
};
export type Batch = { stores: StoreSpec[]; entries: Entry[]; errors: LineError[]; notices: string[] };
export type DeviceStatus = "new" | "exists" | "repeated";
export type PlannedDevice = {
  line: number;
  store_code: string;
  name: string;
  kind: string;
  host: string;
  profile_ids: number[];
  status: DeviceStatus;
};
export type Plan = { stores: StoreSpec[]; devices: PlannedDevice[]; errors: LineError[]; notices: string[] };
export type Applied = { stores: number; devices: number };
export type ScanHit = { host: string; open: boolean; engine: Engine | null; version: string | null };

export type RunStatus =
  | "success"
  | "partial"
  | "failed"
  | "offline"
  | "cancelled"
  | "timed_out"
  | "rehearsed"
  | "applied"
  | "unknown";
export type Failure = { message: string; detail: string | null };
export type Progress =
  | { type: "connecting"; device_id: number }
  | { type: "running"; device_id: number; profile: string }
  | {
      type: "finished";
      device_id: number;
      profile: string | null;
      status: RunStatus;
      failure: Failure | null;
      elapsed_ms: number;
      rows: number;
      sets: number;
      truncated: boolean;
      warnings: number;
      affected_rows: number;
    }
  | { type: "skipped"; device_id: number }
  | { type: "cancel_errors"; errors: string[] }
  | { type: "done" };
export type ProgressEvent = Progress & { run: number };
export type GridColumn = { name: string; numeric: boolean; fixed: boolean };
export type Grid = { columns: GridColumn[]; rows: (string | null)[][]; sets: number };
export type ExportFormat = "csv" | "xlsx";

export type Step = "rehearsal" | "pilot" | "rollout";
export type CorrectionSummary = {
  sql: string;
  values: Record<string, string>;
  /** Pares [equipamento, linhas que o ensaio alteraria]. */
  rehearsed: [number, number][];
  /** Quem pode receber a correção: os ensaiados, ou os escolhidos sem ensaio. */
  candidates: number[];
  rehearsal_skipped: boolean;
  median: number;
  outliers: number[];
  applied: number[];
  unknown: number[];
  pilot_applied: boolean;
};
export type AuditEntry = {
  id: number;
  /** UTC, `AAAA-MM-DDTHH:MM:SSZ`. */
  at: string;
  user: string;
  chain: string;
  step: Step;
  sql: string;
  planned: number;
  rehearsal_skipped: boolean;
  finished: number;
  affected: number;
  statuses: [RunStatus, number][];
};
export type AuditDevice = {
  store: string;
  device: string;
  host: string;
  profile: string | null;
  status: RunStatus;
  affected: number;
  message: string | null;
};

export type Values = Record<string, string>;
export type SavedQuery = {
  id: number;
  chain_id: number;
  name: string;
  description: string;
  profile_id: number | null;
  sql: string;
};
export type SavedView = SavedQuery & { write: boolean; params: string[] };
export type Imported = { added: number; replaced: number };

export type HistoryLimits = { days: number; megabytes: number };
export type HistoryUsage = { limits: HistoryLimits; bytes: number; runs: number };
export type HistoryEntry = {
  id: number;
  /** UTC, `AAAA-MM-DDTHH:MM:SSZ`. */
  at: string;
  label: string;
  /** Nulo quando o SQL citava senha, token ou segredo. */
  sql: string | null;
  values: Values;
  targets: number;
  responded: number;
  offline: number;
  failed: number;
  rows: number;
  elapsed_ms: number;
  bytes: number;
};
export type FinishedProgress = Extract<Progress, { type: "finished" }>;
export type ReopenedDevice = FinishedProgress & { store: string; device: string; host: string };
export type Reopened = { entry: HistoryEntry; devices: ReopenedDevice[] };

export type CompareColumn = { name: string; chosen: boolean; temporal: boolean };
export type CompareGroup = {
  rows: (string | null)[][];
  row_count: number;
  members: { store: string; device: string; host: string }[];
  stores: { store: string; count: number; total: number }[];
};
export type Comparison = { columns: CompareColumn[]; groups: CompareGroup[]; majority: boolean; compared: number };

/** Nomes de `{{nome}}` na ordem em que aparecem, como o núcleo lê. */
export function paramNames(sql: string): string[] {
  return [...new Set([...sql.matchAll(/\{\{([A-Za-z_][A-Za-z0-9_]*)\}\}/g)].map((match) => match[1]))];
}

/** Carimbo local para nomes de arquivo: `20261121-094102`. */
export function fileStamp(now = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
}

export function when(at: string): string {
  return new Date(at).toLocaleString("pt-BR", { day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

export const stepLabel: Record<Step, string> = { rehearsal: "Ensaio", pilot: "Piloto", rollout: "Lote" };

export type Source =
  | { kind: "rule"; rules: AddressRule[]; stores: string; scan: boolean }
  | { kind: "paste"; text: string; profile_id: number | null }
  | { kind: "file"; name: string; bytes: number[]; profile_id: number | null }
  | { kind: "batch"; batch: Batch };

export const engineLabel: Record<Engine, string> = {
  mysql: "MySQL/MariaDB",
  postgres: "PostgreSQL",
  sqlserver: "SQL Server",
};

export const defaultPort: Record<Engine, number> = { mysql: 3306, postgres: 5432, sqlserver: 1433 };

export const api = {
  info: () => invoke<AppInfo>("app_info"),
  chains: () => invoke<Chain[]>("list_chains"),
  addChain: (name: string) => invoke<Chain>("add_chain", { name }),
  renameChain: (id: number, name: string) => invoke<Chain>("rename_chain", { id, name }),
  deleteChain: (id: number) => invoke<void>("delete_chain", { id }),
  theme: () => invoke<Theme>("get_theme"),
  setTheme: (theme: Theme) => invoke<void>("set_theme", { theme }),

  fleet: (chainId: number) => invoke<Fleet>("load_fleet", { chainId }),
  /** `password` nulo mantém a senha do cofre; texto (mesmo vazio) substitui. */
  saveProfile: (profile: Profile, password: string | null) => invoke<Profile>("save_profile", { profile, password }),
  deleteProfile: (id: number) => invoke<void>("delete_profile", { id }),
  saveStore: (store: Store) => invoke<Store>("save_store", { store }),
  deleteStore: (id: number) => invoke<void>("delete_store", { id }),
  saveDevice: (device: Device) => invoke<Device>("save_device", { device }),
  deleteDevice: (id: number) => invoke<void>("delete_device", { id }),
  saveRules: (chainId: number, rules: AddressRule[]) => invoke<void>("save_rules", { chainId, rules }),
  preview: (chainId: number, source: Source) => invoke<Plan>("preview_batch", { chainId, source }),
  apply: (chainId: number, plan: Plan) => invoke<Applied>("apply_batch", { chainId, plan }),
  scanRange: (range: string, port: number) => invoke<ScanHit[]>("scan_range", { range, port }),
  scanStore: (chainId: number, storeId: number) => invoke<ScanHit[]>("scan_store", { chainId, storeId }),

  /** Devolve o número da execução; o progresso chega pelo evento `query-progress`. */
  /** `label` é o nome da consulta salva de onde o SQL veio, para o histórico. */
  runQuery: (chainId: number, deviceIds: number[], sql: string, values: Values, label: string | null) =>
    invoke<number>("run_query", { chainId, deviceIds, sql, values, label }),
  retryQuery: (chainId: number, deviceIds: number[]) => invoke<number>("retry_query", { chainId, deviceIds }),
  cancelQuery: () => invoke<void>("cancel_query"),
  grid: (set: number) => invoke<Grid>("query_grid", { set }),
  exportGrid: (set: number, format: ExportFormat, stem: string) => invoke<string>("export_grid", { set, format, stem }),
  /** `columns` nulo usa o padrão: todas menos as de data e hora. */
  compare: (set: number, columns: string[] | null) => invoke<Comparison>("query_compare", { set, columns }),
  reopenHistory: (id: number) => invoke<Reopened>("reopen_history", { id }),

  rehearse: (chainId: number, deviceIds: number[], sql: string, values: Values) =>
    invoke<number>("rehearse_correction", { chainId, deviceIds, sql, values }),
  skipRehearsal: (chainId: number, deviceIds: number[], sql: string, values: Values) =>
    invoke<void>("skip_rehearsal", { chainId, deviceIds, sql, values }),
  applyPilot: (chainId: number, deviceIds: number[]) => invoke<number>("apply_pilot", { chainId, deviceIds }),
  /** `confirmation` é o número de equipamentos digitado na janela de confirmação. */
  applyRollout: (chainId: number, excluded: number[], confirmation: number) =>
    invoke<number>("apply_rollout", { chainId, excluded, confirmation }),
  correction: () => invoke<CorrectionSummary | null>("correction_summary"),
  abandonCorrection: () => invoke<void>("abandon_correction"),
  auditLog: () => invoke<AuditEntry[]>("audit_log"),
  auditDevices: (auditId: number) => invoke<AuditDevice[]>("audit_devices", { auditId }),
  exportAudit: (format: ExportFormat, stem: string) => invoke<string>("export_audit", { format, stem }),

  savedQueries: (chainId: number) => invoke<SavedView[]>("list_saved", { chainId }),
  /** `id` 0 grava uma consulta nova. */
  saveQuery: (query: SavedQuery) => invoke<SavedView>("save_saved", { query }),
  deleteQuery: (id: number) => invoke<void>("delete_saved", { id }),
  exportLibrary: (chainId: number, stem: string) => invoke<string>("export_library", { chainId, stem }),
  importLibrary: (chainId: number, text: string) => invoke<Imported>("import_library", { chainId, text }),
  history: (chainId: number) => invoke<HistoryEntry[]>("list_history", { chainId }),
  historyUsage: () => invoke<HistoryUsage>("history_usage"),
  setHistoryLimits: (limits: HistoryLimits) => invoke<HistoryUsage>("set_history_limits", { limits }),
  perStoreLimit: () => invoke<number>("per_store_limit"),
  setPerStoreLimit: (limit: number) => invoke<number>("set_per_store_limit", { limit }),
  deleteHistory: (id: number) => invoke<void>("delete_history", { id }),
};

/** Comandos rejeitam com a mensagem em português vinda do núcleo. */
export function message(error: unknown): string {
  return typeof error === "string" ? error : String(error);
}

export function plural(count: number, one: string, many: string): string {
  return `${count.toLocaleString("pt-BR")} ${count === 1 ? one : many}`;
}
