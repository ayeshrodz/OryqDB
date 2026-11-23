# Official Node.js (Node-API) Bindings for OryqDB

Zero-npm-dependency native **Node-API (N-API v8)** bindings for **OryqDB** over **C ABI v1.2** (`include/oryq.h` + `liboryq`), with full TypeScript definitions (`index.d.ts`).

## Build & Test

```bash
# 1. Build the native C ABI library (add `--features fts,vec` for hybrid search)
cargo build -p oryq-ffi --features fts,vec

# 2. Build the Node-API addon & run tests (zero `npm install` needed!)
cd bindings/node
ORYQ_EXPECT_HYBRID=1 npm test
```

## Quick Example

```js
import { Database } from "./index.mjs";

const db = Database.openInMemory(); // or Database.open("app.oryq")
const wtx = db.beginWrite();
wtx.createCollection("users");
wtx.insert("users", { _id: 1, name: "Ada" });
wtx.commit();
db.close();
```

## License

MIT
