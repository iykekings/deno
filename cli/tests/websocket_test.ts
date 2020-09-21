// Copyright 2018-2020 the Deno authors. All rights reserved. MIT license.
import {
  assert,
  assertEquals,
  assertThrows,
  createResolvable,
  fail,
} from "./unit/test_util.ts";

Deno.test("invalid scheme", () => {
  assertThrows(() => new WebSocket("foo://localhost:4242"));
});

Deno.test("fragment", () => {
  assertThrows(() => new WebSocket("ws://localhost:4242/#"));
  assertThrows(() => new WebSocket("ws://localhost:4242/#foo"));
});

Deno.test("duplicate protocols", () => {
  assertThrows(() => new WebSocket("ws://localhost:4242", ["foo", "foo"]));
});

Deno.test("invalid server", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:2121");
  let err = false;
  ws.onerror = (): void => {
    err = true;
  };
  ws.onclose = (): void => {
    if (err) {
      promise.resolve();
    } else {
      fail();
    }
  };
  ws.onopen = (): void => fail();
  await promise;
});

Deno.test("connect & close", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:4242");
  ws.onerror = (): void => fail();
  ws.onopen = (): void => {
    ws.close();
  };
  ws.onclose = (): void => {
    promise.resolve();
  };
  await promise;
});

Deno.test("connect & abort", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:4242");
  ws.close();
  let err = false;
  ws.onerror = (): void => {
    err = true;
  };
  ws.onclose = (): void => {
    if (err) {
      promise.resolve();
    } else {
      fail();
    }
  };
  ws.onopen = (): void => fail();
  await promise;
});

Deno.test("connect & close custom valid code", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:4242");
  ws.onerror = (): void => fail();
  ws.onopen = (): void => ws.close(1000);
  ws.onclose = (): void => {
    promise.resolve();
  };
  await promise;
});

Deno.test("connect & close custom invalid code", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:4242");
  ws.onerror = (): void => fail();
  ws.onopen = (): void => {
    assertThrows(() => ws.close(1001));
    ws.close();
  };
  ws.onclose = (): void => {
    promise.resolve();
  };
  await promise;
});

Deno.test("connect & close custom valid reason", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:4242");
  ws.onerror = (): void => fail();
  ws.onopen = (): void => ws.close(1000, "foo");
  ws.onclose = (): void => {
    promise.resolve();
  };
  await promise;
});

Deno.test("connect & close custom invalid reason", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:4242");
  ws.onerror = (): void => fail();
  ws.onopen = (): void => {
    assertThrows(() => ws.close(1000, "".padEnd(124, "o")));
    ws.close();
  };
  ws.onclose = (): void => {
    promise.resolve();
  };
  await promise;
});

Deno.test("echo string", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:4242");
  ws.onerror = (): void => fail();
  ws.onopen = (): void => ws.send("foo");
  ws.onmessage = (e): void => {
    assertEquals(e.data, "foo");
    ws.close();
  };
  ws.onclose = (): void => {
    promise.resolve();
  };
  await promise;
});

Deno.test("echo string tls", async () => {
  const promise1 = createResolvable();
  const promise2 = createResolvable();
  const ws = new WebSocket("wss://localhost:4243");
  ws.onerror = (): void => fail();
  ws.onopen = (): void => ws.send("foo");
  ws.onmessage = (e): void => {
    assertEquals(e.data, "foo");
    ws.close();
    promise1.resolve();
  };
  ws.onclose = (): void => {
    promise2.resolve();
  };
  await promise1;
  await promise2;
});

Deno.test("websocket error", async () => {
  const promise1 = createResolvable();
  const ws = new WebSocket("wss://localhost:4242");
  ws.onopen = () => fail();
  ws.onerror = (err): void => {
    assert(err instanceof ErrorEvent);
    assertEquals(err.message, "InvalidData: received corrupt message");
    promise1.resolve();
  };
  await promise1;
});

Deno.test("echo blob with binaryType blob", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:4242");
  const blob = new Blob(["foo"]);
  ws.onerror = (): void => fail();
  ws.onopen = (): void => ws.send(blob);
  ws.onmessage = (e): void => {
    e.data.text().then((actual: string) => {
      blob.text().then((expected) => {
        assertEquals(actual, expected);
      });
    });
    ws.close();
  };
  ws.onclose = (): void => {
    promise.resolve();
  };
  await promise;
});

Deno.test("echo blob with binaryType arraybuffer", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:4242");
  ws.binaryType = "arraybuffer";
  const blob = new Blob(["foo"]);
  ws.onerror = (): void => fail();
  ws.onopen = (): void => ws.send(blob);
  ws.onmessage = (e): void => {
    blob.arrayBuffer().then((expected) => {
      assertEquals(e.data, expected);
    });
    ws.close();
  };
  ws.onclose = (): void => {
    promise.resolve();
  };
  await promise;
});

Deno.test("echo uint8array with binaryType blob", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:4242");
  const uint = new Uint8Array([102, 111, 111]);
  ws.onerror = (): void => fail();
  ws.onopen = (): void => ws.send(uint);
  ws.onmessage = (e): void => {
    e.data.arrayBuffer().then((actual: ArrayBuffer) => {
      assertEquals(actual, uint.buffer);
    });
    ws.close();
  };
  ws.onclose = (): void => {
    promise.resolve();
  };
  await promise;
});

Deno.test("echo uint8array with binaryType arraybuffer", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:4242");
  ws.binaryType = "arraybuffer";
  const uint = new Uint8Array([102, 111, 111]);
  ws.onerror = (): void => fail();
  ws.onopen = (): void => ws.send(uint);
  ws.onmessage = (e): void => {
    assertEquals(e.data, uint.buffer);
    ws.close();
  };
  ws.onclose = (): void => {
    promise.resolve();
  };
  await promise;
});

Deno.test("echo arraybuffer with binaryType blob", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:4242");
  const buffer = new ArrayBuffer(3);
  ws.onerror = (): void => fail();
  ws.onopen = (): void => ws.send(buffer);
  ws.onmessage = (e): void => {
    e.data.arrayBuffer().then((actual: ArrayBuffer) => {
      assertEquals(actual, buffer);
    });
    ws.close();
  };
  ws.onclose = (): void => {
    promise.resolve();
  };
  await promise;
});

Deno.test("echo arraybuffer with binaryType arraybuffer", async () => {
  const promise = createResolvable();
  const ws = new WebSocket("ws://localhost:4242");
  ws.binaryType = "arraybuffer";
  const buffer = new ArrayBuffer(3);
  ws.onerror = (): void => fail();
  ws.onopen = (): void => ws.send(buffer);
  ws.onmessage = (e): void => {
    assertEquals(e.data, buffer);
    ws.close();
  };
  ws.onclose = (): void => {
    promise.resolve();
  };
  await promise;
});
