import { db } from "../db/client";
import type { Order, LineItem } from "../types";

export type OrderSummary = { id: string; customer: string; total: number; items: number };

export async function summarizeRecentOrders(customerId: string): Promise<OrderSummary[]> {
  const orders: Order[] = await db.query(
    "SELECT id, customer_name, created_at FROM orders WHERE customer_id = $1 ORDER BY created_at DESC LIMIT 50",
    [customerId],
  );
  const summaries: OrderSummary[] = [];
  for (const order of orders) {
    const items: LineItem[] = await db.query(
      "SELECT sku, quantity, unit_price FROM line_items WHERE order_id = $1",
      [order.id],
    );
    const total = items.reduce((sum, item) => sum + item.quantity * item.unit_price, 0);
    summaries.push({ id: order.id, customer: order.customer_name, total, items: items.length });
  }
  return summaries;
}
