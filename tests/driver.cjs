// The host side of the test suite: loads the -sMODULARIZE test_module
// build, wraps its t_* exports with WebAssembly.promising, and drives
// genuine host-side reentrancy — overlapping parked entries, non-LIFO and
// same-tick wakes, plain-call discipline, exclusive ownership, escaped
// panics as rejections, and cross-instance interleaving.
//
// Modes:
//   node tests/driver.cjs <module.js>            full suite (JSPI build)
//   node tests/driver.cjs --no-jspi <module.js>  plain subset, no suspension
//   node tests/driver.cjs --proof <module.js>    corruption scenario only:
//     run against a --cfg jspi_disable_virtualization build under shell
//     inversion — it must NOT pass cleanly.
//
// Wasm status convention: 0 ok, positive = wasm-side assertion site,
// negative = jspi::EnterError (-1 Nested, -2 Exclusive, -3 Parked).
'use strict';
const path = require('path');
const assert = require('assert');

const args = process.argv.slice(2);
const mode = args.includes('--no-jspi') ? 'no-jspi' : args.includes('--proof') ? 'proof' : 'full';
const factory = require(path.resolve(args.filter((a) => !a.startsWith('--'))[0]));

const tick = (ms = 5) => new Promise((r) => setTimeout(r, ms));

const IN_PROMISING = 1, EXCLUSIVE = 2, ENABLED = 4;
const ERR_NESTED = -1, ERR_EXCLUSIVE = -2, ERR_PARKED = -3;

async function instantiate(tag) {
  const __test = { np: 1, promises: new Map(), results: new Map(), errors: new Map() };
  const M = await factory({
    __test,
    print: (s) => console.log(tag, s),
    printErr: (s) => console.error(tag, s),
  });
  const plain = M.wasmExports;
  assert.ok(plain && plain.t_probe, `${tag}: wasmExports missing (needs -sEXPORTED_RUNTIME_METHODS=wasmExports)`);
  const p = {};
  if (mode !== 'no-jspi') {
    for (const name of Object.keys(plain)) {
      if (name.startsWith('t_')) p[name] = WebAssembly.promising(plain[name]);
    }
  }
  return {
    tag,
    plain,
    p,
    promise() {
      const pid = __test.np++;
      let resolve, reject;
      __test.promises.set(pid, new Promise((res, rej) => { resolve = res; reject = rej; }));
      return { pid, resolve, reject };
    },
    // a second pid waking from the same resolution in the same microtask drain
    share(pid) {
      const pid2 = __test.np++;
      __test.promises.set(pid2, __test.promises.get(pid));
      return pid2;
    },
  };
}

const S = {};

S.probe = async (I) => {
  assert.equal(I.plain.t_probe(), ENABLED, 'probe: expected enabled, not in/exclusive promising');
};

S.basic = async (I) => {
  assert.equal(await I.p.t_sleep_check(10), 0, 'basic: sleep_check');
};

// A plain host call is not promising, even while a sibling is parked; a
// blocking_call from it is denied as a catchable panic. A plain enter is
// permitted (parked siblings are invisible to the parity counter).
S.plain_discipline = async (I) => {
  assert.equal(I.plain.t_plain_denial(), 0, 'plain: denial from quiet state');
  const w = I.promise();
  const parked = I.p.t_park(w.pid, 4, 0x11);
  await tick();
  assert.equal(I.plain.t_plain_denial(), 0, 'plain: denial while sibling parked');
  assert.equal(I.plain.t_probe(), ENABLED, 'plain: parked sibling invisible to in_promising');
  assert.equal(I.plain.t_enter_check(), 0, 'plain: enter Ok while sibling parked');
  w.resolve();
  assert.equal(await parked, 0, 'plain: parked sibling healed');
};

S.nested_denial = async (I) => {
  assert.equal(await I.p.t_nested(), 0, 'nested: Err(Nested) inside open activation');
};

S.reentrant_bracket_denial = async (I) => {
  assert.equal(await I.p.t_reentrant(), 0, 'reentrant: nested bracket denied, frames intact');
};

S.noop_bracket = async (I) => {
  assert.equal(await I.p.t_noop_bracket(), 0, 'noop: non-suspending bracket degradation');
};

// True host-driven reentrancy: the same export re-entered while parked.
S.same_export_reentry = async (I) => {
  const p1 = I.promise(), p2 = I.promise();
  const a = I.p.t_park(p1.pid, 5, 0x66);
  await tick();
  const b = I.p.t_park(p2.pid, 5, 0x99);
  await tick();
  p1.resolve();
  assert.equal(await a, 0, 'reentry: first activation healed');
  p2.resolve();
  assert.equal(await b, 0, 'reentry: second activation healed');
};

// X parks deep, Y parks deep after it, wakes and completes first; Z then
// re-occupies and scribbles the same region; X wakes last and asserts
// every frame on unwind. Wake order is unrelated to park order.
S.non_lifo = async (I) => {
  const px = I.promise(), py = I.promise(), pz = I.promise();
  const x = I.p.t_park(px.pid, 8, 0xC3);
  const y = I.p.t_park(py.pid, 8, 0x35);
  await tick();
  py.resolve();
  assert.equal(await y, 0, 'non-lifo: Y healed');
  const z = I.p.t_park(pz.pid, 12, 0x77);
  await tick();
  pz.resolve();
  assert.equal(await z, 0, 'non-lifo: Z healed');
  px.resolve();
  assert.equal(await x, 0, 'non-lifo: X healed after siblings scribbled its region');
};

// Two parked brackets waking from one resolution in the same microtask
// drain: restore ordering across one engine resume tick.
S.same_tick_double_wake = async (I) => {
  const p = I.promise();
  const pid2 = I.share(p.pid);
  const a = I.p.t_park(p.pid, 3, 0xA1);
  const b = I.p.t_park(pid2, 3, 0xB2);
  await tick();
  p.resolve();
  assert.equal(await a, 0, 'same-tick: first healed');
  assert.equal(await b, 0, 'same-tick: second healed');
};

S.sequential_brackets = async (I) => {
  assert.equal(await I.p.t_sequential(50), 0, 'sequential: 50 brackets');
};

// Memory growth while parked: the restore must use fresh heap views.
S.growth = async (I) => {
  const p = I.promise();
  const a = I.p.t_park(p.pid, 4, 0x7E);
  await tick();
  assert.equal(I.plain.t_grow(128), 0, 'growth: alloc');
  p.resolve();
  assert.equal(await a, 0, 'growth: parked activation healed across growth');
};

// Values resolve through the foreign call and are fetched by plain imports
// post-restore; rejections are recorded, returned normally, and parking
// still works after.
S.value_fetch = async (I) => {
  const p = I.promise();
  const a = I.p.t_value(p.pid);
  await tick();
  p.resolve(42);
  assert.equal(await a, 42, 'value: resolved value fetched post-restore');
  const r = I.promise();
  const b = I.p.t_reject(r.pid);
  await tick();
  r.reject(new Error('rejected by test'));
  assert.equal(await b, 0, 'value: rejection recorded, activation survives');
};

// Caught panics: unwinds run drops against restored slices; a drop may
// itself park while the panic is in flight.
S.caught_panics = async (I) => {
  const p = I.promise();
  const a = I.p.t_caught_panic(p.pid);
  await tick();
  p.resolve();
  assert.equal(await a, 0, 'caught: drop saw healed frame, activation re-parked');
  assert.equal(await I.p.t_park_during_unwind(), 0, 'caught: park during unwind');
};

// An escaped panic crosses the promising boundary as a rejection; the
// module stays fully operational.
S.escaped_panic = async (I) => {
  const p = I.promise();
  const a = I.p.t_panic(p.pid, 0);
  await tick();
  p.resolve();
  await assert.rejects(a, undefined, 'escaped: expected rejection');
  assert.equal(await I.p.t_sleep_check(1), 0, 'escaped: module alive after rejection');
};

S.exclusive = async (I) => {
  // the owner holds the lock across its park; every other enter is denied
  const p = I.promise();
  const ex = I.p.t_exclusive(p.pid);
  await tick();
  assert.equal(I.plain.t_probe(), ENABLED | EXCLUSIVE, 'exclusive: bit visible while owner parked');
  assert.equal(I.plain.t_enter_check(), ERR_EXCLUSIVE, 'exclusive: plain enter denied');
  assert.equal(await I.p.t_enter_check(), ERR_EXCLUSIVE, 'exclusive: promising enter denied');
  assert.equal(I.plain.t_exclusive_check(), ERR_EXCLUSIVE, 'exclusive: second exclusive denied');
  p.resolve();
  assert.equal(await ex, 0, 'exclusive: owner healed, saw lock across park');
  assert.equal(I.plain.t_probe(), ENABLED, 'exclusive: released on completion');
  assert.equal(I.plain.t_enter_check(), 0, 'exclusive: enters permitted again');

  // granted only from quiescence: a parked sibling denies with Parked
  const q = I.promise();
  const parked = I.p.t_park(q.pid, 2, 0x3C);
  await tick();
  assert.equal(I.plain.t_exclusive_check(), ERR_PARKED, 'exclusive: denied while sibling parked');
  q.resolve();
  assert.equal(await parked, 0);
  assert.equal(I.plain.t_exclusive_check(), 0, 'exclusive: Ok from quiescence');

  // an unwinding owner releases the lock
  const r = I.promise();
  const panicking = I.p.t_panic(r.pid, 1);
  await tick();
  assert.equal(I.plain.t_probe(), ENABLED | EXCLUSIVE, 'exclusive: held by panicking owner');
  r.resolve();
  await assert.rejects(panicking, undefined, 'exclusive: owner rejection');
  assert.equal(I.plain.t_probe(), ENABLED, 'exclusive: released by unwind');
  assert.equal(await I.p.t_sleep_check(1), 0, 'exclusive: module alive after owner unwind');
};

// Hood Chatham's reentrancy corruption reproduction: A parks briefly with a
// small frame; V parks below it with a recognizable buffer; A resumes and
// completes (epilogues walk SP back above V); a plain call scribbles a
// large frame down through V's live range; V resumes and asserts its
// frame. The proof lane runs exactly this against a
// --cfg jspi_disable_virtualization build, where it must fail.
S.corruption = async (I) => {
  const above = I.promise(), victim = I.promise();
  const a = I.p.t_park(above.pid, 0, 0x44);
  await tick();
  const v = I.p.t_park(victim.pid, 0, 0x55);
  await tick();
  above.resolve();
  assert.equal(await a, 0, 'corruption: upper activation healed');
  assert.equal(I.plain.t_scribble(), 0);
  victim.resolve();
  assert.equal(await v, 0, 'corruption: victim stack corrupted');
};

// Two instances in one JS context: per-instance registries, interleaved
// suspensions must not cross-wire.
async function twoInstance() {
  const A = await instantiate('A'), B = await instantiate('B');
  const pa = A.promise(), pb = B.promise();
  const a = A.p.t_park(pa.pid, 6, 0xAA);
  const b = B.p.t_park(pb.pid, 6, 0xBB);
  await tick();
  assert.equal(await B.p.t_sleep_check(5), 0, 'two-instance: B suspends while A parked');
  assert.equal(await A.p.t_sleep_check(5), 0, 'two-instance: A suspends while B parked');
  pb.resolve();
  assert.equal(await b, 0, 'two-instance: B healed');
  pa.resolve();
  assert.equal(await a, 0, 'two-instance: A healed');
}

async function noJspi(I) {
  assert.equal(I.plain.t_probe(), 0, 'no-jspi: nothing enabled');
  assert.equal(I.plain.t_enter_check(), 0, 'no-jspi: enter works');
  assert.equal(I.plain.t_exclusive_check(), 0, 'no-jspi: exclusive enter works');
  assert.equal(I.plain.t_noop_bracket(), 0, 'no-jspi: bracket degrades to no-op');
  assert.equal(I.plain.t_plain_denial(), 0, 'no-jspi: misplaced bracket still denied');
}

async function main() {
  const watchdog = setTimeout(() => {
    console.error('driver: timeout');
    process.exit(1);
  }, 60000);

  if (mode === 'no-jspi') {
    await noJspi(await instantiate('M'));
    console.log('driver: no-jspi ok');
  } else if (mode === 'proof') {
    await S.corruption(await instantiate('M'));
    console.log('driver: corruption scenario passed (proof lane expects this NOT to happen)');
  } else {
    const I = await instantiate('M');
    for (const [name, fn] of Object.entries(S)) {
      await fn(I);
      console.log(`driver: ${name} ok`);
    }
    await twoInstance();
    console.log('driver: two_instance ok');
    console.log('driver: all tests passed');
  }
  clearTimeout(watchdog);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
