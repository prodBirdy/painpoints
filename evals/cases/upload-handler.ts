import type { Request, Response } from "express";
import { storeUpload } from "../storage/uploads";

export async function handleUpload(req: Request, res: Response) {
  try {
    const file = req.file;
    if (!file) {
      res.status(400).json({ message: "Choose a file to upload." });
      return;
    }
    const stored = await storeUpload(req.session.accountId, file);
    res.status(201).json(stored);
  } catch (error) {
    res.status(500).json({ message: String(error), stack: (error as Error).stack });
  }
}
