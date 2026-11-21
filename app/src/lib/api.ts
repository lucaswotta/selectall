import { invoke } from "@tauri-apps/api/core";

export type Chain = { id: number; name: string };
export type AppInfo = { version: string; dataPath: string; schemaVersion: number };
export type Theme = "system" | "light" | "dark";

export const api = {
  info: () => invoke<AppInfo>("app_info"),
  chains: () => invoke<Chain[]>("list_chains"),
  addChain: (name: string) => invoke<Chain>("add_chain", { name }),
  theme: () => invoke<Theme>("get_theme"),
  setTheme: (theme: Theme) => invoke<void>("set_theme", { theme }),
};

/** Comandos rejeitam com a mensagem em português vinda do núcleo. */
export function message(error: unknown): string {
  return typeof error === "string" ? error : String(error);
}
