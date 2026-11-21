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
export type ScanHit = { host: string; open: boolean; version: string | null };

export type Source =
  | { kind: "rule"; rules: AddressRule[]; stores: string; scan: boolean }
  | { kind: "paste"; text: string }
  | { kind: "file"; name: string; bytes: number[] }
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
};

/** Comandos rejeitam com a mensagem em português vinda do núcleo. */
export function message(error: unknown): string {
  return typeof error === "string" ? error : String(error);
}

export function plural(count: number, one: string, many: string): string {
  return `${count.toLocaleString("pt-BR")} ${count === 1 ? one : many}`;
}
