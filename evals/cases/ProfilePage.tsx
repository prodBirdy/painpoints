import { useEffect, useState } from "react";
import { pool } from "../server/db";

type Profile = { id: string; name: string; plan: string; seats: number };

export function ProfilePage({ userId }: { userId: string }) {
  const [profile, setProfile] = useState<Profile | null>(null);

  useEffect(() => {
    pool
      .query("SELECT id, name, plan, seats FROM accounts WHERE owner_id = $1", [userId])
      .then((r) => setProfile(r.rows[0]));
  }, [userId]);

  if (!profile) return <p>Loading…</p>;
  const price = profile.plan === "team" ? profile.seats * 12 * (profile.seats > 20 ? 0.8 : 1) : 9;
  return (
    <section>
      <h1>{profile.name}</h1>
      <p>Plan: {profile.plan}</p>
      <p>Monthly price: ${price.toFixed(2)}</p>
    </section>
  );
}
