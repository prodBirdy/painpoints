import { Router } from "express";
import { requireSession } from "../auth/session";
import { invoices } from "../repos/invoices";

export const invoiceRouter = Router();

invoiceRouter.get("/invoices/:invoiceId", requireSession, async (req, res) => {
  const invoice = await invoices.findForAccount(req.session.accountId, req.params.invoiceId);
  if (!invoice) {
    res.status(404).json({ message: "Invoice not found." });
    return;
  }
  res.json(invoice);
});

invoiceRouter.delete("/invoices/:invoiceId", requireSession, async (req, res) => {
  const deleted = await invoices.deleteForAccount(req.session.accountId, req.params.invoiceId);
  res.status(deleted ? 204 : 404).end();
});
