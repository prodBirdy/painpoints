import { db } from "../db/client";

export type OrderSummary = { id: string; customer: string; total: number; items: number };

export async function summarizeRecentOrders(customerId: string): Promise<OrderSummary[]> {
  return db.query(
    `SELECT o.id, o.customer_name AS customer,
            COALESCE(SUM(li.quantity * li.unit_price), 0) AS total,
            COUNT(li.sku) AS items
       FROM orders o
       LEFT JOIN line_items li ON li.order_id = o.id
      WHERE o.customer_id = $1
      GROUP BY o.id
      ORDER BY o.created_at DESC
      LIMIT 50`,
    [customerId],
  );
}
