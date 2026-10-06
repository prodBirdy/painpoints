import { db } from "../db/client";

export async function exportAllEvents(): Promise<string> {
  const rows = await db.query("SELECT * FROM events ORDER BY created_at");
  const header = Object.keys(rows[0] ?? {}).join(",");
  const lines = rows.map((row: Record<string, unknown>) => Object.values(row).join(","));
  return [header, ...lines].join("\n");
}
