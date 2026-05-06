import { Buffer } from "node:buffer";
import { DatabaseSync } from "node:sqlite";

import { workdir } from "./host-ops.mjs";

const { AbstractLevelDOWN } = require("abstract-leveldown");
const AbstractIterator = require("abstract-leveldown/abstract-iterator");

const DB_PATH = `${workdir}/artifacts/railgun.db`;

export class SqliteLevelDOWN extends AbstractLevelDOWN {
  constructor() {
    super({
      bufferKeys: false,
      promises: false,
      snapshots: false,
      permanence: true,
      clear: true,
      createIfMissing: true,
      errorIfExists: false,
      seek: false,
      streams: true,
      encodings: {
        buffer: true,
        utf8: true,
        json: true,
      },
    });
    this.db = null;
    this.statements = null;
  }

  _open(_options, callback) {
    try {
      Deno.mkdirSync(`${workdir}/artifacts`, { recursive: true });
      this.db = new DatabaseSync(DB_PATH);
      this.db.exec(`
        CREATE TABLE IF NOT EXISTS kv (
          key TEXT PRIMARY KEY,
          value BLOB NOT NULL
        ) STRICT
      `);
      this.statements = {
        get: this.db.prepare("SELECT value FROM kv WHERE key = ?"),
        put: this.db.prepare(
          "INSERT INTO kv(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        ),
        del: this.db.prepare("DELETE FROM kv WHERE key = ?"),
      };
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _close(callback) {
    try {
      this.statements = null;
      this.db?.close();
      this.db = null;
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _get(key, options, callback) {
    try {
      const row = this.statement("get").get(String(key));
      if (row == null) {
        callback(notFound(key));
        return;
      }
      callback(null, decodeDatabaseValue(row.value, options));
    } catch (error) {
      callback(error);
    }
  }

  _put(key, value, _options, callback) {
    try {
      this.statement("put").run(String(key), Buffer.from(value));
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _del(key, _options, callback) {
    try {
      this.statement("del").run(String(key));
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _batch(ops, _options, callback) {
    try {
      this.db.exec("BEGIN IMMEDIATE");
      try {
        for (const op of ops) {
          if (op.type === "put") {
            this.statement("put").run(String(op.key), Buffer.from(op.value));
          } else if (op.type === "del") {
            this.statement("del").run(String(op.key));
          }
        }
        this.db.exec("COMMIT");
      } catch (error) {
        this.db.exec("ROLLBACK");
        throw error;
      }
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _clear(options, callback) {
    try {
      const { where, params } = rangeWhere(options);
      this.db.prepare(`DELETE FROM kv${where}`).run(...params);
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _iterator(options) {
    return new SqliteIterator(this, options);
  }

  statement(name) {
    if (this.statements == null) {
      throw new Error("Railgun SQLite database is not open");
    }
    return this.statements[name];
  }

  all(options) {
    const { where, params } = rangeWhere(options);
    const order = options.reverse ? "DESC" : "ASC";
    const limit = typeof options.limit === "number" && options.limit >= 0
      ? ` LIMIT ${options.limit}`
      : "";
    return this.db.prepare(
      `SELECT key, value FROM kv${where} ORDER BY key ${order}${limit}`,
    ).all(...params);
  }
}

class SqliteIterator extends AbstractIterator {
  constructor(db, options) {
    super(db);
    this.options = options;
    this.entries = db.all(options);
    this.index = 0;
  }

  _next(callback) {
    try {
      if (this.index >= this.entries.length) {
        callback();
        return;
      }
      const row = this.entries[this.index++];
      callback(
        null,
        this.options.keys
          ? encodeIteratorKey(row.key, this.options)
          : undefined,
        this.options.values
          ? decodeDatabaseValue(row.value, this.options)
          : undefined,
      );
    } catch (error) {
      callback(error);
    }
  }

  _end(callback) {
    this.entries = [];
    callback();
  }
}

function decodeDatabaseValue(value, options) {
  const bytes = Buffer.from(value);
  return options.asBuffer === false ? bytes.toString("utf8") : bytes;
}

function encodeIteratorKey(key, options) {
  return options.keyAsBuffer === false ? key : Buffer.from(key);
}

function rangeWhere(options) {
  const clauses = [];
  const params = [];
  for (
    const [operator, sql] of [
      ["gt", "key > ?"],
      ["gte", "key >= ?"],
      ["lt", "key < ?"],
      ["lte", "key <= ?"],
    ]
  ) {
    if (options[operator] != null) {
      clauses.push(sql);
      params.push(String(options[operator]));
    }
  }
  return {
    where: clauses.length === 0 ? "" : ` WHERE ${clauses.join(" AND ")}`,
    params,
  };
}

function notFound(key) {
  const error = new Error(`Key not found in database [${String(key)}]`);
  error.notFound = true;
  error.status = 404;
  error.code = "LEVEL_NOT_FOUND";
  return error;
}
