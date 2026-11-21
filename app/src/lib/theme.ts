import type { Theme } from "./api";

export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  if (theme === "system") delete root.dataset.theme;
  else root.dataset.theme = theme;
}

export const nextTheme: Record<Theme, Theme> = { system: "light", light: "dark", dark: "system" };
export const themeLabel: Record<Theme, string> = { system: "Sistema", light: "Claro", dark: "Escuro" };
