import sqlite3
from flask import Flask, request, jsonify

app = Flask(__name__)


def get_conn():
    return sqlite3.connect("app.db")


@app.get("/users/search")
def search_users():
    name = request.args.get("name", "")
    conn = get_conn()
    rows = conn.execute(
        f"SELECT id, name, email FROM users WHERE name LIKE '%{name}%' LIMIT 20"
    ).fetchall()
    return jsonify([{"id": r[0], "name": r[1], "email": r[2]} for r in rows])
