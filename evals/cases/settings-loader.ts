import { readFile } from "node:fs/promises";

export type Settings = { theme: "light" | "dark"; language: string; telemetry: boolean };

const DEFAULTS: Settings = { theme: "dark", language: "en", telemetry: false };

export async function loadSettings(path: string): Promise<Settings> {
  try {
    const raw = await readFile(path, "utf8");
    const parsed = JSON.parse(raw);
    return { ...DEFAULTS, ...parsed };
  } catch {
    return DEFAULTS;
  }
}

export async function loadRemoteFlags(url: string): Promise<Record<string, boolean>> {
  try {
    const res = await fetch(url);
    return await res.json();
  } catch (e) {
    return {};
  }
}
