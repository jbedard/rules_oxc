import assert from "node:assert/strict";

async function fetchOne(): Promise<number> {
  return 1;
}

let resolved = false;
const pending = fetchOne();
assert.equal(typeof pending.then, "function");
pending.then((value) => {
  assert.equal(value, 1);
  resolved = true;
});
process.on("exit", () => assert.ok(resolved, "promise never resolved"));
