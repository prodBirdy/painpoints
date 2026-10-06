export type Charge = { id: string; amount: number; currency: string; status: string };

const BASE = "https://api.payments.example.com/v1";

export async function createCharge(token: string, amount: number, currency: string): Promise<Charge> {
  let lastError: unknown;
  for (let attempt = 0; attempt < 10; attempt++) {
    try {
      const res = await fetch(`${BASE}/charges`, {
        method: "POST",
        headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
        body: JSON.stringify({ amount, currency }),
      });
      if (!res.ok) throw new Error(`status ${res.status}`);
      return (await res.json()) as Charge;
    } catch (err) {
      lastError = err;
    }
  }
  throw lastError;
}
