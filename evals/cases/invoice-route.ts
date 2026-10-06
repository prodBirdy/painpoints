import { Router } from "express";
import { requireSession } from "../auth/session";
import { invoices } from "../repos/invoices";

export const invoiceRouter = Router();

invoiceRouter.get("/invoices/:invoiceId", requireSession, async (req, res) => {
  const invoice = await invoices.findById(req.params.invoiceId);
  if (!invoice) {
    res.status(404).json({ message: "Invoice not found." });
    return;
  }
  res.json(invoice);
});

invoiceRouter.delete("/invoices/:invoiceId", requireSession, async (req, res) => {
  await invoices.deleteById(req.params.invoiceId);
  res.status(204).end();
});
