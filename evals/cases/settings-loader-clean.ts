import { readFile } from "node:fs/promises";

export type Settings = { theme: "light" | "dark"; language: string; telemetry: boolean };

const DEFAULTS: Settings = { theme: "dark", language: "en", telemetry: false };

export class SettingsError extends Error {}

export async function loadSettings(path: string): Promise<Settings> {
  let raw: string;
  try {
    raw = await readFile(path, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return DEFAULTS;
    throw new SettingsError(`could not read settings at ${path}`, { cause: err });
  }
  try {
    return { ...DEFAULTS, ...JSON.parse(raw) };
  } catch (err) {
    throw new SettingsError(`settings at ${path} are not valid JSON`, { cause: err });
  }
}
